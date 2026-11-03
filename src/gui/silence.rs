//! Silencing a thread: it makes no more notifications, while its dot and the
//! agents panel go on saying what it does. herdr-core keeps it as a label on the
//! terminal (`lemur.silent`), so it stays silent across restarts, and from any Mac.

use super::{Key, Lemur};
use crate::actions;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::*;

/// What offers to silence a thread, or to let it notify again.
pub(super) fn silence_label(silent: bool) -> &'static str {
    if silent {
        "Allow notifications"
    } else {
        "Silence notifications"
    }
}

/// The menu item that silences thread `key`, or lets it notify again.
pub(super) fn silence_item(this: &WeakEntity<Lemur>, key: &Key, silent: bool) -> PopupMenuItem {
    let (this, key) = (this.clone(), key.clone());
    PopupMenuItem::new(silence_label(silent)).on_click(move |_, _, cx| {
        this.update(cx, |this, cx| this.set_silent(key.clone(), !silent, cx))
            .ok();
    })
}

impl Lemur {
    /// Silences thread `k`, taking down a popup it already made, or lets it
    /// notify again. Shown at once, until the next refresh says what herdr-core
    /// keeps.
    pub(super) fn set_silent(&mut self, k: Key, silent: bool, cx: &mut Context<Self>) {
        let shown = self
            .snap
            .hosts
            .iter_mut()
            .filter(|h| h.name == k.0)
            .flat_map(|h| {
                let in_bases = h.bases.iter_mut().flat_map(|b| b.worktrees.iter_mut());
                in_bases
                    .flat_map(|w| w.sessions.iter_mut())
                    .chain(h.other.iter_mut())
            });
        for s in shown.filter(|s| s.name == k.1) {
            s.silent = silent;
        }
        if silent {
            self.close_popup(&k, cx);
        }
        self.changes += 1;
        cx.notify();
        let cfg = self.cfg.clone();
        self.in_background(
            cx,
            move || actions::set_silent(&cfg, &k.0, &k.1, silent),
            |this, saved, _, cx| {
                // Refreshes that started before herdr-core had it are stale.
                this.changes += 1;
                if let Err(e) = saved {
                    this.fail(e, cx);
                }
                this.refresh(cx).detach();
            },
        )
        .detach();
    }
}
