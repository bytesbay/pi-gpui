//! One pi session: transcript, composer and its own `pi --mode rpc` process.

mod dialog;
mod overlays;
mod plugin_view;
mod slash;
mod tree;
mod usage;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{
    AnyElement, App, AsyncApp, Context, Corner, ElementId, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, Global, KeyBinding, ListAlignment, ListState, MouseButton, ScrollHandle,
    SharedString, Subscription, Task, WeakEntity, Window, actions, anchored, deferred, div, list,
    px,
};
use serde_json::{Value, json};

use crate::attachments::{self, Picture};
use crate::jsonl::{JsonlMessage, JsonlProcess};
use crate::markdown::{self, Node};
use crate::pi::{self, Installation};
use crate::plugins::{self, CommandContext, PluginEvent, PluginRegistry};
use crate::selectable::{SelectableText, TextSelection};
use crate::sessions;
use crate::text_input::{self, TextInput};
use crate::ui::AnimationTicker;
use crate::ui::prelude::*;

actions!(
    chat,
    [
        Submit,
        CancelRun,
        ToggleModelPicker,
        DismissModelPicker,
        CopySelection,
        ToggleTree,
        TreeUp,
        TreeDown,
        TreeFirst,
        TreeLast,
        TreePreviousBranch,
        TreeNextBranch,
        TreeGo,
        TreeGoWithSummary,
        TreeFork,
        TreeClose,
        SlashUp,
        SlashDown,
        SlashComplete,
        SlashDismiss,
        DialogUp,
        DialogDown,
        DialogConfirm,
        DialogCancel,
        ReleasePluginView,
    ]
);

const KEY_CONTEXT: &str = "Chat";
const MODEL_PICKER_CONTEXT: &str = "ModelPicker";
const TREE_CONTEXT: &str = "SessionTree";
const SLASH_CONTEXT: &str = "SlashMenu";
const COMPOSER_PLACEHOLDER: &str = "Message pi…";
/// Ignore the chip click that arrives right after a click outside closed the picker.
const PICKER_REOPEN_GUARD: Duration = Duration::from_millis(250);
/// Upper bound of pi records applied per UI update, so a burst cannot hold
/// the foreground thread long enough to delay input.
const MAX_RECORDS_PER_UPDATE: usize = 256;
const LIST_OVERDRAW: f32 = 600.;
const CONTENT_MAX_WIDTH: f32 = 820.;
/// Horizontal row padding; the fork button sits in the left one.
const ROW_GUTTER: f32 = 28.;
const ROW_GROUP: &str = "chat-row";
const TOOL_OUTPUT_PREVIEW_LINES: usize = 12;
const TOOL_OUTPUT_PREVIEW_CHARS: usize = 2_000;

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", Submit, Some(text_input::KEY_CONTEXT)),
        KeyBinding::new("escape", CancelRun, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-shift-m", ToggleModelPicker, Some(KEY_CONTEXT)),
        // Text selected in the transcript; the composer has its own cmd-c.
        KeyBinding::new("cmd-c", CopySelection, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", DismissModelPicker, Some(MODEL_PICKER_CONTEXT)),
        KeyBinding::new("cmd-shift-t", ToggleTree, Some(KEY_CONTEXT)),
        KeyBinding::new("up", TreeUp, Some(TREE_CONTEXT)),
        KeyBinding::new("k", TreeUp, Some(TREE_CONTEXT)),
        KeyBinding::new("down", TreeDown, Some(TREE_CONTEXT)),
        KeyBinding::new("j", TreeDown, Some(TREE_CONTEXT)),
        KeyBinding::new("cmd-up", TreeFirst, Some(TREE_CONTEXT)),
        KeyBinding::new("home", TreeFirst, Some(TREE_CONTEXT)),
        KeyBinding::new("cmd-down", TreeLast, Some(TREE_CONTEXT)),
        KeyBinding::new("end", TreeLast, Some(TREE_CONTEXT)),
        KeyBinding::new("left", TreePreviousBranch, Some(TREE_CONTEXT)),
        KeyBinding::new("right", TreeNextBranch, Some(TREE_CONTEXT)),
        KeyBinding::new("enter", TreeGo, Some(TREE_CONTEXT)),
        KeyBinding::new("alt-enter", TreeGoWithSummary, Some(TREE_CONTEXT)),
        KeyBinding::new("cmd-enter", TreeFork, Some(TREE_CONTEXT)),
        KeyBinding::new("escape", TreeClose, Some(TREE_CONTEXT)),
        KeyBinding::new("up", SlashUp, Some(SLASH_CONTEXT)),
        KeyBinding::new("down", SlashDown, Some(SLASH_CONTEXT)),
        KeyBinding::new("tab", SlashComplete, Some(SLASH_CONTEXT)),
        KeyBinding::new("escape", SlashDismiss, Some(SLASH_CONTEXT)),
        KeyBinding::new("up", DialogUp, Some(dialog::DIALOG_CONTEXT)),
        KeyBinding::new("down", DialogDown, Some(dialog::DIALOG_CONTEXT)),
        KeyBinding::new("enter", DialogConfirm, Some(dialog::DIALOG_CONTEXT)),
        KeyBinding::new("escape", DialogCancel, Some(dialog::DIALOG_CONTEXT)),
        KeyBinding::new(
            "escape",
            ReleasePluginView,
            Some(plugin_view::PLUGIN_VIEW_CONTEXT),
        ),
    ]);
}

/// Whether step details (thought text, tool output) start expanded, for all
/// chats. Steps themselves are always shown. Toggled from the title bar and
/// remembered by the hub; toggling also resets per-item choices.
pub struct DetailsExpanded {
    pub expanded: bool,
}

impl Global for DetailsExpanded {}

fn details_expanded(cx: &App) -> bool {
    cx.try_global::<DetailsExpanded>()
        .is_some_and(|details| details.expanded)
}

#[derive(Clone, PartialEq)]
struct ModelInfo {
    provider: SharedString,
    id: SharedString,
    name: SharedString,
    context_window: Option<u64>,
}

impl ModelInfo {
    fn from_value(value: &Value) -> Option<Self> {
        let provider = value["provider"].as_str()?;
        let id = value["id"].as_str()?;
        Some(Self {
            provider: SharedString::from(provider.to_string()),
            id: SharedString::from(id.to_string()),
            name: SharedString::from(value["name"].as_str().unwrap_or(id).to_string()),
            context_window: value["contextWindow"].as_u64(),
        })
    }

    fn matches(&self, query: &str) -> bool {
        let haystack = format!("{} {} {}", self.name, self.id, self.provider).to_lowercase();
        query
            .split_whitespace()
            .all(|token| haystack.contains(&token.to_lowercase()))
    }
}

struct ModelPicker {
    filter: Entity<TextInput>,
    highlighted: usize,
    scroll: ScrollHandle,
    scroll_to_highlight: bool,
    _filter_subscription: Subscription,
}

enum Connection {
    Ready(JsonlProcess),
    Failed(String),
    Exited(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ChatStatus {
    Ready,
    Busy,
    Stopped,
}

/// Semantic changes the sidebar cares about. Streaming text does not emit.
pub enum ChatEvent {
    /// Title, session file or running state changed.
    Changed,
    /// pi finished all automatic work; the session file is up to date.
    Settled,
    /// `/new`: start a new session in the same project.
    NewSession,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SummaryKind {
    Compaction,
    Branch,
}

#[derive(Clone, PartialEq)]
enum SummaryState {
    Running,
    Done,
    Failed(String),
    Aborted,
}

/// The session tree overlay (`/tree`, ⌘⇧T).
struct TreeView {
    /// `None` until `get_entries` answers.
    lines: Option<Vec<tree::TreeLine>>,
    selected: usize,
    scroll: ScrollHandle,
    scroll_to_selected: bool,
    focus: FocusHandle,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolStatus {
    Running,
    Done,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tone {
    Info,
    Warning,
    Error,
}

/// Rows are append-only. Indices therefore stay stable, which keeps
/// `ListState::splice` bookkeeping trivial.
enum Row {
    User {
        text: SharedString,
        steering: bool,
        /// Pasted images (their paths are in the text) and inline images.
        images: Vec<Picture>,
    },
    Assistant {
        text: String,
        /// Thinking content blocks, in order.
        thinking: Vec<String>,
        streaming: bool,
        error: Option<String>,
        /// Thoughts the user opened or closed (index into `split_thoughts`),
        /// overriding the global default. Kept in the row so they survive
        /// scrolling the virtualized list.
        thought_overrides: BTreeMap<usize, bool>,
    },
    Tool {
        name: String,
        detail: String,
        status: ToolStatus,
        output: String,
        /// Images the tool returned (e.g. `read` on a picture).
        images: Vec<Picture>,
        /// The user's choice; `None` follows the global default.
        expanded: Option<bool>,
        /// The tool call id when a plugin draws a live view in this row.
        view: Option<SharedString>,
    },
    Notice {
        tone: Tone,
        text: String,
    },
    /// A compaction (manual or automatic) or a branch summary.
    Summary {
        kind: SummaryKind,
        state: SummaryState,
        summary: String,
        tokens_before: Option<u64>,
        tokens_after: Option<u64>,
        expanded: bool,
    },
    /// A question from an extension (see `dialog`).
    Dialog(dialog::DialogRow),
}

/// A fork in progress: pi forks only from a user message, so a fork from
/// any other item continues after its turn (or clones the whole branch when
/// it is in the last turn).
enum Forking {
    /// Waiting for `get_fork_messages` to find the entry id of a user message.
    Resolving {
        /// Position among user messages, counted from the last one.
        from_end: usize,
        text: String,
        /// Put the message back in the composer (a fork from that message).
        edit: bool,
    },
    /// `fork` or `clone` was sent.
    Switching { edit: bool },
}

pub struct ChatView {
    workspace: PathBuf,
    session_file: Option<PathBuf>,
    session_name: Option<String>,
    installation: Installation,
    connection: Connection,
    loading_history: bool,
    /// A prompt was sent and pi has not started (or rejected) it yet.
    pending_prompt: bool,
    /// Set when rows were replaced wholesale; the list must be reset, not spliced.
    needs_list_reset: bool,
    model: Option<ModelInfo>,
    thinking_level: Option<String>,
    thinking_levels: Vec<SharedString>,
    available_models: Option<Vec<ModelInfo>>,
    model_picker: Option<ModelPicker>,
    /// `set_model` was sent and not answered yet.
    switching_model: bool,
    forking: Option<Forking>,
    /// The session switched to a fork; the sidebar should reload its list.
    forked: bool,
    /// Built-in commands plus pi's (`get_commands`).
    slash_commands: Vec<slash::SlashCommand>,
    slash_highlight: usize,
    /// Composer text at which the user closed the menu with Esc.
    slash_dismissed: Option<SharedString>,
    slash_scroll: ScrollHandle,
    compacting: bool,
    tree: Option<TreeView>,
    /// A tree navigation prompt in flight: request id and the text to put
    /// back into the composer (when moving to a user message).
    navigating: Option<(String, Option<String>)>,
    usage_stats: Option<usage::Stats>,
    usage_extra: Option<usage::Extra>,
    /// Free tokens below which pi compacts, `None` when auto-compaction is off.
    compaction_reserve: Option<u64>,
    usage_open: bool,
    usage_closed_at: Option<Instant>,
    /// `/session` asked for the stats; show them as a notice too.
    stats_notice: bool,
    /// Re-renders the open popover every second for the cache countdown.
    _usage_tick: Task<()>,
    picker_closed_at: Option<Instant>,
    running: bool,
    rows: Vec<Row>,
    list_state: ListState,
    active_assistant: Option<usize>,
    tool_rows: HashMap<String, usize>,
    next_request_id: u64,
    composer: Entity<TextInput>,
    focus_handle: FocusHandle,
    _events_task: Task<()>,
    _composer_subscription: Subscription,
    plugins: Entity<PluginRegistry>,
    _plugins_subscription: Subscription,
    _details_subscription: Subscription,
    /// Fonts and the theme change row heights and colors: re-measure.
    _appearance_subscriptions: [Subscription; 2],
    /// Parsed answer per assistant row, keyed by a hash of its text.
    markdown_cache: HashMap<usize, (u64, Rc<[Node]>)>,
    selection: Entity<TextSelection>,
    _selection_subscription: Subscription,
    animation: AnimationTicker,
    /// The extension dialog pi is waiting on.
    dialog: Option<dialog::PendingDialog>,
    /// Move focus to the new dialog on the next render.
    focus_dialog: bool,
    /// Free text typed for a select; answers the input it leads to.
    queued_dialog_answer: Option<String>,
    /// Keyboard focus of the plugin view the user clicked into.
    view_focus: FocusHandle,
    /// The call whose view has the keyboard, and the modifiers it last saw.
    focused_view: Option<(SharedString, gpui::Modifiers)>,
    _frames_subscription: Subscription,
}

impl EventEmitter<ChatEvent> for ChatView {}

impl ChatView {
    /// Starts pi right away: a new session, or `session_file` resumed with its history.
    pub fn new(
        workspace: PathBuf,
        session_file: Option<PathBuf>,
        installation: Installation,
        plugins: Entity<PluginRegistry>,
        cx: &mut Context<Self>,
    ) -> Self {
        let composer = cx.new(|cx| TextInput::new(COMPOSER_PLACEHOLDER, cx).with_attachments());
        // The send button's state depends on the composer text.
        let composer_subscription = cx.observe(&composer, |this, composer, cx| {
            this.slash_highlight = 0;
            let menu_open = !this.slash_matches(cx).is_empty();
            composer.update(cx, |composer, _| {
                composer.set_vertical_passthrough(menu_open)
            });
            cx.notify();
        });
        // Plugin buttons in the composer reflect plugin state (e.g. recording).
        let plugins_subscription = cx.observe(&plugins, |_, _, cx| cx.notify());
        // Plugin views repaint with every frame.
        let frames = plugins.read(cx).frames().clone();
        let frames_subscription = cx.observe(&frames, |_, _, cx| cx.notify());
        // The global default wins again: forget per-item choices. Heights change.
        let details_subscription = cx.observe_global::<DetailsExpanded>(|this, cx| {
            for row in &mut this.rows {
                match row {
                    Row::Assistant {
                        thought_overrides, ..
                    } => thought_overrides.clear(),
                    Row::Tool { expanded, .. } => *expanded = None,
                    _ => {}
                }
            }
            this.list_state.reset(this.rows.len());
            cx.notify();
        });
        let appearance_subscriptions = [
            cx.observe_global::<crate::settings::Settings>(|this, cx| {
                this.list_state.reset(this.rows.len());
                cx.notify();
            }),
            cx.observe_global::<crate::ui::Theme>(|this, cx| {
                this.list_state.reset(this.rows.len());
                cx.notify();
            }),
        ];
        let focus_handle = cx.focus_handle();
        let selection = cx.new(|_| TextSelection::new(focus_handle.clone()));
        let selection_subscription = cx.observe(&selection, |_, _, cx| cx.notify());
        let mut this = Self {
            workspace,
            session_file,
            session_name: None,
            installation,
            connection: Connection::Failed(String::new()),
            loading_history: false,
            forking: None,
            forked: false,
            slash_commands: slash::builtin(),
            slash_highlight: 0,
            slash_dismissed: None,
            slash_scroll: ScrollHandle::new(),
            compacting: false,
            tree: None,
            navigating: None,
            usage_stats: None,
            usage_extra: None,
            compaction_reserve: None,
            usage_open: false,
            usage_closed_at: None,
            stats_notice: false,
            _usage_tick: Task::ready(()),
            pending_prompt: false,
            needs_list_reset: false,
            model: None,
            thinking_level: None,
            thinking_levels: Vec::new(),
            available_models: None,
            model_picker: None,
            switching_model: false,
            picker_closed_at: None,
            running: false,
            rows: Vec::new(),
            list_state: ListState::new(0, ListAlignment::Bottom, px(LIST_OVERDRAW)),
            active_assistant: None,
            tool_rows: HashMap::new(),
            next_request_id: 0,
            composer,
            focus_handle,
            _events_task: Task::ready(()),
            _composer_subscription: composer_subscription,
            plugins,
            _plugins_subscription: plugins_subscription,
            _details_subscription: details_subscription,
            _appearance_subscriptions: appearance_subscriptions,
            markdown_cache: HashMap::new(),
            selection,
            _selection_subscription: selection_subscription,
            animation: AnimationTicker::default(),
            dialog: None,
            focus_dialog: false,
            queued_dialog_answer: None,
            view_focus: cx.focus_handle(),
            focused_view: None,
            _frames_subscription: frames_subscription,
        };
        this.start_process(cx);
        this
    }

    pub fn composer_focus_handle(&self, cx: &App) -> FocusHandle {
        self.composer.read(cx).focus_handle(cx)
    }

    /// Text a plugin sends to the composer of the active chat.
    pub fn apply_plugin_edit(&mut self, event: &PluginEvent, cx: &mut Context<Self>) {
        match event {
            PluginEvent::ComposerInsert {
                text,
                replace_last,
                space_before,
            } => self.composer.update(cx, |composer, cx| {
                composer.insert_text(text, *replace_last, *space_before, cx)
            }),
            PluginEvent::ComposerSetText(text) => {
                let text = text.clone();
                self.composer
                    .update(cx, |composer, cx| composer.set_text(text, cx));
            }
            PluginEvent::Notify { .. } => {}
        }
    }

    fn run_plugin_command(&mut self, command: &str, cx: &mut Context<Self>) {
        let context = CommandContext {
            workspace: Some(self.workspace.clone()),
            session: self.session_file.clone(),
        };
        self.plugins.update(cx, |registry, cx| {
            registry.run_command(command, context, cx)
        });
    }

    /// Renames the session through pi (`set_session_name`).
    pub fn rename(&mut self, name: &str) {
        self.send_all([json!({ "type": "set_session_name", "name": name })]);
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn session_file(&self) -> Option<&Path> {
        self.session_file.as_deref()
    }

    /// True from the moment a prompt is sent, so the sidebar reacts immediately.
    pub fn is_running(&self) -> bool {
        self.running || self.pending_prompt
    }

    pub fn title(&self) -> String {
        if let Some(name) = &self.session_name {
            return name.clone();
        }
        self.rows
            .iter()
            .find_map(|row| match row {
                Row::User { text, .. } => {
                    Some(sessions::one_line(&attachments::display_text(text), 80))
                }
                _ => None,
            })
            .unwrap_or_else(|| "New session".to_string())
    }

    fn start_process(&mut self, cx: &mut Context<Self>) {
        // Replacing the task drops the previous pump before a new process starts.
        self._events_task = Task::ready(());
        self.running = false;
        let extensions = self.plugins.read(cx).pi_extensions().to_vec();
        let spawned = pi::spawn_pi(
            &self.installation,
            &self.workspace,
            self.session_file.as_deref(),
            &extensions,
        );
        match spawned {
            Ok((process, receiver)) => {
                eprintln!(
                    "pi-gpui: started pi in {} ({})",
                    self.workspace.display(),
                    self.session_file.as_deref().map_or_else(
                        || "new session".to_string(),
                        |file| file.display().to_string()
                    )
                );
                self.connection = Connection::Ready(process);
                self._events_task =
                    cx.spawn(async move |this, cx| pump_events(this, receiver, cx).await);
                let mut commands = vec![
                    json!({ "type": "get_state" }),
                    json!({ "type": "get_commands" }),
                ];
                if self.rows.is_empty() && self.session_file.is_some() {
                    self.loading_history = true;
                    commands.push(json!({ "type": "get_messages" }));
                }
                for command in commands {
                    if let Err(error) = self.send(command) {
                        self.push_notice(Tone::Error, error);
                    }
                }
            }
            Err(error) => {
                let message = format!(
                    "Could not start {}: {error}",
                    self.installation.executable.display()
                );
                eprintln!("pi-gpui: {message}");
                self.connection = Connection::Failed(message);
            }
        }
        cx.emit(ChatEvent::Changed);
        cx.notify();
    }

    /// Sends a command with a fresh request id and returns the id.
    fn send(&mut self, mut command: Value) -> Result<String, String> {
        let Connection::Ready(process) = &self.connection else {
            return Err("pi is not connected".to_string());
        };
        self.next_request_id += 1;
        let id = format!("ui-{}", self.next_request_id);
        if let Some(object) = command.as_object_mut() {
            object.insert("id".into(), json!(id));
        }
        process.send(command).map(|()| id)
    }

    fn submit(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog.is_some() {
            let text = self.composer.read(cx).submit_text();
            if self.submit_to_dialog(&text, cx) {
                return;
            }
        }
        if let Some(&index) = self.slash_matches(cx).get(self.slash_highlight) {
            self.accept_slash(index, true, window, cx);
            return;
        }
        // Chips for pasted files and images become their paths here.
        let text = self.composer.read(cx).submit_text();
        let message = text.trim();
        if message.is_empty() || !matches!(self.connection, Connection::Ready(_)) {
            return;
        }
        if self.compacting || self.navigating.is_some() {
            return;
        }
        if let Some((name, arguments)) = slash::parse(message) {
            let (name, arguments) = (name.to_string(), arguments.to_string());
            if self.run_builtin(&name, &arguments, window, cx) {
                self.composer.update(cx, |composer, cx| composer.clear(cx));
                cx.notify();
                return;
            }
        }

        let steering = self.running;
        let mut command = json!({ "type": "prompt", "message": message });
        if steering {
            command["streamingBehavior"] = json!("steer");
        }

        let previous_len = self.rows.len();
        match self.send(command) {
            Ok(_) => {
                self.rows.push(Row::User {
                    text: SharedString::from(message.to_string()),
                    steering,
                    images: attachments::image_paths_in(message)
                        .into_iter()
                        .map(Picture::File)
                        .collect(),
                });
                self.pending_prompt = true;
                self.composer.update(cx, |composer, cx| composer.clear(cx));
                // Sending is an explicit "show me the latest": re-enter follow mode.
                self.list_state.reset(self.rows.len());
                cx.emit(ChatEvent::Changed);
            }
            Err(error) => {
                self.push_notice(Tone::Error, error);
                self.sync_list(previous_len, Vec::new());
            }
        }
        cx.notify();
    }

    fn cancel_run(&mut self, _: &CancelRun, _window: &mut Window, cx: &mut Context<Self>) {
        // Esc first dismisses the question pi waits on, then stops the run.
        if self.dialog.is_some() {
            self.cancel_dialog(cx);
            return;
        }
        self.request_abort(cx);
    }

    fn request_abort(&mut self, cx: &mut Context<Self>) {
        if !self.running && !self.compacting {
            return;
        }
        if let Err(error) = self.send(json!({ "type": "abort" })) {
            let previous_len = self.rows.len();
            self.push_notice(Tone::Error, error);
            self.sync_list(previous_len, Vec::new());
            cx.notify();
        }
    }

    fn apply_batch(&mut self, batch: Vec<JsonlMessage>, cx: &mut Context<Self>) {
        let previous_len = self.rows.len();
        let summary_before = (self.is_running(), self.title(), self.session_file.clone());
        let mut changed = Vec::new();
        let mut settled = false;
        for message in batch {
            match message {
                JsonlMessage::Record(record) => {
                    settled |= record["type"] == "agent_settled";
                    self.apply_record(&record, &mut changed, cx);
                }
                JsonlMessage::Malformed(line) => self.push_notice(
                    Tone::Warning,
                    format!("Unrecognized line from pi: {}", truncate_chars(&line, 300)),
                ),
                JsonlMessage::Exited {
                    status,
                    stderr_tail,
                } => self.on_process_exited(status, stderr_tail, &mut changed, cx),
            }
        }
        if std::mem::take(&mut self.needs_list_reset) {
            self.list_state.reset(self.rows.len());
        } else {
            self.sync_list(previous_len, changed);
        }
        if summary_before != (self.is_running(), self.title(), self.session_file.clone()) {
            cx.emit(ChatEvent::Changed);
        }
        if settled || std::mem::take(&mut self.forked) {
            cx.emit(ChatEvent::Settled);
        }
        cx.notify();
    }

    /// Tells the list which rows need re-measuring: changed existing rows are
    /// replaced one-for-one, new rows are appended. Never resets the list, so
    /// the reader's scroll position survives streaming.
    fn sync_list(&mut self, previous_len: usize, mut changed: Vec<usize>) {
        changed.sort_unstable();
        changed.dedup();
        for index in changed.into_iter().filter(|index| *index < previous_len) {
            self.list_state.splice(index..index + 1, 1);
        }
        if self.rows.len() > previous_len {
            self.list_state
                .splice(previous_len..previous_len, self.rows.len() - previous_len);
        }
    }

    fn apply_record(&mut self, record: &Value, changed: &mut Vec<usize>, cx: &mut Context<Self>) {
        let record_type = record["type"].as_str().unwrap_or_default();
        match record_type {
            "response" => self.apply_response(record, cx),
            "agent_start" => {
                self.running = true;
                self.pending_prompt = false;
            }
            "agent_settled" => {
                self.running = false;
                self.pending_prompt = false;
                self.close_dialog(dialog::DialogState::Closed, changed, cx);
                self.queued_dialog_answer = None;
                self.finish_streaming(changed);
                self.request_stats();
                if record["aborted"].as_bool() == Some(true) {
                    self.push_notice(Tone::Warning, "Run stopped.".into());
                }
            }
            "message_start" if record["message"]["role"] == "assistant" => {
                self.finish_streaming(changed);
                self.rows.push(Row::Assistant {
                    text: String::new(),
                    thinking: Vec::new(),
                    streaming: true,
                    error: None,
                    thought_overrides: BTreeMap::new(),
                });
                self.active_assistant = Some(self.rows.len() - 1);
            }
            "message_update" => {
                self.apply_message_update(&record["assistantMessageEvent"], changed)
            }
            "message_end" if record["message"]["role"] == "assistant" => {
                self.apply_assistant_end(&record["message"], changed);
            }
            "tool_execution_start" => {
                let name = record["toolName"].as_str().unwrap_or("tool").to_string();
                let detail = tool_detail(&record["args"]);
                self.rows.push(Row::Tool {
                    name,
                    detail,
                    status: ToolStatus::Running,
                    output: String::new(),
                    images: Vec::new(),
                    expanded: None,
                    view: None,
                });
                if let Some(call_id) = record["toolCallId"].as_str() {
                    self.tool_rows
                        .insert(call_id.to_string(), self.rows.len() - 1);
                    self.open_plugin_view(record, call_id, cx);
                }
            }
            "tool_execution_update" | "tool_execution_end" => {
                let finished = record_type == "tool_execution_end";
                let result = if finished {
                    &record["result"]
                } else {
                    &record["partialResult"]
                };
                let index = record["toolCallId"]
                    .as_str()
                    .and_then(|call_id| self.tool_rows.get(call_id).copied());
                if let Some(index) = index
                    && let Some(Row::Tool {
                        status,
                        output,
                        images,
                        ..
                    }) = self.rows.get_mut(index)
                {
                    *output = preview_lines(&content_text(result));
                    // Partial results may carry images too (a live preview, frames).
                    let pictures = Picture::from_content(&result["content"]);
                    if finished || !pictures.is_empty() {
                        *images = pictures;
                    }
                    if finished {
                        *status = if record["isError"].as_bool() == Some(true) {
                            ToolStatus::Failed
                        } else {
                            ToolStatus::Done
                        };
                    }
                    changed.push(index);
                }
            }
            "auto_retry_start" => self.push_notice(
                Tone::Warning,
                format!(
                    "Retry {}/{}: {}",
                    record["attempt"],
                    record["maxAttempts"],
                    record["errorMessage"].as_str().unwrap_or("provider error")
                ),
            ),
            "auto_retry_end" if record["success"].as_bool() == Some(false) => self.push_notice(
                Tone::Error,
                record["finalError"]
                    .as_str()
                    .unwrap_or("Retries did not help")
                    .to_string(),
            ),
            "compaction_start" => {
                self.compacting = true;
                if self.running_summary().is_none() {
                    self.rows.push(Row::Summary {
                        kind: SummaryKind::Compaction,
                        state: SummaryState::Running,
                        summary: String::new(),
                        tokens_before: None,
                        tokens_after: None,
                        expanded: false,
                    });
                }
            }
            "compaction_end" => {
                self.compacting = false;
                self.request_stats();
                let state = if record["result"].is_object() {
                    SummaryState::Done
                } else if record["aborted"].as_bool() == Some(true) {
                    SummaryState::Aborted
                } else {
                    SummaryState::Failed(
                        record["errorMessage"]
                            .as_str()
                            .unwrap_or("Compaction failed")
                            .to_string(),
                    )
                };
                if let Some(index) = self.finish_summary(state, &record["result"]) {
                    changed.push(index);
                }
            }
            "extension_error" => self.push_notice(
                Tone::Error,
                format!(
                    "Extension error in {}: {}",
                    record["extensionPath"].as_str().unwrap_or("?"),
                    record["error"].as_str().unwrap_or("?")
                ),
            ),
            "extension_ui_request" => self.apply_extension_request(record, changed, cx),
            _ => {}
        }
    }

    fn apply_response(&mut self, record: &Value, cx: &mut Context<Self>) {
        let command = record["command"].as_str().unwrap_or("?");
        // Only "started" leads to agent_start; anything else ends the wait here.
        if command == "prompt" && record["data"]["disposition"] != "started" {
            self.pending_prompt = false;
        }
        if command == "set_model" {
            self.switching_model = false;
        }
        let is_navigation = self
            .navigating
            .as_ref()
            .is_some_and(|(id, _)| record["id"].as_str() == Some(id.as_str()));
        if record["success"].as_bool() == Some(false) {
            if matches!(command, "get_fork_messages" | "fork" | "clone") {
                self.forking = None;
            }
            if is_navigation {
                self.navigating = None;
            }
            if command == "compact" {
                self.compacting = false;
                let error = record["error"]
                    .as_str()
                    .unwrap_or("unknown error")
                    .to_string();
                self.finish_summary(SummaryState::Failed(error), &Value::Null);
                return;
            }
            let error = record["error"].as_str().unwrap_or("unknown error");
            self.push_notice(Tone::Error, format!("{command}: {error}"));
            return;
        }
        let data = &record["data"];
        match command {
            "get_state" => {
                self.model = ModelInfo::from_value(&data["model"]);
                self.thinking_level = data["thinkingLevel"].as_str().map(str::to_string);
                self.running = data["isStreaming"].as_bool().unwrap_or(false);
                self.session_name = data["sessionName"].as_str().map(str::to_string);
                if let Some(file) = data["sessionFile"].as_str() {
                    self.session_file = Some(PathBuf::from(file));
                }
            }
            "get_messages" if self.loading_history => {
                self.loading_history = false;
                let messages = data["messages"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                // Notices that arrived during startup stay after the history.
                let mut rows = history_rows(messages);
                rows.append(&mut self.rows);
                self.rows = rows;
                self.needs_list_reset = true;
                self.request_stats();
            }
            "get_available_models" => {
                let mut models: Vec<ModelInfo> = data["models"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(ModelInfo::from_value)
                    .collect();
                // Group by provider, keeping pi's order inside each group.
                models.sort_by(|left, right| left.provider.cmp(&right.provider));
                self.available_models = Some(models);
            }
            "set_model" => {
                if let Some(model) = ModelInfo::from_value(data) {
                    eprintln!("pi-gpui: switched model to {}/{}", model.provider, model.id);
                    self.model = Some(model);
                }
                // The new model may support other thinking levels.
                self.send_all([
                    json!({ "type": "get_available_thinking_levels" }),
                    json!({ "type": "get_state" }),
                    json!({ "type": "get_session_stats" }),
                ]);
            }
            "get_available_thinking_levels" => {
                self.thinking_levels = data["levels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|level| level.as_str())
                    .map(|level| SharedString::from(level.to_string()))
                    .collect();
            }
            "set_thinking_level" => self.send_all([json!({ "type": "get_state" })]),
            "get_fork_messages" => self.continue_fork(data),
            "fork" | "clone" => self.finish_fork(data, cx),
            "prompt" if is_navigation => {
                if let Some((_, text)) = self.navigating.take() {
                    if let Some(text) = text {
                        self.composer
                            .update(cx, |composer, cx| composer.set_text(text, cx));
                    }
                    self.reload_transcript(cx);
                }
            }
            "get_commands" => {
                self.slash_commands = slash::builtin();
                self.slash_commands.extend(slash::from_pi(data));
            }
            "get_entries" => {
                if let Some(view) = &mut self.tree {
                    let (lines, current) = tree::flatten(data);
                    view.selected = current.unwrap_or(lines.len().saturating_sub(1));
                    view.lines = Some(lines);
                    view.scroll_to_selected = true;
                }
            }
            "compact" => {
                self.compacting = false;
                self.finish_summary(SummaryState::Done, data);
            }
            "set_session_name" => self.send_all([json!({ "type": "get_state" })]),
            "get_session_stats" => {
                self.usage_stats = Some(usage::Stats::from_session_stats(data));
                if let Some(model) = &self.model {
                    self.compaction_reserve =
                        usage::compaction_reserve(&self.workspace, &model.provider, &model.id);
                }
                if !std::mem::take(&mut self.stats_notice) {
                    return;
                }
                let context = match (
                    data["contextUsage"]["tokens"].as_u64(),
                    data["contextUsage"]["contextWindow"].as_u64(),
                ) {
                    (Some(tokens), Some(window)) => format!(
                        "Context: {} of {} ({}%).",
                        format_tokens(tokens),
                        format_tokens(window),
                        data["contextUsage"]["percent"]
                            .as_f64()
                            .unwrap_or_default()
                            .round()
                    ),
                    _ => "Context: unknown until the next answer.".to_string(),
                };
                self.push_notice(
                    Tone::Info,
                    format!(
                        "Session: {}\nMessages: {} from you, {} answers, {} tool calls.\nTokens: {} in, {} out, {} cache read.\nCost: ${:.2}.\n{context}",
                        data["sessionFile"].as_str().unwrap_or("not saved yet"),
                        data["userMessages"],
                        data["assistantMessages"],
                        data["toolCalls"],
                        format_tokens(data["tokens"]["input"].as_u64().unwrap_or_default()),
                        format_tokens(data["tokens"]["output"].as_u64().unwrap_or_default()),
                        format_tokens(data["tokens"]["cacheRead"].as_u64().unwrap_or_default()),
                        data["cost"].as_f64().unwrap_or_default(),
                    ),
                );
            }
            "get_last_assistant_text" => match data["text"].as_str() {
                Some(text) if !text.is_empty() => {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.to_string()));
                    self.push_notice(Tone::Info, "Copied the last answer.".to_string());
                }
                _ => self.push_notice(Tone::Info, "There is no answer to copy yet.".to_string()),
            },
            "export_html" => {
                if let Some(path) = data["path"].as_str() {
                    self.push_notice(Tone::Info, format!("Exported to {path}"));
                    cx.open_url(&format!("file://{path}"));
                }
            }
            _ => {}
        }
    }

    fn send_all(&mut self, commands: impl IntoIterator<Item = Value>) {
        for command in commands {
            if let Err(error) = self.send(command) {
                self.push_notice(Tone::Error, error);
                return;
            }
        }
    }

    /// Refreshes the numbers behind the context ring (silently).
    fn request_stats(&mut self) {
        if matches!(self.connection, Connection::Ready(_)) {
            let _ = self.send(json!({ "type": "get_session_stats" }));
        }
    }

    fn toggle_usage(&mut self, cx: &mut Context<Self>) {
        if self.usage_open {
            self.close_usage(cx);
            return;
        }
        // The click that closed it via "mouse down outside" must not reopen it.
        if self
            .usage_closed_at
            .is_some_and(|closed| closed.elapsed() < PICKER_REOPEN_GUARD)
        {
            return;
        }
        self.usage_open = true;
        self.request_stats();
        self._usage_tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
        });
        cx.notify();
    }

    fn close_usage(&mut self, cx: &mut Context<Self>) {
        if self.usage_open {
            self.usage_open = false;
            self.usage_closed_at = Some(Instant::now());
            self._usage_tick = Task::ready(());
            cx.notify();
        }
    }

    /// Matching commands while a `/name` is being typed in the composer.
    fn slash_matches(&self, cx: &App) -> Vec<usize> {
        if self.tree.is_some() || self.model_picker.is_some() {
            return Vec::new();
        }
        let text = self.composer.read(cx).text();
        if self.slash_dismissed.as_ref() == Some(&text) {
            return Vec::new();
        }
        slash::typed_query(&text)
            .map(|query| slash::matching(&self.slash_commands, query))
            .unwrap_or_default()
    }

    fn move_slash_highlight(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.slash_matches(cx).len();
        if count == 0 {
            cx.propagate();
            return;
        }
        self.slash_highlight =
            (self.slash_highlight as isize + delta).rem_euclid(count as isize) as usize;
        self.slash_scroll.scroll_to_item(self.slash_highlight);
        cx.notify();
    }

    fn slash_up(&mut self, _: &SlashUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_slash_highlight(-1, cx);
    }

    fn slash_down(&mut self, _: &SlashDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_slash_highlight(1, cx);
    }

    fn slash_complete(&mut self, _: &SlashComplete, window: &mut Window, cx: &mut Context<Self>) {
        match self.slash_matches(cx).get(self.slash_highlight) {
            Some(&index) => self.accept_slash(index, false, window, cx),
            None => cx.propagate(),
        }
    }

    fn slash_dismiss(&mut self, _: &SlashDismiss, _: &mut Window, cx: &mut Context<Self>) {
        if self.slash_matches(cx).is_empty() {
            cx.propagate();
            return;
        }
        self.slash_dismissed = Some(self.composer.read(cx).text());
        self.composer
            .update(cx, |composer, _| composer.set_vertical_passthrough(false));
        cx.notify();
    }

    /// Enter on a menu entry runs app commands whose arguments are optional;
    /// Tab, and every other command, completes the name so arguments can follow.
    fn accept_slash(
        &mut self,
        index: usize,
        run: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(command) = self.slash_commands.get(index).cloned() else {
            return;
        };
        let runs_now = run
            && command.source == slash::Source::App
            && command
                .arguments
                .is_none_or(|arguments| arguments.starts_with('['));
        if runs_now {
            self.composer.update(cx, |composer, cx| composer.clear(cx));
            self.run_builtin(&command.name, "", window, cx);
        } else {
            let text = format!("/{} ", command.name);
            self.composer
                .update(cx, |composer, cx| composer.set_text(text, cx));
        }
        cx.notify();
    }

    /// Runs one of the app's commands. `false` when `name` is not one of
    /// them: the text then goes to pi, which knows its own commands.
    fn run_builtin(
        &mut self,
        name: &str,
        arguments: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let previous_len = self.rows.len();
        match name {
            "compact" => self.start_compaction(arguments),
            "tree" | "fork" => self.open_tree(window, cx),
            "clone" => {
                if self.can_fork() {
                    self.forking = Some(Forking::Switching { edit: false });
                    if let Err(error) = self.send(json!({ "type": "clone" })) {
                        self.forking = None;
                        self.push_notice(Tone::Error, error);
                    }
                } else {
                    self.push_notice(Tone::Info, "Wait until pi is idle, then clone.".to_string());
                }
            }
            "model" => self.toggle_model_picker(&ToggleModelPicker, window, cx),
            "new" => cx.emit(ChatEvent::NewSession),
            "name" if arguments.is_empty() => {
                self.push_notice(Tone::Info, "Usage: /name <new session name>".to_string())
            }
            "name" => self.send_all([json!({ "type": "set_session_name", "name": arguments })]),
            "session" => {
                self.stats_notice = true;
                self.send_all([json!({ "type": "get_session_stats" })]);
            }
            "copy" => self.send_all([json!({ "type": "get_last_assistant_text" })]),
            "export" => self.send_all([json!({ "type": "export_html" })]),
            _ => return false,
        }
        self.sync_list(previous_len, Vec::new());
        true
    }

    fn start_compaction(&mut self, instructions: &str) {
        if self.compacting {
            return;
        }
        if self.is_running() || self.loading_history {
            self.push_notice(
                Tone::Info,
                "Wait until pi finishes, then compact.".to_string(),
            );
            return;
        }
        let mut command = json!({ "type": "compact" });
        if !instructions.is_empty() {
            command["customInstructions"] = json!(instructions);
        }
        match self.send(command) {
            Ok(_) => {
                self.compacting = true;
                self.rows.push(Row::Summary {
                    kind: SummaryKind::Compaction,
                    state: SummaryState::Running,
                    summary: String::new(),
                    tokens_before: None,
                    tokens_after: None,
                    expanded: false,
                });
            }
            Err(error) => self.push_notice(Tone::Error, error),
        }
    }

    fn running_summary(&self) -> Option<usize> {
        self.rows.iter().rposition(|row| {
            matches!(
                row,
                Row::Summary {
                    state: SummaryState::Running,
                    ..
                }
            )
        })
    }

    /// Completes the running compaction row from a `compaction_end` result
    /// or the `compact` response, whichever comes first.
    fn finish_summary(&mut self, new_state: SummaryState, result: &Value) -> Option<usize> {
        let index = self.running_summary()?;
        if let Some(Row::Summary {
            state,
            summary,
            tokens_before,
            tokens_after,
            ..
        }) = self.rows.get_mut(index)
        {
            *state = new_state;
            *summary = result["summary"].as_str().unwrap_or_default().to_string();
            *tokens_before = result["tokensBefore"].as_u64();
            *tokens_after = result["estimatedTokensAfter"].as_u64();
        }
        self.list_state.splice(index..index + 1, 1);
        Some(index)
    }

    fn toggle_summary(&mut self, row: usize, cx: &mut Context<Self>) {
        if let Some(Row::Summary { expanded, .. }) = self.rows.get_mut(row) {
            *expanded = !*expanded;
            self.list_state.splice(row..row + 1, 1);
            cx.notify();
        }
    }

    /// Reloads the transcript after pi moved to another point or session.
    fn reload_transcript(&mut self, cx: &mut Context<Self>) {
        self.rows.clear();
        self.tool_rows.clear();
        self.active_assistant = None;
        self.markdown_cache.clear();
        self.selection
            .update(cx, |selection, cx| selection.clear(cx));
        self.loading_history = true;
        self.needs_list_reset = true;
        self.forked = true;
        self.send_all([
            json!({ "type": "get_state" }),
            json!({ "type": "get_messages" }),
        ]);
    }

    /// Opens or closes the session tree (also from the title bar).
    pub fn toggle_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tree.is_some() {
            self.close_tree(window, cx);
        } else {
            self.open_tree(window, cx);
        }
    }

    fn toggle_tree_action(&mut self, _: &ToggleTree, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_tree(window, cx);
    }

    fn open_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.connection, Connection::Ready(_)) || self.tree.is_some() {
            return;
        }
        let focus = cx.focus_handle();
        window.focus(&focus);
        self.tree = Some(TreeView {
            lines: None,
            selected: 0,
            scroll: ScrollHandle::new(),
            scroll_to_selected: false,
            focus,
        });
        self.send_all([json!({ "type": "get_entries" })]);
        cx.notify();
    }

    fn close_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tree.take().is_some() {
            window.focus(&self.composer.read(cx).focus_handle(cx));
            cx.notify();
        }
    }

    fn tree_close(&mut self, _: &TreeClose, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tree(window, cx);
    }

    fn select_tree_line(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(view) = &mut self.tree
            && let Some(lines) = &view.lines
            && index < lines.len()
        {
            view.selected = index;
            view.scroll_to_selected = true;
            cx.notify();
        }
    }

    fn tree_target(
        &self,
        step: impl Fn(&[tree::TreeLine], usize) -> Option<usize>,
    ) -> Option<usize> {
        let view = self.tree.as_ref()?;
        step(view.lines.as_deref()?, view.selected)
    }

    fn tree_up(&mut self, _: &TreeUp, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.tree_target(|_, selected| selected.checked_sub(1)) {
            self.select_tree_line(index, cx);
        }
    }

    fn tree_down(&mut self, _: &TreeDown, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.tree_target(|_, selected| Some(selected + 1)) {
            self.select_tree_line(index, cx);
        }
    }

    fn tree_first(&mut self, _: &TreeFirst, _: &mut Window, cx: &mut Context<Self>) {
        self.select_tree_line(0, cx);
    }

    fn tree_last(&mut self, _: &TreeLast, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.tree_target(|lines, _| lines.len().checked_sub(1)) {
            self.select_tree_line(index, cx);
        }
    }

    fn tree_previous_branch(
        &mut self,
        _: &TreePreviousBranch,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) =
            self.tree_target(|lines, selected| Some(tree::branch_start(lines, selected)))
        {
            self.select_tree_line(index, cx);
        }
    }

    fn tree_next_branch(&mut self, _: &TreeNextBranch, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.tree_target(tree::next_branch) {
            self.select_tree_line(index, cx);
        }
    }

    fn selected_tree_line(&self) -> Option<tree::TreeLine> {
        let view = self.tree.as_ref()?;
        view.lines.as_ref()?.get(view.selected).cloned()
    }

    fn tree_go(&mut self, _: &TreeGo, window: &mut Window, cx: &mut Context<Self>) {
        self.navigate_to_selected(false, window, cx);
    }

    fn tree_go_with_summary(
        &mut self,
        _: &TreeGoWithSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_to_selected(true, window, cx);
    }

    /// Moves the session to the selected entry, as pi's `/tree` does: on a
    /// user message the session continues from before it and the message
    /// returns to the composer; on anything else it continues after it.
    fn navigate_to_selected(
        &mut self,
        summarize: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(line) = self.selected_tree_line() else {
            return;
        };
        if line.current && line.kind != tree::NodeKind::User {
            self.close_tree(window, cx);
            return;
        }
        let previous_len = self.rows.len();
        if !self.can_fork() {
            self.push_notice(
                Tone::Info,
                "Wait until pi is idle, then move in the tree.".to_string(),
            );
        } else if pi::bridge_extension().is_none() {
            self.push_notice(
                Tone::Error,
                "Moving in the tree needs the app's pi extension, which could not be written to the app data folder.".to_string(),
            );
        } else {
            let mut message = format!("/{} {}", pi::NAVIGATE_COMMAND, line.id);
            if summarize {
                message.push_str(" summarize");
            }
            match self.send(json!({ "type": "prompt", "message": message })) {
                Ok(id) => {
                    let text = (line.kind == tree::NodeKind::User).then(|| line.text.clone());
                    self.navigating = Some((id, text));
                }
                Err(error) => self.push_notice(Tone::Error, error),
            }
        }
        self.sync_list(previous_len, Vec::new());
        self.close_tree(window, cx);
    }

    /// Forks a new session from the selected user message.
    fn tree_fork(&mut self, _: &TreeFork, window: &mut Window, cx: &mut Context<Self>) {
        let Some(line) = self.selected_tree_line() else {
            return;
        };
        if line.kind != tree::NodeKind::User || !self.can_fork() {
            return;
        }
        self.forking = Some(Forking::Switching { edit: true });
        if let Err(error) = self.send(json!({ "type": "fork", "entryId": line.id })) {
            self.forking = None;
            let previous_len = self.rows.len();
            self.push_notice(Tone::Error, error);
            self.sync_list(previous_len, Vec::new());
        }
        self.close_tree(window, cx);
    }

    /// Forks are possible only while pi is idle and the transcript is final.
    fn can_fork(&self) -> bool {
        matches!(self.connection, Connection::Ready(_))
            && !self.is_running()
            && !self.loading_history
            && self.forking.is_none()
            && !self.compacting
            && self.navigating.is_none()
    }

    /// Starts a fork at `row`: from a user message, the new session ends just
    /// before it and the message returns to the composer, like pi's `/fork`.
    /// From any other item, the new session keeps everything up to the end of
    /// that item's turn.
    fn fork_from_row(&mut self, row: usize, cx: &mut Context<Self>) {
        if !self.can_fork() {
            return;
        }
        let user_rows: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| matches!(row, Row::User { .. }))
            .map(|(index, _)| index)
            .collect();
        let (target, edit) = if matches!(self.rows.get(row), Some(Row::User { .. })) {
            (Some(row), true)
        } else {
            (user_rows.iter().copied().find(|index| *index > row), false)
        };
        let command = match target {
            Some(target) => {
                let Some(Row::User { text, .. }) = self.rows.get(target) else {
                    return;
                };
                let position = user_rows.iter().position(|index| *index == target);
                self.forking = Some(Forking::Resolving {
                    from_end: user_rows.len() - 1 - position.unwrap_or_default(),
                    text: text.to_string(),
                    edit,
                });
                json!({ "type": "get_fork_messages" })
            }
            // The last turn: the fork is a copy of the whole branch.
            None => {
                self.forking = Some(Forking::Switching { edit: false });
                json!({ "type": "clone" })
            }
        };
        if let Err(error) = self.send(command) {
            self.forking = None;
            let previous_len = self.rows.len();
            self.push_notice(Tone::Error, error);
            self.sync_list(previous_len, Vec::new());
        }
        cx.notify();
    }

    /// Maps the chosen user row to pi's entry id and sends `fork`. Rows are
    /// matched from the end (history before a compaction is not in the
    /// transcript) and checked by text.
    fn continue_fork(&mut self, data: &Value) {
        let Some(Forking::Resolving {
            from_end,
            text,
            edit,
        }) = self.forking.take()
        else {
            return;
        };
        let messages: Vec<(&str, &str)> = data["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|message| {
                Some((
                    message["entryId"].as_str()?,
                    message["text"].as_str().unwrap_or_default(),
                ))
            })
            .collect();
        let same = |candidate: &str| candidate.trim() == text.trim();
        let expected = messages
            .len()
            .checked_sub(from_end + 1)
            .and_then(|index| messages.get(index))
            .filter(|(_, candidate)| same(candidate));
        let entry = expected
            .or_else(|| messages.iter().rev().find(|(_, candidate)| same(candidate)))
            .map(|(entry, _)| entry.to_string());
        let Some(entry) = entry else {
            self.push_notice(
                Tone::Error,
                "Could not fork: pi does not list this message as a fork point.".to_string(),
            );
            return;
        };
        self.forking = Some(Forking::Switching { edit });
        if let Err(error) = self.send(json!({ "type": "fork", "entryId": entry })) {
            self.forking = None;
            self.push_notice(Tone::Error, error);
        }
    }

    /// pi now runs the new session: reload its transcript and state.
    fn finish_fork(&mut self, data: &Value, cx: &mut Context<Self>) {
        let Some(Forking::Switching { edit }) = self.forking.take() else {
            return;
        };
        if data["cancelled"].as_bool() == Some(true) {
            self.push_notice(
                Tone::Warning,
                "An extension cancelled the fork.".to_string(),
            );
            return;
        }
        let text = data["text"].as_str().unwrap_or_default().to_string();
        self.composer.update(cx, |composer, cx| {
            if edit && !text.is_empty() {
                composer.set_text(text, cx);
            } else {
                composer.clear(cx);
            }
        });
        self.reload_transcript(cx);
    }

    fn toggle_thought(&mut self, row: usize, thought: usize, cx: &mut Context<Self>) {
        let default = details_expanded(cx);
        if let Some(Row::Assistant {
            thought_overrides, ..
        }) = self.rows.get_mut(row)
        {
            let open = thought_overrides.get(&thought).copied().unwrap_or(default);
            thought_overrides.insert(thought, !open);
            self.list_state.splice(row..row + 1, 1);
            cx.notify();
        }
    }

    fn toggle_tool(&mut self, row: usize, cx: &mut Context<Self>) {
        let default = details_expanded(cx);
        if let Some(Row::Tool { expanded, .. }) = self.rows.get_mut(row) {
            *expanded = Some(!expanded.unwrap_or(default));
            self.list_state.splice(row..row + 1, 1);
            cx.notify();
        }
    }

    fn copy_selection(&mut self, _: &CopySelection, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.selection.read(cx).selected_text() {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        }
    }

    fn toggle_model_picker(
        &mut self,
        _: &ToggleModelPicker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.model_picker.is_some() {
            self.close_model_picker(window, cx);
            return;
        }
        if self
            .picker_closed_at
            .is_some_and(|closed| closed.elapsed() < PICKER_REOPEN_GUARD)
            || !matches!(self.connection, Connection::Ready(_))
        {
            return;
        }
        let filter = cx.new(|cx| TextInput::single_line("Search models…", cx));
        let filter_subscription = cx.observe(&filter, |this, _, cx| {
            if let Some(picker) = &mut this.model_picker {
                picker.highlighted = 0;
                picker.scroll_to_highlight = true;
            }
            cx.notify();
        });
        window.focus(&filter.read(cx).focus_handle(cx));
        self.model_picker = Some(ModelPicker {
            filter,
            highlighted: self.current_model_index(cx).unwrap_or(0),
            scroll: ScrollHandle::new(),
            scroll_to_highlight: true,
            _filter_subscription: filter_subscription,
        });
        let mut commands = vec![json!({ "type": "get_available_thinking_levels" })];
        if self.available_models.is_none() {
            commands.push(json!({ "type": "get_available_models" }));
        }
        self.send_all(commands);
        cx.notify();
    }

    fn dismiss_model_picker(
        &mut self,
        _: &DismissModelPicker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_model_picker(window, cx);
    }

    fn close_model_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model_picker.take().is_some() {
            self.picker_closed_at = Some(Instant::now());
            window.focus(&self.composer_focus_handle(cx));
            cx.notify();
        }
    }

    fn filtered_models(&self, cx: &App) -> Vec<ModelInfo> {
        let query = self
            .model_picker
            .as_ref()
            .map(|picker| picker.filter.read(cx).text())
            .unwrap_or_default();
        self.available_models
            .iter()
            .flatten()
            .filter(|model| model.matches(&query))
            .cloned()
            .collect()
    }

    fn current_model_index(&self, cx: &App) -> Option<usize> {
        let current = self.model.as_ref()?;
        self.filtered_models(cx)
            .iter()
            .position(|model| model.provider == current.provider && model.id == current.id)
    }

    fn move_model_highlight(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.filtered_models(cx).len();
        if let Some(picker) = &mut self.model_picker
            && count > 0
        {
            let next = (picker.highlighted as isize + delta).rem_euclid(count as isize);
            picker.highlighted = next as usize;
            picker.scroll_to_highlight = true;
            cx.notify();
        }
    }

    fn confirm_model(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = &self.model_picker else {
            return;
        };
        let highlighted = picker.highlighted;
        if let Some(model) = self.filtered_models(cx).get(highlighted).cloned() {
            self.select_model(model, window, cx);
        }
    }

    fn select_model(&mut self, model: ModelInfo, window: &mut Window, cx: &mut Context<Self>) {
        self.close_model_picker(window, cx);
        if self.model.as_ref() == Some(&model) {
            return;
        }
        self.switching_model = true;
        self.send_all([json!({
            "type": "set_model",
            "provider": model.provider,
            "modelId": model.id,
        })]);
        cx.notify();
    }

    fn select_thinking_level(&mut self, level: SharedString, cx: &mut Context<Self>) {
        self.thinking_level = Some(level.to_string());
        self.send_all([json!({ "type": "set_thinking_level", "level": level })]);
        cx.notify();
    }

    fn apply_message_update(&mut self, event: &Value, changed: &mut Vec<usize>) {
        let Some(index) = self.active_assistant else {
            return;
        };
        let Some(Row::Assistant { text, thinking, .. }) = self.rows.get_mut(index) else {
            return;
        };
        let delta = event["delta"].as_str().unwrap_or_default();
        match event["type"].as_str() {
            Some("text_delta") => text.push_str(delta),
            Some("thinking_start") => thinking.push(String::new()),
            Some("thinking_delta") => match thinking.last_mut() {
                Some(block) => block.push_str(delta),
                None => thinking.push(delta.to_string()),
            },
            _ => return,
        }
        changed.push(index);
    }

    /// `message_end` carries the authoritative message; it replaces whatever
    /// was reconstructed from deltas.
    fn apply_assistant_end(&mut self, message: &Value, changed: &mut Vec<usize>) {
        let Some(index) = self.active_assistant.take() else {
            return;
        };
        let Some(Row::Assistant {
            text,
            thinking,
            streaming,
            error,
            ..
        }) = self.rows.get_mut(index)
        else {
            return;
        };
        let (final_text, final_thinking) = assistant_blocks(message);
        if !final_text.is_empty() || message["content"].is_array() {
            *text = final_text;
        }
        if !final_thinking.is_empty() {
            *thinking = final_thinking;
        }
        *streaming = false;
        *error = stop_error(message);
        changed.push(index);
    }

    fn finish_streaming(&mut self, changed: &mut Vec<usize>) {
        if let Some(index) = self.active_assistant.take()
            && let Some(Row::Assistant { streaming, .. }) = self.rows.get_mut(index)
        {
            *streaming = false;
            changed.push(index);
        }
    }

    fn apply_extension_request(
        &mut self,
        record: &Value,
        changed: &mut Vec<usize>,
        cx: &mut Context<Self>,
    ) {
        if self.open_dialog(record, changed, cx) {
            return;
        }
        let method = record["method"].as_str().unwrap_or_default();
        match method {
            "notify" => {
                let tone = match record["notifyType"].as_str() {
                    Some("error") => Tone::Error,
                    Some("warning") => Tone::Warning,
                    _ => Tone::Info,
                };
                let text = record["message"].as_str().unwrap_or_default().to_string();
                self.push_notice(tone, text);
            }
            "set_editor_text" => {
                if let Some(text) = record["text"].as_str() {
                    let text = text.to_string();
                    self.composer
                        .update(cx, |composer, cx| composer.set_text(text, cx));
                }
            }
            "setStatus" if record["statusKey"] == usage::STATUS_KEY => {
                if let Some(extra) = record["statusText"].as_str().and_then(usage::Extra::parse) {
                    self.usage_extra = Some(extra);
                }
            }
            // Other statuses, setWidget, setTitle: no UI for them yet.
            _ => {}
        }
    }

    fn on_process_exited(
        &mut self,
        status: Option<std::process::ExitStatus>,
        stderr_tail: String,
        changed: &mut Vec<usize>,
        cx: &mut Context<Self>,
    ) {
        self.running = false;
        self.pending_prompt = false;
        self.finish_streaming(changed);
        self.close_dialog(dialog::DialogState::Closed, changed, cx);
        self.queued_dialog_answer = None;
        let status =
            status.map_or_else(|| "unknown status".to_string(), |status| status.to_string());
        let mut text = format!("pi exited ({status}).");
        let stderr_tail = stderr_tail.trim();
        if !stderr_tail.is_empty() {
            text.push('\n');
            text.push_str(stderr_tail);
        }
        self.connection = Connection::Exited(text.clone());
        self.push_notice(Tone::Error, text);
    }

    fn push_notice(&mut self, tone: Tone, text: String) {
        if tone != Tone::Info {
            eprintln!("pi-gpui: {text}");
        }
        if !text.trim().is_empty() {
            self.rows.push(Row::Notice { tone, text });
        }
    }

    /// What pi is doing right now, shown above the composer.
    fn activity(&self) -> Option<String> {
        if self.forking.is_some() {
            return Some("Forking…".to_string());
        }
        if self.navigating.is_some() {
            return Some("Moving in the session tree…".to_string());
        }
        if self.compacting {
            return Some("Compacting context…".to_string());
        }
        if self.loading_history {
            return Some("Loading history…".to_string());
        }
        if !self.running {
            return self.pending_prompt.then(|| "Sending…".to_string());
        }
        if self.dialog.is_some() {
            return Some("Waiting for your answer…".to_string());
        }
        let running_tool = self.rows.iter().rev().take(64).find_map(|row| match row {
            Row::Tool {
                name,
                status: ToolStatus::Running,
                ..
            } => Some(name.clone()),
            _ => None,
        });
        if let Some(name) = running_tool {
            return Some(format!("Running {name}…"));
        }
        let streaming = self.active_assistant.and_then(|index| self.rows.get(index));
        Some(match streaming {
            Some(Row::Assistant { text, .. }) if !text.is_empty() => "Writing…".to_string(),
            Some(Row::Assistant { thinking, .. })
                if thinking.iter().any(|block| !block.is_empty()) =>
            {
                "Thinking…".to_string()
            }
            _ => "Working…".to_string(),
        })
    }

    pub fn model_label(&self) -> String {
        match (&self.model, &self.thinking_level) {
            (Some(model), Some(level)) => format!("{} · {level}", model.name),
            (Some(model), None) => model.name.to_string(),
            _ => "Model".to_string(),
        }
    }

    /// Coarse state for the window title bar.
    pub fn status(&self) -> ChatStatus {
        match &self.connection {
            Connection::Failed(_) | Connection::Exited(_) => ChatStatus::Stopped,
            Connection::Ready(_) if self.running || self.pending_prompt || self.loading_history => {
                ChatStatus::Busy
            }
            Connection::Ready(_) => ChatStatus::Ready,
        }
    }

    fn render_connection_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let message = match &self.connection {
            Connection::Failed(message) | Connection::Exited(message) => message.clone(),
            Connection::Ready(_) => return None,
        };
        let colors = cx.theme().colors.clone();
        Some(
            h_flex()
                .items_start()
                .gap(px(8.))
                .mx(px(16.))
                .mt(px(12.))
                .px(px(12.))
                .py(px(10.))
                .rounded(px(6.))
                .border_1()
                .border_color(colors.error.opacity(0.4))
                .bg(colors.error_background)
                .child(
                    div()
                        .pt(px(2.))
                        .child(Icon::new(IconName::Warning).color(Color::Error)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .gap(px(8.))
                        .child(Label::new(message).size(LabelSize::Small))
                        .child(
                            h_flex().child(
                                Button::new("restart-pi", "Restart pi")
                                    .icon(IconName::Refresh)
                                    .style(ButtonStyle::Filled)
                                    .on_click(cx.listener(|this, _, _, cx| this.start_process(cx))),
                            ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_row(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(row) = self.rows.get(index) else {
            return div().into_any_element();
        };
        let colors = cx.theme().colors.clone();
        let answer = match row {
            Row::Assistant { text, .. } if !text.is_empty() => {
                let hash = text_hash(text);
                let entry = self
                    .markdown_cache
                    .entry(index)
                    .or_insert_with(|| (hash, markdown::parse(text).into()));
                if entry.0 != hash {
                    *entry = (hash, markdown::parse(text).into());
                }
                Some(entry.1.clone())
            }
            _ => None,
        };

        let default_open = details_expanded(cx);
        let key = |part: &str| SharedString::from(format!("{index}:{part}"));

        let content = match row {
            Row::User {
                text,
                steering,
                images,
            } => v_flex()
                .gap(px(4.))
                .when(*steering, |this| {
                    this.child(
                        h_flex()
                            .gap(px(4.))
                            .child(
                                Icon::new(IconName::ArrowUp)
                                    .size(IconSize::XSmall)
                                    .color(Color::Accent),
                            )
                            .child(
                                Label::new("Steering message")
                                    .size(LabelSize::XSmall)
                                    .color(Color::Accent),
                            ),
                    )
                })
                .child(
                    div()
                        .px(px(12.))
                        .py(px(8.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.panel_background)
                        .child(SelectableText::new(
                            key("user"),
                            attachments::display_text(text),
                            &self.selection,
                        )),
                )
                .when(!images.is_empty(), |this| {
                    this.child(thumbnails(index, images, &colors))
                })
                .into_any_element(),
            Row::Assistant {
                text,
                thinking,
                streaming,
                error,
                thought_overrides,
            } => {
                let thoughts = split_thoughts(thinking);
                if text.is_empty() && thoughts.is_empty() && error.is_none() {
                    // Nothing to show yet; the activity line says what pi is doing.
                    return div().into_any_element();
                }
                let thinking_live = *streaming && text.is_empty();
                v_flex()
                    .gap(px(8.))
                    .when(!thoughts.is_empty(), |this| {
                        this.child(self.render_thoughts(
                            index,
                            &thoughts,
                            thought_overrides,
                            default_open,
                            thinking_live,
                            cx,
                        ))
                    })
                    .when_some(answer, |this, blocks| {
                        this.child(markdown::render(
                            &format!("{index}:md"),
                            &blocks,
                            &self.selection,
                            window,
                            cx,
                        ))
                    })
                    .when_some(error.clone(), |this, error| {
                        this.child(
                            h_flex()
                                .items_start()
                                .gap(px(6.))
                                .px(px(10.))
                                .py(px(6.))
                                .rounded(px(6.))
                                .bg(colors.error_background)
                                .child(
                                    div().pt(px(2.)).child(
                                        Icon::new(IconName::Warning)
                                            .size(IconSize::XSmall)
                                            .color(Color::Error),
                                    ),
                                )
                                .child(
                                    SelectableText::new(key("error"), error, &self.selection)
                                        .size(LabelSize::Small),
                                ),
                        )
                    })
                    .into_any_element()
            }
            Row::Tool {
                name,
                detail,
                status,
                output,
                images,
                expanded,
                view,
            } => {
                let plugin_view = view
                    .as_ref()
                    .map(|call_id| self.render_plugin_view(call_id, name, cx));
                let has_details = !output.is_empty() || !images.is_empty();
                let open = expanded.unwrap_or(default_open) && has_details;
                let status_icon = match status {
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
                };
                v_flex()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(colors.border_variant)
                    .overflow_hidden()
                    .child(
                        h_flex()
                            .gap(px(6.))
                            .h(px(28.))
                            .pl(px(8.))
                            .pr(px(4.))
                            .bg(colors.panel_background)
                            .child(status_icon)
                            .child(
                                Icon::new(tool_icon(name))
                                    .size(IconSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .child(
                                Label::new(name.clone())
                                    .size(LabelSize::Small)
                                    .weight(FontWeight::MEDIUM),
                            )
                            .child(
                                div().flex_1().min_w(px(0.)).child(
                                    Label::new(attachments::display_text(detail))
                                        .size(LabelSize::Small)
                                        .color(Color::Muted)
                                        .monospace()
                                        .truncate(),
                                ),
                            )
                            .when(has_details, |this| {
                                this.child(disclosure_button(
                                    ("tool-disclosure", index),
                                    open,
                                    "output",
                                    cx.listener(move |this, _, _, cx| this.toggle_tool(index, cx)),
                                ))
                            }),
                    )
                    .when(open, |this| {
                        this.child(
                            div()
                                .px(px(8.))
                                .py(px(6.))
                                .border_t_1()
                                .border_color(colors.border_variant)
                                .child(
                                    SelectableText::new(
                                        key("output"),
                                        SharedString::from(output.clone()),
                                        &self.selection,
                                    )
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted)
                                    .monospace(),
                                )
                                .when(!images.is_empty(), |this| {
                                    this.child(
                                        div().pt(px(6.)).child(thumbnails(index, images, &colors)),
                                    )
                                }),
                        )
                    })
                    .children(plugin_view)
                    .into_any_element()
            }
            Row::Summary {
                kind,
                state,
                summary,
                tokens_before,
                tokens_after,
                expanded,
            } => {
                let headline = match (kind, state) {
                    (SummaryKind::Branch, _) => "Summary of the branch you left".to_string(),
                    (_, SummaryState::Running) => "Compacting context…".to_string(),
                    (_, SummaryState::Done) => "Context compacted".to_string(),
                    (_, SummaryState::Aborted) => "Compaction stopped".to_string(),
                    (_, SummaryState::Failed(error)) => {
                        let reason = error.strip_prefix("Compaction failed: ").unwrap_or(error);
                        format!("Compaction failed: {reason}")
                    }
                };
                let detail = match (tokens_before, tokens_after) {
                    (Some(before), Some(after)) => {
                        format!(
                            "{} → {} tokens",
                            format_tokens(*before),
                            format_tokens(*after)
                        )
                    }
                    (Some(before), None) => format!("from {} tokens", format_tokens(*before)),
                    _ => String::new(),
                };
                let icon = match state {
                    SummaryState::Running => Spinner::new()
                        .size(IconSize::XSmall)
                        .color(Color::Accent)
                        .into_any_element(),
                    SummaryState::Failed(_) => Icon::new(IconName::Warning)
                        .size(IconSize::XSmall)
                        .color(Color::Error)
                        .into_any_element(),
                    _ => Icon::new(IconName::Archive)
                        .size(IconSize::XSmall)
                        .color(Color::Muted)
                        .into_any_element(),
                };
                let has_summary = !summary.is_empty();
                let open = *expanded && has_summary;
                let body = open.then(|| {
                    let nodes = markdown::parse(summary);
                    div()
                        .px(px(14.))
                        .py(px(10.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border_variant)
                        .bg(colors.elevated_surface_background)
                        .child(markdown::render(
                            &format!("{index}:summary"),
                            &nodes,
                            &self.selection,
                            window,
                            cx,
                        ))
                });
                let rule = || div().flex_1().h(px(1.)).bg(colors.border_variant);
                v_flex()
                    .gap(px(8.))
                    .child(
                        h_flex()
                            .gap(px(10.))
                            .child(rule())
                            .child(
                                h_flex()
                                    .gap(px(6.))
                                    .flex_none()
                                    .child(icon)
                                    .child(Label::new(headline).size(LabelSize::Small).color(
                                        match state {
                                            SummaryState::Failed(_) => Color::Error,
                                            _ => Color::Muted,
                                        },
                                    ))
                                    .when(!detail.is_empty(), |this| {
                                        this.child(
                                            Label::new(detail)
                                                .size(LabelSize::XSmall)
                                                .color(Color::Placeholder),
                                        )
                                    })
                                    .when(has_summary, |this| {
                                        this.child(disclosure_button(
                                            ("summary-disclosure", index),
                                            open,
                                            "summary",
                                            cx.listener(move |this, _, _, cx| {
                                                this.toggle_summary(index, cx)
                                            }),
                                        ))
                                    }),
                            )
                            .child(rule()),
                    )
                    .children(body)
                    .into_any_element()
            }
            Row::Dialog(dialog) => self.render_dialog(index, dialog, cx),
            Row::Notice { tone, text } => {
                let (icon, color) = match tone {
                    Tone::Info => (IconName::Info, Color::Muted),
                    Tone::Warning => (IconName::Warning, Color::Warning),
                    Tone::Error => (IconName::Warning, Color::Error),
                };
                h_flex()
                    .items_start()
                    .gap(px(6.))
                    .child(
                        div()
                            .pt(px(2.))
                            .child(Icon::new(icon).size(IconSize::XSmall).color(color)),
                    )
                    .child(
                        SelectableText::new(
                            key("notice"),
                            SharedString::from(text.clone()),
                            &self.selection,
                        )
                        .size(LabelSize::Small)
                        .color(color),
                    )
                    .into_any_element()
            }
        };

        let forkable = !matches!(
            row,
            Row::Notice { .. } | Row::Summary { .. } | Row::Dialog(_)
        ) && self.can_fork();
        let fork_tooltip = if matches!(row, Row::User { .. }) {
            "Fork from this message"
        } else {
            "Fork after this turn"
        };
        div()
            .id(("row", index))
            .group(ROW_GROUP)
            .w_full()
            .flex()
            .justify_center()
            .px(px(ROW_GUTTER))
            .py(px(6.))
            .child(
                div()
                    .relative()
                    .w_full()
                    .max_w(px(CONTENT_MAX_WIDTH))
                    .child(content)
                    .when(forkable, |this| {
                        // In the row's left gutter, which is part of the row's
                        // hover area, so the button stays while the pointer moves to it.
                        this.child(
                            div()
                                .absolute()
                                .top(px(0.))
                                .left(px(-(ROW_GUTTER - 2.)))
                                .opacity(0.)
                                .group_hover(ROW_GROUP, |style| style.opacity(1.))
                                .child(
                                    IconButton::new(("fork", index), IconName::Fork)
                                        .size(ButtonSize::Compact)
                                        .icon_size(IconSize::XSmall)
                                        .tooltip(Tooltip::text(fork_tooltip))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.fork_from_row(index, cx)
                                        })),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }

    /// One line per thought with a disclosure button on the right; an open
    /// thought shows its full text once (the preview is not repeated).
    fn render_thoughts(
        &self,
        row: usize,
        thoughts: &[Thought],
        overrides: &BTreeMap<usize, bool>,
        default_open: bool,
        thinking_live: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let last = thoughts.len() - 1;
        let items = thoughts.iter().enumerate().map(|(position, thought)| {
            let expandable = thought.is_expandable();
            let open = expandable && overrides.get(&position).copied().unwrap_or(default_open);
            let live = thinking_live && position == last;

            let content = if open {
                v_flex()
                    .gap(px(2.))
                    .when_some(thought.title.clone(), |this, title| {
                        this.child(
                            Label::new(title)
                                .size(LabelSize::Small)
                                .color(Color::Muted)
                                .weight(FontWeight::MEDIUM),
                        )
                    })
                    .child(
                        SelectableText::new(
                            format!("{row}:thought:{position}"),
                            SharedString::from(thought.body.clone()),
                            &self.selection,
                        )
                        .size(LabelSize::Small)
                        .color(Color::Placeholder)
                        .italic(),
                    )
                    .into_any_element()
            } else if live {
                LoadingLabel::new(thought.preview())
                    .size(LabelSize::Small)
                    .into_any_element()
            } else {
                Label::new(thought.preview())
                    .size(LabelSize::Small)
                    .color(Color::Muted)
                    .italic()
                    .truncate()
                    .into_any_element()
            };

            h_flex()
                .items_start()
                .gap(px(4.))
                .child(div().flex_1().min_w(px(0.)).py(px(2.)).child(content))
                .when(expandable, |this| {
                    this.child(disclosure_button(
                        ("thought-disclosure", position),
                        open,
                        "thought",
                        cx.listener(move |this, _, _, cx| this.toggle_thought(row, position, cx)),
                    ))
                })
        });

        v_flex()
            .gap(px(2.))
            .pl(px(10.))
            .border_l_1()
            .border_color(colors.border_variant)
            .children(items)
            .into_any_element()
    }

    fn render_model_picker(&self, picker: &ModelPicker, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let models = self.filtered_models(cx);
        let highlighted = picker.highlighted.min(models.len().saturating_sub(1));

        let mut rows: Vec<AnyElement> = Vec::new();
        let mut highlighted_child = 0;
        let mut provider: Option<SharedString> = None;
        for (index, model) in models.iter().enumerate() {
            if provider.as_ref() != Some(&model.provider) {
                provider = Some(model.provider.clone());
                rows.push(
                    div()
                        .px(px(8.))
                        .pt(px(if index == 0 { 2. } else { 8. }))
                        .pb(px(2.))
                        .child(
                            Label::new(model.provider.to_uppercase())
                                .size(LabelSize::XSmall)
                                .color(Color::Placeholder),
                        )
                        .into_any_element(),
                );
            }
            if index == highlighted {
                highlighted_child = rows.len();
            }
            let is_current = self.model.as_ref().is_some_and(|current| {
                current.provider == model.provider && current.id == model.id
            });
            let selected = model.clone();
            rows.push(
                ListItem::new(ElementId::Name(
                    format!("model:{}/{}", model.provider, model.id).into(),
                ))
                .toggle_state(index == highlighted)
                .start_slot(
                    div()
                        .w(px(14.))
                        .flex()
                        .justify_center()
                        .when(is_current, |this| {
                            this.child(
                                Icon::new(IconName::Check)
                                    .size(IconSize::XSmall)
                                    .color(Color::Accent),
                            )
                        }),
                )
                .child(
                    Label::new(model.name.clone())
                        .size(LabelSize::Small)
                        .truncate(),
                )
                .when_some(model.context_window.map(format_tokens), |this, context| {
                    this.end_slot(
                        Label::new(context)
                            .size(LabelSize::XSmall)
                            .color(Color::Placeholder),
                    )
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select_model(selected.clone(), window, cx)
                }))
                .into_any_element(),
            );
        }
        if picker.scroll_to_highlight {
            picker.scroll.scroll_to_item(highlighted_child);
        }

        let body = if self.available_models.is_none() {
            h_flex()
                .gap(px(6.))
                .px(px(10.))
                .py(px(10.))
                .child(Spinner::new().size(IconSize::XSmall))
                .child(LoadingLabel::new("Loading models…").size(LabelSize::Small))
                .into_any_element()
        } else if rows.is_empty() {
            div()
                .px(px(10.))
                .py(px(10.))
                .child(
                    Label::new("No models match")
                        .size(LabelSize::Small)
                        .color(Color::Placeholder),
                )
                .into_any_element()
        } else {
            div()
                .id("model-list")
                .max_h(px(300.))
                .overflow_y_scroll()
                .track_scroll(&picker.scroll)
                .p(px(4.))
                .children(rows)
                .into_any_element()
        };

        let current_level = self.thinking_level.clone().unwrap_or_default();
        v_flex()
            .id("model-picker")
            .key_context(MODEL_PICKER_CONTEXT)
            .w(px(380.))
            .mb(px(6.))
            .rounded(px(8.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.elevated_surface_background)
            .shadow_lg()
            .occlude()
            .on_action(cx.listener(Self::confirm_model))
            .on_action(cx.listener(Self::dismiss_model_picker))
            .on_action(
                cx.listener(|this, _: &text_input::Up, _, cx| this.move_model_highlight(-1, cx)),
            )
            .on_action(
                cx.listener(|this, _: &text_input::Down, _, cx| this.move_model_highlight(1, cx)),
            )
            .on_mouse_down_out(
                cx.listener(|this, _, window, cx| this.close_model_picker(window, cx)),
            )
            .child(
                div()
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(picker.filter.clone()),
            )
            .child(body)
            .when(self.thinking_levels.len() > 1, |this| {
                this.child(
                    h_flex()
                        .flex_wrap()
                        .gap(px(4.))
                        .px(px(8.))
                        .py(px(6.))
                        .border_t_1()
                        .border_color(colors.border_variant)
                        .child(
                            div().mr(px(4.)).child(
                                Label::new("Thinking")
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            ),
                        )
                        .children(self.thinking_levels.iter().enumerate().map(
                            |(position, level)| {
                                let level = level.clone();
                                Button::new(("thinking-level", position), level.clone())
                                    .size(ButtonSize::Compact)
                                    .toggle_state(current_level == level.as_ref())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.select_thinking_level(level.clone(), cx)
                                    }))
                            },
                        )),
                )
            })
            .into_any_element()
    }

    fn render_model_chip(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let open = self.model_picker.is_some();
        let chip = h_flex()
            .id("model-chip")
            .gap(px(4.))
            .h(px(22.))
            .px(px(6.))
            .min_w(px(0.))
            .rounded(px(5.))
            .cursor_pointer()
            .when(open, |this| this.bg(colors.ghost_element_selected))
            .hover(|style| style.bg(colors.ghost_element_hover))
            .active(|style| style.bg(colors.ghost_element_active))
            // Keep focus in the composer; the picker focuses its own filter.
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .on_click(cx.listener(|this, _, window, cx| {
                this.toggle_model_picker(&ToggleModelPicker, window, cx)
            }))
            .tooltip(Tooltip::with_keys("Change model", "⌘⇧M"))
            .when(self.switching_model, |this| {
                this.child(Spinner::new().size(IconSize::XSmall).color(Color::Accent))
            })
            .child(
                Label::new(self.model_label())
                    .size(LabelSize::XSmall)
                    .color(Color::Muted)
                    .truncate(),
            )
            .child(
                Icon::new(IconName::ChevronDown)
                    .size(IconSize::XSmall)
                    .color(Color::Placeholder),
            );

        let picker = self
            .model_picker
            .as_ref()
            .map(|picker| self.render_model_picker(picker, cx));
        div()
            .relative()
            .flex()
            .min_w(px(0.))
            .child(chip)
            .when_some(picker, |this, picker| {
                // Anchored at the chip's top-left corner, opening upwards.
                this.child(
                    div().absolute().top_0().left_0().child(
                        deferred(
                            anchored()
                                .anchor(Corner::BottomLeft)
                                .snap_to_window_with_margin(px(8.))
                                .child(picker),
                        )
                        .with_priority(1),
                    ),
                )
            })
            .into_any_element()
    }

    fn render_composer(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors.clone();
        let ready = matches!(self.connection, Connection::Ready(_));
        let composer_focus = self.composer.read(cx).focus_handle(cx);
        let focused = composer_focus.is_focused(window);
        let slash_matches = self.slash_matches(cx);
        let slash_menu =
            (!slash_matches.is_empty()).then(|| self.render_slash_menu(&slash_matches, cx));
        let has_text = !self.composer.read(cx).text().trim().is_empty();

        // Fixed height, so the list does not jump when activity appears.
        let activity_line = h_flex()
            .h(px(22.))
            .gap(px(6.))
            .when_some(self.activity(), |this, activity| {
                this.child(Spinner::new().size(IconSize::XSmall).color(Color::Accent))
                    .child(LoadingLabel::new(activity).size(LabelSize::Small))
            })
            .when(self.running, |this| {
                this.child(div().flex_1()).child(
                    h_flex()
                        .gap(px(4.))
                        .child(
                            Label::new("to stop")
                                .size(LabelSize::XSmall)
                                .color(Color::Placeholder),
                        )
                        .child(KeyHint::new("Esc")),
                )
            });

        let send_tooltip = if self.running {
            "Steer the run"
        } else {
            "Send"
        };
        let plugin_buttons: Vec<AnyElement> = {
            let registry = self.plugins.read(cx);
            registry
                .composer_commands()
                .map(|command| {
                    let active = registry.is_command_active(&command.id);
                    let tooltip = Tooltip::with_optional_keys(
                        command.title.clone(),
                        command
                            .keybinding
                            .as_deref()
                            .map(|keys| plugins::display_keys(keys).into()),
                    );
                    let command_id = command.id.clone();
                    IconButton::new(
                        ElementId::Name(format!("plugin-command:{}", command.id).into()),
                        command.icon,
                    )
                    .size(ButtonSize::Medium)
                    .when(active, |this| {
                        this.style(ButtonStyle::Tinted(TintColor::Error))
                    })
                    .tooltip(tooltip)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.run_plugin_command(&command_id, cx)),
                    )
                    .into_any_element()
                })
                .collect()
        };

        let actions = h_flex()
            .gap(px(4.))
            .children(plugin_buttons)
            .when(self.running, |this| {
                this.child(
                    IconButton::new("stop-run", IconName::Stop)
                        .style(ButtonStyle::Tinted(TintColor::Error))
                        .size(ButtonSize::Medium)
                        .icon_size(IconSize::XSmall)
                        .tooltip(Tooltip::with_keys("Stop", "Esc"))
                        .on_click(cx.listener(|this, _, _, cx| this.request_abort(cx))),
                )
            })
            .child(
                IconButton::new("send", IconName::ArrowUp)
                    .style(if has_text && ready {
                        ButtonStyle::Tinted(TintColor::Accent)
                    } else {
                        ButtonStyle::Filled
                    })
                    .size(ButtonSize::Medium)
                    .disabled(!has_text || !ready)
                    .loading(self.pending_prompt && !self.running)
                    .tooltip(Tooltip::with_keys(send_tooltip, "Enter"))
                    .on_click(cx.listener(|this, _, window, cx| this.submit(&Submit, window, cx))),
            );

        div()
            .flex_none()
            .w_full()
            .flex()
            .justify_center()
            .px(px(20.))
            .pt(px(2.))
            .pb(px(14.))
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(CONTENT_MAX_WIDTH))
                    .gap(px(6.))
                    .child(activity_line)
                    .child(
                        v_flex()
                            .relative()
                            .when(slash_menu.is_some(), |this| this.key_context(SLASH_CONTEXT))
                            // Files dropped from Finder become chips, like pasted ones.
                            .on_drop(cx.listener(
                                |this, paths: &gpui::ExternalPaths, window, cx| {
                                    let paths = paths.paths().to_vec();
                                    this.composer.update(cx, |composer, cx| {
                                        composer.attach_paths(&paths, cx)
                                    });
                                    window.focus(&this.composer.read(cx).focus_handle(cx));
                                },
                            ))
                            .drag_over::<gpui::ExternalPaths>({
                                let border = colors.text_accent;
                                move |style, _, _, _| style.border_color(border)
                            })
                            .children(slash_menu)
                            .w_full()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(if focused {
                                colors.border_focused
                            } else {
                                colors.border
                            })
                            .bg(colors.panel_background)
                            .when(!ready, |this| this.opacity(0.6))
                            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                window.focus(&composer_focus);
                                cx.stop_propagation();
                            })
                            .child(self.composer.clone())
                            .child(
                                h_flex()
                                    .gap(px(8.))
                                    .px(px(8.))
                                    .pb(px(6.))
                                    .child(
                                        h_flex()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .gap(px(2.))
                                            .child(self.render_model_chip(cx))
                                            .children(self.render_usage_ring(cx)),
                                    )
                                    .child(actions),
                            ),
                    ),
            )
    }
}

impl Focusable for ChatView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ChatView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.focus_pending_dialog(window, cx);
        self.sync_plugin_view_focus(window, cx);
        let colors = cx.theme().colors.clone();
        let body = if self.rows.is_empty() {
            let content = match &self.connection {
                Connection::Ready(_) if self.loading_history => v_flex()
                    .items_center()
                    .gap(px(10.))
                    .child(Spinner::new().size(IconSize::Large).color(Color::Muted))
                    .child(LoadingLabel::new("Loading session history…")),
                Connection::Ready(_) => v_flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        Icon::new(IconName::Sparkle)
                            .size(IconSize::Large)
                            .color(Color::Placeholder),
                    )
                    .child(Label::new("Ready").color(Color::Muted))
                    .child(
                        Label::new("Describe a task below. Enter sends it to pi.")
                            .size(LabelSize::Small)
                            .color(Color::Placeholder),
                    ),
                Connection::Failed(_) | Connection::Exited(_) => v_flex(),
            };
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(content)
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h(px(0.))
                .child(
                    list(
                        self.list_state.clone(),
                        cx.processor(|this, index, window, cx| this.render_row(index, window, cx)),
                    )
                    .size_full(),
                )
                .into_any_element()
        };

        // Spinners and loading labels only move while pi is busy.
        let animating = self.running
            || self.pending_prompt
            || self.loading_history
            || self.switching_model
            || self.compacting
            || self.navigating.is_some()
            || self.forking.is_some()
            || self.tree.as_ref().is_some_and(|view| view.lines.is_none());
        self.animation.set_active(animating, cx);
        // The picker scrolled to its highlight during this render's layout.
        if let Some(picker) = &mut self.model_picker {
            picker.scroll_to_highlight = false;
        }

        if let Some(view) = &mut self.tree
            && view.scroll_to_selected
        {
            view.scroll.scroll_to_item(view.selected);
            view.scroll_to_selected = false;
        }
        let tree_overlay = self.tree.as_ref().map(|view| self.render_tree(view, cx));

        let key_context = if self.dialog_takes_keys() {
            format!("{KEY_CONTEXT} {}", dialog::DIALOG_CONTEXT)
        } else {
            KEY_CONTEXT.to_string()
        };
        v_flex()
            .relative()
            .key_context(key_context.as_str())
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::dialog_up))
            .on_action(cx.listener(Self::dialog_down))
            .on_action(cx.listener(Self::dialog_confirm))
            .on_action(cx.listener(Self::dialog_cancel))
            .on_action(cx.listener(Self::release_plugin_view))
            .on_key_down(cx.listener(|this, event, window, cx| {
                // Only while the card has the keys, not while typing in the composer.
                if this.focus_handle.is_focused(window) {
                    this.dialog_key_down(event, cx);
                }
            }))
            .on_action(cx.listener(Self::toggle_tree_action))
            .on_action(cx.listener(Self::tree_up))
            .on_action(cx.listener(Self::tree_down))
            .on_action(cx.listener(Self::tree_first))
            .on_action(cx.listener(Self::tree_last))
            .on_action(cx.listener(Self::tree_previous_branch))
            .on_action(cx.listener(Self::tree_next_branch))
            .on_action(cx.listener(Self::tree_go))
            .on_action(cx.listener(Self::tree_go_with_summary))
            .on_action(cx.listener(Self::tree_fork))
            .on_action(cx.listener(Self::tree_close))
            .on_action(cx.listener(Self::slash_up))
            .on_action(cx.listener(Self::slash_down))
            .on_action(cx.listener(Self::slash_complete))
            .on_action(cx.listener(Self::slash_dismiss))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::cancel_run))
            .on_action(cx.listener(Self::toggle_model_picker))
            .on_action(cx.listener(Self::copy_selection))
            .size_full()
            .bg(colors.editor_background)
            .text_color(colors.text)
            .text_size(px(crate::settings::Settings::get(cx).agent_font_size))
            .line_height(px(crate::settings::Settings::get(cx).agent_line_height()))
            .children(self.render_connection_banner(cx))
            .child(body)
            .child(self.render_composer(window, cx))
            .children(tree_overlay)
    }
}

async fn pump_events(
    this: WeakEntity<ChatView>,
    mut receiver: mpsc::Receiver<JsonlMessage>,
    cx: &mut AsyncApp,
) {
    while let Some(first) = receiver.next().await {
        let mut batch = vec![first];
        while batch.len() < MAX_RECORDS_PER_UPDATE {
            match receiver.try_next() {
                Ok(Some(message)) => batch.push(message),
                _ => break,
            }
        }
        if this
            .update(cx, |this, cx| this.apply_batch(batch, cx))
            .is_err()
        {
            return;
        }
    }
}

/// Rebuilds rows from `get_messages`. Tool calls come from the assistant's
/// `toolCall` blocks and are completed by the matching `toolResult` message,
/// the same shape the live stream produces from `tool_execution_*` events.
fn history_rows(messages: &[Value]) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut tool_rows = HashMap::new();
    for message in messages {
        match message["role"].as_str().unwrap_or_default() {
            "user" => {
                let text = sessions::message_text(message);
                rows.push(Row::User {
                    images: Picture::of_message(&message["content"], &text),
                    text: SharedString::from(text),
                    steering: false,
                })
            }
            "assistant" => {
                let (text, thinking) = assistant_blocks(message);
                let error = stop_error(message);
                if !text.is_empty() || !thinking.is_empty() || error.is_some() {
                    rows.push(Row::Assistant {
                        text,
                        thinking,
                        streaming: false,
                        error,
                        thought_overrides: BTreeMap::new(),
                    });
                }
                for block in message["content"].as_array().into_iter().flatten() {
                    if block["type"] != "toolCall" {
                        continue;
                    }
                    rows.push(Row::Tool {
                        name: block["name"].as_str().unwrap_or("tool").to_string(),
                        detail: tool_detail(&block["arguments"]),
                        status: ToolStatus::Running,
                        output: String::new(),
                        images: Vec::new(),
                        expanded: None,
                        view: None,
                    });
                    if let Some(id) = block["id"].as_str() {
                        tool_rows.insert(id.to_string(), rows.len() - 1);
                    }
                }
            }
            "toolResult" => {
                let index = message["toolCallId"]
                    .as_str()
                    .and_then(|id| tool_rows.get(id).copied());
                if let Some(index) = index
                    && let Some(Row::Tool {
                        status,
                        output,
                        images,
                        ..
                    }) = rows.get_mut(index)
                {
                    *output = preview_lines(&content_text(message));
                    *images = Picture::from_content(&message["content"]);
                    *status = if message["isError"].as_bool() == Some(true) {
                        ToolStatus::Failed
                    } else {
                        ToolStatus::Done
                    };
                }
            }
            "bashExecution" => rows.push(Row::Tool {
                name: "bash (manual)".to_string(),
                detail: tool_detail(&json!({ "command": message["command"] })),
                status: if message["exitCode"].as_i64() == Some(0) {
                    ToolStatus::Done
                } else {
                    ToolStatus::Failed
                },
                output: preview_lines(message["output"].as_str().unwrap_or_default()),
                images: Vec::new(),
                expanded: None,
                view: None,
            }),
            "compactionSummary" => rows.push(Row::Summary {
                kind: SummaryKind::Compaction,
                state: SummaryState::Done,
                summary: message["summary"].as_str().unwrap_or_default().to_string(),
                tokens_before: message["tokensBefore"].as_u64(),
                tokens_after: None,
                expanded: false,
            }),
            "branchSummary" => rows.push(Row::Summary {
                kind: SummaryKind::Branch,
                state: SummaryState::Done,
                summary: message["summary"].as_str().unwrap_or_default().to_string(),
                tokens_before: None,
                tokens_after: None,
                expanded: false,
            }),
            "custom" if message["display"].as_bool() == Some(true) => rows.push(Row::Notice {
                tone: Tone::Info,
                text: sessions::message_text(message),
            }),
            _ => {}
        }
    }
    // A call without a result was interrupted; do not show it as still running.
    for row in &mut rows {
        if let Row::Tool { status, .. } = row
            && *status == ToolStatus::Running
        {
            *status = ToolStatus::Failed;
        }
    }
    rows
}

fn tool_icon(name: &str) -> IconName {
    match name {
        "bash" | "bash (manual)" => IconName::Terminal,
        "read" | "write" | "edit" | "ls" | "find" | "grep" => IconName::File,
        _ => IconName::Tool,
    }
}

fn stop_error(message: &Value) -> Option<String> {
    match message["stopReason"].as_str() {
        Some("error") => Some(
            message["errorMessage"]
                .as_str()
                .unwrap_or("Provider error")
                .to_string(),
        ),
        Some("aborted") => Some("Response interrupted.".to_string()),
        _ => None,
    }
}

/// Text and thinking blocks of an assistant message.
fn assistant_blocks(message: &Value) -> (String, Vec<String>) {
    if let Some(text) = message["content"].as_str() {
        return (text.to_string(), Vec::new());
    }
    let mut text = Vec::new();
    let mut thinking = Vec::new();
    for block in message["content"].as_array().into_iter().flatten() {
        match block["type"].as_str() {
            Some("text") => text.extend(block["text"].as_str()),
            Some("thinking") => thinking.extend(
                block["thinking"]
                    .as_str()
                    .filter(|thought| !thought.trim().is_empty())
                    .map(str::to_string),
            ),
            _ => {}
        }
    }
    (text.join("\n\n"), thinking)
}

struct Thought {
    /// From a `**Heading**` line; untitled thoughts preview their first line.
    title: Option<String>,
    body: String,
}

impl Thought {
    fn preview(&self) -> String {
        self.title
            .clone()
            .unwrap_or_else(|| sessions::one_line(&self.body, 200))
    }

    /// Worth a disclosure button: there is more text than the one-line preview.
    fn is_expandable(&self) -> bool {
        if self.title.is_some() {
            !self.body.is_empty()
        } else {
            self.body
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count()
                > 1
                || self.body.chars().count() > 90
        }
    }
}

fn text_hash(text: &str) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Image previews under a message or tool result. A click on an image file
/// opens it.
fn thumbnails(row: usize, images: &[Picture], colors: &crate::ui::ThemeColors) -> AnyElement {
    h_flex()
        .flex_wrap()
        .gap(px(8.))
        .children(images.iter().enumerate().map(|(position, picture)| {
            let path = match picture {
                Picture::File(path) => Some(path.clone()),
                Picture::Data(_) => None,
            };
            div()
                .id(ElementId::Name(format!("thumb-{row}-{position}").into()))
                .rounded(px(6.))
                .border_1()
                .border_color(colors.border_variant)
                .overflow_hidden()
                .when_some(path, |this, path| {
                    this.cursor_pointer().on_click(move |_, _, cx| {
                        cx.open_url(&format!("file://{}", path.display()))
                    })
                })
                .child(
                    gpui::img(picture.source())
                        .max_w(px(320.))
                        .max_h(px(200.))
                        .object_fit(gpui::ObjectFit::Contain),
                )
        }))
        .into_any_element()
}

/// A small chevron button that opens or closes a detail section.
fn disclosure_button(
    id: impl Into<ElementId>,
    open: bool,
    what: &str,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> IconButton {
    IconButton::new(
        id,
        if open {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        },
    )
    .size(ButtonSize::Compact)
    .icon_size(IconSize::XSmall)
    .tooltip(Tooltip::text(if open {
        format!("Hide {what}")
    } else {
        format!("Show {what}")
    }))
    .on_click(on_click)
}

/// Splits thinking into separate thoughts: every block is a thought, and
/// inside a block a line that is entirely `**Title**` starts a new one (the
/// format reasoning summaries use). Untitled thoughts take their first line.
fn split_thoughts(blocks: &[String]) -> Vec<Thought> {
    let mut thoughts = Vec::new();
    for block in blocks {
        let mut current: Option<Thought> = None;
        for line in block.lines() {
            let trimmed = line.trim();
            let heading = trimmed
                .strip_prefix("**")
                .and_then(|rest| rest.strip_suffix("**"))
                .filter(|title| !title.is_empty() && !title.contains("**"));
            if let Some(title) = heading {
                thoughts.extend(current.take());
                current = Some(Thought {
                    title: Some(title.trim().to_string()),
                    body: String::new(),
                });
            } else {
                let thought = current.get_or_insert_with(|| Thought {
                    title: None,
                    body: String::new(),
                });
                thought.body.push_str(line);
                thought.body.push('\n');
            }
        }
        thoughts.extend(current.take());
    }
    thoughts
        .into_iter()
        .filter_map(|mut thought| {
            thought.body = thought.body.trim().to_string();
            (thought.title.is_some() || !thought.body.is_empty()).then_some(thought)
        })
        .collect()
}

/// `200000` → `200K`, `1000000` → `1M`.
/// "850", "128k", "7.9M", "155M".
fn format_tokens(tokens: u64) -> String {
    match tokens {
        0..1_000 => tokens.to_string(),
        1_000..1_000_000 => format!("{}k", tokens / 1_000),
        1_000_000..10_000_000 => {
            let tenths = tokens / 100_000;
            if tenths.is_multiple_of(10) {
                format!("{}M", tenths / 10)
            } else {
                format!("{}.{}M", tenths / 10, tenths % 10)
            }
        }
        _ => format!("{}M", tokens / 1_000_000),
    }
}

fn content_text(result: &Value) -> String {
    if let Some(text) = result.as_str() {
        return text.to_string();
    }
    result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_detail(args: &Value) -> String {
    const PREFERRED_KEYS: [&str; 7] = [
        "command",
        "path",
        "file_path",
        "pattern",
        "query",
        "url",
        "task",
    ];
    let detail = PREFERRED_KEYS
        .iter()
        .find_map(|key| args[*key].as_str())
        .map(|value| value.lines().next().unwrap_or_default().to_string())
        .unwrap_or_else(|| {
            if args.is_null() {
                String::new()
            } else {
                args.to_string()
            }
        });
    truncate_chars(&detail, 160)
}

fn preview_lines(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut preview = truncate_chars(
        &lines
            .iter()
            .take(TOOL_OUTPUT_PREVIEW_LINES)
            .copied()
            .collect::<Vec<_>>()
            .join("\n"),
        TOOL_OUTPUT_PREVIEW_CHARS,
    );
    if lines.len() > TOOL_OUTPUT_PREVIEW_LINES {
        preview.push_str(&format!(
            "\n… {} more lines",
            lines.len() - TOOL_OUTPUT_PREVIEW_LINES
        ));
    }
    preview
}

fn truncate_chars(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((index, _)) => format!("{}…", &text[..index]),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_blocks_split_text_and_thinking() {
        let message = json!({
            "content": [
                { "type": "thinking", "thinking": "plan" },
                { "type": "text", "text": "one" },
                { "type": "toolCall", "id": "x", "name": "bash", "arguments": {} },
                { "type": "text", "text": "two" }
            ]
        });
        assert_eq!(
            assistant_blocks(&message),
            ("one\n\ntwo".to_string(), vec!["plan".to_string()])
        );
    }

    #[test]
    fn previews_respect_char_boundaries() {
        assert_eq!(truncate_chars("größe", 3), "grö…");
        let long = (1..=20)
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(preview_lines(&long).ends_with("… 8 more lines"));
    }

    #[test]
    fn history_pairs_tool_calls_with_results() {
        let messages = vec![
            json!({ "role": "system", "content": "" }),
            json!({ "role": "user", "content": "list files" }),
            json!({ "role": "assistant", "stopReason": "toolUse", "content": [
                { "type": "text", "text": "Checking" },
                { "type": "toolCall", "id": "a", "name": "bash", "arguments": { "command": "ls" } },
                { "type": "toolCall", "id": "b", "name": "read", "arguments": { "path": "x" } }
            ]}),
            json!({ "role": "toolResult", "toolCallId": "a", "isError": false,
                    "content": [{ "type": "text", "text": "src" }] }),
            json!({ "role": "assistant", "stopReason": "stop", "content": [{ "type": "text", "text": "done" }] }),
        ];
        let rows = history_rows(&messages);
        assert_eq!(rows.len(), 5);
        assert!(matches!(&rows[0], Row::User { text, .. } if text.as_ref() == "list files"));
        assert!(
            matches!(&rows[2], Row::Tool { status: ToolStatus::Done, output, .. } if output == "src")
        );
        // No result for "b": shown as interrupted, not running forever.
        assert!(matches!(
            &rows[3],
            Row::Tool {
                status: ToolStatus::Failed,
                ..
            }
        ));
        assert!(matches!(&rows[4], Row::Assistant { text, .. } if text == "done"));
    }

    #[test]
    fn thoughts_split_by_block_and_heading() {
        let blocks = vec![
            "**Inspecting the repo**\nLooking at src first.\n\n**Planning**\nThen tests."
                .to_string(),
            "Plain thought without a heading.\nSecond line.".to_string(),
        ];
        let thoughts = split_thoughts(&blocks);
        let previews: Vec<String> = thoughts.iter().map(Thought::preview).collect();
        assert_eq!(
            previews,
            [
                "Inspecting the repo",
                "Planning",
                "Plain thought without a heading."
            ]
        );
        assert_eq!(thoughts[0].body, "Looking at src first.");
        // The untitled thought keeps its full text and is not duplicated as a title.
        assert_eq!(thoughts[2].title, None);
        assert_eq!(
            thoughts[2].body,
            "Plain thought without a heading.\nSecond line."
        );
        assert!(thoughts[2].is_expandable());
        let short = split_thoughts(&["Short thought.".to_string()]);
        assert!(!short[0].is_expandable());
        assert!(split_thoughts(&["  ".to_string()]).is_empty());
    }

    #[test]
    fn token_counts_are_short() {
        assert_eq!(format_tokens(850), "850");
        assert_eq!(format_tokens(200_000), "200k");
        assert_eq!(format_tokens(1_000_000), "1M");
        assert_eq!(format_tokens(7_940_000), "7.9M");
        assert_eq!(format_tokens(155_300_000), "155M");
    }

    #[test]
    fn tool_detail_prefers_command() {
        assert_eq!(tool_detail(&json!({ "command": "ls -la\npwd" })), "ls -la");
        assert_eq!(
            tool_detail(&json!({ "path": "src/main.rs" })),
            "src/main.rs"
        );
    }
}
