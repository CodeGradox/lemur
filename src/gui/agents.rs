//! The agents panel, on the right as the notes are, shown by its button at
//! the top right or ⌘⇧A: every agent's thread on every host, in the sidebar's
//! order whatever they're doing, each with what it runs to help it under it, as
//! Herdr's agents list. A click opens one.

use super::folders::listed;
use super::prompt::{agent_icon, thread_icon};
use super::spaces::SpaceView;
use super::sidebar::status_dot;
use super::silence::silence_label;
use super::{Key, Lemur, icons, key, sessions, worktree_label};
use crate::snapshot::{Activity, Helper, Session};
use rune::{Anim, El, ScrollSpec, TextStyle, col, row, text};
use rune_kit::shell::{RowStatus, SidebarRow};
use rune_kit::Kit;

impl Lemur {
    /// The agents' threads, and threads running helpers, as the panel lists
    /// them: in the sidebar's order, projects first, so what they're doing doesn't
    /// move them.
    pub(super) fn agents(&self) -> Vec<(&str, &Session)> {
        let mut agents: Vec<(&str, &Session)> = sessions(&self.snap).filter(|(_, s)| s.agent().is_some() || !s.helpers.is_empty()).collect();
        agents.sort_by_key(|(h, s)| self.project_of(h, &s.name).map_or(usize::MAX, |(base, _)| base));
        agents
    }

    /// The agents' threads the sidebar lists with ⌘., by session, as Rex lists activity by
    /// session: the sessions in the sidebar's order (host by host, its home, then its
    /// projects), each one's threads in its folders' order, those put away last. Not threads
    /// only running helpers, nor what they run.
    pub(super) fn sidebar_agent_groups(&self) -> Vec<(SpaceView, Vec<(&str, &Session)>)> {
        let mut groups = vec![];
        for v in self.spaces() {
            let Some(h) = self.host_view(&v.space.0) else { continue };
            let l = listed(h, Some(&v.space.1));
            let shown = l.groups.iter().flat_map(|g| g.threads.iter().copied());
            let threads: Vec<(&str, &Session)> = shown.chain(l.away.iter().copied()).filter(|s| s.agent().is_some()).map(|s| (h.name.as_str(), s)).collect();
            if !threads.is_empty() {
                groups.push((v, threads));
            }
        }
        groups
    }

    /// The agents' threads the sidebar lists with ⌘., from the top (`sidebar_agent_groups`).
    pub(super) fn sidebar_agent_threads(&self) -> Vec<(&str, &Session)> {
        self.sidebar_agent_groups().into_iter().flat_map(|(_, threads)| threads).collect()
    }

    /// ⌘., or the eye over the sidebar: it lists every agent's thread, or the session again.
    pub fn toggle_sidebar_agents(&mut self) {
        self.sidebar_agents = !self.sidebar_agents;
        // The thread on show's row, in the list now shown (`reveal_row`).
        self.revealed = None;
    }

    /// The agents as Next Agent goes through them: needing you, done while you
    /// looked away, working, then the rest; the latest first in each.
    pub(super) fn agents_by_urgency(&self) -> Vec<(&str, &Session)> {
        let mut agents = self.agents();
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

    /// Next Agent, or with `back` Previous Agent: the ones that need you first, then the
    /// ones done while you looked away, then the ones working, as Herdr's ⌘] and ⌘[ do:
    /// from the thread on show, or the first.
    pub(super) fn next_agent(&mut self, back: bool) {
        if self.overlaid() {
            return;
        }
        // Not one put away: showing it would bring it back for good.
        let now: Vec<Key> = self.agents_by_urgency().into_iter().filter(|(_, s)| !s.away).map(|(h, s)| key(h, s)).collect();
        // Next after next keeps to the order it started from: an agent done while you
        // looked away leaves that rank once opened, which would skip the next one. A new
        // agent, or one that ends, starts over.
        let same = |walk: &[Key]| walk.len() == now.len() && now.iter().all(|k| walk.contains(k));
        let agents = if self.active.as_ref().is_some_and(|a| self.agent_walk.contains(a)) && same(&self.agent_walk) {
            self.agent_walk.clone()
        } else {
            now
        };
        self.agent_walk = agents.clone();
        if agents.is_empty() {
            self.note("No agents' threads".into());
            return;
        }
        let at = self.active.as_ref().and_then(|a| agents.iter().position(|k| k == a));
        let next = at.map_or(0, |i| super::card::step(i, agents.len(), back));
        let (host, session) = agents[next].clone();
        self.open(host, session);
    }
}

pub fn view(s: &Lemur, k: &Kit) -> El {
    let agents = s.agents();
    let rows: Vec<El> = agents
        .iter()
        .enumerate()
        .flat_map(|(i, (host, t))| {
            let mut rows = vec![agent_row(s, &format!("agent-{i}"), host, t, panel_place(s, host, t), k)];
            rows.extend(t.helpers.iter().enumerate().map(|(j, h)| helper_row(s, (i, j), host, t, h, k)));
            rows
        })
        .collect();
    let body = if rows.is_empty() {
        col().key("agents-none").w_full().padding(12.0).child(
            text("No agents' threads. Claude's subagents show here too while they work.").font(k.ty.ui).color(k.c.text_muted).wrap(),
        )
    } else {
        col()
            .key("agents-list")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .padding(8.0)
            .gap(2.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .children(rows)
    };
    // No title: the Agents button over it says what it is.
    col().key("agents-panel").w_full().h_full().min_h(0.0).child(body)
}


/// The sidebar's rows with ⌘.: every agent's thread, as the panel's are but without what
/// they run to help them, under the name of its session, small as a folder's; or why
/// there's none.
pub fn sidebar_rows(s: &Lemur, k: &Kit) -> Vec<El> {
    let groups = s.sidebar_agent_groups();
    if groups.is_empty() {
        let none = text("No agents are running").font(k.ty.ui).color(k.c.text_faint);
        return vec![col().key("sidebar-agents-none").w_full().py(24.0).center().child(none)];
    }
    let mut rows = vec![];
    let mut i = 0;
    for (g, (v, threads)) in groups.into_iter().enumerate() {
        let heading = SidebarRow::new(format!("{} · {}", v.name, v.host)).title_style(k.ty.small).height(24.0).build(k).key(format!("sidebar-agents-session-{g}"));
        rows.push(heading);
        for (host, t) in threads {
            // Its session says where already: its worktree, or its folder, `~` for the home.
            let home = s.host_view(host).is_some_and(|h| h.home.trim_end_matches('/') == t.path.trim_end_matches('/'));
            let folder = || if home { "~".to_string() } else { crate::config::leaf(&t.path).to_string() };
            let mut place = s.project_of(host, &t.name).map_or_else(folder, |(_, w)| worktree_label(w));
            if let Some(issue) = &t.issue {
                place = format!("{place} · {issue}");
            }
            let shown = s.active.as_ref() == Some(&key(host, t));
            rows.push(super::sidebar::glass(agent_row(s, &format!("sidebar-agent-{i}"), host, t, place, k), shown, k));
            i += 1;
        }
    }
    rows
}

/// Where the panel says agent `t` of `host`'s runs: its project and worktree, else its host,
/// and the issue pinned to it.
fn panel_place(s: &Lemur, host: &str, t: &Session) -> String {
    let mut place = match s.project_of(host, &t.name) {
        Some((base, w)) => format!("{} · {}", s.cfg.bases[base].name(), worktree_label(w)),
        None => host.to_string(),
    };
    if let Some(issue) = &t.issue {
        place = format!("{place} · {issue}");
    }
    place
}

/// An agent's thread's row, of two lines: as tall as rune-kit has a sidebar row of two.
const TWO_LINES: f32 = 40.0;

/// An agent's thread, keyed `id`: its name, `place`, where it runs, and what it's doing, in
/// words too when that's for you to see to. Its icon is centred beside both lines, as Rex has
/// it: a sidebar row's own two lines put it by the name alone, high up, the place under it
/// leaving a gap.
fn agent_row(s: &Lemur, id: &str, host: &str, t: &Session, place: String, k: &Kit) -> El {
    let key = key(host, t);
    let shown = s.active.as_ref() == Some(&key);
    let (color, tip, breathes) = s.activity(&key, t, k);
    let says = match t.activity {
        Activity::NeedsInput => Some("Needs attention"),
        Activity::Working => None,
        _ if s.done.contains(&key) => Some("Done"),
        _ => None,
    };
    let mut trailing = row().gap(2.0).center_y();
    // On hover; not the one on show, which there's no peeking at.
    if !shown {
        let peek = key.clone();
        trailing = trailing.child(
            icons::button(&icons::EYE, "Peek at it")
                .on_press(move |s: &mut Lemur| s.peek(peek.clone()))
                .build(k)
                .key(format!("{id}-peek"))
                .group_opacity(0.0, 1.0),
        );
    }
    // On hover, and always once silenced: that's worth seeing.
    if t.agent().is_some() {
        let (silence, silent) = (key.clone(), t.silent);
        let bell = icons::button(if silent { &icons::BELL_OFF } else { &icons::BELL }, silence_label(silent))
            .on_press(move |s: &mut Lemur| s.set_silent(silence.clone(), !silent))
            .build(k)
            .key(format!("{id}-silence"));
        trailing = trailing.child(if silent { bell } else { bell.group_opacity(0.0, 1.0) });
    }
    if let Some(says) = says {
        trailing = trailing.child(text(says).key(format!("{id}-says")).font(k.ty.small).color(color));
    }
    let c = &k.c;
    let lines = col()
        .key("lines")
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(1.0)
        .child(
            text(t.label.clone())
                .key("label")
                .font(TextStyle { weight: 500.0, ..k.ty.ui_compact })
                .color(if shown { c.text_base } else { c.text_muted })
                .truncate()
                .a11y_hidden(),
        )
        .child(text(place.clone()).key("subtitle").font(k.ty.caption()).color(c.text_faint).truncate().a11y_hidden());
    let mut r = row()
        .label(format!("{}, {place}", t.label))
        .w_full()
        .h(TWO_LINES)
        .px(8.0)
        .gap(8.0)
        .center_y()
        .radius(k.radius.md)
        .motion(k.motion.control)
        .hover_group()
        .hover_motion(Anim::INSTANT, Anim::INSTANT)
        .focusable(true)
        .focus_selected(shown);
    r = k.interactive(r);
    if shown {
        r = r.fill(c.pressed);
    }
    let open = key.clone();
    r.child(row().key("avatar").size(20.0, 20.0).center().child(thread_icon(t.agent(), t.color.as_deref(), k)).a11y_hidden())
        .child(lines)
        .child(trailing)
        .child(row().key("status").size(20.0, 20.0).center().child(status_dot(color, tip, breathes)).a11y_hidden())
        .on_press(move |s: &mut Lemur| s.open(open.0.clone(), open.1.clone()))
        // After the handler, which would make it a button.
        .role("listitem")
        .key(id.to_string())
        // Put away: still an agent, but out of the way.
        .opacity(if t.away { 0.6 } else { 1.0 })
}

/// What agent `i` runs, under it (`j`th): one of Claude's subagents, a click on
/// which shows its thread, or a terminal its agent started, which a click peeks
/// at.
fn helper_row(s: &Lemur, (i, j): (usize, usize), host: &str, t: &Session, h: &Helper, k: &Kit) -> El {
    let c = &k.c;
    // A subagent has no terminal of its own: it opens its thread.
    let opens: Key = (host.to_string(), if h.terminal { h.id.clone() } else { t.name.clone() });
    let shown = h.terminal && s.active.as_ref() == Some(&opens);
    let (color, what) = match h.activity {
        Activity::NeedsInput => (c.warning.icon, "Needs input"),
        Activity::Working => (c.success.icon, "Working"),
        Activity::Idle => (c.text_muted.alpha(0.5), "Idle"),
        Activity::Unknown => (c.text_muted.alpha(0.5), "Running"),
    };
    let tip = if h.terminal { format!("{what} · a terminal its agent started") } else { format!("{what} · Claude's subagent") };
    let agent = if h.terminal { None } else { Some(crate::config::Agent::Claude) };
    // A terminal of its own is peeked at, as Herdr does: what the thread's agent
    // started, over the one on show.
    let terminal = h.terminal;
    SidebarRow::new(h.label.clone())
        .avatar(agent_icon(agent, k))
        .depth(1 + h.depth.min(8) as u8)
        .height(24.0)
        .title_style(k.ty.small)
        .status(RowStatus::Other(status_dot(color, tip, h.activity == Activity::Working)))
        .selected(shown)
        .on_select(move |s: &mut Lemur, _| {
            if terminal {
                s.peek(opens.clone());
            } else {
                s.open(opens.0.clone(), opens.1.clone());
            }
        })
        .build(k)
        .key(format!("agent-{i}-{j}"))
}

#[cfg(test)]
#[path = "agents_tests.rs"]
pub(super) mod tests;
