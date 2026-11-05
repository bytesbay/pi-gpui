//! The slash command menu above the composer and the session tree overlay.

use gpui::{ClickEvent, Hsla, Pixels, Point, relative};

use super::tree::{Gutter, NodeKind, TreeLine};
use super::*;

const TREE_LINE_HEIGHT: f32 = 26.;
const TREE_GUTTER_WIDTH: f32 = 16.;

impl ChatView {
    pub(super) fn render_slash_menu(
        &self,
        matches: &[usize],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let highlighted = self.slash_highlight.min(matches.len().saturating_sub(1));
        let rows = matches.iter().enumerate().map(|(position, &index)| {
            let command = &self.slash_commands[index];
            let active = position == highlighted;
            h_flex()
                .id(("slash-command", index))
                .h(px(30.))
                .px(px(10.))
                .gap(px(8.))
                .rounded(px(5.))
                .cursor_pointer()
                .when(active, |this| this.bg(colors.element_active))
                .hover(|style| style.bg(colors.element_hover))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.accept_slash(index, true, window, cx)
                }))
                .child(
                    h_flex()
                        .flex_none()
                        .gap(px(4.))
                        .child(
                            Label::new(format!("/{}", command.name))
                                .size(LabelSize::Small)
                                .weight(FontWeight::MEDIUM),
                        )
                        .when_some(command.arguments, |this, arguments| {
                            this.child(
                                Label::new(arguments)
                                    .size(LabelSize::Small)
                                    .color(Color::Placeholder),
                            )
                        }),
                )
                .child(
                    div().flex_1().min_w(px(0.)).child(
                        Label::new(command.description.clone())
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .truncate(),
                    ),
                )
                .when_some(command.keys, |this, keys| this.child(KeyHint::new(keys)))
                .child(
                    Label::new(command.source.label())
                        .size(LabelSize::XSmall)
                        .color(Color::Placeholder),
                )
        });

        v_flex()
            .absolute()
            .left_0()
            .right_0()
            .bottom_full()
            .mb(px(6.))
            .occlude()
            .rounded(px(8.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.elevated_surface_background)
            .shadow_lg()
            .overflow_hidden()
            .child(
                div()
                    .id("slash-commands")
                    .max_h(px(280.))
                    .overflow_y_scroll()
                    .track_scroll(&self.slash_scroll)
                    .p(px(4.))
                    .children(rows),
            )
            .child(
                h_flex()
                    .gap(px(12.))
                    .px(px(12.))
                    .py(px(6.))
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .child(hint("↑↓", "Select"))
                    .child(hint("Tab", "Complete"))
                    .child(hint("Enter", "Run"))
                    .child(hint("Esc", "Close")),
            )
            .into_any_element()
    }

    pub(super) fn render_tree(&self, view: &TreeView, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let body = match &view.lines {
            None => h_flex()
                .flex_1()
                .justify_center()
                .py(px(40.))
                .gap(px(8.))
                .child(Spinner::new().size(IconSize::Small).color(Color::Muted))
                .child(LoadingLabel::new("Loading the session tree…").size(LabelSize::Small))
                .into_any_element(),
            Some(lines) if lines.is_empty() => h_flex()
                .flex_1()
                .justify_center()
                .py(px(40.))
                .child(
                    Label::new("Nothing here yet: send a message first.")
                        .size(LabelSize::Small)
                        .color(Color::Placeholder),
                )
                .into_any_element(),
            Some(lines) => div()
                .id("tree-lines")
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .track_scroll(&view.scroll)
                .p(px(6.))
                .children(lines.iter().enumerate().map(|(index, line)| {
                    self.render_tree_line(index, line, index == view.selected, cx)
                }))
                .into_any_element(),
        };

        let lines = view.lines.as_deref().unwrap_or_default();
        let branch_points = lines
            .iter()
            .filter(|line| line.gutter.last() == Some(&Gutter::Corner))
            .count();
        let subtitle = match branch_points {
            0 => format!("{} entries, no branches", lines.len()),
            1 => format!("{} entries, 1 branch point", lines.len()),
            count => format!("{} entries, {count} branch points", lines.len()),
        };
        let selected = lines.get(view.selected);
        let preview = selected.map(|line| {
            let (title, text) = match line.kind {
                NodeKind::User => ("Your message", line.text.as_str()),
                NodeKind::Assistant => ("Answer", line.text.as_str()),
                NodeKind::Compaction => ("Compaction summary", line.text.as_str()),
                NodeKind::BranchSummary => ("Branch summary", line.text.as_str()),
            };
            v_flex()
                .id("tree-preview")
                .flex_none()
                .max_h(px(150.))
                .overflow_y_scroll()
                .gap(px(4.))
                .px(px(16.))
                .py(px(10.))
                .border_t_1()
                .border_color(colors.border_variant)
                .child(
                    Label::new(title)
                        .size(LabelSize::XSmall)
                        .color(Color::Placeholder),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .line_height(px(19.))
                        .text_color(colors.text)
                        .child(truncate_chars(text, 3_000)),
                )
        });
        let can_fork = selected.is_some_and(|line| line.kind == NodeKind::User);

        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(0.45))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close_tree(window, cx)),
            )
            .child(
                v_flex()
                    .key_context(TREE_CONTEXT)
                    .track_focus(&view.focus)
                    .w(px(760.))
                    .max_w(relative(0.92))
                    .h(relative(0.82))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.elevated_surface_background)
                    .shadow_lg()
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        h_flex()
                            .flex_none()
                            .gap(px(8.))
                            .px(px(16.))
                            .py(px(10.))
                            .border_b_1()
                            .border_color(colors.border_variant)
                            .child(
                                Icon::new(IconName::ListTree)
                                    .size(IconSize::Small)
                                    .color(Color::Muted),
                            )
                            .child(Label::new("Session tree").weight(FontWeight::MEDIUM))
                            .child(
                                Label::new(subtitle)
                                    .size(LabelSize::Small)
                                    .color(Color::Placeholder),
                            )
                            .child(div().flex_1())
                            .child(KeyHint::new("⌘⇧T")),
                    )
                    .child(body)
                    .children(preview)
                    .child(
                        h_flex()
                            .flex_none()
                            .flex_wrap()
                            .gap_x(px(14.))
                            .gap_y(px(4.))
                            .px(px(16.))
                            .py(px(8.))
                            .border_t_1()
                            .border_color(colors.border_variant)
                            .child(hint("↑↓", "Move"))
                            .child(hint("←→", "Branches"))
                            .child(hint("Enter", "Go here"))
                            .child(hint("⌥Enter", "Go here, summarize the branch you leave"))
                            .when(can_fork, |this| {
                                this.child(hint("⌘Enter", "Fork into a new session"))
                            })
                            .child(hint("Esc", "Close")),
                    ),
            )
            .into_any_element()
    }

    fn render_tree_line(
        &self,
        index: usize,
        line: &TreeLine,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let connector = colors.border;
        let kind_color = match line.kind {
            NodeKind::User => colors.text_accent,
            NodeKind::Assistant => colors.text_muted,
            NodeKind::Compaction | NodeKind::BranchSummary => colors.warning,
        };
        let dot = if line.current {
            div()
                .size(px(10.))
                .rounded_full()
                .border_2()
                .border_color(colors.text_accent)
                .bg(colors.elevated_surface_background)
        } else if line.active {
            div().size(px(8.)).rounded_full().bg(kind_color)
        } else {
            div()
                .size(px(8.))
                .rounded_full()
                .border_1()
                .border_color(kind_color)
        };
        let text = match line.kind {
            NodeKind::User | NodeKind::Assistant => sessions::one_line(&line.text, 200),
            NodeKind::Compaction => "Context compacted".to_string(),
            NodeKind::BranchSummary => {
                format!("Branch summary: {}", sessions::one_line(&line.text, 160))
            }
        };
        let text_color = match (line.active, line.kind) {
            (false, _) => Color::Placeholder,
            (true, NodeKind::User) => Color::Default,
            (true, _) => Color::Muted,
        };

        h_flex()
            .id(("tree-line", index))
            .h(px(TREE_LINE_HEIGHT))
            .px(px(6.))
            .rounded(px(5.))
            .cursor_pointer()
            .when(selected, |this| this.bg(colors.element_active))
            .hover(|style| style.bg(colors.element_hover))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.select_tree_line(index, cx);
                if event.click_count() >= 2 {
                    this.navigate_to_selected(false, window, cx);
                }
            }))
            .children(line.gutter.iter().map(|cell| gutter_cell(*cell, connector)))
            .child(
                div()
                    .w(px(14.))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .child(dot),
            )
            .child(div().w(px(6.)).flex_none())
            .when(line.kind == NodeKind::User, |this| {
                this.child(
                    div().flex_none().mr(px(6.)).child(
                        Label::new("You")
                            .size(LabelSize::XSmall)
                            .color(if line.active {
                                Color::Accent
                            } else {
                                Color::Placeholder
                            }),
                    ),
                )
            })
            .child(
                div().flex_1().min_w(px(0.)).child(
                    Label::new(text)
                        .size(LabelSize::Small)
                        .color(text_color)
                        .truncate(),
                ),
            )
            .when_some(line.label.clone(), |this, label| {
                this.child(
                    div()
                        .flex_none()
                        .ml(px(6.))
                        .px(px(6.))
                        .rounded(px(4.))
                        .bg(colors.element_background)
                        .child(
                            Label::new(label)
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        ),
                )
            })
            .when(line.current, |this| {
                this.child(
                    div()
                        .flex_none()
                        .ml(px(6.))
                        .px(px(6.))
                        .rounded(px(4.))
                        .bg(colors.info_background)
                        .child(
                            Label::new("current")
                                .size(LabelSize::XSmall)
                                .color(Color::Accent),
                        ),
                )
            })
            .into_any_element()
    }
}

/// One column of the branch drawing: `│`, `├─`, `└─` or empty.
fn gutter_cell(cell: Gutter, color: Hsla) -> AnyElement {
    let middle = TREE_LINE_HEIGHT / 2.;
    let vertical = |height: f32| {
        div()
            .absolute()
            .left(px(7.))
            .top_0()
            .w(px(1.))
            .h(px(height))
            .bg(color)
    };
    let horizontal = || {
        div()
            .absolute()
            .left(px(7.))
            .top(px(middle))
            .w(px(TREE_GUTTER_WIDTH - 7.))
            .h(px(1.))
            .bg(color)
    };
    let container = div()
        .relative()
        .flex_none()
        .w(px(TREE_GUTTER_WIDTH))
        .h(px(TREE_LINE_HEIGHT));
    match cell {
        Gutter::Blank => container,
        Gutter::Pipe => container.child(vertical(TREE_LINE_HEIGHT)),
        Gutter::Tee => container
            .child(vertical(TREE_LINE_HEIGHT))
            .child(horizontal()),
        Gutter::Corner => container.child(vertical(middle)).child(horizontal()),
    }
    .into_any_element()
}

fn hint(keys: &'static str, label: &'static str) -> AnyElement {
    h_flex()
        .gap(px(4.))
        .child(KeyHint::new(keys))
        .child(
            Label::new(label)
                .size(LabelSize::XSmall)
                .color(Color::Placeholder),
        )
        .into_any_element()
}

impl ChatView {
    /// A small ring filled to the share of the context window in use; a click
    /// opens the usage popover.
    pub(super) fn render_usage_ring(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let stats = self.usage_stats.as_ref()?;
        let colors = cx.theme().colors.clone();
        let fraction = stats.context_fraction().unwrap_or(0.);
        let compact_at = self.compact_at_fraction();
        let fill = if compact_at.is_some_and(|at| fraction >= at - 0.05) || fraction >= 0.9 {
            colors.warning
        } else {
            colors.text_muted
        };
        let track = colors.border;
        let ring = gpui::canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let center = bounds.center();
                let radius = bounds.size.width.min(bounds.size.height) / 2. - px(1.5);
                let point_at = |turn: f32| {
                    let angle = turn * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                    gpui::point(
                        center.x + radius * angle.cos(),
                        center.y + radius * angle.sin(),
                    )
                };
                let arc = |from: f32, to: f32| -> Vec<Point<Pixels>> {
                    let steps = ((to - from) * 48.).ceil().max(1.) as usize;
                    (0..=steps)
                        .map(|step| point_at(from + (to - from) * step as f32 / steps as f32))
                        .collect()
                };
                let mut background = gpui::PathBuilder::stroke(px(2.));
                background.add_polygon(&arc(0., 1.), true);
                if let Ok(path) = background.build() {
                    window.paint_path(path, track);
                }
                if fraction > 0.005 {
                    let mut progress = gpui::PathBuilder::stroke(px(2.));
                    progress.add_polygon(&arc(0., fraction), false);
                    if let Ok(path) = progress.build() {
                        window.paint_path(path, fill);
                    }
                }
            },
        )
        .size(px(14.));

        let button = h_flex()
            .id("usage-ring")
            .size(px(22.))
            .justify_center()
            .rounded(px(5.))
            .cursor_pointer()
            .when(self.usage_open, |this| {
                this.bg(colors.ghost_element_selected)
            })
            .hover(|style| style.bg(colors.ghost_element_hover))
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .on_click(cx.listener(|this, _, _, cx| this.toggle_usage(cx)))
            .when(!self.usage_open, |this| {
                this.tooltip(Tooltip::text(match stats.context_fraction() {
                    Some(fraction) => format!("Context {:.0}% used", fraction * 100.),
                    None => "Context usage".to_string(),
                }))
            })
            .child(ring);

        let popover = self.usage_open.then(|| self.render_usage_popover(cx));
        Some(
            div()
                .relative()
                .flex_none()
                .child(button)
                .when_some(popover, |this, popover| {
                    this.child(
                        div().absolute().top_0().left_0().child(
                            deferred(
                                anchored()
                                    .anchor(Corner::BottomLeft)
                                    .snap_to_window_with_margin(px(8.))
                                    .child(div().pb(px(6.)).child(popover)),
                            )
                            .with_priority(1),
                        ),
                    )
                })
                .into_any_element(),
        )
    }

    /// Where pi compacts, as a share of the window.
    fn compact_at_fraction(&self) -> Option<f32> {
        let window = self.usage_stats.as_ref()?.context_window?;
        let reserve = self.compaction_reserve?;
        (window > reserve).then(|| (window - reserve) as f32 / window as f32)
    }

    fn render_usage_popover(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let stats = self.usage_stats.clone().unwrap_or_default();
        let extra = self.usage_extra.clone().unwrap_or_default();
        let now = usage::now_ms();

        let title = |text: &'static str| {
            Label::new(text)
                .size(LabelSize::Small)
                .color(Color::Placeholder)
                .into_any_element()
        };
        let row = |label: String, value: String| {
            h_flex()
                .justify_between()
                .gap(px(16.))
                .child(Label::new(label).size(LabelSize::Small).color(Color::Muted))
                .child(Label::new(value).size(LabelSize::Small).color(Color::Muted))
                .into_any_element()
        };
        let section = |children: Vec<AnyElement>| v_flex().gap(px(4.)).children(children);
        let divider = || div().h(px(1.)).bg(colors.border_variant);

        let mut sections: Vec<AnyElement> = Vec::new();

        // Context window.
        let headline = match (stats.context_tokens, stats.context_window) {
            (Some(tokens), Some(window)) => format!(
                "{:.0}% used · {} / {} tokens",
                stats.context_fraction().unwrap_or(0.) * 100.,
                format_tokens(tokens),
                format_tokens(window)
            ),
            (None, Some(window)) => format!(
                "Known after the next answer · {} tokens",
                format_tokens(window)
            ),
            _ => "Unknown for this model".to_string(),
        };
        let compaction = match self.compact_at_fraction() {
            Some(at) => format!("Compacts automatically at {:.0}%", at * 100.),
            None if self.compaction_reserve.is_none() && stats.context_window.is_some() => {
                "Automatic compaction is off".to_string()
            }
            None => String::new(),
        };
        sections.push(
            section(vec![
                title("Context window"),
                Label::new(headline)
                    .weight(FontWeight::MEDIUM)
                    .into_any_element(),
            ])
            .when(!compaction.is_empty(), |this| {
                this.child(
                    Label::new(compaction)
                        .size(LabelSize::Small)
                        .color(Color::Placeholder),
                )
            })
            .into_any_element(),
        );

        // Prompt cache.
        let mut cache_rows = Vec::new();
        if let Some(fraction) = extra
            .last_turn
            .as_ref()
            .and_then(usage::Turn::cached_fraction)
        {
            cache_rows.push(row(
                "Last turn".into(),
                format!("{:.0}% cached", fraction * 100.),
            ));
        }
        if let Some(left) = extra.cache_left(now) {
            cache_rows.push(row(
                "Expires in".into(),
                if left.is_zero() {
                    "expired".to_string()
                } else {
                    usage::format_countdown(left)
                },
            ));
        }
        if !cache_rows.is_empty() {
            let mut children = vec![title("Prompt cache")];
            children.extend(cache_rows);
            sections.push(section(children).into_any_element());
        }

        // Plan limits.
        if !extra.limits.is_empty() {
            let mut children = vec![title("Plan limits")];
            for limit in &extra.limits {
                let resets = limit
                    .resets_at
                    .map(|at| {
                        format!(
                            "resets in {}",
                            usage::format_until(std::time::Duration::from_millis(
                                at.saturating_sub(now)
                            ))
                        )
                    })
                    .unwrap_or_default();
                children.push(row(
                    format!(
                        "{:.0}% of {}",
                        limit.used_percent,
                        usage::limit_name(limit.window_minutes)
                    ),
                    resets,
                ));
            }
            sections.push(section(children).into_any_element());
        }

        // This session.
        sections.push(
            section(vec![
                title("This session"),
                row(
                    "Input / Output".into(),
                    format!(
                        "{} / {}",
                        format_tokens(stats.input),
                        format_tokens(stats.output)
                    ),
                ),
                row(
                    "Cache read / write".into(),
                    format!(
                        "{} / {}",
                        format_tokens(stats.cache_read),
                        format_tokens(stats.cache_write)
                    ),
                ),
                row(
                    "Cost".into(),
                    if extra.subscription {
                        "Subscription".to_string()
                    } else {
                        format!("${:.2}", stats.cost)
                    },
                ),
            ])
            .into_any_element(),
        );

        let mut body = v_flex().gap(px(12.));
        for (index, section) in sections.into_iter().enumerate() {
            if index > 0 {
                body = body.child(divider());
            }
            body = body.child(section);
        }

        v_flex()
            .id("usage-popover")
            .w(px(300.))
            .p(px(14.))
            .rounded(px(10.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.elevated_surface_background)
            .shadow_lg()
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_usage(cx)))
            .child(body)
            .into_any_element()
    }
}
