//! Extension dialogs (`extension_ui_request` with `select`, `confirm`,
//! `input` or `editor`), asked inline in the transcript instead of a popup.
//!
//! pi blocks the extension until the answer arrives, so a dialog must always
//! end with exactly one `extension_ui_response`: an answer or a cancel.
//!
//! - `select` and `confirm` show their choices in the card. Arrows and Enter,
//!   digits 1 to 9 (or Y and N) pick; Esc cancels.
//! - `input` and `editor` are answered in the composer: Enter sends the text
//!   as the answer, Esc cancels.
//!
//! Extensions often ask a chain of dialogs for one question (ask_user_question
//! asks "Other…" as a select followed by an input). A dialog that follows an
//! answered one right away reuses its card, so the chain reads as one card.

use super::*;

pub(super) const DIALOG_CONTEXT: &str = "ChatDialog";
const ANSWER_PLACEHOLDER: &str = "Type your answer. Enter sends it, Esc cancels.";
/// ask_user_question marks its free-text choice with this prefix.
const FREE_TEXT_PREFIX: &str = "✎";

#[derive(Clone)]
pub(super) enum DialogKind {
    Select(Vec<String>),
    Confirm(String),
    Input,
    Editor,
}

#[derive(Clone, PartialEq)]
pub(super) enum DialogState {
    Pending,
    Answered(String),
    Cancelled,
    /// pi resolved it by itself (timeout) or the run ended.
    Closed,
}

pub(super) struct DialogRow {
    pub title: String,
    pub kind: DialogKind,
    pub state: DialogState,
}

/// The dialog pi is waiting on.
pub(super) struct PendingDialog {
    id: String,
    row: usize,
    highlight: usize,
    /// Composer text before an `editor` prefill replaced it.
    saved_draft: Option<SharedString>,
    _timeout: Task<()>,
}

impl DialogKind {
    fn choices(&self) -> Vec<String> {
        match self {
            DialogKind::Select(options) => options.clone(),
            DialogKind::Confirm(_) => vec!["Yes".to_string(), "No".to_string()],
            DialogKind::Input | DialogKind::Editor => Vec::new(),
        }
    }

    fn uses_composer(&self) -> bool {
        matches!(self, DialogKind::Input | DialogKind::Editor)
    }
}

impl ChatView {
    /// Shows a dialog request in the transcript. Returns false for other methods.
    pub(super) fn open_dialog(
        &mut self,
        record: &Value,
        changed: &mut Vec<usize>,
        cx: &mut Context<Self>,
    ) -> bool {
        let method = record["method"].as_str().unwrap_or_default();
        let Some(id) = record["id"].as_str().map(str::to_string) else {
            return matches!(method, "select" | "confirm" | "input" | "editor");
        };
        let title = record["title"].as_str().unwrap_or_default().to_string();
        let kind = match method {
            "select" => DialogKind::Select(
                record["options"]
                    .as_array()
                    .map(|options| {
                        options
                            .iter()
                            .filter_map(|option| option.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
            "confirm" => {
                DialogKind::Confirm(record["message"].as_str().unwrap_or_default().to_string())
            }
            "input" => DialogKind::Input,
            "editor" => DialogKind::Editor,
            _ => return false,
        };

        // A dialog still pending was resolved by pi (timeout): close its card.
        self.close_dialog(DialogState::Closed, changed, cx);

        let queued_answer = self.queued_dialog_answer.take();
        if let (DialogKind::Input, Some(answer)) = (&kind, &queued_answer) {
            // The answer was typed before pi asked for it.
            self.respond_dialog(&id, json!({ "value": answer }));
            self.push_dialog_row(title, kind, DialogState::Answered(answer.clone()), changed);
            return true;
        }

        let row = self.push_dialog_row(title, kind.clone(), DialogState::Pending, changed);
        let saved_draft = match (&kind, record["prefill"].as_str()) {
            (DialogKind::Editor, Some(prefill)) if !prefill.is_empty() => {
                let draft = self.composer.read(cx).text();
                let prefill = prefill.to_string();
                self.composer
                    .update(cx, |composer, cx| composer.set_text(prefill, cx));
                Some(draft)
            }
            _ => None,
        };
        if kind.uses_composer() {
            self.composer.update(cx, |composer, cx| {
                composer.set_placeholder(ANSWER_PLACEHOLDER, cx)
            });
        }
        // pi answers by itself when the timeout passes; close the card then.
        let timeout = match record["timeout"].as_u64() {
            Some(ms) if ms > 0 => {
                let timer = cx.background_executor().timer(Duration::from_millis(ms));
                let dialog_id = id.clone();
                cx.spawn(async move |this, cx| {
                    timer.await;
                    this.update(cx, |this, cx| {
                        if this.dialog.as_ref().is_some_and(|d| d.id == dialog_id) {
                            let mut changed = Vec::new();
                            this.close_dialog(DialogState::Closed, &mut changed, cx);
                            this.sync_list(this.rows.len(), changed);
                            cx.notify();
                        }
                    })
                    .ok();
                })
            }
            _ => Task::ready(()),
        };
        self.dialog = Some(PendingDialog {
            id,
            row,
            highlight: 0,
            saved_draft,
            _timeout: timeout,
        });
        self.focus_dialog = true;
        true
    }

    /// Appends a dialog card, or reuses the card of the dialog answered just
    /// before (the last row), so a chain of dialogs reads as one card.
    fn push_dialog_row(
        &mut self,
        title: String,
        kind: DialogKind,
        state: DialogState,
        changed: &mut Vec<usize>,
    ) -> usize {
        let dialog = DialogRow { title, kind, state };
        if let Some(Row::Dialog(last)) = self.rows.last_mut()
            && matches!(last.state, DialogState::Answered(_))
        {
            *last = dialog;
            let index = self.rows.len() - 1;
            changed.push(index);
            return index;
        }
        self.rows.push(Row::Dialog(dialog));
        self.rows.len() - 1
    }

    /// Ends the pending dialog's card in `state` without answering pi.
    pub(super) fn close_dialog(
        &mut self,
        state: DialogState,
        changed: &mut Vec<usize>,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        if let Some(Row::Dialog(row)) = self.rows.get_mut(dialog.row) {
            if row.kind.uses_composer() {
                self.composer.update(cx, |composer, cx| {
                    composer.set_placeholder(COMPOSER_PLACEHOLDER, cx)
                });
            }
            row.state = state;
            changed.push(dialog.row);
        }
        if let Some(draft) = dialog.saved_draft {
            self.composer
                .update(cx, |composer, cx| composer.set_text(draft, cx));
        }
    }

    /// Sends the response for the pending dialog and updates its card.
    fn answer_dialog(&mut self, response: Value, state: DialogState, cx: &mut Context<Self>) {
        let Some(id) = self.dialog.as_ref().map(|dialog| dialog.id.clone()) else {
            return;
        };
        self.respond_dialog(&id, response);
        let mut changed = Vec::new();
        self.close_dialog(state, &mut changed, cx);
        self.sync_list(self.rows.len(), changed);
        cx.emit(ChatEvent::Changed);
        cx.notify();
    }

    /// Writes an `extension_ui_response`. Its id must be the request's, so it
    /// bypasses `send`, which stamps a fresh id on every command.
    pub(super) fn respond_dialog(&mut self, id: &str, mut response: Value) {
        response["type"] = json!("extension_ui_response");
        response["id"] = json!(id);
        let result = match &self.connection {
            Connection::Ready(process) => process.send(response),
            _ => Err("pi is not connected".to_string()),
        };
        if let Err(error) = result {
            self.push_notice(Tone::Error, error);
        }
    }

    /// The question pi waits on, for plugins (`sessions.get`).
    pub(super) fn pending_dialog_info(&self) -> Value {
        let Some(dialog) = &self.dialog else {
            return Value::Null;
        };
        let Some(Row::Dialog(row)) = self.rows.get(dialog.row) else {
            return Value::Null;
        };
        let (kind, message) = match &row.kind {
            DialogKind::Select(_) => ("select", None),
            DialogKind::Confirm(message) => ("confirm", Some(message.clone())),
            DialogKind::Input => ("input", None),
            DialogKind::Editor => ("editor", None),
        };
        json!({
            "id": dialog.id,
            "kind": kind,
            "title": row.title,
            "message": message,
            "options": row.kind.choices(),
        })
    }

    /// A plugin answers the pending dialog: `answer` (an option, its 1-based
    /// number, or text), `confirmed`, or `cancel`.
    pub(super) fn answer_dialog_for_plugin(
        &mut self,
        id: &str,
        params: &Value,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(dialog) = &self.dialog else {
            return Err("pi is not waiting for an answer".into());
        };
        if dialog.id != id {
            return Err(format!("pi is not waiting on question {id}"));
        }
        let Some(Row::Dialog(row)) = self.rows.get(dialog.row) else {
            return Err("the question is gone".into());
        };
        if params["cancel"].as_bool() == Some(true) {
            self.cancel_dialog(cx);
            return Ok(());
        }
        let answer = params["answer"].as_str().map(str::trim);
        match &row.kind {
            DialogKind::Confirm(_) => {
                let confirmed = match (params["confirmed"].as_bool(), answer) {
                    (Some(confirmed), _) => confirmed,
                    (None, Some(answer)) => matches!(
                        answer.to_lowercase().as_str(),
                        "yes" | "y" | "true" | "1" | "да"
                    ),
                    (None, None) => return Err("give confirmed: true or false".into()),
                };
                let label = if confirmed { "Yes" } else { "No" };
                self.answer_dialog(
                    json!({ "confirmed": confirmed }),
                    DialogState::Answered(label.to_string()),
                    cx,
                );
            }
            DialogKind::Select(options) => {
                let answer = answer.ok_or("give answer: one of the options")?;
                let choice = match answer.parse::<usize>() {
                    Ok(number) if (1..=options.len()).contains(&number) => {
                        options[number - 1].clone()
                    }
                    _ => options
                        .iter()
                        .find(|option| option.trim() == answer)
                        .cloned()
                        .ok_or_else(|| format!("not an option: {answer}"))?,
                };
                self.answer_dialog(
                    json!({ "value": choice }),
                    DialogState::Answered(choice.clone()),
                    cx,
                );
            }
            DialogKind::Input | DialogKind::Editor => {
                let text = answer.ok_or("give answer: the text")?.to_string();
                if row.kind.uses_composer() {
                    self.composer.update(cx, |composer, cx| composer.clear(cx));
                }
                self.answer_dialog(
                    json!({ "value": text }),
                    DialogState::Answered(text.clone()),
                    cx,
                );
            }
        }
        Ok(())
    }

    pub(super) fn cancel_dialog(&mut self, cx: &mut Context<Self>) {
        self.answer_dialog(json!({ "cancelled": true }), DialogState::Cancelled, cx);
    }

    fn pick_choice(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(dialog) = &self.dialog else {
            return;
        };
        let Some(Row::Dialog(row)) = self.rows.get(dialog.row) else {
            return;
        };
        let choices = row.kind.choices();
        let Some(choice) = choices.get(index).cloned() else {
            return;
        };
        match row.kind {
            DialogKind::Confirm(_) => self.answer_dialog(
                json!({ "confirmed": index == 0 }),
                DialogState::Answered(choice),
                cx,
            ),
            _ => self.answer_dialog(
                json!({ "value": choice }),
                DialogState::Answered(choice),
                cx,
            ),
        }
    }

    /// The pending dialog's choices when it shows them in the card.
    fn pending_choices(&self) -> Option<Vec<String>> {
        let dialog = self.dialog.as_ref()?;
        match self.rows.get(dialog.row) {
            Some(Row::Dialog(row)) if !row.kind.uses_composer() => Some(row.kind.choices()),
            _ => None,
        }
    }

    /// The card takes the keys while a select or confirm waits.
    pub(super) fn dialog_takes_keys(&self) -> bool {
        self.pending_choices().is_some()
    }

    /// Enter in the composer while a dialog waits. Returns true when the text
    /// went to the dialog instead of being sent as a prompt.
    pub(super) fn submit_to_dialog(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        let Some(dialog) = &self.dialog else {
            return false;
        };
        let Some(Row::Dialog(row)) = self.rows.get(dialog.row) else {
            return false;
        };
        match &row.kind {
            DialogKind::Input | DialogKind::Editor => {
                let text = text.trim().to_string();
                self.composer.update(cx, |composer, cx| composer.clear(cx));
                self.answer_dialog(
                    json!({ "value": text }),
                    DialogState::Answered(text.clone()),
                    cx,
                );
                true
            }
            DialogKind::Select(options) => {
                let text = text.trim();
                if let Ok(number) = text.parse::<usize>()
                    && (1..=options.len()).contains(&number)
                {
                    self.composer.update(cx, |composer, cx| composer.clear(cx));
                    self.pick_choice(number - 1, cx);
                    return true;
                }
                // Free text answers a select through its free-text choice.
                let Some(index) = options
                    .iter()
                    .position(|option| option.starts_with(FREE_TEXT_PREFIX))
                else {
                    return false;
                };
                let text = text.to_string();
                self.composer.update(cx, |composer, cx| composer.clear(cx));
                // The input that choice leads to is answered with this text.
                self.queued_dialog_answer = Some(text);
                self.pick_choice(index, cx);
                true
            }
            DialogKind::Confirm(_) => false,
        }
    }

    pub(super) fn dialog_up(&mut self, _: &DialogUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_dialog_highlight(-1, cx);
    }

    pub(super) fn dialog_down(&mut self, _: &DialogDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_dialog_highlight(1, cx);
    }

    fn move_dialog_highlight(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(count) = self.pending_choices().map(|choices| choices.len()) else {
            return;
        };
        if count == 0 {
            return;
        }
        if let Some(dialog) = &mut self.dialog {
            dialog.highlight =
                (dialog.highlight as isize + delta).rem_euclid(count as isize) as usize;
            let row = dialog.row;
            self.list_state.splice(row..row + 1, 1);
        }
        cx.notify();
    }

    pub(super) fn dialog_confirm(
        &mut self,
        _: &DialogConfirm,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(highlight) = self.dialog.as_ref().map(|dialog| dialog.highlight) {
            self.pick_choice(highlight, cx);
        }
    }

    pub(super) fn dialog_cancel(
        &mut self,
        _: &DialogCancel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_dialog(cx);
    }

    /// Digits pick a choice, Y and N answer a confirm.
    pub(super) fn dialog_key_down(&mut self, event: &gpui::KeyDownEvent, cx: &mut Context<Self>) {
        let modifiers = &event.keystroke.modifiers;
        if modifiers.platform || modifiers.control || modifiers.alt || modifiers.function {
            return;
        }
        let Some(choices) = self.pending_choices() else {
            return;
        };
        let key = event.keystroke.key.as_str();
        let index = match key {
            "y" if choices.len() == 2 && choices[0] == "Yes" => Some(0),
            "n" if choices.len() == 2 && choices[1] == "No" => Some(1),
            _ => key
                .parse::<usize>()
                .ok()
                .filter(|number| (1..=choices.len().min(9)).contains(number))
                .map(|number| number - 1),
        };
        if let Some(index) = index {
            cx.stop_propagation();
            self.pick_choice(index, cx);
        }
    }

    /// Moves focus where the new dialog is answered. Called from render.
    pub(super) fn focus_pending_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.focus_dialog) || self.dialog.is_none() {
            return;
        }
        if self.dialog_takes_keys() {
            window.focus(&self.focus_handle);
        } else {
            window.focus(&self.composer.read(cx).focus_handle(cx));
        }
    }

    pub(super) fn render_dialog(
        &self,
        index: usize,
        row: &DialogRow,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let pending = row.state == DialogState::Pending
            && self
                .dialog
                .as_ref()
                .is_some_and(|dialog| dialog.row == index);
        let highlight = self.dialog.as_ref().map_or(0, |dialog| dialog.highlight);
        let key = |part: &str| SharedString::from(format!("{index}:{part}"));

        let title = (!row.title.trim().is_empty()).then(|| {
            SelectableText::new(
                key("dialog-title"),
                SharedString::from(row.title.trim().to_string()),
                &self.selection,
            )
            .size(LabelSize::Small)
            .color(if pending {
                Color::Default
            } else {
                Color::Muted
            })
        });
        let message = match &row.kind {
            DialogKind::Confirm(message) if !message.trim().is_empty() => Some(
                SelectableText::new(
                    key("dialog-message"),
                    SharedString::from(message.trim().to_string()),
                    &self.selection,
                )
                .size(LabelSize::Small)
                .color(Color::Muted),
            ),
            _ => None,
        };

        let body: AnyElement = match &row.state {
            DialogState::Pending if pending && !row.kind.uses_composer() => {
                let choices = row.kind.choices();
                let numbered = matches!(row.kind, DialogKind::Confirm(_));
                v_flex()
                    .gap(px(2.))
                    .children(choices.into_iter().enumerate().map(|(position, choice)| {
                        let (label, description) = match choice.split_once(" — ") {
                            Some((label, description)) => {
                                (label.to_string(), Some(description.to_string()))
                            }
                            None => (choice.clone(), None),
                        };
                        let active = position == highlight;
                        h_flex()
                            .id(("dialog-choice", position))
                            .items_start()
                            .gap(px(8.))
                            .px(px(8.))
                            .py(px(5.))
                            .rounded(px(5.))
                            .cursor_pointer()
                            .when(active, |this| this.bg(colors.element_selected))
                            .hover(|style| style.bg(colors.element_hover))
                            .when(numbered, |this| {
                                this.child(KeyHint::new(if position == 0 { "Y" } else { "N" }))
                            })
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .child(Label::new(label).size(LabelSize::Small))
                                    .when_some(description, |this, description| {
                                        this.child(
                                            Label::new(description)
                                                .size(LabelSize::XSmall)
                                                .color(Color::Muted),
                                        )
                                    }),
                            )
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.pick_choice(position, cx)),
                            )
                    }))
                    .into_any_element()
            }
            DialogState::Pending => h_flex()
                .gap(px(6.))
                .child(
                    Icon::new(IconName::Pencil)
                        .size(IconSize::XSmall)
                        .color(Color::Accent),
                )
                .child(
                    Label::new("Answer in the message field below")
                        .size(LabelSize::Small)
                        .color(Color::Accent),
                )
                .into_any_element(),
            DialogState::Answered(answer) => h_flex()
                .items_start()
                .gap(px(6.))
                .child(
                    div().pt(px(2.)).child(
                        Icon::new(IconName::Check)
                            .size(IconSize::XSmall)
                            .color(Color::Success),
                    ),
                )
                .child(
                    SelectableText::new(
                        key("dialog-answer"),
                        SharedString::from(if answer.is_empty() {
                            "(empty answer)".to_string()
                        } else {
                            answer.clone()
                        }),
                        &self.selection,
                    )
                    .size(LabelSize::Small),
                )
                .into_any_element(),
            DialogState::Cancelled | DialogState::Closed => {
                Label::new(if row.state == DialogState::Cancelled {
                    "Cancelled"
                } else {
                    "No longer waiting for an answer"
                })
                .size(LabelSize::Small)
                .color(Color::Placeholder)
                .into_any_element()
            }
        };

        let hints = pending.then(|| {
            let hint = |keys: &'static str, text: &'static str| {
                h_flex().gap(px(4.)).child(KeyHint::new(keys)).child(
                    Label::new(text)
                        .size(LabelSize::XSmall)
                        .color(Color::Placeholder),
                )
            };
            h_flex()
                .gap(px(12.))
                .when(!row.kind.uses_composer(), |this| {
                    this.child(hint("↑↓", "choose"))
                        .child(hint("Enter", "answer"))
                })
                .when(row.kind.uses_composer(), |this| {
                    this.child(hint("Enter", "send"))
                })
                .child(hint("Esc", "cancel"))
                .when(matches!(row.kind, DialogKind::Select(_)), |this| {
                    this.child(
                        Label::new("or type the number or your own answer below")
                            .size(LabelSize::XSmall)
                            .color(Color::Placeholder),
                    )
                })
        });

        v_flex()
            .gap(px(8.))
            .px(px(12.))
            .py(px(10.))
            .rounded(px(8.))
            .border_1()
            .border_color(if pending {
                colors.border_focused
            } else {
                colors.border_variant
            })
            .bg(colors.panel_background)
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(
                        Icon::new(IconName::Message)
                            .size(IconSize::XSmall)
                            .color(if pending { Color::Accent } else { Color::Muted }),
                    )
                    .child(
                        Label::new(if pending { "pi is asking" } else { "pi asked" })
                            .size(LabelSize::XSmall)
                            .color(Color::Muted)
                            .weight(FontWeight::MEDIUM),
                    ),
            )
            .children(title)
            .children(message)
            .child(body)
            .children(hints)
            .into_any_element()
    }
}
