//! What the window knows of herdr-core: refreshes, the watch on its changes, the thread on
//! show, and opening, closing and ordering threads.

use super::snackbar::Kind;
use super::{BURST, JOBS, Key, Lemur, List, REFRESH, Right, key, move_item, sessions, watched, worktree_label};
use crate::actions;
use crate::attach::Active;
use crate::config::{Agent, ThreadOrder};
use crate::snapshot::{self, Activity, Session, Snapshot, Worktree};
use futures::StreamExt;
use std::collections::{HashMap, HashSet};

/// Out of sight, the sidebar refreshes every this many REFRESH ticks.
const AWAY: u32 = 5;

impl Lemur {
    /// The first refresh, the ticks, and the watch on herdr-core's changes.
    pub(super) fn start(&mut self) {
        self.refresh();
        self.tick();
        self.watch();
    }

    /// Every `REFRESH`: a new snapshot, terminals attached again, the panels asked again.
    fn tick(&mut self) {
        if self.looking || self.ticks % u64::from(AWAY) == 0 {
            self.refresh();
        }
        self.ticks += 1;
        // The worktree on show's changes, every `changes::EVERY` while you look at Lemur;
        // out of sight, every `changes::AWAY`.
        let every = if self.looking { super::changes::EVERY } else { super::changes::AWAY };
        if self.ticks % (every.as_secs() / REFRESH.as_secs()).max(1) == 0 {
            self.ask_changes();
        }
        self.panels_tick();
        self.bg.after(REFRESH, |s| s.tick());
    }

    /// herdr-core tells of every change to a terminal as it happens: started, ended,
    /// relabelled, what Claude says it's doing. A burst of changes makes one snapshot.
    fn watch(&mut self) {
        if self.watching {
            return;
        }
        self.watching = true;
        let cfg = self.cfg.clone();
        let back = self.bg.back();
        std::thread::spawn(move || {
            loop {
                let (cores, all) = watched(&cfg);
                if cores.is_empty() {
                    std::thread::sleep(REFRESH);
                    continue;
                }
                // Each core's changes, then word that it's lost; with a core not reached,
                // word to look again in a while.
                let mut streams: Vec<futures::stream::BoxStream<'static, bool>> = cores
                    .iter()
                    .map(|c| c.changes().map(|()| true).chain(futures::stream::once(async { false })).boxed())
                    .collect();
                if !all {
                    let (tx, rx) = futures::channel::mpsc::unbounded();
                    std::thread::spawn(move || {
                        std::thread::sleep(REFRESH * AWAY);
                        let _ = tx.unbounded_send(false);
                    });
                    streams.push(rx.boxed());
                }
                let mut changes = futures::stream::select_all(streams);
                // The window's gone: nothing to watch for.
                if !back.send(|s| s.recompose()) {
                    return;
                }
                while let Some(true) = futures::executor::block_on(changes.next()) {
                    std::thread::sleep(BURST);
                    let mut lost = false;
                    while let Some(Some(changed)) = futures::FutureExt::now_or_never(changes.next()) {
                        lost |= !changed;
                    }
                    if !back.send(|s| s.recompose()) {
                        return;
                    }
                    if lost {
                        break;
                    }
                }
                // Lost a core: connect again in a while.
                std::thread::sleep(REFRESH);
            }
        });
    }

    /// Fetches a snapshot in the background and applies it.
    pub fn refresh(&mut self) {
        let cfg = self.cfg.clone();
        let changes = self.changes;
        self.in_background(
            move || {
                let data = snapshot::fetch_data(&cfg);
                let snap = snapshot::compose(&cfg, &data);
                (data, snap)
            },
            move |s, (data, mut snap)| {
                if s.changes == changes {
                    s.data = data;
                    s.keep_ordered(&mut snap);
                    s.set_snapshot(snap);
                }
            },
        );
    }

    /// Files the terminals as they are now under what the last refresh found, as herdr-core
    /// tells of a change.
    pub(super) fn recompose(&mut self) {
        if !self.loaded {
            return;
        }
        let (cfg, data) = (self.cfg.clone(), self.data.clone());
        let changes = self.changes;
        self.in_background(
            move || snapshot::compose(&cfg, &data),
            move |s, mut snap| {
                if s.changes == changes {
                    s.keep_ordered(&mut snap);
                    s.set_snapshot(snap);
                }
            },
        );
    }

    /// Whether an order dragged on host `name` is being written.
    fn is_ordering(&self, name: &str) -> bool {
        self.ordering.keys().any(|(host, _)| host == name)
    }

    /// Keeps hosts in `snap` as they show while an order dragged there is written, a label at
    /// a time: they'd show it half written.
    fn keep_ordered(&self, snap: &mut Snapshot) {
        for h in &mut snap.hosts {
            if self.is_ordering(&h.name)
                && let Some(kept) = self.snap.hosts.iter().find(|k| k.name == h.name)
            {
                *h = kept.clone();
            }
        }
    }

    pub(super) fn set_snapshot(&mut self, snap: Snapshot) {
        let now: HashMap<Key, &Session> = sessions(&snap).map(|(h, s)| (key(h, s), s)).collect();
        // A thread just started may not be listed yet.
        self.fresh.retain(|k, left| {
            *left = left.saturating_sub(1);
            !now.contains_key(k) && *left > 0
        });
        // Forget terminals whose session is gone (killed, or ended elsewhere). A host that
        // couldn't be reached keeps its own: they aren't known to be gone, and reattach once
        // it's back.
        let fresh = &self.fresh;
        let setups = &self.setups;
        let unreached: HashSet<&str> = snap.hosts.iter().filter(|h| h.error.is_some()).map(|h| h.name.as_str()).collect();
        // A helper's terminal opened from under its thread.
        let helpers: HashSet<Key> = sessions(&snap)
            .flat_map(|(h, s)| s.helpers.iter().filter(|x| x.terminal).map(move |x| (h.to_string(), x.id.clone())))
            .collect();
        let live = |k: &Key| {
            now.contains_key(k)
                || fresh.contains_key(k)
                || helpers.contains(k)
                || setups.contains_key(k)
                || unreached.contains(k.0.as_str())
        };
        // What can show instead of a thread that ended: not one put away, nor one only kept
        // for a host that can't be reached.
        let showable = |k: &Key| now.get(k).is_some_and(|s| !s.away) || fresh.contains_key(k) || helpers.contains(k);
        self.terminals.retain(|k, _| live(k));
        if self.peek.as_ref().is_some_and(|k| !live(k)) {
            self.peek = None;
        }
        // The thread on show ended: another takes its place.
        let mut instead = None;
        if let Some(gone) = self.active.clone().filter(|a| !live(a)) {
            self.active = None;
            instead = self.successor(&gone, self.prefs.thread_order, showable);
        }
        // What each thread did last. A host that can't be reached keeps its own.
        let before = std::mem::take(&mut self.seen);
        // Working -> idle is "finished"; anything -> waiting is "needs input".
        let mut news = vec![];
        for (k, s) in &now {
            let was = before.get(k).copied().unwrap_or(Activity::Unknown);
            let finished = was == Activity::Working && matches!(s.activity, Activity::Idle | Activity::Unknown);
            if finished && self.active.as_ref() != Some(k) {
                self.done.insert(k.clone());
                // It ranks among the done now: Next Agent starts over.
                self.agent_walk.clear();
            }
            let asks = s.activity == Activity::NeedsInput && was != Activity::NeedsInput;
            if (finished || asks) && !s.silent {
                news.push((k.clone(), s.activity, s.waiting_for.clone()));
            }
        }
        self.seen = before
            .into_iter()
            .filter(|(k, _)| unreached.contains(k.0.as_str()) && !now.contains_key(k))
            .chain(now.iter().map(|(k, s)| (k.clone(), s.activity)))
            .collect();
        let kept = |k: &Key| unreached.contains(k.0.as_str()) && !now.contains_key(k);
        self.done.retain(|k| kept(k) || now.get(k).is_some_and(|s| s.activity != Activity::Working));
        let gone: Vec<Key> = self.popups().into_iter().filter(|k| !now.contains_key(k) && !kept(k)).collect();
        // Not one put away since: it stays away across restarts.
        let restore = self.restore.take().filter(|k| now.get(k).is_some_and(|s| !s.away) && self.active.is_none());
        drop(now);
        for k in gone {
            self.close_popup(&k);
        }
        self.snap = snap;
        self.bring_back_needed();
        self.reconnect_terminals();
        self.note_agents();
        self.follow_loads();
        let first = !self.loaded;
        self.loaded = true;
        if first {
            self.first_snapshot();
            self.check_core_updates();
        }
        self.keep_claude_ids();
        self.note_shells();
        self.follow_linear();
        self.notify_news(news);
        if let Some((host, session)) = restore {
            self.open(host, session);
        } else if let Some((host, session)) = instead {
            self.show_thread(host, session);
        }
        self.follow_shown();
    }

    /// What shows once `gone`, the thread on show, has ended: by `order` the one opened last
    /// among the threads it was with (its worktree's, or its host's outside every worktree);
    /// else, and along the tabs, its neighbour there, the next tab first; else the one
    /// opened last anywhere. `live` says what still runs.
    pub(super) fn successor(&self, gone: &Key, order: ThreadOrder, live: impl Fn(&Key) -> bool) -> Option<Key> {
        let with = self.siblings(gone);
        let at = with.iter().position(|k| k == gone).unwrap_or(with.len());
        let after = with.iter().skip(at + 1);
        let before = with.iter().take(at).rev();
        self.recent
            .iter()
            .filter(|k| *k != gone && live(k))
            .find(|k| order == ThreadOrder::Recent && with.contains(k))
            .or_else(|| after.chain(before).find(|k| live(k)))
            .or_else(|| self.recent.iter().find(|k| *k != gone && live(k)))
            .cloned()
    }

    /// Opens a thread just started, which the sidebar may not list yet.
    pub(super) fn open_fresh(&mut self, host: String, session: String) {
        self.fresh.insert((host.clone(), session.clone()), 3);
        self.open(host, session);
    }

    /// Shows a thread, as a click on it does.
    pub fn open(&mut self, host: String, session: String) {
        if self.peek.as_ref().is_some_and(|k| k.0 == host && k.1 == session) {
            self.peek = None;
        }
        self.show_thread(host, session);
    }

    /// Shows a thread: its terminal, attached the first time.
    pub(super) fn show_thread(&mut self, host: String, session: String) {
        let Some(h) = self.cfg.host(&host).cloned() else { return };
        let key = (host.clone(), session.clone());
        self.hide_shown(Some(&key));
        self.terminal_for(&key, h);
        if self.peek.as_ref() == Some(&key) {
            self.peek = None;
        }
        self.done.remove(&key);
        self.close_popup(&key);
        self.reveal(&host, &session);
        // ⌃Tab keeps the order it started with until ⌃ comes up.
        if self.cycling.is_none() {
            self.touch(key.clone());
        }
        self.bring_back(&key);
        self.active = Some(key.clone());
        if let Some(view) = self.terminals.get_mut(&key) {
            view.tell_focus(true);
            // Drawn at once, not at its next output.
            view.drawn_at = None;
        }
        self.ask_changes();
        self.thread_shown();
        self.remember(host, session);
    }

    /// A spotlight, the folder picker, the palette, a diff, a dialog or a peek is over the
    /// window: what would open another over it, or move the keyboard from it, waits.
    pub fn overlaid(&self) -> bool {
        self.prompt.is_some() || self.dialog.is_some() || self.peek.is_some() || self.panels_overlaid()
    }

    /// Escape: closes what's over the window, front first. True if something was.
    pub(super) fn close_overlay(&mut self) -> bool {
        if self.dialog.take().is_some() {
            return true;
        }
        if self.prompt.take().is_some() {
            self.typing = false;
            return true;
        }
        if self.close_panel_overlay() {
            return true;
        }
        if self.peek.take().is_some() {
            return true;
        }
        false
    }

    /// Tells the thread on show's program it's no longer shown, unless it's `next`.
    pub(super) fn hide_shown(&mut self, next: Option<&Key>) {
        if let Some(shown) = self.active.clone().filter(|k| Some(k) != next)
            && let Some(view) = self.terminals.get_mut(&shown)
        {
            view.hidden();
        }
    }

    /// The thread behind `key`, if a snapshot lists it.
    pub fn session_of(&self, key: &Key) -> Option<&Session> {
        sessions(&self.snap).find(|(h, s)| *h == key.0 && s.name == key.1).map(|(_, s)| s)
    }

    /// The worktree the thread on show runs in, and the host it's on.
    pub fn active_worktree(&self) -> Option<(&str, &Worktree)> {
        let (host, session) = self.active.as_ref()?;
        let w = self.snap.worktrees(host).find(|w| w.sessions.iter().any(|s| &s.name == session))?;
        Some((host.as_str(), w))
    }

    /// Where a thread runs, for what starts beside it: its worktree and the project it's
    /// in, or its folder outside every worktree.
    pub fn place_of(&self, key: &Key) -> Option<(String, Option<usize>)> {
        if let Some((base, w)) = self.project_of(&key.0, &key.1) {
            return Some((w.path.clone(), Some(base)));
        }
        Some((self.session_of(key)?.path.clone(), None))
    }

    /// The project (its place in the config) and worktree a session runs in.
    pub fn project_of(&self, host: &str, session: &str) -> Option<(usize, &Worktree)> {
        self.snap.hosts.iter().filter(|h| h.name == host).flat_map(|h| &h.bases).find_map(|b| {
            let w = b.worktrees.iter().find(|w| w.sessions.iter().any(|s| s.name == session))?;
            Some((b.index, w))
        })
    }

    /// A session as the sidebar shows it: `project › worktree`, none outside every project,
    /// and the thread's name there.
    pub fn titles(&self, host: &str, session: &str) -> (Option<String>, String) {
        let thread = self.label_of(host, session);
        match self.project_of(host, session) {
            Some((index, w)) => {
                let project = self.cfg.bases[index].name();
                (Some(format!("{project} › {}", worktree_label(w))), thread)
            }
            None => (None, thread),
        }
    }

    /// What the sidebar calls a thread.
    pub fn label_of(&self, host: &str, session: &str) -> String {
        sessions(&self.snap)
            .find(|(h, s)| *h == host && s.name == session)
            .map(|(_, s)| s.label.clone())
            .unwrap_or_else(|| actions::label(crate::config::leaf(session)).to_string())
    }

    /// The worktree a session runs in, and its label.
    pub fn worktree_of(&self, host: &str, session: &str) -> Option<(String, String)> {
        self.snap
            .worktrees(host)
            .find(|w| w.sessions.iter().any(|s| s.name == session))
            .map(|w| (w.path.clone(), worktree_label(w)))
    }

    fn remember(&self, host: String, session: String) {
        if let Some(path) = &self.state_file {
            let _ = Active { host, session }.save_to(path);
        }
    }

    /// Moves a thread within its list: the sidebar shows the new order at once, and
    /// herdr-core keeps it.
    pub fn reorder(&mut self, list: List, from: usize, to: usize) {
        let Some(sessions) = self.list_mut(&list) else { return };
        if !move_item(sessions, from, to) {
            return;
        }
        let names: Vec<String> = sessions.iter().map(|s| s.name.clone()).collect();
        self.changes += 1;
        match self.ordering.get_mut(&list) {
            // One order at a time, the latest next: two written at once would interleave
            // their labels.
            Some(next) => *next = Some(names),
            None => {
                self.ordering.insert(list.clone(), None);
                self.write_order(list, names);
            }
        }
    }

    /// ⌘⇧← or, with `back` false, ⌘⇧→: moves the tab on show one place along the tabs, as
    /// dragging it there does; at the end it stays.
    pub fn move_tab(&mut self, back: bool) {
        if self.overlaid() {
            return;
        }
        let Some(active) = self.active.clone() else { return };
        let tabs = self.siblings(&active);
        let Some(at) = tabs.iter().position(|k| *k == active) else { return };
        let next = if back { at.checked_sub(1) } else { Some(at + 1).filter(|to| *to < tabs.len()) };
        let Some(next) = next else { return };
        let list: List = (active.0.clone(), self.project_of(&active.0, &active.1).map(|(_, w)| w.path.clone()));
        // In the list, with what's put away among it.
        let place = |this: &mut Self, name: &str| this.list_mut(&list)?.iter().position(|s| s.name == name);
        let (Some(from), Some(to)) = (place(self, &active.1), place(self, &tabs[next].1)) else { return };
        self.reorder(list, from, to);
    }

    /// Writes the order dragged into `list` to herdr-core, a label a thread, then the one
    /// dragged meanwhile, if any.
    fn write_order(&mut self, list: List, names: Vec<String>) {
        let cfg = self.cfg.clone();
        let host = list.0.clone();
        self.in_background(
            move || actions::set_order(&cfg, &host, &names),
            move |s, saved| {
                // Refreshes that started before herdr-core had the order are stale too.
                s.changes += 1;
                if let Err(e) = saved {
                    s.fail(e);
                }
                match s.ordering.get_mut(&list).and_then(Option::take) {
                    Some(next) => s.write_order(list, next),
                    None => {
                        s.ordering.remove(&list);
                        s.refresh();
                    }
                }
            },
        );
    }

    pub(super) fn list_mut(&mut self, (host, worktree): &List) -> Option<&mut Vec<Session>> {
        let h = self.snap.hosts.iter_mut().find(|h| &h.name == host)?;
        match worktree {
            None => Some(&mut h.other),
            Some(path) => {
                h.bases.iter_mut().flat_map(|b| b.worktrees.iter_mut()).find(|w| &w.path == path).map(|w| &mut w.sessions)
            }
        }
    }

    pub fn session_mut(&mut self, (host, name): &Key) -> Option<&mut Session> {
        let h = self.snap.hosts.iter_mut().find(|h| &h.name == host)?;
        h.bases
            .iter_mut()
            .flat_map(|b| b.worktrees.iter_mut())
            .flat_map(|w| w.sessions.iter_mut())
            .chain(h.other.iter_mut())
            .find(|s| &s.name == name)
    }

    /// ⌘W: asks whether to close the thread on show.
    pub fn confirm_close(&mut self) {
        // The peek is in front, not the thread on show under it.
        if self.peek.is_some() && self.prompt.is_none() && self.dialog.is_none() && !self.panels_overlaid() {
            self.peek = None;
            return;
        }
        if self.close_panel_overlay() {
            return;
        }
        if self.overlaid() {
            return;
        }
        if let Some(key) = self.active.clone() {
            self.confirm_close_of(key);
        }
    }

    /// Closes the thread `key`, asking first while something other than its shell runs in
    /// it, in front or as a job of the shell's.
    pub fn confirm_close_of(&mut self, key: Key) {
        let (label, agent) = match self.session_of(&key) {
            Some(s) if !s.busy => {
                // A shell at its prompt closes at once, unless jobs it started still run or
                // wait (`&`, ⌃Z): they'd end too.
                let (cfg, label, pid) = (self.cfg.clone(), s.label.clone(), s.pid);
                let host = key.0.clone();
                self.in_background(
                    move || actions::shell_jobs(&cfg, &host, pid),
                    move |s, jobs| match jobs {
                        Ok(0) => s.close_thread(key),
                        _ => s.ask_close(key, label, JOBS),
                    },
                );
                return;
            }
            Some(s) => (s.label.clone(), s.agent()),
            None => (self.label_of(&key.0, &key.1), None),
        };
        let description = match agent {
            Some(Agent::Claude) => {
                "Claude quits, and its terminal ends. Lemur forgets the conversation, as it does for a thread killed."
            }
            Some(Agent::Codex) => "Codex quits, and its terminal ends.",
            None => "The terminal ends, and whatever runs in it.",
        };
        self.ask_close(key, label, description);
    }

    /// Asks whether to close thread `key`, called `label`, saying what that ends.
    fn ask_close(&mut self, key: Key, label: String, description: &'static str) {
        self.dialog = Some(super::Dialog {
            title: format!("Close {label}?"),
            description: description.into(),
            ok: "Close",
            check: None,
            on_ok: Box::new(move |s, _| s.close_thread(key)),
        });
    }

    /// Ends a thread's terminal; the worktree stays.
    pub fn close_thread(&mut self, key: Key) {
        let cfg = self.cfg.clone();
        let (host, session) = key;
        let killed = (host.clone(), vec![session.clone()]);
        self.in_background(
            move || actions::kill_session(&cfg, &host, &session),
            move |s, result| {
                match result {
                    // Killed, it's not to come back.
                    Ok(()) => s.forget_threads(&killed.0, &killed.1),
                    Err(e) => s.fail(e),
                }
                s.refresh();
            },
        );
    }

    /// The notes, Linear, the changes and the agents take turns on the right: showing one
    /// hides the other, and showing the one that shows hides it.
    pub fn toggle_right(&mut self, which: Right) {
        if self.right == Some(which) {
            self.right = None;
        } else {
            self.right = Some(which);
            self.right_last = which;
            self.panel_shown(which);
        }
        self.inspector.set_open(self.right.is_some());
    }

    /// ⌘R: hides the right panel, or brings back the one shown last; the notes the first
    /// time.
    pub fn toggle_right_panel(&mut self) {
        match self.right {
            Some(_) => {
                self.right = None;
                self.inspector.set_open(false);
            }
            None => self.toggle_right(self.right_last),
        }
    }

    /// Says so when a newer herdr-core is out than runs here or on a machine. Tests don't
    /// ask the release site.
    fn check_core_updates(&mut self) {
        if cfg!(test) {
            return;
        }
        let cores = crate::core_update::Cores::of(&self.cfg);
        self.in_background(
            move || crate::core_update::check(&cores),
            |s, versions| {
                let Ok(versions) = versions else { return };
                let outdated = versions.outdated();
                if let (false, Some(latest)) = (outdated.is_empty(), &versions.latest) {
                    let note = format!(
                        "herdr-core {latest} is out ({}): ⌘⇧P → Update herdr-core, or Settings → herdr-core",
                        versions.describe(&outdated)
                    );
                    s.note(note);
                }
            },
        );
    }

    /// Updates herdr-core here, then on each machine behind, as Herdr does; terminals keep
    /// running.
    pub fn update_core(&mut self) {
        self.status = Some(("Updating herdr-core…".into(), Kind::Progress));
        let cores = crate::core_update::Cores::of(&self.cfg);
        self.in_background(
            move || {
                let versions = crate::core_update::check(&cores)?;
                if versions.latest.is_none() {
                    anyhow::bail!("couldn't ask for herdr-core's latest release");
                }
                let outdated = versions.outdated();
                if outdated.is_empty() {
                    return Ok(format!("herdr-core is up to date ({})", versions.here.unwrap_or_default()));
                }
                crate::core_update::update(&outdated, &cores)
            },
            |s, done| match done {
                Ok(note) => s.note(note),
                Err(e) => s.fail(e),
            },
        );
    }

    /// Says which version this is.
    pub fn about(&mut self) {
        self.dialog = Some(super::Dialog {
            title: format!("Lemur {}", env!("CARGO_PKG_VERSION")),
            description: "Claude Code and Codex sessions on your hosts, in herdr-core.".into(),
            ok: "OK",
            check: None,
            on_ok: Box::new(|_, _| {}),
        });
    }

    /// ⌘1 to ⌘9: shows the tab at `i` of the tab bar, if it has one.
    pub fn go_to_tab(&mut self, i: usize) {
        if self.overlaid() {
            return;
        }
        let Some(active) = self.active.clone() else { return };
        if let Some((host, session)) = self.siblings(&active).get(i).cloned() {
            self.open(host, session);
        }
    }

    /// Follows each host's load: the CPU share is the busy time between two readings.
    pub(super) fn follow_loads(&mut self) {
        for h in &self.snap.hosts {
            let Some(load) = &h.machine.load else { continue };
            let cpu = match self.loads.get(&h.name) {
                Some((before, cpu)) if before.total == load.total => *cpu,
                Some((before, _)) => load.cpu_since(before),
                None => None,
            };
            self.loads.insert(h.name.clone(), (load.clone(), cpu));
        }
        // One that can't be reached shows no load, rather than its last.
        let snap = &self.snap;
        self.loads.retain(|name, _| snap.hosts.iter().any(|h| h.name == *name && h.error.is_none()));
    }

    /// Each host's last load and its CPU share since the reading before.
    pub fn loads(&self) -> &HashMap<String, (crate::machine::Load, Option<f32>)> {
        &self.loads
    }
}

#[cfg(test)]
pub(super) fn wait_for(rt: &mut rune::Runtime<Lemur>, secs: f64, done: impl Fn(&Lemur) -> bool) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed().as_secs_f64() < secs {
        rt.frame(1.0 / 60.0);
        if done(&rt.state) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}
