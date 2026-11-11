//! Root view: side panels (projects and sessions, files, changes)
//! around the active chat.
//!
//! Every opened session owns its own pi process. Switching between them never
//! stops work; closing the app closes stdin of each process.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, AnyView, App, ClickEvent, Context, ElementId, Entity, FocusHandle, Focusable,
    FontWeight, KeyBinding, ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    Pixels, Point, SharedString, StyleRefinement, Subscription, Task, Window, WindowControlArea,
    actions, anchored, deferred, div, point, px,
};
use serde_json::{Value, json};

use crate::chat::{ChatEvent, ChatStatus, ChatView, DetailsExpanded};
use crate::diff::DiffPanel;
use crate::dock::{DockSide, Docks, PanelKind};
use crate::file_tree::{FilesEvent, FilesPanel};
use crate::pi::{self, Installation};
use crate::pi_updates;
use crate::platform;
use crate::plugins::{
    CommandContext, PluginEvent, PluginRegistry, PluginReply, PluginRequest, RunPluginCommand,
};
use crate::sessions::{self, SessionSummary};
use crate::settings::{self, Settings};
use crate::text_input::TextInput;
use crate::ui::AnimationTicker;
use crate::ui::prelude::*;
use crate::updater::{AutoUpdater, UpdateStatus};

actions!(
    hub,
    [
        AddWorkspace,
        NewChat,
        CloseChat,
        ToggleDetails,
        CancelRename,
        OpenSettings,
        Minimize,
        Zoom,
        ToggleFullScreen,
        ToggleLeftDock,
        ToggleRightDock,
        ToggleFiles,
        ToggleChanges,
        ReloadSessions
    ]
);

/// ⌘1…⌘9: the n-th open session tab (1-based).
#[derive(Clone, PartialEq, Debug, gpui::Action)]
#[action(namespace = hub, no_json)]
pub struct SelectSession {
    pub index: usize,
}

const KEY_CONTEXT: &str = "Hub";
const RENAME_CONTEXT: &str = "SessionRename";
const SETTINGS_FIELD_CONTEXT: &str = "SettingsField";
/// Height of the strip of open session tabs above the chat.
const TAB_BAR_HEIGHT: f32 = 34.;
const SESSIONS_PER_WORKSPACE: usize = 20;
/// Our own title bar replaces the native one (see `main.rs`). The traffic
/// lights are placed at (12, 11) so they sit centered in this height.
pub const TITLE_BAR_HEIGHT: f32 = 38.;
/// Room for the macOS traffic lights, as in Zed's `TRAFFIC_LIGHT_PADDING`.
const TRAFFIC_LIGHT_PADDING: f32 = 84.;

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(
            &crate::keys::binding("cmd-o"),
            AddWorkspace,
            Some(KEY_CONTEXT),
        ),
        KeyBinding::new(&crate::keys::binding("cmd-n"), NewChat, Some(KEY_CONTEXT)),
        KeyBinding::new(&crate::keys::binding("cmd-w"), CloseChat, Some(KEY_CONTEXT)),
        KeyBinding::new(
            &crate::keys::binding("cmd-shift-e"),
            ToggleDetails,
            Some(KEY_CONTEXT),
        ),
        KeyBinding::new("escape", CancelRename, Some(RENAME_CONTEXT)),
        KeyBinding::new(
            &crate::keys::binding("cmd-,"),
            OpenSettings,
            Some(KEY_CONTEXT),
        ),
        KeyBinding::new(
            &crate::keys::binding("cmd-b"),
            ToggleLeftDock,
            Some(KEY_CONTEXT),
        ),
        KeyBinding::new(
            &crate::keys::binding("cmd-r"),
            ToggleRightDock,
            Some(KEY_CONTEXT),
        ),
        KeyBinding::new(
            &crate::keys::binding("cmd-shift-r"),
            ReloadSessions,
            Some(KEY_CONTEXT),
        ),
    ]);
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("cmd-m", Minimize, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, Some(KEY_CONTEXT)),
    ]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([KeyBinding::new("f11", ToggleFullScreen, Some(KEY_CONTEXT))]);
    cx.bind_keys((1..=9).map(|index| {
        KeyBinding::new(
            &crate::keys::binding(&format!("cmd-{index}")),
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum UpdatePopover {
    Pi,
    Extensions,
}

/// Text fields of the open Settings page (the other controls need no state).
struct SettingsPage {
    ui_font: Entity<TextInput>,
    code_font: Entity<TextInput>,
}

struct OpenChat {
    view: Entity<ChatView>,
    _subscription: Subscription,
}

/// A project being dragged in the sidebar to reorder it; also its drag preview.
#[derive(Clone)]
struct DraggedWorkspace {
    path: PathBuf,
    name: SharedString,
    index: usize,
}

impl Render for DraggedWorkspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors.clone();
        h_flex()
            .gap(px(6.))
            .h(px(26.))
            .px(px(8.))
            .rounded(px(5.))
            .bg(colors.elevated_surface_background)
            .border_1()
            .border_color(colors.border)
            .shadow_md()
            .child(
                Icon::new(IconName::Folder)
                    .size(IconSize::Small)
                    .color(Color::Muted),
            )
            .child(
                Label::new(self.name.clone())
                    .size(LabelSize::Small)
                    .weight(FontWeight::MEDIUM),
            )
    }
}

/// A session tab being dragged to reorder it; also its drag preview.
#[derive(Clone)]
struct DraggedTab {
    id: gpui::EntityId,
    title: SharedString,
    index: usize,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors.clone();
        h_flex()
            .h(px(26.))
            .max_w(px(220.))
            .px(px(10.))
            .rounded(px(5.))
            .bg(colors.elevated_surface_background)
            .border_1()
            .border_color(colors.border)
            .shadow_md()
            .child(
                Label::new(self.title.clone())
                    .size(LabelSize::Small)
                    .truncate(),
            )
    }
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
    /// Workspace from the command line or an open link, and the session to
    /// open in it (a new one when absent), started once pi is verified.
    pending_open: Option<(PathBuf, Option<PathBuf>)>,
    focus_active_chat: bool,
    /// Mouse went down in the title bar; the next move starts a window drag.
    title_bar_drag_pending: bool,
    /// Hidden from the sidebar by the user (app state, pi's files untouched).
    hidden_sessions: HashSet<PathBuf>,
    /// Workspaces whose hidden sessions are shown for now.
    show_hidden: HashSet<PathBuf>,
    session_menu: Option<SessionMenu>,
    /// Linux has no menu bar: Settings, updates and Quit live in this menu,
    /// opened from the title bar. Holds where it opens.
    app_menu: Option<Point<Pixels>>,
    renaming: Option<Renaming>,
    /// Sessions being renamed through a short-lived pi process.
    renaming_sessions: HashSet<PathBuf>,
    /// ⌘ is held: the tabs show ⌘1…⌘9.
    command_held: bool,
    /// The Settings page replaces the main area while open.
    settings_page: Option<SettingsPage>,
    docks: Docks,
    diff_panel: Entity<DiffPanel>,
    files_panel: Entity<FilesPanel>,
    /// Right-click menu of a side panel.
    panel_menu: Option<PanelMenu>,
    /// A dock edge is being dragged.
    resizing_dock: Option<DockSide>,
    /// The chat the panels last followed; they catch up in the next render.
    panels_follow: Option<gpui::EntityId>,
    /// Tab order set by dragging; open sessions missing here go last.
    tab_order: Vec<gpui::EntityId>,

    /// Available pi and extension updates (applied only on request).
    pi_updates: pi_updates::Updates,
    update_popover: Option<UpdatePopover>,
    update_popover_closed_at: Option<std::time::Instant>,
    /// What `pi update` is doing right now.
    pi_updating: Option<String>,
    _pi_updates_task: Task<()>,
    focus_handle: FocusHandle,
    plugins: Entity<PluginRegistry>,
    _plugin_subscriptions: Vec<Subscription>,
    _check_task: Task<()>,
    _prompt_task: Task<()>,
    _open_task: Task<()>,
    /// Checking pi before every open session gets a fresh pi.
    reloading_sessions: bool,
    _reload_task: Task<()>,
    _save_task: Task<()>,
    animation: AnimationTicker,
}

impl HubView {
    pub fn new(
        initial_workspace: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // "theme": "system" follows the system light and dark mode.
        cx.observe_window_appearance(window, |_, window, cx| {
            settings::apply_theme_for(window.appearance(), cx)
        })
        .detach();
        // The page shows the current values; refresh fields not being edited.
        cx.observe_global::<Settings>(|this: &mut Self, cx| {
            if let Some(page) = &this.settings_page {
                let settings = Settings::get(cx).clone();
                for (input, value) in [
                    (page.ui_font.clone(), settings.ui_font_family),
                    (page.code_font.clone(), settings.buffer_font_family),
                ] {
                    input.update(cx, |input, cx| {
                        if input.text() != value {
                            input.set_text(value, cx);
                        }
                    });
                }
            }
            cx.notify();
        })
        .detach();
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
        let docks = state_path.as_deref().map(load_docks).unwrap_or_default();
        let diff_panel = cx.new(|_| DiffPanel::new());
        let files_panel = cx.new(FilesPanel::new);
        cx.subscribe(
            &files_panel,
            |this: &mut Self, _, event: &FilesEvent, cx| {
                let FilesEvent::InsertPath(path) = event;
                if let Some(chat) = &this.active_chat {
                    let edit = PluginEvent::ComposerInsert {
                        text: format!("{path} "),
                        replace_last: false,
                        space_before: true,
                    };
                    chat.update(cx, |chat, cx| chat.apply_plugin_edit(&edit, cx));
                    this.focus_active_chat = true;
                    cx.notify();
                }
            },
        )
        .detach();
        // Changes made in an editor show up when coming back to the app.
        cx.observe_window_activation(window, |this: &mut Self, window, cx| {
            if window.is_window_active() {
                this.refresh_visible_panels(cx);
            }
        })
        .detach();
        let mut this = Self {
            workspaces: Vec::new(),
            active_chat: None,
            installation: InstallationState::Checking,
            notice: None,
            opening_session: None,
            state_path,
            pending_open: None,
            focus_active_chat: false,
            title_bar_drag_pending: false,
            hidden_sessions: HashSet::new(),
            show_hidden: HashSet::new(),
            session_menu: None,
            app_menu: None,
            renaming: None,
            renaming_sessions: HashSet::new(),
            command_held: false,
            settings_page: None,
            docks,
            diff_panel,
            files_panel,
            panel_menu: None,
            resizing_dock: None,
            panels_follow: None,
            tab_order: Vec::new(),

            pi_updates: pi_updates::Updates::default(),
            update_popover: None,
            update_popover_closed_at: None,
            pi_updating: None,
            _pi_updates_task: Task::ready(()),
            focus_handle: cx.focus_handle(),
            plugins,
            _plugin_subscriptions: plugin_subscriptions,
            _check_task: Task::ready(()),
            _prompt_task: Task::ready(()),
            _open_task: Task::ready(()),
            reloading_sessions: false,
            _reload_task: Task::ready(()),
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
            this.pending_open = Some((path, None));
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

    fn open_chats(&self) -> Vec<Entity<ChatView>> {
        self.workspaces
            .iter()
            .flat_map(|entry| entry.chats.iter().map(|chat| chat.view.clone()))
            .collect()
    }

    /// ⌘⇧R: every open session gets a fresh pi, so updated pi, extensions,
    /// skills and settings take effect without reopening each one.
    fn reload_sessions(&mut self, _: &ReloadSessions, window: &mut Window, cx: &mut Context<Self>) {
        let chats = self.open_chats();
        if chats.is_empty() || self.reloading_sessions {
            return;
        }
        let running = chats
            .iter()
            .filter(|chat| chat.read(cx).is_running())
            .count();
        if running == 0 {
            self.restart_sessions(cx);
            return;
        }
        let question = match running {
            1 => "pi is working in a session. Reload anyway?".to_string(),
            count => format!("pi is working in {count} sessions. Reload anyway?"),
        };
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            &question,
            Some("Running work stops; the sessions themselves are saved."),
            &["Reload", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await.ok() == Some(0) {
                this.update(cx, |this, cx| this.restart_sessions(cx)).ok();
            }
        })
        .detach();
    }

    /// Checks pi again (it may have been updated), then restarts each open
    /// session with it.
    fn restart_sessions(&mut self, cx: &mut Context<Self>) {
        self.reloading_sessions = true;
        let check = cx.background_spawn(async { pi::check_installation() });
        self._reload_task = cx.spawn(async move |this, cx| {
            let result = check.await;
            this.update(cx, |this, cx| {
                this.reloading_sessions = false;
                match result {
                    Ok(installation) => {
                        eprintln!(
                            "pi-gpui: reloading open sessions with pi {}",
                            installation.version
                        );
                        let version = installation.version;
                        AutoUpdater::global(cx)
                            .update(cx, |updater, _| updater.set_pi_version(version));
                        this.installation = InstallationState::Ready(installation.clone());
                        let chats = this.open_chats();
                        for chat in &chats {
                            chat.update(cx, |chat, cx| chat.restart(installation.clone(), cx));
                        }
                        this.notice = Some((
                            match chats.len() {
                                1 => "Reloaded 1 session.".to_string(),
                                count => format!("Reloaded {count} sessions."),
                            },
                            Color::Default,
                        ));
                        this.check_pi_updates(cx);
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
            PluginEvent::Request(request) => self.handle_plugin_request(request.clone(), cx),
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
                        this.check_pi_updates(cx);
                        match this.pending_open.take() {
                            Some((path, Some(session))) => this.open_session(&path, session, cx),
                            Some((path, None)) => this.start_chat(&path, None, cx),
                            None => {}
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

    /// Handles a `pi-gpui://open` link (see `open_url.rs`): adds the project
    /// and opens the session, or a new one when the link names none.
    pub fn open_url(&mut self, url: &str, cx: &mut Context<Self>) {
        let request = crate::open_url::parse(url);
        let Some(workspace) = request
            .as_ref()
            .and_then(|request| PathBuf::from(&request.workspace).canonicalize().ok())
            .filter(|path| path.is_dir())
        else {
            self.notice = Some((
                format!("Cannot open {url}: no such project folder."),
                Color::Warning,
            ));
            cx.notify();
            return;
        };
        let session = request
            .and_then(|request| request.session)
            .map(PathBuf::from)
            .filter(|path| path.is_file());
        self.add_workspace(workspace.clone(), cx);
        self.save(cx);
        if matches!(self.installation, InstallationState::Checking) {
            self.pending_open = Some((workspace, session));
            cx.notify();
            return;
        }
        match session {
            Some(session) => self.open_session(&workspace, session, cx),
            None => self.start_chat(&workspace, None, cx),
        }
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
        // On macOS our own picker reuses one panel; gpui's builds a new one each
        // time and blocks the main thread while doing it.
        #[cfg(target_os = "macos")]
        let picker = platform::pick_folders("Add");
        #[cfg(not(target_os = "macos"))]
        let picker = {
            let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
                files: false,
                directories: true,
                multiple: true,
                prompt: Some("Add".into()),
            });
            async move {
                match paths.await {
                    Ok(Ok(paths)) => Ok(paths),
                    Ok(Err(error)) => Err(error.to_string()),
                    Err(_) => Ok(None),
                }
            }
        };
        self._prompt_task = cx.spawn(async move |this, cx| {
            let selection = picker.await;
            this.update(cx, |this, cx| {
                match selection {
                    Ok(Some(paths)) => {
                        for path in paths {
                            this.add_workspace(path, cx);
                        }
                        this.save(cx);
                    }
                    Ok(None) => {}
                    Err(error) => {
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
        for chat in &removed.chats {
            let handle = chat.view.read(cx).handle().to_string();
            self.plugins
                .update(cx, |registry, cx| registry.session_closed(&handle, cx));
        }
        if let Some(active) = &self.active_chat
            && removed.chats.iter().any(|chat| chat.view == *active)
        {
            self.active_chat = None;
        }
        self.save(cx);
        cx.notify();
    }

    /// Moves a project to `target`'s place in the sidebar, like Zed does for
    /// dragged worktrees.
    fn move_workspace(&mut self, workspace: &Path, target: usize, cx: &mut Context<Self>) {
        let Some(from) = self
            .workspaces
            .iter()
            .position(|entry| entry.path == workspace)
        else {
            return;
        };
        let target = target.min(self.workspaces.len() - 1);
        if from == target {
            return;
        }
        let entry = self.workspaces.remove(from);
        self.workspaces.insert(target, entry);
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
        self.start_chat_with(workspace, session, true, cx);
    }

    /// Starts pi on a session; `activate` shows it in the main area.
    fn start_chat_with(
        &mut self,
        workspace: &Path,
        session: Option<PathBuf>,
        activate: bool,
        cx: &mut Context<Self>,
    ) -> Option<Entity<ChatView>> {
        let installation = match &self.installation {
            InstallationState::Ready(installation) => installation.clone(),
            InstallationState::Checking => {
                self.notice = Some((
                    "pi is still being checked, try again in a moment.".to_string(),
                    Color::Warning,
                ));
                cx.notify();
                return None;
            }
            InstallationState::Failed(_) => {
                self.notice = Some((
                    "pi is not ready: see the steps in the main area.".to_string(),
                    Color::Warning,
                ));
                cx.notify();
                return None;
            }
        };
        let workspace_path = workspace.to_path_buf();
        let plugins = self.plugins.clone();
        let view = cx.new(|cx| ChatView::new(workspace_path, session, installation, plugins, cx));
        let subscription = cx.subscribe(&view, Self::on_chat_event);
        let entry = self.entry_mut(workspace)?;
        entry.expanded = true;
        entry.chats.push(OpenChat {
            view: view.clone(),
            _subscription: subscription,
        });
        self.report_session_state(&view, cx);
        if activate {
            self.activate(view.clone(), cx);
        } else {
            cx.notify();
        }
        Some(view)
    }

    /// Tells plugins that follow this session about its new summary.
    fn report_session_state(&mut self, chat: &Entity<ChatView>, cx: &mut Context<Self>) {
        let handle = chat.read(cx).handle().to_string();
        if !self.plugins.read(cx).wants_session_events(&handle) {
            return;
        }
        let info = self.session_info(chat, cx);
        self.plugins
            .update(cx, |registry, cx| registry.session_state(&handle, info, cx));
    }

    fn session_info(&self, chat: &Entity<ChatView>, cx: &App) -> Value {
        let mut info = chat.read(cx).info();
        info["active"] = json!(self.active_chat.as_ref() == Some(chat));
        info
    }

    fn chat_by_handle(&self, handle: &str, cx: &App) -> Option<Entity<ChatView>> {
        self.workspaces
            .iter()
            .flat_map(|entry| entry.chats.iter())
            .find(|chat| chat.view.read(cx).handle() == handle)
            .map(|chat| chat.view.clone())
    }

    /// `sessions.*` requests from plugins with the `sessions` permission.
    fn handle_plugin_request(&mut self, request: PluginRequest, cx: &mut Context<Self>) {
        let PluginRequest {
            reply,
            method,
            params,
        } = request;
        let plugins = self.plugins.clone();
        match method.as_str() {
            "sessions.list" => self.list_sessions_for_plugin(&params, reply, cx),
            "sessions.open" => self.open_session_for_plugin(&params, reply, cx),
            _ => {
                let handle = params["session"].as_str().unwrap_or_default();
                let Some(chat) = self.chat_by_handle(handle, cx) else {
                    let error = format!("no open session \"{handle}\"; see sessions.list");
                    reply.finish(&plugins, Err(error), cx);
                    return;
                };
                match method.as_str() {
                    "sessions.get" => {
                        let info = self.session_info(&chat, cx);
                        reply.finish(&plugins, Ok(info), cx);
                    }
                    "sessions.focus" => {
                        self.activate(chat.clone(), cx);
                        let info = self.session_info(&chat, cx);
                        reply.finish(&plugins, Ok(info), cx);
                    }
                    "sessions.prompt" => {
                        chat.update(cx, |chat, cx| chat.plugin_prompt(&params, reply, cx))
                    }
                    "sessions.abort" => chat.update(cx, |chat, cx| chat.plugin_abort(reply, cx)),
                    "sessions.answer" => {
                        chat.update(cx, |chat, cx| chat.plugin_answer(&params, reply, cx))
                    }
                    "sessions.rpc" => {
                        chat.update(cx, |chat, cx| chat.plugin_rpc(&params, reply, cx))
                    }
                    _ => reply.finish(&plugins, Err(format!("unknown method {method}")), cx),
                }
            }
        }
    }

    /// Projects in the sidebar with their recent sessions, and the open ones.
    fn list_sessions_for_plugin(
        &mut self,
        params: &Value,
        reply: PluginReply,
        cx: &mut Context<Self>,
    ) {
        let limit = params["limit"].as_u64().unwrap_or(20).clamp(1, 200) as usize;
        let projects: Vec<PathBuf> = self
            .workspaces
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        let listing = cx.background_spawn(async move {
            projects
                .into_iter()
                .map(|path| {
                    let sessions = sessions::list_sessions(&path, limit);
                    (path, sessions)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let listed = listing.await;
            this.update(cx, |this, cx| {
                let open: Vec<Entity<ChatView>> = this
                    .workspaces
                    .iter()
                    .flat_map(|entry| entry.chats.iter().map(|chat| chat.view.clone()))
                    .collect();
                let projects: Vec<Value> = listed
                    .iter()
                    .map(|(path, summaries)| {
                        let name = this
                            .workspaces
                            .iter()
                            .find(|entry| entry.path == *path)
                            .map(|entry| entry.name.to_string());
                        let sessions: Vec<Value> = summaries
                            .iter()
                            .map(|summary| {
                                let handle = open
                                    .iter()
                                    .find(|chat| {
                                        chat.read(cx).session_file() == Some(summary.path.as_path())
                                    })
                                    .map(|chat| chat.read(cx).handle().to_string());
                                json!({
                                    "file": summary.path,
                                    "title": summary.title,
                                    "modified": summary
                                        .modified
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|age| age.as_millis() as u64)
                                        .unwrap_or_default(),
                                    "hidden": this.hidden_sessions.contains(&summary.path),
                                    "open": handle,
                                })
                            })
                            .collect();
                        json!({ "path": path, "name": name, "sessions": sessions })
                    })
                    .collect();
                let open: Vec<Value> = open
                    .iter()
                    .map(|chat| this.session_info(chat, cx))
                    .collect();
                let result = json!({ "projects": projects, "open": open });
                reply.finish(&this.plugins, Ok(result), cx);
            })
            .ok();
        })
        .detach();
    }

    /// Opens a session file (or a new session when `file` is absent) in a
    /// project, adding the project to the sidebar if needed.
    fn open_session_for_plugin(
        &mut self,
        params: &Value,
        reply: PluginReply,
        cx: &mut Context<Self>,
    ) {
        let plugins = self.plugins.clone();
        let focus = params["focus"].as_bool().unwrap_or(false);
        let file = params["file"]
            .as_str()
            .filter(|file| !file.is_empty())
            .map(PathBuf::from);
        if let Some(file) = &file
            && let Some(chat) = self
                .workspaces
                .iter()
                .flat_map(|entry| entry.chats.iter())
                .find(|chat| chat.view.read(cx).session_file() == Some(file.as_path()))
                .map(|chat| chat.view.clone())
        {
            if focus {
                self.activate(chat.clone(), cx);
            }
            let info = self.session_info(&chat, cx);
            reply.finish(&plugins, Ok(info), cx);
            return;
        }
        let Some(workspace) = params["workspace"]
            .as_str()
            .map(PathBuf::from)
            .and_then(|path| path.canonicalize().ok())
            .filter(|path| path.is_dir())
        else {
            reply.finish(
                &plugins,
                Err("workspace: give the path of an existing project folder".into()),
                cx,
            );
            return;
        };
        if self.entry_mut(&workspace).is_none() {
            self.add_workspace(workspace.clone(), cx);
            self.save(cx);
        }
        let Some(file) = file else {
            let result = self
                .start_chat_with(&workspace, None, focus, cx)
                .map(|chat| self.session_info(&chat, cx))
                .ok_or_else(|| "pi is not ready yet".to_string());
            reply.finish(&plugins, result, cx);
            return;
        };
        if !file.is_file() {
            let error = format!("no session file {}", file.display());
            reply.finish(&plugins, Err(error), cx);
            return;
        }
        let lease = cx.background_spawn({
            let file = file.clone();
            async move { sessions::foreign_lease_holder(&file) }
        });
        cx.spawn(async move |this, cx| {
            let holder = lease.await;
            this.update(cx, |this, cx| {
                let result = match holder {
                    Some(holder) => Err(format!("the session is open in {holder}")),
                    None => this
                        .start_chat_with(&workspace, Some(file), focus, cx)
                        .map(|chat| this.session_info(&chat, cx))
                        .ok_or_else(|| "pi is not ready yet".to_string()),
                };
                reply.finish(&plugins, result, cx);
            })
            .ok();
        })
        .detach();
    }

    fn activate(&mut self, view: Entity<ChatView>, cx: &mut Context<Self>) {
        self.settings_page = None;
        self.active_chat = Some(view);
        self.focus_active_chat = true;
        self.notice = None;
        cx.notify();
    }

    fn on_chat_event(&mut self, chat: Entity<ChatView>, event: &ChatEvent, cx: &mut Context<Self>) {
        match event {
            ChatEvent::Changed => {
                self.report_session_state(&chat, cx);
                cx.notify();
            }
            ChatEvent::NewSession => {
                let workspace = chat.read(cx).workspace().to_path_buf();
                self.start_chat(&workspace, None, cx);
            }
            ChatEvent::Settled => {
                // The run is written to disk now; refresh titles and order.
                let workspace = chat.read(cx).workspace().to_path_buf();
                self.load_sessions(&workspace, cx);
                // The agent may have changed files.
                if self.diff_panel.read(cx).workspace() == Some(workspace.as_path()) {
                    self.refresh_diff(cx);
                }
                if self.files_panel.read(cx).workspace() == Some(workspace.as_path()) {
                    self.refresh_files(cx);
                }
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
            "docks": self.docks.to_json(),
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
            || self.pi_updating.is_some()
            || self.reloading_sessions
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

    /// Projects and their sessions, in whichever dock holds them.
    fn render_sessions_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut workspaces = Vec::with_capacity(self.workspaces.len());
        for (index, entry) in self.workspaces.iter().enumerate() {
            workspaces.push(self.render_workspace(index, entry, cx));
        }
        v_flex()
            .size_full()
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
            .into_any_element()
    }

    /// Both docks are plain sidebars, flush with the window edges.
    fn render_dock(&self, side: DockSide, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dock = self.docks.dock(side);
        if !dock.is_shown() {
            return None;
        }
        let active = dock.active?;
        let colors = cx.theme().colors.clone();
        let content = match active {
            PanelKind::Sessions => self.render_sessions_panel(cx),
            PanelKind::Files => AnyView::from(self.files_panel.clone())
                .cached(StyleRefinement::default().size_full())
                .into_any_element(),
            PanelKind::Diff => AnyView::from(self.diff_panel.clone())
                .cached(StyleRefinement::default().size_full())
                .into_any_element(),
        };
        let body = v_flex()
            .size_full()
            .child(self.render_dock_header(side, cx))
            .child(v_flex().flex_1().min_h(px(0.)).child(content));
        let handle = div()
            .id(match side {
                DockSide::Left => "left-dock-resize",
                DockSide::Right => "right-dock-resize",
            })
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(5.))
            .map(|this| match side {
                DockSide::Left => this.right_0(),
                DockSide::Right => this.left_0(),
            })
            .cursor_col_resize()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.resizing_dock = Some(side);
                    cx.stop_propagation();
                }),
            );
        Some(
            div()
                .relative()
                .flex_none()
                .w(px(dock.width))
                .h_full()
                .bg(colors.panel_background)
                .border_color(colors.border_variant)
                .map(|this| match side {
                    DockSide::Left => this.border_r_1(),
                    DockSide::Right => this.border_l_1(),
                })
                .child(body)
                .child(handle)
                .into_any_element(),
        )
    }

    /// Tabs for the open sessions only, in sidebar order. Hidden while no
    /// session is open.
    fn render_tab_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let colors = cx.theme().colors.clone();
        let tabs: Vec<AnyElement> = self
            .tab_chats()
            .into_iter()
            .enumerate()
            .map(|(index, (name, view))| self.render_tab(index + 1, &name, view, cx))
            .collect();
        if tabs.is_empty() {
            return None;
        }
        Some(
            h_flex()
                .id("session-tabs")
                .flex_none()
                .w_full()
                .h(px(TAB_BAR_HEIGHT))
                .bg(colors.panel_background)
                .overflow_x_scroll()
                .children(tabs)
                // Fills the rest of the strip and carries its bottom border.
                // Dropping a tab here moves it to the end.
                .child(
                    div()
                        .id("session-tabs-end")
                        .on_drop(cx.listener(|this, dragged: &DraggedTab, _, cx| {
                            this.move_tab(dragged.id, usize::MAX, cx);
                        }))
                        .flex_1()
                        .min_w(px(0.))
                        .h_full()
                        .border_b_1()
                        .border_color(colors.border_variant),
                )
                .into_any_element(),
        )
    }

    /// Open sessions in tab order, with their project names.
    fn tab_chats(&self) -> Vec<(SharedString, Entity<ChatView>)> {
        let mut chats: Vec<_> = self
            .workspaces
            .iter()
            .flat_map(|entry| {
                entry
                    .chats
                    .iter()
                    .map(|chat| (entry.name.clone(), chat.view.clone()))
            })
            .collect();
        // Stable: sessions never dragged keep sidebar order at the end.
        chats.sort_by_key(|(_, view)| {
            self.tab_order
                .iter()
                .position(|id| *id == view.entity_id())
                .unwrap_or(usize::MAX)
        });
        chats
    }

    /// Moves a tab to `target`'s place (`usize::MAX`: the end).
    fn move_tab(&mut self, id: gpui::EntityId, target: usize, cx: &mut Context<Self>) {
        let mut order: Vec<_> = self
            .tab_chats()
            .iter()
            .map(|(_, view)| view.entity_id())
            .collect();
        let Some(from) = order.iter().position(|tab| *tab == id) else {
            return;
        };
        let target = target.min(order.len() - 1);
        if from == target {
            return;
        }
        let tab = order.remove(from);
        order.insert(target, tab);
        self.tab_order = order;
        cx.notify();
    }

    fn render_tab(
        &self,
        number: usize,
        workspace_name: &SharedString,
        view: Entity<ChatView>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let is_active = self.settings_page.is_none() && self.active_chat.as_ref() == Some(&view);
        let chat = view.read(cx);
        let title = chat.title();
        let marker = match chat.status() {
            ChatStatus::Busy => Spinner::new()
                .size(IconSize::XSmall)
                .color(Color::Accent)
                .into_any_element(),
            ChatStatus::Stopped => Indicator::dot(Color::Error).into_any_element(),
            ChatStatus::Ready => Icon::new(IconName::Message)
                .size(IconSize::XSmall)
                .color(Color::Muted)
                .into_any_element(),
        };
        let id = view.entity_id();
        let group: SharedString = format!("session-tab:{id}").into();
        let tooltip = format!("{workspace_name} › {title}");
        let index = number - 1;
        let dragged = DraggedTab {
            id,
            title: title.clone().into(),
            index,
        };
        let drop_target = colors.text_accent.opacity(0.12);
        // While ⌘ is held, the shortcut takes the close button's place.
        let end = if self.command_held && number <= 9 {
            KeyHint::new(format!("⌘{number}")).into_any_element()
        } else {
            div()
                .flex_none()
                .when(!is_active, |this| {
                    this.opacity(0.)
                        .group_hover(group.clone(), |style| style.opacity(1.))
                })
                .child(
                    IconButton::new(ElementId::from(("session-tab-close", id)), IconName::Close)
                        .size(ButtonSize::Compact)
                        .icon_size(IconSize::XSmall)
                        .tooltip(Tooltip::with_optional_keys(
                            "Close session",
                            is_active.then(|| "⌘W".into()),
                        ))
                        .on_click(cx.listener({
                            let view = view.clone();
                            move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.confirm_close(view.clone(), window, cx);
                            }
                        })),
                )
                .into_any_element()
        };
        h_flex()
            .id(ElementId::from(("session-tab", id)))
            .group(group.clone())
            .flex_none()
            .max_w(px(220.))
            .h_full()
            .pl(px(10.))
            .pr(px(4.))
            .gap(px(6.))
            .border_r_1()
            .border_color(colors.border_variant)
            .cursor_pointer()
            .map(|this| {
                if is_active {
                    this.bg(colors.editor_background)
                } else {
                    this.border_b_1()
                        .hover(|style| style.bg(colors.ghost_element_hover))
                }
            })
            .tooltip(Tooltip::text(tooltip))
            .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .drag_over::<DraggedTab>(move |style, dragged, _, _| {
                if dragged.index == index {
                    style
                } else {
                    style.bg(drop_target)
                }
            })
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| {
                this.move_tab(dragged.id, index, cx);
            }))
            .on_click(cx.listener({
                let view = view.clone();
                move |this, _, _, cx| this.activate(view.clone(), cx)
            }))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener({
                    let view = view.clone();
                    move |this, _, window, cx| this.confirm_close(view.clone(), window, cx)
                }),
            )
            .child(div().flex_none().child(marker))
            .child(
                div().min_w(px(0.)).child(
                    Label::new(title)
                        .size(LabelSize::Small)
                        .color(if is_active {
                            Color::Default
                        } else {
                            Color::Muted
                        })
                        .truncate(),
                ),
            )
            .child(end)
            .into_any_element()
    }

    /// A dock with one panel shows its title; with several, a tab per panel.
    /// Right-click a tab or the title to move the panel.
    fn render_dock_header(&self, side: DockSide, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let dock = self.docks.dock(side);
        let active = dock.active;
        let tabbed = dock.panels.len() > 1;
        let title =
            if tabbed {
                h_flex()
                    .gap(px(2.))
                    .children(dock.panels.iter().map(|&kind| {
                        let selected = active == Some(kind);
                        h_flex()
                            .id(ElementId::Name(format!("dock-tab:{kind:?}").into()))
                            .flex_none()
                            .h(px(24.))
                            .px(px(6.))
                            .gap(px(5.))
                            .rounded(px(5.))
                            .cursor_pointer()
                            .when(selected, |this| this.bg(colors.ghost_element_selected))
                            .hover(|style| style.bg(colors.ghost_element_hover))
                            .when(!selected, |this| this.tooltip(Tooltip::text(kind.title())))
                            .on_click(cx.listener(move |this, _, _, cx| this.show_panel(kind, cx)))
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.open_panel_menu(kind, event.position, cx);
                                }),
                            )
                            .child(Icon::new(kind.icon()).size(IconSize::Small).color(
                                if selected {
                                    Color::Default
                                } else {
                                    Color::Muted
                                },
                            ))
                            .when(selected, |this| {
                                this.child(
                                    Label::new(kind.title())
                                        .size(LabelSize::Small)
                                        .weight(FontWeight::MEDIUM),
                                )
                            })
                    }))
                    .into_any_element()
            } else {
                Label::new(active.map(PanelKind::title).unwrap_or_default())
                    .size(LabelSize::Small)
                    .color(Color::Muted)
                    .weight(FontWeight::MEDIUM)
                    .into_any_element()
            };
        let actions = (active == Some(PanelKind::Sessions)).then(|| {
            IconButton::new("add-workspace", IconName::Plus)
                .tooltip(Tooltip::with_keys("Add project", "⌘O"))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.prompt_add_workspace(&AddWorkspace, window, cx)
                }))
        });
        h_flex()
            .id(match side {
                DockSide::Left => "left-dock-header",
                DockSide::Right => "right-dock-header",
            })
            .flex_none()
            .h(px(40.))
            .pl(px(if tabbed { 8. } else { 12. }))
            .pr(px(8.))
            .gap(px(4.))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    if let Some(kind) = active {
                        this.open_panel_menu(kind, event.position, cx);
                    }
                }),
            )
            .child(div().flex_1().min_w(px(0.)).flex().child(title))
            .children(actions)
            .into_any_element()
    }

    fn open_panel_menu(
        &mut self,
        kind: PanelKind,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.panel_menu = Some(PanelMenu { kind, position });
        cx.notify();
    }

    fn render_panel_menu(&self, menu: &PanelMenu, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let kind = menu.kind;
        let target = self.docks.side_of(kind).other();
        let item = |id: &'static str, icon: IconName, label: &'static str| {
            h_flex()
                .id(id)
                .h(px(28.))
                .px(px(8.))
                .gap(px(8.))
                .rounded(px(5.))
                .cursor_pointer()
                .hover(|style| style.bg(colors.element_hover))
                .child(Icon::new(icon).size(IconSize::XSmall).color(Color::Default))
                .child(Label::new(label).size(LabelSize::Small))
        };
        let move_item = match target {
            DockSide::Left => item("menu-move-panel", IconName::PanelLeft, "Move to the left"),
            DockSide::Right => item("menu-move-panel", IconName::PanelRight, "Move to the right"),
        }
        .on_click(cx.listener(move |this, _, _, cx| {
            this.panel_menu = None;
            this.move_panel(kind, target, cx);
        }));
        let hide = item("menu-hide-panel", IconName::Close, "Hide panel").on_click(cx.listener(
            move |this, _, _, cx| {
                this.panel_menu = None;
                let side = this.docks.side_of(kind);
                this.set_dock_open(side, false, cx);
            },
        ));
        deferred(
            anchored().position(menu.position).child(
                v_flex()
                    .id("panel-menu")
                    .w(px(180.))
                    .p(px(4.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.elevated_surface_background)
                    .shadow_lg()
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.panel_menu = None;
                        cx.notify();
                    }))
                    .child(move_item)
                    .child(hide),
            ),
        )
        .with_priority(1)
        .into_any_element()
    }

    /// What the macOS menu bar holds, for platforms without one.
    fn render_app_menu(&self, position: Point<Pixels>, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let item = |id: &'static str, label: &'static str, keys: Option<&'static str>| {
            h_flex()
                .id(id)
                .h(px(28.))
                .px(px(8.))
                .gap(px(8.))
                .rounded(px(5.))
                .cursor_pointer()
                .hover(|style| style.bg(colors.element_hover))
                .child(
                    div()
                        .flex_1()
                        .child(Label::new(label).size(LabelSize::Small)),
                )
                .children(keys.map(KeyHint::new))
        };
        let settings = item("app-menu-settings", "Settings", Some("⌘,")).on_click(cx.listener(
            |this, _, window, cx| {
                this.app_menu = None;
                this.open_settings(&OpenSettings, window, cx);
            },
        ));
        let updates = item("app-menu-updates", "Check for Updates", None).on_click(cx.listener(
            |this, _, _, cx| {
                this.app_menu = None;
                AutoUpdater::global(cx).update(cx, |updater, cx| updater.check(true, cx));
                cx.notify();
            },
        ));
        let about = item("app-menu-about", "About Pi GPUI", None).on_click(cx.listener(
            |this, _, _, cx| {
                this.app_menu = None;
                cx.open_url("https://github.com/bytesbay/pi-gpui");
                cx.notify();
            },
        ));
        let quit = item("app-menu-quit", "Quit", Some("⌘Q")).on_click(|_, _, cx| cx.quit());
        deferred(
            anchored()
                .position(position)
                .anchor(gpui::Corner::TopRight)
                .child(
                    v_flex()
                        .id("app-menu")
                        .w(px(220.))
                        .p(px(4.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background)
                        .shadow_lg()
                        .occlude()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.app_menu = None;
                            cx.notify();
                        }))
                        .child(settings)
                        .child(updates)
                        .child(about)
                        .child(div().my(px(4.)).h(px(1.)).bg(colors.border_variant))
                        .child(quit),
                ),
        )
        .with_priority(1)
        .into_any_element()
    }

    fn move_panel(&mut self, kind: PanelKind, to: DockSide, cx: &mut Context<Self>) {
        self.docks.move_panel(kind, to);
        self.save(cx);
        self.refresh_visible_panels(cx);
        cx.notify();
    }

    fn set_dock_open(&mut self, side: DockSide, open: bool, cx: &mut Context<Self>) {
        let dock = self.docks.dock_mut(side);
        if dock.panels.is_empty() || dock.open == open {
            return;
        }
        dock.open = open;
        if open {
            self.refresh_visible_panels(cx);
        } else {
            // The keyboard may have been inside the closed panel.
            self.focus_active_chat = true;
        }
        self.save(cx);
        cx.notify();
    }

    fn toggle_left_dock(&mut self, _: &ToggleLeftDock, _: &mut Window, cx: &mut Context<Self>) {
        let open = !self.docks.left.open;
        self.set_dock_open(DockSide::Left, open, cx);
    }

    fn toggle_right_dock(&mut self, _: &ToggleRightDock, _: &mut Window, cx: &mut Context<Self>) {
        let open = !self.docks.right.open;
        self.set_dock_open(DockSide::Right, open, cx);
    }

    /// Shows the panel, or hides its dock when the panel is already showing.
    fn toggle_panel(&mut self, kind: PanelKind, cx: &mut Context<Self>) {
        if self.docks.is_visible(kind) {
            let side = self.docks.side_of(kind);
            self.set_dock_open(side, false, cx);
        } else {
            self.show_panel(kind, cx);
        }
    }

    fn toggle_files(&mut self, _: &ToggleFiles, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_panel(PanelKind::Files, cx);
    }

    fn toggle_changes(&mut self, _: &ToggleChanges, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_panel(PanelKind::Diff, cx);
    }

    fn show_panel(&mut self, kind: PanelKind, cx: &mut Context<Self>) {
        self.docks.show(kind);
        self.save(cx);
        match kind {
            PanelKind::Files => self.refresh_files(cx),
            PanelKind::Diff => self.refresh_diff(cx),
            PanelKind::Sessions => {}
        }
        cx.notify();
    }

    /// Follows the active session's project; reads the disk only while visible.
    fn refresh_files(&mut self, cx: &mut Context<Self>) {
        let workspace = self
            .active_chat
            .as_ref()
            .map(|chat| chat.read(cx).workspace().to_path_buf());
        let visible = self.docks.is_visible(PanelKind::Files);
        self.files_panel.update(cx, |panel, cx| {
            panel.set_workspace(workspace, cx);
            if visible {
                panel.refresh(cx);
            }
        });
    }

    /// Follows the active session's project; reads git only while visible.
    fn refresh_diff(&mut self, cx: &mut Context<Self>) {
        let workspace = self
            .active_chat
            .as_ref()
            .map(|chat| chat.read(cx).workspace().to_path_buf());
        let visible = self.docks.is_visible(PanelKind::Diff);
        self.diff_panel.update(cx, |panel, cx| {
            panel.set_workspace(workspace, cx);
            if visible {
                panel.refresh(cx);
            }
        });
    }

    fn refresh_visible_panels(&mut self, cx: &mut Context<Self>) {
        self.refresh_files(cx);
        self.refresh_diff(cx);
    }

    fn render_workspace(
        &self,
        index: usize,
        entry: &WorkspaceEntry,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = entry.path.clone();
        // Keyed by path, not position, so hover and tooltip state follow a
        // project when it is dragged to another place.
        let workspace_id =
            |kind: &str| ElementId::Name(format!("{kind}:{}", entry.path.display()).into());
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

        let header = ListItem::new(workspace_id("workspace"))
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
                rows.push(self.render_session_row(row, cx));
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

        let dragged = DraggedWorkspace {
            path: path.clone(),
            name: entry.name.clone(),
            index,
        };
        let header = div()
            .id(workspace_id("workspace-drag"))
            .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .child(header);

        let drop_target = cx.theme().colors.text_accent.opacity(0.12);
        v_flex()
            .id(workspace_id("workspace-block"))
            .mb(px(2.))
            .rounded(px(5.))
            .drag_over::<DraggedWorkspace>(move |style, dragged, _, _| {
                if dragged.index == index {
                    style
                } else {
                    style.bg(drop_target)
                }
            })
            .on_drop(cx.listener(move |this, dragged: &DraggedWorkspace, _, cx| {
                this.move_workspace(&dragged.path, index, cx);
            }))
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

    /// ⌘1…⌘9: switches to the n-th open session tab.
    fn select_session(&mut self, action: &SelectSession, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, view)) = self
            .tab_chats()
            .into_iter()
            .nth(action.index.saturating_sub(1))
        {
            self.activate(view, cx);
        }
    }

    fn render_session_row(&self, row: SessionRow, cx: &mut Context<Self>) -> AnyElement {
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
        let hide_button = row.file.clone().map(|file| {
            let hidden = self.hidden_sessions.contains(&file);
            IconButton::new(
                ElementId::Name(format!("session-hide:{}", row.id).into()),
                if hidden {
                    IconName::Eye
                } else {
                    IconName::EyeOff
                },
            )
            .size(ButtonSize::Compact)
            .icon_size(IconSize::XSmall)
            .tooltip(Tooltip::text(if hidden { "Show" } else { "Hide" }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_hidden(file.clone(), cx);
            }))
        });
        let close_button = row.open_chat.clone().map(|view| {
            IconButton::new(
                ElementId::Name(format!("session-close:{}", row.id).into()),
                IconName::Close,
            )
            .size(ButtonSize::Compact)
            .icon_size(IconSize::XSmall)
            .tooltip(Tooltip::with_optional_keys(
                "Close session",
                is_active.then(|| "⌘W".into()),
            ))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.confirm_close(view.clone(), window, cx);
            }))
        });
        let hover_buttons = h_flex()
            .gap(px(2.))
            .children(hide_button)
            .child(menu_button)
            .children(close_button);

        let item = ListItem::new(row.id.clone())
            .indent_level(1)
            .toggle_state(is_active)
            .start_slot(div().w(px(12.)).flex().justify_center().child(marker))
            .child(title)
            .on_click(on_click);
        item.when_some(row.age.clone(), |item, age| {
            item.end_slot(
                Label::new(age)
                    .size(LabelSize::XSmall)
                    .color(Color::Placeholder),
            )
        })
        .end_hover_slot(hover_buttons)
        .into_any_element()
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
        let close = row.open_chat.clone().map(|view| {
            item(
                "menu-close",
                IconName::Close,
                "Close session",
                Color::Default,
                true,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.session_menu = None;
                this.confirm_close(view.clone(), window, cx);
            }))
        });
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
                    .children(close)
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

    /// ⌘W: closes the session on screen.
    fn close_active_chat(&mut self, _: &CloseChat, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.active_chat.clone() {
            self.confirm_close(view, window, cx);
        }
    }

    /// Closes an open session, asking first while pi is working in it.
    fn confirm_close(
        &mut self,
        view: Entity<ChatView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !view.read(cx).is_running() {
            self.close_chat(&view, cx);
            return;
        }
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            &format!("Close \"{}\"?", sessions::one_line(&view.read(cx).title(), 60)),
            Some("pi is still working in this session. Closing it stops the run. The session itself is kept."),
            &["Close", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await.ok() != Some(0) {
                return;
            }
            this.update(cx, |this, cx| this.close_chat(&view, cx)).ok();
        })
        .detach();
    }

    /// Stops the session's pi (dropping the chat closes its stdin) and keeps
    /// the session file, so it can be opened again from the sidebar.
    fn close_chat(&mut self, view: &Entity<ChatView>, cx: &mut Context<Self>) {
        let Some(index) = self
            .workspaces
            .iter()
            .position(|entry| entry.chats.iter().any(|chat| &chat.view == view))
        else {
            return;
        };
        let entry = &mut self.workspaces[index];
        let position = entry.chats.iter().position(|chat| &chat.view == view);
        entry.chats.retain(|chat| &chat.view != view);
        let workspace = entry.path.clone();
        if self.active_chat.as_ref() == Some(view) {
            // The neighbour in the same project takes its place, if any.
            let next = position.and_then(|position| {
                let chats = &self.workspaces[index].chats;
                chats
                    .get(position)
                    .or_else(|| chats.last())
                    .map(|chat| chat.view.clone())
            });
            match next {
                Some(next) => self.activate(next, cx),
                None => {
                    self.active_chat = None;
                    self.focus_active_chat = true;
                }
            }
        }
        // A session started here may have just been written; list it.
        self.load_sessions(&workspace, cx);
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
                let buttons = self.render_update_buttons(cx);
                footer.child(
                    h_flex()
                        .gap(px(6.))
                        .child(self.render_pi_version(installation, untested))
                        .child(div().flex_1())
                        .children(buttons),
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
                                .font_family(
                                    crate::settings::Settings::get(cx)
                                        .buffer_font_family
                                        .clone(),
                                )
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
                    "Installer",
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

    fn open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        let settings = Settings::get(cx).clone();
        let text_field =
            |value: SharedString, placeholder: &'static str, cx: &mut Context<Self>| {
                cx.new(|cx| {
                    let mut input = TextInput::single_line(placeholder, cx);
                    input.set_text(value, cx);
                    input
                })
            };
        if self.settings_page.is_none() {
            self.settings_page = Some(SettingsPage {
                ui_font: text_field(settings.ui_font_family.clone(), ".SystemUIFont", cx),
                code_font: text_field(settings.buffer_font_family.clone(), "JetBrains Mono NL", cx),
            });
        }
        window.focus(&self.focus_handle);
        cx.notify();
    }

    fn close_settings(&mut self, cx: &mut Context<Self>) {
        if self.settings_page.take().is_some() {
            self.focus_active_chat = true;
            cx.notify();
        }
    }

    /// Saves a font field when it is confirmed or loses focus.
    fn commit_font_field(&mut self, key: &'static str, cx: &mut Context<Self>) {
        let Some(page) = &self.settings_page else {
            return;
        };
        let input = if key == "ui_font_family" {
            &page.ui_font
        } else {
            &page.code_font
        };
        let text = input.read(cx).text().trim().to_string();
        let current = match key {
            "ui_font_family" => Settings::get(cx).ui_font_family.to_string(),
            _ => Settings::get(cx).buffer_font_family.to_string(),
        };
        if !text.is_empty() && text != current {
            settings::set(cx, key, json!(text));
        }
    }

    /// Zed-style settings: each row explains a setting and changes it in
    /// place; the JSON file stays the source of truth and is one click away.
    fn render_settings_page(&self, page: &SettingsPage, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let current = Settings::get(cx).clone();
        let file = cx.global::<settings::SettingsFile>();
        let path = file.path.clone();
        let error = file.error.clone();

        let row = |title: &'static str, description: &'static str, control: AnyElement| {
            h_flex()
                .gap(px(24.))
                .py(px(12.))
                .border_b_1()
                .border_color(colors.border_variant)
                .child(
                    v_flex()
                        .flex_1()
                        .min_w(px(0.))
                        .gap(px(2.))
                        .child(Label::new(title))
                        .child(
                            Label::new(description)
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                )
                .child(div().flex_none().child(control))
                .into_any_element()
        };
        let group = |title: &'static str| {
            div().pt(px(24.)).pb(px(4.)).child(
                Label::new(title)
                    .size(LabelSize::Small)
                    .color(Color::Placeholder)
                    .weight(FontWeight::MEDIUM),
            )
        };

        let theme_options = [
            (settings::THEME_SYSTEM, "System"),
            (settings::THEME_LIGHT, "Ayu Light"),
            (settings::THEME_DARK, "Ayu Dark"),
        ];
        // A segmented control, as in Zed's settings.
        let theme_control = h_flex()
            .p(px(2.))
            .gap(px(2.))
            .rounded(px(7.))
            .border_1()
            .border_color(colors.border_variant)
            .bg(colors.element_background)
            .children(theme_options.into_iter().map(|(value, label)| {
                let selected = current.theme == value;
                div()
                    .id(ElementId::Name(format!("theme-{value}").into()))
                    .h(px(24.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .rounded(px(5.))
                    .border_1()
                    .cursor_pointer()
                    .when(selected, |this| {
                        this.bg(colors.elevated_surface_background)
                            .border_color(colors.border)
                    })
                    .when(!selected, |this| {
                        this.border_color(gpui::transparent_black())
                            .hover(|style| style.bg(colors.ghost_element_hover))
                    })
                    .on_click(cx.listener(move |_, _, _, cx| {
                        settings::set(cx, "theme", json!(value));
                    }))
                    .child(Label::new(label).size(LabelSize::Small).color(if selected {
                        Color::Default
                    } else {
                        Color::Muted
                    }))
            }))
            .into_any_element();

        let stepper = |key: &'static str, value: f32, cx: &mut Context<Self>| {
            let step = |delta: f32| {
                move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
                    let next = (value + delta).clamp(8., 32.);
                    settings::set(cx, key, json!(next as u64));
                }
            };
            let _ = cx;
            h_flex()
                .gap(px(4.))
                .child(
                    IconButton::new(
                        ElementId::Name(format!("{key}-down").into()),
                        IconName::Dash,
                    )
                    .tooltip(Tooltip::text("Smaller"))
                    .on_click(step(-1.)),
                )
                .child(
                    div()
                        .w(px(44.))
                        .flex()
                        .justify_center()
                        .child(Label::new(format!("{} px", value as u64))),
                )
                .child(
                    IconButton::new(ElementId::Name(format!("{key}-up").into()), IconName::Plus)
                        .tooltip(Tooltip::text("Larger"))
                        .on_click(step(1.)),
                )
                .into_any_element()
        };

        let field = |key: &'static str, input: &Entity<TextInput>, cx: &mut Context<Self>| {
            div()
                .w(px(220.))
                .rounded(px(6.))
                .border_1()
                .border_color(colors.border)
                .bg(colors.editor_background)
                .key_context(SETTINGS_FIELD_CONTEXT)
                .on_action(cx.listener(move |this, _: &crate::chat::Submit, _, cx| {
                    this.commit_font_field(key, cx)
                }))
                .on_mouse_down_out(
                    cx.listener(move |this, _, _, cx| this.commit_font_field(key, cx)),
                )
                .child(input.clone())
                .into_any_element()
        };

        let path_label = sessions::display_path(&path);
        let header = h_flex()
            .gap(px(12.))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(20.))
                            .line_height(px(28.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Settings"),
                    )
                    .child(
                        Label::new(format!(
                            "Saved in {path_label}. The file is the source of truth: edit it by hand or let an agent change it, and the app applies it on save."
                        ))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                    ),
            )
            .child(
                Button::new("open-settings-file", "Open settings.json")
                    .icon(IconName::File)
                    .style(ButtonStyle::Filled)
                    .on_click({
                        let path = path.clone();
                        move |_, _, _| open_in_editor(&path)
                    }),
            )
            .child(
                IconButton::new("close-settings", IconName::Close)
                    .tooltip(Tooltip::text("Close settings"))
                    .on_click(cx.listener(|this, _, _, cx| this.close_settings(cx))),
            );

        let error_banner = error.map(|error| {
            h_flex()
                .mt(px(16.))
                .gap(px(8.))
                .px(px(12.))
                .py(px(8.))
                .rounded(px(6.))
                .bg(colors.error_background)
                .child(Icon::new(IconName::Warning).size(IconSize::XSmall).color(Color::Error))
                .child(
                    Label::new(format!(
                        "settings.json could not be read, the previous settings stay in effect: {error}"
                    ))
                    .size(LabelSize::Small),
                )
        });

        div()
            .id("settings-page")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .flex()
            .justify_center()
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(720.))
                    .px(px(32.))
                    .py(px(28.))
                    .child(header)
                    .children(error_banner)
                    .child(group("Appearance"))
                    .child(row(
                        "Theme",
                        "System follows the system light or dark mode and switches between the two Ayu themes.",
                        theme_control,
                    ))
                    .child(group("Fonts"))
                    .child(row(
                        "Interface font",
                        "Font family of the whole interface. \".SystemUIFont\" is the system font.",
                        field("ui_font_family", &page.ui_font, cx),
                    ))
                    .child(row(
                        "Message size",
                        "Text size of your messages and the agent's answers.",
                        stepper("agent_font_size", current.agent_font_size, cx),
                    ))
                    .child(row(
                        "Code font",
                        "Font family of code blocks, tool output and inline code.",
                        field("buffer_font_family", &page.code_font, cx),
                    ))
                    .child(row(
                        "Code size",
                        "Text size of code blocks and tool output.",
                        stepper("buffer_font_size", current.buffer_font_size, cx),
                    )),
            )
            .into_any_element()
    }

    /// Looks for pi and package updates in the background.
    fn check_pi_updates(&mut self, cx: &mut Context<Self>) {
        let InstallationState::Ready(installation) = &self.installation else {
            return;
        };
        let installation = installation.clone();
        let check = cx.background_spawn(async move { pi_updates::check(&installation) });
        self._pi_updates_task = cx.spawn(async move |this, cx| {
            let updates = check.await;
            this.update(cx, |this, cx| {
                this.pi_updates = updates;
                cx.notify();
            })
            .ok();
        });
    }

    fn toggle_update_popover(&mut self, popover: UpdatePopover, cx: &mut Context<Self>) {
        if self.update_popover == Some(popover) {
            self.update_popover = None;
        } else if self
            .update_popover_closed_at
            .is_none_or(|closed| closed.elapsed() > std::time::Duration::from_millis(250))
        {
            self.update_popover = Some(popover);
        }
        cx.notify();
    }

    fn close_update_popover(&mut self, cx: &mut Context<Self>) {
        if self.update_popover.take().is_some() {
            self.update_popover_closed_at = Some(std::time::Instant::now());
            cx.notify();
        }
    }

    /// Runs `pi update <args>` and reports back. Open sessions keep the pi
    /// and extensions they started with until they are reopened.
    fn run_pi_update(&mut self, label: String, args: Vec<String>, cx: &mut Context<Self>) {
        let InstallationState::Ready(installation) = &self.installation else {
            return;
        };
        if self.pi_updating.is_some() {
            return;
        }
        self.update_popover = None;
        self.pi_updating = Some(label.clone());
        let installation = installation.clone();
        let updates_pi = args.is_empty();
        let target = args.clone();
        let job = cx.background_spawn(async move {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            pi_updates::run(&installation, &args)
        });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            this.update(cx, |this, cx| {
                this.pi_updating = None;
                let open_sessions = this.workspaces.iter().any(|entry| !entry.chats.is_empty());
                let reopen = if open_sessions {
                    crate::keys::in_text(
                        " Open sessions keep the previous version until you reload them (⌘⇧R).",
                    )
                } else {
                    String::new()
                };
                if result.is_ok() {
                    // Drop what was just updated right away; the re-check
                    // below takes a few seconds (npm, `pi list`).
                    match target.as_slice() {
                        [] => this.pi_updates.pi = None,
                        [flag] if flag == "--extensions" => this.pi_updates.packages.clear(),
                        sources => this
                            .pi_updates
                            .packages
                            .retain(|package| !sources.contains(&package.source)),
                    }
                }
                this.notice = Some(match result {
                    Ok(_) => (format!("{label}: done.{reopen}"), Color::Default),
                    Err(error) => (format!("{label} failed: {error}"), Color::Warning),
                });
                if updates_pi {
                    // The version gate runs again and then looks for updates.
                    this.check_installation(cx);
                } else {
                    this.check_pi_updates(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn render_pi_version(&self, installation: &Installation, untested: bool) -> AnyElement {
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
            })
            .into_any_element()
    }

    /// Buttons next to the pi version: a pi update and package updates.
    fn render_update_buttons(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut buttons = Vec::new();
        if let Some(label) = &self.pi_updating {
            buttons.push(
                h_flex()
                    .gap(px(4.))
                    .child(Spinner::new().size(IconSize::XSmall))
                    .child(LoadingLabel::new(label.clone()).size(LabelSize::XSmall))
                    .into_any_element(),
            );
            return buttons;
        }
        let colors = cx.theme().colors.clone();
        let chip = |id: &'static str,
                    text: String,
                    accent: bool,
                    popover: UpdatePopover,
                    cx: &mut Context<Self>| {
            let open = self.update_popover == Some(popover);
            h_flex()
                .id(id)
                .h(px(20.))
                .px(px(6.))
                .gap(px(3.))
                .rounded(px(5.))
                .border_1()
                .border_color(if accent {
                    colors.text_accent.opacity(0.35)
                } else {
                    colors.border_variant
                })
                .when(open, |this| this.bg(colors.ghost_element_selected))
                .hover(|style| style.bg(colors.ghost_element_hover))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                .on_click(
                    cx.listener(move |this, _, _, cx| this.toggle_update_popover(popover, cx)),
                )
                .child(
                    Icon::new(IconName::ArrowUp)
                        .size(IconSize::XSmall)
                        .color(if accent { Color::Accent } else { Color::Muted }),
                )
                .child(Label::new(text).size(LabelSize::XSmall).color(if accent {
                    Color::Accent
                } else {
                    Color::Muted
                }))
        };
        if let Some(version) = self.pi_updates.pi {
            let accent =
                pi_updates::compatibility(version) != pi_updates::PiCompatibility::NeedsAppUpdate;
            let element = chip(
                "pi-update",
                format!("pi {version}"),
                accent,
                UpdatePopover::Pi,
                cx,
            );
            buttons.push(self.with_update_popover(element, UpdatePopover::Pi, cx));
        }
        let count = self.pi_updates.packages.len();
        if count > 0 {
            let text = if count == 1 {
                "1 extension".to_string()
            } else {
                format!("{count} extensions")
            };
            let element = chip(
                "extension-updates",
                text,
                true,
                UpdatePopover::Extensions,
                cx,
            );
            buttons.push(self.with_update_popover(element, UpdatePopover::Extensions, cx));
        }
        buttons
    }

    fn with_update_popover(
        &self,
        button: gpui::Stateful<gpui::Div>,
        popover: UpdatePopover,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let content = (self.update_popover == Some(popover)).then(|| match popover {
            UpdatePopover::Pi => self.render_pi_update_popover(cx),
            UpdatePopover::Extensions => self.render_extension_popover(cx),
        });
        div()
            .relative()
            .flex_none()
            .child(button)
            .when_some(content, |this, content| {
                this.child(
                    div().absolute().top_0().left_0().child(
                        deferred(
                            anchored()
                                .anchor(gpui::Corner::BottomLeft)
                                .snap_to_window_with_margin(px(8.))
                                .child(div().pb(px(6.)).child(content)),
                        )
                        .with_priority(1),
                    ),
                )
            })
            .into_any_element()
    }

    fn update_popover_frame(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let colors = cx.theme().colors.clone();
        v_flex()
            .id("update-popover")
            .w(px(320.))
            .p(px(12.))
            .gap(px(10.))
            .rounded(px(10.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.elevated_surface_background)
            .shadow_lg()
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_update_popover(cx)))
    }

    fn render_pi_update_popover(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(latest) = self.pi_updates.pi else {
            return div().into_any_element();
        };
        let current = match &self.installation {
            InstallationState::Ready(installation) => installation.version.to_string(),
            _ => "?".to_string(),
        };
        let (note, color, allowed) = match pi_updates::compatibility(latest) {
            pi_updates::PiCompatibility::Supported => (
                "Works with this version of Pi GPUI.".to_string(),
                Color::Success,
                true,
            ),
            pi_updates::PiCompatibility::Untested => (
                format!(
                    "Not tested with this app yet (tested up to {}.{}.x). It should work; if something breaks, check for app updates.",
                    pi::TESTED_PI.major,
                    pi::TESTED_PI.minor
                ),
                Color::Warning,
                true,
            ),
            pi_updates::PiCompatibility::NeedsAppUpdate => (
                format!(
                    "This app supports pi {}.x. Update Pi GPUI first, or wait for a version that supports pi {}.",
                    pi::MIN_PI.major,
                    latest.major
                ),
                Color::Error,
                false,
            ),
        };
        self.update_popover_frame(cx)
            .child(
                Label::new("pi update")
                    .size(LabelSize::Small)
                    .color(Color::Placeholder),
            )
            .child(Label::new(format!("{current} → {latest}")).weight(FontWeight::MEDIUM))
            .child(Label::new(note).size(LabelSize::Small).color(color))
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        Button::new("run-pi-update", "Update pi")
                            .style(ButtonStyle::Tinted(TintColor::Accent))
                            .disabled(!allowed)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.run_pi_update(
                                    format!("Updating pi to {latest}"),
                                    Vec::new(),
                                    cx,
                                )
                            })),
                    )
                    .when(!allowed, |this| {
                        this.child(
                            Button::new("pi-update-app", "Check for app updates").on_click(
                                |_, _, cx| {
                                    AutoUpdater::global(cx)
                                        .update(cx, |updater, cx| updater.check(true, cx))
                                },
                            ),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_extension_popover(&self, cx: &mut Context<Self>) -> AnyElement {
        let packages = self.pi_updates.packages.clone();
        let rows: Vec<AnyElement> = packages
            .iter()
            .enumerate()
            .map(|(index, package)| {
                let source = package.source.clone();
                let label = format!("Updating {}", package.name);
                h_flex()
                    .gap(px(8.))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .child(Label::new(package.name.clone()).size(LabelSize::Small))
                            .child(
                                Label::new(format!("{} → {}", package.current, package.latest))
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            ),
                    )
                    .child(
                        Button::new(
                            ElementId::Name(format!("update-extension-{index}").into()),
                            "Update",
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.run_pi_update(label.clone(), vec![source.clone()], cx)
                        })),
                    )
                    .into_any_element()
            })
            .collect();
        let count = packages.len();
        self.update_popover_frame(cx)
            .child(
                Label::new("Extension updates")
                    .size(LabelSize::Small)
                    .color(Color::Placeholder),
            )
            .children(rows)
            .when(count > 1, |this| {
                this.child(
                    h_flex().child(
                        Button::new("update-all-extensions", format!("Update all {count}"))
                            .style(ButtonStyle::Tinted(TintColor::Accent))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.run_pi_update(
                                    format!("Updating {count} extensions"),
                                    vec!["--extensions".to_string()],
                                    cx,
                                )
                            })),
                    ),
                )
            })
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
        let (breadcrumb, status) = if self.settings_page.is_some() {
            (
                Label::new("Settings")
                    .size(LabelSize::Small)
                    .weight(FontWeight::MEDIUM)
                    .color(text_color)
                    .into_any_element(),
                None,
            )
        } else {
            (breadcrumb, status)
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
            // Double-click follows the user's macOS setting (zoom or minimize);
            // on Linux it maximizes, and a right click opens the window menu.
            .on_click(|event, window, _| {
                if event.click_count() >= 2 {
                    if cfg!(target_os = "macos") {
                        window.titlebar_double_click();
                    } else {
                        window.zoom_window();
                    }
                }
            })
            .when(!cfg!(target_os = "macos"), |this| {
                this.on_mouse_down(MouseButton::Right, |event, window, _| {
                    window.show_window_menu(event.position)
                })
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    // The breadcrumb starts right after the traffic lights,
                    // which are hidden in full screen (and absent on Linux).
                    .pl(px(
                        if window.is_fullscreen() || !cfg!(target_os = "macos") {
                            12.
                        } else {
                            TRAFFIC_LIGHT_PADDING
                        },
                    ))
                    .pr(px(10.))
                    .gap(px(12.))
                    .children(self.render_dock_toggle(DockSide::Left, cx))
                    .child(div().flex_1().min_w(px(0.)).flex().child(breadcrumb))
                    .children(plugin_statuses)
                    .children(status)
                    .children(self.render_update_chip(cx))
                    .when(
                        !self.workspaces.iter().all(|entry| entry.chats.is_empty()),
                        |this| {
                            this.child(
                                IconButton::new("reload-sessions", IconName::Refresh)
                                    .loading(self.reloading_sessions)
                                    .tooltip(Tooltip::with_keys("Reload all open sessions", "⌘⇧R"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.reload_sessions(&ReloadSessions, window, cx)
                                    })),
                            )
                        },
                    )
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
                    })
                    .children(self.render_dock_toggle(DockSide::Right, cx))
                    .when(!cfg!(target_os = "macos"), |this| {
                        this.child(
                            IconButton::new("app-menu", IconName::Ellipsis)
                                .toggle_state(self.app_menu.is_some())
                                .tooltip(Tooltip::text("Settings and more"))
                                .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                                    // Below the button, right edges roughly aligned.
                                    let click = event.position();
                                    this.app_menu =
                                        Some(point(click.x + px(14.), click.y + px(18.)));
                                    cx.notify();
                                })),
                        )
                    })
                    .children(render_window_controls(window)),
            )
    }

    /// Title bar button for a dock, hidden while the dock holds no panels.
    fn render_dock_toggle(&self, side: DockSide, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dock = self.docks.dock(side);
        if dock.panels.is_empty() {
            return None;
        }
        let (id, icon, title, keys) = match side {
            DockSide::Left => ("toggle-left-dock", IconName::PanelLeft, "Left panel", "⌘B"),
            DockSide::Right => (
                "toggle-right-dock",
                IconName::PanelRight,
                "Right panel",
                "⌘R",
            ),
        };
        Some(
            IconButton::new(id, icon)
                .toggle_state(dock.open)
                .tooltip(Tooltip::with_keys(title, keys))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let open = !this.docks.dock(side).open;
                    this.set_dock_open(side, open, cx);
                }))
                .into_any_element(),
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

        // Panels follow the active session, wherever it was switched.
        let active_id = self.active_chat.as_ref().map(|chat| chat.entity_id());
        if self.panels_follow != active_id {
            self.panels_follow = active_id;
            cx.defer_in(window, |this, _, cx| this.refresh_visible_panels(cx));
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
        let main = if let Some(page) = &self.settings_page {
            self.render_settings_page(page, cx)
        } else {
            match &self.active_chat {
                Some(chat) => div()
                    .flex_1()
                    .min_h(px(0.))
                    .child(
                        AnyView::from(chat.clone()).cached(StyleRefinement::default().size_full()),
                    )
                    .into_any_element(),
                None => match &self.installation {
                    InstallationState::Failed(error) => self.render_pi_setup(error, cx),
                    _ => self.render_placeholder(cx).into_any_element(),
                },
            }
        };

        let content = v_flex()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::prompt_add_workspace))
            .on_action(cx.listener(Self::new_chat))
            .on_action(cx.listener(Self::close_active_chat))
            .on_action(cx.listener(Self::run_plugin_command))
            .on_action(cx.listener(Self::toggle_details))
            .on_action(cx.listener(Self::select_session))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::toggle_left_dock))
            .on_action(cx.listener(Self::toggle_right_dock))
            .on_action(cx.listener(Self::toggle_files))
            .on_action(cx.listener(Self::toggle_changes))
            .on_action(cx.listener(Self::reload_sessions))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                let Some(side) = this.resizing_dock else {
                    return;
                };
                if event.pressed_button != Some(MouseButton::Left) {
                    this.resizing_dock = None;
                    this.save(cx);
                    return;
                }
                let x = f32::from(event.position.x);
                let width = match side {
                    DockSide::Left => x,
                    DockSide::Right => f32::from(window.viewport_size().width) - x,
                };
                this.docks.dock_mut(side).width = width.clamp(side.min_width(), side.max_width());
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.resizing_dock.take().is_some() {
                        this.save(cx);
                    }
                }),
            )
            .on_action(|_: &Minimize, window, _| window.minimize_window())
            .on_action(|_: &Zoom, window, _| window.zoom_window())
            .on_action(|_: &ToggleFullScreen, window, _| window.toggle_fullscreen())
            .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, _, cx| {
                // ⌘ on macOS, Ctrl elsewhere, and nothing else held.
                let modifiers = &event.modifiers;
                let other = if cfg!(target_os = "macos") {
                    modifiers.control
                } else {
                    modifiers.platform
                };
                let held = modifiers.secondary() && !modifiers.shift && !modifiers.alt && !other;
                if held != this.command_held {
                    this.command_held = held;
                    cx.notify();
                }
            }))
            .size_full()
            .font_family(crate::settings::Settings::get(cx).ui_font_family.clone())
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
                    .children(self.render_dock(DockSide::Left, cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .h_full()
                            .children(self.render_tab_bar(cx))
                            .children(notice)
                            .child(main),
                    )
                    .children(self.render_dock(DockSide::Right, cx)),
            )
            .children(
                self.session_menu
                    .as_ref()
                    .map(|menu| self.render_session_menu(menu, cx)),
            )
            .children(
                self.panel_menu
                    .as_ref()
                    .map(|menu| self.render_panel_menu(menu, cx)),
            )
            .children(
                self.app_menu
                    .map(|position| self.render_app_menu(position, cx)),
            );
        crate::window_frame::decorate(content, window, cx)
    }
}

/// Minimize, maximize and close, when the app draws its own frame (Linux
/// without server-side decorations). The window manager draws them otherwise.
fn render_window_controls(window: &Window) -> Option<AnyElement> {
    if !matches!(
        window.window_decorations(),
        gpui::Decorations::Client { .. }
    ) {
        return None;
    }
    let controls = window.window_controls();
    let maximized = window.is_maximized();
    Some(
        h_flex()
            .flex_none()
            .gap(px(4.))
            .ml(px(4.))
            .when(controls.minimize, |this| {
                this.child(
                    IconButton::new("window-minimize", IconName::Dash)
                        .tooltip(Tooltip::text("Minimize"))
                        .on_click(|_, window, _| window.minimize_window()),
                )
            })
            .when(controls.maximize, |this| {
                this.child(
                    IconButton::new(
                        "window-maximize",
                        if maximized {
                            IconName::Restore
                        } else {
                            IconName::Maximize
                        },
                    )
                    .tooltip(Tooltip::text(if maximized {
                        "Restore"
                    } else {
                        "Maximize"
                    }))
                    .on_click(|_, window, _| window.zoom_window()),
                )
            })
            .child(
                IconButton::new("window-close", IconName::Close)
                    .tooltip(Tooltip::text("Close"))
                    .on_click(|_, window, _| window.remove_window()),
            )
            .into_any_element(),
    )
}

struct PanelMenu {
    kind: PanelKind,
    position: Point<Pixels>,
}

fn load_docks(path: &Path) -> Docks {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .map(|state| Docks::from_json(&state["docks"]))
        .unwrap_or_default()
}

/// `state.json` in the app data directory. App data lives apart from pi's own
/// files, as the principles require.
fn state_file_path() -> Option<PathBuf> {
    Some(sessions::app_data_directory()?.join("state.json"))
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

pub(crate) use crate::platform::open_in_editor;
