//! Links from a tool call to another pi session, for example the session of
//! a subagent the tool started. A pi extension names them in its tool
//! result (`details.piGpui.sessionLinks`); a plugin can point at a field of
//! an existing tool's arguments or result (`contributes.sessionLinks`). The
//! transcript shows a button that opens the session, when its project is in
//! the sidebar.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use gpui::{Global, SharedString};
use serde_json::Value;

use crate::sessions;

/// `contributes.sessionLinks`: in calls of `tool`, the strings at `field`
/// (`args.…` or `details.…`, arrays are walked) are session files.
#[derive(Clone, Debug)]
pub struct SessionLinkRule {
    pub tool: SharedString,
    pub field: String,
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LinkSpec {
    pub file: PathBuf,
    pub label: Option<String>,
}

/// What the app found at a linked file.
#[derive(Clone, Debug)]
pub enum LinkState {
    Checking,
    /// A pi session; `workspace` is its header `cwd`.
    Session {
        workspace: PathBuf,
        title: String,
    },
    /// Why it cannot be opened: a few words for next to the link, and a
    /// sentence for its tooltip.
    Unavailable {
        short: String,
        long: String,
    },
}

/// The projects in the sidebar, for deciding whether a link can open.
#[derive(Default)]
pub struct KnownProjects(pub Vec<PathBuf>);

impl Global for KnownProjects {}

impl KnownProjects {
    /// The sidebar project a session of `workspace` belongs to.
    pub fn find(&self, workspace: &Path) -> Option<&PathBuf> {
        let workspace = workspace.to_string_lossy();
        self.0
            .iter()
            .find(|project| sessions::same_directory(&workspace, project))
    }
}

/// Links of one tool call: the result's own `details.piGpui.sessionLinks`,
/// then the fields plugins declared for this tool. Without duplicates.
pub fn collect(
    rules: &[SessionLinkRule],
    tool: &str,
    args: &Value,
    details: &Value,
) -> Vec<LinkSpec> {
    let mut links = Vec::new();
    for item in details["piGpui"]["sessionLinks"]
        .as_array()
        .into_iter()
        .flatten()
    {
        match item {
            Value::String(file) => push(&mut links, file, None),
            Value::Object(_) => {
                if let Some(file) = item["file"].as_str() {
                    push(&mut links, file, item["label"].as_str().map(str::to_string));
                }
            }
            _ => {}
        }
    }
    for rule in rules.iter().filter(|rule| rule.tool == tool) {
        let (root, path) = match rule.field.split_once('.') {
            Some(("args", path)) => (args, path),
            Some(("details", path)) => (details, path),
            _ => continue,
        };
        let mut found = Vec::new();
        walk(root, &path.split('.').collect::<Vec<_>>(), &mut found);
        for file in found {
            push(&mut links, &file, rule.label.clone());
        }
    }
    links
}

fn push(links: &mut Vec<LinkSpec>, file: &str, label: Option<String>) {
    let file = file.trim();
    if file.is_empty() {
        return;
    }
    let file = match file.strip_prefix("~/") {
        Some(rest) => match sessions::home_directory() {
            Some(home) => home.join(rest),
            None => PathBuf::from(file),
        },
        None => PathBuf::from(file),
    };
    if !links.iter().any(|link| link.file == file) {
        links.push(LinkSpec { file, label });
    }
}

/// Strings at `path` under `value`; arrays on the way are walked.
fn walk(value: &Value, path: &[&str], found: &mut Vec<String>) {
    match value {
        Value::Array(items) => {
            for item in items {
                walk(item, path, found);
            }
        }
        _ => match path.split_first() {
            None => {
                if let Some(text) = value.as_str() {
                    found.push(text.to_string());
                }
            }
            Some((key, rest)) => walk(&value[*key], rest, found),
        },
    }
}

/// Reads the session header and title. Blocks on the filesystem.
pub fn inspect(file: &Path) -> LinkState {
    let shown = sessions::display_path(file);
    let handle = match File::open(file) {
        Ok(handle) => handle,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return unavailable("file not found", format!("{shown} doesn't exist."));
        }
        Err(error) => {
            return unavailable("can't be read", format!("{shown} can't be read: {error}."));
        }
    };
    let mut first = Vec::new();
    if let Err(error) = BufReader::new(handle).read_until(b'\n', &mut first) {
        return unavailable("can't be read", format!("{shown} can't be read: {error}."));
    }
    let header: Option<Value> = serde_json::from_slice(sessions::trim_line(&first)).ok();
    let Some(cwd) = header
        .as_ref()
        .filter(|header| header["type"] == "session")
        .and_then(|header| header["cwd"].as_str())
    else {
        return unavailable(
            "not a pi session",
            format!("{shown} isn't a pi session file."),
        );
    };
    let workspace = PathBuf::from(cwd);
    let title = sessions::session_title(file, &workspace).unwrap_or_else(|| {
        file.file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    LinkState::Session { workspace, title }
}

fn unavailable(short: &str, long: String) -> LinkState {
    LinkState::Unavailable {
        short: short.to_string(),
        long,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collects_links_from_results_and_rules() {
        let details = json!({
            "piGpui": { "sessionLinks": ["/s/a.jsonl", { "file": "/s/b.jsonl", "label": "Reviewer" }] },
            "results": [{ "sessionFile": "/s/c.jsonl" }, { "sessionFile": "/s/a.jsonl" }, {}],
        });
        let args = json!({ "resume": "/s/d.jsonl" });
        let rules = vec![
            SessionLinkRule {
                tool: "subagent".into(),
                field: "details.results.sessionFile".to_string(),
                label: Some("Subagent".to_string()),
            },
            SessionLinkRule {
                tool: "subagent".into(),
                field: "args.resume".to_string(),
                label: None,
            },
            SessionLinkRule {
                tool: "other".into(),
                field: "args.resume".to_string(),
                label: None,
            },
        ];
        let links = collect(&rules, "subagent", &args, &details);
        let files: Vec<_> = links
            .iter()
            .map(|link| link.file.to_str().unwrap())
            .collect();
        assert_eq!(
            files,
            ["/s/a.jsonl", "/s/b.jsonl", "/s/c.jsonl", "/s/d.jsonl"]
        );
        assert_eq!(links[1].label.as_deref(), Some("Reviewer"));
        assert_eq!(links[2].label.as_deref(), Some("Subagent"));
        assert!(collect(&rules, "read", &args, &json!({})).is_empty());
    }

    #[test]
    fn inspects_session_files() {
        let directory =
            std::env::temp_dir().join(format!("pi-gpui-session-links-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let session = directory.join("s.jsonl");
        std::fs::write(
            &session,
            "{\"type\":\"session\",\"cwd\":\"/work/app\"}\n{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"Review the diff\"}}\n",
        )
        .unwrap();
        let other = directory.join("notes.jsonl");
        std::fs::write(&other, "{\"type\":\"note\"}\n").unwrap();

        match inspect(&session) {
            LinkState::Session { workspace, title } => {
                assert_eq!(workspace, PathBuf::from("/work/app"));
                assert_eq!(title, "Review the diff");
            }
            state => panic!("{state:?}"),
        }
        let state = inspect(&other);
        assert!(
            matches!(&state, LinkState::Unavailable { short, .. } if short == "not a pi session"),
            "{state:?}"
        );
        assert!(matches!(
            inspect(&directory.join("gone.jsonl")),
            LinkState::Unavailable { short, .. } if short == "file not found"
        ));

        let known = KnownProjects(vec![PathBuf::from("/work/app")]);
        assert!(known.find(Path::new("/work/app")).is_some());
        assert!(known.find(Path::new("/work/other")).is_none());
        std::fs::remove_dir_all(&directory).ok();
    }
}
