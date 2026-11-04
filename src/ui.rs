//! Small component library modeled on Zed's `ui` crate: semantic theme
//! tokens, `RenderOnce` builders and consistent interaction states.
//!
//! Components expose their full builder API (all sizes, styles, states) even
//! where the screens do not use every option yet.
#![allow(dead_code)]

mod animation;
mod button;
mod icon;
mod label;
mod list_item;
pub mod pi_gui_theme;
mod theme;

pub use animation::AnimationTicker;
pub use button::{Button, ButtonSize, ButtonStyle, IconButton, TintColor};
pub use icon::{Assets, Icon, IconName, IconSize, Spinner};
pub use label::{Label, LabelSize, LoadingLabel};
pub use list_item::{Indicator, KeyHint, ListItem, Tooltip};
pub use theme::{ActiveTheme, Color, SyntaxColors, Theme};

use gpui::{Div, div, prelude::*};

/// Horizontal stack with centered items, like Zed's `h_flex`.
pub fn h_flex() -> Div {
    div().flex().flex_row().items_center()
}

/// Vertical stack, like Zed's `v_flex`.
pub fn v_flex() -> Div {
    div().flex().flex_col()
}

pub mod prelude {
    pub use super::{
        ActiveTheme as _, Button, ButtonSize, ButtonStyle, Color, Icon, IconButton, IconName,
        IconSize, Indicator, KeyHint, Label, LabelSize, ListItem, LoadingLabel, Spinner, TintColor,
        Tooltip, h_flex, v_flex,
    };
    pub use gpui::prelude::*;
}
