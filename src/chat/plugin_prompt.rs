//! Text fields plugins ask for inside tool calls (`prompt.input`), e.g. the
//! value of a secret the agent requested. The field sits in the tool call
//! row, so the question and the call that needs it stay together.
//!
//! The answer goes to the plugin, never to pi: a secret typed here does not
//! enter the transcript. A prompt closes when it is answered or cancelled,
//! when the plugin closes it, or when its tool call ends.

use super::*;

pub(super) const PROMPT_CONTEXT: &str = "PluginPrompt";

/// The field of one open prompt in this chat.
pub(super) struct PromptField {
    plugin_id: SharedString,
    id: SharedString,
    call_id: SharedString,
    input: Entity<TextInput>,
}

impl ChatView {
    /// Matches fields to the registry's prompts for this chat's tool calls:
    /// new prompts get a field (and the keyboard), closed ones lose theirs.
    pub(super) fn sync_plugin_prompts(&mut self, cx: &mut Context<Self>) {
        let prompts: Vec<plugins::PluginPrompt> = self
            .plugins
            .read(cx)
            .prompts()
            .iter()
            .filter(|prompt| self.tool_rows.contains_key(prompt.call_id.as_ref()))
            .cloned()
            .collect();
        let mut changed = Vec::new();
        self.prompt_fields.retain(|field| {
            let open = prompts
                .iter()
                .any(|prompt| prompt.plugin_id == field.plugin_id && prompt.id == field.id);
            if !open && let Some(row) = self.tool_rows.get(field.call_id.as_ref()) {
                changed.push(*row);
            }
            open
        });
        for prompt in prompts {
            let existing = self
                .prompt_fields
                .iter()
                .find(|field| field.plugin_id == prompt.plugin_id && field.id == prompt.id);
            if let Some(field) = existing {
                // The plugin may re-ask with other settings (a new placeholder).
                let (placeholder, secret) = (prompt.placeholder.clone(), prompt.secret);
                field.input.update(cx, |input, cx| {
                    input.set_placeholder(placeholder, cx);
                    if !secret && input.is_masked() {
                        input.set_masked(false, cx);
                    }
                });
                continue;
            }
            let input = cx.new(|cx| {
                let mut input = TextInput::single_line(prompt.placeholder.clone(), cx);
                input.set_masked(prompt.secret, cx);
                input
            });
            if let Some(row) = self.tool_rows.get(prompt.call_id.as_ref()) {
                changed.push(*row);
            }
            self.prompt_fields.push(PromptField {
                plugin_id: prompt.plugin_id,
                id: prompt.id,
                call_id: prompt.call_id,
                input,
            });
            self.focus_prompt = true;
        }
        if !changed.is_empty() {
            self.sync_list(self.rows.len(), changed);
        }
    }

    /// Gives the keyboard to a prompt that just opened. Called from render.
    pub(super) fn focus_new_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.focus_prompt) {
            return;
        }
        if let Some(field) = self.prompt_fields.last() {
            window.focus(&field.input.read(cx).focus_handle(cx));
        }
    }

    /// pi finished the call: a prompt still open in it has nobody to answer.
    pub(super) fn close_call_prompts(&mut self, call_id: &str, cx: &mut Context<Self>) {
        if !self
            .prompt_fields
            .iter()
            .any(|field| field.call_id == call_id)
        {
            return;
        }
        self.plugins.update(cx, |registry, cx| {
            registry.close_prompts_for_call(call_id, cx)
        });
    }

    fn field_index(&self, id: &(SharedString, SharedString)) -> Option<usize> {
        self.prompt_fields
            .iter()
            .position(|field| field.plugin_id == id.0 && field.id == id.1)
    }

    fn answer_prompt(
        &mut self,
        id: &(SharedString, SharedString),
        submit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = self.field_index(id).map(|index| &self.prompt_fields[index]) else {
            return;
        };
        let value = submit.then(|| field.input.read(cx).text().trim().to_string());
        if value.as_ref().is_some_and(String::is_empty) {
            return;
        }
        let (plugin_id, id) = (field.plugin_id.clone(), field.id.clone());
        // Drop the text from memory now; the field goes away with the prompt.
        field.input.update(cx, |input, cx| input.clear(cx));
        self.plugins.update(cx, |registry, cx| {
            registry.answer_prompt(&plugin_id, &id, value, false, cx)
        });
        window.focus(&self.composer.read(cx).focus_handle(cx));
    }

    fn toggle_prompt_mask(&mut self, id: &(SharedString, SharedString), cx: &mut Context<Self>) {
        if let Some(field) = self.field_index(id).map(|index| &self.prompt_fields[index]) {
            field.input.update(cx, |input, cx| {
                let masked = input.is_masked();
                input.set_masked(!masked, cx);
            });
        }
    }

    /// The field of the prompt open in the tool call shown at row `index`.
    pub(super) fn render_plugin_prompt(
        &self,
        index: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let position = self
            .prompt_fields
            .iter()
            .position(|field| self.tool_rows.get(field.call_id.as_ref()) == Some(&index))?;
        let field = &self.prompt_fields[position];
        let key = (field.plugin_id.clone(), field.id.clone());
        let prompt = self
            .plugins
            .read(cx)
            .prompts()
            .iter()
            .find(|prompt| prompt.plugin_id == field.plugin_id && prompt.id == field.id)?
            .clone();
        let colors = cx.theme().colors.clone();
        let input = field.input.read(cx);
        let focused = input.focus_handle(cx).is_focused(window);
        let masked = input.is_masked();
        let empty = input.text().trim().is_empty();

        let hint = |keys: &'static str, text: &'static str| {
            h_flex().gap(px(4.)).child(KeyHint::new(keys)).child(
                Label::new(text)
                    .size(LabelSize::XSmall)
                    .color(Color::Placeholder),
            )
        };
        let field_box = h_flex()
            .flex_1()
            .min_w(px(0.))
            .rounded(px(6.))
            .border_1()
            .border_color(if focused {
                colors.border_focused
            } else {
                colors.border
            })
            .bg(colors.editor_background)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .py(px(2.))
                    .child(field.input.clone()),
            )
            .when(prompt.secret, |this| {
                this.child(
                    div().pr(px(3.)).child(
                        IconButton::new(
                            ("prompt-mask", position),
                            if masked {
                                IconName::Eye
                            } else {
                                IconName::EyeOff
                            },
                        )
                        .icon_size(IconSize::XSmall)
                        .tooltip(Tooltip::text(if masked { "Show" } else { "Hide" }))
                        .on_click(cx.listener({
                            let key = key.clone();
                            move |this, _, _, cx| this.toggle_prompt_mask(&key, cx)
                        })),
                    ),
                )
            });

        Some(
            v_flex()
                .gap(px(8.))
                .px(px(10.))
                .pt(px(10.))
                .pb(px(8.))
                .border_t_1()
                .border_color(colors.border_variant)
                .key_context(PROMPT_CONTEXT)
                .on_action(cx.listener({
                    let key = key.clone();
                    move |this, _: &Submit, window, cx| this.answer_prompt(&key, true, window, cx)
                }))
                .on_action(cx.listener({
                    let key = key.clone();
                    move |this, _: &PromptCancel, window, cx| {
                        this.answer_prompt(&key, false, window, cx)
                    }
                }))
                .child(
                    v_flex()
                        .gap(px(2.))
                        .child(
                            Label::new(prompt.title.clone())
                                .size(LabelSize::Small)
                                .weight(FontWeight::MEDIUM),
                        )
                        .when_some(prompt.description.clone(), |this, description| {
                            this.child(
                                Label::new(description)
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            )
                        }),
                )
                .child(
                    h_flex()
                        .items_start()
                        .gap(px(6.))
                        .child(field_box)
                        .child(
                            Button::new(("prompt-submit", position), prompt.submit_label.clone())
                                .style(ButtonStyle::Tinted(TintColor::Accent))
                                .size(ButtonSize::Medium)
                                .disabled(empty)
                                .on_click(cx.listener({
                                    let key = key.clone();
                                    move |this, _, window, cx| {
                                        this.answer_prompt(&key, true, window, cx)
                                    }
                                })),
                        )
                        .child(
                            Button::new(("prompt-cancel", position), "Cancel")
                                .size(ButtonSize::Medium)
                                .on_click(cx.listener({
                                    let key = key.clone();
                                    move |this, _, window, cx| {
                                        this.answer_prompt(&key, false, window, cx)
                                    }
                                })),
                        ),
                )
                .child(
                    h_flex()
                        .gap(px(12.))
                        .child(hint("Enter", "submit"))
                        .child(hint("Esc", "cancel"))
                        .when(prompt.secret, |this| {
                            this.child(
                                Label::new("Goes to the plugin, not to the model or the chat")
                                    .size(LabelSize::XSmall)
                                    .color(Color::Placeholder),
                            )
                        }),
                )
                .into_any_element(),
        )
    }
}
