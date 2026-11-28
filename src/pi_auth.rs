//! Logging pi in to model providers with pi's own `pi auth` commands.
//!
//! `pi auth login` needs a terminal, so it runs in a pseudo-terminal: the app
//! shows what pi prints and sends what the user types. pi does the sign-in
//! itself (it opens the browser and saves the credential in its auth.json);
//! the app never sees a token, only an API key the user types for pi.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::channel::mpsc;
use serde_json::Value;

use crate::pi::{self, Installation};

/// Not a provider: asking pi to log in to it makes pi list the real ones.
const NO_PROVIDER: &str = "__pi-gpui__";

#[derive(Clone, Debug, PartialEq)]
pub enum AuthState {
    /// Ready, and how: `oauth` (an account), `api_key`, or what pi reports.
    Ready(String),
    NotReady,
    Invalid,
}

#[derive(Clone, Debug)]
pub struct ProviderStatus {
    pub id: String,
    pub state: AuthState,
    /// pi stored a credential for it (`pi auth logout` can remove it).
    pub stored: bool,
}

fn command(installation: &Installation) -> Command {
    let mut command = Command::new(&installation.executable);
    command.env(
        "PATH",
        pi::path_with_executable_dir(&installation.executable),
    );
    command
}

/// The providers pi can log in to, with their state. Blocking: runs pi
/// once to list them and once per provider to check it, a few at a time.
pub fn providers(installation: &Installation) -> Result<Vec<ProviderStatus>, String> {
    let listed = command(installation)
        .args(["auth", "login", NO_PROVIDER])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("could not run pi: {error}"))?;
    let ids = list_after(&String::from_utf8_lossy(&listed.stderr), "Providers:");
    if ids.is_empty() {
        return Err("pi did not list its providers".to_string());
    }
    let stored = command(installation)
        .args(["auth", "logout", NO_PROVIDER])
        .stdin(Stdio::null())
        .output()
        .map(|output| {
            list_after(
                &String::from_utf8_lossy(&output.stderr),
                "Stored credentials:",
            )
        })
        .unwrap_or_default();

    let states: Vec<Mutex<Option<AuthState>>> = ids.iter().map(|_| Mutex::new(None)).collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(id) = ids.get(index) else {
                        break;
                    };
                    *states[index].lock().unwrap() = Some(check(installation, id));
                }
            });
        }
    });
    Ok(ids
        .iter()
        .zip(states)
        .map(|(id, state)| ProviderStatus {
            stored: stored.contains(id),
            id: id.clone(),
            state: state.into_inner().unwrap().unwrap_or(AuthState::Invalid),
        })
        .collect())
}

fn check(installation: &Installation, provider: &str) -> AuthState {
    let Ok(output) = command(installation)
        .args([
            "auth",
            "check",
            "--provider",
            provider,
            "--json",
            "--no-refresh",
        ])
        .stdin(Stdio::null())
        .output()
    else {
        return AuthState::Invalid;
    };
    let result: Value = serde_json::from_slice(&output.stdout).unwrap_or_default();
    match result["status"].as_str() {
        Some("ready") => AuthState::Ready(result["authType"].as_str().unwrap_or("").to_string()),
        Some("not_ready") => AuthState::NotReady,
        _ => AuthState::Invalid,
    }
}

/// `Providers: a, b, c` → `[a, b, c]`.
fn list_after(text: &str, prefix: &str) -> Vec<String> {
    text.lines()
        .find_map(|line| line.trim().strip_prefix(prefix))
        .map(|list| {
            list.split(',')
                .map(str::trim)
                .filter(|id| !id.is_empty() && *id != "none")
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// `pi auth logout <provider>`. Blocking.
pub fn logout(installation: &Installation, provider: &str) -> Result<(), String> {
    let output = command(installation)
        .args(["auth", "logout", provider])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("could not run pi: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(last_line(&String::from_utf8_lossy(&output.stderr))
            .unwrap_or_else(|| "pi auth logout failed".to_string()))
    }
}

fn last_line(text: &str) -> Option<String> {
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

pub enum LoginEvent {
    Output(Vec<u8>),
    Exited(Option<i32>),
}

/// A running `pi auth login`.
pub struct LoginProcess {
    input: File,
    child: Arc<Mutex<Child>>,
}

impl LoginProcess {
    /// Starts `pi auth login <provider>` (`--api-key` to save a key instead
    /// of signing in with an account) in a pseudo-terminal.
    pub fn start(
        installation: &Installation,
        provider: &str,
        api_key: bool,
    ) -> std::io::Result<(Self, mpsc::UnboundedReceiver<LoginEvent>)> {
        let (master, slave) = open_pty()?;
        let mut command = command(installation);
        command.args(["auth", "login", provider]);
        if api_key {
            command.arg("--api-key");
        }
        command
            .env("TERM", "xterm-256color")
            .env("NO_COLOR", "1")
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave));
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = Arc::new(Mutex::new(command.spawn()?));
        // The parent's copies of the slave are closed now (dropped with
        // `command`), so reading the master ends when pi exits.
        drop(command);
        let master = File::from(master);
        let input = master.try_clone()?;
        let (sender, receiver) = mpsc::unbounded();
        let waiter = child.clone();
        std::thread::spawn(move || {
            let mut master = master;
            let mut buffer = [0u8; 4096];
            loop {
                match master.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if sender
                            .unbounded_send(LoginEvent::Output(buffer[..read].to_vec()))
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            let code = loop {
                match waiter.lock().unwrap().try_wait() {
                    Ok(Some(status)) => break status.code(),
                    Ok(None) => {}
                    Err(_) => break None,
                }
                std::thread::sleep(Duration::from_millis(50));
            };
            sender.unbounded_send(LoginEvent::Exited(code)).ok();
        });
        Ok((Self { input, child }, receiver))
    }

    /// Answers pi's question: the text and Enter.
    pub fn answer(&mut self, text: &str) -> std::io::Result<()> {
        let line: String = text
            .chars()
            .filter(|character| !character.is_control())
            .collect();
        self.input.write_all(line.as_bytes())?;
        self.input.write_all(b"\r")?;
        self.input.flush()
    }

    /// Ctrl+C, as in a terminal; the process is killed if it does not stop.
    pub fn cancel(&mut self) {
        self.input.write_all(&[3]).ok();
        let child = self.child.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(2));
            let mut child = child.lock().unwrap();
            if matches!(child.try_wait(), Ok(None)) {
                child.kill().ok();
            }
        });
    }
}

impl Drop for LoginProcess {
    fn drop(&mut self) {
        let mut child = self.child.lock().unwrap();
        if matches!(child.try_wait(), Ok(None)) {
            child.kill().ok();
        }
    }
}

/// A pseudo-terminal wide enough that pi's lines do not wrap.
fn open_pty() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut master = -1;
    let mut slave = -1;
    let size = libc::winsize {
        ws_row: 40,
        ws_col: 200,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: openpty writes two descriptors we then own.
    let result = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &size as *const libc::winsize as *mut libc::winsize,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: both descriptors are open and owned by nobody else.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    // The master must not leak into pi.
    // SAFETY: setting FD_CLOEXEC on a descriptor we own.
    unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
    Ok((master, slave))
}

/// What pi printed, as lines: escape sequences removed, a carriage return
/// or "move to column 1" starts the line over, as in a terminal.
#[derive(Default)]
pub struct Transcript {
    pub lines: Vec<String>,
    /// The line pi is writing; a question while pi waits for an answer.
    pub current: String,
    /// The next character replaces `current` (after `\r`).
    restart: bool,
    /// Drop pi's echo of the answer until the line ends.
    swallow: bool,
    escape: Vec<u8>,
    pending: Vec<u8>,
}

impl Transcript {
    pub fn feed(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
        // Keep an incomplete UTF-8 sequence for the next chunk.
        let valid = match std::str::from_utf8(&self.pending) {
            Ok(text) => text.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => self.pending.len(),
        };
        let text = String::from_utf8_lossy(&self.pending[..valid]).into_owned();
        self.pending.drain(..valid);
        for character in text.chars() {
            self.push_char(character);
        }
    }

    fn push_char(&mut self, character: char) {
        if !self.escape.is_empty() {
            self.escape.extend(character.to_string().as_bytes());
            let sequence = &self.escape;
            let done = match sequence.get(1) {
                Some(b'[') => {
                    sequence.len() > 2 && (0x40..=0x7e).contains(sequence.last().unwrap())
                }
                // OSC: until BEL or ESC \.
                Some(b']') => sequence.ends_with(&[7]) || sequence.ends_with(b"\x1b\\"),
                Some(_) => true,
                None => false,
            };
            if done {
                let sequence = std::mem::take(&mut self.escape);
                if sequence.get(1) == Some(&b'[') {
                    let last = *sequence.last().unwrap();
                    let parameters =
                        String::from_utf8_lossy(&sequence[2..sequence.len() - 1]).to_string();
                    let first = parameters.split(';').next().unwrap_or("");
                    match last {
                        b'G' if first.is_empty() || first == "1" => self.restart = true,
                        b'K' | b'J' if self.restart || first == "2" => {
                            self.current.clear();
                            self.restart = false;
                        }
                        _ => {}
                    }
                }
            }
            return;
        }
        match character {
            '\u{1b}' => self.escape.push(0x1b),
            '\r' => self.restart = true,
            '\n' => {
                if !self.swallow {
                    self.lines.push(std::mem::take(&mut self.current));
                }
                self.current.clear();
                self.swallow = false;
                self.restart = false;
            }
            '\u{8}' | '\u{7f}' => {
                if !self.swallow {
                    self.current.pop();
                }
            }
            character if character.is_control() => {}
            character => {
                if self.swallow {
                    return;
                }
                if self.restart {
                    self.current.clear();
                    self.restart = false;
                }
                self.current.push(character);
            }
        }
    }

    /// The question pi waits on, if the line it is writing looks like one.
    pub fn question(&self) -> Option<&str> {
        let question = self.current.trim();
        (question.ends_with(':') || question.ends_with('?')).then_some(question)
    }

    /// The user answered: the question goes into the lines with what was
    /// sent (dots for a secret), and pi's echo of it is dropped.
    pub fn answered(&mut self, answer: &str, secret: bool) {
        let shown = if secret {
            "•".repeat(answer.chars().count().min(24))
        } else {
            answer.to_string()
        };
        let question = std::mem::take(&mut self.current);
        self.lines.push(format!("{} {shown}", question.trim_end()));
        self.swallow = true;
        self.restart = false;
    }
}

/// Whether a question asks for something to keep hidden.
pub fn is_secret_question(question: &str) -> bool {
    let question = question.to_lowercase();
    ["api key", "token", "password", "secret"]
        .iter()
        .any(|word| question.contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_follows_the_terminal() {
        let mut transcript = Transcript::default();
        transcript.feed(b"Open this URL to sign in:\r\n  https://example.com/auth\r\n");
        transcript.feed(b"\x1b[1G\x1b[0JPaste the code (http://127.0.0.1:1/cb): \x1b[166G");
        assert_eq!(
            transcript.lines,
            ["Open this URL to sign in:", "  https://example.com/auth"]
        );
        assert_eq!(
            transcript.question(),
            Some("Paste the code (http://127.0.0.1:1/cb):")
        );

        transcript.answered("abc", false);
        transcript.feed(b"abc\r\r\nExchanging...\r\n");
        assert_eq!(
            transcript.lines[2],
            "Paste the code (http://127.0.0.1:1/cb): abc"
        );
        assert_eq!(transcript.lines[3], "Exchanging...");
        assert_eq!(transcript.question(), None);
    }

    #[test]
    fn split_utf8_and_secrets() {
        let mut transcript = Transcript::default();
        let bytes = "Ключ API key: ".as_bytes();
        transcript.feed(&bytes[..3]);
        transcript.feed(&bytes[3..]);
        assert_eq!(transcript.question(), Some("Ключ API key:"));
        assert!(is_secret_question("Enter DeepSeek API key:"));
        assert!(!is_secret_question(
            "Paste the authorization code / redirect URL here:"
        ));
        transcript.answered("sk-123", true);
        assert_eq!(transcript.lines[0], "Ключ API key: ••••••");
    }

    #[test]
    fn lists_after_a_prefix() {
        let text = "Unknown provider \"x\".\nProviders: anthropic, openai, xai\n";
        assert_eq!(
            list_after(text, "Providers:"),
            ["anthropic", "openai", "xai"]
        );
        assert!(list_after("Stored credentials: none", "Stored credentials:").is_empty());
    }
}
