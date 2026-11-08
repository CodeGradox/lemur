//! What the window calls as threads show and snapshots come in, for the panels on the right
//! (notes, Linear, changes, links, agents) and what else shows besides the terminal: the
//! folder picker, the command palette, a diff over the window, the usage meters, the
//! dictation bar, notifications and Settings. Each lives in a module of its own; this file
//! only says when each is told.

use super::{Key, Lemur, Right, agents, changes_panel, commands, dictation, linear_panel, links_panel, meter, notes, notify, picker, settings};
use crate::snapshot::Activity;
use rune::{El, Tx};
use rune_kit::Kit;
use std::path::Path;

pub struct Panels {
    pub notes: notes::Notes,
    pub changes: changes_panel::ChangesPanel,
    pub links: links_panel::LinksPanel,
    pub linear: linear_panel::LinearPanel,
    pub meters: meter::Meters,
    pub settings: settings::Settings,
    pub picker: Option<picker::Picker>,
    pub commands: Option<commands::Commands>,
    pub notify: notify::Notify,
    pub dictation: Option<dictation::Dictation>,
}

impl Panels {
    /// `data_dir` is where Lemur keeps notes and links (`~/.lemur`); tests have none.
    pub fn new(data_dir: Option<&Path>) -> Panels {
        Panels {
            notes: notes::Notes::new(data_dir),
            changes: changes_panel::ChangesPanel::default(),
            links: links_panel::LinksPanel::new(data_dir),
            linear: linear_panel::LinearPanel::default(),
            meters: meter::Meters::default(),
            settings: settings::Settings::default(),
            picker: None,
            commands: None,
            notify: notify::Notify::default(),
            dictation: None,
        }
    }
}

impl Lemur {
    /// Every refresh tick (`REFRESH`).
    pub(super) fn panels_tick(&mut self) {
        if self.right == Some(Right::Changes) && self.ticks % 3 == 0 {
            self.refresh_changes();
        }
        // It asks again once `links_panel::EVERY` has passed.
        if self.right == Some(Right::Links) {
            self.show_links();
        }
        self.tick_meter();
    }

    /// Before each frame: a field of the panels' that took the keyboard, or gave it back to
    /// a click elsewhere or its panel hidden, and a page to open in the browser (Linear's
    /// sign-in). True if that changed anything.
    pub(super) fn panels_poll(&mut self, tx: &mut Tx) -> bool {
        self.links_keyboard() | self.linear_poll(tx)
    }

    /// Something of the panels' is over the window: what would open another over it, or
    /// move the keyboard from it, waits.
    pub(super) fn panels_overlaid(&self) -> bool {
        self.panels.picker.is_some() || self.panels.commands.is_some() || self.panels.settings.open || self.diff_overlaid()
    }

    /// A text field of the panels' has the keyboard.
    pub(super) fn panels_editing(&self) -> bool {
        self.committing() || self.panels.dictation.as_ref().is_some_and(|d| d.input.focused)
    }

    /// Escape or ⌘W: closes what of the panels' is over the window, front first.
    pub(super) fn close_panel_overlay(&mut self) -> bool {
        if self.panels.commands.is_some() {
            self.close_palette();
            return true;
        }
        if self.panels.picker.is_some() {
            self.close_picker();
            return true;
        }
        if self.panels.settings.open {
            self.close_settings();
            return true;
        }
        self.close_diff_preview()
    }

    /// Panel `which` came out on the right.
    pub(super) fn panel_shown(&mut self, which: Right) {
        if which != Right::Notes {
            self.notes_hidden();
        }
        match which {
            Right::Changes => self.show_changes(),
            Right::Links => self.show_links(),
            Right::Linear => self.follow_linear(),
            Right::Notes | Right::Agents => {}
        }
    }

    /// A thread came on show: the panels go where it runs.
    pub(super) fn thread_shown(&mut self) {
        if self.right == Some(Right::Changes) {
            self.show_changes();
        }
        if self.right == Some(Right::Links) {
            self.show_links();
        }
        self.follow_linear();
        self.notes_follow_thread();
    }

    /// The first snapshot is in: the hosts are known, and the meters can ask.
    pub(super) fn first_snapshot(&mut self) {
        self.tick_meter();
    }

    /// The Changes and Links panels go by the thread on show; with none, they say so, rather
    /// than showing the worktree of the one that was. One just started isn't listed yet when
    /// it shows: once it is, they show where it runs. The notes wait for it the same way.
    pub(super) fn follow_shown(&mut self) {
        self.changes_follow_shown();
        self.links_follow_shown();
        self.notes_follow_shown();
    }

    /// Sessions that finished or need input, to tell of.
    pub(super) fn notify_news(&mut self, news: Vec<(Key, Activity, Option<String>)>) {
        self.notify(news);
    }

    /// The notification popups that show, by thread.
    pub(super) fn popups(&self) -> Vec<Key> {
        self.panels.notify.shown()
    }
}

/// The panel on the right.
pub fn right(s: &Lemur, k: &Kit) -> El {
    match s.right {
        Some(Right::Notes) | None => notes::view(s, k),
        Some(Right::Linear) => linear_panel::view(s, k),
        Some(Right::Changes) => changes_panel::view(s, k),
        Some(Right::Links) => links_panel::view(s, k),
        Some(Right::Agents) => agents::view(s, k),
    }
}

/// What shows over the window besides the spotlight, a dialog and a peek: front last.
pub fn overlays(s: &Lemur, k: &Kit, tx_layer: &mut Vec<El>) -> Vec<El> {
    let _ = tx_layer;
    [changes_panel::diff_overlay(s, k), settings::overlay(s, k), picker::overlay(s, k), commands::overlay(s, k)].into_iter().flatten().collect()
}

/// Under the terminal, over the status bar: the dictation bar while dictating.
pub fn under_terminal(s: &Lemur, k: &Kit) -> Option<El> {
    dictation::bar(s, k)
}

/// At the sidebar's foot, over the hosts' load: how much of each agent's plan is used.
pub fn sidebar_foot(s: &Lemur, k: &Kit) -> Vec<El> {
    meter::meters(s, k)
}
