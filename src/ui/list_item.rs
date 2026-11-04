use gpui::{
    AnyElement, AnyView, App, ClickEvent, ElementId, SharedString, Window, div, prelude::*, px,
};

use super::label::{Label, LabelSize};
use super::theme::{ActiveTheme as _, Color};

const ROW_HEIGHT: f32 = 28.;
const INDENT_WIDTH: f32 = 14.;

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;
type TooltipBuilder = Box<dyn Fn(&mut Window, &mut App) -> AnyView + 'static>;

/// A clickable row with slots, like Zed's `ui::ListItem`.
#[derive(IntoElement)]
pub struct ListItem {
    id: ElementId,
    indent_level: usize,
    selected: bool,
    start_slot: Option<AnyElement>,
    end_slot: Option<AnyElement>,
    end_hover_slot: Option<AnyElement>,
    children: Vec<AnyElement>,
    on_click: Option<ClickHandler>,
    tooltip: Option<TooltipBuilder>,
}

impl ListItem {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            indent_level: 0,
            selected: false,
            start_slot: None,
            end_slot: None,
            end_hover_slot: None,
            children: Vec::new(),
            on_click: None,
            tooltip: None,
        }
    }

    pub fn indent_level(mut self, level: usize) -> Self {
        self.indent_level = level;
        self
    }

    pub fn toggle_state(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn start_slot(mut self, slot: impl IntoElement) -> Self {
        self.start_slot = Some(slot.into_any_element());
        self
    }

    pub fn end_slot(mut self, slot: impl IntoElement) -> Self {
        self.end_slot = Some(slot.into_any_element());
        self
    }

    /// Shown only while the row is hovered, e.g. row actions.
    pub fn end_hover_slot(mut self, slot: impl IntoElement) -> Self {
        self.end_hover_slot = Some(slot.into_any_element());
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    pub fn tooltip(mut self, tooltip: impl Fn(&mut Window, &mut App) -> AnyView + 'static) -> Self {
        self.tooltip = Some(Box::new(tooltip));
        self
    }
}

impl ParentElement for ListItem {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for ListItem {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let group: SharedString = format!("list-item:{}", self.id).into();
        let clickable = self.on_click.is_some();

        div()
            .id(self.id)
            .group(group.clone())
            .flex()
            .items_center()
            .gap(px(6.))
            .w_full()
            .h(px(ROW_HEIGHT))
            .pl(px(6. + self.indent_level as f32 * INDENT_WIDTH))
            .pr(px(4.))
            .rounded(px(5.))
            .when(self.selected, |this| this.bg(colors.ghost_element_selected))
            .when(!self.selected, |this| {
                this.hover(|style| style.bg(colors.ghost_element_hover))
            })
            .when(clickable, |this| {
                this.cursor_pointer()
                    .active(|style| style.bg(colors.ghost_element_active))
            })
            .when_some(self.on_click, |this, on_click| this.on_click(on_click))
            .when_some(self.tooltip, |this, tooltip| {
                this.tooltip(move |window, cx| tooltip(window, cx))
            })
            .when_some(self.start_slot, |this, slot| {
                this.child(div().flex().flex_none().items_center().child(slot))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .children(self.children),
            )
            .map(|this| match (self.end_slot, self.end_hover_slot) {
                (Some(slot), None) => {
                    this.child(div().flex().flex_none().items_center().child(slot))
                }
                // gpui 0.2.2 has no `invisible()`; transparency keeps the layout stable.
                (None, Some(hover)) => this.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .opacity(0.)
                        .group_hover(group, |style| style.opacity(1.))
                        .child(hover),
                ),
                // On hover the hover slot takes the end slot's place, as in Zed.
                // The hover slot keeps its room; the end slot sits on top of it.
                (Some(slot), Some(hover)) => this.child(
                    div()
                        .relative()
                        .flex()
                        .flex_none()
                        .items_center()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .opacity(0.)
                                .group_hover(group.clone(), |style| style.opacity(1.))
                                .child(hover),
                        )
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .right_0()
                                .flex()
                                .items_center()
                                .group_hover(group, |style| style.opacity(0.))
                                .child(slot),
                        ),
                ),
                (None, None) => this,
            })
    }
}

/// A small status dot.
#[derive(IntoElement)]
pub struct Indicator {
    color: Color,
}

impl Indicator {
    pub fn dot(color: Color) -> Self {
        Self { color }
    }
}

impl RenderOnce for Indicator {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .flex_none()
            .size(px(6.))
            .rounded_full()
            .bg(self.color.icon(cx))
    }
}

/// A keyboard shortcut chip, e.g. `⌘N`.
#[derive(IntoElement)]
pub struct KeyHint {
    keys: SharedString,
}

impl KeyHint {
    pub fn new(keys: impl Into<SharedString>) -> Self {
        Self { keys: keys.into() }
    }
}

impl RenderOnce for KeyHint {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = &cx.theme().colors;
        div()
            .flex_none()
            .px(px(4.))
            .rounded(px(3.))
            .border_1()
            .border_color(colors.border_variant)
            .bg(colors.element_background)
            .child(
                Label::new(self.keys)
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
    }
}

pub struct Tooltip {
    title: SharedString,
    keys: Option<SharedString>,
}

impl Tooltip {
    pub fn text(title: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView {
        Self::build(title.into(), None)
    }

    pub fn with_keys(
        title: impl Into<SharedString>,
        keys: impl Into<SharedString>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView {
        Self::build(title.into(), Some(keys.into()))
    }

    /// A tooltip with an optional shortcut, for when the shortcut is dynamic.
    pub fn with_optional_keys(
        title: impl Into<SharedString>,
        keys: Option<SharedString>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView {
        Self::build(title.into(), keys)
    }

    fn build(
        title: SharedString,
        keys: Option<SharedString>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView {
        move |_, cx| {
            let tooltip = Tooltip {
                title: title.clone(),
                keys: keys.clone(),
            };
            cx.new(|_| tooltip).into()
        }
    }
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        // Tooltips render in their own layer: offset from the cursor.
        div().pl(px(8.)).pt(px(10.)).child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(8.))
                .py(px(4.))
                .rounded(px(6.))
                .border_1()
                .border_color(colors.border_variant)
                .bg(colors.elevated_surface_background)
                .shadow_md()
                .child(Label::new(self.title.clone()).size(LabelSize::Small))
                .when_some(self.keys.clone(), |this, keys| {
                    this.child(KeyHint::new(keys))
                }),
        )
    }
}
