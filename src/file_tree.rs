//! The Files panel: the active session's project as a tree, colored by git
//! status like Zed's project panel. Folders are read lazily when expanded.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gpui::{
    AnyElement, ClickEvent, Context, EventEmitter, FocusHandle, KeyBinding, MouseButton,
    MouseDownEvent, Pixels, Point, ScrollStrategy, SharedString, Task, UniformListScrollHandle,
    Window, actions, anchored, deferred, div, px, uniform_list,
};

use crate::diff::FileStatus;
use crate::ui::prelude::*;

actions!(
    files,
    [
        SelectPrevious,
        SelectNext,
        CollapseOrParent,
        ExpandOrChild,
        OpenSelected
    ]
);

const KEY_CONTEXT: &str = "FileTree";
const ROW_HEIGHT: f32 = 24.;
const INDENT: f32 = 14.;
/// Not worth showing: git's own data and Finder's folder metadata.
const SKIPPED_NAMES: [&str; 2] = [".git", ".DS_Store"];

pub fn bind_keys(cx: &mut gpui::App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectPrevious, Some(KEY_CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(KEY_CONTEXT)),
        KeyBinding::new("left", CollapseOrParent, Some(KEY_CONTEXT)),
        KeyBinding::new("right", ExpandOrChild, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", OpenSelected, Some(KEY_CONTEXT)),
    ]);
}

pub enum FilesEvent {
    /// "Add to message": a path relative to the project.
    InsertPath(String),
}

#[derive(Clone)]
struct Entry {
    name: SharedString,
    path: PathBuf,
    is_dir: bool,
}

#[derive(Clone)]
struct VisibleRow {
    entry: Entry,
    depth: usize,
}

#[derive(Default)]
struct GitState {
    status: HashMap<PathBuf, FileStatus>,
    /// Untracked folders git reports as a whole (`?? dir/`).
    untracked_dirs: Vec<PathBuf>,
    ignored: Vec<PathBuf>,
    /// Folders holding a changed file, colored like the change.
    changed_dirs: HashMap<PathBuf, FileStatus>,
}

impl GitState {
    fn status(&self, path: &Path) -> Option<FileStatus> {
        if let Some(status) = self.status.get(path) {
            return Some(*status);
        }
        if let Some(status) = self.changed_dirs.get(path) {
            return Some(*status);
        }
        self.untracked_dirs
            .iter()
            .any(|dir| path.starts_with(dir))
            .then_some(FileStatus::Untracked)
    }

    fn is_ignored(&self, path: &Path) -> bool {
        self.ignored.iter().any(|ignored| path.starts_with(ignored))
    }
}

struct FileMenu {
    path: PathBuf,
    position: Point<Pixels>,
}

pub struct FilesPanel {
    workspace: Option<PathBuf>,
    children: HashMap<PathBuf, Vec<Entry>>,
    expanded: HashSet<PathBuf>,
    /// Expanded folders of projects shown before, restored on coming back.
    remembered: HashMap<PathBuf, HashSet<PathBuf>>,
    selected: Option<PathBuf>,
    git: GitState,
    error: Option<String>,
    rows: Vec<VisibleRow>,
    refreshing: bool,
    scroll: UniformListScrollHandle,
    focus_handle: FocusHandle,
    menu: Option<FileMenu>,
    _refresh_task: Task<()>,
}

impl EventEmitter<FilesEvent> for FilesPanel {}

impl FilesPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            workspace: None,
            children: HashMap::new(),
            expanded: HashSet::new(),
            remembered: HashMap::new(),
            selected: None,
            git: GitState::default(),
            error: None,
            rows: Vec::new(),
            refreshing: false,
            scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            menu: None,
            _refresh_task: Task::ready(()),
        }
    }

    pub fn workspace(&self) -> Option<&Path> {
        self.workspace.as_deref()
    }

    /// Follows the active session's project.
    pub fn set_workspace(&mut self, workspace: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.workspace == workspace {
            return;
        }
        if let Some(previous) = self.workspace.take() {
            self.remembered
                .insert(previous, std::mem::take(&mut self.expanded));
        }
        self.expanded = workspace
            .as_ref()
            .and_then(|path| self.remembered.get(path).cloned())
            .unwrap_or_default();
        self.workspace = workspace;
        self.children.clear();
        self.git = GitState::default();
        self.selected = None;
        self.error = None;
        self.menu = None;
        self.rows.clear();
        self.refreshing = false;
        self._refresh_task = Task::ready(());
        cx.notify();
    }

    /// Re-reads the root, every expanded folder and git status.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.clone() else {
            return;
        };
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        let mut directories = vec![root.clone()];
        directories.extend(self.expanded.iter().cloned());
        let load = cx.background_spawn(async move {
            let listings: Vec<(PathBuf, Result<Vec<Entry>, String>)> = directories
                .into_iter()
                .map(|directory| {
                    let listing = read_directory(&directory);
                    (directory, listing)
                })
                .collect();
            (listings, git_state(&root))
        });
        self._refresh_task = cx.spawn(async move |this, cx| {
            let (listings, git) = load.await;
            this.update(cx, |this, cx| {
                this.refreshing = false;
                this.children.clear();
                this.error = None;
                for (directory, listing) in listings {
                    match listing {
                        Ok(entries) => {
                            this.children.insert(directory, entries);
                        }
                        Err(error) if Some(&directory) == this.workspace.as_ref() => {
                            this.error = Some(error);
                        }
                        // Gone since it was expanded.
                        Err(_) => {
                            this.expanded.remove(&directory);
                        }
                    }
                }
                this.git = git;
                this.rebuild_rows();
                cx.notify();
            })
            .ok();
        });
        cx.notify();
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        if let Some(root) = &self.workspace {
            self.push_rows(root, 0, &mut rows);
        }
        self.rows = rows;
    }

    fn push_rows(&self, directory: &Path, depth: usize, rows: &mut Vec<VisibleRow>) {
        let Some(entries) = self.children.get(directory) else {
            return;
        };
        for entry in entries {
            rows.push(VisibleRow {
                entry: entry.clone(),
                depth,
            });
            if entry.is_dir && self.expanded.contains(&entry.path) {
                self.push_rows(&entry.path, depth + 1, rows);
            }
        }
    }

    fn set_expanded(&mut self, directory: PathBuf, expanded: bool, cx: &mut Context<Self>) {
        if !expanded {
            self.expanded.remove(&directory);
            self.rebuild_rows();
            cx.notify();
            return;
        }
        if !self.expanded.insert(directory.clone()) {
            return;
        }
        if self.children.contains_key(&directory) {
            self.rebuild_rows();
            cx.notify();
            return;
        }
        let load = {
            let directory = directory.clone();
            cx.background_spawn(async move { read_directory(&directory) })
        };
        cx.spawn(async move |this, cx| {
            let listing = load.await;
            this.update(cx, |this, cx| {
                match listing {
                    Ok(entries) => {
                        this.children.insert(directory, entries);
                    }
                    Err(error) => {
                        eprintln!("pi-gpui: could not read {}: {error}", directory.display());
                        this.expanded.remove(&directory);
                    }
                }
                this.rebuild_rows();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn selected_index(&self) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        self.rows.iter().position(|row| &row.entry.path == selected)
    }

    fn select_index(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get(index) {
            self.selected = Some(row.entry.path.clone());
            self.scroll.scroll_to_item(index, ScrollStrategy::Top);
            cx.notify();
        }
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        let index = match self.selected_index() {
            Some(index) => index.saturating_sub(1),
            None => self.rows.len().saturating_sub(1),
        };
        self.select_index(index, cx);
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let index = match self.selected_index() {
            Some(index) => (index + 1).min(self.rows.len().saturating_sub(1)),
            None => 0,
        };
        self.select_index(index, cx);
    }

    fn collapse_or_parent(&mut self, _: &CollapseOrParent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.selected_index() else {
            return;
        };
        let row = self.rows[index].clone();
        if row.entry.is_dir && self.expanded.contains(&row.entry.path) {
            self.set_expanded(row.entry.path, false, cx);
            return;
        }
        if let Some(parent) = row.entry.path.parent()
            && let Some(parent_index) = self.rows.iter().position(|row| row.entry.path == parent)
        {
            self.select_index(parent_index, cx);
        }
    }

    fn expand_or_child(&mut self, _: &ExpandOrChild, _: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.selected_index() else {
            return;
        };
        let entry = self.rows[index].entry.clone();
        if !entry.is_dir {
            return;
        }
        if self.expanded.contains(&entry.path) {
            if self
                .rows
                .get(index + 1)
                .is_some_and(|row| row.entry.path.parent() == Some(entry.path.as_path()))
            {
                self.select_index(index + 1, cx);
            }
        } else {
            self.set_expanded(entry.path, true, cx);
        }
    }

    fn open_selected(&mut self, _: &OpenSelected, _: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.selected_index() else {
            return;
        };
        let entry = self.rows[index].entry.clone();
        if entry.is_dir {
            let expanded = self.expanded.contains(&entry.path);
            self.set_expanded(entry.path, !expanded, cx);
        } else {
            crate::hub::open_in_editor(&entry.path);
        }
    }

    fn click_row(
        &mut self,
        entry: &Entry,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        self.selected = Some(entry.path.clone());
        if entry.is_dir {
            let expanded = self.expanded.contains(&entry.path);
            self.set_expanded(entry.path.clone(), !expanded, cx);
        } else if event.click_count() >= 2 {
            crate::hub::open_in_editor(&entry.path);
        }
        cx.notify();
    }

    fn relative_path(&self, path: &Path) -> String {
        self.workspace
            .as_deref()
            .and_then(|root| path.strip_prefix(root).ok())
            .unwrap_or(path)
            .display()
            .to_string()
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let colors = cx.theme().colors.clone();
        range
            .filter_map(|index| {
                let row = self.rows.get(index)?.clone();
                let entry = row.entry;
                let name = entry.name.clone();
                let selected = self.selected.as_ref() == Some(&entry.path);
                let expanded = entry.is_dir && self.expanded.contains(&entry.path);
                let color = if self.git.is_ignored(&entry.path) {
                    Color::Placeholder
                } else {
                    match self.git.status(&entry.path) {
                        Some(status) => status.color(),
                        None => Color::Default,
                    }
                };
                let icon = match (entry.is_dir, expanded) {
                    (true, true) => IconName::FolderOpen,
                    (true, false) => IconName::Folder,
                    (false, _) => IconName::File,
                };
                let disclosure = if entry.is_dir {
                    Icon::new(if expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .size(IconSize::XSmall)
                    .color(Color::Muted)
                    .into_any_element()
                } else {
                    div().w(px(12.)).into_any_element()
                };
                let menu_path = entry.path.clone();
                Some(
                    h_flex()
                        .id(("file-row", index))
                        .h(px(ROW_HEIGHT))
                        .pl(px(8. + row.depth as f32 * INDENT))
                        .pr(px(8.))
                        .gap(px(4.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .when(selected, |this| this.bg(colors.ghost_element_selected))
                        .hover(|style| style.bg(colors.ghost_element_hover))
                        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                            this.click_row(&entry, event, window, cx)
                        }))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                window.focus(&this.focus_handle);
                                this.selected = Some(menu_path.clone());
                                this.menu = Some(FileMenu {
                                    path: menu_path.clone(),
                                    position: event.position,
                                });
                                cx.notify();
                            }),
                        )
                        .child(disclosure)
                        .child(Icon::new(icon).size(IconSize::Small).color(Color::Muted))
                        .child(
                            div().flex_1().min_w(px(0.)).child(
                                Label::new(name)
                                    .size(LabelSize::Small)
                                    .color(color)
                                    .truncate(),
                            ),
                        )
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn render_menu(&self, menu: &FileMenu, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let is_dir = fs::metadata(&menu.path).is_ok_and(|metadata| metadata.is_dir());
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
        let path = menu.path.clone();
        let relative = self.relative_path(&menu.path);
        let open = (!is_dir).then(|| {
            let path = path.clone();
            item("file-menu-open", IconName::File, "Open in editor").on_click(cx.listener(
                move |this, _, _, cx| {
                    this.menu = None;
                    crate::hub::open_in_editor(&path);
                    cx.notify();
                },
            ))
        });
        let insert = {
            let relative = relative.clone();
            item("file-menu-insert", IconName::Message, "Add to message").on_click(cx.listener(
                move |this, _, _, cx| {
                    this.menu = None;
                    cx.emit(FilesEvent::InsertPath(relative.clone()));
                    cx.notify();
                },
            ))
        };
        let reveal = {
            let path = path.clone();
            item("file-menu-reveal", IconName::FolderOpen, "Reveal in Finder").on_click(
                cx.listener(move |this, _, _, cx| {
                    this.menu = None;
                    reveal_in_finder(&path);
                    cx.notify();
                }),
            )
        };
        let copy = item("file-menu-copy", IconName::Copy, "Copy relative path").on_click(
            cx.listener(move |this, _, _, cx| {
                this.menu = None;
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(relative.clone()));
                cx.notify();
            }),
        );
        deferred(
            anchored().position(menu.position).child(
                v_flex()
                    .id("file-menu")
                    .w(px(190.))
                    .p(px(4.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.elevated_surface_background)
                    .shadow_lg()
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.menu = None;
                        cx.notify();
                    }))
                    .children(open)
                    .child(insert)
                    .child(reveal)
                    .child(copy),
            ),
        )
        .with_priority(1)
        .into_any_element()
    }

    fn render_message(&self, text: String) -> AnyElement {
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .px(px(24.))
            .child(
                div().text_center().child(
                    Label::new(text)
                        .size(LabelSize::Small)
                        .color(Color::Placeholder),
                ),
            )
            .into_any_element()
    }
}

impl Render for FilesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors.clone();
        let content = if self.workspace.is_none() {
            self.render_message("Open a session to see its project's files.".into())
        } else if let Some(error) = &self.error {
            self.render_message(format!("Could not read the folder: {error}"))
        } else if self.rows.is_empty() && self.refreshing {
            h_flex()
                .flex_1()
                .justify_center()
                .child(Spinner::new().size(IconSize::Small).color(Color::Muted))
                .into_any_element()
        } else if self.rows.is_empty() {
            self.render_message("This folder is empty.".into())
        } else {
            uniform_list(
                "file-tree",
                self.rows.len(),
                cx.processor(|this, range, _, cx| this.render_rows(range, cx)),
            )
            .track_scroll(self.scroll.clone())
            .flex_1()
            .px(px(6.))
            .pb(px(10.))
            .into_any_element()
        };
        let project = self.workspace.as_ref().map(|workspace| {
            let name = workspace
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| workspace.display().to_string());
            h_flex()
                .flex_none()
                .h(px(30.))
                .pl(px(12.))
                .pr(px(6.))
                .gap(px(6.))
                .border_b_1()
                .border_color(colors.border_variant)
                .child(
                    div().flex_1().min_w(px(0.)).child(
                        Label::new(name)
                            .size(LabelSize::XSmall)
                            .color(Color::Muted)
                            .truncate(),
                    ),
                )
                .child(
                    IconButton::new("refresh-files", IconName::Refresh)
                        .size(ButtonSize::Compact)
                        .icon_size(IconSize::XSmall)
                        .tooltip(Tooltip::text("Refresh"))
                        .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                )
        });
        let menu = self.menu.as_ref().map(|menu| self.render_menu(menu, cx));
        v_flex()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::collapse_or_parent))
            .on_action(cx.listener(Self::expand_or_child))
            .on_action(cx.listener(Self::open_selected))
            .size_full()
            .children(project)
            .child(content)
            .children(menu)
    }
}

/// Folders first, then files, by name ignoring case.
fn read_directory(directory: &Path) -> Result<Vec<Entry>, String> {
    let mut entries: Vec<Entry> = fs::read_dir(directory)
        .map_err(|error| error.to_string())?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if SKIPPED_NAMES.contains(&name.as_str()) {
                return None;
            }
            let path = entry.path();
            // Follows symlinks, so a linked folder can be opened.
            let is_dir = fs::metadata(&path).is_ok_and(|metadata| metadata.is_dir());
            Some(Entry {
                name: name.into(),
                path,
                is_dir,
            })
        })
        .collect();
    entries.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok(entries)
}

/// Empty outside a git work tree or when git is missing.
fn git_state(workspace: &Path) -> GitState {
    let run = |arguments: &[&str]| -> Option<Vec<u8>> {
        let output = Command::new("git")
            .arg("-C")
            .arg(workspace)
            .args(arguments)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        output.status.success().then_some(output.stdout)
    };
    let Some(root) = run(&["rev-parse", "--show-toplevel"]) else {
        return GitState::default();
    };
    let root = PathBuf::from(String::from_utf8_lossy(&root).trim_end_matches(['\n', '\r']));
    let Some(status) = run(&["status", "--porcelain=v1", "-z", "--ignored"]) else {
        return GitState::default();
    };
    parse_status(&String::from_utf8_lossy(&status), &root)
}

fn parse_status(output: &str, root: &Path) -> GitState {
    let mut state = GitState::default();
    let mut tokens = output.split('\0').filter(|token| !token.is_empty());
    while let Some(token) = tokens.next() {
        if token.len() < 4 {
            continue;
        }
        let (code, path) = token.split_at(3);
        let code = code.trim_end();
        let is_dir = path.ends_with('/');
        let path = root.join(path.trim_end_matches('/'));
        let status = match code {
            "!!" => {
                state.ignored.push(path);
                continue;
            }
            "??" => FileStatus::Untracked,
            _ if code.contains('R') || code.contains('C') => {
                // The original path follows as its own token.
                tokens.next();
                FileStatus::Renamed
            }
            _ if code.contains('A') => FileStatus::Added,
            _ if code.contains('D') => FileStatus::Deleted,
            _ => FileStatus::Modified,
        };
        if is_dir {
            state.untracked_dirs.push(path.clone());
        } else {
            state.status.insert(path.clone(), status);
        }
        let mut parent = path.parent();
        while let Some(directory) = parent {
            if directory == root || !directory.starts_with(root) {
                break;
            }
            let entry = state
                .changed_dirs
                .entry(directory.to_path_buf())
                .or_insert(status);
            // A modification outweighs additions in the folder's color.
            if status == FileStatus::Modified {
                *entry = FileStatus::Modified;
            }
            parent = directory.parent();
        }
    }
    state
}

fn reveal_in_finder(path: &Path) {
    if let Err(error) = Command::new("open").arg("-R").arg(path).spawn() {
        eprintln!("pi-gpui: could not reveal {}: {error}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_marks_files_folders_and_ignored_paths() {
        let root = Path::new("/repo");
        let output = " M src/main.rs\0?? notes/\0R  new.rs\0old.rs\0!! target/\0A  src/ui/new.rs\0";
        let state = parse_status(output, root);
        assert_eq!(
            state.status(Path::new("/repo/src/main.rs")),
            Some(FileStatus::Modified)
        );
        assert_eq!(
            state.status(Path::new("/repo/notes/todo.md")),
            Some(FileStatus::Untracked)
        );
        assert_eq!(
            state.status(Path::new("/repo/new.rs")),
            Some(FileStatus::Renamed)
        );
        assert_eq!(state.status(Path::new("/repo/old.rs")), None);
        assert_eq!(
            state.status(Path::new("/repo/src")),
            Some(FileStatus::Modified)
        );
        assert_eq!(
            state.status(Path::new("/repo/src/ui")),
            Some(FileStatus::Added)
        );
        assert!(state.is_ignored(Path::new("/repo/target/debug/app")));
        assert!(!state.is_ignored(Path::new("/repo/src/main.rs")));
    }
}
