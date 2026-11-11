//! Text editor for the message field (multi-line, soft-wrapped) and for small
//! single-line fields such as filters: IME, selection, clipboard, undo/redo and
//! the standard macOS editing keys.
//!
//! Started from `examples/input.rs` of GPUI 0.2.2 (Apache-2.0,
//! https://github.com/zed-industries/zed): the UTF-16 conversion and the
//! platform input handler come from there. Layout, navigation, editing and
//! undo were rewritten for multi-line text.

use std::ops::Range;
use std::time::{Duration, Instant};

use gpui::{
    App, AvailableSpace, Bounds, ClipboardItem, Context, CursorStyle, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId,
    KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    Pixels, Point, ScrollHandle, SharedString, Style, TextAlign, TextRun, UTF16Selection,
    UnderlineStyle, Window, WrappedLine, actions, div, fill, point, prelude::*, px, relative, size,
};
use unicode_segmentation::*;

use crate::attachments::{self, Attachment, AttachmentKind};
use crate::ui::ActiveTheme as _;

pub const KEY_CONTEXT: &str = "TextInput";
const LINE_HEIGHT: f32 = 22.;
const MAX_VISIBLE_LINES: f32 = 10.;
/// A masked field wraps its dots up to this many rows, then scrolls.
const MAX_MASKED_LINES: f32 = 3.;
/// Shown for every character of a masked field.
const MASK_CHAR: char = '•';
const UNDO_LIMIT: usize = 200;
/// Consecutive typing within this window is undone as one step.
const UNDO_COALESCE: Duration = Duration::from_millis(1000);

actions!(
    text_input,
    [
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteWordRight,
        DeleteToLineStart,
        DeleteToLineEnd,
        Left,
        Right,
        Up,
        Down,
        WordLeft,
        WordRight,
        LineStart,
        LineEnd,
        DocumentStart,
        DocumentEnd,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectWordLeft,
        SelectWordRight,
        SelectToLineStart,
        SelectToLineEnd,
        SelectToDocumentStart,
        SelectToDocumentEnd,
        SelectAll,
        Newline,
        Undo,
        Redo,
        Paste,
        Copy,
        Cut,
        ShowCharacterPalette,
    ]
);

/// Text editing keys: the macOS set, or the usual Linux one elsewhere.
pub fn bind_keys(cx: &mut App) {
    if cfg!(target_os = "macos") {
        bind_mac_keys(cx);
    } else {
        bind_linux_keys(cx);
    }
}

/// GTK and Qt text fields: Ctrl for words and clipboard, Home and End for lines.
fn bind_linux_keys(cx: &mut App) {
    let context = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, context),
        KeyBinding::new("shift-backspace", Backspace, context),
        KeyBinding::new("delete", Delete, context),
        KeyBinding::new("ctrl-backspace", DeleteWordLeft, context),
        KeyBinding::new("ctrl-delete", DeleteWordRight, context),
        KeyBinding::new("ctrl-shift-backspace", DeleteToLineStart, context),
        KeyBinding::new("ctrl-shift-delete", DeleteToLineEnd, context),
        KeyBinding::new("left", Left, context),
        KeyBinding::new("right", Right, context),
        KeyBinding::new("up", Up, context),
        KeyBinding::new("down", Down, context),
        KeyBinding::new("ctrl-left", WordLeft, context),
        KeyBinding::new("ctrl-right", WordRight, context),
        KeyBinding::new("home", LineStart, context),
        KeyBinding::new("end", LineEnd, context),
        KeyBinding::new("ctrl-home", DocumentStart, context),
        KeyBinding::new("ctrl-end", DocumentEnd, context),
        KeyBinding::new("shift-left", SelectLeft, context),
        KeyBinding::new("shift-right", SelectRight, context),
        KeyBinding::new("shift-up", SelectUp, context),
        KeyBinding::new("shift-down", SelectDown, context),
        KeyBinding::new("ctrl-shift-left", SelectWordLeft, context),
        KeyBinding::new("ctrl-shift-right", SelectWordRight, context),
        KeyBinding::new("shift-home", SelectToLineStart, context),
        KeyBinding::new("shift-end", SelectToLineEnd, context),
        KeyBinding::new("ctrl-shift-home", SelectToDocumentStart, context),
        KeyBinding::new("ctrl-shift-end", SelectToDocumentEnd, context),
        KeyBinding::new("ctrl-a", SelectAll, context),
        KeyBinding::new("shift-enter", Newline, context),
        KeyBinding::new("alt-enter", Newline, context),
        KeyBinding::new("ctrl-z", Undo, context),
        KeyBinding::new("ctrl-shift-z", Redo, context),
        KeyBinding::new("ctrl-y", Redo, context),
        KeyBinding::new("ctrl-v", Paste, context),
        KeyBinding::new("shift-insert", Paste, context),
        KeyBinding::new("ctrl-shift-v", Paste, context),
        KeyBinding::new("ctrl-c", Copy, context),
        KeyBinding::new("ctrl-insert", Copy, context),
        KeyBinding::new("ctrl-x", Cut, context),
    ]);
}

/// macOS text editing keys, including the Emacs-style control bindings that
/// Cocoa text views support.
fn bind_mac_keys(cx: &mut App) {
    let context = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, context),
        KeyBinding::new("shift-backspace", Backspace, context),
        KeyBinding::new("ctrl-h", Backspace, context),
        KeyBinding::new("delete", Delete, context),
        KeyBinding::new("ctrl-d", Delete, context),
        KeyBinding::new("alt-backspace", DeleteWordLeft, context),
        KeyBinding::new("alt-delete", DeleteWordRight, context),
        KeyBinding::new("cmd-backspace", DeleteToLineStart, context),
        KeyBinding::new("cmd-delete", DeleteToLineEnd, context),
        KeyBinding::new("ctrl-k", DeleteToLineEnd, context),
        KeyBinding::new("left", Left, context),
        KeyBinding::new("ctrl-b", Left, context),
        KeyBinding::new("right", Right, context),
        KeyBinding::new("ctrl-f", Right, context),
        KeyBinding::new("up", Up, context),
        KeyBinding::new("ctrl-p", Up, context),
        KeyBinding::new("down", Down, context),
        KeyBinding::new("ctrl-n", Down, context),
        KeyBinding::new("alt-left", WordLeft, context),
        KeyBinding::new("alt-right", WordRight, context),
        KeyBinding::new("cmd-left", LineStart, context),
        KeyBinding::new("ctrl-a", LineStart, context),
        KeyBinding::new("home", LineStart, context),
        KeyBinding::new("cmd-right", LineEnd, context),
        KeyBinding::new("ctrl-e", LineEnd, context),
        KeyBinding::new("end", LineEnd, context),
        KeyBinding::new("cmd-up", DocumentStart, context),
        KeyBinding::new("cmd-down", DocumentEnd, context),
        KeyBinding::new("shift-left", SelectLeft, context),
        KeyBinding::new("shift-right", SelectRight, context),
        KeyBinding::new("shift-up", SelectUp, context),
        KeyBinding::new("shift-down", SelectDown, context),
        KeyBinding::new("alt-shift-left", SelectWordLeft, context),
        KeyBinding::new("alt-shift-right", SelectWordRight, context),
        KeyBinding::new("cmd-shift-left", SelectToLineStart, context),
        KeyBinding::new("shift-home", SelectToLineStart, context),
        KeyBinding::new("cmd-shift-right", SelectToLineEnd, context),
        KeyBinding::new("shift-end", SelectToLineEnd, context),
        KeyBinding::new("cmd-shift-up", SelectToDocumentStart, context),
        KeyBinding::new("cmd-shift-down", SelectToDocumentEnd, context),
        KeyBinding::new("cmd-a", SelectAll, context),
        KeyBinding::new("shift-enter", Newline, context),
        KeyBinding::new("alt-enter", Newline, context),
        KeyBinding::new("cmd-z", Undo, context),
        KeyBinding::new("cmd-shift-z", Redo, context),
        KeyBinding::new("cmd-v", Paste, context),
        KeyBinding::new("cmd-c", Copy, context),
        KeyBinding::new("cmd-x", Cut, context),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, context),
    ]);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Typing,
    Deletion,
    Other,
}

struct Snapshot {
    content: SharedString,
    selection: Range<usize>,
}

/// Shaped text: one wrapped line per hard line. Shared with read-only
/// selectable text in the transcript.
#[derive(Clone)]
pub(crate) struct TextLayout {
    pub(crate) lines: Vec<WrappedLine>,
    /// Byte offset where each hard line starts.
    line_starts: Vec<usize>,
    /// Top of each hard line, relative to the text origin.
    pub(crate) line_tops: Vec<Pixels>,
    pub(crate) line_height: Pixels,
    height: Pixels,
    /// For a masked field: the real text. The shaped lines hold one
    /// `MASK_CHAR` per character, so offsets are mapped between the two.
    masked: Option<SharedString>,
}

impl TextLayout {
    fn with_mask(mut self, content: SharedString) -> Self {
        self.masked = Some(content);
        self
    }

    /// Text offset → offset in the shaped (displayed) text.
    fn display_offset(&self, offset: usize) -> usize {
        match &self.masked {
            Some(content) => {
                let offset = offset.min(content.len());
                content
                    .get(..offset)
                    .map_or(0, |prefix| prefix.chars().count())
                    * MASK_CHAR.len_utf8()
            }
            None => offset,
        }
    }

    /// Offset in the shaped text → text offset.
    fn text_offset(&self, offset: usize) -> usize {
        match &self.masked {
            Some(content) => content
                .char_indices()
                .nth(offset / MASK_CHAR.len_utf8())
                .map_or(content.len(), |(index, _)| index),
            None => offset,
        }
    }

    pub(crate) fn new(lines: Vec<WrappedLine>, line_height: Pixels) -> Self {
        let mut line_starts = Vec::with_capacity(lines.len());
        let mut line_tops = Vec::with_capacity(lines.len());
        let (mut start, mut top) = (0, px(0.));
        for line in &lines {
            line_starts.push(start);
            line_tops.push(top);
            start += line.len() + 1;
            top += line.size(line_height).height;
        }
        Self {
            lines,
            line_starts,
            line_tops,
            line_height,
            height: top.max(line_height),
            masked: None,
        }
    }

    fn line_for_offset(&self, offset: usize) -> usize {
        self.line_starts
            .iter()
            .rposition(|start| *start <= offset)
            .unwrap_or(0)
    }

    pub(crate) fn position_for_offset(&self, offset: usize) -> Point<Pixels> {
        let offset = self.display_offset(offset);
        let Some(line) = self.lines.get(self.line_for_offset(offset)) else {
            return point(px(0.), px(0.));
        };
        let index = self.line_for_offset(offset);
        let local = offset - self.line_starts[index];
        let position = line
            .position_for_index(local, self.line_height)
            .unwrap_or_else(|| point(line.width(), px(0.)));
        point(position.x, self.line_tops[index] + position.y)
    }

    pub(crate) fn offset_for_position(&self, position: Point<Pixels>) -> usize {
        if self.lines.is_empty() {
            return 0;
        }
        let index = self
            .line_tops
            .iter()
            .rposition(|top| *top <= position.y)
            .unwrap_or(0);
        let line = &self.lines[index];
        // Stay inside the line: below its last row the lookup would return 0.
        let line_height = line.size(self.line_height).height;
        let local_y = (position.y - self.line_tops[index]).clamp(px(0.), line_height - px(1.));
        let local =
            match line.closest_index_for_position(point(position.x, local_y), self.line_height) {
                Ok(index) | Err(index) => index,
            };
        self.text_offset(self.line_starts[index] + local.min(line.len()))
    }

    /// Start and end of the visual (wrapped) row containing `offset`.
    fn visual_row(&self, offset: usize) -> Range<usize> {
        let y = self.position_for_offset(offset).y + self.line_height / 2.;
        self.offset_for_position(point(px(0.), y))..self.offset_for_position(point(Pixels::MAX, y))
    }
}

pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    single_line: bool,
    /// Shows a dot per character (passwords, keys); copy and cut do nothing.
    masked: bool,
    /// Up and Down go to the parent even in a multi-line field (e.g. while a
    /// completion menu is open).
    vertical_passthrough: bool,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<TextLayout>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    /// Column kept while moving up and down through lines of different length.
    goal_x: Option<Pixels>,
    undo_stack: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
    last_edit: Option<(EditKind, Instant)>,
    scroll_handle: ScrollHandle,
    /// Bring the cursor into view on the next paint (after edits and moves).
    scroll_to_cursor: bool,
    /// Text range of the last plugin insertion, so streaming results (e.g.
    /// dictation) can replace their previous partial text.
    plugin_insertion: Option<Range<usize>>,
    /// Pasting files and images inserts them as chips (message composer only).
    attachments_enabled: bool,
    /// Every attachment ever added; a chip is its `[label]` token in the text,
    /// so undo and redo keep working on plain text.
    attachments: Vec<Attachment>,
    /// Painted chip rectangles (window coordinates) and their attachment.
    chip_bounds: Vec<(Bounds<Pixels>, usize)>,
    hovered_chip: Option<usize>,
}

impl TextInput {
    /// A multi-line field that wraps and grows up to a few lines.
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: SharedString::default(),
            placeholder: placeholder.into(),
            single_line: false,
            masked: false,
            vertical_passthrough: false,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
            goal_x: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit: None,
            scroll_handle: ScrollHandle::new(),
            scroll_to_cursor: false,
            plugin_insertion: None,
            attachments_enabled: false,
            attachments: Vec::new(),
            chip_bounds: Vec::new(),
            hovered_chip: None,
        }
    }

    /// Files and images can be pasted or dropped in; they become chips and
    /// are sent as paths (see `submit_text`).
    pub fn with_attachments(mut self) -> Self {
        self.attachments_enabled = true;
        self
    }

    /// Chip ranges in the text, sorted, with their attachment index.
    fn chips(&self) -> Vec<(Range<usize>, usize)> {
        let mut chips: Vec<(Range<usize>, usize)> = self
            .attachments
            .iter()
            .enumerate()
            .filter_map(|(index, attachment)| {
                let token = attachment.token();
                self.content
                    .find(&token)
                    .map(|start| (start..start + token.len(), index))
            })
            .collect();
        chips.sort_by_key(|(range, _)| range.start);
        chips
    }

    /// The text to send: chips become the paths they stand for.
    pub fn submit_text(&self) -> String {
        let mut text = self.content.to_string();
        for (range, index) in self.chips().into_iter().rev() {
            let path = self.attachments[index].path.display().to_string();
            text.replace_range(range, &path);
        }
        text
    }

    /// Adds files (pasted, dropped or picked) as chips at the cursor.
    pub fn attach_paths(&mut self, paths: &[std::path::PathBuf], cx: &mut Context<Self>) {
        for path in paths {
            let kind = if attachments::is_image(path) {
                AttachmentKind::Image
            } else {
                AttachmentKind::File
            };
            let linked = attachments::link_file(path).unwrap_or_else(|error| {
                eprintln!("pi-gpui: could not link {}: {error}", path.display());
                path.clone()
            });
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            self.insert_attachment(name, linked, kind, cx);
        }
    }

    fn insert_attachment(
        &mut self,
        base_label: String,
        path: std::path::PathBuf,
        kind: AttachmentKind,
        cx: &mut Context<Self>,
    ) {
        // Labels are unique, so a chip always maps back to one attachment.
        let mut label = base_label.replace(['[', ']'], "");
        let mut copy = 2;
        while self
            .attachments
            .iter()
            .any(|attachment| attachment.label == label)
        {
            label = format!("{} ({copy})", base_label.replace(['[', ']'], ""));
            copy += 1;
        }
        let attachment = Attachment { label, path, kind };
        let token = attachment.token();
        self.attachments.push(attachment);

        let range = self.selected_range.clone();
        let before = self.content[..range.start].chars().next_back();
        let after = self.content[range.end..].chars().next();
        let text = format!(
            "{}{token}{}",
            if before.is_some_and(|character| !character.is_whitespace()) {
                " "
            } else {
                ""
            },
            if after.is_none_or(|character| !character.is_whitespace()) {
                " "
            } else {
                ""
            },
        );
        self.record_undo(EditKind::Other);
        self.replace(range, &text);
        self.goal_x = None;
        self.plugin_insertion = None;
        cx.notify();
    }

    fn image_label(&self) -> String {
        let count = self
            .attachments
            .iter()
            .filter(|attachment| {
                attachment.kind == AttachmentKind::Image && attachment.label.starts_with("Image ")
            })
            .count();
        format!("Image {}", count + 1)
    }

    /// Moves an offset that falls inside a chip to the chip's nearer edge.
    fn snap_out_of_chip(&self, offset: usize, prefer_end: bool) -> usize {
        for (range, _) in self.chips() {
            if range.start < offset && offset < range.end {
                return if prefer_end { range.end } else { range.start };
            }
        }
        offset
    }

    /// A one-line field (filters, search). Up and Down are left to the parent.
    pub fn single_line(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            single_line: true,
            ..Self::new(placeholder, cx)
        }
    }

    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.placeholder = placeholder.into();
        cx.notify();
    }

    /// Hides or shows the text of a field holding a secret.
    pub fn set_masked(&mut self, masked: bool, cx: &mut Context<Self>) {
        self.masked = masked;
        cx.notify();
    }

    pub fn is_masked(&self) -> bool {
        self.masked
    }

    /// Multi-line fields wrap; so do masked ones, so a long key stays visible as dots.
    fn wraps(&self) -> bool {
        !self.single_line || self.masked
    }

    pub fn set_vertical_passthrough(&mut self, passthrough: bool) {
        self.vertical_passthrough = passthrough;
    }

    pub fn text(&self) -> SharedString {
        self.content.clone()
    }

    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.record_undo(EditKind::Other);
        self.content = self.normalize(text.into().as_ref()).into();
        let end = self.content.len();
        self.select_range(end..end, false);
        self.marked_range = None;
        self.plugin_insertion = None;
        cx.notify();
    }

    /// Clears the field. Undoable, so a message sent by mistake can be recovered.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        if self.content.is_empty() {
            return;
        }
        self.record_undo(EditKind::Other);
        self.content = SharedString::default();
        self.select_range(0..0, false);
        // An unfinished IME composition refers to the old text; drop it.
        self.marked_range = None;
        self.plugin_insertion = None;
        cx.notify();
    }

    /// Inserts at the cursor, replacing the selection. With `replace_last`,
    /// replaces the previous plugin insertion instead, as long as the cursor
    /// still sits right after it (the user has not moved or typed since).
    pub fn insert_text(
        &mut self,
        text: &str,
        replace_last: bool,
        space_before: bool,
        cx: &mut Context<Self>,
    ) {
        let range = match &self.plugin_insertion {
            Some(previous)
                if replace_last
                    && previous.end <= self.content.len()
                    && self.selected_range == (previous.end..previous.end) =>
            {
                previous.clone()
            }
            _ => self.selected_range.clone(),
        };
        let range = self.clamp_range(range);
        let needs_space = space_before
            && self.content[..range.start]
                .chars()
                .next_back()
                .is_some_and(|character| !character.is_whitespace());
        let inserted = if needs_space {
            format!(" {text}")
        } else {
            text.to_string()
        };
        // Partial results of one utterance undo together with the utterance.
        if !replace_last {
            self.record_undo(EditKind::Other);
        }
        let end = self.replace(range.clone(), &inserted);
        self.plugin_insertion = Some(range.start..end);
        cx.notify();
    }

    fn normalize(&self, text: &str) -> String {
        let text = text.replace("\r\n", "\n");
        if self.single_line {
            text.replace('\n', " ")
        } else {
            text
        }
    }

    /// Largest valid offset at or below `offset`.
    fn clamp_offset(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.content.len());
        while !self.content.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    /// Ranges can come from the platform (IME) or from state that went stale;
    /// slicing with them must never panic.
    fn clamp_range(&self, range: Range<usize>) -> Range<usize> {
        let start = self.clamp_offset(range.start);
        start..self.clamp_offset(range.end).max(start)
    }

    /// Replaces `range` with `text` and puts the cursor after it. Returns the end offset.
    fn replace(&mut self, range: Range<usize>, text: &str) -> usize {
        let range = self.clamp_range(range);
        self.content = format!(
            "{}{}{}",
            &self.content[..range.start],
            text,
            &self.content[range.end..]
        )
        .into();
        let end = range.start + text.len();
        self.select_range(end..end, false);
        self.marked_range = None;
        end
    }

    fn select_range(&mut self, range: Range<usize>, reversed: bool) {
        let range = self.clamp_range(range);
        // A chip is one unit: the cursor and selections never split it.
        let range = if self.attachments.is_empty() {
            range
        } else if range.is_empty() {
            let offset = self.snap_out_of_chip(range.start, false);
            offset..offset
        } else {
            self.snap_out_of_chip(range.start, false)..self.snap_out_of_chip(range.end, true)
        };
        self.selected_range = range;
        self.selection_reversed = reversed;
        self.scroll_to_cursor = true;
    }

    fn record_undo(&mut self, kind: EditKind) {
        let now = Instant::now();
        let coalesce = kind != EditKind::Other
            && self.last_edit.is_some_and(|(last_kind, at)| {
                last_kind == kind && now.duration_since(at) < UNDO_COALESCE
            });
        if !coalesce {
            self.undo_stack.push(Snapshot {
                content: self.content.clone(),
                selection: self.selected_range.clone(),
            });
            if self.undo_stack.len() > UNDO_LIMIT {
                self.undo_stack.remove(0);
            }
        }
        self.redo_stack.clear();
        self.last_edit = Some((kind, now));
    }

    fn restore(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        self.content = snapshot.content;
        let selection = snapshot.selection.start.min(self.content.len())
            ..snapshot.selection.end.min(self.content.len());
        self.select_range(selection, false);
        self.marked_range = None;
        self.plugin_insertion = None;
        self.last_edit = None;
        cx.notify();
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(snapshot) = self.undo_stack.pop() {
            self.redo_stack.push(Snapshot {
                content: self.content.clone(),
                selection: self.selected_range.clone(),
            });
            self.restore(snapshot, cx);
        }
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(snapshot) = self.redo_stack.pop() {
            self.undo_stack.push(Snapshot {
                content: self.content.clone(),
                selection: self.selected_range.clone(),
            });
            self.restore(snapshot, cx);
        }
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.select_range(offset..offset, false);
        self.goal_x = None;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let anchor = if self.selection_reversed {
            self.selected_range.end
        } else {
            self.selected_range.start
        };
        if offset < anchor {
            self.select_range(offset..anchor, true);
        } else {
            self.select_range(anchor..offset, false);
        }
        cx.notify();
    }

    fn delete_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let range = self.clamp_range(range);
        if range.is_empty() {
            return;
        }
        self.record_undo(EditKind::Deletion);
        self.replace(range, "");
        self.goal_x = None;
        self.plugin_insertion = None;
        cx.notify();
    }

    /// Deletes the selection if there is one, otherwise `cursor..target`.
    fn delete_towards(&mut self, target: usize, cx: &mut Context<Self>) {
        let range = if self.selected_range.is_empty() {
            let cursor = self.cursor_offset();
            cursor.min(target)..cursor.max(target)
        } else {
            self.selected_range.clone()
        };
        self.delete_range(range, cx);
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        if let Some((range, _)) = self
            .chips()
            .into_iter()
            .find(|(range, _)| range.start < offset && offset <= range.end)
        {
            return range.start;
        }
        self.content[..offset]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        if let Some((range, _)) = self
            .chips()
            .into_iter()
            .find(|(range, _)| range.start <= offset && offset < range.end)
        {
            return range.end;
        }
        self.content[offset..]
            .graphemes(true)
            .next()
            .map_or(self.content.len(), |grapheme| offset + grapheme.len())
    }

    fn previous_word_start(&self, offset: usize) -> usize {
        let target = previous_word_start(&self.content, offset);
        self.snap_out_of_chip(target, false)
    }

    fn next_word_end(&self, offset: usize) -> usize {
        let target = next_word_end(&self.content, offset);
        self.snap_out_of_chip(target, true)
    }

    fn word_range_at(&self, offset: usize) -> Range<usize> {
        word_range_at(&self.content, offset)
    }

    fn paragraph_range_at(&self, offset: usize) -> Range<usize> {
        paragraph_range_at(&self.content, offset)
    }

    /// Start and end of the visual row; falls back to the hard line before
    /// the first layout.
    fn row_range(&self, offset: usize) -> Range<usize> {
        match &self.last_layout {
            Some(layout) if !self.content.is_empty() => layout.visual_row(offset),
            _ => self.paragraph_range_at(offset),
        }
    }

    /// Offset one visual row above or below, keeping the goal column. `None`
    /// when there is no such row.
    fn vertical_target(&mut self, direction: f32) -> Option<usize> {
        let layout = self.last_layout.as_ref()?;
        if self.content.is_empty() {
            return None;
        }
        let position = layout.position_for_offset(self.cursor_offset());
        let goal_x = *self.goal_x.get_or_insert(position.x);
        let y = position.y + layout.line_height / 2. + layout.line_height * direction;
        if y < px(0.) || y >= layout.height {
            return None;
        }
        Some(layout.offset_for_position(point(goal_x, y)))
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        let target = self.previous_boundary(self.cursor_offset());
        self.delete_towards(target, cx);
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        let target = self.next_boundary(self.cursor_offset());
        self.delete_towards(target, cx);
    }

    fn delete_word_left(&mut self, _: &DeleteWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let target = self.previous_word_start(self.cursor_offset());
        self.delete_towards(target, cx);
    }

    fn delete_word_right(&mut self, _: &DeleteWordRight, _: &mut Window, cx: &mut Context<Self>) {
        let target = self.next_word_end(self.cursor_offset());
        self.delete_towards(target, cx);
    }

    fn delete_to_line_start(
        &mut self,
        _: &DeleteToLineStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cursor = self.cursor_offset();
        let start = self.row_range(cursor).start;
        // At a row start, join with the previous line, like Cocoa text views.
        let target = if start == cursor {
            self.previous_boundary(cursor)
        } else {
            start
        };
        self.delete_towards(target, cx);
    }

    fn delete_to_line_end(&mut self, _: &DeleteToLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        let end = self.row_range(cursor).end;
        let target = if end == cursor {
            self.next_boundary(cursor)
        } else {
            end
        };
        self.delete_towards(target, cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx);
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.end, cx);
        }
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        if self.single_line || self.vertical_passthrough {
            cx.propagate();
            return;
        }
        match self.vertical_target(-1.) {
            Some(target) => {
                // Keep the column set by vertical_target across consecutive moves.
                let goal_x = self.goal_x;
                self.move_to(target, cx);
                self.goal_x = goal_x;
            }
            None => self.move_to(0, cx),
        }
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        if self.single_line || self.vertical_passthrough {
            cx.propagate();
            return;
        }
        match self.vertical_target(1.) {
            Some(target) => {
                // Keep the column set by vertical_target across consecutive moves.
                let goal_x = self.goal_x;
                self.move_to(target, cx);
                self.goal_x = goal_x;
            }
            None => self.move_to(self.content.len(), cx),
        }
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.previous_word_start(self.cursor_offset()), cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.next_word_end(self.cursor_offset()), cx);
    }

    fn line_start(&mut self, _: &LineStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.row_range(self.cursor_offset()).start, cx);
    }

    fn line_end(&mut self, _: &LineEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.row_range(self.cursor_offset()).end, cx);
    }

    fn document_start(&mut self, _: &DocumentStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn document_end(&mut self, _: &DocumentEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        let target = self.vertical_target(-1.).unwrap_or(0);
        self.select_to(target, cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        let target = self.vertical_target(1.).unwrap_or(self.content.len());
        self.select_to(target, cx);
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_word_start(self.cursor_offset()), cx);
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_word_end(self.cursor_offset()), cx);
    }

    fn select_to_line_start(
        &mut self,
        _: &SelectToLineStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.row_range(self.cursor_offset()).start, cx);
    }

    fn select_to_line_end(&mut self, _: &SelectToLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.row_range(self.cursor_offset()).end, cx);
    }

    fn select_to_document_start(
        &mut self,
        _: &SelectToDocumentStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(0, cx);
    }

    fn select_to_document_end(
        &mut self,
        _: &SelectToDocumentEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.content.len(), cx);
    }

    pub fn select_all_text(&mut self, cx: &mut Context<Self>) {
        self.select_range(0..self.content.len(), false);
        cx.notify();
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.select_range(0..self.content.len(), false);
        cx.notify();
    }

    fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        if self.single_line {
            return;
        }
        self.record_undo(EditKind::Other);
        self.replace(self.selected_range.clone(), "\n");
        self.goal_x = None;
        self.plugin_insertion = None;
        cx.notify();
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if self.attachments_enabled {
            // Like pi's terminal UI: copied files first, then an image, then text.
            let files = crate::platform::clipboard_file_paths();
            if !files.is_empty() {
                self.attach_paths(&files, cx);
                return;
            }
            let image = cx.read_from_clipboard().and_then(|item| {
                item.entries().iter().find_map(|entry| match entry {
                    gpui::ClipboardEntry::Image(image) => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(image) = image {
                match attachments::save_clipboard_image(&image) {
                    Ok(path) => {
                        let label = self.image_label();
                        self.insert_attachment(label, path, AttachmentKind::Image, cx);
                    }
                    Err(error) => eprintln!("pi-gpui: could not save the pasted image: {error}"),
                }
                return;
            }
        }
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let text = self.normalize(&text);
            self.record_undo(EditKind::Other);
            self.replace(self.selected_range.clone(), &text);
            self.goal_x = None;
            self.plugin_insertion = None;
            cx.notify();
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() && !self.masked {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() && !self.masked {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.delete_range(self.selected_range.clone(), cx);
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        match (&self.last_layout, &self.last_bounds) {
            (Some(layout), Some(bounds)) if !self.content.is_empty() => {
                layout.offset_for_position(position - bounds.origin)
            }
            _ => 0,
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let offset = self.index_for_mouse_position(event.position);
        self.goal_x = None;
        match event.click_count {
            2 => {
                let word = self.word_range_at(offset);
                self.select_range(word, false);
            }
            3.. => {
                let paragraph = self.paragraph_range_at(offset);
                self.select_range(paragraph, false);
            }
            _ => {
                self.is_selecting = true;
                if event.modifiers.shift {
                    self.select_to(offset, cx);
                } else {
                    self.select_range(offset..offset, false);
                }
            }
        }
        cx.notify();
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
        let hovered = self
            .chip_bounds
            .iter()
            .find(|(bounds, _)| bounds.contains(&event.position))
            .map(|(_, index)| *index);
        if hovered != self.hovered_chip {
            self.hovered_chip = hovered;
            cx.notify();
        }
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for character in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += character.len_utf16();
            utf8_offset += character.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for character in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += character.len_utf8();
            utf16_offset += character.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.clamp_range(self.range_from_utf16(&range_utf16));
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Typing by the user ends any streaming plugin insertion.
        self.plugin_insertion = None;
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let new_text = self.normalize(new_text);
        self.record_undo(if new_text.contains(char::is_whitespace) {
            EditKind::Other
        } else {
            EditKind::Typing
        });
        self.replace(range, &new_text);
        self.goal_x = None;
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Typing by the user ends any streaming plugin insertion.
        self.plugin_insertion = None;
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let range = self.clamp_range(range);
        self.record_undo(EditKind::Typing);

        self.content = format!(
            "{}{}{}",
            &self.content[..range.start],
            new_text,
            &self.content[range.end..]
        )
        .into();
        self.marked_range =
            (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        let selection = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.start)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.select_range(selection, false);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        let start = layout.position_for_offset(range.start);
        let end = layout.position_for_offset(range.end);
        Some(Bounds::from_corners(
            bounds.origin + start,
            bounds.origin + point(end.x, end.y + layout.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let layout = self.last_layout.as_ref()?;
        let offset = layout.offset_for_position(point - bounds.origin);
        Some(self.offset_to_utf16(offset))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    layout: Option<TextLayout>,
    cursor: Option<PaintQuad>,
    selections: Vec<PaintQuad>,
    /// Chip backgrounds and their hover areas.
    chips: Vec<(PaintQuad, usize)>,
}

impl TextElement {
    /// The text to shape (content or placeholder) and its style runs.
    fn text_and_runs(&self, window: &Window, cx: &App) -> (SharedString, Vec<TextRun>) {
        let input = self.input.read(cx);
        let style = window.text_style();
        let (text, color) = if input.content.is_empty() {
            (
                input.placeholder.clone(),
                cx.theme().colors.text_placeholder,
            )
        } else if input.masked {
            let dots: String = input.content.chars().map(|_| MASK_CHAR).collect();
            (dots.into(), style.color)
        } else {
            (input.content.clone(), style.color)
        };
        let run = TextRun {
            len: text.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let chips = if input.content.is_empty() || input.masked {
            Vec::new()
        } else {
            input.chips()
        };
        if !chips.is_empty() && input.marked_range.is_none() {
            let accent = cx.theme().colors.text_accent;
            let mut runs = Vec::new();
            let mut position = 0;
            for (range, _) in &chips {
                if range.start > position {
                    runs.push(TextRun {
                        len: range.start - position,
                        ..run.clone()
                    });
                }
                runs.push(TextRun {
                    len: range.len(),
                    color: accent,
                    ..run.clone()
                });
                position = range.end;
            }
            if position < text.len() {
                runs.push(TextRun {
                    len: text.len() - position,
                    ..run
                });
            }
            return (text, runs);
        }
        let runs = match input.marked_range.as_ref() {
            Some(marked) if !input.content.is_empty() && !input.masked => vec![
                TextRun {
                    len: marked.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked.end - marked.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: text.len() - marked.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect(),
            _ => vec![run],
        };
        (text, runs)
    }
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
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
        let (text, runs) = self.text_and_runs(window, cx);
        let wrap = self.input.read(cx).wraps();
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let layout_id =
            window.request_measured_layout(style, move |known, available, window, _cx| {
                let width = known.width.or(match available.width {
                    AvailableSpace::Definite(width) => Some(width),
                    _ => None,
                });
                let lines = window
                    .text_system()
                    .shape_text(text.clone(), font_size, &runs, width.filter(|_| wrap), None)
                    .unwrap_or_default();
                let height = lines
                    .iter()
                    .map(|line| line.size(line_height).height)
                    .fold(px(0.), |total, height| total + height)
                    .max(line_height);
                size(width.unwrap_or(px(0.)), height)
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
        let (text, runs) = self.text_and_runs(window, cx);
        let input = self.input.read(cx);
        let wrap_width = input.wraps().then_some(bounds.size.width);
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let Ok(lines) = window
            .text_system()
            .shape_text(text, font_size, &runs, wrap_width, None)
        else {
            return PrepaintState {
                layout: None,
                cursor: None,
                selections: Vec::new(),
                chips: Vec::new(),
            };
        };
        let mut layout = TextLayout::new(lines.into_vec(), line_height);
        if input.masked && !input.content.is_empty() {
            layout = layout.with_mask(input.content.clone());
        }
        let colors = cx.theme().colors.clone();
        let input = self.input.read(cx);

        let mut selections = Vec::new();
        let mut cursor = None;
        if input.selected_range.is_empty() {
            let position = if input.content.is_empty() {
                point(px(0.), px(0.))
            } else {
                layout.position_for_offset(input.cursor_offset())
            };
            cursor = Some(fill(
                Bounds::new(bounds.origin + position, size(px(2.), line_height)),
                colors.info,
            ));
        } else {
            let start = layout.position_for_offset(input.selected_range.start);
            let end = layout.position_for_offset(input.selected_range.end);
            let color = colors.info.opacity(0.24);
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

        // Keep the cursor visible inside the scrolling field after edits and moves.
        if input.scroll_to_cursor {
            let viewport = input.scroll_handle.bounds();
            let cursor_offset = if input.content.is_empty() {
                0
            } else {
                input.cursor_offset()
            };
            let cursor_top = bounds.origin.y + layout.position_for_offset(cursor_offset).y;
            let cursor_bottom = cursor_top + line_height;
            let mut offset = input.scroll_handle.offset();
            if viewport.size.height > px(0.) {
                if cursor_top < viewport.top() {
                    offset.y += viewport.top() - cursor_top;
                } else if cursor_bottom > viewport.bottom() {
                    offset.y -= cursor_bottom - viewport.bottom();
                }
                if offset != input.scroll_handle.offset() {
                    input.scroll_handle.set_offset(offset);
                    window.request_animation_frame();
                }
            }
        }

        // A rounded tag behind each chip, one per visual row it spans.
        let mut chips = Vec::new();
        if !input.content.is_empty() {
            for (range, index) in input.chips() {
                let start = layout.position_for_offset(range.start);
                let end = layout.position_for_offset(range.end);
                let mut row_top = start.y;
                while row_top <= end.y {
                    let left = if row_top == start.y { start.x } else { px(0.) };
                    let right = if row_top == end.y {
                        end.x
                    } else {
                        bounds.size.width
                    };
                    if right > left {
                        let chip = Bounds::from_corners(
                            bounds.origin + point(left - px(2.), row_top + px(1.)),
                            bounds.origin + point(right + px(2.), row_top + line_height - px(1.)),
                        );
                        chips.push((
                            gpui::quad(
                                chip,
                                px(4.),
                                colors.text_accent.opacity(0.12),
                                px(1.),
                                colors.text_accent.opacity(0.3),
                                gpui::BorderStyle::Solid,
                            ),
                            index,
                        ));
                    }
                    row_top += line_height;
                }
            }
        }

        PrepaintState {
            layout: Some(layout),
            cursor,
            selections,
            chips,
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
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        let chip_bounds: Vec<(Bounds<Pixels>, usize)> = prepaint
            .chips
            .iter()
            .map(|(quad, index)| (quad.bounds, *index))
            .collect();
        for (chip, _) in prepaint.chips.drain(..) {
            window.paint_quad(chip);
        }
        for selection in prepaint.selections.drain(..) {
            window.paint_quad(selection);
        }
        let Some(layout) = prepaint.layout.take() else {
            return;
        };
        for (line, top) in layout.lines.iter().zip(&layout.line_tops) {
            let origin = bounds.origin + point(px(0.), *top);
            if let Err(error) = line.paint(
                origin,
                layout.line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            ) {
                eprintln!("pi-gpui: failed to paint input text: {error}");
            }
        }
        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }
        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(layout);
            input.last_bounds = Some(bounds);
            input.scroll_to_cursor = false;
            input.chip_bounds = chip_bounds;
        });
    }
}

impl TextInput {
    /// A preview above the hovered chip: the image itself, or the file path.
    fn render_chip_preview(&self, cx: &App) -> Option<gpui::AnyElement> {
        let index = self.hovered_chip?;
        let attachment = self.attachments.get(index)?;
        let (bounds, _) = self.chip_bounds.iter().find(|(_, chip)| *chip == index)?;
        let colors = cx.theme().colors.clone();
        let content = match attachment.kind {
            AttachmentKind::Image => div()
                .p(px(4.))
                .child(
                    gpui::img(attachment.path.clone())
                        .max_w(px(320.))
                        .max_h(px(240.))
                        .object_fit(gpui::ObjectFit::Contain)
                        .rounded(px(4.)),
                )
                .into_any_element(),
            AttachmentKind::File => div()
                .px(px(10.))
                .py(px(6.))
                .text_size(px(12.))
                .text_color(colors.text_muted)
                .child(
                    std::fs::read_link(&attachment.path)
                        .unwrap_or_else(|_| attachment.path.clone())
                        .display()
                        .to_string(),
                )
                .into_any_element(),
        };
        Some(
            gpui::deferred(
                gpui::anchored()
                    .position(bounds.origin)
                    .anchor(gpui::Corner::BottomLeft)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .mb(px(6.))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(colors.border)
                            .bg(colors.elevated_surface_background)
                            .shadow_lg()
                            .child(content),
                    ),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let max_lines = if self.single_line && self.masked {
            MAX_MASKED_LINES
        } else if self.single_line {
            1.
        } else {
            MAX_VISIBLE_LINES
        };
        let preview = self.render_chip_preview(cx);
        div()
            .id("text-input")
            .flex()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered && this.hovered_chip.take().is_some() {
                    cx.notify();
                }
            }))
            .children(preview)
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_word_right))
            .on_action(cx.listener(Self::delete_to_line_start))
            .on_action(cx.listener(Self::delete_to_line_end))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::line_start))
            .on_action(cx.listener(Self::line_end))
            .on_action(cx.listener(Self::document_start))
            .on_action(cx.listener(Self::document_end))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::select_to_line_start))
            .on_action(cx.listener(Self::select_to_line_end))
            .on_action(cx.listener(Self::select_to_document_start))
            .on_action(cx.listener(Self::select_to_document_end))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::show_character_palette))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .w_full()
            .text_size(px(if self.single_line { 13. } else { 15. }))
            .line_height(px(LINE_HEIGHT))
            .child(
                div()
                    .id("text-input-scroll")
                    .w_full()
                    .max_h(px(LINE_HEIGHT * max_lines + 18.))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    // Single-line fields sit inside other controls: compact.
                    .px(px(if self.single_line { 8. } else { 12. }))
                    .py(px(if self.single_line { 3. } else { 9. }))
                    .child(TextElement { input: cx.entity() }),
            )
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn previous_word_start(text: &str, offset: usize) -> usize {
    text[..offset]
        .split_word_bound_indices()
        .rev()
        .find(|(_, word)| word.chars().any(char::is_alphanumeric))
        .map_or(0, |(index, _)| index)
}

fn next_word_end(text: &str, offset: usize) -> usize {
    text[offset..]
        .split_word_bound_indices()
        .find(|(_, word)| word.chars().any(char::is_alphanumeric))
        .map_or(text.len(), |(index, word)| offset + index + word.len())
}

pub(crate) fn word_range_at(text: &str, offset: usize) -> Range<usize> {
    text.split_word_bound_indices()
        .find(|(index, word)| *index <= offset && offset < index + word.len())
        .map_or(offset..offset, |(index, word)| index..index + word.len())
}

pub(crate) fn paragraph_range_at(text: &str, offset: usize) -> Range<usize> {
    let start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
    let end = text[offset..]
        .find('\n')
        .map_or(text.len(), |index| offset + index);
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "fix the größe bug,  now\nsecond line";

    #[test]
    fn word_navigation_skips_punctuation_and_spaces() {
        let comma = TEXT.find(',').unwrap_or_default();
        // From after "bug," back to the start of "bug".
        assert_eq!(&TEXT[previous_word_start(TEXT, comma + 1)..comma], "bug");
        // From the start of "größe" forward to its end, across multi-byte characters.
        let word = TEXT.find("größe").unwrap_or_default();
        assert_eq!(next_word_end(TEXT, word), word + "größe".len());
        // Past trailing spaces to the end of "now".
        assert_eq!(
            &TEXT[..next_word_end(TEXT, comma + 1)],
            "fix the größe bug,  now"
        );
        assert_eq!(previous_word_start(TEXT, 0), 0);
        assert_eq!(next_word_end(TEXT, TEXT.len()), TEXT.len());
    }

    #[test]
    fn word_and_paragraph_ranges() {
        let inside = TEXT.find("öß").unwrap_or_default();
        assert_eq!(&TEXT[word_range_at(TEXT, inside)], "größe");
        let second = TEXT.find("second").unwrap_or_default();
        assert_eq!(&TEXT[paragraph_range_at(TEXT, second + 3)], "second line");
        assert_eq!(
            &TEXT[paragraph_range_at(TEXT, 2)],
            "fix the größe bug,  now"
        );
    }
}
