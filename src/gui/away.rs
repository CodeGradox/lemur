//! Putting a thread away, as Herdr puts a tab away: out of the sidebar's
//! lists and the tabs, still running, under its list's "Put away". It
//! comes back when it needs you, or when you open it. herdr-core keeps
//! it as a label on the terminal (`lemur.away`), so it stays away across
//! restarts, and from any Mac.

use super::{Key, Lemur, key};
use crate::actions;
use crate::config::ThreadOrder;
use crate::snapshot::Session;

impl Lemur {
    /// Puts thread `k` away. On show, the one you opened before it in its
    /// list shows instead, else its neighbour.
    pub(super) fn put_away(&mut self, k: Key) {
        self.mark_away(&k, true);
        if self.active.as_ref() == Some(&k) {
            let next = self.successor(&k, ThreadOrder::Recent, |other| {
                *other != k && self.session_of(other).is_some_and(|s| !s.away)
            });
            self.hide_shown(None);
            self.active = None;
            if let Some((host, session)) = next {
                self.open(host, session);
            }
            self.follow_shown();
        }
        self.save_away(k, true);
    }

    /// Brings thread `k` back, if it's away.
    pub(super) fn bring_back(&mut self, k: &Key) {
        if self.session_of(k).is_some_and(|s| s.away) {
            self.mark_away(k, false);
            self.save_away(k.clone(), false);
        }
    }

    /// Opens or folds the "Put away" of the list `list`.
    pub(super) fn toggle_away_list(&mut self, list: String) {
        if !self.away_open.remove(&list) {
            self.away_open.insert(list);
        }
            }

    /// Shows it so at once, its list in its new order, until the next
    /// refresh says what herdr-core keeps.
    fn mark_away(&mut self, k: &Key, away: bool) {
        let lists = self
            .snap
            .hosts
            .iter_mut()
            .filter(|h| h.name == k.0)
            .flat_map(|h| {
                let worktrees = h.bases.iter_mut().flat_map(|b| b.worktrees.iter_mut());
                worktrees.map(|w| &mut w.sessions).chain([&mut h.other])
            });
        for list in lists {
            if let Some(s) = list.iter_mut().find(|s| s.name == k.1) {
                s.away = away;
                // Stable: the rest keep their order.
                list.sort_by_key(|s: &Session| s.away);
                return;
            }
        }
    }

    fn save_away(&mut self, (host, session): Key, away: bool) {
        self.changes += 1;
                let cfg = self.cfg.clone();
        self.in_background(
            move || actions::set_away(&cfg, &host, &session, away),
            |this, saved| {
                // Refreshes that started before herdr-core had it are stale.
                this.changes += 1;
                if let Err(e) = saved {
                    this.fail(e);
                }
                this.refresh();
            },
        )
        ;
    }

    /// The threads put away that need you now: they come back.
    pub(super) fn bring_back_needed(&mut self) {
        let needed: Vec<Key> = super::sessions(&self.snap)
            .filter(|(_, s)| s.away && s.activity == crate::snapshot::Activity::NeedsInput)
            .map(|(h, s)| key(h, s))
            .collect();
        for k in needed {
            self.bring_back(&k);
        }
    }
}
