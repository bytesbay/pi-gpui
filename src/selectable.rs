//! Read-only text that can be selected with the mouse and copied: messages,
//! thoughts and tool output in the transcript. Plain GPUI text cannot be
//! selected, so this element shapes and paints its text itself.
//!
//! A selection lives inside one block (one message, one thought...). It is
//! stored in a shared [`TextSelection`] entity rather than in the element, so
//! it survives the virtualized list re-creating rows while scrolling.

use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, AvailableSpace, Bounds, Context, CursorStyle, DispatchPhase, ElementId, Entity,
    FocusHandle, FontStyle, FontWeight, GlobalElementId, Hitbox, HitboxBehavior, Hsla, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, SharedString,
    StrikethroughStyle, Style, TextAlign, TextRun, UnderlineStyle, Window, fill, point, prelude::*,
    px, relative, size,
};

use crate::text_input::{TextLayout, paragraph_range_at, word_range_at};
use crate::ui::{ActiveTheme as _, Color, LabelSize};

/// Inline formatting for a byte range of a [`SelectableText`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpanStyle {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    /// Syntax highlighting color.
    pub color: Option<Hsla>,
    /// Opened on a plain click (one that selects nothing).
    pub link: Option<SharedString>,
}

pub struct TextSelection {
    block: Option<SharedString>,
    text: SharedString,
    anchor: usize,
    head: usize,
    dragging: bool,
    /// Receives keyboard focus on selection, so Copy reaches the owning view.
    copy_focus: FocusHandle,
}

impl TextSelection {
    pub fn new(copy_focus: FocusHandle) -> Self {
        Self {
            block: None,
            text: SharedString::default(),
            anchor: 0,
            head: 0,
            dragging: false,
            copy_focus,
        }
    }

    pub fn selected_text(&self) -> Option<String> {
        let range = self.range();
        (!range.is_empty())
            .then(|| self.text.get(range).map(str::to_string))
            .flatten()
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        if self.block.take().is_some() {
            self.dragging = false;
            cx.notify();
        }
    }

    fn range(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    fn range_in(&self, key: &SharedString) -> Option<Range<usize>> {
        (self.block.as_ref() == Some(key)).then(|| self.range())
    }

    fn begin(&mut self, key: SharedString, text: SharedString, offset: usize, clicks: usize) {
        let range = match clicks {
            2 => word_range_at(&text, offset),
            3.. => paragraph_range_at(&text, offset),
            _ => offset..offset,
        };
        self.block = Some(key);
        self.text = text;
        self.anchor = range.start;
        self.head = range.end;
        self.dragging = clicks == 1;
    }
}

pub struct SelectableText {
    key: SharedString,
    text: SharedString,
    selection: Entity<TextSelection>,
    color: Option<Color>,
    size: Option<LabelSize>,
    /// Font size and line height, overriding `size`.
    text_size: Option<(Pixels, Pixels)>,
    weight: Option<FontWeight>,
    italic: bool,
    monospace: bool,
    font_family: Option<SharedString>,
    /// Lay out on one line per hard line break and report the natural width.
    no_wrap: bool,
    spans: Rc<[(Range<usize>, SpanStyle)]>,
}

impl SelectableText {
    /// `key` identifies the block across frames (e.g. `"12:text"`).
    pub fn new(
        key: impl Into<SharedString>,
        text: impl Into<SharedString>,
        selection: &Entity<TextSelection>,
    ) -> Self {
        Self {
            key: key.into(),
            text: text.into(),
            selection: selection.clone(),
            color: None,
            size: None,
            text_size: None,
            weight: None,
            italic: false,
            monospace: false,
            font_family: None,
            no_wrap: false,
            spans: Rc::from([]),
        }
    }

    pub fn font_family(mut self, family: impl Into<SharedString>) -> Self {
        self.font_family = Some(family.into());
        self
    }

    pub fn no_wrap(mut self) -> Self {
        self.no_wrap = true;
        self
    }

    /// Sorted, non-overlapping styled ranges.
    pub fn spans(mut self, spans: Vec<(Range<usize>, SpanStyle)>) -> Self {
        self.spans = spans.into();
        self
    }

    pub fn text_size(mut self, font_size: Pixels, line_height: Pixels) -> Self {
        self.text_size = Some((font_size, line_height));
        self
    }

    pub fn weight(mut self, weight: FontWeight) -> Self {
        self.weight = Some(weight);
        self
    }

    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    pub fn size(mut self, size: LabelSize) -> Self {
        self.size = Some(size);
        self
    }

    pub fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    pub fn monospace(mut self) -> Self {
        self.monospace = true;
        self
    }

    /// Runs, font size and line height derived from the inherited text style.
    fn text_params(&self, window: &Window, cx: &App) -> (Vec<TextRun>, Pixels, Pixels) {
        let mut style = window.text_style();
        if self.monospace {
            style.font_family = "Menlo".into();
        }
        if let Some(family) = &self.font_family {
            style.font_family = family.clone();
        }
        if self.italic {
            style.font_style = FontStyle::Italic;
        }
        if let Some(weight) = self.weight {
            style.font_weight = weight;
        }
        let (font_size, line_height) = match (self.text_size, self.size) {
            (Some(text_size), _) => text_size,
            (None, Some(size)) => (size.pixels(), size.line_height()),
            (None, None) => (
                style.font_size.to_pixels(window.rem_size()),
                window.line_height(),
            ),
        };
        let base = TextRun {
            len: 0,
            font: style.font(),
            color: self.color.map_or(style.color, |color| color.text(cx)),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let colors = &cx.theme().colors;
        let mut runs = Vec::with_capacity(self.spans.len() * 2 + 1);
        let mut position = 0;
        let len = self.text.len();
        for (range, span) in self.spans.iter() {
            let range = range.start.clamp(position, len)..range.end.clamp(position, len);
            if range.start > position {
                runs.push(TextRun {
                    len: range.start - position,
                    ..base.clone()
                });
            }
            if range.is_empty() {
                continue;
            }
            let mut run = TextRun {
                len: range.len(),
                ..base.clone()
            };
            // Inline styles follow Zed's agent markdown style.
            if let Some(color) = span.color {
                run.color = color;
            }
            if span.bold {
                run.font.weight = FontWeight::BOLD;
                run.color = colors.text;
            }
            if span.italic {
                run.font.style = FontStyle::Italic;
            }
            if span.code {
                run.font.family = "Menlo".into();
                run.background_color = Some(colors.text.opacity(0.08));
            }
            if span.strike {
                run.strikethrough = Some(StrikethroughStyle {
                    thickness: px(1.),
                    color: Some(run.color),
                });
            }
            if span.link.is_some() {
                run.color = colors.text_accent;
                run.background_color = Some(colors.text.opacity(0.025));
                run.underline = Some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(colors.text_accent.opacity(0.5)),
                    wavy: false,
                });
            }
            runs.push(run);
            position = range.end;
        }
        if position < len || runs.is_empty() {
            runs.push(TextRun {
                len: len - position,
                ..base
            });
        }
        (runs, font_size, line_height)
    }

    fn shape(&self, width: Option<Pixels>, window: &mut Window, cx: &App) -> Option<TextLayout> {
        let (runs, font_size, line_height) = self.text_params(window, cx);
        let lines = window
            .text_system()
            .shape_text(self.text.clone(), font_size, &runs, width, None)
            .ok()?;
        Some(TextLayout::new(lines.into_vec(), line_height))
    }
}

impl IntoElement for SelectableText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

pub struct PrepaintState {
    layout: Option<Rc<TextLayout>>,
    hitbox: Hitbox,
    selections: Vec<PaintQuad>,
}

impl Element for SelectableText {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let (runs, font_size, line_height) = self.text_params(window, cx);
        let text = self.text.clone();
        let no_wrap = self.no_wrap;
        let mut style = Style::default();
        if no_wrap {
            // Natural width; a scrolling parent shows what does not fit.
            style.flex_shrink = 0.;
        } else {
            style.size.width = relative(1.).into();
        }
        let layout_id =
            window.request_measured_layout(style, move |known, available, window, _cx| {
                let width = if no_wrap {
                    None
                } else {
                    known.width.or(match available.width {
                        AvailableSpace::Definite(width) => Some(width),
                        _ => None,
                    })
                };
                let lines = window
                    .text_system()
                    .shape_text(text.clone(), font_size, &runs, width, None)
                    .unwrap_or_default();
                let height = lines
                    .iter()
                    .map(|line| line.size(line_height).height)
                    .fold(px(0.), |total, height| total + height)
                    .max(line_height);
                let width = width.unwrap_or_else(|| {
                    lines
                        .iter()
                        .map(|line| line.size(line_height).width)
                        .fold(px(0.), Pixels::max)
                });
                size(width, height)
            });
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let wrap_width = (!self.no_wrap).then_some(bounds.size.width);
        let layout = self.shape(wrap_width, window, cx).map(Rc::new);
        let mut selections = Vec::new();
        if let (Some(layout), Some(range)) = (&layout, self.selection.read(cx).range_in(&self.key))
        {
            let range = range.start.min(self.text.len())..range.end.min(self.text.len());
            if !range.is_empty() {
                let start = layout.position_for_offset(range.start);
                let end = layout.position_for_offset(range.end);
                let color = cx.theme().colors.info.opacity(0.3);
                let line_height = layout.line_height;
                let mut row_top = start.y;
                while row_top <= end.y {
                    let left = if row_top == start.y { start.x } else { px(0.) };
                    let right = if row_top == end.y {
                        end.x
                    } else {
                        bounds.size.width
                    };
                    if right > left {
                        selections.push(fill(
                            Bounds::from_corners(
                                bounds.origin + point(left, row_top),
                                bounds.origin + point(right, row_top + line_height),
                            ),
                            color,
                        ));
                    }
                    row_top += line_height;
                }
            }
        }
        PrepaintState {
            layout,
            hitbox,
            selections,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.set_cursor_style(CursorStyle::IBeam, &prepaint.hitbox);
        let Some(layout) = prepaint.layout.clone() else {
            return;
        };
        for (line, top) in layout.lines.iter().zip(&layout.line_tops) {
            if !self.spans.is_empty() {
                line.paint_background(
                    bounds.origin + point(px(0.), *top),
                    layout.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
            }
            if let Err(error) = line.paint(
                bounds.origin + point(px(0.), *top),
                layout.line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            ) {
                eprintln!("pi-gpui: failed to paint text: {error}");
            }
        }

        // Over the text, so code backgrounds do not hide it.
        for selection in prepaint.selections.drain(..) {
            window.paint_quad(selection);
        }

        let origin = bounds.origin;
        let hitbox = prepaint.hitbox.clone();
        let selection = self.selection.clone();
        let key = self.key.clone();
        let text = self.text.clone();
        window.on_mouse_event({
            let layout = layout.clone();
            let selection = selection.clone();
            let key = key.clone();
            move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                    return;
                }
                if hitbox.is_hovered(window) {
                    let offset = layout.offset_for_position(event.position - origin);
                    let focus = selection.update(cx, |selection, cx| {
                        selection.begin(key.clone(), text.clone(), offset, event.click_count);
                        cx.notify();
                        selection.copy_focus.clone()
                    });
                    window.focus(&focus);
                } else {
                    // A click elsewhere ends this block's selection. If the click
                    // started a selection in another block, that block owns it now.
                    selection.update(cx, |selection, cx| {
                        if selection.block.as_ref() == Some(&key) {
                            selection.clear(cx);
                        }
                    });
                }
            }
        });
        window.on_mouse_event({
            let layout = layout.clone();
            let selection = selection.clone();
            let key = key.clone();
            move |event: &MouseMoveEvent, phase, _window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let dragging_here = {
                    let selection = selection.read(cx);
                    selection.dragging && selection.block.as_ref() == Some(&key)
                };
                if dragging_here {
                    let offset = layout.offset_for_position(event.position - origin);
                    selection.update(cx, |selection, cx| {
                        if selection.head != offset {
                            selection.head = offset;
                            cx.notify();
                        }
                    });
                }
            }
        });
        let links: Vec<(Range<usize>, SharedString)> = self
            .spans
            .iter()
            .filter_map(|(range, span)| Some((range.clone(), span.link.clone()?)))
            .collect();
        let hitbox = prepaint.hitbox.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let clicked = selection.update(cx, |selection, _| {
                let owned = selection.block.as_ref() == Some(&key);
                let clicked = owned && selection.dragging && selection.range().is_empty();
                if owned {
                    selection.dragging = false;
                }
                clicked
            });
            // A click that selected nothing follows a link under the pointer.
            if clicked && !links.is_empty() && hitbox.is_hovered(window) {
                let offset = layout.offset_for_position(event.position - origin);
                if let Some((_, url)) = links.iter().find(|(range, _)| range.contains(&offset)) {
                    cx.open_url(url);
                }
            }
        });
    }
}
