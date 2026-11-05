//! Markdown for assistant answers, styled after Zed's agent panel
//! (`MarkdownStyle::themed(MarkdownFont::Agent)` and the default renderer in
//! Zed's `markdown` crate). The source is parsed once per change into a small
//! tree; every text block renders as one selectable text element.

use std::ops::Range;

use gpui::{
    AnyElement, App, ClipboardItem, ElementId, Entity, FontWeight, Hsla, Pixels, SharedString,
    TextRun, Window, div, px,
};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::selectable::{SelectableText, SpanStyle, TextSelection};
use crate::settings::Settings;
use crate::ui::prelude::*;

/// Text plus styled byte ranges (sorted, not overlapping).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inline {
    pub text: String,
    pub spans: Vec<(Range<usize>, SpanStyle)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    /// `tight`: text of a tight list item, which has no paragraph around it.
    Paragraph {
        inline: Inline,
        tight: bool,
    },
    Heading {
        level: u8,
        inline: Inline,
    },
    Code {
        language: Option<String>,
        text: String,
    },
    Rule,
    Quote(Vec<Node>),
    /// `start` is the first number of an ordered list.
    List {
        start: Option<u64>,
        items: Vec<Item>,
    },
    /// Rows of cells; the first row is the header when `header` is set.
    Table {
        rows: Vec<Vec<Inline>>,
        header: bool,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    /// `Some` for task list items: whether the box is checked.
    pub task: Option<bool>,
    pub children: Vec<Node>,
}

pub fn parse(source: &str) -> Vec<Node> {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut builder = Builder {
        stack: vec![Frame::Root(Vec::new())],
        ..Builder::default()
    };
    for event in Parser::new_ext(source, options) {
        builder.event(event);
    }
    builder.finish()
}

enum Frame {
    Root(Vec<Node>),
    Quote(Vec<Node>),
    List(Option<u64>, Vec<Item>),
    Item(Item),
}

enum Open {
    Paragraph { tight: bool },
    Heading(u8),
    Code(Option<String>),
}

#[derive(Default)]
struct Builder {
    stack: Vec<Frame>,
    inline: Inline,
    open: Option<Open>,
    bold: usize,
    italic: usize,
    strike: usize,
    links: Vec<SharedString>,
    table: Option<(Vec<Vec<Inline>>, bool)>,
}

impl Builder {
    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.push_text(&text, false),
            Event::Code(text) | Event::InlineMath(text) | Event::DisplayMath(text) => {
                self.push_text(&text, true)
            }
            Event::Html(text) | Event::InlineHtml(text) => self.push_text(&text, false),
            Event::FootnoteReference(name) => self.push_text(&format!("[{name}]"), false),
            // Like Zed's agent panel: a single newline in an answer is a line break.
            Event::SoftBreak | Event::HardBreak => self.push_text("\n", false),
            Event::Rule => {
                self.flush();
                self.push_node(Node::Rule);
            }
            Event::TaskListMarker(checked) => {
                if let Some(Frame::Item(item)) = self.stack.last_mut() {
                    item.task = Some(checked);
                }
            }
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {
                self.flush();
                self.open = Some(Open::Paragraph { tight: false });
            }
            Tag::Heading { level, .. } => {
                self.flush();
                self.open = Some(Open::Heading(heading_level(level)));
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .map(str::to_string)
                        .filter(|language| !language.is_empty()),
                    CodeBlockKind::Indented => None,
                };
                self.open = Some(Open::Code(language));
            }
            Tag::HtmlBlock => {
                self.flush();
                self.open = Some(Open::Code(None));
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.stack.push(Frame::Quote(Vec::new()));
            }
            Tag::List(start) => {
                self.flush();
                self.stack.push(Frame::List(start, Vec::new()));
            }
            Tag::Item => {
                self.flush();
                self.stack.push(Frame::Item(Item::default()));
            }
            Tag::Table(_) => {
                self.flush();
                self.table = Some((Vec::new(), false));
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some((rows, _)) = &mut self.table {
                    rows.push(Vec::new());
                }
            }
            Tag::TableCell => self.inline = Inline::default(),
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                self.links.push(SharedString::from(dest_url.to_string()))
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::HtmlBlock => {
                self.flush()
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                if let Some(Frame::Quote(children)) = self.stack.pop() {
                    self.push_node(Node::Quote(children));
                }
            }
            TagEnd::List(_) => {
                self.flush();
                if let Some(Frame::List(start, items)) = self.stack.pop() {
                    self.push_node(Node::List { start, items });
                }
            }
            TagEnd::Item => {
                self.flush();
                if let Some(Frame::Item(item)) = self.stack.pop()
                    && let Some(Frame::List(_, items)) = self.stack.last_mut()
                {
                    items.push(item);
                }
            }
            TagEnd::TableHead => {
                if let Some((_, header)) = &mut self.table {
                    *header = true;
                }
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.inline);
                if let Some((rows, _)) = &mut self.table
                    && let Some(row) = rows.last_mut()
                {
                    row.push(cell);
                }
            }
            TagEnd::Table => {
                if let Some((rows, header)) = self.table.take() {
                    self.push_node(Node::Table { rows, header });
                }
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link | TagEnd::Image => {
                self.links.pop();
            }
            _ => {}
        }
    }

    fn push_text(&mut self, text: &str, code: bool) {
        if text.is_empty() {
            return;
        }
        if self.open.is_none() && self.table.is_none() {
            // Tight list items have text without a paragraph around it.
            self.open = Some(Open::Paragraph { tight: true });
        }
        let style = if matches!(self.open, Some(Open::Code(_))) {
            SpanStyle::default()
        } else {
            SpanStyle {
                bold: self.bold > 0,
                italic: self.italic > 0,
                code,
                strike: self.strike > 0,
                color: None,
                link: self.links.last().cloned(),
            }
        };
        let start = self.inline.text.len();
        self.inline.text.push_str(text);
        let end = self.inline.text.len();
        if style == SpanStyle::default() {
            return;
        }
        match self.inline.spans.last_mut() {
            Some((range, last)) if range.end == start && *last == style => range.end = end,
            _ => self.inline.spans.push((start..end, style)),
        }
    }

    /// Ends the open text block, if any.
    fn flush(&mut self) {
        let Some(open) = self.open.take() else {
            return;
        };
        let inline = std::mem::take(&mut self.inline);
        let node = match open {
            Open::Paragraph { tight } => Node::Paragraph { inline, tight },
            Open::Heading(level) => Node::Heading { level, inline },
            Open::Code(language) => Node::Code {
                language,
                text: inline.text.trim_end_matches('\n').to_string(),
            },
        };
        self.push_node(node);
    }

    fn push_node(&mut self, node: Node) {
        match self.stack.last_mut() {
            Some(Frame::Root(children) | Frame::Quote(children)) => children.push(node),
            Some(Frame::Item(item)) => item.children.push(node),
            // Lists hold items only; anything else is malformed and dropped.
            Some(Frame::List(..)) | None => {}
        }
    }

    /// Closes whatever is still open: a streaming answer often ends mid-block.
    fn finish(mut self) -> Vec<Node> {
        self.flush();
        if let Some((rows, header)) = self.table.take() {
            self.push_node(Node::Table { rows, header });
        }
        while self.stack.len() > 1 {
            match self.stack.pop() {
                Some(Frame::Quote(children)) => self.push_node(Node::Quote(children)),
                Some(Frame::List(start, items)) => self.push_node(Node::List { start, items }),
                Some(Frame::Item(item)) => {
                    if let Some(Frame::List(_, items)) = self.stack.last_mut() {
                        items.push(item);
                    }
                }
                _ => {}
            }
        }
        match self.stack.pop() {
            Some(Frame::Root(children)) => children,
            _ => Vec::new(),
        }
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

// Spacing from Zed's agent markdown style. Text sizes come from the
// settings (`agent_font_size`, `buffer_font_*`), with Zed's proportions.
const PARAGRAPH_SPACING: f32 = 8.;

/// Renders parsed markdown. `key` must be unique and stable per message: it
/// identifies text blocks for selection and element ids.
pub fn render(
    key: &str,
    nodes: &[Node],
    selection: &Entity<TextSelection>,
    window: &mut Window,
    cx: &App,
) -> AnyElement {
    let settings = Settings::get(cx);
    let renderer = Renderer {
        selection,
        cx,
        body: settings.agent_font_size,
        body_line: settings.agent_line_height(),
        code: settings.buffer_font_size,
        code_line: settings.buffer_line_height(),
        code_font: settings.buffer_font_family.clone(),
    };
    let last = nodes.len().saturating_sub(1);
    v_flex()
        .w_full()
        .text_size(px(renderer.body))
        .line_height(px(renderer.body_line))
        .children(nodes.iter().enumerate().map(|(index, node)| {
            // The message row has its own spacing: drop the outer margins.
            let edge = Edges {
                first: index == 0,
                last: index == last,
            };
            renderer.node(&format!("{key}:{index}"), node, edge, window)
        }))
        .into_any_element()
}

#[derive(Clone, Copy, Default)]
struct Edges {
    first: bool,
    last: bool,
}

struct Renderer<'a> {
    selection: &'a Entity<TextSelection>,
    cx: &'a App,
    body: f32,
    body_line: f32,
    code: f32,
    code_line: f32,
    code_font: SharedString,
}

impl Renderer<'_> {
    fn text(&self, key: String, inline: &Inline) -> SelectableText {
        SelectableText::new(key, inline.text.clone(), self.selection)
            .spans(inline.spans.clone())
            .text_size(px(self.body), px(self.body_line))
    }

    fn children(&self, key: &str, nodes: &[Node], window: &mut Window) -> Vec<AnyElement> {
        nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                self.node(&format!("{key}.{index}"), node, Edges::default(), window)
            })
            .collect()
    }

    fn node(&self, key: &str, node: &Node, edges: Edges, window: &mut Window) -> AnyElement {
        let colors = &self.cx.theme().colors;
        let top = |margin: f32| if edges.first { px(0.) } else { px(margin) };
        let bottom = |margin: f32| if edges.last { px(0.) } else { px(margin) };
        match node {
            Node::Paragraph { inline, tight } => div()
                .when(!tight, |this| this.mb(bottom(PARAGRAPH_SPACING)))
                .child(self.text(key.to_string(), inline))
                .into_any_element(),
            Node::Heading { level, inline } => {
                // Zed's agent heading sizes: 1.15, 1.1, 1.05, 1, 0.95, 0.875 of the body.
                let scale = [1.15, 1.1, 1.05, 1., 0.95, 0.875][usize::from(level - 1).min(5)];
                let size = self.body * scale;
                div()
                    .mt(top(if *level == 1 { 16. } else { 24. }))
                    .mb(bottom(8.))
                    .child(
                        self.text(key.to_string(), inline)
                            .text_size(px(size), px(self.body_line.max(size * 1.3))),
                    )
                    .into_any_element()
            }
            Node::Code { language, text } => self.code_block(key, text, language.as_deref(), edges),
            Node::Rule => div()
                .mt(top(PARAGRAPH_SPACING))
                .mb(bottom(PARAGRAPH_SPACING))
                .border_b_1()
                .border_color(colors.border)
                .into_any_element(),
            Node::Quote(children) => div()
                .pl(px(16.))
                .mb(bottom(PARAGRAPH_SPACING))
                .border_l_4()
                .border_color(colors.border)
                .children(self.children(key, children, window))
                .into_any_element(),
            Node::List { start, items } => div()
                .pl(px(10.))
                .children(items.iter().enumerate().map(|(index, item)| {
                    let item_key = format!("{key}.{index}");
                    let bullet = match (item.task, start) {
                        (Some(checked), _) => self.checkbox(checked),
                        (None, Some(start)) => div()
                            .child(format!("{}.", start + index as u64))
                            .into_any_element(),
                        (None, None) => div().child("•").into_any_element(),
                    };
                    h_flex()
                        .items_start()
                        .gap(px(4.))
                        .mb(px(4.))
                        .child(bullet)
                        .child(div().flex_1().min_w(px(0.)).children(self.children(
                            &item_key,
                            &item.children,
                            window,
                        )))
                }))
                .into_any_element(),
            Node::Table { rows, header } => self.table(key, rows, *header, edges, window),
        }
    }

    /// Zed's default code block: editor background, thin border, horizontal
    /// scrolling, and a copy button that appears on hover.
    fn code_block(
        &self,
        key: &str,
        text: &str,
        language: Option<&str>,
        edges: Edges,
    ) -> AnyElement {
        let colors = &self.cx.theme().colors;
        let spans: Vec<(Range<usize>, SpanStyle)> =
            crate::highlight::highlight(text, language, &colors.syntax)
                .map(|highlights| {
                    highlights
                        .iter()
                        .map(|(range, color)| {
                            (
                                range.clone(),
                                SpanStyle {
                                    color: Some(*color),
                                    ..SpanStyle::default()
                                },
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
        let code = text.to_string();
        let group = SharedString::from(format!("{key}:code"));
        let mut scroller = div()
            .id(ElementId::Name(format!("{key}:scroll").into()))
            .flex()
            .overflow_x_scroll()
            .child(
                SelectableText::new(key.to_string(), text.to_string(), self.selection)
                    .spans(spans)
                    .text_size(px(self.code), px(self.code_line))
                    .font_family(self.code_font.clone())
                    .no_wrap(),
            );
        // Vertical wheel scrolling must keep moving the transcript.
        scroller.style().restrict_scroll_to_axis = Some(true);

        div()
            .group(group.clone())
            .relative()
            .w_full()
            .mt(if edges.first { px(0.) } else { px(8.) })
            .mb(if edges.last { px(0.) } else { px(12.) })
            .p(px(8.))
            .rounded(px(8.))
            .border_1()
            .border_color(colors.border_variant)
            .bg(colors.code_block_background)
            .child(scroller)
            .child(
                h_flex()
                    .absolute()
                    .top(px(4.))
                    .right(px(4.))
                    .bg(colors.elevated_surface_background)
                    .rounded(px(4.))
                    .opacity(0.)
                    .group_hover(group, |style| style.opacity(1.))
                    .child(
                        IconButton::new(
                            ElementId::Name(format!("{key}:copy").into()),
                            IconName::Copy,
                        )
                        .icon_size(IconSize::XSmall)
                        .tooltip(Tooltip::text("Copy Code"))
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(code.clone()))
                        }),
                    ),
            )
            .into_any_element()
    }

    /// Zed's visualization-only checkbox: a 16 px box in a 20 px slot,
    /// centered on the first text line.
    fn checkbox(&self, checked: bool) -> AnyElement {
        let colors = &self.cx.theme().colors;
        div()
            .h(px(self.body_line))
            .flex()
            .items_center()
            .child(
                div()
                    .size(px(16.))
                    .m(px(2.))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded(px(2.))
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.element_background)
                    .when(checked, |this| {
                        this.child(
                            Icon::new(IconName::Check)
                                .size(IconSize::XSmall)
                                .color(Color::Accent),
                        )
                    }),
            )
            .into_any_element()
    }

    /// Zed's markdown table: columns as wide as their content, 1.5 px outer
    /// border, title bar colored header, striped rows, horizontal scrolling.
    fn table(
        &self,
        key: &str,
        rows: &[Vec<Inline>],
        header: bool,
        edges: Edges,
        window: &mut Window,
    ) -> AnyElement {
        let colors = &self.cx.theme().colors;
        let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
        let mut widths = vec![px(0.); columns];
        for (row_index, row) in rows.iter().enumerate() {
            let weight = if header && row_index == 0 {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::NORMAL
            };
            for (column, cell) in row.iter().enumerate() {
                widths[column] =
                    widths[column].max(text_width(&cell.text, weight, self.body, window));
            }
        }
        let border = colors.border;
        let header_background = colors.title_bar_background;
        let stripe = colors.panel_background;

        let table = v_flex()
            .flex_none()
            .border_1()
            .border_color(border)
            .rounded(px(2.))
            .overflow_hidden()
            .children(rows.iter().enumerate().map(|(row_index, row)| {
                let is_header = header && row_index == 0;
                let background: Option<Hsla> = if is_header {
                    Some(header_background)
                } else if row_index % 2 == 1 {
                    Some(stripe)
                } else {
                    None
                };
                let mut row_element = h_flex();
                // Cells fill the row height so their borders line up.
                row_element.style().align_items = Some(gpui::AlignItems::Stretch);
                row_element
                    .when_some(background, |this, background| this.bg(background))
                    .when(row_index > 0, |this| this.border_t_1().border_color(border))
                    .children((0..columns).map(|column| {
                        let cell = row.get(column).cloned().unwrap_or_default();
                        div()
                            .w(widths[column] + px(8. + 2.))
                            .flex_none()
                            .px(px(4.))
                            .py(px(2.))
                            .when(column > 0, |this| this.border_l_1().border_color(border))
                            .child(
                                self.text(format!("{key}.{row_index}.{column}"), &cell)
                                    .no_wrap()
                                    .when(is_header, |this| this.weight(FontWeight::SEMIBOLD)),
                            )
                    }))
            }));

        let mut scroller = div()
            .id(ElementId::Name(format!("{key}:table").into()))
            .flex()
            .overflow_x_scroll()
            .mb(if edges.last { px(0.) } else { px(8.) })
            .child(table);
        scroller.style().restrict_scroll_to_axis = Some(true);
        scroller.into_any_element()
    }
}

/// Unwrapped width of the widest line of `text` in the body font.
fn text_width(text: &str, weight: FontWeight, size: f32, window: &mut Window) -> Pixels {
    let mut font = window.text_style().font();
    font.weight = weight;
    text.split('\n')
        .map(|line| {
            let run = TextRun {
                len: line.len(),
                font: font.clone(),
                color: Hsla::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            window
                .text_system()
                .shape_line(SharedString::from(line.to_string()), px(size), &[run], None)
                .width
        })
        .fold(px(0.), Pixels::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(node: &Node) -> &str {
        match node {
            Node::Paragraph { inline, .. } | Node::Heading { inline, .. } => &inline.text,
            Node::Code { text, .. } => text,
            _ => "",
        }
    }

    #[test]
    fn inline_styles_become_spans() {
        let nodes = parse("Use **bold**, *it*, `code` and [a link](https://x.dev).");
        let Node::Paragraph { inline, tight } = &nodes[0] else {
            panic!("expected a paragraph, got {:?}", nodes[0]);
        };
        assert!(!tight);
        assert_eq!(inline.text, "Use bold, it, code and a link.");
        let styled: Vec<&str> = inline
            .spans
            .iter()
            .map(|(range, _)| &inline.text[range.clone()])
            .collect();
        assert_eq!(styled, ["bold", "it", "code", "a link"]);
        assert!(inline.spans[0].1.bold);
        assert!(inline.spans[2].1.code);
        assert_eq!(
            inline.spans[3].1.link.as_ref().map(|url| url.as_ref()),
            Some("https://x.dev")
        );
    }

    #[test]
    fn blocks_keep_structure() {
        let source = "# Title\n\nText\nmore.\n\n- one\n- [x] two\n  1. nested\n\n```rust\nfn main() {}\n```\n\n> quoted\n\n---\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        let nodes = parse(source);
        assert!(matches!(nodes[0], Node::Heading { level: 1, .. }));
        // Soft breaks are line breaks, as in Zed's agent panel.
        assert_eq!(text_of(&nodes[1]), "Text\nmore.");
        let Node::List { start: None, items } = &nodes[2] else {
            panic!("expected a bullet list, got {:?}", nodes[2]);
        };
        assert_eq!(items.len(), 2);
        assert!(matches!(
            &items[0].children[0],
            Node::Paragraph { tight: true, .. }
        ));
        assert_eq!(items[1].task, Some(true));
        assert!(matches!(
            &items[1].children[1],
            Node::List { start: Some(1), .. }
        ));
        assert_eq!(text_of(&nodes[3]), "fn main() {}");
        assert!(matches!(&nodes[4], Node::Quote(children) if text_of(&children[0]) == "quoted"));
        assert_eq!(nodes[5], Node::Rule);
        let Node::Table { rows, header } = &nodes[6] else {
            panic!("expected a table, got {:?}", nodes[6]);
        };
        assert!(header);
        assert_eq!(rows[1][1].text, "2");
    }

    #[test]
    fn unfinished_streaming_markdown_still_parses() {
        let nodes = parse("Intro\n\n- item\n  ```py\n  print(1)");
        let Node::List { items, .. } = &nodes[1] else {
            panic!("expected a list, got {:?}", nodes[1]);
        };
        assert_eq!(text_of(&items[0].children[1]), "print(1)");
    }
}
