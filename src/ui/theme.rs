use gpui::{App, Global, Hsla, rgba};

/// Semantic color tokens, named after Zed's theme schema so components read
/// the same way as Zed's `ui` crate.
#[derive(Clone)]
pub struct ThemeColors {
    pub title_bar_background: Hsla,
    pub title_bar_inactive_background: Hsla,
    pub panel_background: Hsla,
    pub editor_background: Hsla,
    pub elevated_surface_background: Hsla,
    pub element_background: Hsla,
    pub element_hover: Hsla,
    pub element_active: Hsla,
    pub element_selected: Hsla,
    pub ghost_element_hover: Hsla,
    pub ghost_element_active: Hsla,
    pub ghost_element_selected: Hsla,
    pub border: Hsla,
    pub border_variant: Hsla,
    pub border_focused: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_placeholder: Hsla,
    pub text_disabled: Hsla,
    pub text_accent: Hsla,
    pub icon: Hsla,
    pub icon_muted: Hsla,
    pub icon_disabled: Hsla,
    pub icon_accent: Hsla,
    pub error: Hsla,
    pub error_background: Hsla,
    pub warning: Hsla,
    pub warning_background: Hsla,
    pub success: Hsla,
    pub success_background: Hsla,
    pub info: Hsla,
    pub info_background: Hsla,
    /// Behind fenced code in answers.
    pub code_block_background: Hsla,
    pub syntax: SyntaxColors,
}

/// Code colors, one per scope group (see `highlight.rs`). The palettes are
/// the syntax themes pi-gui pairs with each preset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SyntaxColors {
    pub comment: Hsla,
    pub keyword: Hsla,
    pub string: Hsla,
    pub number: Hsla,
    pub function: Hsla,
    pub r#type: Hsla,
    pub constant: Hsla,
    pub operator: Hsla,
    /// `None` keeps the text color.
    pub punctuation: Option<Hsla>,
    pub property: Hsla,
    pub tag: Hsla,
    pub attribute: Hsla,
}

impl SyntaxColors {
    #[allow(clippy::too_many_arguments)]
    const fn palette(
        comment: u32,
        keyword: u32,
        string: u32,
        number: u32,
        function: u32,
        r#type: u32,
        constant: u32,
        operator: u32,
        punctuation: Option<u32>,
        property: u32,
        tag: u32,
        attribute: u32,
    ) -> [Option<u32>; 12] {
        [
            Some(comment),
            Some(keyword),
            Some(string),
            Some(number),
            Some(function),
            Some(r#type),
            Some(constant),
            Some(operator),
            punctuation,
            Some(property),
            Some(tag),
            Some(attribute),
        ]
    }

    fn from_palette(palette: [Option<u32>; 12]) -> Self {
        let color = |index: usize| hex((palette[index].unwrap_or(0) << 8) | 0xff);
        Self {
            comment: color(0),
            keyword: color(1),
            string: color(2),
            number: color(3),
            function: color(4),
            r#type: color(5),
            constant: color(6),
            operator: color(7),
            punctuation: palette[8].map(|value| hex((value << 8) | 0xff)),
            property: color(9),
            tag: color(10),
            attribute: color(11),
        }
    }

    /// GitHub Dark Default.
    pub fn github_dark() -> Self {
        Self::from_palette(Self::palette(
            0x8b949e, 0xff7b72, 0xa5d6ff, 0x79c0ff, 0xd2a8ff, 0xffa657, 0x79c0ff, 0xff7b72, None,
            0x79c0ff, 0x7ee787, 0x79c0ff,
        ))
    }

    /// GitHub Light Default.
    pub fn github_light() -> Self {
        Self::from_palette(Self::palette(
            0x6e7781, 0xcf222e, 0x0a3069, 0x0550ae, 0x8250df, 0x953800, 0x0550ae, 0xcf222e, None,
            0x0550ae, 0x116329, 0x0550ae,
        ))
    }

    /// The syntax theme pi-gui uses for a preset and variant.
    pub fn for_pi_gui_preset(preset: &str, dark: bool) -> Self {
        let palette = match (preset, dark) {
            ("vscode", true) => Self::palette(
                0x6a9955, 0x569cd6, 0xce9178, 0xb5cea8, 0xdcdcaa, 0x4ec9b0, 0x569cd6, 0xd4d4d4,
                None, 0x9cdcfe, 0x569cd6, 0x9cdcfe,
            ),
            ("vscode", false) => Self::palette(
                0x008000, 0x0000ff, 0xa31515, 0x098658, 0x795e26, 0x267f99, 0x0000ff, 0x000000,
                None, 0x001080, 0x800000, 0xe50000,
            ),
            ("catppuccin", true) => Self::palette(
                0x9399b2,
                0xcba6f7,
                0xa6e3a1,
                0xfab387,
                0x89b4fa,
                0xf9e2af,
                0xfab387,
                0x94e2d5,
                Some(0x9399b2),
                0xb4befe,
                0xcba6f7,
                0xf9e2af,
            ),
            ("catppuccin", false) => Self::palette(
                0x7c7f93,
                0x8839ef,
                0x40a02b,
                0xfe640b,
                0x1e66f5,
                0xdf8e1d,
                0xfe640b,
                0x179299,
                Some(0x7c7f93),
                0x7287fd,
                0x8839ef,
                0xdf8e1d,
            ),
            ("tokyo-night", true) => Self::palette(
                0x565f89,
                0xbb9af7,
                0x9ece6a,
                0xff9e64,
                0x7aa2f7,
                0x2ac3de,
                0xff9e64,
                0x89ddff,
                Some(0x89ddff),
                0x73daca,
                0xf7768e,
                0xbb9af7,
            ),
            ("nord", true) => Self::palette(
                0x616e88, 0x81a1c1, 0xa3be8c, 0xb48ead, 0x88c0d0, 0x8fbcbb, 0x81a1c1, 0x81a1c1,
                None, 0xd8dee9, 0x81a1c1, 0x8fbcbb,
            ),
            ("dracula", true) => Self::palette(
                0x6272a4, 0xff79c6, 0xf1fa8c, 0xbd93f9, 0x50fa7b, 0x8be9fd, 0xbd93f9, 0xff79c6,
                None, 0x66d9ef, 0xff79c6, 0x50fa7b,
            ),
            ("gruvbox", true) => Self::palette(
                0x928374, 0xfb4934, 0xb8bb26, 0xd3869b, 0xfabd2f, 0xfabd2f, 0xd3869b, 0x8ec07c,
                None, 0x83a598, 0x8ec07c, 0xfabd2f,
            ),
            // default, github and ayu use GitHub's themes, as do the light
            // variants without a palette here.
            (_, true) => return Self::github_dark(),
            (_, false) => return Self::github_light(),
        };
        Self::from_palette(palette)
    }
}

pub struct Theme {
    pub name: &'static str,
    pub colors: ThemeColors,
}

impl Global for Theme {}

impl Theme {
    /// Dark themes have a dark editor background.
    pub fn is_dark(&self) -> bool {
        self.colors.editor_background.l < 0.5
    }
}

impl Theme {
    /// The built-in Ayu themes, derived the way pi-gui derives its Ayu preset.
    pub fn ayu(dark: bool) -> Self {
        let mut theme = super::pi_gui_theme::theme("ayu", dark);
        theme.name = if dark { "Ayu Dark" } else { "Ayu Light" };
        theme
    }

    /// Values from Zed's bundled "Ayu Dark" theme (assets/themes/ayu/ayu.json,
    /// MIT, Copyright (c) 2016 Ike Ku).
    pub fn ayu_dark() -> Self {
        Self {
            name: "Ayu Dark",
            colors: ThemeColors {
                title_bar_background: hex(0x313337ff),
                title_bar_inactive_background: hex(0x1f2127ff),
                panel_background: hex(0x1f2127ff),
                editor_background: hex(0x0d1016ff),
                elevated_surface_background: hex(0x1f2127ff),
                element_background: hex(0x1f2127ff),
                element_hover: hex(0x2d2f34ff),
                element_active: hex(0x3e4043ff),
                element_selected: hex(0x3e4043ff),
                ghost_element_hover: hex(0x2d2f34ff),
                ghost_element_active: hex(0x3e4043ff),
                ghost_element_selected: hex(0x3e4043ff),
                border: hex(0x3f4043ff),
                border_variant: hex(0x2d2f34ff),
                border_focused: hex(0x1b4a6eff),
                text: hex(0xbfbdb6ff),
                text_muted: hex(0x8a8986ff),
                text_placeholder: hex(0x696a6aff),
                text_disabled: hex(0x696a6aff),
                text_accent: hex(0x5ac1feff),
                icon: hex(0xbfbdb6ff),
                icon_muted: hex(0x8a8986ff),
                icon_disabled: hex(0x696a6aff),
                icon_accent: hex(0x5ac1feff),
                error: hex(0xef7177ff),
                error_background: hex(0x48161bff),
                warning: hex(0xfeb454ff),
                warning_background: hex(0x572815ff),
                success: hex(0xaad84cff),
                success_background: hex(0x294113ff),
                info: hex(0x5ac1feff),
                info_background: hex(0x0d2f4eff),
                code_block_background: hex(0x0d1016ff),
                syntax: SyntaxColors::github_dark(),
            },
        }
    }

    /// Values from Zed's bundled "One Dark" theme (assets/themes/one/one.json,
    /// MIT, Copyright (c) 2014 GitHub Inc.).
    pub fn one_dark() -> Self {
        Self {
            name: "One Dark",
            colors: ThemeColors {
                title_bar_background: hex(0x3b414dff),
                title_bar_inactive_background: hex(0x2e343eff),
                panel_background: hex(0x2f343eff),
                editor_background: hex(0x282c33ff),
                elevated_surface_background: hex(0x2f343eff),
                element_background: hex(0x2e343eff),
                element_hover: hex(0x363c46ff),
                element_active: hex(0x454a56ff),
                element_selected: hex(0x454a56ff),
                ghost_element_hover: hex(0x363c46ff),
                ghost_element_active: hex(0x454a56ff),
                ghost_element_selected: hex(0x454a56ff),
                border: hex(0x464b57ff),
                border_variant: hex(0x363c46ff),
                border_focused: hex(0x47679eff),
                text: hex(0xdce0e5ff),
                text_muted: hex(0xa9afbcff),
                text_placeholder: hex(0x878a98ff),
                text_disabled: hex(0x878a98ff),
                text_accent: hex(0x74ade8ff),
                icon: hex(0xdce0e5ff),
                icon_muted: hex(0xa9afbcff),
                icon_disabled: hex(0x878a98ff),
                icon_accent: hex(0x74ade8ff),
                error: hex(0xd07277ff),
                error_background: hex(0xd0727726),
                warning: hex(0xdec184ff),
                warning_background: hex(0xdec18426),
                success: hex(0xa1c181ff),
                success_background: hex(0xa1c18126),
                info: hex(0x74ade8ff),
                info_background: hex(0x74ade826),
                code_block_background: hex(0x282c33ff),
                syntax: SyntaxColors::github_dark(),
            },
        }
    }
}

pub trait ActiveTheme {
    fn theme(&self) -> &Theme;
}

impl ActiveTheme for App {
    fn theme(&self) -> &Theme {
        self.global::<Theme>()
    }
}

/// Semantic text and icon colors, as in Zed's `ui::Color`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Color {
    #[default]
    Default,
    Muted,
    Placeholder,
    Disabled,
    Accent,
    Error,
    Warning,
    Success,
}

impl Color {
    pub fn text(self, cx: &App) -> Hsla {
        let colors = &cx.theme().colors;
        match self {
            Color::Default => colors.text,
            Color::Muted => colors.text_muted,
            Color::Placeholder => colors.text_placeholder,
            Color::Disabled => colors.text_disabled,
            Color::Accent => colors.text_accent,
            Color::Error => colors.error,
            Color::Warning => colors.warning,
            Color::Success => colors.success,
        }
    }

    pub fn icon(self, cx: &App) -> Hsla {
        let colors = &cx.theme().colors;
        match self {
            Color::Default => colors.icon,
            Color::Muted => colors.icon_muted,
            Color::Placeholder | Color::Disabled => colors.icon_disabled,
            Color::Accent => colors.icon_accent,
            _ => self.text(cx),
        }
    }
}

fn hex(value: u32) -> Hsla {
    rgba(value).into()
}
