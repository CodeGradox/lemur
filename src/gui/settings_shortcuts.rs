//! Settings' Shortcuts page: the order ⌃Tab goes in, and every shortcut, group by group.

use super::{group, item, picker};
use crate::config::ThreadOrder;
use crate::gui::Lemur;
use crate::gui::actions::{ACTIONS, Action};
use rune::{Chord, El, KeyPlatform, row, text};
use rune_kit::{Keybind, Kit};

/// A shortcut on the Shortcuts page.
pub(super) struct Shortcut {
    pub(super) what: &'static str,
    pub(super) how: Press,
    pub(super) more: Option<&'static str>,
}

/// How a shortcut goes: the keys of actions, or keys (in `Chord`'s words), each an
/// alternative, or a click with keys held.
pub(super) enum Press {
    Actions(&'static [Action]),
    Keys(&'static [&'static str]),
    Click(&'static str),
}

impl Press {
    /// One set of keycaps for each way.
    pub(super) fn keycaps(&self) -> Vec<Vec<String>> {
        let chords: Vec<&str> = match self {
            Press::Actions(actions) => {
                actions.iter().filter_map(|a| ACTIONS.iter().find(|(b, ..)| b == a).and_then(|(.., key)| *key)).collect()
            }
            Press::Keys(keys) => keys.to_vec(),
            Press::Click(_) => vec![],
        };
        chords.into_iter().filter_map(|c| Chord::parse(c).ok()).map(|c| c.keycaps_on(KeyPlatform::current())).collect()
    }
}

const fn plain(what: &'static str, how: Press) -> Shortcut {
    Shortcut { what, how, more: None }
}

const fn told(what: &'static str, how: Press, more: &'static str) -> Shortcut {
    Shortcut { what, how, more: Some(more) }
}

/// lemur's shortcuts, group by group: the actions' keys, and the ones the window's menus,
/// the terminal, the notes and the spotlights take themselves.
pub(super) const SHORTCUTS: &[(&str, &[Shortcut])] = &[
    (
        "Window",
        &[
            told("Command palette", Press::Actions(&[Action::CommandPalette]), "Every command by name, with its key."),
            plain("Toggle the sidebar", Press::Actions(&[Action::ToggleSidebar])),
            told("Toggle the right panel", Press::Actions(&[Action::ToggleRightPanel]), "The panel that showed last: the notes at first."),
            plain("Toggle the agents", Press::Actions(&[Action::ToggleAgents])),
            plain("Add a project", Press::Actions(&[Action::AddProject])),
            told("Zoom in", Press::Actions(&[Action::ZoomIn]), "Everything larger, the terminal's text too. ⌘+ does it as well."),
            plain("Zoom out", Press::Actions(&[Action::ZoomOut])),
            plain("Actual size", Press::Actions(&[Action::ActualSize])),
            told("Go to a thread, from any app", Press::Keys(&[crate::gui::SUMMON]), "Brings Lemur forward, asking which thread."),
            plain("Settings", Press::Keys(&["mod+,"])),
            plain("Close Settings", Press::Keys(&["mod+w", "escape"])),
            plain("Hide Lemur", Press::Keys(&["mod+h"])),
            plain("Hide the other apps", Press::Keys(&["alt+mod+h"])),
            plain("Quit Lemur", Press::Keys(&["mod+q"])),
        ],
    ),
    (
        "Threads",
        &[
            told("New terminal", Press::Actions(&[Action::NewTerminal]), "A new tab where the thread on show runs."),
            told(
                "New Claude or Codex thread or shell, or go to one",
                Press::Actions(&[Action::NewThreadHere]),
                "Named, where the thread on show runs; type to find another thread.",
            ),
            plain("Go to a thread", Press::Actions(&[Action::GoToThread])),
            told("Close the thread on show", Press::Actions(&[Action::CloseThread]), "Asking first while something runs in it."),
            told(
                "Next thread in the worktree",
                Press::Actions(&[Action::NextThread]),
                "In the order set above; hold ⌃ and press Tab again to go further.",
            ),
            plain("Previous thread in the worktree", Press::Actions(&[Action::PreviousThread])),
            plain("Move the tab left or right", Press::Actions(&[Action::MoveTabLeft, Action::MoveTabRight])),
            told("Go to a tab", Press::Keys(&["mod+1", "mod+9"]), "⌘1 shows the first tab, ⌘2 the second, and so on up to the ninth."),
        ],
    ),
    (
        "Dictation",
        &[
            told(
                "Dictate",
                Press::Actions(&[Action::Dictate]),
                "Opens a bar under the terminal that writes down what you say; again stops listening, and once more adds to it.",
            ),
            told("Paste it into the thread on show", Press::Keys(&["mod+enter"]), "Without sending it. From the terminal too, after switching threads."),
            plain("Throw it away", Press::Keys(&["escape"])),
        ],
    ),
    (
        "Terminal",
        &[
            told("Paste", Press::Keys(&["mod+v"]), "An image or copied files go to the host, and their paths are pasted."),
            told("Open a link", Press::Click("⌘-click"), "Holding ⌘ underlines it first."),
        ],
    ),
    (
        "Notes",
        &[
            told("Open a link", Press::Click("⌘-click"), "Holding ⌘ underlines it first."),
            told("Make a link", Press::Keys(&["mod+k"]), "Of the selected text, to the URL on the clipboard; in a link, it selects the URL."),
            told("Link the selected text", Press::Keys(&["mod+v"]), "Pasting a URL over text on one line."),
            told("Tick a box", Press::Click("⌘-click"), "The [ ] of a - [ ] item; again unticks it."),
            plain("Tick the box on the cursor's line", Press::Keys(&["mod+enter", "mod+shift+u"])),
            plain("Make the lines a checklist", Press::Keys(&["mod+shift+l"])),
            told("Next list item", Press::Keys(&["enter"]), "At the end of a list item; on an empty one, it ends the list."),
            plain("Indent a list item", Press::Keys(&["tab", "mod+]"])),
            plain("Outdent a list item", Press::Keys(&["shift+tab", "mod+["])),
            told("Move the lines up or down", Press::Keys(&["ctrl+mod+up", "ctrl+mod+down"]), "A list item takes its nested items along."),
        ],
    ),
    (
        "Naming in the spotlight",
        &[
            plain("Pick Claude, Codex or Shell, for a new thread", Press::Keys(&["up", "down"])),
            plain("Start or rename", Press::Keys(&["enter"])),
            plain("Next field", Press::Keys(&["tab"])),
            plain("Cancel", Press::Keys(&["escape"])),
        ],
    ),
    (
        "Adding a project",
        &[
            plain("Choose a folder", Press::Keys(&["up", "down"])),
            plain("Go into the folder", Press::Keys(&["tab"])),
            plain("Add it", Press::Keys(&["enter"])),
            plain("Close", Press::Keys(&["escape"])),
        ],
    ),
];

pub(super) fn page(s: &Lemur, k: &Kit) -> Vec<El> {
    let c = &k.c;
    let orders = ThreadOrder::ALL.iter().map(|o| (o.as_str().to_owned(), thread_order_label(*o).to_owned())).collect();
    let order = picker(
        "thread-order",
        "Thread order",
        orders,
        s.prefs.thread_order.as_str(),
        |s, id| s.set_thread_order(ThreadOrder::ALL.into_iter().find(|o| o.as_str() == id).unwrap_or_default()),
        k,
    );
    let help = "The order ⌃Tab goes through the worktree's threads in, and ⌃⇧Tab back, and which one shows when you \
                close one. By last opened starts with the thread opened before this one, as ⌘Tab does with apps.";
    let mut groups = vec![group(None, None, vec![item("thread-order", "Thread order", order, Some(help.into()), k)], k)];
    for (g, (title, list)) in SHORTCUTS.iter().enumerate() {
        let items = list
            .iter()
            .enumerate()
            .map(|(i, shortcut)| {
                let keys = match &shortcut.how {
                    Press::Click(click) => row().child(text(*click).font(k.ty.ui).color(c.text_muted)),
                    how => row().gap(8.0).center_y().children(how.keycaps().into_iter().map(|caps| Keybind::new(&caps).build(k))),
                };
                item(&format!("shortcut-{g}-{i}"), shortcut.what, keys, shortcut.more.map(str::to_owned), k)
            })
            .collect();
        groups.push(group(Some(title), None, items, k));
    }
    groups
}

fn thread_order_label(order: ThreadOrder) -> &'static str {
    match order {
        ThreadOrder::Tabs => "Along the tabs",
        ThreadOrder::Recent => "By last opened",
    }
}
