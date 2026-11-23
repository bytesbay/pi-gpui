//! The session search page (⌘⇧F), modeled on Zed's project search: a query
//! field with match case, whole word and regex toggles, and the matching
//! lines grouped by session. Enter or a click opens the session at the line.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui::{
    AnyElement, App, Context, ElementId, Entity, Focusable, FontWeight, HighlightStyle,
    ScrollHandle, SharedString, StyledText, Subscription, Task, Window, div, px,
};

use super::{
    CloseSessionSearch, HubView, SearchSessions, ToggleSearchCase, ToggleSearchRegex,
    ToggleSearchWord,
};
use crate::session_search::{self, Query, Results, Role};
use crate::sessions;
use crate::text_input::{self, TextInput};
use crate::ui::prelude::*;

pub(super) const SEARCH_CONTEXT: &str = "SessionSearch";
/// Typing pauses this long before a search starts.
const DEBOUNCE: Duration = Duration::from_millis(150);

pub(super) struct SearchPage {
    input: Entity<TextInput>,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
    /// The query the current results (or search) belong to.
    query: Query,
    results: Option<Results>,
    searching: bool,
    error: Option<String>,
    /// Index into the flat list of matching lines.
    selected: usize,
    scroll: ScrollHandle,
    scroll_to_selected: bool,
    /// Set to stop the running search when the query changes.
    cancel: Arc<AtomicBool>,
    _task: Task<()>,
    _input_subscription: Subscription,
}

impl SearchPage {
    fn current_query(&self, cx: &App) -> Query {
        Query {
            text: self.input.read(cx).text().to_string(),
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            regex: self.regex,
        }
    }

    /// `(session, line)` of every shown matching line, in order.
    fn hit_positions(&self) -> Vec<(usize, usize)> {
        self.results
            .iter()
            .flat_map(|results| results.sessions.iter().enumerate())
            .flat_map(|(session, found)| (0..found.hits.len()).map(move |hit| (session, hit)))
            .collect()
    }
}

impl HubView {
    pub(super) fn open_session_search(
        &mut self,
        _: &SearchSessions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.search.is_none() {
            let input = cx.new(|cx| TextInput::single_line("Search sessions…", cx));
            let subscription = cx.observe(&input, |this, _, cx| this.update_search(cx));
            self.search = Some(SearchPage {
                input,
                case_sensitive: false,
                whole_word: false,
                regex: false,
                query: Query::default(),
                results: None,
                searching: false,
                error: None,
                selected: 0,
                scroll: ScrollHandle::new(),
                scroll_to_selected: false,
                cancel: Arc::new(AtomicBool::new(false)),
                _task: Task::ready(()),
                _input_subscription: subscription,
            });
        }
        self.settings_page = None;
        self.search_open = true;
        if let Some(page) = &self.search {
            let input = page.input.clone();
            input.update(cx, |input, cx| input.select_all_text(cx));
            window.focus(&input.read(cx).focus_handle(cx));
        }
        cx.notify();
    }

    pub(super) fn close_session_search(&mut self, cx: &mut Context<Self>) {
        if self.search_open {
            self.search_open = false;
            self.focus_active_chat = true;
            cx.notify();
        }
    }

    fn toggle_search_option(&mut self, option: fn(&mut SearchPage), cx: &mut Context<Self>) {
        if let Some(page) = &mut self.search {
            option(page);
            self.update_search(cx);
            cx.notify();
        }
    }

    /// Starts a search when the query or an option changed.
    fn update_search(&mut self, cx: &mut Context<Self>) {
        let workspaces: Vec<PathBuf> = self
            .workspaces
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        let Some(page) = &mut self.search else {
            return;
        };
        let query = page.current_query(cx);
        if query == page.query {
            return;
        }
        page.query = query.clone();
        page.cancel.store(true, Ordering::Relaxed);
        page.selected = 0;
        page.scroll_to_selected = true;
        let regex = match query.compile() {
            Ok(Some(regex)) => regex,
            Ok(None) => {
                page.results = None;
                page.searching = false;
                page.error = None;
                page._task = Task::ready(());
                cx.notify();
                return;
            }
            Err(error) => {
                page.searching = false;
                page.error = Some(error);
                page._task = Task::ready(());
                cx.notify();
                return;
            }
        };
        page.error = None;
        page.searching = true;
        let cancel = Arc::new(AtomicBool::new(false));
        page.cancel = cancel.clone();
        page._task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let results = cx
                .background_spawn({
                    let cancel = cancel.clone();
                    async move { session_search::search(&workspaces, &regex, &cancel) }
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(page) = &mut this.search
                    && Arc::ptr_eq(&page.cancel, &cancel)
                {
                    page.results = Some(results);
                    page.searching = false;
                    page.selected = 0;
                    page.scroll_to_selected = true;
                    cx.notify();
                }
            })
            .ok();
        });
        cx.notify();
    }

    fn move_search_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(page) = &mut self.search else {
            return;
        };
        let count = page.hit_positions().len();
        if count == 0 {
            return;
        }
        page.selected = (page.selected as isize + delta).clamp(0, count as isize - 1) as usize;
        page.scroll_to_selected = true;
        cx.notify();
    }

    fn open_selected_hit(&mut self, cx: &mut Context<Self>) {
        let Some(page) = &self.search else {
            return;
        };
        let Some(&(session, hit)) = page.hit_positions().get(page.selected) else {
            return;
        };
        self.open_search_result(session, Some(hit), cx);
    }

    /// Opens a session from the results, scrolled to `hit` when given.
    fn open_search_result(&mut self, session: usize, hit: Option<usize>, cx: &mut Context<Self>) {
        let Some(found) = self
            .search
            .as_ref()
            .and_then(|page| page.results.as_ref())
            .and_then(|results| results.sessions.get(session))
        else {
            return;
        };
        let workspace = found.workspace.clone();
        let path = found.path.clone();
        let line = hit
            .and_then(|hit| found.hits.get(hit))
            .map(|hit| hit.line.clone());
        if let Some(page) = &mut self.search
            && let Some(hit) = hit
        {
            page.selected = page
                .hit_positions()
                .iter()
                .position(|position| *position == (session, hit))
                .unwrap_or(page.selected);
        }
        self.search_open = false;

        let open = self
            .workspaces
            .iter()
            .flat_map(|entry| entry.chats.iter())
            .find(|chat| chat.view.read(cx).session_file() == Some(path.as_path()))
            .map(|chat| chat.view.clone());
        if let Some(view) = open {
            if let Some(line) = line {
                view.update(cx, |view, cx| view.reveal_line(line, cx));
            }
            self.activate(view, cx);
            return;
        }
        self.pending_reveal = line.map(|line| (path.clone(), line));
        self.open_session(&workspace, path, cx);
        cx.notify();
    }

    pub(super) fn render_search_page(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let page = self.search.as_ref()?;
        let scroll = page.scroll.clone();
        let scroll_to_selected = page.scroll_to_selected;
        let input = page.input.clone();
        let colors = cx.theme().colors.clone();

        let toggle = |id: &'static str,
                      label: &'static str,
                      on: bool,
                      tip: &'static str,
                      keys: &'static str,
                      option: fn(&mut SearchPage),
                      cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex_none()
                .h(px(22.))
                .min_w(px(26.))
                .px(px(5.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .border_1()
                .cursor_pointer()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .when(on, |this| {
                    this.bg(colors.element_selected)
                        .border_color(colors.border)
                        .text_color(colors.text)
                })
                .when(!on, |this| {
                    this.border_color(gpui::transparent_black())
                        .text_color(colors.text_muted)
                        .hover(|style| style.bg(colors.ghost_element_hover))
                })
                .tooltip(Tooltip::with_keys(tip, keys))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_search_option(option, cx)))
                .child(label)
        };

        let field = h_flex()
            .gap(px(4.))
            .pl(px(10.))
            .pr(px(4.))
            .py(px(4.))
            .rounded(px(6.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.editor_background)
            .child(
                Icon::new(IconName::Search)
                    .size(IconSize::Small)
                    .color(Color::Muted),
            )
            .child(div().flex_1().min_w(px(0.)).child(input.clone()))
            .child(toggle(
                "search-case",
                "Aa",
                page.case_sensitive,
                "Match case",
                "⌥⌘C",
                |page| page.case_sensitive = !page.case_sensitive,
                cx,
            ))
            .child(toggle(
                "search-word",
                "ab",
                page.whole_word,
                "Match whole words",
                "⌥⌘W",
                |page| page.whole_word = !page.whole_word,
                cx,
            ))
            .child(toggle(
                "search-regex",
                ".*",
                page.regex,
                "Use regular expression",
                "⌥⌘X",
                |page| page.regex = !page.regex,
                cx,
            ));

        let status: AnyElement = if let Some(error) = &page.error {
            Label::new(format!("Invalid regular expression: {error}"))
                .size(LabelSize::Small)
                .color(Color::Error)
                .into_any_element()
        } else if page.query.text.is_empty() {
            Label::new(
                "Searches what you wrote and what pi answered, in the sessions of every project in the sidebar. Tool output isn't searched.",
            )
            .size(LabelSize::Small)
            .color(Color::Muted)
            .into_any_element()
        } else {
            let summary = match &page.results {
                Some(results) if results.matches == 0 => "No matches".to_string(),
                Some(results) => {
                    let mut text = format!(
                        "{} {} in {} {}",
                        thousands(results.matches),
                        if results.matches == 1 {
                            "match"
                        } else {
                            "matches"
                        },
                        results.sessions.len(),
                        if results.sessions.len() == 1 {
                            "session"
                        } else {
                            "sessions"
                        },
                    );
                    if results.truncated {
                        text.push_str(&format!(
                            ". Stopped at {}: narrow the search to see the rest",
                            thousands(session_search::MAX_MATCHES)
                        ));
                    }
                    text
                }
                None => String::new(),
            };
            h_flex()
                .gap(px(6.))
                .when(page.searching, |this| {
                    this.child(Spinner::new().size(IconSize::XSmall))
                        .child(LoadingLabel::new("Searching").size(LabelSize::Small))
                })
                .when(!summary.is_empty() && !page.searching, |this| {
                    this.child(
                        Label::new(summary)
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                })
                .into_any_element()
        };

        let highlight = HighlightStyle {
            background_color: Some(colors.text_accent.opacity(0.28)),
            color: Some(colors.text),
            ..HighlightStyle::default()
        };
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut selected_child = None;
        let mut position = 0;
        for (session_index, found) in page
            .results
            .iter()
            .filter(|_| page.error.is_none())
            .flat_map(|results| results.sessions.iter().enumerate())
        {
            let project = self
                .workspaces
                .iter()
                .find(|entry| entry.path == found.workspace)
                .map(|entry| entry.name.clone())
                .unwrap_or_default();
            rows.push(
                h_flex()
                    .id(ElementId::Name(
                        format!("search-session:{session_index}").into(),
                    ))
                    .gap(px(8.))
                    .mt(px(if session_index == 0 { 4. } else { 14. }))
                    .px(px(10.))
                    .py(px(4.))
                    .rounded(px(5.))
                    .cursor_pointer()
                    .hover(|style| style.bg(colors.ghost_element_hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_search_result(session_index, None, cx)
                    }))
                    .child(
                        Icon::new(IconName::Message)
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(
                        div().flex_1().min_w(px(0.)).child(
                            Label::new(found.title.clone())
                                .size(LabelSize::Small)
                                .weight(FontWeight::MEDIUM)
                                .truncate(),
                        ),
                    )
                    .child(
                        Label::new(format!(
                            "{project} · {} · {}",
                            match found.matches {
                                1 => "1 match".to_string(),
                                matches => format!("{} matches", thousands(matches)),
                            },
                            sessions::relative_age(found.modified)
                        ))
                        .size(LabelSize::XSmall)
                        .color(Color::Placeholder),
                    )
                    .into_any_element(),
            );
            for (hit_index, hit) in found.hits.iter().enumerate() {
                let selected = position == page.selected;
                if selected {
                    selected_child = Some(rows.len());
                }
                position += 1;
                let excerpt = SharedString::from(hit.excerpt.clone());
                let ranges: Vec<_> = hit
                    .ranges
                    .iter()
                    .map(|range| (range.clone(), highlight))
                    .collect();
                rows.push(
                    h_flex()
                        .id(ElementId::Name(
                            format!("search-hit:{session_index}:{hit_index}").into(),
                        ))
                        .items_start()
                        .gap(px(10.))
                        .ml(px(22.))
                        .px(px(10.))
                        .py(px(4.))
                        .rounded(px(5.))
                        .cursor_pointer()
                        .when(selected, |this| this.bg(colors.element_selected))
                        .when(!selected, |this| {
                            this.hover(|style| style.bg(colors.ghost_element_hover))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_search_result(session_index, Some(hit_index), cx)
                        }))
                        .child(
                            div().w(px(26.)).flex_none().pt(px(1.)).child(
                                Label::new(match hit.role {
                                    Role::User => "you",
                                    Role::Assistant => "pi",
                                })
                                .size(LabelSize::XSmall)
                                .color(Color::Placeholder),
                            ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .text_size(px(13.))
                                .line_height(px(19.))
                                .text_color(colors.text_muted)
                                .child(StyledText::new(excerpt).with_highlights(ranges)),
                        )
                        .into_any_element(),
                );
            }
            if found.hits.len() >= session_search::LINES_PER_SESSION {
                rows.push(
                    div()
                        .ml(px(58.))
                        .py(px(2.))
                        .child(
                            Label::new("More lines match in this session; open it to see them.")
                                .size(LabelSize::XSmall)
                                .color(Color::Placeholder),
                        )
                        .into_any_element(),
                );
            }
        }
        if let Some(child) = selected_child
            && scroll_to_selected
        {
            scroll.scroll_to_item(child);
        }
        if let Some(page) = &mut self.search {
            page.scroll_to_selected = false;
        }

        let header = h_flex()
            .gap(px(12.))
            .child(
                div()
                    .flex_1()
                    .text_size(px(20.))
                    .line_height(px(28.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Search sessions"),
            )
            .child(
                IconButton::new("close-search", IconName::Close)
                    .tooltip(Tooltip::with_keys("Close search", "esc"))
                    .on_click(cx.listener(|this, _, _, cx| this.close_session_search(cx))),
            );

        Some(
            v_flex()
                .id("search-page")
                .key_context(SEARCH_CONTEXT)
                .flex_1()
                .min_h(px(0.))
                .on_action(
                    cx.listener(|this, _: &CloseSessionSearch, _, cx| {
                        this.close_session_search(cx)
                    }),
                )
                .on_action(cx.listener(|this, _: &ToggleSearchCase, _, cx| {
                    this.toggle_search_option(|page| page.case_sensitive = !page.case_sensitive, cx)
                }))
                .on_action(cx.listener(|this, _: &ToggleSearchWord, _, cx| {
                    this.toggle_search_option(|page| page.whole_word = !page.whole_word, cx)
                }))
                .on_action(cx.listener(|this, _: &ToggleSearchRegex, _, cx| {
                    this.toggle_search_option(|page| page.regex = !page.regex, cx)
                }))
                .on_action(
                    cx.listener(|this, _: &text_input::Up, _, cx| {
                        this.move_search_selection(-1, cx)
                    }),
                )
                .on_action(cx.listener(|this, _: &text_input::Down, _, cx| {
                    this.move_search_selection(1, cx)
                }))
                .on_action(
                    cx.listener(|this, _: &crate::chat::Submit, _, cx| this.open_selected_hit(cx)),
                )
                .child(
                    v_flex()
                        .flex_none()
                        .w_full()
                        .max_w(px(860.))
                        .mx_auto()
                        .px(px(32.))
                        .pt(px(28.))
                        .pb(px(12.))
                        .gap(px(12.))
                        .child(header)
                        .child(field)
                        .child(status),
                )
                .child(
                    // Rows are direct children so `scroll_to_item` can find them.
                    v_flex()
                        .id("search-results")
                        .flex_1()
                        .min_h(px(0.))
                        .items_center()
                        .px(px(22.))
                        .pb(px(28.))
                        .overflow_y_scroll()
                        .track_scroll(&scroll)
                        .children(
                            rows.into_iter()
                                .map(|row| div().w_full().max_w(px(816.)).child(row)),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// 12345 → "12,345".
fn thousands(number: usize) -> String {
    let digits = number.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}
