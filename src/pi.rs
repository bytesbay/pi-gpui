//! Installed pi: discovery, version gating and starting `pi --mode rpc`.
//!
//! Everything here blocks; call it from a background task.

use std::ffi::OsString;
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use futures::channel::mpsc;

use crate::jsonl::{JsonlMessage, JsonlProcess};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

/// Field order makes the derived ordering compare major, then minor, then patch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    /// Finds the first `x.y.z` in `text`, ignoring prefixes such as `v` or `pi `.
    pub fn find_in(text: &str) -> Option<Self> {
        text.split(|character: char| !(character.is_ascii_digit() || character == '.'))
            .find_map(|candidate| {
                let mut parts = candidate.split('.');
                let major = parts.next()?.parse().ok()?;
                let minor = parts.next()?.parse().ok()?;
                let patch = parts.next()?.parse().ok()?;
                Some(Self {
                    major,
                    minor,
                    patch,
                })
            })
    }

    pub fn app() -> Self {
        Self {
            major: env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap_or(0),
            minor: env!("CARGO_PKG_VERSION_MINOR").parse().unwrap_or(0),
            patch: env!("CARGO_PKG_VERSION_PATCH").parse().unwrap_or(0),
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The oldest pi the app supports. Newer pi versions with the same major are
/// supported too; a new major is not (see docs/APP_PRINCIPLES.md).
pub const MIN_PI: Version = Version {
    major: 1,
    minor: 4,
    patch: 0,
};

/// The newest pi minor the app was tested with. Newer minors of the same
/// major still run, with a note that they are untested.
pub const TESTED_PI: Version = Version {
    major: 1,
    minor: 4,
    patch: 0,
};

/// Whether this app supports `pi`: at least `MIN_PI`, same major.
pub fn supports(pi: Version) -> bool {
    pi >= MIN_PI && pi.major == MIN_PI.major
}

/// Supported, but newer than anything the app was tested with.
pub fn is_untested(pi: Version) -> bool {
    (pi.major, pi.minor) > (TESTED_PI.major, TESTED_PI.minor)
}

/// The app's pi support as JSON, published next to each release so the
/// updater can tell which release fits the installed pi.
pub fn compatibility_json() -> String {
    serde_json::json!({
        "app": env!("CARGO_PKG_VERSION"),
        "pi": {
            "min": MIN_PI.to_string(),
            "major": MIN_PI.major,
            "tested": format!("{}.{}", TESTED_PI.major, TESTED_PI.minor),
        },
    })
    .to_string()
}

#[derive(Clone, Debug)]
pub struct Installation {
    pub executable: PathBuf,
    pub version: Version,
}

impl Installation {
    pub fn is_untested(&self) -> bool {
        is_untested(self.version)
    }
}

#[derive(Clone, Debug)]
pub enum CheckError {
    NotFound,
    VersionUnreadable {
        executable: PathBuf,
        details: String,
    },
    Unsupported {
        installation: Installation,
    },
}

impl fmt::Display for CheckError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CheckError::NotFound => write!(
                formatter,
                "pi was not found. Install pi or set its path in PI_EXECUTABLE."
            ),
            CheckError::VersionUnreadable {
                executable,
                details,
            } => write!(
                formatter,
                "Could not read the version of {}: {details}",
                executable.display()
            ),
            CheckError::Unsupported { installation } if installation.version < MIN_PI => write!(
                formatter,
                "Installed pi {} ({}) is too old: this app needs pi {MIN_PI} or newer. Update pi, then reopen the app.",
                installation.version,
                installation.executable.display(),
            ),
            CheckError::Unsupported { installation } => write!(
                formatter,
                "Installed pi {} ({}) is a new major version: this app supports pi {}.x. Check for app updates.",
                installation.version,
                installation.executable.display(),
                MIN_PI.major,
            ),
        }
    }
}

/// Finds pi, reads its version and checks that the app supports it.
pub fn check_installation() -> Result<Installation, CheckError> {
    let executable = discover().ok_or(CheckError::NotFound)?;
    let output = run_with_timeout(
        Command::new(&executable)
            .arg("--version")
            .env("PATH", path_with_executable_dir(&executable)),
        COMMAND_TIMEOUT,
    )
    .map_err(|details| CheckError::VersionUnreadable {
        executable: executable.clone(),
        details,
    })?;

    let version = Version::find_in(&output).ok_or_else(|| CheckError::VersionUnreadable {
        executable: executable.clone(),
        details: format!("unexpected output `{}`", output.trim()),
    })?;

    let installation = Installation {
        executable,
        version,
    };
    if supports(installation.version) {
        Ok(installation)
    } else {
        Err(CheckError::Unsupported { installation })
    }
}

/// Explicit `PI_EXECUTABLE` first, then the general lookup.
fn discover() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("PI_EXECUTABLE") {
        let path = PathBuf::from(explicit);
        return path.is_file().then_some(path);
    }
    find_executable("pi")
}

/// The inherited `PATH`, then the user's login shell. Apps started from Finder
/// do not inherit PATH entries added by shell rc files (nvm, fnm, volta), so
/// the shell lookup is what usually finds `pi` and `node`. Blocking.
pub fn find_executable(name: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PATH")
        && let Some(found) = std::env::split_paths(&path)
            .map(|directory| directory.join(name))
            .find(|candidate| candidate.is_file())
    {
        return Some(found);
    }

    // `name` comes from our code or a plugin manifest: only plain names are looked up.
    if !name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
    {
        return None;
    }
    let shell = std::env::var_os("SHELL").unwrap_or_else(|| OsString::from("/bin/zsh"));
    let output = run_with_timeout(
        Command::new(shell).args(["-ilc", &format!("command -v {name}")]),
        COMMAND_TIMEOUT,
    )
    .ok()?;

    // Interactive shells may print banners; the answer is the last absolute path.
    output
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with('/'))
        .map(PathBuf::from)
        .filter(|path| path.is_file())
}

/// pi is usually a `#!/usr/bin/env node` script, so the directory holding the
/// executable (where nvm also puts `node`) has to be on PATH for the child.
pub fn path_with_executable_dir(executable: &Path) -> OsString {
    let mut directories: Vec<PathBuf> = executable
        .parent()
        .map(|directory| vec![directory.to_path_buf()])
        .unwrap_or_default();
    if let Some(path) = std::env::var_os("PATH") {
        directories.extend(std::env::split_paths(&path));
    }
    std::env::join_paths(directories).unwrap_or_default()
}

fn run_with_timeout(command: &mut Command, timeout: Duration) -> Result<String, String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;

    let mut stdout = child.stdout.take().ok_or("stdout is unavailable")?;
    let mut stderr = child.stderr.take().ok_or("stderr is unavailable")?;
    let reader = thread::spawn(move || {
        let mut text = String::new();
        stdout.read_to_string(&mut text).map(|_| text)
    });
    // Read stderr too, so a chatty child cannot block on a full pipe; its
    // last line explains failures (e.g. "env: node: No such file or directory").
    let error_reader = thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let text = reader
                    .join()
                    .map_err(|_| "the reader thread panicked".to_string())?
                    .map_err(|error| error.to_string())?;
                let error_text = error_reader.join().unwrap_or_default();
                return if status.success() {
                    Ok(text)
                } else {
                    match error_text
                        .lines()
                        .rev()
                        .map(str::trim)
                        .find(|line| !line.is_empty())
                    {
                        Some(line) => Err(line.to_string()),
                        None => Err(format!("exited with {status}")),
                    }
                };
            }
            Ok(None) if started.elapsed() > timeout => {
                if let Err(error) = child.kill() {
                    return Err(format!("timed out and could not be stopped: {error}"));
                }
                return Err(format!("no answer within {} s", timeout.as_secs()));
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(error) => return Err(error.to_string()),
        }
    }
}

/// Starts `pi --mode rpc` in `workspace`: a new session, or `session` resumed.
pub fn spawn_pi(
    installation: &Installation,
    workspace: &Path,
    session: Option<&Path>,
    extensions: &[PathBuf],
) -> std::io::Result<(JsonlProcess, mpsc::Receiver<JsonlMessage>)> {
    let mut command = Command::new(&installation.executable);
    command.args(["--mode", "rpc"]);
    if let Some(bridge) = bridge_extension() {
        command.arg("--extension").arg(bridge);
    }
    // Tools that plugins ship for the agent, e.g. one whose calls a plugin draws.
    for extension in extensions {
        command.arg("--extension").arg(extension);
    }
    if let Some(session) = session {
        command.arg("--session").arg(session);
    }
    command
        .current_dir(workspace)
        .env("PATH", path_with_executable_dir(&installation.executable));
    JsonlProcess::spawn(&mut command)
}

/// The app's own extension (session tree navigation, see
/// `extensions/pi-gpui-bridge.ts`). It is embedded in the binary and written
/// to the app's data directory, then loaded with `--extension` for our pi
/// processes only. `None` if it cannot be written; tree navigation is then
/// unavailable but everything else works.
pub fn bridge_extension() -> Option<&'static Path> {
    static PATH: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        const SOURCE: &str = include_str!("../extensions/pi-gpui-bridge.ts");
        let dir = crate::sessions::home_directory()?
            .join("Library/Application Support/pi-gpui/extensions");
        let path = dir.join("pi-gpui-bridge.ts");
        // Left over from when the app was called Pi GPU UI.
        let _ = std::fs::remove_file(dir.join("pi-gpu-ui-bridge.ts"));
        if std::fs::read_to_string(&path).ok().as_deref() != Some(SOURCE) {
            let written =
                std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, SOURCE));
            if let Err(error) = written {
                eprintln!("pi-gpui: could not write the bridge extension: {error}");
                return None;
            }
        }
        Some(path)
    })
    .as_deref()
}

/// Names a session that is not open: starts pi on it just long enough to send
/// `set_session_name`, so pi itself writes the change.
pub async fn rename_session(
    installation: Installation,
    workspace: PathBuf,
    session: PathBuf,
    name: String,
) -> Result<(), String> {
    use futures::StreamExt as _;
    let (process, mut receiver) = spawn_pi(&installation, &workspace, Some(&session), &[])
        .map_err(|error| error.to_string())?;
    process
        .send(serde_json::json!({ "id": "rename", "type": "set_session_name", "name": name }))?;
    let mut result = Err("pi exited before renaming the session".to_string());
    while let Some(message) = receiver.next().await {
        match message {
            JsonlMessage::Record(record)
                if record["type"] == "response" && record["id"] == "rename" =>
            {
                result = if record["success"].as_bool() == Some(true) {
                    Ok(())
                } else {
                    Err(record["error"]
                        .as_str()
                        .unwrap_or("rename failed")
                        .to_string())
                };
                break;
            }
            JsonlMessage::Exited { stderr_tail, .. } => {
                if !stderr_tail.trim().is_empty() {
                    result = Err(stderr_tail.trim().to_string());
                }
                return result;
            }
            _ => {}
        }
    }
    // Closing stdin ends pi; wait for it so the session file is complete.
    drop(process);
    while let Some(message) = receiver.next().await {
        if matches!(message, JsonlMessage::Exited { .. }) {
            break;
        }
    }
    result
}

/// The bridge's command; hidden from the command menu.
pub const NAVIGATE_COMMAND: &str = "pi-gpui-navigate";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supports_the_minimum_and_newer_minors_of_the_same_major() {
        let version = |text| Version::find_in(text).expect("version");
        assert!(!supports(version("1.3.9")));
        assert!(supports(version("1.4.0")));
        assert!(supports(version("1.4.7")));
        assert!(supports(version("1.9.0")));
        assert!(!supports(version("2.0.0")));
        assert!(!is_untested(version("1.4.7")));
        assert!(is_untested(version("1.5.0")));
    }

    #[test]
    fn version_is_found_inside_noise() {
        assert_eq!(
            Version::find_in("pi v1.4.0\n"),
            Some(Version {
                major: 1,
                minor: 4,
                patch: 0
            })
        );
        assert_eq!(Version::find_in("no version"), None);
    }
}
