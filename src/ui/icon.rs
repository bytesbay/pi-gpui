use std::borrow::Cow;
use std::time::Duration;

use gpui::{
    App, AssetSource, Pixels, SharedString, Transformation, Window, percentage, prelude::*, px, svg,
};

use super::animation::phase;
use super::theme::Color;

const SPINNER_PERIOD: Duration = Duration::from_millis(900);

macro_rules! icons {
    ($($variant:ident => $file:literal),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum IconName { $($variant),* }

        impl IconName {
            pub fn path(self) -> &'static str {
                match self { $(IconName::$variant => concat!("icons/", $file, ".svg")),* }
            }
        }

        /// Icons are compiled into the binary; there is nothing to install next to it.
        pub struct Assets;

        impl AssetSource for Assets {
            fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
                Ok(match path {
                    $(concat!("icons/", $file, ".svg") => Some(Cow::Borrowed(
                        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/", $file, ".svg")).as_slice(),
                    )),)*
                    _ => None,
                })
            }

            fn list(&self, _path: &str) -> gpui::Result<Vec<SharedString>> {
                Ok(vec![$(SharedString::from(concat!("icons/", $file, ".svg"))),*])
            }
        }
    };
}

icons! {
    ArrowUp => "arrow_up",
    Archive => "archive",
    Check => "check",
    ChevronDown => "chevron_down",
    ChevronRight => "chevron_right",
    Close => "close",
    Compact => "compact",
    Copy => "copy",
    Dash => "dash",
    Diff => "diff",
    Ellipsis => "ellipsis",
    Eye => "eye",
    EyeOff => "eye_off",
    File => "file",
    Folder => "folder",
    Fork => "fork",
    FolderOpen => "folder_open",
    Info => "info",
    LoadCircle => "load_circle",
    ListTree => "list_tree",
    Message => "message",
    Microphone => "microphone",
    PanelLeft => "panel_left",
    PanelRight => "panel_right",
    Pencil => "pencil",
    Plus => "plus",
    Refresh => "refresh",
    Sparkle => "sparkle",
    Stop => "stop",
    Terminal => "terminal",
    Tool => "tool",
    Trash => "trash",
    Warning => "warning",
}

impl IconName {
    /// Names plugins may use in their manifests (see docs/PLUGINS.md).
    pub fn from_plugin_name(name: &str) -> Option<Self> {
        Some(match name {
            "microphone" => IconName::Microphone,
            "terminal" => IconName::Terminal,
            "file" => IconName::File,
            "sparkle" => IconName::Sparkle,
            "message" => IconName::Message,
            "tool" => IconName::Tool,
            "folder" => IconName::Folder,
            "refresh" => IconName::Refresh,
            "check" => IconName::Check,
            "info" => IconName::Info,
            "warning" => IconName::Warning,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IconSize {
    XSmall,
    #[default]
    Small,
    Medium,
    Large,
}

impl IconSize {
    pub fn pixels(self) -> Pixels {
        match self {
            IconSize::XSmall => px(12.),
            IconSize::Small => px(14.),
            IconSize::Medium => px(16.),
            IconSize::Large => px(24.),
        }
    }
}

#[derive(IntoElement)]
pub struct Icon {
    name: IconName,
    size: IconSize,
    color: Color,
}

impl Icon {
    pub fn new(name: IconName) -> Self {
        Self {
            name,
            size: IconSize::default(),
            color: Color::default(),
        }
    }

    pub fn size(mut self, size: IconSize) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }
}

impl RenderOnce for Icon {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        svg()
            .path(self.name.path())
            .flex_none()
            .size(self.size.pixels())
            .text_color(self.color.icon(cx))
    }
}

/// A rotating loader. Its angle comes from wall time, so the owning view only
/// needs an active [`super::AnimationTicker`] to keep it turning.
#[derive(IntoElement)]
pub struct Spinner {
    size: IconSize,
    color: Color,
}

impl Spinner {
    pub fn new() -> Self {
        Self {
            size: IconSize::Small,
            color: Color::Muted,
        }
    }

    pub fn size(mut self, size: IconSize) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }
}

impl RenderOnce for Spinner {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        svg()
            .path(IconName::LoadCircle.path())
            .flex_none()
            .size(self.size.pixels())
            .text_color(self.color.icon(cx))
            .with_transformation(Transformation::rotate(percentage(phase(SPINNER_PERIOD))))
    }
}
