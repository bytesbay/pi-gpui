//! Child processes that speak line-delimited JSON over stdio: pi in RPC mode
//! and app plugins. Blocking I/O runs on dedicated threads; the UI receives
//! [`JsonlMessage`] values through a bounded channel.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use futures::SinkExt as _;
use futures::channel::mpsc;
use serde_json::Value;

/// Capacity of the stdout → UI queue. When the UI falls behind, the reader
/// thread blocks, the pipe fills, and the child slows down instead of the app
/// buffering without limit.
const EVENT_QUEUE_CAPACITY: usize = 1024;
const STDERR_TAIL_LINES: usize = 40;
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

/// What the reader threads deliver to the UI.
pub enum JsonlMessage {
    Record(Value),
    /// A stdout line that was not valid JSON. Kept visible instead of dropped.
    Malformed(String),
    Exited {
        status: Option<ExitStatus>,
        stderr_tail: String,
    },
}

/// A child process that speaks line-delimited JSON on stdin/stdout. Dropping
/// it closes stdin, which asks the child to shut down cleanly; a watchdog kills
/// it if it does not exit in time.
pub struct JsonlProcess {
    commands: Option<std_mpsc::Sender<String>>,
    child: Arc<Mutex<Child>>,
}

impl JsonlProcess {
    /// Spawns `command` with piped stdio and starts the reader/writer threads.
    pub fn spawn(command: &mut Command) -> std::io::Result<(Self, mpsc::Receiver<JsonlMessage>)> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stdin = child.stdin.take().ok_or_else(missing_pipe)?;
        let stdout = child.stdout.take().ok_or_else(missing_pipe)?;
        let stderr = child.stderr.take().ok_or_else(missing_pipe)?;
        let child = Arc::new(Mutex::new(child));
        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));

        let (commands, command_receiver) = std_mpsc::channel::<String>();
        thread::spawn(move || write_commands(stdin, command_receiver));

        thread::spawn({
            let stderr_tail = stderr_tail.clone();
            move || collect_stderr(stderr, stderr_tail)
        });

        let (sender, receiver) = mpsc::channel(EVENT_QUEUE_CAPACITY);
        thread::spawn({
            let child = child.clone();
            move || read_records(stdout, sender, child, stderr_tail)
        });

        Ok((
            Self {
                commands: Some(commands),
                child,
            },
            receiver,
        ))
    }

    /// Queues one command. Writing happens on a dedicated thread, so a full
    /// pipe never blocks the UI.
    pub fn send(&self, command: Value) -> Result<(), String> {
        let line = command.to_string();
        self.commands
            .as_ref()
            .ok_or_else(|| "the process has stopped".to_string())?
            .send(line)
            .map_err(|_| "the process does not accept input".to_string())
    }
}

impl Drop for JsonlProcess {
    fn drop(&mut self) {
        self.commands.take();
        let child = self.child.clone();
        thread::spawn(move || {
            let deadline = Instant::now() + SHUTDOWN_GRACE;
            while Instant::now() < deadline {
                match child.lock().map(|mut child| child.try_wait()) {
                    Ok(Ok(Some(_))) | Err(_) => return,
                    _ => thread::sleep(Duration::from_millis(50)),
                }
            }
            if let Ok(mut child) = child.lock()
                && let Err(error) = child.kill()
            {
                eprintln!("pi-gpui: failed to stop a child process: {error}");
            }
        });
    }
}

fn missing_pipe() -> std::io::Error {
    std::io::Error::other("pipe was not created")
}

fn write_commands(mut stdin: ChildStdin, commands: std_mpsc::Receiver<String>) {
    for line in commands {
        let written = stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush());
        if let Err(error) = written {
            eprintln!("pi-gpui: writing to a child process failed: {error}");
            return;
        }
    }
    // Sender dropped: returning drops stdin, which is pi's shutdown signal.
}

fn collect_stderr(stderr: impl Read, tail: Arc<Mutex<VecDeque<String>>>) {
    for line in BufReader::new(stderr).lines() {
        let Ok(line) = line else { return };
        if let Ok(mut tail) = tail.lock() {
            if tail.len() == STDERR_TAIL_LINES {
                tail.pop_front();
            }
            tail.push_back(line);
        }
    }
}

/// Splits stdout strictly on LF, as pi's protocol (and ours) requires. `lines()` would
/// be fine too, but byte splitting keeps invalid UTF-8 from ending the stream.
fn read_records(
    stdout: impl Read,
    mut sender: mpsc::Sender<JsonlMessage>,
    child: Arc<Mutex<Child>>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
) {
    let mut reader = BufReader::new(stdout);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) => {
                eprintln!("pi-gpui: reading child stdout failed: {error}");
                break;
            }
        }
        while matches!(buffer.last(), Some(b'\n' | b'\r')) {
            buffer.pop();
        }
        if buffer.is_empty() {
            continue;
        }
        let message = match serde_json::from_slice::<Value>(&buffer) {
            Ok(value) => JsonlMessage::Record(value),
            Err(_) => JsonlMessage::Malformed(String::from_utf8_lossy(&buffer).into_owned()),
        };
        // Blocking here is the backpressure path: the UI is behind, so stop reading.
        if futures::executor::block_on(sender.send(message)).is_err() {
            return;
        }
    }

    let status = child.lock().ok().and_then(|mut child| child.wait().ok());
    // Give the stderr thread a moment to drain the final lines.
    thread::sleep(Duration::from_millis(100));
    let stderr_tail = stderr_tail
        .lock()
        .map(|tail| tail.iter().cloned().collect::<Vec<_>>().join("\n"))
        .unwrap_or_default();
    futures::executor::block_on(sender.send(JsonlMessage::Exited {
        status,
        stderr_tail,
    }))
    .ok();
}
