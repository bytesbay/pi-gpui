//! App settings, as in Zed: a JSON file (comments and trailing commas
//! allowed) is the source of truth. The app watches it and applies changes
//! as soon as it is saved, so a person, the Settings page or a pi agent can
//! all edit it. The Settings page rewrites only the value it changes and
//! keeps everything else, comments included.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use gpui::{App, Global, SharedString, Task};
use serde_json::Value;

use crate::platform;
use crate::ui::Theme;

pub const THEME_SYSTEM: &str = "system";
pub const THEME_DARK: &str = "Ayu Dark";
pub const THEME_LIGHT: &str = "Ayu Light";
pub const THEMES: [&str; 2] = [THEME_DARK, THEME_LIGHT];
const DEFAULT_BUFFER_FONT: &str = "JetBrains Mono NL";
/// Ships with macOS; used when the configured code font is missing.
const FALLBACK_BUFFER_FONT: &str = "Menlo";
const POLL_INTERVAL: Duration = Duration::from_secs(1);

const DEFAULT_FILE: &str = r#"// Pi GPUI settings.
//
// This file is the source of truth: the app applies changes as soon as it is
// saved, and the Settings page (Pi GPUI > Settings…, ⌘,) writes here too.
// Comments and trailing commas are allowed.
{
  // "system" follows macOS, or pick a theme: "Ayu Dark", "Ayu Light".
  "theme": "system",

  // Font of the interface. ".SystemUIFont" is the macOS system font.
  "ui_font_family": ".SystemUIFont",

  // Text size of messages and answers, in pixels.
  "agent_font_size": 14,

  // Font and size of code blocks, tool output and inline code. Menlo is
  // used when the font is not installed.
  "buffer_font_family": "JetBrains Mono NL",
  "buffer_font_size": 12,
}
"#;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub theme: String,
    pub ui_font_family: SharedString,
    pub agent_font_size: f32,
    pub buffer_font_family: SharedString,
    pub buffer_font_size: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: THEME_SYSTEM.to_string(),
            ui_font_family: ".SystemUIFont".into(),
            agent_font_size: 14.,
            buffer_font_family: DEFAULT_BUFFER_FONT.into(),
            buffer_font_size: 12.,
        }
    }
}

impl Global for Settings {}

impl Settings {
    pub fn get(cx: &App) -> &Settings {
        cx.global::<Settings>()
    }

    /// Unknown or invalid values fall back to defaults one by one, so a typo
    /// in one setting does not reset the others.
    fn from_json(value: &Value) -> Self {
        let defaults = Self::default();
        let text = |key: &str, fallback: SharedString| {
            value[key]
                .as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map_or(fallback, |text| SharedString::from(text.to_string()))
        };
        let size = |key: &str, fallback: f32| {
            value[key]
                .as_f64()
                .map(|size| (size as f32).clamp(8., 32.))
                .unwrap_or(fallback)
        };
        Self {
            theme: value["theme"]
                .as_str()
                .filter(|theme| *theme == THEME_SYSTEM || THEMES.contains(theme))
                .map_or(defaults.theme, str::to_string),
            ui_font_family: text("ui_font_family", defaults.ui_font_family),
            agent_font_size: size("agent_font_size", defaults.agent_font_size),
            buffer_font_family: text("buffer_font_family", defaults.buffer_font_family),
            buffer_font_size: size("buffer_font_size", defaults.buffer_font_size),
        }
    }

    /// Line height for transcript text.
    pub fn agent_line_height(&self) -> f32 {
        (self.agent_font_size * 1.5).round()
    }

    pub fn buffer_line_height(&self) -> f32 {
        (self.buffer_font_size * 1.618).round()
    }
}

/// The settings file and its watcher.
pub struct SettingsFile {
    pub path: PathBuf,
    /// Why the last saved version could not be read; the previous settings
    /// stay in effect meanwhile.
    pub error: Option<String>,
    modified: Option<SystemTime>,
    _watch: Task<()>,
}

impl Global for SettingsFile {}

pub fn path() -> PathBuf {
    crate::sessions::home_directory()
        .unwrap_or_default()
        .join("Library/Application Support/pi-gpui/settings.json")
}

/// Loads (or creates) the settings file, applies it and starts watching it.
pub fn init(cx: &mut App) {
    let path = path();
    if !path.exists() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = std::fs::write(&path, DEFAULT_FILE) {
            eprintln!("pi-gpui: could not create {}: {error}", path.display());
        }
    }
    cx.set_global(Settings::default());
    let watch = cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(POLL_INTERVAL).await;
            if cx.update(reload_if_changed).is_err() {
                return;
            }
        }
    });
    cx.set_global(SettingsFile {
        path,
        error: None,
        modified: None,
        _watch: watch,
    });
    reload_if_changed(cx);
    apply_theme(cx);
}

fn reload_if_changed(cx: &mut App) {
    let file = cx.global::<SettingsFile>();
    let modified = std::fs::metadata(&file.path)
        .and_then(|metadata| metadata.modified())
        .ok();
    if modified.is_some() && modified == file.modified {
        return;
    }
    let path = file.path.clone();
    let parsed = std::fs::read_to_string(&path)
        .map_err(|error| error.to_string())
        .and_then(|text| parse(&text));
    let file = cx.global_mut::<SettingsFile>();
    file.modified = modified;
    match parsed {
        Ok(mut settings) => {
            file.error = None;
            // A code font that is not installed would fall back to a
            // proportional font; use Menlo instead.
            let installed = cx
                .text_system()
                .all_font_names()
                .iter()
                .any(|name| *name == settings.buffer_font_family.as_ref());
            if !installed {
                eprintln!(
                    "pi-gpui: code font \"{}\" is not installed; using {FALLBACK_BUFFER_FONT}",
                    settings.buffer_font_family
                );
                settings.buffer_font_family = FALLBACK_BUFFER_FONT.into();
            }
            let theme_changed = Settings::get(cx).theme != settings.theme;
            if *Settings::get(cx) != settings {
                cx.set_global(settings);
                if theme_changed {
                    apply_theme(cx);
                }
            }
        }
        Err(error) => {
            eprintln!("pi-gpui: {}: {error}", path.display());
            file.error = Some(error);
        }
    }
    cx.refresh_windows();
}

/// Sets the theme global from the setting and the macOS appearance.
pub fn apply_theme(cx: &mut App) {
    let system_is_dark = matches!(
        cx.window_appearance(),
        gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
    );
    let dark = match Settings::get(cx).theme.as_str() {
        THEME_DARK => true,
        THEME_LIGHT => false,
        _ => system_is_dark,
    };
    let theme = Theme::ayu(dark);
    platform::set_app_appearance(theme.is_dark());
    cx.set_global(theme);
    cx.refresh_windows();
}

/// Writes one setting into the file, keeping the rest of it as it is, and
/// applies it right away.
pub fn set(cx: &mut App, key: &str, value: Value) {
    let path = cx.global::<SettingsFile>().path.clone();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| DEFAULT_FILE.to_string());
    let updated = set_top_level_value(&text, key, &value);
    if let Err(error) = std::fs::write(&path, updated) {
        cx.global_mut::<SettingsFile>().error = Some(format!("could not save: {error}"));
        cx.refresh_windows();
        return;
    }
    reload_if_changed(cx);
}

fn parse(text: &str) -> Result<Settings, String> {
    let value: Value =
        serde_json::from_str(&strip_jsonc(text)).map_err(|error| error.to_string())?;
    if !value.is_object() {
        return Err("the file must contain one JSON object".to_string());
    }
    Ok(Settings::from_json(&value))
}

/// JSON with comments and trailing commas → plain JSON. Positions of the
/// remaining characters are kept (comments become spaces), so parse errors
/// point at the right line.
fn strip_jsonc(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    let mut in_string = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            out.push(byte);
            if byte == b'\\' && index + 1 < bytes.len() {
                out.push(bytes[index + 1]);
                index += 2;
                continue;
            }
            in_string = byte != b'"';
            index += 1;
            continue;
        }
        match (byte, bytes.get(index + 1)) {
            (b'"', _) => {
                in_string = true;
                out.push(byte);
                index += 1;
            }
            (b'/', Some(b'/')) => {
                while index < bytes.len() && bytes[index] != b'\n' {
                    out.push(b' ');
                    index += 1;
                }
            }
            (b'/', Some(b'*')) => {
                out.extend_from_slice(b"  ");
                index += 2;
                while index < bytes.len()
                    && !(bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/'))
                {
                    out.push(if bytes[index] == b'\n' { b'\n' } else { b' ' });
                    index += 1;
                }
                out.extend_from_slice(b"  ");
                index += 2;
            }
            (b',', _) => {
                // A trailing comma: the next meaningful character closes.
                let mut next = index + 1;
                while next < bytes.len() && bytes[next].is_ascii_whitespace() {
                    next += 1;
                }
                let closes = matches!(bytes.get(next), Some(b'}') | Some(b']'));
                out.push(if closes { b' ' } else { b',' });
                index += 1;
            }
            _ => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_default()
}

/// Replaces the value of a top-level `key` in a JSONC document, or adds the
/// key before the closing brace. Everything else stays byte for byte.
fn set_top_level_value(text: &str, key: &str, value: &Value) -> String {
    let rendered = value.to_string();
    if let Some(span) = top_level_value_span(text, key) {
        return format!("{}{rendered}{}", &text[..span.start], &text[span.end..]);
    }
    // Not present: insert before the last closing brace.
    let stripped = strip_jsonc(text);
    let Some(close) = stripped.rfind('}') else {
        return format!("{{\n  \"{key}\": {rendered}\n}}\n");
    };
    let before = stripped[..close].trim_end();
    let needs_comma = !before.ends_with('{') && !before.ends_with(',');
    let insert_at = before.len();
    format!(
        "{}{}\n  \"{key}\": {rendered},\n{}",
        &text[..insert_at],
        if needs_comma { "," } else { "" },
        &text[close..]
    )
}

/// Byte range of the value of a top-level key, found on the comment-free
/// copy (same positions as the original).
fn top_level_value_span(text: &str, key: &str) -> Option<std::ops::Range<usize>> {
    let stripped = strip_jsonc(text);
    let bytes = stripped.as_bytes();
    let mut depth = 0usize;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth = depth.saturating_sub(1),
            b'"' => {
                let start = index + 1;
                let end = string_end(bytes, start)?;
                let is_key = depth == 1 && {
                    let mut after = end + 1;
                    while after < bytes.len() && bytes[after].is_ascii_whitespace() {
                        after += 1;
                    }
                    bytes.get(after) == Some(&b':')
                };
                if is_key && &stripped[start..end] == key {
                    let mut value_start = end + 1;
                    while bytes[value_start] != b':' {
                        value_start += 1;
                    }
                    value_start += 1;
                    while value_start < bytes.len() && bytes[value_start].is_ascii_whitespace() {
                        value_start += 1;
                    }
                    return Some(value_start..value_end(bytes, value_start)?);
                }
                index = end;
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn string_end(bytes: &[u8], mut index: usize) -> Option<usize> {
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'"' => return Some(index),
            _ => index += 1,
        }
    }
    None
}

/// End of a JSON value starting at `start` (string, number, literal, or a
/// nested object or array).
fn value_end(bytes: &[u8], start: usize) -> Option<usize> {
    match bytes.get(start)? {
        b'"' => string_end(bytes, start + 1).map(|end| end + 1),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut index = start;
            while index < bytes.len() {
                match bytes[index] {
                    b'"' => index = string_end(bytes, index + 1)?,
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(index + 1);
                        }
                    }
                    _ => {}
                }
                index += 1;
            }
            None
        }
        _ => {
            let mut index = start;
            while index < bytes.len()
                && !matches!(
                    bytes[index],
                    b',' | b'}' | b']' | b'\n' | b' ' | b'\t' | b'\r'
                )
            {
                index += 1;
            }
            Some(index)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_file_parses_to_defaults() {
        assert_eq!(parse(DEFAULT_FILE), Ok(Settings::default()));
    }

    #[test]
    fn invalid_values_fall_back_one_by_one() {
        let settings =
            parse(r#"{ "theme": "Nope", "agent_font_size": 16, "buffer_font_size": "big" }"#)
                .unwrap();
        assert_eq!(settings.theme, THEME_SYSTEM);
        assert_eq!(settings.agent_font_size, 16.);
        assert_eq!(settings.buffer_font_size, 12.);
        assert!(parse("{ broken").is_err());
    }

    #[test]
    fn setting_a_value_keeps_comments_and_other_keys() {
        let updated = set_top_level_value(DEFAULT_FILE, "theme", &json!("Ayu Light"));
        assert!(updated.contains(r#""theme": "Ayu Light""#));
        assert!(updated.contains("// \"system\" follows macOS"));
        assert_eq!(parse(&updated).unwrap().theme, THEME_LIGHT);

        let sized = set_top_level_value(&updated, "agent_font_size", &json!(16));
        assert_eq!(parse(&sized).unwrap().agent_font_size, 16.);
        assert_eq!(sized.len(), updated.len());
    }

    #[test]
    fn missing_keys_are_added_and_nested_keys_ignored() {
        let text = "{\n  // note\n  \"nested\": { \"theme\": \"x\" }\n}\n";
        let updated = set_top_level_value(text, "theme", &json!("Ayu Dark"));
        let value: Value = serde_json::from_str(&strip_jsonc(&updated)).unwrap();
        assert_eq!(value["theme"], "Ayu Dark");
        assert_eq!(value["nested"]["theme"], "x");
        assert!(updated.contains("// note"));
    }
}
