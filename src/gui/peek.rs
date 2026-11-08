//! Peeking at a thread, as Herdr peeks at a sub-agent: its terminal over the one on show,
//! live, without switching to it. ↗ shows it instead; × or a click outside puts the one on
//! show back. Setting up a host shows its terminal the same way.

use super::prompt::agent_icon;
use super::sidebar::status_dot;
use super::{Key, Lemur, icons, terminal, worktree_label};
use crate::actions;
use crate::config::Host;
use crate::hc;
use rune::{El, col, el, row, text};
use rune_kit::{IconButton, IconButtonVariant, Kit};

/// Rune drops the state as the app quits.
impl Drop for Lemur {
    fn drop(&mut self) {
        self.end_setups();
    }
}

/// `card` over the window, which a click outside closes with `close`; `place` lays it out
/// in the window. Only the card itself keeps its clicks.
pub fn over_window<S: 'static>(
    key: &str,
    card: El,
    place: impl FnOnce(El) -> El,
    close: impl Fn(&mut S) + 'static,
    k: &Kit,
) -> El {
    el().key(format!("over-{key}"))
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .fill(k.c.scrim)
        .on_press(close)
        .child(place(card.on_press(|_: &mut S| {})))
}

/// In the middle of the window.
pub fn centred(card: El) -> El {
    col().w_full().h_full().center().child(card)
}

impl Lemur {
    /// Peeks at thread `key`: a helper's terminal, or another thread.
    pub fn peek(&mut self, key: Key) {
        if self.active.as_ref() == Some(&key) || self.overlaid() {
            return;
        }
        let Some(h) = self.cfg.host(&key.0).cloned() else { return };
        self.terminal_for(&key, h);
        if let Some(view) = self.terminals.get_mut(&key) {
            view.tell_focus(true);
            view.drawn_at = None;
        }
        self.peek = Some(key);
    }

    pub fn close_peek(&mut self) {
        let closed = self.peek.take();
        if let Some(view) = closed.as_ref().and_then(|k| self.terminals.get_mut(k)) {
            view.hidden();
        }
        if let Some(key) = closed {
            self.set_up_done(key);
        }
    }

    /// Opens `hc machine add` for `host` in a terminal over the window, as Herdr does, where
    /// you answer what herdr-core asks to add it: to install herdr-core there or update it,
    /// a password, a new host key.
    pub fn set_up_host(&mut self, host: Host) {
        // A double click, or one while it starts.
        if !self.setting_up.insert(host.name.clone()) {
            return;
        }
        let asked = host.clone();
        self.in_background(
            move || actions::set_up_host(&asked),
            move |s, started| {
                s.setting_up.remove(&host.name);
                match started {
                    Ok((address, here)) => {
                        // It takes the card; another set-up on it ends.
                        if s.peek.is_some() {
                            s.close_peek();
                        }
                        let key = (here.name.clone(), address);
                        s.setups.insert(key.clone(), host);
                        s.terminal_for(&key, here);
                        s.peek = Some(key);
                    }
                    Err(e) => s.fail(e.context(format!("setting up {}", host.name))),
                }
            },
        );
    }

    /// Lemur is quitting: nothing would show a set-up still asking, and being kept it would
    /// stay, so it ends.
    pub fn end_setups(&mut self) {
        for ((_, address), host) in self.setups.drain() {
            if let Ok(core) = hc::core(hc::machines_session(&host).as_deref()) {
                let _ = core.kill(&address);
            }
        }
    }

    /// Done with setting up a host, if `key` is the terminal that did: it goes, ended if it
    /// still runs, and herdr-core is asked again.
    fn set_up_done(&mut self, key: Key) {
        let Some(host) = self.setups.remove(&key) else { return };
        self.terminals.remove(&key);
        self.in_background(
            move || {
                let core = hc::core(hc::machines_session(&host).as_deref())?;
                core.kill(&key.1)?;
                core.forget_machine(&host);
                anyhow::Ok(())
            },
            |s, done| {
                if let Err(e) = done {
                    s.fail(e);
                }
                s.refresh();
            },
        );
    }
}

/// The thread peeked at, over the window: who it is and how it's doing over its terminal.
pub fn card(s: &Lemur, k: &Kit) -> Option<El> {
    let c = &k.c;
    let key = s.peek.clone()?;
    let view = s.terminals.get(&key)?;
    let session = s.session_of(&key);
    let setting_up = s.setups.get(&key).map(|h| format!("Setting up {}", h.name));
    let label = setting_up.clone().unwrap_or_else(|| session.map_or_else(|| s.label_of(&key.0, &key.1), |t| t.label.clone()));
    let place = match s.project_of(&key.0, &key.1) {
        Some((base, w)) => format!("{} · {}", s.cfg.bases[base].name(), worktree_label(w)),
        None => key.0.clone(),
    };
    let mut header = row()
        .key("peek-header")
        .h(36.0)
        .px(12.0)
        .gap(8.0)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], c.border_weak)
        .children(session.map(|t| agent_icon(t.agent(), k)))
        .child(text(label).font(k.ty.ui_medium).color(c.text_base))
        .child(text(place).font(k.ty.small).color(c.text_muted).truncate().grow(1.0));
    if let Some(t) = session {
        let (color, tip, breathes) = s.activity(&key, t, k);
        header = header.child(status_dot(color, tip, breathes));
    }
    // A host's set-up isn't a thread to show instead.
    if setting_up.is_none() {
        let open = key.clone();
        header = header.child(
            IconButton::new(&icons::EXTERNAL_LINK)
                .variant(IconButtonVariant::GhostMuted)
                .label("Show it instead")
                .help("Show it instead")
                .on_press(move |s: &mut Lemur| s.open(open.0.clone(), open.1.clone()))
                .build(k)
                .key("peek-open"),
        );
    }
    header = header.child(
        IconButton::new(&icons::X)
            .variant(IconButtonVariant::GhostMuted)
            .label("Close")
            .on_press(|s: &mut Lemur| s.close_peek())
            .build(k)
            .key("peek-close"),
    );
    let card = col()
        .key("peek")
        .w_frac(0.8)
        .h_full()
        .radius(k.radius.lg)
        .border(1.0, c.border_base)
        .fill(c.bg_base)
        .shadow(12.0, 40.0, c.scrim)
        .clip_rounded(k.radius.lg)
        .child(header)
        .child(col().w_full().grow(1.0).min_h(0.0).child(terminal::screen(view, s.metrics, s.keyboard_on_peek(), "peek")));
    let place = |card: El| col().w_full().h_full().py(56.0).items_center().child(card);
    Some(over_window("peek", card, place, |s: &mut Lemur| s.close_peek(), k))
}
