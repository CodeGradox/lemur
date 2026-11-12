//! Finding text in the terminal on show (⌘F): a bar at its top right to type in, the matches
//! marked where they show in Ghostty's colours, and the next one up, older, picked with ⌘G
//! or Enter, the next one down with ⇧⌘G or ⇧Enter. Ghostty's search does the looking,
//! through the scrollback too, a few milliseconds a frame. Each thread keeps its own bar.

use super::icons;
use super::terminal::TermView;
use super::{Key, Lemur};
use rune::{Color, El, Key as K, Tx, row, text};
use rune_kit::{Kit, TextInput};
use std::time::Duration;

/// The most a frame gives the search.
const BUDGET: Duration = Duration::from_millis(4);

/// What a match shows on, the one picked more strongly, its text black over it: Ghostty's
/// colours.
pub fn match_fill(picked: bool) -> Color {
    rune::color::hex(if picked { "#f2a57e" } else { "#ffe082" }).unwrap_or(Color::WHITE)
}

impl TermView {
    /// Looks for what its find bar says, picking the newest match once there's one.
    fn look_for(&mut self, budget: Duration) {
        let needle = self.find.text.clone();
        if let Some(g) = &mut self.grid {
            let _ = g.find(&needle);
            g.find_more(budget);
        }
        self.find_pick = !needle.is_empty();
    }

    /// Picks the next match up (`older`) or down, and shows it.
    fn step(&mut self, older: bool) {
        self.find_pick = false;
        if let Some(g) = &mut self.grid
            && g.find_next(older)
        {
            self.drawn_at = None;
        }
    }
}

impl Lemur {
    fn shown_mut(&mut self) -> Option<&mut TermView> {
        let key = self.active.as_ref()?;
        self.terminals.get_mut(key)
    }

    /// ⌘F: the thread on show's find bar opens, or has the keyboard again, its text selected
    /// so typing replaces it.
    pub(super) fn open_find(&mut self) {
        let Some(view) = self.shown_mut() else { return };
        view.find.focused = true;
        view.find.set_selection(0, view.find.text.len());
        if !view.finding {
            view.finding = true;
            view.look_for(BUDGET);
        }
    }

    /// ⌘G or ⇧⌘G: the next match up or down. With the bar closed, it opens again on what it
    /// looked for last, the keyboard left where it is.
    pub(super) fn find_next(&mut self, older: bool) {
        let Some(view) = self.shown_mut() else { return };
        if view.find.text.is_empty() {
            self.open_find();
            return;
        }
        if !view.finding {
            view.finding = true;
            view.look_for(BUDGET);
        }
        view.step(older);
    }

    fn find_in(&mut self, key: &Key, older: bool) {
        if let Some(view) = self.terminals.get_mut(key) {
            view.step(older);
        }
    }

    fn find_typed(&mut self, key: &Key) {
        if let Some(view) = self.terminals.get_mut(key) {
            view.look_for(BUDGET);
        }
    }

    /// The bar closes and the matches go; the view stays where the last one was.
    fn close_find(&mut self, key: &Key, tx: &mut Tx) {
        let Some(view) = self.terminals.get_mut(key) else { return };
        if std::mem::take(&mut view.find.focused) {
            tx.blur();
        }
        view.finding = false;
        view.find_pick = false;
        if let Some(g) = &mut view.grid {
            let _ = g.find("");
        }
    }

    /// The find bar of the thread on show has the keyboard.
    pub(super) fn find_editing(&self) -> bool {
        self.active.as_ref().and_then(|k| self.terminals.get(k)).is_some_and(|v| v.finding && v.find.focused)
    }

    /// Each poll: the search of the thread on show looks further while it has more to look
    /// through, and picks the newest match once one's found after typing. True when what it
    /// found changed.
    pub(super) fn tick_find(&mut self, tx: &mut Tx) -> bool {
        let Some(view) = self.shown_mut() else { return false };
        let Some(g) = view.grid.as_mut().filter(|_| view.finding && !view.find.text.is_empty()) else { return false };
        // Attached again, the terminal is a new one.
        if !g.looking() {
            let _ = g.find(&view.find.text);
            view.find_pick = true;
        }
        if !g.found().searching && !view.find_pick {
            return false;
        }
        let mut changed = g.find_more(BUDGET);
        let found = g.found();
        if view.find_pick && (found.total > 0 || !found.searching) {
            if found.picked.is_none() && found.total > 0 {
                view.step(true);
                changed = true;
            }
            view.find_pick = false;
        }
        if view.grid.as_ref().is_some_and(|g| g.found().searching) {
            tx.wake_after(Duration::ZERO);
        }
        changed
    }
}

/// "3 of 12" (1 the newest), how many there are while none's picked, or that there are none.
fn count(view: &TermView) -> String {
    let Some(found) = view.grid.as_ref().map(|g| g.found()) else { return String::new() };
    match found.picked {
        Some(i) => format!("{} of {}", i + 1, found.total),
        None if found.total > 0 => format!("{} found", found.total),
        None if !found.searching && !view.find.text.is_empty() => "No matches".into(),
        None => String::new(),
    }
}

/// `view`'s find bar, while it's open, over the top right of its terminal.
pub(super) fn bar(view: &TermView, key: &Key, k: &Kit) -> Option<El> {
    if !view.finding {
        return None;
    }
    let c = &k.c;
    let (edit, typed, submit, keys) = (key.clone(), key.clone(), key.clone(), key.clone());
    let field = TextInput::new(&view.find)
        .placeholder("Find")
        .leading_icon(&icons::SEARCH)
        .width(Some(240.0))
        .trailing(text(count(view)).key("find-count").font(k.ty.small).color(c.text_faint))
        .on_edit(move |s: &mut Lemur| &mut s.terminals.get_mut(&edit).expect("its terminal").find)
        .on_change(move |s: &mut Lemur, _| s.find_typed(&typed))
        .on_submit(move |s: &mut Lemur, _| s.find_in(&submit, true))
        .on_key(move |s: &mut Lemur, ev, tx| match ev.key {
            K::Enter if ev.mods.shift => {
                s.find_in(&keys, false);
                true
            }
            K::Escape => {
                s.close_find(&keys, tx);
                true
            }
            _ => false,
        })
        .build(k)
        .key("find-field");
    let (up, down, close) = (key.clone(), key.clone(), key.clone());
    Some(
        row()
            .key("find-bar")
            .pin(None, Some(8.0), Some(20.0), None)
            .padding(4.0)
            .gap(2.0)
            .center_y()
            .radius(k.radius.lg)
            .fill(c.layer_02)
            .border(1.0, c.border_base)
            .shadow(4.0, 16.0, c.scrim)
            .cursor(rune::Cursor::Default)
            // A press between its controls stays off the terminal under it.
            .on_press(|_: &mut Lemur| {})
            .child(field)
            .child(icons::button(&icons::CHEVRON_UP, "Find next (⌘G)").on_press(move |s: &mut Lemur| s.find_in(&up, true)).build(k).key("find-up"))
            .child(icons::button(&icons::CHEVRON_DOWN, "Find previous (⌘⇧G)").on_press(move |s: &mut Lemur| s.find_in(&down, false)).build(k).key("find-down"))
            .child(icons::button(&icons::X, "Close (Esc)").on_press_tx(move |s: &mut Lemur, tx| s.close_find(&close, tx)).build(k).key("find-close")),
    )
}
