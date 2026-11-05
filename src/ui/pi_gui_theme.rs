//! pi-gui's themes, so the app looks like the user's pi-gui.
//!
//! pi-gui derives every color from a six-color seed per preset and variant
//! (`deriveThemeTokens` in its renderer). This is a port of that derivation
//! and of its preset table; the selected preset and mode are read from
//! pi-gui's `ui-state.json`, which is only read, never written.

use gpui::{Hsla, Rgba};

use super::theme::{SyntaxColors, Theme, ThemeColors};

type Rgb = [f64; 3];

struct Seed {
    surface: &'static str,
    ink: &'static str,
    accent: &'static str,
    added: &'static str,
    removed: &'static str,
    warning: &'static str,
}

const fn seed(
    surface: &'static str,
    ink: &'static str,
    accent: &'static str,
    added: &'static str,
    removed: &'static str,
    warning: &'static str,
) -> Seed {
    Seed {
        surface,
        ink,
        accent,
        added,
        removed,
        warning,
    }
}

/// `(id, name, light, dark)`, in pi-gui's order.
const PRESETS: &[(&str, &str, Seed, Seed)] = &[
    (
        "default",
        "Default",
        seed(
            "#ffffff", "#282825", "#526795", "#2ea043", "#c45666", "#d97706",
        ),
        seed(
            "#1d1d1c", "#eeeeea", "#9aaed8", "#40c977", "#e05467", "#d97706",
        ),
    ),
    (
        "catppuccin",
        "Catppuccin",
        seed(
            "#eff1f5", "#4c4f69", "#8839ef", "#40a02b", "#d20f39", "#df8e1d",
        ),
        seed(
            "#1e1e2e", "#cdd6f4", "#cba6f7", "#a6e3a1", "#f38ba8", "#f9e2af",
        ),
    ),
    (
        "tokyo-night",
        "Tokyo Night",
        seed(
            "#d5d6db", "#343b58", "#34548a", "#485e30", "#8c4351", "#8f5e15",
        ),
        seed(
            "#1a1b26", "#c0caf5", "#7aa2f7", "#9ece6a", "#f7768e", "#e0af68",
        ),
    ),
    (
        "nord",
        "Nord",
        seed(
            "#eceff4", "#2e3440", "#5e81ac", "#5d7f45", "#bf616a", "#b0802a",
        ),
        seed(
            "#2e3440", "#eceff4", "#88c0d0", "#a3be8c", "#bf616a", "#ebcb8b",
        ),
    ),
    (
        "dracula",
        "Dracula",
        seed(
            "#fffbeb", "#1f1f1f", "#644ac9", "#14710a", "#cb3a2a", "#a34d14",
        ),
        seed(
            "#282a36", "#f8f8f2", "#bd93f9", "#50fa7b", "#ff5555", "#ffb86c",
        ),
    ),
    (
        "gruvbox",
        "Gruvbox",
        seed(
            "#fbf1c7", "#3c3836", "#458588", "#79740e", "#9d0006", "#b57614",
        ),
        seed(
            "#282828", "#ebdbb2", "#83a598", "#b8bb26", "#fb4934", "#fabd2f",
        ),
    ),
    (
        "github",
        "GitHub",
        seed(
            "#ffffff", "#1f2328", "#0969da", "#1a7f37", "#cf222e", "#9a6700",
        ),
        seed(
            "#0d1117", "#e6edf3", "#4493f8", "#3fb950", "#f85149", "#d29922",
        ),
    ),
    (
        "vscode",
        "VS Code",
        seed(
            "#ffffff", "#1f1f1f", "#007acc", "#587c0c", "#ad0707", "#bf8803",
        ),
        seed(
            "#1e1e1e", "#d4d4d4", "#3794ff", "#81b88b", "#f14c4c", "#cca700",
        ),
    ),
    (
        "ayu",
        "Ayu",
        seed(
            "#fcfcfc", "#5c6166", "#ffaa33", "#6cbf43", "#e65050", "#fa8d3e",
        ),
        seed(
            "#0b0e14", "#bfbdb6", "#e6b450", "#aad94c", "#d95757", "#ff8f40",
        ),
    ),
];

/// The theme pi-gui shows for `preset_id` (unknown ids fall back to
/// pi-gui's default preset, as pi-gui does).
pub fn theme(preset_id: &str, dark: bool) -> Theme {
    let (_, name, light_seed, dark_seed) = PRESETS
        .iter()
        .find(|(id, ..)| *id == preset_id)
        .unwrap_or(&PRESETS[0]);
    let seed = if dark { dark_seed } else { light_seed };
    let tokens = Tokens::derive(seed, !dark);
    let mut colors = tokens.colors();
    colors.syntax = SyntaxColors::for_pi_gui_preset(preset_id, dark);
    Theme { name, colors }
}

/// The subset of pi-gui's CSS tokens the app uses.
struct Tokens {
    window: Rgb,
    sidebar: Rgb,
    main: Rgb,
    surface: Rgb,
    surface_muted: Rgb,
    line: Rgb,
    line_strong: Rgb,
    text: Rgb,
    text_strong: Rgb,
    muted: Rgb,
    muted_soft: Rgb,
    muted_subtle: Rgb,
    muted_icon: Rgb,
    accent: Rgb,
    error: Rgb,
    success: Rgb,
    warning: Rgb,
}

impl Tokens {
    /// Port of pi-gui's `deriveThemeTokens`.
    fn derive(seed: &Seed, light: bool) -> Self {
        let surface = parse(seed.surface);
        let ink = parse(seed.ink);
        let [accent, added, removed, warning] =
            [seed.accent, seed.added, seed.removed, seed.warning]
                .map(|color| deepen_for_contrast(parse(color), ink, surface, 3.));
        let pick = |light_value: f64, dark_value: f64| if light { light_value } else { dark_value };
        let tint = |light_amount: f64, dark_amount: f64| {
            mix(surface, ink, pick(light_amount, dark_amount))
        };
        let black = [0., 0., 0.];
        let white = [255., 255., 255.];
        let sidebar = if light {
            tint(0.04, 0.)
        } else {
            mix(surface, black, 0.17)
        };
        let fade = |light_amount: f64, dark_amount: f64, min_contrast: f64| {
            fade_with_contrast(
                ink,
                surface,
                pick(light_amount, dark_amount),
                min_contrast,
                sidebar,
            )
        };
        Self {
            window: if light {
                tint(0.07, 0.)
            } else {
                mix(surface, black, 0.28)
            },
            sidebar,
            main: surface,
            surface: if light {
                mix(surface, white, 0.4)
            } else {
                tint(0., 0.025)
            },
            surface_muted: tint(0.04, 0.055),
            line: tint(0.12, 0.11),
            line_strong: tint(0.19, 0.21),
            text: fade(0.1, 0.13, 7.),
            text_strong: ink,
            muted: fade(0.35, 0.37, 4.5),
            muted_soft: fade(0.41, 0.42, 4.),
            muted_subtle: fade(0.48, 0.56, 3.),
            muted_icon: fade(0.38, 0.42, 3.5),
            accent,
            error: removed,
            success: added,
            warning,
        }
    }

    /// Where pi-gui uses each token: `--main` behind the transcript and top
    /// bar, `--sidebar`, `--surface` for the composer and the active session,
    /// foreground-alpha overlays for hover, `--line` for dividers.
    fn colors(&self) -> ThemeColors {
        let ink = |amount: f32| hsla(self.text_strong, amount);
        ThemeColors {
            title_bar_background: hsla(self.main, 1.),
            title_bar_inactive_background: hsla(self.window, 1.),
            panel_background: hsla(self.sidebar, 1.),
            editor_background: hsla(self.main, 1.),
            elevated_surface_background: hsla(self.surface, 1.),
            element_background: hsla(self.surface_muted, 1.),
            element_hover: ink(0.06),
            element_active: ink(0.09),
            element_selected: hsla(self.surface, 1.),
            ghost_element_hover: ink(0.06),
            ghost_element_active: ink(0.09),
            ghost_element_selected: hsla(self.surface, 1.),
            border: hsla(self.line_strong, 1.),
            border_variant: hsla(self.line, 1.),
            border_focused: hsla(self.accent, 0.45),
            text: hsla(self.text_strong, 1.),
            text_muted: hsla(self.muted, 1.),
            text_placeholder: hsla(self.muted_soft, 1.),
            text_disabled: hsla(self.muted_subtle, 1.),
            text_accent: hsla(self.accent, 1.),
            icon: hsla(self.text, 1.),
            icon_muted: hsla(self.muted_icon, 1.),
            icon_disabled: hsla(self.muted_subtle, 1.),
            icon_accent: hsla(self.accent, 1.),
            error: hsla(self.error, 1.),
            error_background: hsla(self.error, 0.1),
            warning: hsla(self.warning, 1.),
            warning_background: hsla(self.warning, 0.12),
            success: hsla(self.success, 1.),
            success_background: hsla(self.success, 0.14),
            info: hsla(self.accent, 1.),
            info_background: hsla(self.accent, 0.12),
            // `--code-block-bg`: dark blocks darken what is behind them.
            code_block_background: if self.main.iter().sum::<f64>() < 384. {
                Hsla::black().opacity(0.25)
            } else {
                hsla(mix(self.main, self.text_strong, 0.035), 1.)
            },
            syntax: SyntaxColors::github_dark(),
        }
    }
}

fn parse(hex: &str) -> Rgb {
    let value = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or_default();
    [
        f64::from((value >> 16) & 255),
        f64::from((value >> 8) & 255),
        f64::from(value & 255),
    ]
}

/// Like pi-gui's `mix`, rounded to whole channels as its `toHex` does.
fn mix(from: Rgb, to: Rgb, amount: f64) -> Rgb {
    [0, 1, 2].map(|i| (from[i] + (to[i] - from[i]) * amount).round())
}

fn relative_luminance(color: Rgb) -> f64 {
    let [r, g, b] = color.map(|channel| {
        let value = channel / 255.;
        if value <= 0.03928 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    });
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn contrast_ratio(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn fade_with_contrast(ink: Rgb, surface: Rgb, amount: f64, min_contrast: f64, against: Rgb) -> Rgb {
    let mut fitted = amount;
    while fitted > 0. && contrast_ratio(mix(ink, surface, fitted), against) < min_contrast {
        fitted = (fitted - 0.01).max(0.);
    }
    mix(ink, surface, fitted)
}

fn deepen_for_contrast(color: Rgb, ink: Rgb, surface: Rgb, min_contrast: f64) -> Rgb {
    let mut amount = 0.;
    while amount < 1. && contrast_ratio(mix(color, ink, amount), surface) < min_contrast {
        amount = f64::min(1., amount + 0.02);
    }
    mix(color, ink, amount)
}

fn hsla(color: Rgb, alpha: f32) -> Hsla {
    Rgba {
        r: color[0] as f32 / 255.,
        g: color[1] as f32 / 255.,
        b: color[2] as f32 / 255.,
        a: alpha,
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(color: Rgb) -> String {
        format!(
            "#{:02x}{:02x}{:02x}",
            color[0] as u8, color[1] as u8, color[2] as u8
        )
    }

    /// Values computed by pi-gui's own `themeTokensFor("ayu", "dark")`.
    #[test]
    fn ayu_dark_matches_pi_gui() {
        let (.., dark) = PRESETS.iter().find(|(id, ..)| *id == "ayu").unwrap();
        let tokens = Tokens::derive(dark, false);
        assert_eq!(hex(tokens.window), "#080a0e");
        assert_eq!(hex(tokens.sidebar), "#090c11");
        assert_eq!(hex(tokens.main), "#0b0e14");
        assert_eq!(hex(tokens.surface), "#101218");
        assert_eq!(hex(tokens.surface_muted), "#15181d");
        assert_eq!(hex(tokens.line), "#1f2126");
        assert_eq!(hex(tokens.line_strong), "#313336");
        assert_eq!(hex(tokens.text), "#a8a6a1");
        assert_eq!(hex(tokens.muted), "#7c7c7a");
        assert_eq!(hex(tokens.muted_soft), "#737472");
        assert_eq!(hex(tokens.muted_subtle), "#5e5f5f");
        assert_eq!(hex(tokens.accent), "#e6b450");
        assert_eq!(hex(tokens.error), "#d95757");
        assert_eq!(hex(tokens.warning), "#ff8f40");
    }
}
