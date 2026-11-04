use gpui::{
    AnyView, App, ClickEvent, ElementId, Hsla, MouseButton, Pixels, SharedString, Window, div,
    prelude::*, px,
};

use super::icon::{Icon, IconName, IconSize, Spinner};
use super::label::{Label, LabelSize};
use super::theme::{ActiveTheme as _, Color};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonStyle {
    /// Visible background at rest. For primary actions in a toolbar or form.
    Filled,
    /// Background only on hover. The default, like Zed.
    #[default]
    Subtle,
    /// A colored background for the one action that matters most here.
    Tinted(TintColor),
    Outlined,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TintColor {
    Accent,
    Error,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonSize {
    Large,
    Medium,
    #[default]
    Default,
    Compact,
}

impl ButtonSize {
    fn height(self) -> Pixels {
        match self {
            ButtonSize::Large => px(32.),
            ButtonSize::Medium => px(28.),
            ButtonSize::Default => px(22.),
            ButtonSize::Compact => px(18.),
        }
    }
}

struct StyleColors {
    background: Hsla,
    hover: Hsla,
    active: Hsla,
    border: Option<Hsla>,
}

impl ButtonStyle {
    fn colors(self, cx: &App) -> StyleColors {
        let colors = &cx.theme().colors;
        let transparent = gpui::transparent_black();
        match self {
            ButtonStyle::Filled => StyleColors {
                background: colors.element_background,
                hover: colors.element_hover,
                active: colors.element_active,
                border: None,
            },
            ButtonStyle::Subtle => StyleColors {
                background: transparent,
                hover: colors.ghost_element_hover,
                active: colors.ghost_element_active,
                border: None,
            },
            ButtonStyle::Outlined => StyleColors {
                background: transparent,
                hover: colors.ghost_element_hover,
                active: colors.ghost_element_active,
                border: Some(colors.border),
            },
            // Themes define status backgrounds as opaque (Ayu) or translucent
            // (One Dark); mixing in the accent works for both.
            ButtonStyle::Tinted(TintColor::Accent) => StyleColors {
                background: colors.info_background,
                hover: colors.info_background.blend(colors.info.opacity(0.15)),
                active: colors.info_background.blend(colors.info.opacity(0.28)),
                border: Some(colors.info.opacity(0.35)),
            },
            ButtonStyle::Tinted(TintColor::Error) => StyleColors {
                background: colors.error_background,
                hover: colors.error_background.blend(colors.error.opacity(0.15)),
                active: colors.error_background.blend(colors.error.opacity(0.28)),
                border: Some(colors.error.opacity(0.35)),
            },
        }
    }
}

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;
type TooltipBuilder = Box<dyn Fn(&mut Window, &mut App) -> AnyView + 'static>;

/// Shared behavior of every button: states, click handling and tooltip.
/// Like Zed's `ButtonLike`, so `Button` and `IconButton` stay thin.
struct ButtonLike {
    id: ElementId,
    style: ButtonStyle,
    size: ButtonSize,
    disabled: bool,
    selected: bool,
    loading: bool,
    square: bool,
    full_width: bool,
    on_click: Option<ClickHandler>,
    tooltip: Option<TooltipBuilder>,
}

impl ButtonLike {
    fn new(id: ElementId) -> Self {
        Self {
            id,
            style: ButtonStyle::default(),
            size: ButtonSize::default(),
            disabled: false,
            selected: false,
            loading: false,
            square: false,
            full_width: false,
            on_click: None,
            tooltip: None,
        }
    }

    fn foreground(&self, fallback: Color) -> Color {
        if self.disabled {
            Color::Disabled
        } else {
            self.style_foreground().unwrap_or(fallback)
        }
    }

    fn style_foreground(&self) -> Option<Color> {
        match self.style {
            ButtonStyle::Tinted(TintColor::Accent) => Some(Color::Accent),
            ButtonStyle::Tinted(TintColor::Error) => Some(Color::Error),
            _ => None,
        }
    }

    fn render(self, children: Vec<gpui::AnyElement>, cx: &App) -> impl IntoElement + use<> {
        let colors = self.style.colors(cx);
        let theme = &cx.theme().colors;
        let height = self.size.height();
        let interactive = !self.disabled && !self.loading;
        let background = if self.selected {
            theme.element_selected
        } else {
            colors.background
        };

        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(px(4.))
            .h(height)
            .rounded(px(5.))
            .bg(background)
            .when(self.square, |this| this.w(height))
            .when(!self.square, |this| this.px(px(8.)))
            .when(self.full_width, |this| this.w_full())
            .when_some(colors.border, |this, border| {
                this.border_1().border_color(border)
            })
            .when(interactive, |this| {
                this.cursor_pointer()
                    .hover(|style| style.bg(colors.hover))
                    .active(|style| style.bg(colors.active))
            })
            .when(self.disabled, |this| this.cursor_not_allowed())
            .when_some(self.on_click.filter(|_| interactive), |this, on_click| {
                // Keep focus where it is (e.g. in the composer) when pressing a button.
                this.on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .on_click(move |event, window, cx| {
                        cx.stop_propagation();
                        on_click(event, window, cx);
                    })
            })
            .when_some(self.tooltip, |this, tooltip| {
                this.tooltip(move |window, cx| tooltip(window, cx))
            })
            .children(children)
    }
}

macro_rules! button_builder {
    () => {
        pub fn style(mut self, style: ButtonStyle) -> Self {
            self.base.style = style;
            self
        }

        pub fn size(mut self, size: ButtonSize) -> Self {
            self.base.size = size;
            self
        }

        pub fn disabled(mut self, disabled: bool) -> Self {
            self.base.disabled = disabled;
            self
        }

        pub fn toggle_state(mut self, selected: bool) -> Self {
            self.base.selected = selected;
            self
        }

        /// Shows a spinner and ignores clicks while work is in flight.
        pub fn loading(mut self, loading: bool) -> Self {
            self.base.loading = loading;
            self
        }

        pub fn on_click(
            mut self,
            handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        ) -> Self {
            self.base.on_click = Some(Box::new(handler));
            self
        }

        pub fn tooltip(
            mut self,
            tooltip: impl Fn(&mut Window, &mut App) -> AnyView + 'static,
        ) -> Self {
            self.base.tooltip = Some(Box::new(tooltip));
            self
        }
    };
}

#[derive(IntoElement)]
pub struct Button {
    base: ButtonLike,
    label: SharedString,
    icon: Option<IconName>,
    label_size: LabelSize,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            base: ButtonLike::new(id.into()),
            label: label.into(),
            icon: None,
            label_size: LabelSize::Small,
        }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn full_width(mut self) -> Self {
        self.base.full_width = true;
        self
    }

    button_builder!();
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = self.base.foreground(Color::Default);
        let mut children = Vec::new();
        if self.base.loading {
            children.push(
                Spinner::new()
                    .size(IconSize::XSmall)
                    .color(color)
                    .into_any_element(),
            );
        } else if let Some(icon) = self.icon {
            children.push(
                Icon::new(icon)
                    .size(IconSize::XSmall)
                    .color(color)
                    .into_any_element(),
            );
        }
        children.push(
            Label::new(self.label)
                .size(self.label_size)
                .color(color)
                .into_any_element(),
        );
        self.base.render(children, cx)
    }
}

#[derive(IntoElement)]
pub struct IconButton {
    base: ButtonLike,
    icon: IconName,
    icon_size: IconSize,
    icon_color: Color,
}

impl IconButton {
    pub fn new(id: impl Into<ElementId>, icon: IconName) -> Self {
        let mut base = ButtonLike::new(id.into());
        base.square = true;
        Self {
            base,
            icon,
            icon_size: IconSize::Small,
            icon_color: Color::Muted,
        }
    }

    pub fn icon_size(mut self, size: IconSize) -> Self {
        self.icon_size = size;
        self
    }

    pub fn icon_color(mut self, color: Color) -> Self {
        self.icon_color = color;
        self
    }

    button_builder!();
}

impl RenderOnce for IconButton {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = self.base.foreground(self.icon_color);
        let child = if self.base.loading {
            Spinner::new()
                .size(self.icon_size)
                .color(color)
                .into_any_element()
        } else {
            Icon::new(self.icon)
                .size(self.icon_size)
                .color(color)
                .into_any_element()
        };
        self.base.render(vec![child], cx)
    }
}
