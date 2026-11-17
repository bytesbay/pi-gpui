//! Forms plugins ask inside tool calls (`prompt.form`): one question at a
//! time, with options to pick (one or several), a field for an answer of the
//! user's own, and Back / Next / Submit. Answers stay while the user moves
//! between questions; the plugin gets all of them at once.
//!
//! Keys: arrows move between the options and the text field, Space or a digit
//! picks, Enter goes on (picking the highlighted option first when that is
//! unambiguous), Left and Right change the question, Esc cancels.

use super::plugin_prompt::{PROMPT_CONTEXT, PromptBody};
use super::*;
use crate::plugins::{FormAnswer, FormQuestion, PromptAnswer, PromptForm};

type PromptKey = (SharedString, SharedString);

/// The answers of one open form.
pub(super) struct FormState {
    form: Arc<PromptForm>,
    /// Holds the keyboard while no text field does.
    focus: FocusHandle,
    /// The question shown.
    step: usize,
    /// The highlighted row of that question: its options, then its text field.
    highlight: usize,
    /// Picked options per question, ascending.
    selected: Vec<Vec<usize>>,
    /// The text field of each question that takes text.
    texts: Vec<Option<Entity<TextInput>>>,
}

impl FormState {
    pub(super) fn new(form: Arc<PromptForm>, cx: &mut Context<ChatView>) -> Self {
        let texts = form
            .questions
            .iter()
            .map(|question| {
                question.allow_text.then(|| {
                    cx.new(|cx| {
                        if question.options.is_empty() {
                            TextInput::new(question.placeholder.clone(), cx)
                        } else {
                            TextInput::single_line(question.placeholder.clone(), cx)
                        }
                    })
                })
            })
            .collect();
        Self {
            focus: cx.focus_handle(),
            step: 0,
            highlight: 0,
            selected: vec![Vec::new(); form.questions.len()],
            texts,
            form,
        }
    }

    /// Whether this state answers `form`. The same prompt asked again with
    /// other questions starts over.
    pub(super) fn is_for(&self, form: &Arc<PromptForm>) -> bool {
        self.form == *form
    }

    fn question(&self) -> &FormQuestion {
        &self.form.questions[self.step]
    }

    fn text(&self, index: usize, cx: &App) -> String {
        self.texts[index]
            .as_ref()
            .map(|input| input.read(cx).text().to_string())
            .unwrap_or_default()
    }

    /// The answer to question `index`. With one choice allowed, typed text
    /// replaces the picked option.
    fn answer(&self, index: usize, cx: &App) -> FormAnswer {
        let question = &self.form.questions[index];
        let text = self.text(index, cx);
        let typed = !text.trim().is_empty();
        FormAnswer {
            selected: if typed && !question.multiple {
                Vec::new()
            } else {
                self.selected[index].clone()
            },
            text,
        }
    }

    fn answers(&self, cx: &App) -> Vec<FormAnswer> {
        (0..self.form.questions.len())
            .map(|index| self.answer(index, cx))
            .collect()
    }

    fn answered(&self, index: usize, cx: &App) -> bool {
        self.form.questions[index].answered(&self.answer(index, cx))
    }

    fn text_focused(&self, window: &Window, cx: &App) -> bool {
        self.texts[self.step]
            .as_ref()
            .is_some_and(|input| input.read(cx).focus_handle(cx).is_focused(window))
    }

    /// Shows question `step` with the highlight on its answer.
    fn show(&mut self, step: usize, cx: &App) {
        self.step = step;
        let answer = self.answer(step, cx);
        self.highlight = match answer.selected.first() {
            Some(option) => *option,
            None if !answer.text.trim().is_empty() => self.question().options.len(),
            None => 0,
        };
    }

    /// Gives the keyboard to the highlighted row: the text field, or the
    /// card for an option.
    pub(super) fn focus(&self, window: &mut Window, cx: &App) {
        match &self.texts[self.step] {
            Some(input) if self.highlight >= self.question().options.len() => {
                window.focus(&input.read(cx).focus_handle(cx));
            }
            _ => window.focus(&self.focus),
        }
    }
}

impl ChatView {
    fn form_mut(&mut self, id: &PromptKey) -> Option<&mut FormState> {
        let index = self.field_index(id)?;
        match &mut self.prompt_fields[index].body {
            PromptBody::Form(state) => Some(state),
            PromptBody::Input(_) => None,
        }
    }

    /// The card changed height: measure its row again.
    fn refresh_form_row(&mut self, id: &PromptKey) {
        let row = self
            .field_index(id)
            .and_then(|index| {
                self.tool_rows
                    .get(self.prompt_fields[index].call_id.as_ref())
            })
            .copied();
        if let Some(row) = row {
            self.sync_list(self.rows.len(), vec![row]);
        }
    }

    /// Picks an option of the question shown: the only one, or one more.
    fn form_pick(
        &mut self,
        id: &PromptKey,
        option: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.form_mut(id) else {
            return;
        };
        let step = state.step;
        if option >= state.question().options.len() {
            return;
        }
        state.highlight = option;
        if state.question().multiple {
            let selected = &mut state.selected[step];
            match selected.binary_search(&option) {
                Ok(position) => {
                    selected.remove(position);
                }
                Err(position) => selected.insert(position, option),
            }
        } else {
            state.selected[step] = vec![option];
            // One answer: the option replaces what was typed.
            if let Some(input) = state.texts[step].clone() {
                input.update(cx, |input, cx| input.clear(cx));
            }
        }
        if let Some(state) = self.form_mut(id) {
            state.focus(window, cx);
        }
        cx.notify();
    }

    fn form_move(
        &mut self,
        id: &PromptKey,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.form_mut(id) else {
            return;
        };
        let options = state.question().options.len();
        let rows = options + usize::from(state.question().allow_text);
        if rows == 0 {
            return;
        }
        let current = if state.text_focused(window, cx) {
            options
        } else {
            state.highlight.min(rows - 1)
        };
        state.highlight = (current as isize + delta).rem_euclid(rows as isize) as usize;
        state.focus(window, cx);
        cx.notify();
    }

    fn form_back(&mut self, id: &PromptKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.form_mut(id) else {
            return;
        };
        if state.step == 0 {
            return;
        }
        state.show(state.step - 1, cx);
        state.focus(window, cx);
        self.refresh_form_row(id);
        cx.notify();
    }

    /// Goes to the next question, or sends the answers after the last one.
    /// A required question holds the form until it has an answer.
    fn form_next(&mut self, id: &PromptKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.form_mut(id) else {
            return;
        };
        let count = state.form.questions.len();
        if state.question().required && !state.answered(state.step, cx) {
            return;
        }
        let next = if state.step + 1 < count {
            Some(state.step + 1)
        } else {
            // An earlier answer may have been cleared after going back.
            (0..count)
                .find(|index| state.form.questions[*index].required && !state.answered(*index, cx))
        };
        match next {
            Some(step) => {
                state.show(step, cx);
                state.focus(window, cx);
                self.refresh_form_row(id);
                cx.notify();
            }
            None => {
                let answers = state.form.result(&state.answers(cx));
                self.send_prompt_answer(id, PromptAnswer::Form(answers), window, cx);
            }
        }
    }

    /// Cancels the form; the plugin still gets the answers given so far.
    fn form_cancel(&mut self, id: &PromptKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.form_mut(id) else {
            return;
        };
        let answer = PromptAnswer::Cancelled {
            answers: Some(state.form.result(&state.answers(cx))),
            closed: false,
        };
        self.send_prompt_answer(id, answer, window, cx);
    }

    fn form_key_down(
        &mut self,
        id: &PromptKey,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let modifiers = &event.keystroke.modifiers;
        let key = event.keystroke.key.as_str();
        if modifiers.secondary() {
            // Cmd/Ctrl+Enter goes on from anywhere in the card.
            if key == "enter" {
                cx.stop_propagation();
                self.form_next(id, window, cx);
            }
            return;
        }
        if modifiers.platform || modifiers.control || modifiers.alt || modifiers.function {
            return;
        }
        // The rest belongs to the card; in a text field these keys are text.
        let Some(state) = self.form_mut(id) else {
            return;
        };
        if !state.focus.is_focused(window) {
            return;
        }
        let (highlight, options, multiple) = (
            state.highlight,
            state.question().options.len(),
            state.question().multiple,
        );
        let answered = state.answered(state.step, cx);
        match key {
            "up" => self.form_move(id, -1, window, cx),
            "down" => self.form_move(id, 1, window, cx),
            "left" => self.form_back(id, window, cx),
            "right" => self.form_next(id, window, cx),
            "space" => self.form_pick(id, highlight, window, cx),
            "enter" if multiple && !answered => self.form_pick(id, highlight, window, cx),
            "enter" => {
                if !multiple && highlight < options {
                    self.form_pick(id, highlight, window, cx);
                }
                self.form_next(id, window, cx);
            }
            _ => match key.parse::<usize>() {
                Ok(number) if (1..=options.min(9)).contains(&number) => {
                    self.form_pick(id, number - 1, window, cx);
                }
                _ => return,
            },
        }
        cx.stop_propagation();
    }

    pub(super) fn render_plugin_form(
        &self,
        position: usize,
        key: PromptKey,
        prompt: &plugins::PluginPrompt,
        state: &FormState,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let question = state.question();
        let count = state.form.questions.len();
        let last = state.step + 1 == count;
        let answer = state.answer(state.step, cx);
        let answered = question.answered(&answer);
        let typed = !answer.text.trim().is_empty();
        let text_focused = state.text_focused(window, cx);
        let card_focused = state.focus.is_focused(window);
        let highlight = if text_focused {
            question.options.len()
        } else {
            state.highlight
        };

        let marker = |checked: bool| {
            let marker = div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(14.))
                .border_1()
                .border_color(if checked {
                    colors.icon_accent
                } else {
                    colors.text_muted
                });
            if question.multiple {
                marker.rounded(px(3.)).when(checked, |this| {
                    this.child(
                        Icon::new(IconName::Check)
                            .size(IconSize::XSmall)
                            .color(Color::Accent),
                    )
                })
            } else {
                marker.rounded_full().when(checked, |this| {
                    this.child(div().size(px(6.)).rounded_full().bg(colors.icon_accent))
                })
            }
        };

        let options = question
            .options
            .iter()
            .enumerate()
            .map(|(option, choice)| {
                let checked = answer.selected.contains(&option);
                h_flex()
                    .id(("form-option", position * 100 + option))
                    .items_start()
                    .gap(px(8.))
                    .px(px(7.))
                    .py(px(4.))
                    .rounded(px(5.))
                    .cursor_pointer()
                    // A ring as well as a fill: on some themes the fill alone
                    // is the color of the card.
                    .border_1()
                    .border_color(gpui::transparent_black())
                    .when(option == highlight && card_focused, |this| {
                        this.bg(colors.element_hover)
                            .border_color(colors.border_focused)
                    })
                    .hover(|style| style.bg(colors.element_hover))
                    .child(div().pt(px(2.)).child(marker(checked)))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .child(Label::new(choice.label.clone()).size(LabelSize::Small))
                            .when_some(choice.description.clone(), |this, description| {
                                this.child(
                                    Label::new(description)
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                            }),
                    )
                    .on_click(cx.listener({
                        let key = key.clone();
                        move |this, _, window, cx| this.form_pick(&key, option, window, cx)
                    }))
            })
            .collect::<Vec<_>>();

        let text_row = state.texts[state.step].clone().map(|input| {
            let field = div()
                .flex_1()
                .min_w(px(0.))
                .rounded(px(6.))
                .border_1()
                .border_color(if text_focused {
                    colors.border_focused
                } else {
                    colors.border
                })
                .bg(colors.editor_background)
                .child(input);
            h_flex()
                .items_center()
                .gap(px(8.))
                .when(!question.options.is_empty(), |this| {
                    // Lines up with the options: this row is one more choice.
                    this.px(px(8.)).pt(px(3.)).child(marker(typed))
                })
                .child(field)
        });

        let hint = |keys: &'static str, text: &'static str| {
            h_flex().gap(px(4.)).child(KeyHint::new(keys)).child(
                Label::new(text)
                    .size(LabelSize::XSmall)
                    .color(Color::Placeholder),
            )
        };
        let next_label: SharedString = if last {
            prompt.submit_label.clone()
        } else {
            "Next".into()
        };

        v_flex()
            .gap(px(8.))
            .px(px(10.))
            .pt(px(10.))
            .pb(px(8.))
            .border_t_1()
            .border_color(colors.border_variant)
            .key_context(PROMPT_CONTEXT)
            .track_focus(&state.focus)
            .on_key_down(cx.listener({
                let key = key.clone();
                move |this, event, window, cx| this.form_key_down(&key, event, window, cx)
            }))
            // Enter in a text field, and the arrows it leaves to us.
            .on_action(cx.listener({
                let key = key.clone();
                move |this, _: &Submit, window, cx| this.form_next(&key, window, cx)
            }))
            .on_action(cx.listener({
                let key = key.clone();
                move |this, _: &text_input::Up, window, cx| this.form_move(&key, -1, window, cx)
            }))
            .on_action(cx.listener({
                let key = key.clone();
                move |this, _: &text_input::Down, window, cx| this.form_move(&key, 1, window, cx)
            }))
            .on_action(cx.listener({
                let key = key.clone();
                move |this, _: &PromptCancel, window, cx| this.form_cancel(&key, window, cx)
            }))
            .child(
                h_flex()
                    .items_start()
                    .gap(px(8.))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .gap(px(2.))
                            .when(!prompt.title.is_empty(), |this| {
                                this.child(
                                    Label::new(prompt.title.clone())
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                            })
                            .child(
                                Label::new(question.title.clone())
                                    .size(LabelSize::Small)
                                    .weight(FontWeight::MEDIUM),
                            )
                            .when_some(question.description.clone(), |this, description| {
                                this.child(
                                    Label::new(description)
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                            }),
                    )
                    .when(count > 1, |this| {
                        this.child(
                            Label::new(format!("{} / {count}", state.step + 1))
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                    }),
            )
            .when(!options.is_empty() || text_row.is_some(), |this| {
                this.child(v_flex().gap(px(2.)).children(options).children(text_row))
            })
            .child(
                // The key hints drop to a second line in a narrow chat.
                h_flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .when(state.step > 0, |this| {
                        this.child(
                            Button::new(("form-back", position), "Back")
                                .size(ButtonSize::Medium)
                                .on_click(cx.listener({
                                    let key = key.clone();
                                    move |this, _, window, cx| this.form_back(&key, window, cx)
                                })),
                        )
                    })
                    .child(
                        Button::new(("form-next", position), next_label)
                            .style(ButtonStyle::Tinted(TintColor::Accent))
                            .size(ButtonSize::Medium)
                            .disabled(question.required && !answered)
                            .on_click(cx.listener({
                                let key = key.clone();
                                move |this, _, window, cx| this.form_next(&key, window, cx)
                            })),
                    )
                    .child(
                        Button::new(("form-cancel", position), "Cancel")
                            .size(ButtonSize::Medium)
                            .on_click(cx.listener({
                                let key = key.clone();
                                move |this, _, window, cx| this.form_cancel(&key, window, cx)
                            })),
                    )
                    .child(
                        h_flex()
                            .ml_auto()
                            .flex_none()
                            .gap(px(6.))
                            .when(!question.options.is_empty(), |this| {
                                this.child(hint("↑↓", "move")).child(hint(
                                    "Space",
                                    if question.multiple { "toggle" } else { "pick" },
                                ))
                            })
                            .child(hint("Enter", if last { "submit" } else { "next" }))
                            .child(hint("Esc", "cancel")),
                    ),
            )
            .into_any_element()
    }
}
