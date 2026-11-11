//! Read-only access to pi's session files: where they live, short summaries for
//! the sidebar and pi-gui's single-writer leases. Never writes into pi's data.
//!
//! All functions block on the filesystem; call them from a background task.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use serde_json::Value;

/// pi-gui refreshes its leases every minute and treats them as stale after five.
const LEASE_TTL: Duration = Duration::from_secs(5 * 60);
const TITLE_MAX_CHARS: usize = 80;

#[derive(Clone, Debug)]
pub struct SessionSummary {
    pub path: PathBuf,
    pub title: String,
    pub modified: SystemTime,
}

/// The most recently modified sessions whose header `cwd` is `workspace`.
/// Sessions without a user message and without a name are skipped: pi creates
/// them, but there is nothing to resume.
pub fn list_sessions(workspace: &Path, limit: usize) -> Vec<SessionSummary> {
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for directory in session_directories(workspace) {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .is_none_or(|extension| extension != "jsonl")
            {
                continue;
            }
            let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified()) else {
                continue;
            };
            if seen.insert(path.clone()) {
                candidates.push((path, modified));
            }
        }
    }
    candidates.sort_by(|left, right| right.1.cmp(&left.1));

    candidates
        .into_iter()
        .filter_map(|(path, modified)| {
            let title = read_title(&path, workspace)?;
            Some(SessionSummary {
                path,
                title,
                modified,
            })
        })
        .take(limit)
        .collect()
}

/// Mirrors pi's lookup order: `PI_CODING_AGENT_SESSION_DIR`, the project and
/// global `sessionDir` settings, then `<agent dir>/sessions`. Sessions are
/// normally grouped in a per-workspace subdirectory; a custom directory may be
/// flat, so both locations are scanned and filtered by the header `cwd`.
fn session_directories(workspace: &Path) -> Vec<PathBuf> {
    let agent_directory = std::env::var_os("PI_CODING_AGENT_DIR")
        .map(PathBuf::from)
        .or_else(|| home_directory().map(|home| home.join(".pi/agent")));

    let base = std::env::var_os("PI_CODING_AGENT_SESSION_DIR")
        .map(PathBuf::from)
        .or_else(|| setting_session_dir(&workspace.join(".pi/settings.json")))
        .or_else(|| {
            agent_directory
                .as_ref()
                .and_then(|directory| setting_session_dir(&directory.join("settings.json")))
        })
        .or_else(|| agent_directory.map(|directory| directory.join("sessions")));

    let Some(base) = base else {
        return Vec::new();
    };
    let base = if base.is_relative() {
        workspace.join(base)
    } else {
        base
    };
    vec![base.join(encode_workspace(workspace)), base]
}

fn setting_session_dir(settings_path: &Path) -> Option<PathBuf> {
    let settings: Value = serde_json::from_slice(&fs::read(settings_path).ok()?).ok()?;
    settings["sessionDir"].as_str().map(PathBuf::from)
}

/// `/Users/me/project` → `--Users-me-project--`, as documented in pi's session format.
fn encode_workspace(workspace: &Path) -> String {
    let path = workspace.to_string_lossy();
    let trimmed = path.trim_start_matches(['/', '\\']);
    format!("--{}--", trimmed.replace(['/', '\\', ':'], "-"))
}

/// Session name if one was set, otherwise the first user message. Only lines
/// that can matter are parsed as JSON, so large sessions stay cheap to scan.
fn read_title(path: &Path, workspace: &Path) -> Option<String> {
    let mut reader = BufReader::new(File::open(path).ok()?);
    let mut line = Vec::new();

    reader.read_until(b'\n', &mut line).ok()?;
    let header: Value = serde_json::from_slice(trim_line(&line)).ok()?;
    if header["type"] != "session" || !same_directory(header["cwd"].as_str()?, workspace) {
        return None;
    }

    let mut first_user_text: Option<String> = None;
    let mut session_name: Option<String> = None;
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let text = trim_line(&line);
        let is_session_info = contains(text, br#""type":"session_info""#);
        let is_user_message = first_user_text.is_none() && contains(text, br#""role":"user""#);
        if !is_session_info && !is_user_message {
            continue;
        }
        let Ok(entry) = serde_json::from_slice::<Value>(text) else {
            continue;
        };
        if entry["type"] == "session_info" {
            // The latest entry wins; an absent name clears an earlier one.
            session_name = entry["name"]
                .as_str()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string);
        } else if entry["type"] == "message" && entry["message"]["role"] == "user" {
            first_user_text = Some(message_text(&entry["message"]));
        }
    }

    let title = session_name.or(first_user_text)?;
    Some(one_line(
        &crate::attachments::display_text(&title),
        TITLE_MAX_CHARS,
    ))
}

/// Who else holds `session` open, according to pi-gui's lease convention.
/// pi itself does not prevent two writers, so this is the only signal we get.
pub fn foreign_lease_holder(session: &Path) -> Option<String> {
    let lease_path = PathBuf::from(format!("{}.lease", session.display()));
    let age = fs::metadata(&lease_path)
        .and_then(|metadata| metadata.modified())
        .ok()?
        .elapsed()
        .unwrap_or_default();
    if age > LEASE_TTL {
        return None;
    }
    let lease: Value = serde_json::from_slice(&fs::read(&lease_path).ok()?).ok()?;
    if let Some(pid) = lease["pid"].as_u64()
        && !process_is_alive(pid)
    {
        return None;
    }
    Some(
        lease["surface"]
            .as_str()
            .unwrap_or("another app")
            .to_string(),
    )
}

fn process_is_alive(pid: u64) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

pub fn message_text(message: &Value) -> String {
    if let Some(text) = message["content"].as_str() {
        return text.to_string();
    }
    message["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn one_line(text: &str, limit: usize) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    match line.char_indices().nth(limit) {
        Some((index, _)) => format!("{}…", &line[..index]),
        None => line.to_string(),
    }
}

/// "now", "5m", "3h", "2d".
pub fn relative_age(modified: SystemTime) -> String {
    let seconds = modified.elapsed().map_or(0, |elapsed| elapsed.as_secs());
    match seconds {
        0..60 => "now".to_string(),
        60..3_600 => format!("{}m", seconds / 60),
        3_600..86_400 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// `/Users/me/project` → `~/project`.
pub fn display_path(path: &Path) -> String {
    match home_directory() {
        Some(home) => match path.strip_prefix(&home) {
            Ok(relative_path) => format!("~/{}", relative_path.display()),
            Err(_) => path.display().to_string(),
        },
        None => path.display().to_string(),
    }
}

pub fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// The app's own files (settings, projects, plugins): `~/Library/Application
/// Support/pi-gpui` on macOS, `$XDG_CONFIG_HOME/pi-gpui` (`~/.config/pi-gpui`)
/// elsewhere.
pub fn app_data_directory() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        return Some(home_directory()?.join("Library/Application Support/pi-gpui"));
    }
    Some(xdg_directory("XDG_CONFIG_HOME", ".config")?.join("pi-gpui"))
}

/// Downloads that can be thrown away: `~/Library/Caches/pi-gpui` on macOS,
/// `$XDG_CACHE_HOME/pi-gpui` (`~/.cache/pi-gpui`) elsewhere.
pub fn app_cache_directory() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        return Some(home_directory()?.join("Library/Caches/pi-gpui"));
    }
    Some(xdg_directory("XDG_CACHE_HOME", ".cache")?.join("pi-gpui"))
}

/// An XDG base directory; relative values are ignored, as the spec says.
fn xdg_directory(variable: &str, fallback: &str) -> Option<PathBuf> {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| Some(home_directory()?.join(fallback)))
}

fn same_directory(cwd: &str, workspace: &Path) -> bool {
    let cwd = Path::new(cwd);
    cwd == workspace || cwd.canonicalize().is_ok_and(|cwd| cwd == workspace)
}

fn trim_line(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    while end > 0 && matches!(line[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    &line[..end]
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn workspace_encoding_matches_pi() {
        assert_eq!(
            encode_workspace(Path::new("/Users/me/projects/pi-gpui")),
            "--Users-me-projects-pi-gpui--"
        );
    }

    #[test]
    fn title_prefers_latest_session_name() {
        let directory = std::env::temp_dir().join(format!("pi-gpui-test-{}", std::process::id()));
        fs::create_dir_all(&directory).expect("temp dir");
        let workspace = directory.canonicalize().expect("canonical");
        let session = workspace.join("session.jsonl");
        let mut file = File::create(&session).expect("session file");
        writeln!(
            file,
            r#"{{"type":"session","version":3,"id":"x","cwd":"{}"}}"#,
            workspace.display()
        )
        .expect("write");
        writeln!(file, r#"{{"type":"message","id":"a","message":{{"role":"user","content":"  \nFirst question\nsecond line"}}}}"#).expect("write");
        drop(file);
        assert_eq!(
            read_title(&session, &workspace).as_deref(),
            Some("First question")
        );

        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&session)
            .expect("append");
        writeln!(
            file,
            r#"{{"type":"session_info","id":"b","name":"Refactoring"}}"#
        )
        .expect("write");
        drop(file);
        assert_eq!(
            read_title(&session, &workspace).as_deref(),
            Some("Refactoring")
        );

        assert_eq!(read_title(&session, Path::new("/elsewhere")), None);
        fs::remove_dir_all(&directory).ok();
    }
}
