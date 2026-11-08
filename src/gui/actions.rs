//! What the menus and keys do: one action each, run the same way from the menu bar, a key
//! or the command palette.

use super::{Lemur, Right};
use rune::{Chord, Key, KeyEvent, MenuItem, Tx};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Settings,
    AddProject,
    NewTerminal,
    NewThreadHere,
    GoToThread,
    CloseThread,
    PutAway,
    Dictate,
    Refresh,
    CommandPalette,
    ToggleSidebar,
    ToggleRightPanel,
    ToggleNotes,
    ToggleLinear,
    ToggleChanges,
    ToggleLinks,
    ToggleAgents,
    NextThread,
    PreviousThread,
    MoveTabLeft,
    MoveTabRight,
    NextAgent,
    PreviousAgent,
    UpdateCore,
    GoToTab(usize),
    /// ⌘⏎: what was dictated goes to the thread on show.
    PasteDictation,
}

/// Every action with a name, its menu and its key, as the menus and the palette list them.
pub const ACTIONS: &[(Action, &str, &str, Option<&str>)] = &[
    (Action::UpdateCore, "Update herdr-core", "Lemur", None),
    (Action::AddProject, "Add Project…", "File", Some("mod+o")),
    (Action::NewTerminal, "New Terminal", "File", Some("mod+t")),
    (Action::NewThreadHere, "New Thread in Worktree…", "File", Some("mod+j")),
    (Action::GoToThread, "Go to Thread…", "File", Some("mod+p")),
    (Action::CloseThread, "Close Thread", "File", Some("mod+w")),
    (Action::PutAway, "Put Thread Away", "File", None),
    (Action::Dictate, "Dictate", "File", Some("mod+d")),
    (Action::Refresh, "Refresh", "File", None),
    (Action::CommandPalette, "Command Palette…", "View", Some("mod+shift+p")),
    (Action::ToggleSidebar, "Toggle Sidebar", "View", Some("mod+b")),
    (Action::ToggleRightPanel, "Toggle Right Panel", "View", Some("mod+r")),
    (Action::ToggleNotes, "Toggle Notes", "View", None),
    (Action::ToggleLinear, "Toggle Linear", "View", None),
    (Action::ToggleChanges, "Toggle Changes", "View", None),
    (Action::ToggleLinks, "Toggle Links", "View", None),
    (Action::ToggleAgents, "Toggle Agents", "View", Some("mod+shift+a")),
    (Action::NextThread, "Next Thread in Worktree", "View", Some("ctrl+tab")),
    (Action::PreviousThread, "Previous Thread in Worktree", "View", Some("ctrl+shift+tab")),
    (Action::MoveTabLeft, "Move Tab Left", "View", Some("mod+shift+left")),
    (Action::MoveTabRight, "Move Tab Right", "View", Some("mod+shift+right")),
    (Action::NextAgent, "Next Agent", "View", None),
    (Action::PreviousAgent, "Previous Agent", "View", None),
];

impl Action {
    /// The action a key does, if any. ⌘1 to ⌘9 show the tabs by their place.
    pub fn for_key(ev: &KeyEvent, tx: &Tx) -> Option<Action> {
        if let Key::Command(c @ '1'..='9') = ev.key
            && !ev.mods.shift
            && !ev.mods.alt
        {
            return Some(Action::GoToTab(c as usize - '1' as usize));
        }
        if Chord::parse("mod+,").is_ok_and(|c| c.matches_tx(ev, tx)) {
            return Some(Action::Settings);
        }
        if Chord::parse("mod+enter").is_ok_and(|c| c.matches_tx(ev, tx)) {
            return Some(Action::PasteDictation);
        }
        ACTIONS.iter().find_map(|(action, _, _, key)| {
            let chord = Chord::parse((*key)?).ok()?;
            chord.matches_tx(ev, tx).then_some(*action)
        })
    }

    /// The key that does it, as menus show it (`⌘T`).
    pub fn key_label(self) -> Option<String> {
        let (_, _, _, key) = ACTIONS.iter().find(|(a, ..)| *a == self)?;
        Some(Chord::parse((*key)?).ok()?.label())
    }
}

/// Runs `action` on the app.
pub fn run(s: &mut Lemur, action: Action, tx: &mut Tx) {
    match action {
        Action::Settings => s.open_settings(tx),
        Action::AddProject => s.open_picker(),
        Action::NewTerminal => s.new_terminal_here(),
        Action::NewThreadHere => s.new_thread_here(),
        Action::GoToThread => s.find_thread(),
        Action::CloseThread => s.confirm_close(),
        Action::PutAway => {
            if let Some(k) = s.active.clone() {
                s.put_away(k);
            }
        }
        Action::Dictate => s.dictate(),
        Action::Refresh => s.refresh(),
        Action::CommandPalette => s.open_palette(),
        Action::ToggleSidebar => s.sidebar.toggle(),
        Action::ToggleRightPanel => s.toggle_right_panel(),
        Action::ToggleNotes => s.toggle_right(Right::Notes),
        Action::ToggleLinear => s.toggle_right(Right::Linear),
        Action::ToggleChanges => s.toggle_right(Right::Changes),
        Action::ToggleLinks => s.toggle_right(Right::Links),
        Action::ToggleAgents => s.toggle_right(Right::Agents),
        Action::NextThread => s.cycle_threads(false, tx),
        Action::PreviousThread => s.cycle_threads(true, tx),
        Action::MoveTabLeft => s.move_tab(true),
        Action::MoveTabRight => s.move_tab(false),
        Action::NextAgent => s.next_agent(false),
        Action::PreviousAgent => s.next_agent(true),
        Action::UpdateCore => s.update_core(),
        Action::GoToTab(i) => s.go_to_tab(i),
        Action::PasteDictation => s.paste_dictation(),
    }
}

macro_rules! menu_fn {
    ($($name:ident => $action:expr;)*) => {
        $(fn $name(s: &mut Lemur, tx: &mut Tx) { run(s, $action, tx) })*
    };
}

menu_fn! {
    m_settings => Action::Settings;
    m_update_core => Action::UpdateCore;
    m_add_project => Action::AddProject;
    m_new_terminal => Action::NewTerminal;
    m_new_thread_here => Action::NewThreadHere;
    m_go_to_thread => Action::GoToThread;
    m_close_thread => Action::CloseThread;
    m_put_away => Action::PutAway;
    m_dictate => Action::Dictate;
    m_refresh => Action::Refresh;
    m_palette => Action::CommandPalette;
    m_sidebar => Action::ToggleSidebar;
    m_right => Action::ToggleRightPanel;
    m_notes => Action::ToggleNotes;
    m_linear => Action::ToggleLinear;
    m_changes => Action::ToggleChanges;
    m_links => Action::ToggleLinks;
    m_agents => Action::ToggleAgents;
    m_next_thread => Action::NextThread;
    m_previous_thread => Action::PreviousThread;
    m_tab_left => Action::MoveTabLeft;
    m_tab_right => Action::MoveTabRight;
    m_next_agent => Action::NextAgent;
    m_previous_agent => Action::PreviousAgent;
}

fn m_about(s: &mut Lemur, _: &mut Tx) {
    s.about();
}

/// The menu bar as macOS apps have it; shortcuts are the items' key equivalents.
pub fn menus(app: rune::App<Lemur>) -> rune::App<Lemur> {
    let item = |title: &str, f: fn(&mut Lemur, &mut Tx), menu: &str, key: Option<&str>| {
        let mut item = MenuItem::new(title, f).id(title);
        if menu != "Lemur" {
            item = item.menu(menu);
        }
        if let Some(shortcut) = key.and_then(|k| k.replace("mod+", "cmd+").parse().ok()) {
            item = item.key_equivalent(shortcut);
        }
        // Text fields select to the line's ends with ⌘⇧← and ⌘⇧→ first.
        if title.starts_with("Move Tab") {
            item = item.keys_first();
        }
        item
    };
    let fns: &[(Action, fn(&mut Lemur, &mut Tx))] = &[
        (Action::UpdateCore, m_update_core),
        (Action::AddProject, m_add_project),
        (Action::NewTerminal, m_new_terminal),
        (Action::NewThreadHere, m_new_thread_here),
        (Action::GoToThread, m_go_to_thread),
        (Action::CloseThread, m_close_thread),
        (Action::PutAway, m_put_away),
        (Action::Dictate, m_dictate),
        (Action::Refresh, m_refresh),
        (Action::CommandPalette, m_palette),
        (Action::ToggleSidebar, m_sidebar),
        (Action::ToggleRightPanel, m_right),
        (Action::ToggleNotes, m_notes),
        (Action::ToggleLinear, m_linear),
        (Action::ToggleChanges, m_changes),
        (Action::ToggleLinks, m_links),
        (Action::ToggleAgents, m_agents),
        (Action::NextThread, m_next_thread),
        (Action::PreviousThread, m_previous_thread),
        (Action::MoveTabLeft, m_tab_left),
        (Action::MoveTabRight, m_tab_right),
        (Action::NextAgent, m_next_agent),
        (Action::PreviousAgent, m_previous_agent),
    ];
    let mut app = app
        .menu_item(MenuItem::new("About Lemur", m_about).id("about"))
        .menu_item(MenuItem::separator())
        .menu_item(item("Settings…", m_settings, "Lemur", Some("mod+,")))
        .menu_item(MenuItem::separator());
    let mut last = "Lemur";
    for (action, title, menu, key) in ACTIONS {
        let Some((_, f)) = fns.iter().find(|(a, _)| a == action) else { continue };
        if *menu != last && last != "Lemur" {
            app = app.menu_item(MenuItem::separator().menu(last));
        }
        last = menu;
        app = app.menu_item(item(title, *f, menu, *key));
    }
    app
}
