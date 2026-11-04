use std::time::Duration;

use gpui::{App, FontWeight, Pixels, SharedString, Window, div, prelude::*, pulsating_between, px};

use super::animation::phase;
use super::theme::Color;

const PULSE_PERIOD: Duration = Duration::from_millis(1600);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LabelSize {
    Large,
    #[default]
    Default,
    Small,
    XSmall,
}

impl LabelSize {
    pub fn pixels(self) -> Pixels {
        match self {
            LabelSize::Large => px(16.),
            LabelSize::Default => px(14.),
            LabelSize::Small => px(12.),
            LabelSize::XSmall => px(11.),
        }
    }

    pub(crate) fn line_height(self) -> Pixels {
        match self {
            LabelSize::Large => px(22.),
            LabelSize::Default => px(20.),
            LabelSize::Small => px(17.),
            LabelSize::XSmall => px(15.),
        }
    }
}

/// Text with a semantic size and color, like Zed's `ui::Label`.
#[derive(IntoElement)]
pub struct Label {
    text: SharedString,
    size: LabelSize,
    color: Color,
    weight: Option<FontWeight>,
    italic: bool,
    truncate: bool,
    monospace: bool,
}

impl Label {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            size: LabelSize::default(),
            color: Color::default(),
            weight: None,
            italic: false,
            truncate: false,
            monospace: false,
        }
    }

    pub fn size(mut self, size: LabelSize) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    pub fn weight(mut self, weight: FontWeight) -> Self {
        self.weight = Some(weight);
        self
    }

    pub fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    /// Single line with an ellipsis. The parent must allow shrinking (`min_w(px(0.))`).
    pub fn truncate(mut self) -> Self {
        self.truncate = true;
        self
    }

    pub fn monospace(mut self) -> Self {
        self.monospace = true;
        self
    }
}

impl RenderOnce for Label {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .text_size(self.size.pixels())
            .line_height(self.size.line_height())
            .text_color(self.color.text(cx))
            .when_some(self.weight, |this, weight| this.font_weight(weight))
            .when(self.italic, |this| this.italic())
            .when(self.monospace, |this| this.font_family("Menlo"))
            .when(self.truncate, |this| this.min_w(px(0.)).truncate())
            .child(self.text)
    }
}

/// A label that breathes while something is in progress. Like [`super::Spinner`],
/// it needs the owning view's [`super::AnimationTicker`] to move.
#[derive(IntoElement)]
pub struct LoadingLabel {
    label: Label,
}

impl LoadingLabel {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            label: Label::new(text).color(Color::Muted),
        }
    }

    pub fn size(mut self, size: LabelSize) -> Self {
        self.label = self.label.size(size);
        self
    }

    pub fn color(mut self, color: Color) -> Self {
        self.label = self.label.color(color);
        self
    }
}

impl RenderOnce for LoadingLabel {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let opacity = pulsating_between(0.45, 1.)(phase(PULSE_PERIOD));
        div().opacity(opacity).child(self.label)
    }
}
