//! The Changes panel: `git diff` of the active session's project against
//! HEAD, untracked files included. Read-only; git is run directly with
//! separate arguments, never through a shell.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gpui::{AnyElement, Context, ElementId, FontWeight, Task, Window, div, px};

use crate::sessions;
use crate::settings::Settings;
use crate::ui::AnimationTicker;
use crate::ui::prelude::*;

/// Lines drawn per file; the rest is summarized. Keeps a huge generated
/// file from building thousands of elements.
const MAX_LINES_PER_FILE: usize = 1_500;
/// Untracked files larger than this are listed without content.
const MAX_UNTRACKED_BYTES: u64 = 512 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
}

impl FileStatus {
    fn letter(self) -> &'static str {
        match self {
            FileStatus::Modified => "M",
            FileStatus::Added => "A",
            FileStatus::Deleted => "D",
            FileStatus::Renamed => "R",
            FileStatus::Untracked => "U",
        }
    }

    pub fn color(self) -> Color {
        match self {
            FileStatus::Modified | FileStatus::Renamed => Color::Warning,
            FileStatus::Added | FileStatus::Untracked => Color::Success,
            FileStatus::Deleted => Color::Error,
        }
    }

    fn description(self) -> &'static str {
        match self {
            FileStatus::Modified => "Modified",
            FileStatus::Added => "Added",
            FileStatus::Deleted => "Deleted",
            FileStatus::Renamed => "Renamed",
            FileStatus::Untracked => "Untracked",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Hunk,
    Context,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub binary: bool,
    pub additions: usize,
    pub deletions: usize,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug)]
pub struct Changes {
    pub root: PathBuf,
    pub files: Vec<FileDiff>,
}

enum State {
    /// No session is open, so there is no project to look at.
    NoProject,
    Loading,
    NotARepository,
    Failed(String),
    Loaded(Changes),
}

pub struct DiffPanel {
    workspace: Option<PathBuf>,
    state: State,
    /// A refresh is in flight while the previous result stays on screen.
    refreshing: bool,
    expanded: HashSet<String>,
    _task: Task<()>,
    animation: AnimationTicker,
}

impl DiffPanel {
    pub fn new() -> Self {
        Self {
            workspace: None,
            state: State::NoProject,
            refreshing: false,
            expanded: HashSet::new(),
            _task: Task::ready(()),
            animation: AnimationTicker::default(),
        }
    }

    /// Follows the active session's project. Returns true when it changed.
    pub fn set_workspace(&mut self, workspace: Option<PathBuf>, cx: &mut Context<Self>) -> bool {
        if self.workspace == workspace {
            return false;
        }
        self.workspace = workspace;
        self.expanded.clear();
        self.state = if self.workspace.is_some() {
            State::Loading
        } else {
            State::NoProject
        };
        self._task = Task::ready(());
        self.refreshing = false;
        cx.notify();
        true
    }

    pub fn workspace(&self) -> Option<&Path> {
        self.workspace.as_deref()
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        let load = cx.background_spawn(async move { load_changes(&workspace) });
        self._task = cx.spawn(async move |this, cx| {
            let result = load.await;
            this.update(cx, |this, cx| {
                this.refreshing = false;
                this.state = match result {
                    Ok(Some(changes)) => State::Loaded(changes),
                    Ok(None) => State::NotARepository,
                    Err(error) => {
                        eprintln!("pi-gpui: git diff failed: {error}");
                        State::Failed(error)
                    }
                };
                cx.notify();
            })
            .ok();
        });
        cx.notify();
    }

    /// "3 files  +12 −4" for the panel header.
    pub fn summary(&self) -> Option<String> {
        let State::Loaded(changes) = &self.state else {
            return None;
        };
        if changes.files.is_empty() {
            return None;
        }
        let additions: usize = changes.files.iter().map(|file| file.additions).sum();
        let deletions: usize = changes.files.iter().map(|file| file.deletions).sum();
        let files = match changes.files.len() {
            1 => "1 file".to_string(),
            count => format!("{count} files"),
        };
        Some(format!("{files}  +{additions} −{deletions}"))
    }

    fn toggle_file(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_string());
        }
        cx.notify();
    }

    fn render_message(&self, message: String) -> AnyElement {
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .px(px(24.))
            .child(
                div().text_center().child(
                    Label::new(message)
                        .size(LabelSize::Small)
                        .color(Color::Placeholder),
                ),
            )
            .into_any_element()
    }

    fn render_file(&self, file: &FileDiff, root: &Path, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let expanded = self.expanded.contains(&file.path);
        let (directory, name) = match file.path.rsplit_once('/') {
            Some((directory, name)) => (format!("{directory}/"), name.to_string()),
            None => (String::new(), file.path.clone()),
        };
        let path = file.path.clone();
        let full_path = root.join(&file.path);
        let tooltip = match &file.old_path {
            Some(old_path) => format!("{} from {old_path}", file.status.description()),
            None => format!(
                "{}: {}",
                file.status.description(),
                sessions::display_path(&full_path)
            ),
        };

        let header = h_flex()
            .id(ElementId::Name(format!("diff-file:{}", file.path).into()))
            .h(px(28.))
            .px(px(8.))
            .gap(px(6.))
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|style| style.bg(colors.element_hover))
            .tooltip(Tooltip::text(tooltip))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_file(&path, cx)))
            .child(
                Icon::new(if expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .size(IconSize::XSmall)
                .color(Color::Muted),
            )
            .child(
                div().flex_none().w(px(12.)).child(
                    Label::new(file.status.letter())
                        .size(LabelSize::XSmall)
                        .weight(FontWeight::MEDIUM)
                        .color(file.status.color()),
                ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(4.))
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_none()
                            .child(Label::new(name).size(LabelSize::Small)),
                    )
                    .child(
                        div().min_w(px(0.)).child(
                            Label::new(directory)
                                .size(LabelSize::XSmall)
                                .color(Color::Placeholder)
                                .truncate(),
                        ),
                    ),
            )
            .when(file.additions > 0, |this| {
                this.child(
                    Label::new(format!("+{}", file.additions))
                        .size(LabelSize::XSmall)
                        .color(Color::Success),
                )
            })
            .when(file.deletions > 0, |this| {
                this.child(
                    Label::new(format!("−{}", file.deletions))
                        .size(LabelSize::XSmall)
                        .color(Color::Error),
                )
            });

        let body = expanded.then(|| self.render_file_lines(file, cx));
        v_flex().child(header).children(body).into_any_element()
    }

    fn render_file_lines(&self, file: &FileDiff, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let font = Settings::get(cx).buffer_font_family.clone();
        if file.binary {
            return div()
                .px(px(28.))
                .py(px(4.))
                .child(
                    Label::new("Binary file")
                        .size(LabelSize::XSmall)
                        .color(Color::Placeholder),
                )
                .into_any_element();
        }
        if file.lines.is_empty() {
            return div()
                .px(px(28.))
                .py(px(4.))
                .child(
                    Label::new("No text changes")
                        .size(LabelSize::XSmall)
                        .color(Color::Placeholder),
                )
                .into_any_element();
        }
        let hidden = file.lines.len().saturating_sub(MAX_LINES_PER_FILE);
        let lines = file.lines.iter().take(MAX_LINES_PER_FILE).map(|line| {
            let (sign, background, text_color) = match line.kind {
                LineKind::Hunk => (
                    "",
                    Some(colors.info_background.opacity(0.5)),
                    colors.text_muted,
                ),
                LineKind::Context => (" ", None, colors.text_muted),
                LineKind::Added => ("+", Some(colors.success.opacity(0.14)), colors.text),
                LineKind::Removed => ("−", Some(colors.error.opacity(0.14)), colors.text),
            };
            h_flex()
                .items_start()
                .when_some(background, |this, background| this.bg(background))
                .child(
                    div()
                        .flex_none()
                        .w(px(16.))
                        .text_center()
                        .text_color(colors.text_placeholder)
                        .child(sign),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .pr(px(6.))
                        .text_color(text_color)
                        // Keeps empty lines one line tall.
                        .child(if line.text.is_empty() {
                            " ".to_string()
                        } else {
                            line.text.replace('\t', "    ")
                        }),
                )
        });
        v_flex()
            .mx(px(4.))
            .mb(px(6.))
            .py(px(2.))
            .rounded(px(4.))
            .overflow_hidden()
            .bg(colors.editor_background)
            .border_1()
            .border_color(colors.border_variant)
            .font_family(font)
            .text_size(px(11.5))
            .line_height(px(17.))
            .children(lines)
            .when(hidden > 0, |this| {
                this.child(
                    div().px(px(16.)).py(px(4.)).child(
                        Label::new(format!("{hidden} more lines not shown"))
                            .size(LabelSize::XSmall)
                            .color(Color::Placeholder),
                    ),
                )
            })
            .into_any_element()
    }
}

impl Render for DiffPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let loading = matches!(self.state, State::Loading);
        self.animation.set_active(loading || self.refreshing, cx);
        let colors = cx.theme().colors.clone();
        let content = match &self.state {
            State::NoProject => {
                self.render_message("Open a session to see its project's changes.".into())
            }
            State::Loading => h_flex()
                .flex_1()
                .justify_center()
                .gap(px(8.))
                .child(Spinner::new().size(IconSize::Small).color(Color::Muted))
                .child(LoadingLabel::new("Reading git diff…").size(LabelSize::Small))
                .into_any_element(),
            State::NotARepository => {
                self.render_message("This project is not a git repository.".into())
            }
            State::Failed(error) => self.render_message(format!("git diff failed: {error}")),
            State::Loaded(changes) if changes.files.is_empty() => {
                self.render_message("No changes against HEAD.".into())
            }
            State::Loaded(changes) => {
                let root = changes.root.clone();
                let files: Vec<AnyElement> = changes
                    .files
                    .iter()
                    .map(|file| self.render_file(file, &root, cx))
                    .collect();
                div()
                    .id("diff-files")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .px(px(6.))
                    .pb(px(10.))
                    .children(files)
                    .into_any_element()
            }
        };
        let toolbar = self.workspace.is_some().then(|| {
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
                        Label::new(
                            self.summary()
                                .unwrap_or_else(|| "Working tree against HEAD".to_string()),
                        )
                        .size(LabelSize::XSmall)
                        .color(Color::Muted)
                        .truncate(),
                    ),
                )
                .child(
                    IconButton::new("refresh-diff", IconName::Refresh)
                        .size(ButtonSize::Compact)
                        .icon_size(IconSize::XSmall)
                        .loading(self.refreshing)
                        .tooltip(Tooltip::text("Refresh"))
                        .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                )
        });
        v_flex().size_full().children(toolbar).child(content)
    }
}

/// `Ok(None)` when the folder is not inside a git work tree.
fn load_changes(workspace: &Path) -> Result<Option<Changes>, String> {
    let root = match git(workspace, &["rev-parse", "--show-toplevel"]) {
        Ok(output) => PathBuf::from(output.trim_end_matches(['\n', '\r'])),
        Err(GitError::Failed(_)) => return Ok(None),
        Err(GitError::Spawn(error)) => return Err(error),
    };
    // A fresh repository has no HEAD yet: compare with the empty tree.
    let base = match git(&root, &["rev-parse", "--verify", "--quiet", "HEAD"]) {
        Ok(_) => "HEAD".to_string(),
        Err(_) => git(&root, &["hash-object", "-t", "tree", "/dev/null"])
            .map_err(GitError::message)?
            .trim()
            .to_string(),
    };
    let diff = git(
        &root,
        &[
            "-c",
            "core.quotePath=false",
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--find-renames",
            &base,
        ],
    )
    .map_err(GitError::message)?;
    let mut files = parse_diff(&diff);
    let untracked = git(&root, &["ls-files", "--others", "--exclude-standard", "-z"])
        .map_err(GitError::message)?;
    for path in untracked.split('\0').filter(|path| !path.is_empty()) {
        files.push(untracked_file(&root, path));
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(Some(Changes { root, files }))
}

enum GitError {
    Spawn(String),
    Failed(String),
}

impl GitError {
    fn message(self) -> String {
        match self {
            GitError::Spawn(message) | GitError::Failed(message) => message,
        }
    }
}

fn git(directory: &Path, arguments: &[&str]) -> Result<String, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| GitError::Spawn(format!("could not run git: {error}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(GitError::Failed(if stderr.is_empty() {
            format!("git exited with {}", output.status)
        } else {
            stderr
        }));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn untracked_file(root: &Path, path: &str) -> FileDiff {
    let mut file = FileDiff {
        path: path.to_string(),
        old_path: None,
        status: FileStatus::Untracked,
        binary: false,
        additions: 0,
        deletions: 0,
        lines: Vec::new(),
    };
    let full_path = root.join(path);
    let too_big =
        fs::metadata(&full_path).is_ok_and(|metadata| metadata.len() > MAX_UNTRACKED_BYTES);
    match fs::read(&full_path) {
        Ok(bytes) if !too_big && !bytes.contains(&0) => {
            let text = String::from_utf8_lossy(&bytes);
            file.lines = text
                .lines()
                .map(|line| DiffLine {
                    kind: LineKind::Added,
                    text: line.to_string(),
                })
                .collect();
            file.additions = file.lines.len();
        }
        _ => file.binary = true,
    }
    file
}

/// Parses `git diff` output (unified format, no color).
pub fn parse_diff(output: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut in_hunk = false;
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            in_hunk = false;
            let path = rest
                .split_once(" b/")
                .map(|(_, path)| path)
                .unwrap_or(rest)
                .to_string();
            files.push(FileDiff {
                path,
                old_path: None,
                status: FileStatus::Modified,
                binary: false,
                additions: 0,
                deletions: 0,
                lines: Vec::new(),
            });
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if line.starts_with("@@") {
            in_hunk = true;
            file.lines.push(DiffLine {
                kind: LineKind::Hunk,
                text: line.to_string(),
            });
            continue;
        }
        if !in_hunk {
            if line.starts_with("new file mode") {
                file.status = FileStatus::Added;
            } else if line.starts_with("deleted file mode") {
                file.status = FileStatus::Deleted;
            } else if let Some(from) = line.strip_prefix("rename from ") {
                file.status = FileStatus::Renamed;
                file.old_path = Some(from.to_string());
            } else if let Some(to) = line.strip_prefix("rename to ") {
                file.path = to.to_string();
            } else if line.starts_with("Binary files ") {
                file.binary = true;
            } else if let Some(path) = line.strip_prefix("+++ b/") {
                file.path = path.to_string();
            }
            continue;
        }
        let (kind, text) = match line.as_bytes().first() {
            Some(b'+') => (LineKind::Added, &line[1..]),
            Some(b'-') => (LineKind::Removed, &line[1..]),
            Some(b' ') => (LineKind::Context, &line[1..]),
            // "\ No newline at end of file" and anything unexpected.
            _ => continue,
        };
        match kind {
            LineKind::Added => file.additions += 1,
            LineKind::Removed => file.deletions += 1,
            _ => {}
        }
        file.lines.push(DiffLine {
            kind,
            text: text.to_string(),
        });
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modified_added_deleted_and_renamed_files() {
        let output = "\
diff --git a/src/main.rs b/src/main.rs
index 1111111..2222222 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,3 @@ fn main() {
 keep
-old
+new
\\ No newline at end of file
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/new.txt
@@ -0,0 +1 @@
+hello
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
--- a/gone.txt
+++ /dev/null
@@ -1 +0,0 @@
-bye
diff --git a/a b.txt b/c d.txt
similarity index 100%
rename from a b.txt
rename to c d.txt
diff --git a/logo.png b/logo.png
index 4444444..5555555 100644
Binary files a/logo.png and b/logo.png differ
";
        let files = parse_diff(output);
        assert_eq!(files.len(), 5);
        assert_eq!(files[0].path, "src/main.rs");
        assert_eq!(files[0].status, FileStatus::Modified);
        assert_eq!((files[0].additions, files[0].deletions), (1, 1));
        assert_eq!(files[0].lines.len(), 4);
        assert_eq!(files[0].lines[0].kind, LineKind::Hunk);
        assert_eq!(files[1].status, FileStatus::Added);
        assert_eq!(files[1].additions, 1);
        assert_eq!(files[2].status, FileStatus::Deleted);
        assert_eq!(files[2].path, "gone.txt");
        assert_eq!(files[3].status, FileStatus::Renamed);
        assert_eq!(files[3].path, "c d.txt");
        assert_eq!(files[3].old_path.as_deref(), Some("a b.txt"));
        assert!(files[4].binary);
    }
}
