//! The agents panel, on the right as the notes are, shown by its button at
//! the top right or ⌘⇧A: every Claude thread on every host, the most
//! urgent first, each with what it runs to help it under it, as Herdr's
//! agents list. A click opens one.

use super::sidebar::{claude_icon, status_dot, thread_icon};
use super::{Key, Lemur, key, sessions, worktree_label};
use crate::snapshot::{Activity, Helper, Session};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

/// Renders `Lemur::agents_panel`. As a cached view, its working dots'
/// animation doesn't redraw the rest of the window.
pub(super) struct AgentsView {
    lemur: WeakEntity<Lemur>,
}

impl AgentsView {
    pub(super) fn new(lemur: WeakEntity<Lemur>) -> Self {
        AgentsView { lemur }
    }
}

impl Render for AgentsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.lemur.upgrade() {
            Some(lemur) => lemur.update(cx, |lemur, cx| lemur.agents_panel(cx).into_any_element()),
            None => div().into_any_element(),
        }
    }
}

impl Lemur {
    /// The Claude threads, and threads running helpers, in the order the
    /// panel lists them: needing you, done while you looked away, working,
    /// then the rest; the latest first in each.
    pub(super) fn agents(&self) -> Vec<(&str, &Session)> {
        let mut agents: Vec<(&str, &Session)> = sessions(&self.snap)
            .filter(|(_, s)| s.runs_claude() || !s.helpers.is_empty())
            .collect();
        agents.sort_by_key(|(h, s)| {
            let rank = match s.activity {
                Activity::NeedsInput => 0,
                _ if self.done.contains(&key(h, s)) => 1,
                Activity::Working => 2,
                _ => 3,
            };
            (rank, s.idle_secs)
        });
        agents
    }

    /// ⌘] and ⌘[: shows the next agent, or the one before, in the agents
    /// panel's order, as Herdr's do: from the thread on show, or the first.
    pub(super) fn next_agent(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let agents: Vec<Key> = self.agents().into_iter().map(|(h, s)| key(h, s)).collect();
        if agents.is_empty() {
            self.note("No Claude threads".into(), cx);
            return;
        }
        let at = self
            .active
            .as_ref()
            .and_then(|a| agents.iter().position(|k| k == a));
        let n = agents.len();
        let next = match (at, back) {
            (Some(i), false) => (i + 1) % n,
            (Some(i), true) => (i + n - 1) % n,
            (None, _) => 0,
        };
        let (host, session) = agents[next].clone();
        self.open(host, session, window, cx);
    }

    fn agents_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let agents = self.agents();
        let rows: Vec<AnyElement> = agents
            .iter()
            .enumerate()
            .flat_map(|(i, (host, s))| {
                let mut rows = vec![self.agent_row(i, host, s, cx)];
                rows.extend(
                    s.helpers
                        .iter()
                        .enumerate()
                        .map(|(j, h)| self.helper_row(i, j, host, s, h, cx)),
                );
                rows
            })
            .collect();
        let body = if rows.is_empty() {
            div()
                .p_3()
                .text_sm()
                .text_color(muted)
                .child("No Claude threads. Their subagents show here too while they work.")
                .into_any_element()
        } else {
            v_flex()
                .id("agents-list")
                .flex_1()
                .min_h_0()
                .p_2()
                .gap_0p5()
                .overflow_y_scroll()
                .children(rows)
                .into_any_element()
        };
        // No title: the Agents button over it says what it is.
        v_flex().size_full().child(body)
    }

    /// A Claude thread: its name, where it runs, how long since it last
    /// said anything, and what it's doing.
    fn agent_row(&self, i: usize, host: &str, s: &Session, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let k = key(host, s);
        let shown = self.active.as_ref() == Some(&k);
        let (color, tip) = self.activity(&k, s, cx);
        let mut place = match self.project_of(host, &s.name) {
            Some((base, w)) => format!("{} · {}", self.cfg.bases[base].name(), worktree_label(w)),
            None => host.to_string(),
        };
        if let Some(issue) = &s.issue {
            place = format!("{place} · {issue}");
        }
        let open = {
            let (host, name) = (host.to_string(), s.name.clone());
            cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.open(host.clone(), name.clone(), window, cx)
            })
        };
        let peek = {
            let k = k.clone();
            Button::new(SharedString::from(format!("peek-{i}")))
                .icon(IconName::Eye)
                .ghost()
                .xsmall()
                .tooltip("Peek at it")
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.peek(k.clone(), window, cx);
                }))
        };
        let group = SharedString::from(format!("agent-row-{i}"));
        h_flex()
            .id(SharedString::from(format!("agent-{i}")))
            .debug_selector(move || format!("agent-{i}"))
            .group(group.clone())
            .w_full()
            .px_2()
            .py_1()
            .gap_2()
            .items_center()
            .rounded(theme.radius)
            .cursor_pointer()
            .map(|row| {
                if shown {
                    row.bg(theme.tokens.sidebar_accent)
                } else {
                    row.hover(|row| row.bg(theme.sidebar_accent.opacity(0.5)))
                }
            })
            .on_click(open)
            // Put away: still an agent, but out of the way.
            .when(s.away, |row| row.opacity(0.6))
            .child(thread_icon(s).small())
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_sm().truncate().child(s.label.clone()))
                    .child(
                        div()
                            .text_xs()
                            .truncate()
                            .text_color(theme.muted_foreground)
                            .child(place),
                    ),
            )
            .when(!shown, |row| {
                row.child(
                    div()
                        .invisible()
                        .group_hover(group, |style| style.visible())
                        .child(peek),
                )
            })
            .child(status_dot(&k, color, self.breathes(s.activity), tip))
            .into_any_element()
    }

    /// What agent `i` runs, under it: one of Claude's subagents, a click on
    /// which shows its thread, or a terminal its agent started, which a
    /// click opens.
    fn helper_row(
        &self,
        i: usize,
        j: usize,
        host: &str,
        s: &Session,
        h: &Helper,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        // A subagent has no terminal of its own: it opens its thread.
        let opens = if h.terminal {
            h.id.clone()
        } else {
            s.name.clone()
        };
        let shown = h.terminal && self.active.as_ref() == Some(&(host.to_string(), h.id.clone()));
        let (color, what) = match h.activity {
            Activity::NeedsInput => (theme.warning, "Needs input"),
            Activity::Working => (theme.success, "Working"),
            Activity::Idle => (theme.muted_foreground.opacity(0.5), "Idle"),
            Activity::Unknown => (theme.muted_foreground.opacity(0.5), "Running"),
        };
        let tip: SharedString = if h.terminal {
            format!("{what} · a terminal its agent started")
        } else {
            format!("{what} · Claude's subagent")
        }
        .into();
        let dot_key: Key = (host.to_string(), format!("{}/{}", s.name, h.id));
        // A terminal of its own is peeked at, as Herdr does: what the
        // thread's agent started, over the one on show.
        let terminal = h.terminal;
        let open = {
            let host = host.to_string();
            cx.listener(move |this, _: &ClickEvent, window, cx| {
                if terminal {
                    this.peek((host.clone(), opens.clone()), window, cx);
                } else {
                    this.open(host.clone(), opens.clone(), window, cx);
                }
            })
        };
        let icon = if h.terminal {
            Icon::new(IconName::SquareTerminal)
        } else {
            claude_icon()
        };
        let label = SharedString::from(h.label.clone());
        h_flex()
            .id(SharedString::from(format!("agent-{i}-{j}")))
            .debug_selector(move || format!("agent-{i}-{j}"))
            .w_full()
            .pl(px(28.0 + 14.0 * h.depth as f32))
            .pr_2()
            .py_0p5()
            .gap_2()
            .items_center()
            .rounded(theme.radius)
            .text_xs()
            .cursor_pointer()
            .map(|row| {
                if shown {
                    row.bg(theme.tokens.sidebar_accent)
                } else {
                    row.hover(|row| row.bg(theme.sidebar_accent.opacity(0.5)))
                }
            })
            .on_click(open)
            .child(icon.xsmall())
            .child(div().flex_1().min_w_0().truncate().child(label))
            .child(status_dot(&dot_key, color, self.breathes(h.activity), tip))
            .into_any_element()
    }
}
