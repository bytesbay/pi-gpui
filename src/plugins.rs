//! Plugin host: discovers app plugins, starts each one on first use and routes
//! its messages. The manifest and protocol are specified in docs/PLUGINS.md.
//!
//! A plugin is a separate process speaking line-delimited JSON over stdio, so
//! it can be written in any language and cannot crash or block the app.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{
    AppContext as _, AsyncApp, Context, Entity, EventEmitter, KeyBinding, Keystroke, SharedString,
    Task, WeakEntity,
};
use serde_json::{Value, json};

mod settings;

pub use settings::{PluginSetting, SettingKind};

use crate::jsonl::{JsonlMessage, JsonlProcess};
use crate::pi;
use crate::sessions;
use crate::ui::{Color, IconName};

pub const PROTOCOL_VERSION: u64 = 1;
const MAX_MESSAGES_PER_UPDATE: usize = 256;

/// Runs a plugin command; bound to the keybindings from plugin manifests.
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = plugins, no_json)]
pub struct RunPluginCommand {
    pub command: SharedString,
}

#[derive(Clone)]
pub struct PluginCommand {
    pub id: SharedString,
    pub plugin_id: SharedString,
    pub title: SharedString,
    pub icon: IconName,
    pub keybinding: Option<SharedString>,
    pub in_composer: bool,
}

#[derive(Clone)]
pub struct StatusItem {
    pub plugin_id: SharedString,
    pub id: SharedString,
    pub text: SharedString,
    pub color: Color,
    pub busy: bool,
}

/// A live view a plugin draws inside the tool call rows of one tool
/// (`contributes.toolViews`), e.g. a game for the `doom` tool.
#[derive(Clone)]
pub struct ToolView {
    pub plugin_id: SharedString,
    pub tool: SharedString,
    pub title: SharedString,
    /// Height divided by width.
    pub aspect: f32,
}

/// A text field a plugin asks for inside a tool call (`prompt.input`), for
/// example the value of a secret the agent requested.
#[derive(Clone)]
pub struct PluginPrompt {
    pub plugin_id: SharedString,
    pub id: SharedString,
    pub call_id: SharedString,
    pub title: SharedString,
    pub description: Option<SharedString>,
    pub placeholder: SharedString,
    pub submit_label: SharedString,
    /// Typed text is shown as dots and never copied.
    pub secret: bool,
}

/// Frames of plugin views. Separate from the registry so a frame only
/// re-renders the chats, not everything that watches plugins.
#[derive(Default)]
pub struct ViewFrames {
    /// The last frame of each tool call that showed a view. Decoded here, so
    /// a frame paints at once (an encoded image would load asynchronously and
    /// blink) and does not pile up in the asset cache.
    frames: HashMap<SharedString, Arc<gpui::RenderImage>>,
    /// The call each plugin streams to: one at a time, the newest call wins.
    live: HashMap<SharedString, SharedString>,
}

impl ViewFrames {
    pub fn frame(&self, call_id: &str) -> Option<Arc<gpui::RenderImage>> {
        self.frames.get(call_id).cloned()
    }

    pub fn is_live(&self, call_id: &str) -> bool {
        self.live.values().any(|live| live == call_id)
    }
}

/// Where a command was invoked; sent to the plugin with `command.run`.
#[derive(Clone, Default)]
pub struct CommandContext {
    pub workspace: Option<PathBuf>,
    pub session: Option<PathBuf>,
}

/// A request from a plugin that the hub answers (`sessions.*`), see
/// "Requests" in docs/PLUGINS.md.
#[derive(Clone)]
pub struct PluginRequest {
    pub reply: PluginReply,
    pub method: String,
    pub params: Value,
}

/// Where the answer to a plugin request goes. Answer exactly once.
#[derive(Clone)]
pub struct PluginReply {
    plugin_id: SharedString,
    id: Value,
}

impl PluginReply {
    pub fn finish(
        self,
        plugins: &Entity<PluginRegistry>,
        result: Result<Value, String>,
        cx: &mut gpui::App,
    ) {
        plugins.update(cx, |registry, _| registry.respond(self, result));
    }
}

pub enum PluginEvent {
    Request(PluginRequest),
    ComposerInsert {
        text: String,
        replace_last: bool,
        space_before: bool,
    },
    ComposerSetText(String),
    Notify {
        message: String,
        color: Color,
    },
}

enum PluginState {
    Stopped,
    Starting,
    Running(JsonlProcess),
}

struct Plugin {
    id: SharedString,
    name: SharedString,
    dir: PathBuf,
    run: Vec<String>,
    /// `"activation": "startup"`: started with the app, not on first use.
    startup: bool,
    /// `"permissions": ["sessions"]`: may read and drive pi sessions.
    sessions: bool,
    /// `"permissions": ["app"]`: may check for updates and restart the app.
    app: bool,
    state: PluginState,
    /// Messages for the plugin while its process is starting.
    pending: Vec<Value>,
    _start_task: Task<()>,
    _pump_task: Task<()>,
}

pub struct PluginRegistry {
    plugins: Vec<Plugin>,
    commands: Vec<PluginCommand>,
    active_commands: HashSet<SharedString>,
    statuses: Vec<StatusItem>,
    tool_views: Vec<ToolView>,
    /// Tools whose calls a plugin follows (`contributes.toolEvents`): plugin
    /// id and tool name.
    tool_events: Vec<(SharedString, SharedString)>,
    /// Open `prompt.input` fields, oldest first.
    prompts: Vec<PluginPrompt>,
    /// pi extensions plugins ship (`contributes.piExtensions`), loaded into
    /// every pi this app starts.
    pi_extensions: Vec<PathBuf>,
    /// `sessions.subscribe`: plugin id and the session handle it follows
    /// (`None` for all sessions).
    session_subscriptions: Vec<(SharedString, Option<String>)>,
    /// Settings plugins declare (`contributes.settings`), in manifest order.
    settings: Vec<PluginSetting>,
    /// The settings each running plugin last got, to send only changes.
    sent_settings: HashMap<SharedString, Value>,
    frames: Entity<ViewFrames>,
    _load_task: Task<()>,
}

impl EventEmitter<PluginEvent> for PluginRegistry {}

impl PluginRegistry {
    /// Starts with no plugins and reads manifests in the background: plugin
    /// folders can sit in places macOS guards (e.g. ~/Documents), and a
    /// permission prompt must never block the first frame.
    pub fn new(key_context: &'static str, cx: &mut Context<Self>) -> Self {
        let discovery = cx.background_spawn(async { discover_plugins() });
        // settings.json changed: running plugins get their new values.
        cx.observe_global::<crate::settings::Settings>(|this: &mut Self, cx| {
            this.send_settings(cx)
        })
        .detach();
        let load_task = cx.spawn(async move |this, cx| {
            let found = discovery.await;
            this.update(cx, |this, cx| this.register(found, key_context, cx))
                .ok();
        });
        Self {
            plugins: Vec::new(),
            commands: Vec::new(),
            active_commands: HashSet::new(),
            statuses: Vec::new(),
            tool_views: Vec::new(),
            tool_events: Vec::new(),
            prompts: Vec::new(),
            pi_extensions: Vec::new(),
            session_subscriptions: Vec::new(),
            settings: Vec::new(),
            sent_settings: HashMap::new(),
            frames: cx.new(|_| ViewFrames::default()),
            _load_task: load_task,
        }
    }

    fn register(
        &mut self,
        found: Vec<Result<(PathBuf, ParsedManifest), String>>,
        key_context: &'static str,
        cx: &mut Context<Self>,
    ) {
        let mut errors = Vec::new();
        let mut bindings = Vec::new();
        for result in found {
            let (plugin_dir, parsed) = match result {
                Ok(found) => found,
                Err(error) => {
                    errors.push(error);
                    continue;
                }
            };
            if self.plugins.iter().any(|plugin| plugin.id == parsed.id) {
                errors.push(format!(
                    "Plugin {} is already loaded; skipped the copy in {}.",
                    parsed.id,
                    plugin_dir.display()
                ));
                continue;
            }
            for command in &parsed.commands {
                if let Some(keys) = &command.keybinding {
                    bindings.push(KeyBinding::new(
                        &crate::keys::binding(keys),
                        RunPluginCommand {
                            command: command.id.clone(),
                        },
                        Some(key_context),
                    ));
                }
            }
            self.commands.extend(parsed.commands);
            self.tool_views.extend(parsed.tool_views);
            self.tool_events.extend(
                parsed
                    .tool_events
                    .into_iter()
                    .map(|tool| (parsed.id.clone(), tool)),
            );
            self.pi_extensions.extend(parsed.pi_extensions);
            self.settings.extend(parsed.settings);
            self.plugins.push(Plugin {
                id: parsed.id,
                name: parsed.name,
                dir: plugin_dir,
                run: parsed.run,
                startup: parsed.startup,
                sessions: parsed.sessions,
                app: parsed.app,
                state: PluginState::Stopped,
                pending: Vec::new(),
                _start_task: Task::ready(()),
                _pump_task: Task::ready(()),
            });
        }
        cx.bind_keys(bindings);
        let startup: Vec<SharedString> = self
            .plugins
            .iter()
            .filter(|plugin| plugin.startup)
            .map(|plugin| plugin.id.clone())
            .collect();
        for plugin_id in startup {
            self.start(plugin_id, cx);
        }
        eprintln!(
            "pi-gpui: loaded {} plugin(s): {}",
            self.plugins.len(),
            self.plugins
                .iter()
                .map(|plugin| plugin.id.as_ref())
                .collect::<Vec<_>>()
                .join(", ")
        );
        if !errors.is_empty() {
            for error in &errors {
                eprintln!("pi-gpui: plugin not loaded: {error}");
            }
            cx.emit(PluginEvent::Notify {
                message: format!("Plugins not loaded: {}", errors.join("; ")),
                color: Color::Warning,
            });
        }
        cx.notify();
    }

    pub fn composer_commands(&self) -> impl Iterator<Item = &PluginCommand> {
        self.commands.iter().filter(|command| command.in_composer)
    }

    pub fn is_command_active(&self, command: &str) -> bool {
        self.active_commands.contains(command)
    }

    pub fn statuses(&self) -> &[StatusItem] {
        &self.statuses
    }

    pub fn has_busy_status(&self) -> bool {
        self.statuses.iter().any(|status| status.busy)
    }

    pub fn tool_view(&self, tool: &str) -> Option<&ToolView> {
        self.tool_views.iter().find(|view| view.tool == tool)
    }

    /// Plugins with settings, in load order, with their settings.
    pub fn settings_sections(&self) -> Vec<(SharedString, Vec<PluginSetting>)> {
        self.plugins
            .iter()
            .filter_map(|plugin| {
                let settings: Vec<PluginSetting> = self
                    .settings
                    .iter()
                    .filter(|setting| setting.plugin_id == plugin.id)
                    .cloned()
                    .collect();
                (!settings.is_empty()).then(|| (plugin.name.clone(), settings))
            })
            .collect()
    }

    pub fn setting(&self, full_key: &str) -> Option<&PluginSetting> {
        self.settings
            .iter()
            .find(|setting| setting.full_key() == full_key)
    }

    fn settings_values(&self, plugin_id: &str, cx: &gpui::App) -> Value {
        settings::values(
            &self.settings,
            plugin_id,
            crate::settings::Settings::get(cx),
        )
    }

    /// `settings.changed` to running plugins whose values changed.
    fn send_settings(&mut self, cx: &mut Context<Self>) {
        let running: Vec<SharedString> = self
            .plugins
            .iter()
            .filter(|plugin| matches!(plugin.state, PluginState::Running(_)))
            .map(|plugin| plugin.id.clone())
            .collect();
        for plugin_id in running {
            let values = self.settings_values(&plugin_id, cx);
            if values.as_object().is_none_or(|values| values.is_empty())
                || self.sent_settings.get(&plugin_id) == Some(&values)
            {
                continue;
            }
            self.sent_settings.insert(plugin_id.clone(), values.clone());
            let message = json!({ "method": "settings.changed", "params": { "settings": values } });
            self.send(&plugin_id, message, cx);
        }
    }

    /// `settings.get` and `settings.set`: a plugin's own settings only.
    fn apply_settings_request(
        &mut self,
        reply: PluginReply,
        method: &str,
        params: &Value,
        cx: &mut Context<Self>,
    ) {
        let plugin_id = reply.plugin_id.clone();
        let result = match method {
            "settings.get" => Ok(json!({ "settings": self.settings_values(&plugin_id, cx) })),
            "settings.set" => {
                let key = params["key"].as_str().unwrap_or_default();
                match self
                    .settings
                    .iter()
                    .find(|setting| setting.plugin_id == plugin_id && setting.key == key)
                {
                    None => Err(format!("no setting \"{key}\" in this plugin's manifest")),
                    Some(setting) => match setting.validate(&params["value"]) {
                        None => Err(format!("value does not fit the setting \"{key}\"")),
                        Some(value) => {
                            let full_key = setting.full_key();
                            crate::settings::set(cx, &full_key, value);
                            Ok(json!({}))
                        }
                    },
                }
            }
            _ => Err(format!("unknown method {method}")),
        };
        self.respond(reply, result);
    }

    pub fn pi_extensions(&self) -> &[PathBuf] {
        &self.pi_extensions
    }

    pub fn frames(&self) -> &Entity<ViewFrames> {
        &self.frames
    }

    /// A new call of a tool with a view: the plugin's stream moves to it.
    pub fn open_view(
        &mut self,
        tool: &str,
        call_id: &str,
        args: &Value,
        context: CommandContext,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.tool_view(tool).cloned() else {
            return;
        };
        let call_id = SharedString::from(call_id.to_string());
        self.frames.update(cx, |frames, cx| {
            frames.live.insert(view.plugin_id.clone(), call_id.clone());
            cx.notify();
        });
        let message = json!({
            "method": "view.open",
            "params": {
                "tool": view.tool,
                "callId": call_id,
                "args": args,
                "workspace": context.workspace,
                "session": context.session,
            },
        });
        self.send(&view.plugin_id, message, cx);
    }

    /// pi started a tool call: tells the plugins that follow this tool.
    pub fn tool_started(
        &mut self,
        tool: &str,
        call_id: &str,
        args: &Value,
        context: &CommandContext,
        cx: &mut Context<Self>,
    ) {
        let followers: Vec<SharedString> = self
            .tool_events
            .iter()
            .filter(|(_, followed)| followed == tool)
            .map(|(plugin_id, _)| plugin_id.clone())
            .collect();
        for plugin_id in followers {
            let message = json!({
                "method": "tool.start",
                "params": {
                    "tool": tool,
                    "callId": call_id,
                    "args": args,
                    "workspace": context.workspace,
                    "session": context.session,
                },
            });
            self.send(&plugin_id, message, cx);
        }
    }

    pub fn prompts(&self) -> &[PluginPrompt] {
        &self.prompts
    }

    /// Sends the answer to an open prompt and removes it. `None` cancels;
    /// `closed` tells the plugin nobody waits for an answer anymore (the tool
    /// call ended).
    pub fn answer_prompt(
        &mut self,
        plugin_id: &str,
        id: &str,
        value: Option<String>,
        closed: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(position) = self
            .prompts
            .iter()
            .position(|prompt| prompt.plugin_id == plugin_id && prompt.id == id)
        else {
            return;
        };
        let prompt = self.prompts.remove(position);
        let params = match value {
            Some(value) => json!({ "id": prompt.id, "value": value }),
            None => json!({ "id": prompt.id, "cancelled": true, "closed": closed }),
        };
        self.send(
            &prompt.plugin_id,
            json!({ "method": "prompt.result", "params": params }),
            cx,
        );
        cx.notify();
    }

    /// A tool call ended: its prompts have nobody to answer anymore.
    pub fn close_prompts_for_call(&mut self, call_id: &str, cx: &mut Context<Self>) {
        let open: Vec<(SharedString, SharedString)> = self
            .prompts
            .iter()
            .filter(|prompt| prompt.call_id == call_id)
            .map(|prompt| (prompt.plugin_id.clone(), prompt.id.clone()))
            .collect();
        for (plugin_id, id) in open {
            self.answer_prompt(&plugin_id, &id, None, true, cx);
        }
    }

    fn open_prompt(&mut self, plugin_id: &SharedString, params: &Value) {
        let text = |key: &str| {
            params[key]
                .as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(|text| SharedString::from(text.to_string()))
        };
        let (Some(id), Some(call_id)) = (text("id"), text("callId")) else {
            eprintln!("pi-gpui: plugin {plugin_id}: prompt.input needs an id and a callId");
            return;
        };
        let secret = params["secret"].as_bool().unwrap_or(false);
        let prompt = PluginPrompt {
            plugin_id: plugin_id.clone(),
            title: text("title").unwrap_or_else(|| "Input".into()),
            description: text("description"),
            placeholder: text("placeholder").unwrap_or_else(|| {
                if secret {
                    "Paste or type the value".into()
                } else {
                    "Type your answer".into()
                }
            }),
            submit_label: text("submitLabel").unwrap_or_else(|| "Submit".into()),
            secret,
            id,
            call_id,
        };
        match self
            .prompts
            .iter_mut()
            .find(|open| open.plugin_id == *plugin_id && open.id == prompt.id)
        {
            Some(open) => *open = prompt,
            None => self.prompts.push(prompt),
        }
    }

    /// Keyboard input for a live view; ignored for calls that no longer stream.
    pub fn send_view_input(&mut self, call_id: &str, params: Value, cx: &mut Context<Self>) {
        let Some(plugin_id) = self
            .frames
            .read(cx)
            .live
            .iter()
            .find(|(_, live)| live.as_ref() == call_id)
            .map(|(plugin_id, _)| plugin_id.clone())
        else {
            return;
        };
        let mut params = params;
        params["callId"] = json!(call_id);
        let method = if params.get("focused").is_some() {
            "view.focus"
        } else {
            "view.key"
        };
        self.send(
            &plugin_id,
            json!({ "method": method, "params": params }),
            cx,
        );
    }

    /// Answers a plugin request; dropped when the plugin is no longer running.
    pub fn respond(&mut self, reply: PluginReply, result: Result<Value, String>) {
        let Some(plugin) = self.plugin_mut(&reply.plugin_id) else {
            return;
        };
        let PluginState::Running(process) = &plugin.state else {
            return;
        };
        let message = match result {
            Ok(result) => json!({ "id": reply.id, "result": result }),
            Err(message) => json!({ "id": reply.id, "error": { "message": message } }),
        };
        process.send(message).ok();
    }

    /// Whether any plugin follows this session's events, so a chat clones
    /// its records only when someone listens.
    pub fn wants_session_events(&self, session: &str) -> bool {
        self.session_subscriptions.iter().any(|(_, followed)| {
            followed
                .as_deref()
                .is_none_or(|followed| followed == session)
        })
    }

    fn session_followers(&self, session: &str) -> Vec<SharedString> {
        let mut followers: Vec<SharedString> = Vec::new();
        for (plugin_id, followed) in &self.session_subscriptions {
            if followed
                .as_deref()
                .is_none_or(|followed| followed == session)
                && !followers.contains(plugin_id)
            {
                followers.push(plugin_id.clone());
            }
        }
        followers
    }

    /// pi records of one session, in order, for the plugins that follow it.
    pub fn session_events(&mut self, session: &str, events: Vec<Value>, cx: &mut Context<Self>) {
        if events.is_empty() {
            return;
        }
        let message = json!({
            "method": "session.events",
            "params": { "session": session, "events": events },
        });
        for plugin_id in self.session_followers(session) {
            self.send(&plugin_id, message.clone(), cx);
        }
    }

    /// A session's summary changed (title, file, running) or it was opened.
    pub fn session_state(&mut self, session: &str, info: Value, cx: &mut Context<Self>) {
        let message = json!({ "method": "session.state", "params": { "session": info } });
        for plugin_id in self.session_followers(session) {
            self.send(&plugin_id, message.clone(), cx);
        }
    }

    pub fn session_closed(&mut self, session: &str, cx: &mut Context<Self>) {
        let message = json!({ "method": "session.closed", "params": { "session": session } });
        for plugin_id in self.session_followers(session) {
            self.send(&plugin_id, message.clone(), cx);
        }
    }

    /// A message with an `id` is a request and always gets an answer.
    fn apply_request(
        &mut self,
        plugin_id: &SharedString,
        id: Value,
        method: &str,
        params: &Value,
        cx: &mut Context<Self>,
    ) {
        let reply = PluginReply {
            plugin_id: plugin_id.clone(),
            id,
        };
        if method.starts_with("settings.") {
            self.apply_settings_request(reply, method, params, cx);
            return;
        }
        let Some((sessions, app)) = self
            .plugins
            .iter()
            .find(|plugin| plugin.id == *plugin_id)
            .map(|plugin| (plugin.sessions, plugin.app))
        else {
            return;
        };
        let (allowed, permission) = if method.starts_with("sessions.") {
            (sessions, "sessions")
        } else if method.starts_with("app.") {
            (app, "app")
        } else {
            self.respond(reply, Err(format!("unknown method {method}")));
            return;
        };
        if !allowed {
            let prefix = method.split('.').next().unwrap_or_default();
            self.respond(
                reply,
                Err(format!(
                    "{prefix}.* needs \"permissions\": [\"{permission}\"] in plugin.json"
                )),
            );
            return;
        }
        match method {
            "sessions.subscribe" | "sessions.unsubscribe" => {
                let session = params["session"]
                    .as_str()
                    .filter(|session| *session != "*")
                    .map(str::to_string);
                self.session_subscriptions
                    .retain(|(owner, followed)| !(owner == plugin_id && *followed == session));
                if method == "sessions.subscribe" {
                    self.session_subscriptions
                        .push((plugin_id.clone(), session));
                }
                self.respond(reply, Ok(json!({})));
            }
            _ => cx.emit(PluginEvent::Request(PluginRequest {
                reply,
                method: method.to_string(),
                params: params.clone(),
            })),
        }
    }

    fn apply_frame(&mut self, plugin_id: &SharedString, params: &Value, cx: &mut Context<Self>) {
        use base64::Engine as _;
        let Some(call_id) = params["callId"].as_str() else {
            return;
        };
        // Only the call the plugin streams to; late frames for an older one are dropped.
        if self
            .frames
            .read(cx)
            .live
            .get(plugin_id)
            .map(|live| live.as_ref())
            != Some(call_id)
        {
            return;
        }
        let Some(bytes) = params["data"]
            .as_str()
            .and_then(|data| base64::engine::general_purpose::STANDARD.decode(data).ok())
        else {
            return;
        };
        let Ok(decoded) = image::load_from_memory(&bytes) else {
            return;
        };
        let mut pixels = decoded.into_rgba8();
        // GPUI textures are BGRA.
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        let frame = Arc::new(gpui::RenderImage::new(vec![image::Frame::new(pixels)]));
        let call_id = SharedString::from(call_id.to_string());
        let previous = self.frames.update(cx, |frames, cx| {
            cx.notify();
            frames.frames.insert(call_id, frame)
        });
        // Free the replaced frame's texture; a stream would fill the atlas otherwise.
        if let Some(previous) = previous {
            cx.drop_image(previous, None);
        }
    }

    pub fn run_command(&mut self, command: &str, context: CommandContext, cx: &mut Context<Self>) {
        let Some(plugin_id) = self
            .commands
            .iter()
            .find(|candidate| candidate.id == command)
            .map(|candidate| candidate.plugin_id.clone())
        else {
            return;
        };
        let message = json!({
            "method": "command.run",
            "params": {
                "command": command,
                "workspace": context.workspace,
                "session": context.session,
            },
        });
        self.send(&plugin_id, message, cx);
    }

    fn plugin_mut(&mut self, plugin_id: &str) -> Option<&mut Plugin> {
        self.plugins
            .iter_mut()
            .find(|plugin| plugin.id == plugin_id)
    }

    /// Delivers `message`, starting the plugin first if it is not running.
    fn send(&mut self, plugin_id: &SharedString, message: Value, cx: &mut Context<Self>) {
        let Some(plugin) = self.plugin_mut(plugin_id) else {
            return;
        };
        match &plugin.state {
            PluginState::Running(process) => {
                if let Err(error) = process.send(message) {
                    let name = plugin.name.clone();
                    self.notify(format!("Plugin \"{name}\" is unavailable: {error}"), cx);
                }
            }
            PluginState::Starting => plugin.pending.push(message),
            PluginState::Stopped => {
                plugin.pending.push(message);
                self.start(plugin_id.clone(), cx);
            }
        }
    }

    fn start(&mut self, plugin_id: SharedString, cx: &mut Context<Self>) {
        let Some(plugin) = self.plugin_mut(&plugin_id) else {
            return;
        };
        plugin.state = PluginState::Starting;
        let directory = plugin.dir.clone();
        let run = plugin.run.clone();
        // Resolving `node` & co. can need a login shell, which takes a moment.
        let resolve = cx.background_spawn(async move { resolve_program(&directory, &run) });
        plugin._start_task = cx.spawn(async move |this, cx| {
            let resolved = resolve.await;
            this.update(cx, |this, cx| this.finish_start(plugin_id, resolved, cx))
                .ok();
        });
    }

    fn finish_start(
        &mut self,
        plugin_id: SharedString,
        resolved: Result<(PathBuf, Vec<String>), String>,
        cx: &mut Context<Self>,
    ) {
        let settings = self.settings_values(&plugin_id, cx);
        self.sent_settings
            .insert(plugin_id.clone(), settings.clone());
        let Some(plugin) = self.plugin_mut(&plugin_id) else {
            return;
        };
        let spawned = resolved.and_then(|(program, args)| {
            let mut command = Command::new(&program);
            command
                .args(&args)
                .current_dir(&plugin.dir)
                .env("PATH", pi::path_with_executable_dir(&program))
                .env("PI_GPUI_PLUGIN_DIR", &plugin.dir);
            JsonlProcess::spawn(&mut command).map_err(|error| error.to_string())
        });
        match spawned {
            Ok((process, receiver)) => {
                eprintln!("pi-gpui: started plugin {plugin_id}");
                let initialize = json!({
                    "method": "initialize",
                    "params": {
                        "protocol": PROTOCOL_VERSION,
                        "app": env!("CARGO_PKG_VERSION"),
                        "plugin": { "id": plugin.id, "dir": plugin.dir },
                        "settings": settings,
                    },
                });
                let mut failure = None;
                for message in std::iter::once(initialize).chain(plugin.pending.drain(..)) {
                    if let Err(error) = process.send(message) {
                        failure = Some(error);
                        break;
                    }
                }
                plugin.state = PluginState::Running(process);
                let pump_id = plugin_id.clone();
                plugin._pump_task =
                    cx.spawn(async move |this, cx| pump_plugin(this, pump_id, receiver, cx).await);
                if let Some(error) = failure {
                    let name = plugin.name.clone();
                    self.notify(
                        format!("Plugin \"{name}\" does not accept messages: {error}"),
                        cx,
                    );
                }
            }
            Err(error) => {
                plugin.state = PluginState::Stopped;
                plugin.pending.clear();
                let name = plugin.name.clone();
                self.notify(format!("Could not start plugin \"{name}\": {error}"), cx);
            }
        }
        cx.notify();
    }

    fn apply_messages(
        &mut self,
        plugin_id: &SharedString,
        messages: Vec<JsonlMessage>,
        cx: &mut Context<Self>,
    ) {
        // Frames, logs and events change nothing the registry's observers
        // show; notifying for them would re-render every chat per frame.
        let mut changed = false;
        for message in messages {
            match message {
                JsonlMessage::Record(mut record) => {
                    let method = record["method"].as_str().unwrap_or_default().to_string();
                    match record.get_mut("id").map(Value::take) {
                        Some(id) if !id.is_null() => {
                            self.apply_request(plugin_id, id, &method, &record["params"], cx);
                        }
                        _ => {
                            changed |= matches!(
                                method.as_str(),
                                "command.setState"
                                    | "status.set"
                                    | "status.clear"
                                    | "prompt.input"
                                    | "prompt.close"
                            );
                            self.apply_record(plugin_id, &record, cx);
                        }
                    }
                }
                JsonlMessage::Malformed(line) => {
                    eprintln!("pi-gpui: plugin {plugin_id} wrote non-JSON: {line}");
                }
                JsonlMessage::Exited {
                    status,
                    stderr_tail,
                } => {
                    changed = true;
                    self.on_exited(plugin_id, status, stderr_tail, cx);
                }
            }
        }
        if changed {
            cx.notify();
        }
    }

    fn apply_record(&mut self, plugin_id: &SharedString, record: &Value, cx: &mut Context<Self>) {
        let params = &record["params"];
        match record["method"].as_str().unwrap_or_default() {
            "composer.insert" => cx.emit(PluginEvent::ComposerInsert {
                text: params["text"].as_str().unwrap_or_default().to_string(),
                replace_last: params["replaceLast"].as_bool().unwrap_or(false),
                space_before: params["spaceBefore"].as_bool().unwrap_or(false),
            }),
            "composer.setText" => cx.emit(PluginEvent::ComposerSetText(
                params["text"].as_str().unwrap_or_default().to_string(),
            )),
            "command.setState" => {
                let Some(command) = params["command"].as_str() else {
                    return;
                };
                // A plugin may only toggle its own commands.
                let owned = self
                    .commands
                    .iter()
                    .any(|candidate| candidate.id == command && candidate.plugin_id == *plugin_id);
                if !owned {
                    return;
                }
                if params["active"].as_bool().unwrap_or(false) {
                    self.active_commands
                        .insert(SharedString::from(command.to_string()));
                } else {
                    self.active_commands.remove(command);
                }
            }
            "status.set" => {
                let Some(id) = params["id"].as_str() else {
                    return;
                };
                let item = StatusItem {
                    plugin_id: plugin_id.clone(),
                    id: SharedString::from(id.to_string()),
                    text: sessions::one_line(params["text"].as_str().unwrap_or_default(), 60)
                        .into(),
                    color: tone_color(params["tone"].as_str()),
                    busy: params["busy"].as_bool().unwrap_or(false),
                };
                match self
                    .statuses
                    .iter_mut()
                    .find(|status| status.plugin_id == *plugin_id && status.id == id)
                {
                    Some(existing) => *existing = item,
                    None => self.statuses.push(item),
                }
            }
            "status.clear" => {
                let id = params["id"].as_str().unwrap_or_default();
                self.statuses
                    .retain(|status| !(status.plugin_id == *plugin_id && status.id == id));
            }
            "notify" => cx.emit(PluginEvent::Notify {
                message: params["message"].as_str().unwrap_or_default().to_string(),
                color: tone_color(params["tone"].as_str()),
            }),
            "view.frame" => self.apply_frame(plugin_id, params, cx),
            "prompt.input" => self.open_prompt(plugin_id, params),
            "prompt.close" => {
                let id = params["id"].as_str().unwrap_or_default();
                self.prompts
                    .retain(|prompt| !(prompt.plugin_id == *plugin_id && prompt.id == id));
            }
            "log" => eprintln!(
                "pi-gpui: plugin {plugin_id}: {}",
                params["message"].as_str().unwrap_or_default()
            ),
            // Unknown methods are ignored so the protocol can grow.
            _ => {}
        }
    }

    fn on_exited(
        &mut self,
        plugin_id: &SharedString,
        status: Option<std::process::ExitStatus>,
        stderr_tail: String,
        cx: &mut Context<Self>,
    ) {
        // Whatever the plugin showed belongs to the process that just ended.
        self.statuses.retain(|item| item.plugin_id != *plugin_id);
        self.prompts.retain(|prompt| prompt.plugin_id != *plugin_id);
        self.session_subscriptions
            .retain(|(owner, _)| owner != plugin_id);
        self.frames.update(cx, |frames, cx| {
            frames.live.remove(plugin_id);
            cx.notify();
        });
        let owned: Vec<SharedString> = self
            .commands
            .iter()
            .filter(|command| command.plugin_id == *plugin_id)
            .map(|command| command.id.clone())
            .collect();
        for command in owned {
            self.active_commands.remove(&command);
        }
        let Some(plugin) = self.plugin_mut(plugin_id) else {
            return;
        };
        plugin.state = PluginState::Stopped;
        let name = plugin.name.clone();
        eprintln!("pi-gpui: plugin {plugin_id} exited ({status:?})");
        if status.is_some_and(|status| !status.success()) {
            let details = stderr_tail.lines().last().unwrap_or_default().to_string();
            self.notify(
                format!("Plugin \"{name}\" stopped with an error. {details}"),
                cx,
            );
        }
    }

    fn notify(&mut self, message: String, cx: &mut Context<Self>) {
        eprintln!("pi-gpui: {message}");
        cx.emit(PluginEvent::Notify {
            message,
            color: Color::Error,
        });
    }
}

async fn pump_plugin(
    this: WeakEntity<PluginRegistry>,
    plugin_id: SharedString,
    mut receiver: mpsc::Receiver<JsonlMessage>,
    cx: &mut AsyncApp,
) {
    while let Some(first) = receiver.next().await {
        let mut batch = vec![first];
        while batch.len() < MAX_MESSAGES_PER_UPDATE {
            match receiver.try_next() {
                Ok(Some(message)) => batch.push(message),
                _ => break,
            }
        }
        if this
            .update(cx, |this, cx| this.apply_messages(&plugin_id, batch, cx))
            .is_err()
        {
            return;
        }
    }
}

/// Reads every manifest. Blocking; runs on a background thread.
fn discover_plugins() -> Vec<Result<(PathBuf, ParsedManifest), String>> {
    let mut found = Vec::new();
    for directory in plugin_directories() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        let mut plugin_dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.join("plugin.json").is_file())
            .collect();
        plugin_dirs.sort();
        for plugin_dir in plugin_dirs {
            found.push(
                parse_manifest(&plugin_dir)
                    .map(|parsed| (plugin_dir.clone(), parsed))
                    .map_err(|error| format!("{}: {error}", plugin_dir.display())),
            );
        }
    }
    found
}

/// User plugins first, then built-in ones: `Contents/Resources/plugins` in
/// the macOS app bundle, `share/pi-gpui/plugins` next to `bin/` in a Linux
/// install, or the repository's `plugins/` when run from a build.
fn plugin_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(data) = sessions::app_data_directory() {
        directories.push(data.join("plugins"));
    }
    let bundled = std::env::current_exe().ok().and_then(|executable| {
        let contents = executable.parent()?.parent()?;
        if cfg!(target_os = "macos") {
            let resources = contents.join("Resources/plugins");
            (contents.file_name()? == "Contents" && resources.is_dir()).then_some(resources)
        } else {
            let share = contents.join("share/pi-gpui/plugins");
            share.is_dir().then_some(share)
        }
    });
    directories.push(
        bundled.unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/plugins"))),
    );
    directories
}

struct ParsedManifest {
    id: SharedString,
    name: SharedString,
    run: Vec<String>,
    commands: Vec<PluginCommand>,
    tool_views: Vec<ToolView>,
    tool_events: Vec<SharedString>,
    pi_extensions: Vec<PathBuf>,
    startup: bool,
    sessions: bool,
    app: bool,
    settings: Vec<PluginSetting>,
}

fn parse_manifest(directory: &Path) -> Result<ParsedManifest, String> {
    let bytes = fs::read(directory.join("plugin.json")).map_err(|error| error.to_string())?;
    let manifest: Value =
        serde_json::from_slice(&bytes).map_err(|error| format!("plugin.json: {error}"))?;

    let id = manifest["id"].as_str().unwrap_or_default();
    if id.is_empty()
        || !id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
    {
        return Err("id: use ASCII letters, digits, - _ .".to_string());
    }
    let protocol = manifest["protocol"].as_u64();
    if protocol != Some(PROTOCOL_VERSION) {
        return Err(format!(
            "written for plugin protocol {}, but this app speaks protocol {PROTOCOL_VERSION}",
            protocol.map_or_else(|| "?".to_string(), |protocol| protocol.to_string())
        ));
    }
    let run: Vec<String> = manifest["run"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| part.as_str().map(str::to_string))
        .collect();
    if run.is_empty() {
        return Err("run: a program to start is required".to_string());
    }

    let id = SharedString::from(id.to_string());
    let mut commands = Vec::new();
    for command in manifest["contributes"]["commands"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let command_id = command["id"].as_str().unwrap_or_default();
        if !command_id.starts_with(&format!("{id}.")) {
            return Err(format!(
                "command \"{command_id}\" must start with \"{id}.\""
            ));
        }
        let keybinding = match command["keybinding"].as_str() {
            Some(keys) => {
                for keystroke in keys.split_whitespace() {
                    Keystroke::parse(keystroke)
                        .map_err(|_| format!("invalid keybinding \"{keys}\""))?;
                }
                Some(SharedString::from(keys.to_string()))
            }
            None => None,
        };
        commands.push(PluginCommand {
            id: SharedString::from(command_id.to_string()),
            plugin_id: id.clone(),
            title: SharedString::from(command["title"].as_str().unwrap_or(command_id).to_string()),
            icon: command["icon"]
                .as_str()
                .and_then(IconName::from_plugin_name)
                .unwrap_or(IconName::Tool),
            keybinding,
            in_composer: command["placement"]
                .as_array()
                .is_some_and(|places| places.iter().any(|place| place == "composer")),
        });
    }

    let mut tool_views = Vec::new();
    for view in manifest["contributes"]["toolViews"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let Some(tool) = view["tool"].as_str().filter(|tool| !tool.is_empty()) else {
            return Err("toolViews: each view needs a \"tool\"".to_string());
        };
        let width = view["width"].as_f64().unwrap_or(640.).max(1.);
        let height = view["height"].as_f64().unwrap_or(400.).max(1.);
        tool_views.push(ToolView {
            plugin_id: id.clone(),
            tool: SharedString::from(tool.to_string()),
            title: SharedString::from(view["title"].as_str().unwrap_or(tool).to_string()),
            aspect: (height / width) as f32,
        });
    }
    let tool_events = manifest["contributes"]["toolEvents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|tool| !tool.is_empty())
        .map(|tool| SharedString::from(tool.to_string()))
        .collect();
    let mut pi_extensions = Vec::new();
    for extension in manifest["contributes"]["piExtensions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let path = directory.join(extension);
        if !path.is_file() {
            return Err(format!("piExtensions: no such file {extension}"));
        }
        pi_extensions.push(path);
    }

    let startup = match manifest["activation"].as_str() {
        None | Some("lazy") => false,
        Some("startup") => true,
        Some(other) => return Err(format!("activation: unknown value \"{other}\"")),
    };
    let permitted = |name: &str| {
        manifest["permissions"]
            .as_array()
            .is_some_and(|permissions| permissions.iter().any(|permission| permission == name))
    };
    let sessions = permitted("sessions");
    let app = permitted("app");
    let settings = settings::parse(&manifest, &id)?;

    Ok(ParsedManifest {
        settings,
        name: SharedString::from(manifest["name"].as_str().unwrap_or(&id).to_string()),
        startup,
        sessions,
        app,
        id,
        run,
        commands,
        tool_views,
        tool_events,
        pi_extensions,
    })
}

/// `./program` is relative to the plugin folder; plain names are looked up
/// like `pi` (PATH, then the login shell, which finds nvm's `node`).
fn resolve_program(directory: &Path, run: &[String]) -> Result<(PathBuf, Vec<String>), String> {
    let (program, args) = run.split_first().ok_or("empty run command")?;
    let path = if program.starts_with("./") || program.starts_with("../") {
        directory.join(program)
    } else if Path::new(program).is_absolute() {
        PathBuf::from(program)
    } else {
        pi::find_executable(program).ok_or_else(|| format!("program \"{program}\" not found"))?
    };
    if !path.is_file() {
        return Err(format!("no such file: {}", path.display()));
    }
    Ok((path, args.to_vec()))
}

fn tone_color(tone: Option<&str>) -> Color {
    match tone {
        Some("error") => Color::Error,
        Some("warning") => Color::Warning,
        Some("success") => Color::Success,
        Some("info") => Color::Accent,
        _ => Color::Muted,
    }
}

/// `cmd-shift-d` → `⌘⇧D`, for tooltips.
pub fn display_keys(keys: &str) -> String {
    keys.split_whitespace()
        .map(|keystroke| {
            keystroke
                .split('-')
                .map(|part| match part {
                    "cmd" => "⌘".to_string(),
                    "shift" => "⇧".to_string(),
                    "alt" => "⌥".to_string(),
                    "ctrl" => "⌃".to_string(),
                    "enter" => "↩".to_string(),
                    "space" => "Space".to_string(),
                    key => key.to_uppercase(),
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_manifest(directory: &Path, manifest: Value) {
        fs::create_dir_all(directory).expect("plugin dir");
        fs::write(directory.join("plugin.json"), manifest.to_string()).expect("manifest");
    }

    #[test]
    fn manifest_validation() {
        let root = std::env::temp_dir().join(format!("pi-gpui-plugins-{}", std::process::id()));

        let good = root.join("good");
        write_manifest(
            &good,
            json!({
                "id": "demo", "name": "Demo", "protocol": PROTOCOL_VERSION, "run": ["./demo"],
                "contributes": { "commands": [
                    { "id": "demo.go", "title": "Go", "icon": "microphone",
                      "keybinding": "cmd-shift-d", "placement": ["composer"] }
                ]}
            }),
        );
        let parsed = parse_manifest(&good).expect("valid manifest");
        assert_eq!(parsed.commands.len(), 1);
        assert!(parsed.commands[0].in_composer);
        assert_eq!(parsed.commands[0].icon, IconName::Microphone);

        let wrong_protocol = root.join("wrong-protocol");
        write_manifest(
            &wrong_protocol,
            json!({ "id": "x", "protocol": 99, "run": ["x"] }),
        );
        assert!(parse_manifest(&wrong_protocol).is_err());

        let foreign_command = root.join("foreign-command");
        write_manifest(
            &foreign_command,
            json!({ "id": "x", "protocol": PROTOCOL_VERSION, "run": ["x"],
                    "contributes": { "commands": [{ "id": "other.go" }] } }),
        );
        assert!(parse_manifest(&foreign_command).is_err());
        assert!(!parsed.startup && !parsed.sessions && !parsed.app);

        let server = root.join("server");
        write_manifest(
            &server,
            json!({ "id": "srv", "protocol": PROTOCOL_VERSION, "run": ["x"],
                    "activation": "startup", "permissions": ["sessions", "app"] }),
        );
        let parsed = parse_manifest(&server).expect("server manifest");
        assert!(parsed.startup && parsed.sessions && parsed.app);

        let bad_activation = root.join("bad-activation");
        write_manifest(
            &bad_activation,
            json!({ "id": "x", "protocol": PROTOCOL_VERSION, "run": ["x"], "activation": "often" }),
        );
        assert!(parse_manifest(&bad_activation).is_err());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn keys_are_displayed_like_macos() {
        assert_eq!(display_keys("cmd-shift-d"), "⌘⇧D");
    }
}
