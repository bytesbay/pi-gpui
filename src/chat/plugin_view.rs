//! Live plugin views in tool call rows (`contributes.toolViews`).
//!
//! When pi calls a tool that a plugin draws (say `doom`), the row shows the
//! frames the plugin streams. A plugin streams to one call at a time: a newer
//! call of the same tool takes the stream over, and older rows keep their
//! last frame. Clicking the view gives it the keyboard; keys go to the plugin
//! until Esc or a click elsewhere.

use gpui::{KeyDownEvent, KeyUpEvent, ModifiersChangedEvent};

use super::*;

pub(super) const PLUGIN_VIEW_CONTEXT: &str = "PluginView";
/// The widest a view gets; it also never exceeds the transcript column.
const MAX_VIEW_WIDTH: f32 = 640.;

impl ChatView {
    /// Called for every `tool_execution_start`: moves the plugin's stream to
    /// this call if a plugin draws the tool.
    pub(super) fn open_plugin_view(
        &mut self,
        record: &Value,
        call_id: &str,
        cx: &mut Context<Self>,
    ) {
        let tool = record["toolName"].as_str().unwrap_or_default();
        if self.plugins.read(cx).tool_view(tool).is_none() {
            return;
        }
        if let Some(Row::Tool { view, .. }) = self.rows.last_mut() {
            *view = Some(SharedString::from(call_id.to_string()));
        }
        let context = CommandContext {
            workspace: Some(self.workspace.clone()),
            session: self.session_file.clone(),
        };
        let args = record["args"].clone();
        self.plugins.update(cx, |registry, cx| {
            registry.open_view(tool, call_id, &args, context, cx)
        });
    }

    pub(super) fn render_plugin_view(
        &self,
        call_id: &SharedString,
        tool: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let registry = self.plugins.read(cx);
        let Some(view) = registry.tool_view(tool).cloned() else {
            return div().into_any_element();
        };
        let frames = registry.frames().read(cx);
        let frame = frames.frame(call_id);
        let live = frames.is_live(call_id);
        let focused = self
            .focused_view
            .as_ref()
            .is_some_and(|(focused, _)| focused == call_id);

        let hint = if !live {
            "Playing in a newer message"
        } else if focused {
            "Esc to release the keyboard"
        } else {
            "Click to play"
        };
        let screen = div()
            .relative()
            .w(px(MAX_VIEW_WIDTH))
            .max_w_full()
            .overflow_hidden()
            .rounded(px(6.))
            .border_1()
            .border_color(if focused {
                colors.border_focused
            } else {
                colors.border_variant
            })
            .bg(gpui::black())
            .child(
                div()
                    .w(px(MAX_VIEW_WIDTH))
                    .max_w_full()
                    .h(px(MAX_VIEW_WIDTH * view.aspect))
                    .flex()
                    .items_center()
                    .justify_center()
                    .map(|this| match frame {
                        Some(frame) => this.child(
                            gpui::img(gpui::ImageSource::Render(frame))
                                .size_full()
                                .object_fit(gpui::ObjectFit::Contain),
                        ),
                        None => this.child(
                            h_flex()
                                .gap(px(6.))
                                .child(Spinner::new().size(IconSize::XSmall))
                                .child(
                                    Label::new(format!("Starting {}…", view.title))
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                ),
                        ),
                    }),
            )
            .when(!live, |this| this.opacity(0.55));

        let call = call_id.clone();
        v_flex()
            .id(ElementId::Name(format!("plugin-view:{call_id}").into()))
            .px(px(8.))
            .py(px(8.))
            .gap(px(6.))
            .border_t_1()
            .border_color(colors.border_variant)
            .when(live, |this| {
                this.cursor_pointer()
                    .key_context(PLUGIN_VIEW_CONTEXT)
                    .when(focused, |this| this.track_focus(&self.view_focus))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.focus_plugin_view(call.clone(), window, cx)
                    }))
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        this.plugin_view_key(event.keystroke.key.as_str(), true, event.is_held, cx);
                        cx.stop_propagation();
                    }))
                    .on_key_up(cx.listener(|this, event: &KeyUpEvent, _, cx| {
                        this.plugin_view_key(event.keystroke.key.as_str(), false, false, cx);
                        cx.stop_propagation();
                    }))
                    .on_modifiers_changed(cx.listener(
                        |this, event: &ModifiersChangedEvent, _, cx| {
                            this.plugin_view_modifiers(event.modifiers, cx)
                        },
                    ))
            })
            .child(screen)
            .child(Label::new(hint).size(LabelSize::XSmall).color(if focused {
                Color::Accent
            } else {
                Color::Placeholder
            }))
            .into_any_element()
    }

    fn focus_plugin_view(
        &mut self,
        call_id: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .focused_view
            .as_ref()
            .is_some_and(|(focused, _)| *focused == call_id)
        {
            return;
        }
        self.blur_plugin_view(cx);
        self.focused_view = Some((call_id.clone(), window.modifiers()));
        window.focus(&self.view_focus);
        self.plugins.update(cx, |registry, cx| {
            registry.send_view_input(&call_id, json!({ "focused": true }), cx)
        });
        cx.notify();
    }

    /// Tells the plugin the view lost the keyboard, so it can release held keys.
    fn blur_plugin_view(&mut self, cx: &mut Context<Self>) {
        if let Some((call_id, _)) = self.focused_view.take() {
            self.plugins.update(cx, |registry, cx| {
                registry.send_view_input(&call_id, json!({ "focused": false }), cx)
            });
            cx.notify();
        }
    }

    pub(super) fn release_plugin_view(
        &mut self,
        _: &ReleasePluginView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.blur_plugin_view(cx);
        window.focus(&self.composer.read(cx).focus_handle(cx));
    }

    /// A click elsewhere moved focus away: release the view. Called from render.
    pub(super) fn sync_plugin_view_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.focused_view.is_some() && !self.view_focus.is_focused(window) {
            self.blur_plugin_view(cx);
        }
    }

    fn plugin_view_key(&mut self, key: &str, down: bool, held: bool, cx: &mut Context<Self>) {
        if held {
            return;
        }
        let Some((call_id, _)) = self.focused_view.clone() else {
            return;
        };
        self.plugins.update(cx, |registry, cx| {
            registry.send_view_input(&call_id, json!({ "key": key, "down": down }), cx)
        });
    }

    /// Modifiers arrive as state changes; the plugin gets them as keys.
    fn plugin_view_modifiers(&mut self, modifiers: gpui::Modifiers, cx: &mut Context<Self>) {
        let Some((call_id, previous)) = self.focused_view.as_mut() else {
            return;
        };
        let changes = [
            ("control", previous.control, modifiers.control),
            ("shift", previous.shift, modifiers.shift),
            ("alt", previous.alt, modifiers.alt),
        ];
        *previous = modifiers;
        let call_id = call_id.clone();
        for (key, before, now) in changes {
            if before != now {
                self.plugins.update(cx, |registry, cx| {
                    registry.send_view_input(&call_id, json!({ "key": key, "down": now }), cx)
                });
            }
        }
    }
}
