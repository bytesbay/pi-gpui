//! Custom themes: JSON files in the app's `themes` folder.
//!
//! A file is either the app's own short format, where six seed colors are
//! enough and everything else is derived the way the built-in themes are:
//!
//! ```json
//! {
//!   "name": "Midnight",
//!   "background": "#0f1115",
//!   "text": "#e6e6e6",
//!   "accent": "#7aa2f7",
//!   "added": "#9ece6a",
//!   "removed": "#f7768e",
//!   "warning": "#e0af68",
//!   "style": { "panel.background": "#0b0d10", "syntax": { "keyword": "#bb9af7" } }
//! }
//! ```
//!
//! or a Zed theme family (`{"themes": [{"name", "appearance", "style"}]}`),
//! since the app's color tokens are Zed's. `style` overrides single tokens
//! in both.

use std::path::{Path, PathBuf};

use gpui::{Hsla, Rgba, SharedString};
use serde_json::{Map, Value};

use super::pi_gui_theme::{self, Seed};
use super::theme::{SyntaxColors, Theme, ThemeColors};

#[derive(Clone)]
pub struct UserTheme {
    pub name: SharedString,
    pub path: PathBuf,
    dark: Option<bool>,
    /// The theme's own object (seed colors and `style`).
    spec: Map<String, Value>,
}

/// The template "New theme" writes.
pub const TEMPLATE: &str = r##"{
  // Your theme. Pick it in Settings; the app applies changes on save.
  "name": "My Theme",

  // Six colors are enough: the rest is derived from them.
  "background": "#14161b",
  "text": "#e3e1db",
  "accent": "#7aa2f7",
  "added": "#9ece6a",
  "removed": "#f7768e",
  "warning": "#e0af68",

  // Optional: override single colors with Zed's theme keys
  // (https://zed.dev/docs/themes), syntax colors under "syntax".
  "style": {
    // "panel.background": "#101216",
    // "border": "#2a2d34",
    "syntax": {
      // "keyword": "#bb9af7",
      // "string": "#9ece6a",
      // "comment": "#5c6370"
    }
  }
}
"##;

/// All themes in `directory`, sorted by name, and why other files were
/// skipped.
pub fn load_dir(directory: &Path) -> (Vec<UserTheme>, Vec<String>) {
    let mut themes = Vec::new();
    let mut errors = Vec::new();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return (themes, errors);
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    paths.sort();
    for path in paths {
        match load_file(&path) {
            Ok(found) => themes.extend(found),
            Err(error) => {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                errors.push(format!("{name}: {error}"));
            }
        }
    }
    themes.sort_by_key(|theme| theme.name.to_lowercase());
    themes.dedup_by(|later, earlier| later.name == earlier.name);
    (themes, errors)
}

fn load_file(path: &Path) -> Result<Vec<UserTheme>, String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let value: Value = serde_json::from_str(&crate::settings::strip_jsonc(&text))
        .map_err(|error| error.to_string())?;
    let stem = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let theme = |object: &Map<String, Value>, fallback_name: &str| UserTheme {
        name: object
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(fallback_name)
            .to_string()
            .into(),
        path: path.to_path_buf(),
        dark: match object.get("appearance").and_then(Value::as_str) {
            Some("dark") => Some(true),
            Some("light") => Some(false),
            _ => None,
        },
        spec: object.clone(),
    };
    // A Zed theme family.
    if let Some(family) = value["themes"].as_array() {
        let themes: Vec<UserTheme> = family
            .iter()
            .filter_map(Value::as_object)
            .map(|object| theme(object, &stem))
            .collect();
        if themes.is_empty() {
            return Err("\"themes\" has no themes".to_string());
        }
        return Ok(themes);
    }
    let object = value
        .as_object()
        .ok_or("the file must contain one JSON object")?;
    let user_theme = theme(object, &stem);
    if user_theme.background().is_none() {
        return Err("needs a \"background\" color (or \"style\": {\"editor.background\"})".into());
    }
    Ok(vec![user_theme])
}

impl UserTheme {
    fn style(&self) -> Option<&Map<String, Value>> {
        self.spec.get("style").and_then(Value::as_object)
    }

    /// A seed color: the short format's key, else a Zed style key.
    fn color(&self, key: &str, style_keys: &[&str]) -> Option<Rgba> {
        self.spec
            .get(key)
            .and_then(parse_color)
            .or_else(|| {
                let style = self.style()?;
                style_keys
                    .iter()
                    .find_map(|key| style.get(*key).and_then(parse_color))
            })
            .map(|color| Rgba { a: 1., ..color })
    }

    fn background(&self) -> Option<Rgba> {
        self.color("background", &["editor.background", "background"])
    }

    pub fn theme(&self) -> Theme {
        let background = self.background().unwrap_or(Rgba {
            r: 0.,
            g: 0.,
            b: 0.,
            a: 1.,
        });
        let dark = self.dark.unwrap_or_else(|| Hsla::from(background).l < 0.5);
        let defaults = pi_gui_theme::default_seed(dark);
        let hex = |color: Option<Rgba>, fallback: &str| {
            color.map_or(fallback.to_string(), |color| {
                let channel = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
                format!(
                    "#{:02x}{:02x}{:02x}",
                    channel(color.r),
                    channel(color.g),
                    channel(color.b)
                )
            })
        };
        let surface = hex(Some(background), defaults.surface);
        let ink = hex(self.color("text", &["text"]), defaults.ink);
        let accent = hex(
            self.color("accent", &["text.accent", "icon.accent", "border.focused"]),
            defaults.accent,
        );
        let added = hex(self.color("added", &["created", "success"]), defaults.added);
        let removed = hex(
            self.color("removed", &["deleted", "error"]),
            defaults.removed,
        );
        let warning = hex(self.color("warning", &["warning"]), defaults.warning);
        let seed = Seed {
            surface: &surface,
            ink: &ink,
            accent: &accent,
            added: &added,
            removed: &removed,
            warning: &warning,
        };
        let mut colors = pi_gui_theme::colors_from_seed(&seed, dark);
        if let Some(style) = self.style() {
            apply_style(&mut colors, style);
        }
        Theme {
            name: self.name.clone(),
            colors,
        }
    }
}

/// Zed style keys over the derived colors.
fn apply_style(colors: &mut ThemeColors, style: &Map<String, Value>) {
    for (key, value) in style {
        let Some(color) = parse_color(value).map(Hsla::from) else {
            continue;
        };
        let slot = match key.as_str() {
            "title_bar.background" => &mut colors.title_bar_background,
            "title_bar.inactive_background" => &mut colors.title_bar_inactive_background,
            "panel.background" => &mut colors.panel_background,
            "editor.background" => &mut colors.editor_background,
            "elevated_surface.background" => &mut colors.elevated_surface_background,
            "element.background" => &mut colors.element_background,
            "element.hover" => &mut colors.element_hover,
            "element.active" => &mut colors.element_active,
            "element.selected" => &mut colors.element_selected,
            "ghost_element.hover" => &mut colors.ghost_element_hover,
            "ghost_element.active" => &mut colors.ghost_element_active,
            "ghost_element.selected" => &mut colors.ghost_element_selected,
            "border" => &mut colors.border,
            "border.variant" => &mut colors.border_variant,
            "border.focused" => &mut colors.border_focused,
            "text" => &mut colors.text,
            "text.muted" => &mut colors.text_muted,
            "text.placeholder" => &mut colors.text_placeholder,
            "text.disabled" => &mut colors.text_disabled,
            "text.accent" => &mut colors.text_accent,
            "icon" => &mut colors.icon,
            "icon.muted" => &mut colors.icon_muted,
            "icon.disabled" => &mut colors.icon_disabled,
            "icon.accent" => &mut colors.icon_accent,
            "error" => &mut colors.error,
            "error.background" => &mut colors.error_background,
            "warning" => &mut colors.warning,
            "warning.background" => &mut colors.warning_background,
            "success" => &mut colors.success,
            "success.background" => &mut colors.success_background,
            "info" => &mut colors.info,
            "info.background" => &mut colors.info_background,
            "code_block.background" => &mut colors.code_block_background,
            _ => continue,
        };
        *slot = color;
    }
    if let Some(syntax) = style.get("syntax").and_then(Value::as_object) {
        apply_syntax(&mut colors.syntax, syntax);
    }
}

/// Zed writes `"keyword": {"color": "#…"}`; a plain color works too.
fn apply_syntax(colors: &mut SyntaxColors, syntax: &Map<String, Value>) {
    let color = |key: &str| {
        let value = syntax.get(key)?;
        parse_color(value.get("color").unwrap_or(value)).map(Hsla::from)
    };
    let slots: [(&str, &mut Hsla); 11] = [
        ("comment", &mut colors.comment),
        ("keyword", &mut colors.keyword),
        ("string", &mut colors.string),
        ("number", &mut colors.number),
        ("function", &mut colors.function),
        ("type", &mut colors.r#type),
        ("constant", &mut colors.constant),
        ("operator", &mut colors.operator),
        ("property", &mut colors.property),
        ("tag", &mut colors.tag),
        ("attribute", &mut colors.attribute),
    ];
    for (key, slot) in slots {
        if let Some(value) = color(key) {
            *slot = value;
        }
    }
    if let Some(value) = color("punctuation") {
        colors.punctuation = Some(value);
    }
}

/// `#rgb`, `#rrggbb` or `#rrggbbaa`.
fn parse_color(value: &Value) -> Option<Rgba> {
    let hex = value.as_str()?.trim().strip_prefix('#')?;
    let expanded: String = if hex.len() == 3 {
        hex.chars().flat_map(|digit| [digit, digit]).collect()
    } else {
        hex.to_string()
    };
    let channels = match expanded.len() {
        6 => u32::from_str_radix(&expanded, 16).ok()? << 8 | 0xff,
        8 => u32::from_str_radix(&expanded, 16).ok()?,
        _ => return None,
    };
    Some(gpui::rgba(channels))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(name: &str, text: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("pi-gpui-themes-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(name), text).unwrap();
        directory
    }

    #[test]
    fn loads_short_and_zed_themes() {
        write("own.json", TEMPLATE);
        write(
            "zed.json",
            r##"{ "themes": [
                { "name": "Z Light", "appearance": "light",
                  "style": { "editor.background": "#fafafa", "text": "#222222",
                             "panel.background": "#eeeeeeff",
                             "syntax": { "keyword": { "color": "#ff0000" } } } } ] }"##,
        );
        let directory = write("broken.json", "{ \"name\": \"No background\" }");
        let (themes, errors) = load_dir(&directory);
        let names: Vec<&str> = themes.iter().map(|theme| theme.name.as_ref()).collect();
        assert_eq!(names, ["My Theme", "Z Light"]);
        assert_eq!(errors.len(), 1);

        let own = themes[0].theme();
        assert!(own.is_dark());
        let zed = themes[1].theme();
        assert!(!zed.is_dark());
        assert_eq!(zed.colors.panel_background, Hsla::from(gpui::rgb(0xeeeeee)));
        assert_eq!(zed.colors.syntax.keyword, Hsla::from(gpui::rgb(0xff0000)));
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn a_theme_without_a_name_takes_the_file_name() {
        let directory = std::env::temp_dir().join(format!("pi-gpui-noname-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("dusk.json"),
            r##"{ "background": "#101010" }"##,
        )
        .unwrap();
        let (themes, errors) = load_dir(&directory);
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].name.as_ref(), "dusk");
        assert!(errors.is_empty());
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn parses_short_and_alpha_colors() {
        assert_eq!(
            parse_color(&Value::from("#fff")),
            Some(gpui::rgba(0xffffffff))
        );
        assert_eq!(
            parse_color(&Value::from("#11223344")),
            Some(gpui::rgba(0x11223344))
        );
        assert_eq!(parse_color(&Value::from("red")), None);
    }
}
