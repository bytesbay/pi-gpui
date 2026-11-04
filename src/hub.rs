//! Root view: a sidebar of workspaces and their pi sessions, and the active chat.
//!
//! Every opened session owns its own pi process. Switching between them never
//! stops work; closing the app closes stdin of each process.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, AnyView, App, ClickEvent, Context, ElementId, Entity, FocusHandle, Focusable,
    FontWeight, KeyBinding, ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    PathPromptOptions, Pixels, Point, SharedString, StyleRefinement, Subscription, Task, Window,
    WindowControlArea, actions, anchored, deferred, div, px,
};
use serde_json::{Value, json};

use crate::chat::{ChatEvent, ChatStatus, ChatView, DetailsExpanded};
use crate::pi::{self, Installation};
use crate::platform;
use crate::plugins::{CommandContext, PluginEvent, PluginRegistry, RunPluginCommand};
use crate::sessions::{self, SessionSummary};
use crate::text_input::TextInput;
use crate::ui::AnimationTicker;
use crate::ui::prelude::*;
use crate::updater::{AutoUpdater, UpdateStatus};

actions!(hub, [AddWorkspace, NewChat, ToggleDetails, CancelRename]);

/// ⌘1…⌘9: the n-th session in the sidebar (1-based).
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = hub, no_json)]
pub struct SelectSession {
    pub index: usize,
}

const KEY_CONTEXT: &str = "Hub";
const RENAME_CONTEXT: &str = "SessionRename";
const SIDEBAR_WIDTH: f32 = 272.;
const SESSIONS_PER_WORKSPACE: usize = 20;
/// Our own title bar replaces the native one (see `main.rs`). The traffic
/// lights are placed at (12, 11) so they sit centered in this height.
pub const TITLE_BAR_HEIGHT: f32 = 38.;
/// Room for the macOS traffic lights, as in Zed's `TRAFFIC_LIGHT_PADDING`.
const TRAFFIC_LIGHT_PADDING: f32 = 84.;

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-o", AddWorkspace, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-n", NewChat, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-shift-e", ToggleDetails, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", CancelRename, Some(RENAME_CONTEXT)),
    ]);
    cx.bind_keys((1..=9).map(|index| {
        KeyBinding::new(
            &format!("cmd-{index}"),
            SelectSession { index },
            Some(KEY_CONTEXT),
        )
    }));
}

enum InstallationState {
    Checking,
    Ready(Installation),
    Failed(pi::CheckError),
}

#[derive(PartialEq, Eq)]
enum SessionsState {
    NotLoaded,
    Loading,
    Loaded,
}

/// One session line in the sidebar.
#[derive(Clone)]
struct SessionRow {
    id: ElementId,
    title: String,
    open_chat: Option<Entity<ChatView>>,
    workspace: PathBuf,
    file: Option<PathBuf>,
    /// "3h" for listed sessions.
    age: Option<String>,
    hidden: bool,
}

struct SessionMenu {
    row: SessionRow,
    position: Point<Pixels>,
}

struct Renaming {
    row: SessionRow,
    input: Entity<TextInput>,
}

impl Renaming {
    fn matches(&self, row: &SessionRow) -> bool {
        match (&self.row.file, &row.file) {
            (Some(left), Some(right)) => left == right,
            _ => self.row.open_chat.is_some() && self.row.open_chat == row.open_chat,
        }
    }
}

struct OpenChat {
    view: Entity<ChatView>,
    _subscription: Subscription,
}

struct WorkspaceEntry {
    path: PathBuf,
    name: SharedString,
    expanded: bool,
    sessions: Vec<SessionSummary>,
    sessions_state: SessionsState,
    /// A listing is in flight; the previous list stays visible meanwhile.
    refreshing: bool,
    chats: Vec<OpenChat>,
    _load_task: Task<()>,
}

impl WorkspaceEntry {
    fn new(path: PathBuf, expanded: bool) -> Self {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        Self {
            path,
            name: name.into(),
            expanded,
            sessions: Vec::new(),
            sessions_state: SessionsState::NotLoaded,
            refreshing: false,
            chats: Vec::new(),
            _load_task: Task::ready(()),
        }
    }

    fn has_running_chat(&self, cx: &App) -> bool {
        self.chats
            .iter()
            .any(|chat| chat.view.read(cx).is_running())
    }
}

pub struct HubView {
    workspaces: Vec<WorkspaceEntry>,
    active_chat: Option<Entity<ChatView>>,
    installation: InstallationState,
    /// Message bar above the main area; the color picks its style.
    notice: Option<(String, Color)>,
    /// Session whose lease check is in flight; its row shows a spinner.
    opening_session: Option<PathBuf>,
    state_path: Option<PathBuf>,
    /// Workspace from the command line: open a new chat once pi is verified.
    pending_new_chat: Option<PathBuf>,
    focus_active_chat: bool,
    /// Mouse went down in the title bar; the next move starts a window drag.
    title_bar_drag_pending: bool,
    /// Hidden from the sidebar by the user (app state, pi's files untouched).
    hidden_sessions: HashSet<PathBuf>,
    /// Workspaces whose hidden sessions are shown for now.
    show_hidden: HashSet<PathBuf>,
    session_menu: Option<SessionMenu>,
    renaming: Option<Renaming>,
    /// Sessions being renamed through a short-lived pi process.
    renaming_sessions: HashSet<PathBuf>,
    /// ⌘ is held: the sidebar shows ⌘1…⌘9 next to sessions.
    command_held: bool,
    focus_handle: FocusHandle,
    plugins: Entity<PluginRegistry>,
    _plugin_subscriptions: Vec<Subscription>,
    _check_task: Task<()>,
    _prompt_task: Task<()>,
    _open_task: Task<()>,
    _save_task: Task<()>,
    animation: AnimationTicker,
}

impl HubView {
    pub fn new(
        initial_workspace: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Installed or updated pi in a terminal? Coming back checks again.
        cx.observe_window_activation(window, |this: &mut Self, window, cx| {
            if window.is_window_active()
                && matches!(this.installation, InstallationState::Failed(_))
            {
                this.check_installation(cx);
            }
        })
        .detach();
        let state_path = state_file_path();
        let expanded = state_path.as_deref().is_some_and(load_expand_details);
        cx.set_global(DetailsExpanded { expanded });
        let plugins = cx.new(|cx| PluginRegistry::new(KEY_CONTEXT, cx));
        let plugin_subscriptions = vec![
            cx.subscribe(&plugins, Self::on_plugin_event),
            // Plugin statuses live in the title bar.
            cx.observe(&plugins, |_, _, cx| cx.notify()),
            // The update chip lives in the title bar; manual checks report here.
            cx.observe(&AutoUpdater::global(cx), |this, updater, cx| {
                let result = updater.update(cx, |updater, _| updater.take_manual_result());
                if let Some(result) = result {
                    let current = updater.read(cx).current_version();
                    this.notice = match result {
                        UpdateStatus::UpToDate | UpdateStatus::Idle => Some((
                            format!("Pi GPUI {current} is up to date."),
                            Color::Default,
                        )),
                        UpdateStatus::Ready(version) => Some((
                            format!("Pi GPUI {version} is ready: use \"Restart to update\" in the title bar."),
                            Color::Default,
                        )),
                        UpdateStatus::Failed(error) => {
                            Some((format!("Update failed: {error}"), Color::Warning))
                        }
                        UpdateStatus::Checking | UpdateStatus::Downloading(_) => None,
                    };
                }
                cx.notify();
            }),
        ];
        let mut this = Self {
            workspaces: Vec::new(),
            active_chat: None,
            installation: InstallationState::Checking,
            notice: None,
            opening_session: None,
            state_path,
            pending_new_chat: None,
            focus_active_chat: false,
            title_bar_drag_pending: false,
            hidden_sessions: HashSet::new(),
            show_hidden: HashSet::new(),
            session_menu: None,
            renaming: None,
            renaming_sessions: HashSet::new(),
            command_held: false,
            focus_handle: cx.focus_handle(),
            plugins,
            _plugin_subscriptions: plugin_subscriptions,
            _check_task: Task::ready(()),
            _prompt_task: Task::ready(()),
            _open_task: Task::ready(()),
            _save_task: Task::ready(()),
            animation: AnimationTicker::default(),
        };

        // The state file is tiny; reading it before the first frame avoids an
        // empty flash. Project folders are not touched here: they may need a
        // macOS permission prompt, which must not block startup.
        for (path, expanded) in this
            .state_path
            .as_deref()
            .map(load_state)
            .unwrap_or_default()
        {
            this.workspaces.push(WorkspaceEntry::new(path, expanded));
        }
        this.hidden_sessions = this
            .state_path
            .as_deref()
            .map(load_hidden_sessions)
            .unwrap_or_default();
        if let Some(path) = initial_workspace {
            this.add_workspace(path.clone(), cx);
            this.pending_new_chat = Some(path);
            this.save(cx);
        }
        let expanded: Vec<PathBuf> = this
            .workspaces
            .iter()
            .filter(|entry| entry.expanded)
            .map(|entry| entry.path.clone())
            .collect();
        for path in expanded {
            this.load_sessions(&path, cx);
        }
        this.check_installation(cx);
        let updated_from =
            AutoUpdater::global(cx).update(cx, |updater, _| updater.take_updated_from());
        if let Some(previous) = updated_from {
            this.notice = Some((
                format!(
                    "Updated to Pi GPUI {} (from {previous}).",
                    env!("CARGO_PKG_VERSION")
                ),
                Color::Default,
            ));
        }
        this
    }

    fn restart_to_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let running = self
            .workspaces
            .iter()
            .filter(|entry| entry.has_running_chat(cx))
            .count();
        let updater = AutoUpdater::global(cx);
        if running == 0 {
            updater.update(cx, |updater, cx| updater.restart_to_update(cx));
            return;
        }
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            "pi is still working. Restart anyway?",
            Some("Running work stops; the sessions themselves are saved."),
            &["Restart", "Cancel"],
            cx,
        );
        cx.spawn(async move |_, cx| {
            if answer.await.ok() == Some(0) {
                updater
                    .update(cx, |updater, cx| updater.restart_to_update(cx))
                    .ok();
            }
        })
        .detach();
    }

    fn toggle_details(&mut self, _: &ToggleDetails, _: &mut Window, cx: &mut Context<Self>) {
        let expanded = !cx.global::<DetailsExpanded>().expanded;
        cx.set_global(DetailsExpanded { expanded });
        self.save(cx);
        cx.notify();
    }

    fn on_plugin_event(
        &mut self,
        _: Entity<PluginRegistry>,
        event: &PluginEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            PluginEvent::Notify { message, color } => {
                self.notice = Some((message.clone(), *color));
                cx.notify();
            }
            PluginEvent::ComposerInsert { .. } | PluginEvent::ComposerSetText(_) => {
                match &self.active_chat {
                    Some(chat) => chat.update(cx, |chat, cx| chat.apply_plugin_edit(event, cx)),
                    None => {
                        self.notice = Some((
                            "Open a session first: the plugin has nowhere to insert text."
                                .to_string(),
                            Color::Warning,
                        ));
                        cx.notify();
                    }
                }
            }
        }
    }

    fn run_plugin_command(
        &mut self,
        action: &RunPluginCommand,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let context = match &self.active_chat {
            Some(chat) => {
                let chat = chat.read(cx);
                CommandContext {
                    workspace: Some(chat.workspace().to_path_buf()),
                    session: chat.session_file().map(Path::to_path_buf),
                }
            }
            None => CommandContext::default(),
        };
        self.plugins.update(cx, |registry, cx| {
            registry.run_command(&action.command, context, cx)
        });
    }

    fn check_installation(&mut self, cx: &mut Context<Self>) {
        self.installation = InstallationState::Checking;
        let check = cx.background_spawn(async { pi::check_installation() });
        self._check_task = cx.spawn(async move |this, cx| {
            let result = check.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(installation) => {
                        eprintln!(
                            "pi-gpui: using pi {} from {}",
                            installation.version,
                            installation.executable.display()
                        );
                        let version = installation.version;
                        AutoUpdater::global(cx)
                            .update(cx, |updater, _| updater.set_pi_version(version));
                        this.installation = InstallationState::Ready(installation);
                        if let Some(path) = this.pending_new_chat.take() {
                            this.start_chat(&path, None, cx);
                        }
                    }
                    Err(error) => {
                        eprintln!("pi-gpui: {error}");
                        this.installation = InstallationState::Failed(error);
                    }
                }
                cx.notify();
            })
            .ok();
        });
        cx.notify();
    }

    fn entry_mut(&mut self, workspace: &Path) -> Option<&mut WorkspaceEntry> {
        self.workspaces
            .iter_mut()
            .find(|entry| entry.path == workspace)
    }

    fn add_workspace(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let path = path.canonicalize().unwrap_or(path);
        if let Some(entry) = self.entry_mut(&path) {
            entry.expanded = true;
        } else {
            self.workspaces
                .push(WorkspaceEntry::new(path.clone(), true));
        }
        self.load_sessions(&path, cx);
    }

    fn prompt_add_workspace(&mut self, _: &AddWorkspace, _: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some("Add".into()),
        });
        self._prompt_task = cx.spawn(async move |this, cx| {
            let selection = paths.await;
            this.update(cx, |this, cx| {
                match selection {
                    Ok(Ok(Some(paths))) => {
                        for path in paths {
                            this.add_workspace(path, cx);
                        }
                        this.save(cx);
                    }
                    Ok(Ok(None)) | Err(_) => {}
                    Ok(Err(error)) => {
                        this.notice = Some((
                            format!("Could not open the folder picker: {error}"),
                            Color::Warning,
                        ));
                    }
                }
                cx.notify();
            })
            .ok();
        });
    }

    fn remove_workspace(&mut self, workspace: &Path, cx: &mut Context<Self>) {
        let Some(index) = self
            .workspaces
            .iter()
            .position(|entry| entry.path == workspace)
        else {
            return;
        };
        if self.workspaces[index].has_running_chat(cx) {
            self.notice = Some((
                format!(
                    "Work is still running in \"{}\". Stop it before removing the project.",
                    self.workspaces[index].name
                ),
                Color::Warning,
            ));
            cx.notify();
            return;
        }
        // Dropping the chats closes their pi processes. Session files stay untouched.
        let removed = self.workspaces.remove(index);
        if let Some(active) = &self.active_chat
            && removed.chats.iter().any(|chat| chat.view == *active)
        {
            self.active_chat = None;
        }
        self.save(cx);
        cx.notify();
    }

    fn toggle_workspace(&mut self, workspace: &Path, cx: &mut Context<Self>) {
        let Some(entry) = self.entry_mut(workspace) else {
            return;
        };
        entry.expanded = !entry.expanded;
        let needs_load = entry.expanded && entry.sessions_state == SessionsState::NotLoaded;
        if needs_load {
            self.load_sessions(workspace, cx);
        }
        self.save(cx);
        cx.notify();
    }

    fn load_sessions(&mut self, workspace: &Path, cx: &mut Context<Self>) {
        let path = workspace.to_path_buf();
        let listing = cx.background_spawn({
            let path = path.clone();
            async move { sessions::list_sessions(&path, SESSIONS_PER_WORKSPACE) }
        });
        let task = cx.spawn(async move |this, cx| {
            let summaries = listing.await;
            this.update(cx, |this, cx| {
                if let Some(entry) = this.entry_mut(&path) {
                    entry.sessions = summaries;
                    entry.sessions_state = SessionsState::Loaded;
                    entry.refreshing = false;
                    cx.notify();
                }
            })
            .ok();
        });
        if let Some(entry) = self.entry_mut(workspace) {
            if entry.sessions_state == SessionsState::NotLoaded {
                entry.sessions_state = SessionsState::Loading;
            }
            entry.refreshing = true;
            // Replacing the task cancels an older, possibly stale listing.
            entry._load_task = task;
        }
    }

    fn new_chat(&mut self, _: &NewChat, _: &mut Window, cx: &mut Context<Self>) {
        let workspace = self
            .active_chat
            .as_ref()
            .map(|chat| chat.read(cx).workspace().to_path_buf())
            .or_else(|| self.workspaces.first().map(|entry| entry.path.clone()));
        match workspace {
            Some(workspace) => self.start_chat(&workspace, None, cx),
            None => {
                self.notice = Some(("Add a project first.".to_string(), Color::Warning));
                cx.notify();
            }
        }
    }

    /// Activates an already open chat for `session`, otherwise starts pi on it
    /// unless pi-gui currently holds the session.
    fn open_session(&mut self, workspace: &Path, session: PathBuf, cx: &mut Context<Self>) {
        let existing = self.entry_mut(workspace).and_then(|entry| {
            entry
                .chats
                .iter()
                .find(|chat| chat.view.read(cx).session_file() == Some(session.as_path()))
                .map(|chat| chat.view.clone())
        });
        if let Some(view) = existing {
            self.activate(view, cx);
            return;
        }

        self.opening_session = Some(session.clone());
        cx.notify();
        let workspace = workspace.to_path_buf();
        let lease = cx.background_spawn({
            let session = session.clone();
            async move { sessions::foreign_lease_holder(&session) }
        });
        self._open_task = cx.spawn(async move |this, cx| {
            let holder = lease.await;
            this.update(cx, |this, cx| {
                this.opening_session = None;
                match holder {
                Some(holder) => {
                    this.notice = Some((format!(
                        "This session is open in {holder}. Close it there first: pi does not protect a session file from two writers."
                    ), Color::Warning));
                    cx.notify();
                }
                None => this.start_chat(&workspace, Some(session), cx),
                }
            })
            .ok();
        });
    }

    fn start_chat(&mut self, workspace: &Path, session: Option<PathBuf>, cx: &mut Context<Self>) {
        let installation = match &self.installation {
            InstallationState::Ready(installation) => installation.clone(),
            InstallationState::Checking => {
                self.notice = Some((
                    "pi is still being checked, try again in a moment.".to_string(),
                    Color::Warning,
                ));
                cx.notify();
                return;
            }
            InstallationState::Failed(_) => {
                self.notice = Some((
                    "pi is not ready: see the steps in the main area.".to_string(),
                    Color::Warning,
                ));
                cx.notify();
                return;
            }
        };
        let workspace_path = workspace.to_path_buf();
        let plugins = self.plugins.clone();
        let view = cx.new(|cx| ChatView::new(workspace_path, session, installation, plugins, cx));
        let subscription = cx.subscribe(&view, Self::on_chat_event);
        let Some(entry) = self.entry_mut(workspace) else {
            return;
        };
        entry.expanded = true;
        entry.chats.push(OpenChat {
            view: view.clone(),
            _subscription: subscription,
        });
        self.activate(view, cx);
    }

    fn activate(&mut self, view: Entity<ChatView>, cx: &mut Context<Self>) {
        self.active_chat = Some(view);
        self.focus_active_chat = true;
        self.notice = None;
        cx.notify();
    }

    fn on_chat_event(&mut self, chat: Entity<ChatView>, event: &ChatEvent, cx: &mut Context<Self>) {
        match event {
            ChatEvent::Changed => cx.notify(),
            ChatEvent::NewSession => {
                let workspace = chat.read(cx).workspace().to_path_buf();
                self.start_chat(&workspace, None, cx);
            }
            ChatEvent::Settled => {
                // The run is written to disk now; refresh titles and order.
                let workspace = chat.read(cx).workspace().to_path_buf();
                self.load_sessions(&workspace, cx);
            }
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(state_path) = self.state_path.clone() else {
            return;
        };
        let state = json!({
            "version": 1,
            "expandDetails": cx.global::<DetailsExpanded>().expanded,
            "hiddenSessions": self.hidden_sessions.iter().collect::<Vec<_>>(),
            "workspaces": self.workspaces.iter().map(|entry| json!({
                "path": entry.path,
                "expanded": entry.expanded,
            })).collect::<Vec<_>>(),
        });
        let write = cx.background_spawn(async move { write_atomically(&state_path, &state) });
        self._save_task = cx.spawn(async move |this, cx| {
            if let Err(error) = write.await {
                eprintln!("pi-gpui: saving workspaces failed: {error}");
                this.update(cx, |this, cx| {
                    this.notice = Some((
                        format!("Could not save the project list: {error}"),
                        Color::Warning,
                    ));
                    cx.notify();
                })
                .ok();
            }
        });
    }

    /// Spinners in the sidebar. A running active chat already re-renders the
    /// hub (notifying a view also dirties its ancestors), so it does not count.
    fn sidebar_is_animating(&self, cx: &App) -> bool {
        matches!(self.installation, InstallationState::Checking)
            || self.opening_session.is_some()
            || self.plugins.read(cx).has_busy_status()
            || self.workspaces.iter().any(|entry| {
                entry.refreshing
                    || entry.chats.iter().any(|chat| {
                        Some(&chat.view) != self.active_chat.as_ref()
                            && chat.view.read(cx).is_running()
                    })
            })
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors.clone();
        let mut workspaces = Vec::with_capacity(self.workspaces.len());
        let mut numbered = 0;
        for (index, entry) in self.workspaces.iter().enumerate() {
            workspaces.push(self.render_workspace(index, entry, &mut numbered, cx));
        }

        v_flex()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .bg(colors.panel_background)
            .border_r_1()
            .border_color(colors.border_variant)
            .child(
                h_flex()
                    .flex_none()
                    .h(px(40.))
                    .pl(px(12.))
                    .pr(px(8.))
                    .child(
                        div().flex_1().child(
                            Label::new("Projects")
                                .size(LabelSize::Small)
                                .color(Color::Muted)
                                .weight(FontWeight::MEDIUM),
                        ),
                    )
                    .child(
                        IconButton::new("add-workspace", IconName::Plus)
                            .tooltip(Tooltip::with_keys("Add project", "⌘O"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.prompt_add_workspace(&AddWorkspace, window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .id("sidebar-scroll")
                    .when(
                        matches!(self.installation, InstallationState::Failed(_)),
                        |this| this.opacity(0.55),
                    )
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .px(px(6.))
                    .pb(px(10.))
                    .children(workspaces),
            )
            .child(self.render_installation_status(cx))
    }

    fn render_workspace(
        &self,
        index: usize,
        entry: &WorkspaceEntry,
        numbered: &mut usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = entry.path.clone();
        let running = entry.has_running_chat(cx);
        let status: Option<AnyElement> = if entry.refreshing {
            Some(Spinner::new().size(IconSize::XSmall).into_any_element())
        } else if running {
            Some(Indicator::dot(Color::Accent).into_any_element())
        } else {
            None
        };
        let (chevron, folder) = if entry.expanded {
            (IconName::ChevronDown, IconName::FolderOpen)
        } else {
            (IconName::ChevronRight, IconName::Folder)
        };

        let row_actions = h_flex()
            .gap(px(2.))
            .child(
                IconButton::new(("new-chat", index), IconName::Plus)
                    .size(ButtonSize::Compact)
                    .icon_size(IconSize::XSmall)
                    .tooltip(Tooltip::with_keys("New session", "⌘N"))
                    .on_click(cx.listener({
                        let path = path.clone();
                        move |this, _, _, cx| this.start_chat(&path, None, cx)
                    })),
            )
            .child(
                IconButton::new(("remove-workspace", index), IconName::Close)
                    .size(ButtonSize::Compact)
                    .icon_size(IconSize::XSmall)
                    .disabled(running)
                    .tooltip(Tooltip::text(if running {
                        "Work is running: stop it first"
                    } else {
                        "Remove from list"
                    }))
                    .on_click(cx.listener({
                        let path = path.clone();
                        move |this, _, _, cx| this.remove_workspace(&path, cx)
                    })),
            );

        let header = ListItem::new(("workspace", index))
            .start_slot(
                h_flex()
                    .gap(px(4.))
                    .child(
                        Icon::new(chevron)
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(Icon::new(folder).size(IconSize::Small).color(Color::Muted)),
            )
            .child(
                Label::new(entry.name.clone())
                    .size(LabelSize::Small)
                    .weight(FontWeight::MEDIUM)
                    .truncate(),
            )
            .when_some(status, |this, status| this.end_slot(status))
            .end_hover_slot(row_actions)
            .tooltip(Tooltip::text(path.display().to_string()))
            .on_click(cx.listener({
                let path = path.clone();
                move |this, _, _, cx| this.toggle_workspace(&path, cx)
            }));

        let mut rows = Vec::new();
        if entry.expanded {
            let all = self.session_rows(entry, cx);
            let hidden_count = all.iter().filter(|row| row.hidden).count();
            let show_hidden = self.show_hidden.contains(&entry.path);
            for row in all {
                if row.hidden && !show_hidden {
                    continue;
                }
                let number = (!row.hidden).then(|| {
                    *numbered += 1;
                    *numbered
                });
                rows.push(self.render_session_row(row, number.filter(|n| *n <= 9), cx));
            }
            if hidden_count > 0 {
                let label = if show_hidden {
                    "Hide hidden sessions".to_string()
                } else if hidden_count == 1 {
                    "1 hidden session".to_string()
                } else {
                    format!("{hidden_count} hidden sessions")
                };
                rows.push(
                    ListItem::new(("show-hidden", index))
                        .indent_level(1)
                        .start_slot(
                            div().w(px(12.)).flex().justify_center().child(
                                Icon::new(if show_hidden {
                                    IconName::Eye
                                } else {
                                    IconName::EyeOff
                                })
                                .size(IconSize::XSmall)
                                .color(Color::Placeholder),
                            ),
                        )
                        .child(
                            Label::new(label)
                                .size(LabelSize::XSmall)
                                .color(Color::Placeholder),
                        )
                        .on_click(cx.listener({
                            let path = path.clone();
                            move |this, _, _, cx| {
                                if !this.show_hidden.remove(&path) {
                                    this.show_hidden.insert(path.clone());
                                }
                                cx.notify();
                            }
                        }))
                        .into_any_element(),
                );
            }
            if rows.is_empty() {
                let placeholder = match entry.sessions_state {
                    SessionsState::Loaded => h_flex().child(
                        Label::new("No sessions yet")
                            .size(LabelSize::Small)
                            .color(Color::Placeholder),
                    ),
                    SessionsState::Loading | SessionsState::NotLoaded => h_flex()
                        .gap(px(6.))
                        .child(Spinner::new().size(IconSize::XSmall))
                        .child(LoadingLabel::new("Loading sessions…").size(LabelSize::Small)),
                };
                rows.push(
                    div()
                        .id(("sessions-placeholder", index))
                        .h(px(26.))
                        .pl(px(26.))
                        .flex()
                        .items_center()
                        .child(placeholder)
                        .into_any_element(),
                );
            }
        }

        v_flex()
            .mb(px(2.))
            .child(header)
            .children(rows)
            .into_any_element()
    }

    /// Session rows of a workspace in sidebar order: open chats that are not
    /// in the listing yet (new sessions), then the listing.
    fn session_rows(&self, entry: &WorkspaceEntry, cx: &App) -> Vec<SessionRow> {
        let mut rows = Vec::new();
        for chat in &entry.chats {
            let view = chat.view.read(cx);
            let listed = view
                .session_file()
                .is_some_and(|file| entry.sessions.iter().any(|summary| summary.path == file));
            if !listed {
                rows.push(SessionRow {
                    id: ElementId::from(("open-chat", chat.view.entity_id())),
                    title: view.title(),
                    open_chat: Some(chat.view.clone()),
                    workspace: entry.path.clone(),
                    file: view.session_file().map(Path::to_path_buf),
                    age: None,
                    hidden: false,
                });
            }
        }
        for summary in &entry.sessions {
            let open = entry
                .chats
                .iter()
                .find(|chat| chat.view.read(cx).session_file() == Some(summary.path.as_path()))
                .map(|chat| chat.view.clone());
            let title = open
                .as_ref()
                .map_or_else(|| summary.title.clone(), |view| view.read(cx).title());
            let is_active = open.is_some() && open == self.active_chat;
            rows.push(SessionRow {
                id: ElementId::Path(summary.path.as_path().into()),
                title,
                open_chat: open,
                workspace: entry.path.clone(),
                file: Some(summary.path.clone()),
                age: Some(sessions::relative_age(summary.modified)),
                // The session being worked on stays visible even if hidden.
                hidden: self.hidden_sessions.contains(&summary.path) && !is_active,
            });
        }
        rows
    }

    /// The sessions ⌘1…⌘9 select, in sidebar order.
    fn numbered_sessions(&self, cx: &App) -> Vec<SessionRow> {
        self.workspaces
            .iter()
            .filter(|entry| entry.expanded)
            .flat_map(|entry| self.session_rows(entry, cx))
            .filter(|row| !row.hidden)
            .take(9)
            .collect()
    }

    fn select_session(&mut self, action: &SelectSession, _: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self
            .numbered_sessions(cx)
            .into_iter()
            .nth(action.index.saturating_sub(1))
        else {
            return;
        };
        match (row.open_chat, row.file) {
            (Some(view), _) => self.activate(view, cx),
            (None, Some(file)) => self.open_session(&row.workspace, file, cx),
            (None, None) => {}
        }
    }

    fn render_session_row(
        &self,
        row: SessionRow,
        number: Option<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_active = row
            .open_chat
            .as_ref()
            .is_some_and(|view| self.active_chat.as_ref() == Some(view));
        let is_running = row
            .open_chat
            .as_ref()
            .is_some_and(|view| view.read(cx).is_running());
        let is_busy = row.file.as_ref().is_some_and(|file| {
            self.opening_session.as_ref() == Some(file) || self.renaming_sessions.contains(file)
        });

        let marker = if is_running || is_busy {
            Spinner::new()
                .size(IconSize::XSmall)
                .color(if is_running {
                    Color::Accent
                } else {
                    Color::Muted
                })
                .into_any_element()
        } else if row.open_chat.is_some() {
            Indicator::dot(Color::Muted).into_any_element()
        } else {
            div().into_any_element()
        };

        let renaming = self
            .renaming
            .as_ref()
            .filter(|renaming| renaming.matches(&row))
            .map(|renaming| renaming.input.clone());

        let on_click = {
            let workspace = row.workspace.clone();
            let open_chat = row.open_chat.clone();
            let file = row.file.clone();
            cx.listener(
                move |this, _: &ClickEvent, _, cx| match (&open_chat, &file) {
                    (Some(view), _) => this.activate(view.clone(), cx),
                    (None, Some(file)) => this.open_session(&workspace, file.clone(), cx),
                    (None, None) => {}
                },
            )
        };

        let title: AnyElement =
            match renaming {
                Some(input) => div()
                    .flex_1()
                    .key_context(RENAME_CONTEXT)
                    .on_action(
                        cx.listener(|this, _: &crate::chat::Submit, _, cx| this.commit_rename(cx)),
                    )
                    .on_action(cx.listener(|this, _: &CancelRename, window, cx| {
                        this.cancel_rename(window, cx)
                    }))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| this.commit_rename(cx)))
                    .child(input)
                    .into_any_element(),
                None => Label::new(row.title.clone())
                    .size(LabelSize::Small)
                    .color(if is_active {
                        Color::Default
                    } else if row.hidden {
                        Color::Placeholder
                    } else {
                        Color::Muted
                    })
                    .truncate()
                    .into_any_element(),
            };

        let menu_button = IconButton::new(
            ElementId::Name(format!("session-menu:{}", row.id).into()),
            IconName::Ellipsis,
        )
        .size(ButtonSize::Compact)
        .icon_size(IconSize::XSmall)
        .tooltip(Tooltip::text("More"))
        .on_click(cx.listener({
            let row = row.clone();
            move |this, _, window, cx| {
                this.session_menu = Some(SessionMenu {
                    row: row.clone(),
                    position: window.mouse_position(),
                });
                cx.notify();
            }
        }));

        let item = ListItem::new(row.id.clone())
            .indent_level(1)
            .toggle_state(is_active)
            .start_slot(div().w(px(12.)).flex().justify_center().child(marker))
            .child(title)
            .on_click(on_click);
        match number {
            Some(number) if self.command_held => item
                .end_slot(KeyHint::new(format!("⌘{number}")))
                .into_any_element(),
            _ => item
                .when_some(row.age.clone(), |item, age| {
                    item.end_slot(
                        Label::new(age)
                            .size(LabelSize::XSmall)
                            .color(Color::Placeholder),
                    )
                })
                .end_hover_slot(menu_button)
                .into_any_element(),
        }
    }

    fn render_session_menu(&self, menu: &SessionMenu, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let row = menu.row.clone();
        let running = row
            .open_chat
            .as_ref()
            .is_some_and(|view| view.read(cx).is_running());
        let hidden = row
            .file
            .as_ref()
            .is_some_and(|file| self.hidden_sessions.contains(file));
        let item =
            |id: &'static str, icon: IconName, label: &'static str, color: Color, enabled: bool| {
                h_flex()
                    .id(id)
                    .h(px(28.))
                    .px(px(8.))
                    .gap(px(8.))
                    .rounded(px(5.))
                    .when(enabled, |this| {
                        this.cursor_pointer()
                            .hover(|style| style.bg(colors.element_hover))
                    })
                    .when(!enabled, |this| this.opacity(0.45))
                    .child(Icon::new(icon).size(IconSize::XSmall).color(color))
                    .child(Label::new(label).size(LabelSize::Small).color(color))
            };

        let rename = item(
            "menu-rename",
            IconName::Pencil,
            "Rename",
            Color::Default,
            true,
        )
        .on_click(cx.listener({
            let row = row.clone();
            move |this, _, window, cx| {
                this.session_menu = None;
                this.start_rename(row.clone(), window, cx);
            }
        }));
        let has_file = row.file.is_some();
        let hide = item(
            "menu-hide",
            if hidden {
                IconName::Eye
            } else {
                IconName::EyeOff
            },
            if hidden {
                "Show session"
            } else {
                "Hide session"
            },
            Color::Default,
            has_file,
        )
        .on_click(cx.listener({
            let file = row.file.clone();
            move |this, _, _, cx| {
                this.session_menu = None;
                if let Some(file) = &file {
                    this.toggle_hidden(file.clone(), cx);
                }
            }
        }));
        let delete = item(
            "menu-delete",
            IconName::Trash,
            "Delete…",
            Color::Error,
            has_file && !running,
        )
        .on_click(cx.listener({
            let row = row.clone();
            move |this, _, window, cx| {
                this.session_menu = None;
                this.confirm_delete(row.clone(), running, window, cx);
            }
        }));

        deferred(
            anchored().position(menu.position).child(
                v_flex()
                    .id("session-menu")
                    .w(px(190.))
                    .p(px(4.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.elevated_surface_background)
                    .shadow_lg()
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.session_menu = None;
                        cx.notify();
                    }))
                    .child(rename)
                    .child(hide)
                    .child(div().my(px(4.)).h(px(1.)).bg(colors.border_variant))
                    .child(delete),
            ),
        )
        .with_priority(1)
        .into_any_element()
    }

    fn start_rename(&mut self, row: SessionRow, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            let mut input = TextInput::single_line("Session name", cx);
            input.set_text(row.title.clone(), cx);
            input
        });
        window.focus(&input.read(cx).focus_handle(cx));
        input.update(cx, |input, cx| input.select_all_text(cx));
        self.renaming = Some(Renaming { row, input });
        cx.notify();
    }

    fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.renaming.take().is_some() {
            self.focus_active_chat = true;
            window.refresh();
            cx.notify();
        }
    }

    /// Applies the name through pi: the open chat's process, or a short-lived
    /// pi process for a session that is not open.
    fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(renaming) = self.renaming.take() else {
            return;
        };
        self.focus_active_chat = true;
        cx.notify();
        let name = renaming.input.read(cx).text().trim().to_string();
        let row = renaming.row;
        if name.is_empty() || name == row.title {
            return;
        }
        if let Some(view) = &row.open_chat {
            view.update(cx, |chat, _| chat.rename(&name));
            return;
        }
        let Some(file) = row.file.clone() else {
            return;
        };
        let InstallationState::Ready(installation) = &self.installation else {
            self.notice = Some((
                "pi is not ready yet; try again in a moment.".to_string(),
                Color::Warning,
            ));
            return;
        };
        let installation = installation.clone();
        self.renaming_sessions.insert(file.clone());
        let workspace = row.workspace.clone();
        let task = cx.background_spawn({
            let file = file.clone();
            let workspace = workspace.clone();
            async move {
                if let Some(holder) = sessions::foreign_lease_holder(&file) {
                    return Err(format!(
                        "This session is open in {holder}. Rename it there or close it first."
                    ));
                }
                pi::rename_session(installation, workspace, file, name).await
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.renaming_sessions.remove(&file);
                if let Err(error) = result {
                    this.notice = Some((
                        format!("Could not rename the session: {error}"),
                        Color::Warning,
                    ));
                }
                this.load_sessions(&workspace, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn toggle_hidden(&mut self, file: PathBuf, cx: &mut Context<Self>) {
        if !self.hidden_sessions.remove(&file) {
            self.hidden_sessions.insert(file);
        }
        self.save(cx);
        cx.notify();
    }

    fn confirm_delete(
        &mut self,
        row: SessionRow,
        running: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if running || row.file.is_none() {
            return;
        }
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            &format!("Delete \"{}\"?", sessions::one_line(&row.title, 60)),
            Some("The session file moves to the Trash, so it can still be restored from there."),
            &["Delete", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await.ok() != Some(0) {
                return;
            }
            this.update(cx, |this, cx| this.delete_session(row, cx))
                .ok();
        })
        .detach();
    }

    fn delete_session(&mut self, row: SessionRow, cx: &mut Context<Self>) {
        let Some(file) = row.file.clone() else {
            return;
        };
        if let Some(holder) = sessions::foreign_lease_holder(&file) {
            self.notice = Some((
                format!("This session is open in {holder}. Close it there first."),
                Color::Warning,
            ));
            cx.notify();
            return;
        }
        // Close our pi process for it first (dropping the chat closes stdin).
        if let Some(view) = &row.open_chat {
            if self.active_chat.as_ref() == Some(view) {
                self.active_chat = None;
            }
            if let Some(entry) = self.entry_mut(&row.workspace) {
                entry.chats.retain(|chat| &chat.view != view);
            }
        }
        match platform::move_to_trash(&file) {
            Ok(()) => {
                self.hidden_sessions.remove(&file);
                self.save(cx);
            }
            Err(error) => {
                self.notice = Some((
                    format!("Could not delete the session: {error}"),
                    Color::Warning,
                ));
            }
        }
        self.load_sessions(&row.workspace, cx);
        cx.notify();
    }

    fn render_installation_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors.clone();
        let footer = v_flex()
            .flex_none()
            .gap(px(6.))
            .px(px(12.))
            .py(px(8.))
            .border_t_1()
            .border_color(colors.border_variant);
        match &self.installation {
            InstallationState::Checking => footer.child(
                h_flex()
                    .gap(px(6.))
                    .child(Spinner::new().size(IconSize::XSmall))
                    .child(LoadingLabel::new("Checking pi…").size(LabelSize::Small)),
            ),
            InstallationState::Ready(installation) => {
                let untested = installation.is_untested();
                footer.child(
                    h_flex()
                        .id("pi-version")
                        .gap(px(6.))
                        .child(Indicator::dot(if untested {
                            Color::Warning
                        } else {
                            Color::Success
                        }))
                        .child(
                            Label::new(if untested {
                                format!("pi {} · not tested yet", installation.version)
                            } else {
                                format!("pi {}", installation.version)
                            })
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                        )
                        .when(untested, |this| {
                            this.tooltip(Tooltip::text(format!(
                                "This app was tested with pi up to {}.{}.x. It should work; check for app updates.",
                                pi::TESTED_PI.major,
                                pi::TESTED_PI.minor
                            )))
                        }),
                )
            }
            InstallationState::Failed(error) => {
                let (color, label) = match error {
                    pi::CheckError::NotFound => (Color::Error, "pi not installed".to_string()),
                    pi::CheckError::VersionUnreadable { .. } => {
                        (Color::Error, "pi did not start".to_string())
                    }
                    pi::CheckError::Unsupported { installation }
                        if installation.version < pi::MIN_PI =>
                    {
                        (
                            Color::Warning,
                            format!("pi {} · update needed", installation.version),
                        )
                    }
                    pi::CheckError::Unsupported { installation } => (
                        Color::Accent,
                        format!("pi {} · not supported yet", installation.version),
                    ),
                };
                footer.child(
                    h_flex()
                        .gap(px(6.))
                        .child(Indicator::dot(color))
                        .child(Label::new(label).size(LabelSize::Small).color(Color::Muted)),
                )
            }
        }
    }

    /// Shown instead of the main area while pi cannot be used: what is wrong
    /// and the exact next step, with commands ready to copy.
    fn render_pi_setup(&self, error: &pi::CheckError, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let (icon, icon_color, title, text): (IconName, Color, String, String) = match error {
            pi::CheckError::NotFound => (
                IconName::Terminal,
                Color::Muted,
                "Install pi to get started".to_string(),
                "Pi GPUI is a window for the pi coding agent. Install pi once; your models, extensions and sessions are shared with the terminal.".to_string(),
            ),
            pi::CheckError::Unsupported { installation } if installation.version < pi::MIN_PI => (
                IconName::Warning,
                Color::Warning,
                "Update pi".to_string(),
                format!(
                    "pi {} is installed. This app needs pi {} or newer.",
                    installation.version,
                    pi::MIN_PI
                ),
            ),
            pi::CheckError::Unsupported { installation } => (
                IconName::Refresh,
                Color::Accent,
                format!("pi {}.{} is newer than this app", installation.version.major, installation.version.minor),
                format!(
                    "This version of Pi GPUI supports pi {}.x. A newer app version may support pi {}.",
                    pi::MIN_PI.major,
                    installation.version.major
                ),
            ),
            pi::CheckError::VersionUnreadable { .. } => (
                IconName::Warning,
                Color::Error,
                "Could not start pi".to_string(),
                "pi was found, but `pi --version` did not answer.".to_string(),
            ),
        };

        let command = |id: &'static str, label: &'static str, command: String| {
            let copy = command.clone();
            v_flex()
                .gap(px(6.))
                .child(
                    Label::new(label)
                        .size(LabelSize::XSmall)
                        .color(Color::Placeholder),
                )
                .child(
                    h_flex()
                        .gap(px(8.))
                        .pl(px(12.))
                        .pr(px(6.))
                        .py(px(6.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border_variant)
                        .bg(colors.code_block_background)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .font_family("Menlo")
                                .text_size(px(12.5))
                                .text_color(colors.text)
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(command),
                        )
                        .child(Button::new(id, "Copy").style(ButtonStyle::Filled).on_click(
                            move |_, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone()))
                            },
                        )),
                )
                .into_any_element()
        };
        let hint = |text: String| {
            Label::new(text)
                .size(LabelSize::Small)
                .color(Color::Placeholder)
                .into_any_element()
        };
        let check_again = Button::new("setup-check-again", "Check again")
            .icon(IconName::Refresh)
            .style(ButtonStyle::Tinted(TintColor::Accent))
            .on_click(cx.listener(|this, _, _, cx| this.check_installation(cx)));
        let auto_check = h_flex()
            .gap(px(6.))
            .child(
                Icon::new(IconName::Refresh)
                    .size(IconSize::XSmall)
                    .color(Color::Placeholder),
            )
            .child(hint(
                "Also checks again when you return to the app".to_string(),
            ));

        let mut body: Vec<AnyElement> = Vec::new();
        let mut actions = h_flex().gap(px(12.)).mt(px(8.));
        let details: Option<String>;
        match error {
            pi::CheckError::NotFound => {
                body.push(command(
                    "copy-installer",
                    "Installer (macOS)",
                    "curl -fsSL https://pi.dev/install.sh | sh".to_string(),
                ));
                body.push(command(
                    "copy-npm",
                    "Or with npm (Node.js 22.19+)",
                    "npm install -g --ignore-scripts @earendil-works/pi-coding-agent".to_string(),
                ));
                actions = actions.child(check_again).child(
                    Button::new("setup-docs", "pi documentation")
                        .on_click(|_, _, cx| cx.open_url("https://pi.dev")),
                );
                details = Some(
                    "Installed somewhere unusual? Set PI_EXECUTABLE to its path and reopen the app."
                        .to_string(),
                );
            }
            pi::CheckError::Unsupported { installation } if installation.version < pi::MIN_PI => {
                body.push(command(
                    "copy-update",
                    "Run in a terminal",
                    "pi update".to_string(),
                ));
                body.push(hint(
                    "Installed with npm? Run the npm install command again instead.".to_string(),
                ));
                actions = actions.child(check_again);
                details = Some(format!("Found {}", installation.executable.display()));
            }
            pi::CheckError::Unsupported { installation } => {
                actions = actions
                    .child(
                        Button::new("setup-app-update", "Check for Updates")
                            .icon(IconName::Refresh)
                            .style(ButtonStyle::Tinted(TintColor::Accent))
                            .on_click(|_, _, cx| {
                                AutoUpdater::global(cx)
                                    .update(cx, |updater, cx| updater.check(true, cx))
                            }),
                    )
                    .child(
                        Button::new("setup-check-pi", "Check pi again")
                            .on_click(cx.listener(|this, _, _, cx| this.check_installation(cx))),
                    );
                details = Some(format!(
                    "Found pi {} at {}",
                    installation.version,
                    installation.executable.display()
                ));
            }
            pi::CheckError::VersionUnreadable {
                executable,
                details: error,
            } => {
                body.push(command("copy-error", "Error", error.clone()));
                body.push(hint(
                    "pi needs Node.js 22.19 or newer on the same PATH.".to_string(),
                ));
                actions = actions.child(check_again);
                details = Some(format!("Found {}", executable.display()));
            }
        }
        let shows_auto_check = !matches!(
            error,
            pi::CheckError::Unsupported { installation } if installation.version >= pi::MIN_PI
        );

        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .child(
                v_flex()
                    .w(px(520.))
                    .gap(px(14.))
                    .child(
                        div()
                            .size(px(44.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(12.))
                            .border_1()
                            .border_color(colors.border_variant)
                            .bg(colors.elevated_surface_background)
                            .child(Icon::new(icon).size(IconSize::Medium).color(icon_color)),
                    )
                    .child(
                        v_flex()
                            .gap(px(6.))
                            .child(
                                div()
                                    .text_size(px(20.))
                                    .line_height(px(28.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .child(Label::new(text).color(Color::Muted)),
                    )
                    .children(body)
                    .child(actions)
                    .when(shows_auto_check, |this| this.child(auto_check))
                    .when_some(details, |this, details| {
                        this.child(
                            div()
                                .pt(px(12.))
                                .border_t_1()
                                .border_color(colors.border_variant)
                                .child(hint(details)),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_title_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors.clone();
        let is_active = window.is_window_active();
        let text_color = if is_active {
            Color::Default
        } else {
            Color::Muted
        };

        let (breadcrumb, status) = match &self.active_chat {
            Some(chat) => {
                let chat = chat.read(cx);
                let workspace = chat.workspace();
                let workspace_name = workspace
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| workspace.display().to_string());
                let breadcrumb = h_flex()
                    .id("title-breadcrumb")
                    .gap(px(6.))
                    .min_w(px(0.))
                    .tooltip(Tooltip::text(sessions::display_path(workspace)))
                    .child(
                        Icon::new(IconName::Folder)
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(
                        div().flex_none().child(
                            Label::new(workspace_name)
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                    )
                    .child(
                        Icon::new(IconName::ChevronRight)
                            .size(IconSize::XSmall)
                            .color(Color::Placeholder),
                    )
                    .child(
                        div().min_w(px(0.)).child(
                            Label::new(chat.title())
                                .size(LabelSize::Small)
                                .weight(FontWeight::MEDIUM)
                                .color(text_color)
                                .truncate(),
                        ),
                    )
                    .into_any_element();
                let status = match chat.status() {
                    ChatStatus::Busy => h_flex()
                        .gap(px(6.))
                        .child(Spinner::new().size(IconSize::XSmall).color(Color::Accent))
                        .child(LoadingLabel::new("Working").size(LabelSize::Small)),
                    ChatStatus::Ready => h_flex()
                        .gap(px(6.))
                        .child(Indicator::dot(Color::Success))
                        .child(
                            Label::new("Ready")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                    ChatStatus::Stopped => h_flex()
                        .gap(px(6.))
                        .child(Indicator::dot(Color::Error))
                        .child(
                            Label::new("Stopped")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                };
                (breadcrumb, Some(status))
            }
            None => (
                Label::new("Pi GPUI")
                    .size(LabelSize::Small)
                    .color(Color::Placeholder)
                    .into_any_element(),
                None,
            ),
        };

        let plugin_statuses: Vec<AnyElement> = self
            .plugins
            .read(cx)
            .statuses()
            .iter()
            .map(|status| {
                let indicator = if status.busy {
                    Spinner::new()
                        .size(IconSize::XSmall)
                        .color(status.color)
                        .into_any_element()
                } else {
                    Indicator::dot(status.color).into_any_element()
                };
                let label = if status.busy {
                    LoadingLabel::new(status.text.clone())
                        .size(LabelSize::Small)
                        .color(status.color)
                        .into_any_element()
                } else {
                    Label::new(status.text.clone())
                        .size(LabelSize::Small)
                        .color(Color::Muted)
                        .into_any_element()
                };
                h_flex()
                    .flex_none()
                    .gap(px(6.))
                    .child(indicator)
                    .child(label)
                    .into_any_element()
            })
            .collect();

        h_flex()
            .id("title-bar")
            .flex_none()
            .w_full()
            .h(px(TITLE_BAR_HEIGHT))
            .bg(if is_active {
                colors.title_bar_background
            } else {
                colors.title_bar_inactive_background
            })
            .border_b_1()
            .border_color(colors.border_variant)
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, _| {
                    this.title_bar_drag_pending = event.click_count == 1;
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.title_bar_drag_pending = false),
            )
            .on_mouse_move(cx.listener(|this, _: &MouseMoveEvent, window, _| {
                if std::mem::take(&mut this.title_bar_drag_pending) {
                    platform::start_window_drag(window);
                }
            }))
            // Double-click follows the user's macOS setting (zoom or minimize).
            .on_click(|event, window, _| {
                if event.click_count() >= 2 {
                    window.titlebar_double_click();
                }
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    // The breadcrumb starts right after the traffic lights,
                    // which are hidden in full screen.
                    .pl(px(if window.is_fullscreen() {
                        12.
                    } else {
                        TRAFFIC_LIGHT_PADDING
                    }))
                    .pr(px(16.))
                    .gap(px(12.))
                    .child(div().flex_1().min_w(px(0.)).flex().child(breadcrumb))
                    .children(plugin_statuses)
                    .children(status)
                    .children(self.render_update_chip(cx))
                    .when_some(self.active_chat.clone(), |this, chat| {
                        this.child(
                            IconButton::new("session-tree", IconName::ListTree)
                                .tooltip(Tooltip::with_keys("Session tree", "⌘⇧T"))
                                .on_click(move |_, window, cx| {
                                    chat.update(cx, |chat, cx| chat.toggle_tree(window, cx))
                                }),
                        )
                    })
                    .child({
                        let expanded = cx.global::<DetailsExpanded>().expanded;
                        IconButton::new(
                            "toggle-details",
                            if expanded {
                                IconName::Eye
                            } else {
                                IconName::EyeOff
                            },
                        )
                        .tooltip(Tooltip::with_keys(
                            if expanded {
                                "Collapse step details"
                            } else {
                                "Expand step details"
                            },
                            "⌘⇧E",
                        ))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_details(&ToggleDetails, window, cx)
                        }))
                    }),
            )
    }

    /// Zed-style: quiet while downloading, a button once the update is ready.
    fn render_update_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        match AutoUpdater::global(cx).read(cx).status().clone() {
            UpdateStatus::Downloading(version) => Some(
                h_flex()
                    .flex_none()
                    .gap(px(6.))
                    .child(Spinner::new().size(IconSize::XSmall).color(Color::Muted))
                    .child(
                        Label::new(format!("Downloading {version}"))
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .into_any_element(),
            ),
            UpdateStatus::Ready(version) => Some(
                Button::new("restart-to-update", "Restart to update")
                    .icon(IconName::Refresh)
                    .style(ButtonStyle::Tinted(TintColor::Accent))
                    .tooltip(Tooltip::text(format!("Pi GPUI {version} is downloaded")))
                    .on_click(cx.listener(|this, _, window, cx| this.restart_to_update(window, cx)))
                    .into_any_element(),
            ),
            _ => None,
        }
    }

    fn render_placeholder(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let no_workspaces = self.workspaces.is_empty();
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(px(10.))
            .child(
                Icon::new(if no_workspaces {
                    IconName::Folder
                } else {
                    IconName::Message
                })
                .size(IconSize::Large)
                .color(Color::Placeholder),
            )
            .child(
                Label::new(if no_workspaces {
                    "Add a project folder to get started"
                } else {
                    "Pick a session on the left or start a new one"
                })
                .color(Color::Muted),
            )
            .child(if no_workspaces {
                h_flex()
                    .gap(px(8.))
                    .child(
                        Button::new("placeholder-add-workspace", "Add project")
                            .icon(IconName::Plus)
                            .style(ButtonStyle::Filled)
                            .size(ButtonSize::Medium)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.prompt_add_workspace(&AddWorkspace, window, cx)
                            })),
                    )
                    .child(KeyHint::new("⌘O"))
            } else {
                h_flex()
                    .gap(px(6.))
                    .child(
                        Label::new("New session")
                            .size(LabelSize::Small)
                            .color(Color::Placeholder),
                    )
                    .child(KeyHint::new("⌘N"))
            })
    }
}

impl Focusable for HubView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for HubView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.focus_active_chat) {
            match &self.active_chat {
                Some(chat) => window.focus(&chat.read(cx).composer_focus_handle(cx)),
                // Keep hub shortcuts (⌘N, ⌘1…) working without a chat.
                None => window.focus(&self.focus_handle),
            }
        }
        let colors = cx.theme().colors.clone();
        self.animation.set_active(self.sidebar_is_animating(cx), cx);

        let notice = self.notice.clone().map(|(message, color)| {
            let (icon, background, border) = match color {
                Color::Error => (IconName::Warning, colors.error_background, colors.error),
                Color::Warning => (IconName::Warning, colors.warning_background, colors.warning),
                _ => (IconName::Info, colors.info_background, colors.info),
            };
            h_flex()
                .flex_none()
                .gap(px(8.))
                .px(px(12.))
                .py(px(6.))
                .bg(background)
                .border_b_1()
                .border_color(border.opacity(0.3))
                .child(Icon::new(icon).size(IconSize::XSmall).color(color))
                .child(
                    div()
                        .flex_1()
                        .child(Label::new(message).size(LabelSize::Small)),
                )
                .child(
                    IconButton::new("dismiss-notice", IconName::Close)
                        .icon_size(IconSize::XSmall)
                        .tooltip(Tooltip::text("Dismiss"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.notice = None;
                            cx.notify();
                        })),
                )
        });

        // Cached: sidebar ticks and hub updates reuse the chat's last frame
        // unless the chat itself was notified.
        let main = match &self.active_chat {
            Some(chat) => div()
                .flex_1()
                .min_h(px(0.))
                .child(AnyView::from(chat.clone()).cached(StyleRefinement::default().size_full()))
                .into_any_element(),
            None => match &self.installation {
                InstallationState::Failed(error) => self.render_pi_setup(error, cx),
                _ => self.render_placeholder(cx).into_any_element(),
            },
        };

        v_flex()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::prompt_add_workspace))
            .on_action(cx.listener(Self::new_chat))
            .on_action(cx.listener(Self::run_plugin_command))
            .on_action(cx.listener(Self::toggle_details))
            .on_action(cx.listener(Self::select_session))
            .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, _, cx| {
                let held = event.modifiers.platform
                    && !event.modifiers.shift
                    && !event.modifiers.alt
                    && !event.modifiers.control;
                if held != this.command_held {
                    this.command_held = held;
                    cx.notify();
                }
            }))
            .size_full()
            .bg(colors.editor_background)
            .text_color(colors.text)
            .text_size(px(14.))
            .line_height(px(21.))
            .child(self.render_title_bar(window, cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.))
                    .child(self.render_sidebar(cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .h_full()
                            .children(notice)
                            .child(main),
                    ),
            )
            .children(
                self.session_menu
                    .as_ref()
                    .map(|menu| self.render_session_menu(menu, cx)),
            )
    }
}

/// `~/Library/Application Support/pi-gpui/state.json` on macOS. App data
/// lives apart from pi's own files, as the principles require.
fn state_file_path() -> Option<PathBuf> {
    let home = sessions::home_directory()?;
    let directory = if cfg!(target_os = "macos") {
        home.join("Library/Application Support/pi-gpui")
    } else {
        home.join(".config/pi-gpui")
    };
    Some(directory.join("state.json"))
}

fn load_state(path: &Path) -> Vec<(PathBuf, bool)> {
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    let state: Value = match serde_json::from_slice(&bytes) {
        Ok(state) => state,
        Err(error) => {
            eprintln!("pi-gpui: ignoring unreadable {}: {error}", path.display());
            return Vec::new();
        }
    };
    state["workspaces"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|workspace| {
            let path = PathBuf::from(workspace["path"].as_str()?);
            Some((path, workspace["expanded"].as_bool().unwrap_or(true)))
        })
        .collect()
}

fn load_hidden_sessions(path: &Path) -> HashSet<PathBuf> {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|state| {
            state["hiddenSessions"].as_array().map(|paths| {
                paths
                    .iter()
                    .filter_map(|path| path.as_str().map(PathBuf::from))
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// Step details start collapsed unless the user expanded them last time.
fn load_expand_details(path: &Path) -> bool {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|state| state["expandDetails"].as_bool())
        .unwrap_or(false)
}

fn write_atomically(path: &Path, value: &Value) -> Result<(), String> {
    let directory = path.parent().ok_or("no parent folder")?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    let contents = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    fs::write(&temporary, contents).map_err(|error| error.to_string())?;
    fs::rename(&temporary, path).map_err(|error| error.to_string())
}
