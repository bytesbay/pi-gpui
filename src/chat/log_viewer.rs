//! The log of one tool call in an overlay: live while the command runs,
//! the complete output once it ends (from the file pi saves when it cuts
//! long output).

use gpui::relative;

use super::*;

pub(super) const LOG_CONTEXT: &str = "ToolLog";
/// The tail of a log file larger than this is shown.
const MAX_FILE_BYTES: u64 = 16 << 20;

pub(super) struct LogViewer {
    pub row: usize,
    lines: Vec<SharedString>,
    /// The row text the lines were split from; `None` once they come from
    /// the log file.
    source: Option<Arc<str>>,
    /// The whole text shown, for copying.
    text: Arc<str>,
    loading: bool,
    /// The file held more than the viewer reads.
    cut_bytes: u64,
    error: Option<String>,
    list: ListState,
    focus: FocusHandle,
    _load: Task<()>,
}

impl ChatView {
    pub(super) fn open_log(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Row::Tool { status, log, .. }) = self.rows.get(row) else {
            return;
        };
        let load_file = *status != ToolStatus::Running && log.file.is_some();
        let focus = cx.focus_handle();
        window.focus(&focus);
        self.log_viewer = Some(LogViewer {
            row,
            lines: Vec::new(),
            source: None,
            text: Arc::from(""),
            loading: false,
            cut_bytes: 0,
            error: None,
            list: ListState::new(0, ListAlignment::Bottom, px(400.)),
            focus,
            _load: Task::ready(()),
        });
        self.sync_log_viewer();
        if load_file {
            self.load_log_file(cx);
        }
        cx.notify();
    }

    pub(super) fn close_log(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.log_viewer.take().is_some() {
            window.focus(&self.composer.read(cx).focus_handle(cx));
            cx.notify();
        }
    }

    pub(super) fn log_close(&mut self, _: &LogClose, window: &mut Window, cx: &mut Context<Self>) {
        self.close_log(window, cx);
    }

    /// Takes the row's latest output, unless the full file is shown.
    fn sync_log_viewer(&mut self) {
        let Some(viewer) = &mut self.log_viewer else {
            return;
        };
        let Some(Row::Tool { log, .. }) = self.rows.get(viewer.row) else {
            return;
        };
        if viewer.loading
            || (viewer.source.is_none() && !viewer.lines.is_empty())
            || viewer
                .source
                .as_ref()
                .is_some_and(|source| Arc::ptr_eq(source, &log.text))
        {
            return;
        }
        let lines: Vec<SharedString> = log
            .text
            .lines()
            .map(|line| SharedString::from(line.to_string()))
            .collect();
        let previous = viewer.lines.len();
        // Appended lines keep the scroll position; a moved window (pi sends
        // only the newest lines of a long run) starts over at the bottom.
        let appended = lines.len() >= previous
            && previous > 0
            && lines[..previous - 1] == viewer.lines[..previous - 1];
        if appended {
            viewer
                .list
                .splice(previous - 1..previous, lines.len() - previous + 1);
        } else {
            viewer.list.reset(lines.len());
        }
        viewer.lines = lines;
        viewer.text = log.text.clone();
        viewer.source = Some(log.text.clone());
    }

    /// Reads the complete output pi saved, in the background.
    pub(super) fn load_log_file(&mut self, cx: &mut Context<Self>) {
        let Some(viewer) = &mut self.log_viewer else {
            return;
        };
        let Some(Row::Tool { log, .. }) = self.rows.get(viewer.row) else {
            return;
        };
        let Some(file) = log.file.clone() else {
            return;
        };
        viewer.loading = true;
        let row = viewer.row;
        let read = cx.background_spawn(async move { read_tail(&file) });
        viewer._load = cx.spawn(async move |this, cx| {
            let result = read.await;
            this.update(cx, |this, cx| {
                let Some(viewer) = this.log_viewer.as_mut().filter(|viewer| viewer.row == row)
                else {
                    return;
                };
                viewer.loading = false;
                match result {
                    Ok((text, cut_bytes)) => {
                        viewer.lines = text
                            .lines()
                            .map(|line| SharedString::from(line.to_string()))
                            .collect();
                        viewer.list.reset(viewer.lines.len());
                        viewer.text = Arc::from(text);
                        viewer.source = None;
                        viewer.cut_bytes = cut_bytes;
                    }
                    // Cleaned up temp files: the row's own output stays.
                    Err(error) => viewer.error = Some(error),
                }
                cx.notify();
            })
            .ok();
        });
    }

    pub(super) fn render_log_viewer(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some(row) = self.log_finished.take()
            && self
                .log_viewer
                .as_ref()
                .is_some_and(|viewer| viewer.row == row)
        {
            self.load_log_file(cx);
        }
        self.sync_log_viewer();
        let viewer = self.log_viewer.as_ref()?;
        let Some(Row::Tool {
            name,
            detail,
            status,
            log,
            ..
        }) = self.rows.get(viewer.row)
        else {
            return None;
        };
        let colors = cx.theme().colors.clone();
        let running = *status == ToolStatus::Running;
        let row = viewer.row;

        let mut facts = vec![match status {
            ToolStatus::Running => "running".to_string(),
            ToolStatus::Done => "done".to_string(),
            ToolStatus::Failed => "failed".to_string(),
        }];
        match (running, log.started, log.duration_ms) {
            (true, Some(started), _) => facts.push(format!("{} s", started.elapsed().as_secs())),
            (false, _, Some(ms)) => facts.push(format_duration(ms)),
            _ => {}
        }
        facts.push(match viewer.lines.len() {
            1 => "1 line".to_string(),
            count => format!("{count} lines"),
        });

        // Where the text shown comes from, when it is not everything.
        let note = if viewer.loading {
            Some("Loading the full log…".to_string())
        } else if let Some(error) = &viewer.error {
            Some(format!(
                "The full log is gone ({error}); this is what pi reported."
            ))
        } else if viewer.cut_bytes > 0 {
            Some(format!(
                "Showing the last 16 MB; {} MB more are in the log file.",
                viewer.cut_bytes >> 20
            ))
        } else if running && log.file.is_some() {
            Some(
                "Live: the newest output (pi sends up to 2,000 lines or 50 KB). The full log opens here when the command ends."
                    .to_string(),
            )
        } else if !running && viewer.source.is_some() && log.text.contains("Full output: ") {
            Some("pi cut this output, and its full log file is gone.".to_string())
        } else {
            None
        };

        let body = if viewer.lines.is_empty() {
            h_flex()
                .flex_1()
                .justify_center()
                .py(px(40.))
                .child(
                    Label::new(if running {
                        "No output yet…"
                    } else {
                        "No output."
                    })
                    .size(LabelSize::Small)
                    .color(Color::Placeholder),
                )
                .into_any_element()
        } else {
            let font = crate::settings::Settings::get(cx)
                .buffer_font_family
                .clone();
            list(
                viewer.list.clone(),
                cx.processor(move |this, index: usize, _, _| {
                    let Some(line) = this
                        .log_viewer
                        .as_ref()
                        .and_then(|viewer| viewer.lines.get(index))
                    else {
                        return div().into_any_element();
                    };
                    div()
                        .px(px(14.))
                        .child(
                            SelectableText::new(
                                SharedString::from(format!("log:{row}:{index}")),
                                if line.is_empty() {
                                    SharedString::new_static(" ")
                                } else {
                                    line.clone()
                                },
                                &this.selection,
                            )
                            .size(LabelSize::XSmall)
                            .monospace(),
                        )
                        .into_any_element()
                }),
            )
            .font_family(font)
            .flex_1()
            .py(px(8.))
            .into_any_element()
        };

        let copy_text = viewer.text.clone();
        let file = log.file.clone().filter(|_| viewer.source.is_none());
        let editor_text = viewer.text.clone();
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.45))
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_log(window, cx)),
                )
                .child(
                    v_flex()
                        .key_context(LOG_CONTEXT)
                        .track_focus(&viewer.focus)
                        .w(px(980.))
                        .max_w(relative(0.94))
                        .h(relative(0.86))
                        .rounded(px(10.))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.editor_background)
                        .shadow_lg()
                        .overflow_hidden()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            h_flex()
                                .flex_none()
                                .gap(px(8.))
                                .px(px(14.))
                                .py(px(8.))
                                .border_b_1()
                                .border_color(colors.border_variant)
                                .bg(colors.panel_background)
                                .child(match status {
                                    ToolStatus::Running => Spinner::new()
                                        .size(IconSize::XSmall)
                                        .color(Color::Accent)
                                        .into_any_element(),
                                    ToolStatus::Done => Icon::new(IconName::Check)
                                        .size(IconSize::XSmall)
                                        .color(Color::Success)
                                        .into_any_element(),
                                    ToolStatus::Failed => Icon::new(IconName::Close)
                                        .size(IconSize::XSmall)
                                        .color(Color::Error)
                                        .into_any_element(),
                                })
                                .child(Label::new(name.clone()).weight(FontWeight::MEDIUM))
                                .child(
                                    div().flex_1().min_w(px(0.)).child(
                                        Label::new(attachments::display_text(detail))
                                            .size(LabelSize::Small)
                                            .color(Color::Muted)
                                            .monospace()
                                            .truncate(),
                                    ),
                                )
                                .child(
                                    Label::new(facts.join(" · "))
                                        .size(LabelSize::XSmall)
                                        .color(Color::Placeholder),
                                )
                                .child(
                                    IconButton::new("log-copy", IconName::Copy)
                                        .size(ButtonSize::Compact)
                                        .icon_size(IconSize::XSmall)
                                        .tooltip(Tooltip::text("Copy the log"))
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                                copy_text.to_string(),
                                            ))
                                        }),
                                )
                                .child(
                                    IconButton::new("log-open", IconName::File)
                                        .size(ButtonSize::Compact)
                                        .icon_size(IconSize::XSmall)
                                        .tooltip(Tooltip::text("Open in editor"))
                                        .on_click(move |_, _, _| {
                                            open_log_in_editor(file.as_deref(), &editor_text)
                                        }),
                                )
                                .child(
                                    IconButton::new("log-close", IconName::Close)
                                        .size(ButtonSize::Compact)
                                        .icon_size(IconSize::XSmall)
                                        .tooltip(Tooltip::with_keys("Close", "Esc"))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.close_log(window, cx)
                                        })),
                                ),
                        )
                        .children(note.map(|note| {
                            h_flex()
                                .flex_none()
                                .gap(px(6.))
                                .px(px(14.))
                                .py(px(5.))
                                .border_b_1()
                                .border_color(colors.border_variant)
                                .child(
                                    Icon::new(IconName::Info)
                                        .size(IconSize::XSmall)
                                        .color(Color::Muted),
                                )
                                .child(Label::new(note).size(LabelSize::XSmall).color(Color::Muted))
                        }))
                        .child(body),
                )
                .into_any_element(),
        )
    }
}

/// The file, or its last `MAX_FILE_BYTES` and how many bytes came before.
fn read_tail(path: &Path) -> Result<(String, u64), String> {
    use std::io::{Read as _, Seek as _, SeekFrom};

    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let size = file.metadata().map_err(|error| error.to_string())?.len();
    let skip = size.saturating_sub(MAX_FILE_BYTES);
    file.seek(SeekFrom::Start(skip))
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    // Start at a whole line when the head was skipped.
    if skip > 0
        && let Some(newline) = text.find('\n')
    {
        text.drain(..=newline);
    }
    Ok((text, skip))
}

/// The log file itself, or the text in a temporary file.
fn open_log_in_editor(file: Option<&Path>, text: &str) {
    if let Some(file) = file {
        crate::platform::open_in_editor(file);
        return;
    }
    let path = std::env::temp_dir().join(format!(
        "pi-gpui-log-{}.log",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|age| age.as_millis())
            .unwrap_or_default()
    ));
    match std::fs::write(&path, text) {
        Ok(()) => crate::platform::open_in_editor(&path),
        Err(error) => eprintln!("pi-gpui: could not write {}: {error}", path.display()),
    }
}
