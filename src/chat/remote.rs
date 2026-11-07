//! Session control for plugins with the `sessions` permission: prompts,
//! aborts, answers to extension questions and a set of pi RPC commands. What a
//! plugin does here shows in the transcript as if the user did it.

use super::*;
use crate::plugins::PluginReply;

/// pi commands a plugin may send through `sessions.rpc`. Prompts go through
/// `sessions.prompt` (the transcript shows them); fork, clone and session
/// switches change what this chat shows and stay with the user.
const RPC_ALLOWED: &[&str] = &[
    "get_state",
    "get_messages",
    "get_session_stats",
    "get_commands",
    "get_available_models",
    "get_available_thinking_levels",
    "get_last_assistant_text",
    "get_entries",
    "get_tree",
    "get_fork_messages",
    "set_model",
    "cycle_model",
    "set_thinking_level",
    "cycle_thinking_level",
    "set_session_name",
    "compact",
    "set_auto_compaction",
    "set_auto_retry",
    "abort_retry",
    "set_steering_mode",
    "set_follow_up_mode",
    "clear_queue",
    "bash",
    "abort_bash",
    "export_html",
];

/// Responses the chat applies to its own state after a plugin got them; the
/// rest (copy to the clipboard, open the export, the tree) belong to the
/// user's own commands.
const RPC_SYNCED: &[&str] = &[
    "get_state",
    "get_session_stats",
    "get_commands",
    "set_model",
    "set_thinking_level",
    "set_session_name",
    "compact",
];

/// Short ids ("1", "2", …) that a model can type; unique while the app runs.
pub(super) fn next_handle() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        .to_string()
}

pub(super) struct PluginCall {
    reply: PluginReply,
    command: String,
}

impl ChatView {
    /// The handle plugins use for this open session.
    pub fn handle(&self) -> &str {
        &self.handle
    }

    /// A summary for plugins (`sessions.list`, `sessions.get`, `session.state`).
    pub fn info(&self) -> Value {
        json!({
            "id": self.handle,
            "workspace": self.workspace,
            "file": self.session_file,
            "title": self.title(),
            "status": match self.status() {
                ChatStatus::Ready => "ready",
                ChatStatus::Busy => "busy",
                ChatStatus::Stopped => "stopped",
            },
            "running": self.is_running(),
            "compacting": self.compacting,
            "model": self.model.as_ref().map(|model| json!({
                "provider": model.provider,
                "id": model.id,
                "name": model.name,
            })),
            "thinkingLevel": self.thinking_level,
            "question": self.pending_dialog_info(),
        })
    }

    /// Sends a message like the composer does. While pi works it steers,
    /// unless `mode` is `followUp`.
    pub fn plugin_prompt(&mut self, params: &Value, reply: PluginReply, cx: &mut Context<Self>) {
        let message = params["message"].as_str().unwrap_or_default().trim();
        let error = if message.is_empty() {
            Some("message is empty")
        } else if self.compacting {
            Some("pi is compacting the context; try again when it is done")
        } else if self.navigating.is_some() || self.forking.is_some() {
            Some("the session is switching branches; try again in a moment")
        } else if !matches!(self.connection, Connection::Ready(_)) {
            Some("pi is not running in this session")
        } else {
            None
        };
        if let Some(error) = error {
            reply.finish(&self.plugins, Err(error.to_string()), cx);
            return;
        }
        let queued = self.running || self.pending_prompt;
        let mut command = json!({ "type": "prompt", "message": message });
        if queued {
            command["streamingBehavior"] = match params["mode"].as_str() {
                Some("followUp") => json!("followUp"),
                _ => json!("steer"),
            };
        }
        let previous_len = self.rows.len();
        match self.send(command) {
            Ok(id) => {
                self.rows.push(Row::User {
                    text: SharedString::from(message.to_string()),
                    steering: queued,
                    images: attachments::image_paths_in(message)
                        .into_iter()
                        .map(Picture::File)
                        .collect(),
                });
                self.pending_prompt = true;
                self.plugin_calls.insert(
                    id,
                    PluginCall {
                        reply,
                        command: "prompt".into(),
                    },
                );
                cx.emit(ChatEvent::Changed);
            }
            Err(error) => {
                self.push_notice(Tone::Error, error.clone());
                reply.finish(&self.plugins, Err(error), cx);
            }
        }
        self.sync_list(previous_len, Vec::new());
        cx.notify();
    }

    pub fn plugin_abort(&mut self, reply: PluginReply, cx: &mut Context<Self>) {
        if !self.running && !self.compacting && !self.pending_prompt {
            reply.finish(&self.plugins, Ok(json!({ "aborted": false })), cx);
            return;
        }
        self.plugin_command(json!({ "type": "abort" }), reply, cx);
    }

    pub fn plugin_answer(&mut self, params: &Value, reply: PluginReply, cx: &mut Context<Self>) {
        let id = params["question"].as_str().unwrap_or_default();
        let result = self
            .answer_dialog_for_plugin(id, params, cx)
            .map(|()| json!({}));
        reply.finish(&self.plugins, result, cx);
    }

    pub fn plugin_rpc(&mut self, params: &Value, reply: PluginReply, cx: &mut Context<Self>) {
        let command = &params["command"];
        let kind = command["type"].as_str().unwrap_or_default();
        if !RPC_ALLOWED.contains(&kind) {
            let error = format!(
                "pi command \"{kind}\" is not available to plugins; allowed: {}",
                RPC_ALLOWED.join(", ")
            );
            reply.finish(&self.plugins, Err(error), cx);
            return;
        }
        self.plugin_command(command.clone(), reply, cx);
    }

    fn plugin_command(&mut self, command: Value, reply: PluginReply, cx: &mut Context<Self>) {
        let kind = command["type"].as_str().unwrap_or_default().to_string();
        match self.send(command) {
            Ok(id) => {
                self.plugin_calls.insert(
                    id,
                    PluginCall {
                        reply,
                        command: kind,
                    },
                );
            }
            Err(error) => reply.finish(&self.plugins, Err(error), cx),
        }
    }

    /// Answers the plugin that sent this command. Returns true when the chat
    /// should not apply the response itself.
    pub(super) fn finish_plugin_call(&mut self, record: &Value, cx: &mut Context<Self>) -> bool {
        let Some(call) = record["id"]
            .as_str()
            .and_then(|id| self.plugin_calls.remove(id))
        else {
            return false;
        };
        let result = if record["success"].as_bool() == Some(false) {
            Err(record["error"]
                .as_str()
                .unwrap_or("unknown error")
                .to_string())
        } else {
            Ok(record.get("data").cloned().unwrap_or(json!({})))
        };
        let own_state = call.command == "prompt" || RPC_SYNCED.contains(&call.command.as_str());
        call.reply.finish(&self.plugins, result, cx);
        !own_state
    }

    /// pi exited: nothing will answer the open calls.
    pub(super) fn fail_plugin_calls(&mut self, cx: &mut Context<Self>) {
        for (_, call) in self.plugin_calls.drain() {
            call.reply
                .finish(&self.plugins, Err("pi exited".to_string()), cx);
        }
    }

    /// Hands this batch's pi records to the plugins that follow the session.
    pub(super) fn forward_to_plugins(&mut self, batch: &[JsonlMessage], cx: &mut Context<Self>) {
        if !self.plugins.read(cx).wants_session_events(&self.handle) {
            return;
        }
        let events: Vec<Value> = batch
            .iter()
            .filter_map(|message| match message {
                // Answers to commands go to whoever sent them.
                JsonlMessage::Record(record) if record["type"] != "response" => {
                    Some(record.clone())
                }
                JsonlMessage::Record(_) => None,
                JsonlMessage::Malformed(_) => None,
                JsonlMessage::Exited { status, .. } => Some(json!({
                    "type": "process_exited",
                    "code": status.and_then(|status| status.code()),
                })),
            })
            .collect();
        let handle = self.handle.clone();
        self.plugins.update(cx, |registry, cx| {
            registry.session_events(&handle, events, cx)
        });
    }
}
