//! One app per user on Linux. macOS routes `pi-gpui://` links and reopening to
//! the running app by itself; on Linux every launch (from the dock, a
//! `.desktop` link handler or a terminal) is a new process. The first one
//! listens on a socket in `$XDG_RUNTIME_DIR`; later ones hand it their link
//! and exit.

use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

/// Sent when the app is launched again without a link: bring it forward.
pub const ACTIVATE: &str = "activate";

/// Hands `message` to an app that is already running. `true` when it took
/// it, so this process should exit.
pub fn forward(message: &str) -> bool {
    let Some(path) = socket_path() else {
        return false;
    };
    let Ok(mut stream) = UnixStream::connect(&path) else {
        return false;
    };
    stream.write_all(format!("{message}\n").as_bytes()).is_ok()
}

/// Listens for later launches; each message goes to `deliver` on a
/// background thread.
pub fn listen(deliver: impl Fn(String) + Send + 'static) {
    let Some(path) = socket_path() else {
        return;
    };
    // `forward` found nobody listening, so a file left here is stale.
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("pi-gpui: cannot listen on {}: {error}", path.display());
            return;
        }
    };
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(stream).read_line(&mut line).is_ok() {
                let message = line.trim().to_string();
                if !message.is_empty() {
                    deliver(message);
                }
            }
        }
    });
}

fn socket_path() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_dir())?;
    // Development builds keep to themselves, so `cargo run` works while the
    // installed app is open.
    Some(runtime.join(if cfg!(debug_assertions) {
        "pi-gpui-dev.sock"
    } else {
        "pi-gpui.sock"
    }))
}
