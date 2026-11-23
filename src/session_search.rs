//! Search across pi's session files (⌘⇧F), like Zed's project search: plain
//! text or a regular expression, optionally case-sensitive or whole words.
//! Only what you wrote and what pi answered is searched, not tool output.
//!
//! Read-only, and it blocks on the filesystem: run it on a background thread.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use regex::{Regex, RegexBuilder};
use serde_json::Value;

use crate::sessions;

/// The search stops after this many matches; the page says so.
pub const MAX_MATCHES: usize = 2_000;
/// Lines shown per session; the rest are only counted.
pub const LINES_PER_SESSION: usize = 30;
/// Characters kept before the first match of a long line.
const EXCERPT_LEAD: usize = 48;
const EXCERPT_LENGTH: usize = 200;
const TITLE_MAX_CHARS: usize = 80;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

impl Query {
    /// `Ok(None)` for an empty query; the error is short enough to show.
    pub fn compile(&self) -> Result<Option<Regex>, String> {
        if self.text.is_empty() {
            return Ok(None);
        }
        let pattern = if self.regex {
            self.text.clone()
        } else {
            regex::escape(&self.text)
        };
        let pattern = if self.whole_word {
            format!(r"\b(?:{pattern})\b")
        } else {
            pattern
        };
        RegexBuilder::new(&pattern)
            .case_insensitive(!self.case_sensitive)
            .size_limit(1 << 22)
            .build()
            .map(Some)
            .map_err(|error| match error {
                regex::Error::Syntax(message) => message
                    .lines()
                    .last()
                    .unwrap_or("invalid regular expression")
                    .trim_start_matches("error: ")
                    .to_string(),
                _ => "the regular expression is too big".to_string(),
            })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    User,
    Assistant,
}

/// One line of a message with at least one match.
#[derive(Clone, Debug)]
pub struct Hit {
    pub role: Role,
    /// The line, shortened around the first match when it is long.
    pub excerpt: String,
    /// Matches in `excerpt`, as byte ranges.
    pub ranges: Vec<Range<usize>>,
    /// The whole line, to find the message again in the open chat.
    pub line: String,
}

#[derive(Clone, Debug)]
pub struct SessionHits {
    pub workspace: PathBuf,
    pub path: PathBuf,
    pub title: String,
    pub modified: SystemTime,
    /// At most `LINES_PER_SESSION`.
    pub hits: Vec<Hit>,
    pub matches: usize,
}

#[derive(Clone, Default, Debug)]
pub struct Results {
    /// Most recently changed sessions first.
    pub sessions: Vec<SessionHits>,
    pub matches: usize,
    /// Stopped at `MAX_MATCHES`.
    pub truncated: bool,
}

/// Searches every session of `workspaces`. Checks `cancel` between files.
pub fn search(workspaces: &[PathBuf], regex: &Regex, cancel: &AtomicBool) -> Results {
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    for workspace in workspaces {
        for directory in sessions::session_directories(workspace) {
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
                if seen.insert((workspace.clone(), path.clone())) {
                    files.push((workspace.clone(), path, modified));
                }
            }
        }
    }
    search_files(files, regex, cancel)
}

/// Searches `(workspace, session file, modified)` triples, newest first.
fn search_files(
    mut files: Vec<(PathBuf, PathBuf, SystemTime)>,
    regex: &Regex,
    cancel: &AtomicBool,
) -> Results {
    files.sort_by(|left, right| right.2.cmp(&left.2));

    let mut results = Results::default();
    for (workspace, path, modified) in files {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let budget = MAX_MATCHES - results.matches;
        let Some(session) = search_file(&workspace, &path, modified, regex, budget) else {
            continue;
        };
        results.matches += session.matches;
        results.sessions.push(session);
        if results.matches >= MAX_MATCHES {
            results.truncated = true;
            break;
        }
    }
    results
}

/// The matches in one session file whose header `cwd` is `workspace`.
fn search_file(
    workspace: &Path,
    path: &Path,
    modified: SystemTime,
    regex: &Regex,
    budget: usize,
) -> Option<SessionHits> {
    let mut reader = BufReader::new(File::open(path).ok()?);
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line).ok()?;
    let header: Value = serde_json::from_slice(sessions::trim_line(&line)).ok()?;
    if header["type"] != "session" || !sessions::same_directory(header["cwd"].as_str()?, workspace)
    {
        return None;
    }

    let mut session_name: Option<String> = None;
    let mut first_user_text: Option<String> = None;
    let mut hits = Vec::new();
    let mut matches = 0;
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let text = sessions::trim_line(&line);
        let is_session_info = sessions::contains(text, br#""type":"session_info""#);
        let is_message = sessions::contains(text, br#""role":"user""#)
            || sessions::contains(text, br#""role":"assistant""#);
        if !is_session_info && !is_message {
            continue;
        }
        let Ok(entry) = serde_json::from_slice::<Value>(text) else {
            continue;
        };
        if entry["type"] == "session_info" {
            session_name = entry["name"]
                .as_str()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string);
            continue;
        }
        if entry["type"] != "message" || matches >= budget {
            continue;
        }
        let role = match entry["message"]["role"].as_str() {
            Some("user") => Role::User,
            Some("assistant") => Role::Assistant,
            _ => continue,
        };
        let content = sessions::message_text(&entry["message"]);
        let content = match role {
            Role::User => crate::attachments::display_text(&content),
            Role::Assistant => content,
        };
        if role == Role::User && first_user_text.is_none() {
            first_user_text = Some(content.clone());
        }
        for text_line in content.lines() {
            let ranges: Vec<Range<usize>> = regex
                .find_iter(text_line)
                .map(|found| found.range())
                .filter(|range| !range.is_empty())
                .collect();
            if ranges.is_empty() {
                continue;
            }
            matches += ranges.len();
            if hits.len() < LINES_PER_SESSION {
                hits.push(hit(role, text_line, &ranges));
            }
            if matches >= budget {
                break;
            }
        }
    }
    if matches == 0 {
        return None;
    }
    let title = session_name
        .or(first_user_text)
        .map(|title| sessions::one_line(&title, TITLE_MAX_CHARS))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| "Untitled session".to_string());
    Some(SessionHits {
        workspace: workspace.to_path_buf(),
        path: path.to_path_buf(),
        title,
        modified,
        hits,
        matches,
    })
}

/// A line cut to `EXCERPT_LENGTH` characters around its first match, with
/// the match ranges moved into the excerpt.
fn hit(role: Role, line: &str, ranges: &[Range<usize>]) -> Hit {
    let trimmed_start = line.len() - line.trim_start().len();
    let short = line.trim().chars().count() <= EXCERPT_LENGTH;
    let lead_start = if short {
        trimmed_start
    } else {
        let first = ranges[0].start.max(trimmed_start);
        let cut = line[trimmed_start..first]
            .char_indices()
            .rev()
            .nth(EXCERPT_LEAD.saturating_sub(1))
            .map_or(trimmed_start, |(index, _)| trimmed_start + index);
        // Start at a word, not in the middle of one.
        match line[cut..first].find(char::is_whitespace) {
            Some(space) if cut > trimmed_start => {
                let after = cut + space;
                after + line[after..].len() - line[after..].trim_start().len()
            }
            _ => cut,
        }
    };
    let end = line[lead_start..]
        .char_indices()
        .nth(EXCERPT_LENGTH)
        .map_or(line.len(), |(index, _)| lead_start + index);
    let prefix = if lead_start > trimmed_start {
        "…"
    } else {
        ""
    };
    let suffix = if end < line.trim_end().len() {
        "…"
    } else {
        ""
    };
    let body = line[lead_start..end].trim_end();
    let excerpt = format!("{prefix}{body}{suffix}");
    let shift = prefix.len();
    let ranges = ranges
        .iter()
        .filter_map(|range| {
            let start = range.start.max(lead_start);
            let end = range.end.min(lead_start + body.len());
            (start < end).then(|| start - lead_start + shift..end - lead_start + shift)
        })
        .collect();
    Hit {
        role,
        excerpt,
        ranges,
        line: line.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn query(text: &str) -> Query {
        Query {
            text: text.to_string(),
            ..Query::default()
        }
    }

    fn session(directory: &Path, name: &str, workspace: &Path, lines: &[Value]) -> PathBuf {
        let path = directory.join(name);
        let mut file = File::create(&path).unwrap();
        let header = serde_json::json!({ "type": "session", "cwd": workspace });
        writeln!(file, "{header}").unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
        path
    }

    fn message(role: &str, text: &str) -> Value {
        serde_json::json!({
            "type": "message",
            "message": { "role": role, "content": [{ "type": "text", "text": text }] }
        })
    }

    #[test]
    fn options_change_what_matches() {
        let line = "Retry the Pull request; pull again";
        let found =
            |query: Query| -> usize { query.compile().unwrap().unwrap().find_iter(line).count() };
        assert_eq!(found(query("pull")), 2);
        assert_eq!(
            found(Query {
                case_sensitive: true,
                ..query("pull")
            }),
            1
        );
        assert_eq!(
            found(Query {
                whole_word: true,
                ..query("pul")
            }),
            0
        );
        assert_eq!(
            found(Query {
                regex: true,
                ..query(r"p\w+l")
            }),
            2
        );
        // Without the regex option, regex characters are literal.
        assert_eq!(found(query(r"p\w+l")), 0);
        assert!(
            Query {
                regex: true,
                ..query("(")
            }
            .compile()
            .is_err()
        );
        assert!(query("").compile().unwrap().is_none());
    }

    #[test]
    fn finds_messages_in_the_workspace_sessions() {
        let root = std::env::temp_dir().join(format!("pi-gpui-search-{}", std::process::id()));
        let workspace = root.join("project");
        fs::create_dir_all(&workspace).unwrap();
        let sessions_dir = root.join("sessions");
        fs::create_dir_all(&sessions_dir).unwrap();

        let ours = session(
            &sessions_dir,
            "a.jsonl",
            &workspace,
            &[
                message("user", "Add retries to SyncClient::pull"),
                message(
                    "assistant",
                    "I'll read the client first.\nRetries now back off.",
                ),
                serde_json::json!({ "type": "message", "message": { "role": "toolResult", "content": [{ "type": "text", "text": "retries in tool output" }] } }),
            ],
        );
        let other = session(
            &sessions_dir,
            "b.jsonl",
            &root.join("elsewhere"),
            &[message("user", "retries somewhere else")],
        );

        let regex = query("retries").compile().unwrap().unwrap();
        let now = SystemTime::now();
        let files = vec![
            (workspace.clone(), ours, now),
            (workspace.clone(), other, now),
        ];
        let results = search_files(files, &regex, &AtomicBool::new(false));
        assert_eq!(results.sessions.len(), 1);
        let found = &results.sessions[0];
        assert_eq!(found.title, "Add retries to SyncClient::pull");
        assert_eq!(found.matches, 2);
        assert_eq!(found.hits[0].role, Role::User);
        assert_eq!(found.hits[1].role, Role::Assistant);
        assert_eq!(found.hits[1].excerpt, "Retries now back off.");
        assert_eq!(found.hits[1].ranges, vec![0..7]);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn long_lines_are_cut_around_the_match() {
        let line = format!("{}needle{}", "a".repeat(300), "b".repeat(300));
        let ranges = std::slice::from_ref(&(300..306));
        let hit = hit(Role::Assistant, &line, ranges);
        assert!(hit.excerpt.starts_with('…') && hit.excerpt.ends_with('…'));
        let range = hit.ranges[0].clone();
        assert_eq!(&hit.excerpt[range], "needle");
        assert!(hit.excerpt.chars().count() <= EXCERPT_LENGTH + 2);
    }
}
