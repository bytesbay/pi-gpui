//! Slash commands in the message field, as in pi's terminal UI.
//!
//! pi's own commands (extension commands, prompt templates, skills) come from
//! RPC `get_commands` and run by sending the text as a prompt. pi's built-in
//! terminal commands do not exist in RPC mode, so the app implements the
//! useful ones itself on top of RPC commands.

use gpui::SharedString;
use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Source {
    /// Implemented by the app.
    App,
    Extension,
    Prompt,
    Skill,
}

impl Source {
    pub(super) fn label(self) -> &'static str {
        match self {
            Source::App => "app",
            Source::Extension => "extension",
            Source::Prompt => "prompt",
            Source::Skill => "skill",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct SlashCommand {
    /// Without the leading slash.
    pub name: SharedString,
    pub description: SharedString,
    pub source: Source,
    /// Shown as a hint after the name, e.g. `[instructions]`.
    pub arguments: Option<&'static str>,
    /// Keyboard shortcut for the same action, if any.
    pub keys: Option<&'static str>,
}

/// The app's commands. Names follow pi's terminal UI.
pub(super) fn builtin() -> Vec<SlashCommand> {
    let command = |name: &'static str,
                   description: &'static str,
                   arguments: Option<&'static str>,
                   keys: Option<&'static str>| SlashCommand {
        name: name.into(),
        description: description.into(),
        source: Source::App,
        arguments,
        keys,
    };
    vec![
        command(
            "compact",
            "Summarize earlier history to free up context",
            Some("[instructions]"),
            None,
        ),
        command(
            "tree",
            "Browse the session tree and move to another point",
            None,
            Some("⌘⇧T"),
        ),
        command(
            "fork",
            "Fork a new session from one of your messages (opens the tree)",
            None,
            None,
        ),
        command("clone", "Copy this session into a new one", None, None),
        command("model", "Switch the model", None, Some("⌘⇧M")),
        command("new", "Start a new session", None, Some("⌘N")),
        command("name", "Rename this session", Some("<name>"), None),
        command("session", "Show session info, usage and cost", None, None),
        command("copy", "Copy the last answer", None, None),
        command("export", "Export the session to HTML", None, None),
    ]
}

/// pi's commands from a `get_commands` response.
pub(super) fn from_pi(data: &Value) -> Vec<SlashCommand> {
    data["commands"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|command| {
            let name = command["name"].as_str()?;
            if name == crate::pi::NAVIGATE_COMMAND {
                return None;
            }
            let source = match command["source"].as_str() {
                Some("prompt") => Source::Prompt,
                Some("skill") => Source::Skill,
                _ => Source::Extension,
            };
            Some(SlashCommand {
                name: name.to_string().into(),
                description: command["description"]
                    .as_str()
                    .map(|text| crate::sessions::one_line(text, 120))
                    .unwrap_or_default()
                    .into(),
                source,
                arguments: None,
                keys: None,
            })
        })
        .collect()
}

/// The command name being typed: the text is `/` plus a name without spaces.
pub(super) fn typed_query(text: &str) -> Option<&str> {
    let query = text.strip_prefix('/')?;
    (!query.contains(char::is_whitespace)).then_some(query)
}

/// Indices of commands matching `query`: name prefixes first, then names
/// containing it, then descriptions containing it. Skills also match without
/// their `skill:` prefix.
pub(super) fn matching(commands: &[SlashCommand], query: &str) -> Vec<usize> {
    let query = query.to_lowercase();
    let mut ranked: Vec<(u8, usize)> = commands
        .iter()
        .enumerate()
        .filter_map(|(index, command)| {
            let name = command.name.to_lowercase();
            let short = name.strip_prefix("skill:").unwrap_or(&name);
            let rank = if name.starts_with(&query) || short.starts_with(&query) {
                0
            } else if name.contains(&query) {
                1
            } else if command.description.to_lowercase().contains(&query) {
                2
            } else {
                return None;
            };
            Some((rank, index))
        })
        .collect();
    ranked.sort_by_key(|(rank, index)| (*rank, *index));
    ranked.into_iter().map(|(_, index)| index).collect()
}

/// Splits `/name rest` into the name and trimmed arguments.
pub(super) fn parse(text: &str) -> Option<(&str, &str)> {
    let rest = text.trim().strip_prefix('/')?;
    let (name, arguments) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    (!name.is_empty()).then(|| (name, arguments.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_stops_at_whitespace() {
        assert_eq!(typed_query("/com"), Some("com"));
        assert_eq!(typed_query("/"), Some(""));
        assert_eq!(typed_query("/compact now"), None);
        assert_eq!(typed_query("hello"), None);
    }

    #[test]
    fn prefixes_rank_first_and_skills_match_short_names() {
        let mut commands = builtin();
        commands.push(SlashCommand {
            name: "skill:commit".into(),
            description: "Create a git commit".into(),
            source: Source::Skill,
            arguments: None,
            keys: None,
        });
        let names = |query| -> Vec<String> {
            matching(&commands, query)
                .into_iter()
                .map(|index| commands[index].name.to_string())
                .collect()
        };
        assert_eq!(names("co")[..3], ["compact", "copy", "skill:commit"]);
        assert!(names("git").contains(&"skill:commit".to_string()));
    }

    #[test]
    fn parse_splits_name_and_arguments() {
        assert_eq!(
            parse("/compact  keep the API notes "),
            Some(("compact", "keep the API notes"))
        );
        assert_eq!(parse("/tree"), Some(("tree", "")));
        assert_eq!(parse("/"), None);
        assert_eq!(parse("plain"), None);
    }
}
