//! ⌃Tab: going through the threads of the session of the one on show, but a folded folder's,
//! or with ⌘. every agent's as the sidebar lists them then, in the sidebar's order
//! or, as Settings can say, the one opened last first, the way ⌘Tab goes through apps. While
//! ⌃ is held each Tab goes one further; letting go of it settles on the thread on show.

use super::{Key, Lemur};
use crate::config::ThreadOrder;
use rune::Tx;

/// `threads` by when they were last opened (`recent`, the latest first); ones never opened
/// follow, in their own order.
pub(super) fn by_recent(mut threads: Vec<Key>, recent: &[Key]) -> Vec<Key> {
    threads.sort_by_key(|key| recent.iter().position(|r| r == key).unwrap_or(usize::MAX));
    threads
}

impl Lemur {
    /// The threads of the folder `active` is in, as ⌘1 to ⌘9 go through them: those put away
    /// aren't, unless on show.
    pub fn siblings(&self, active: &Key) -> Vec<Key> {
        let (host, session) = active;
        let mut names = self.group_of(active);
        if !names.contains(session) {
            names.push(session.clone());
        }
        names.into_iter().map(|name| (host.clone(), name)).collect()
    }

    /// Shows the next thread in the session of the one on show, or with ⌘. the next agent's
    /// down the sidebar, or with `back` the one before, in the order Settings says.
    pub fn cycle_threads(&mut self, back: bool, tx: &Tx) {
        if self.overlaid() {
            return;
        }
        let Some(active) = self.active.clone() else { return };
        if self.cycling.is_none() {
            let mut threads = match self.sidebar_agents {
                true => self.sidebar_threads(),
                // Not a folded folder's, as ⌘1–9 go: showing one would unfold it.
                false => {
                    let threads = self.space_of(&active).map(|space| self.space_threads(&space)).unwrap_or_default();
                    let folded = |k: &Key| self.session_of(k).is_some_and(|s| self.folded(&k.0, &super::folders::folder_of(s)));
                    threads.into_iter().filter(|k| !folded(k)).collect()
                }
            };
            // Put away, it's still where it starts from.
            if !threads.contains(&active) {
                threads.push(active.clone());
            }
            let order = match self.prefs.thread_order {
                ThreadOrder::Tabs => threads,
                ThreadOrder::Recent => by_recent(threads, &self.recent),
            };
            if order.len() < 2 {
                return;
            }
            let at = order.iter().position(|k| *k == active).unwrap_or(0);
            self.cycling = Some((order, at));
        }
        let Some((order, at)) = &mut self.cycling else { return };
        *at = super::card::step(*at, order.len(), back);
        let (host, session) = order[*at].clone();
        self.open(host, session);
        // From the menu, or a window that doesn't say ⌃ is still held: settled.
        if !tx.mods().ctrl {
            self.end_cycle();
        }
    }

    /// ⌃ came up: the thread on show is the one opened last.
    pub fn end_cycle(&mut self) {
        if self.cycling.take().is_some()
            && let Some(active) = self.active.clone()
        {
            self.touch(active);
        }
    }

    /// Notes `key` as the thread opened last.
    pub(super) fn touch(&mut self, key: Key) {
        self.recent.retain(|k| *k != key);
        self.recent.insert(0, key);
    }
}

#[cfg(test)]
mod tests {
    use super::by_recent;

    #[test]
    fn threads_go_by_when_they_were_opened() {
        let key = |s: &str| ("local".to_string(), s.to_string());
        let threads = vec![key("a"), key("b"), key("c"), key("d")];
        let recent = vec![key("c"), key("x"), key("a")];
        assert_eq!(by_recent(threads, &recent), [key("c"), key("a"), key("b"), key("d")]);
    }
}
