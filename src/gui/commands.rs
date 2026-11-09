//! The command palette (⌘⇧P), as Herdr's: every command by name, with its
//! key. What's typed narrows them down; ↑↓ picks one and ⏎ runs it.

use super::card;
use super::actions::{self, ACTIONS, Action};
use super::{Lemur, icons};
use rune::{Chord, El, Key as K, KeyPlatform, TextEdit, Tx, col, row, text};
use rune_kit::{Keybind, KeybindVariant, Kit, TextInput};

/// How many commands it lists at most.
const SHOWN: usize = 10;

/// What's typed, and the command picked among those listed.
pub struct Commands {
    input: TextEdit,
    picked: usize,
}

/// What a command does: an action, as its menu item and key do, or quitting.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Command {
    Run(Action),
    Quit,
}

/// Every command, by name: what the menus have, but the palette itself. Lemur's own menu's
/// come last, as they're asked for least.
fn commands() -> Vec<(&'static str, Command)> {
    let (lemur, rest): (Vec<_>, Vec<_>) = ACTIONS
        .iter()
        .filter(|(a, ..)| *a != Action::CommandPalette)
        .map(|(a, name, menu, _)| (*name, Command::Run(*a), *menu == "Lemur"))
        .partition(|(.., lemur)| *lemur);
    let named = |(name, command, _)| (name, command);
    rest.into_iter()
        .map(named)
        .chain([("Settings…", Command::Run(Action::Settings))])
        .chain(lemur.into_iter().map(named))
        .chain([("Quit Lemur", Command::Quit)])
        .collect()
}

/// The commands whose name has every word typed, in the palette's order.
fn matching(typed: &str) -> Vec<(&'static str, Command)> {
    let typed = typed.to_lowercase();
    let words: Vec<&str> = typed.split_whitespace().collect();
    commands()
        .into_iter()
        .filter(|(name, _)| {
            let name = name.to_lowercase();
            words.iter().all(|w| name.contains(w))
        })
        .collect()
}

/// The keys that run `command`, one keycap each, as the menus have them.
fn keycaps(command: Command) -> Option<Vec<String>> {
    let key = match command {
        // Neither is in `ACTIONS`: the menus add them apart.
        Command::Run(Action::Settings) => "mod+,",
        Command::Quit => "mod+q",
        Command::Run(action) => ACTIONS.iter().find(|(a, ..)| *a == action)?.3?,
    };
    Some(Chord::parse(key).ok()?.keycaps_on(KeyPlatform::current()))
}

impl Lemur {
    /// ⌘⇧P: opens the palette, over the window, its field with the keyboard.
    pub(super) fn open_palette(&mut self) {
        if self.overlaid() {
            return;
        }
        let mut input = TextEdit::new("");
        input.focused = true;
        self.panels.commands = Some(Commands { input, picked: 0 });
    }

    pub(super) fn close_palette(&mut self) {
        self.panels.commands = None;
    }

    fn palette_matches(&self) -> Vec<(&'static str, Command)> {
        let Some(palette) = &self.panels.commands else { return vec![] };
        matching(&palette.input.text)
    }

    /// Moves the pick one down, or with `back` up, round the list.
    fn step_palette(&mut self, back: bool) {
        let count = self.palette_matches().len().min(SHOWN);
        let Some(palette) = &mut self.panels.commands else { return };
        palette.picked = card::step(palette.picked, count, back);
    }

    /// Runs the command picked, as its key or menu item would, once the palette is gone and
    /// the terminal on show has the keyboard back.
    fn run_picked(&mut self, tx: &mut Tx) {
        let Some(picked) = self.panels.commands.as_ref().map(|p| p.picked) else { return };
        let command = self.palette_matches().into_iter().nth(picked);
        self.close_palette();
        match command {
            Some((_, Command::Run(action))) => actions::run(self, action, tx),
            Some((_, Command::Quit)) => tx.quit(),
            None => {}
        }
    }
}

/// The palette over the window, while it's open.
pub fn overlay(s: &Lemur, k: &Kit, layer: &mut Vec<El>) {
    let Some(palette) = s.panels.commands.as_ref() else { return };
    let c = &k.c;
    let input = TextInput::new(&palette.input)
        .placeholder("Run a command…")
        .width(None)
        .plain()
        .on_edit(|s: &mut Lemur| &mut s.panels.commands.as_mut().expect("the palette").input)
        .on_change(|s: &mut Lemur, _| {
            if let Some(p) = &mut s.panels.commands {
                p.picked = 0;
            }
        })
        .on_submit(|s: &mut Lemur, tx| s.run_picked(tx))
        .on_key(|s: &mut Lemur, ev, tx| match ev.key {
            K::Up => {
                s.step_palette(true);
                tx.instant();
                true
            }
            K::Down => {
                s.step_palette(false);
                tx.instant();
                true
            }
            _ => false,
        })
        .build(k)
        .key("palette-input");
    let mut rows = col().key("commands").padding(4.0).gap(2.0);
    let matches = s.palette_matches();
    if matches.is_empty() {
        rows = rows.child(
            row()
                .key("palette-empty")
                .px(8.0)
                .h(28.0)
                .center_y()
                .child(text("No command by that name").font(k.ty.ui).color(c.text_muted)),
        );
    }
    for (i, (name, command)) in matches.into_iter().take(SHOWN).enumerate() {
        let picked = palette.picked == i;
        let caps = keycaps(command).map(|caps| Keybind::new(&caps).variant(if picked { KeybindVariant::Neutral } else { KeybindVariant::Ghost }).build(k));
        let r = card::pick_row(format!("command-{i}"), None, name.to_string(), None, caps, picked, k);
        rows = rows.child(r.on_press_tx(move |s: &mut Lemur, tx: &mut Tx| {
            if let Some(p) = &mut s.panels.commands {
                p.picked = i;
            }
            s.run_picked(tx);
        }));
    }
    let card = card::surface("palette", card::WIDTH, k)
        .child(card::title("palette-title", &icons::SEARCH, "Commands".into(), k))
        .child(card::field(input))
        .child(card::divider(k))
        .child(rows)
        .child(card::footer("palette-footer", "↑↓ pick · ⏎ run · esc cancel".into(), false, k));
    card::over_window("palette", card, |s: &mut Lemur| s.close_palette(), k, layer);
}

#[cfg(test)]
pub(super) mod tests {
    use super::matching;
    use crate::config::Config;
    use crate::gui::{Action, Lemur, Right, actions, on_key, poll, view};
    use rune::{Env, Key as K, KeyEvent, Mods, Runtime};

    /// The window on `cfg`, as `gui::tests` makes it: no herdr-core asked.
    pub(in crate::gui) fn window(cfg: Config) -> Runtime<Lemur> {
        let env = Env { system_dark: true, scale: 2.0 };
        let mut rt = Runtime::new(Lemur::new(cfg, None, None), view, (1280.0, 820.0), env);
        rt.on_key = Some(on_key);
        rt.poll = Some(poll);
        rt.frame(0.0);
        rt
    }

    pub(in crate::gui) fn press(rt: &mut Runtime<Lemur>, key: K, mods: Mods) {
        rt.key(KeyEvent { key, mods });
        rt.frame(1.0 / 60.0);
    }

    pub(in crate::gui) fn cmd() -> Mods {
        Mods { cmd: true, logo: true, ..Mods::default() }
    }

    /// Presses the element keyed `key`, once what just came in has faded in.
    pub(in crate::gui) fn click(rt: &mut Runtime<Lemur>, key: &str) {
        for _ in 0..20 {
            rt.frame(1.0 / 60.0);
        }
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element keyed {key}"));
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..4 {
            rt.frame(1.0 / 60.0);
        }
    }

    pub(in crate::gui) fn type_text(rt: &mut Runtime<Lemur>, text: &str) {
        for c in text.chars() {
            let shift = c.is_ascii_uppercase();
            rt.key(KeyEvent { key: K::Text(c.to_string()), mods: Mods { shift, ..Mods::default() } });
        }
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn every_word_typed_narrows_the_commands() {
        let names = |typed: &str| matching(typed).into_iter().map(|(n, _)| n).collect::<Vec<_>>();
        assert_eq!(names("toggle ag"), ["Toggle Agents"]);
        assert_eq!(names("AGENT"), ["Toggle Agents", "Next Agent", "Previous Agent"]);
        assert!(names("").len() > 10, "all of them");
        assert!(names("nothing like it").is_empty());
        assert!(!names("").contains(&"Command Palette…"), "not itself");
    }

    /// ⌘⇧P lists every command; what's typed narrows them, ↓ picks the next, and ⏎ runs it
    /// as its key would. Esc closes it.
    #[test]
    fn the_palette_runs_commands_by_name() {
        let mut rt = window(toml::from_str("").unwrap());
        let shift_cmd = Mods { shift: true, ..cmd() };
        let open = |rt: &Runtime<Lemur>| (rt.state.panels.commands.is_some(), rt.state.right());
        press(&mut rt, K::Command('p'), shift_cmd);
        assert_eq!(open(&rt), (true, None));
        assert!(!rt.state.keyboard_on_terminal(), "its field has the keyboard");
        rt.frame(1.0 / 60.0);
        assert!(rt.find("command-0").is_some());
        type_text(&mut rt, "toggle agents");
        press(&mut rt, K::Enter, Mods::default());
        assert_eq!(open(&rt), (false, Some(Right::Agents)), "Toggle Agents ran");
        assert!(rt.state.keyboard_on_terminal(), "the keyboard's back");

        press(&mut rt, K::Command('p'), shift_cmd);
        rt.frame(1.0 / 60.0);
        type_text(&mut rt, "toggle");
        // Toggle Sidebar, Right Panel, Notes: the third.
        press(&mut rt, K::Down, Mods::default());
        press(&mut rt, K::Down, Mods::default());
        press(&mut rt, K::Enter, Mods::default());
        assert_eq!(open(&rt), (false, Some(Right::Notes)), "Toggle Notes ran");

        press(&mut rt, K::Command('p'), shift_cmd);
        rt.frame(1.0 / 60.0);
        press(&mut rt, K::Escape, Mods::default());
        assert_eq!(open(&rt), (false, Some(Right::Notes)));

        // What would open a spotlight waits until the palette's gone: it would open under
        // it, and take the keyboard from it.
        press(&mut rt, K::Command('p'), shift_cmd);
        rt.update(|s, tx| actions::run(s, Action::GoToThread, tx));
        assert!(rt.state.panels.commands.is_some() && rt.state.prompt.is_none());
    }

    /// ⌘O and ⌘D wait under what's over the window, as ⌘P does: the folder picker would open
    /// beneath the palette and take the keyboard, and dictation would listen with its bar
    /// hidden.
    #[test]
    fn the_picker_and_dictation_wait_under_the_palette() {
        let mut rt = window(toml::from_str("").unwrap());
        press(&mut rt, K::Command('p'), Mods { shift: true, ..cmd() });
        // Their menu items' keys reach them whatever has the keyboard.
        rt.update(|s, tx| actions::run(s, Action::AddProject, tx));
        rt.update(|s, tx| actions::run(s, Action::Dictate, tx));
        rt.frame(1.0 / 60.0);
        assert!(rt.state.panels.commands.is_some());
        assert!(rt.state.panels.picker.is_none(), "no picker under it");
        assert!(rt.state.panels.dictation.is_none(), "no dictation under it");
    }

    /// Clicking a command runs it.
    #[test]
    fn a_click_runs_a_command() {
        let mut rt = window(toml::from_str("").unwrap());
        rt.update(|s, _| s.open_palette());
        rt.frame(1.0 / 60.0);
        type_text(&mut rt, "toggle sidebar");
        click(&mut rt, "command-0");
        assert!(rt.state.panels.commands.is_none());
        assert!(!rt.state.sidebar.is_open());
    }
}
