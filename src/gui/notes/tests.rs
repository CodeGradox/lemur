//! The notes panel, driven as a person would through Rune's `Runtime`, and what its keys and
//! buttons make of the text.

use super::edits::{bulleted, checkbox, indented, link, list_item, moved, next_item, next_marker, pasted_link, tick, ticked, urls};
use super::{Notes, markdown, modifiers};
use crate::config::Config;
use crate::gui::{Lemur, Right, on_key, poll, view};
use crate::notes;
use crate::snapshot::{Activity, BaseView, HostView, Session, Snapshot, Worktree};
use crate::test_support::{HcServer, TempDir};
use rune::clipboard::MemoryClipboard;
use rune::{Env, Key as K, KeyEvent, Mods, PhysicalInput, Runtime};
use rune_kit::Kit;
use std::path::Path;
use crate::gui::tests::{cmd};

fn shift() -> Mods {
    Mods { shift: true, ..Mods::default() }
}

/// The window on `cfg`, keeping notes under `dir`, with a clipboard of its own.
fn window_on(cfg: Config, dir: Option<&Path>) -> (Runtime<Lemur>, MemoryClipboard) {
    let env = Env { system_dark: true, scale: 2.0 };
    let mut s = Lemur::new(cfg, None, None);
    s.panels.notes = Notes::new(dir);
    let mut rt = Runtime::new(s, view, (1280.0, 820.0), env);
    rt.on_key = Some(on_key);
    rt.poll = Some(poll);
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    rt.frame(0.0);
    (rt, clipboard)
}

fn window(dir: &Path) -> (Runtime<Lemur>, MemoryClipboard) {
    window_on(toml::from_str("").unwrap(), Some(dir))
}

fn frames(rt: &mut Runtime<Lemur>, n: usize) {
    for _ in 0..n {
        rt.frame(1.0 / 60.0);
    }
}

/// Opens the notes panel on `worktree`'s notes, as a worktree's menu does.
fn show(rt: &mut Runtime<Lemur>, worktree: &str) {
    rt.update(|s, _| s.open_notes("agent", worktree, "wt"));
    // The panel slides out, and what came in fades in: presses land once it shows.
    frames(rt, 30);
}

fn text(rt: &Runtime<Lemur>) -> String {
    rt.state.panels.notes.edit.text.clone()
}

fn saved(dir: &Path, worktree: &str) -> String {
    notes::load(&notes::file(&dir.join("notes"), "agent", worktree)).unwrap()
}

fn click_at(rt: &mut Runtime<Lemur>, (x, y): (f32, f32)) {
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    frames(rt, 4);
}

fn click(rt: &mut Runtime<Lemur>, key: &str) {
    frames(rt, 20);
    let at = rt.find(key).unwrap_or_else(|| panic!("no element keyed {key}"));
    click_at(rt, at);
}

fn press(rt: &mut Runtime<Lemur>, key: K, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    frames(rt, 1);
}

fn type_text(rt: &mut Runtime<Lemur>, text: &str) {
    for c in text.chars() {
        let shift = c.is_ascii_uppercase();
        rt.key(KeyEvent { key: K::Text(c.to_string()), mods: Mods { shift, ..Mods::default() } });
    }
    frames(rt, 1);
}

/// Holds ⌘ down, or lets it go, as the window tells of it.
fn hold_cmd(rt: &mut Runtime<Lemur>, down: bool) {
    let mods = if down { cmd() } else { Mods::default() };
    rt.physical_input(PhysicalInput::Modifiers(mods));
    rt.update(|s, tx| modifiers(s, mods, tx));
    frames(rt, 2);
}

/// Typing goes straight into the notes and is saved; each worktree keeps
/// its own.
#[test]
fn typing_is_saved_and_each_worktree_keeps_its_own() {
    let dir = TempDir::new("notes-edit");
    notes::save(&notes::file(&dir.join("notes"), "agent", "/srv/b"), "b's notes").unwrap();
    let (mut rt, _) = window(&dir);
    show(&mut rt, "/srv/a");
    assert_eq!(text(&rt), "");

    click(&mut rt, "notes-editor");
    type_text(&mut rt, "See https://example.com");
    assert_eq!(saved(&dir, "/srv/a"), "See https://example.com");

    show(&mut rt, "/srv/b");
    assert_eq!(text(&rt), "b's notes");
    // Showing them didn't write them anywhere else.
    assert_eq!(saved(&dir, "/srv/a"), "See https://example.com");
    show(&mut rt, "/srv/a");
    assert_eq!(text(&rt), "See https://example.com");
}

/// While the notes have the keyboard, keys don't reach the terminal; they
/// give it back on Escape, when they fold away, and when another panel
/// takes their place.
#[test]
fn the_notes_give_the_keyboard_back() {
    let dir = TempDir::new("notes-keyboard");
    let (mut rt, _) = window(&dir);
    show(&mut rt, "/srv/a");
    let typing = |rt: &mut Runtime<Lemur>| {
        frames(rt, 2);
        (rt.state.notes_editing(), rt.state.keyboard_on_terminal())
    };
    assert_eq!(typing(&mut rt), (false, true));

    click(&mut rt, "notes-editor");
    assert_eq!(typing(&mut rt), (true, false));
    press(&mut rt, K::Escape, Mods::default());
    assert_eq!(typing(&mut rt), (false, true), "Escape");
    // Under the text is the notes' too.
    let ([x, y, w, h], _) = rt.presented("notes-page").expect("the page");
    click_at(&mut rt, (x + w / 2.0, y + h - 40.0));
    assert_eq!(typing(&mut rt), (true, false), "a click under the text");
    press(&mut rt, K::Escape, Mods::default());

    click(&mut rt, "notes-editor");
    press(&mut rt, K::Command('r'), cmd());
    assert_eq!(rt.state.right(), None);
    assert_eq!(typing(&mut rt), (false, true), "folded away");
    press(&mut rt, K::Command('r'), cmd());
    assert_eq!(rt.state.right(), Some(Right::Notes));
    assert_eq!(typing(&mut rt), (false, true), "and back, without it");

    click(&mut rt, "notes-editor");
    rt.update(|s, _| s.toggle_right(Right::Changes));
    assert_eq!(typing(&mut rt), (false, true), "another panel");
}

#[test]
fn a_url_in_the_notes_is_found_where_prose_ends_it() {
    let text = "first line\nsee https://example.com/a?b=c, then";
    let at = text.find("https").unwrap();
    assert_eq!(urls(text), [at..at + 25]);
    let text = "See https://example.com/a?b=c. Or (http://x.io/y), or [docs](https://d.dev/p).";
    let found: Vec<&str> = urls(text).into_iter().map(|r| &text[r]).collect();
    assert_eq!(found, ["https://example.com/a?b=c", "http://x.io/y", "https://d.dev/p"]);
    assert!(urls("plain words").is_empty());
}

/// Holding ⌘ over a URL and clicking opens it; a plain click puts the
/// cursor there.
#[test]
fn cmd_click_opens_a_url() {
    let dir = TempDir::new("notes-links");
    let words = "https://example.com/docs and more words";
    notes::save(&notes::file(&dir.join("notes"), "agent", "/srv/a"), words).unwrap();
    let (mut rt, _) = window(&dir);
    show(&mut rt, "/srv/a");
    let ([x, y, _, _], _) = rt.presented("notes-editor").expect("the editor");
    // In the URL, which starts the first line.
    let at = (x + 40.0, y + 8.0);

    assert!(rt.find("notes-url-0").is_none(), "not a link with ⌘ up");
    click_at(&mut rt, at);
    assert!(rt.take_url_requests().is_empty());
    let n = &rt.state.panels.notes;
    assert!(n.edit.focused && (1..24).contains(&n.edit.focus), "the cursor goes in the URL");

    hold_cmd(&mut rt, true);
    let link = rt.find("notes-url-0").expect("a link while ⌘ is down");
    assert!((link.1 - at.1).abs() < 6.0, "over the URL's line: {link:?}, {at:?}");
    click_at(&mut rt, at);
    assert_eq!(rt.take_url_requests(), ["https://example.com/docs"]);
    assert_eq!(text(&rt), words);
    hold_cmd(&mut rt, false);
    assert!(rt.find("notes-url-0").is_none());
}

#[test]
fn a_box_starts_a_list_item() {
    assert_eq!(checkbox("- [ ] milk"), Some(2));
    assert_eq!(checkbox("  * [x] eggs"), Some(4));
    assert_eq!(checkbox("12) [X]"), Some(4));
    assert_eq!(checkbox("+\t[ ]\tbread"), Some(2));
    for not in ["[ ] milk", "-[ ] milk", "- [ ]milk", "- [-] milk", "a. [ ] milk", "- milk [ ]", "1234567890. [ ] milk"] {
        assert_eq!(checkbox(not), None, "{not:?}");
    }
    assert_eq!(next_marker("  9. "), "  10. ");
    assert_eq!(next_marker("- "), "- ");
}

#[test]
fn a_list_item_is_a_marker_and_a_space() {
    assert_eq!(list_item("- milk"), Some(2));
    assert_eq!(list_item("  10) "), Some(6));
    assert_eq!(list_item("- [ ] milk"), Some(2));
    for not in ["-milk", "**bold**", "milk", ""] {
        assert_eq!(list_item(not), None, "{not:?}");
    }
}

#[test]
fn list_buttons_bullet_the_lines_or_take_the_bullets_off() {
    // An empty line gets a bullet, with the cursor after it.
    assert_eq!(bulleted("", 0..0, false), (0..0, "- ".into(), 2..2));
    // The cursor's line keeps its indent, and the cursor its word.
    let text = "a\n  milk\nb";
    assert_eq!(bulleted(text, 5..5, true), (2..8, "  - [ ] milk".into(), 11..11));
    // Of several lines, blank ones stay blank, a plain bullet gets a
    // box, and a ticked one keeps its tick.
    let text = "- milk\n\n- [x] eggs\nbread";
    let boxed = "- [ ] milk\n\n- [x] eggs\n- [ ] bread";
    assert_eq!(bulleted(text, 0..24, true), (0..24, boxed.into(), 0..34));
    // All boxed already, they lose their boxes and bullets.
    assert_eq!(bulleted(boxed, 0..34, true), (0..34, "milk\n\neggs\nbread".into(), 0..16));
    // The bullet button takes a box off and keeps the number.
    assert_eq!(bulleted("1. [x] milk", 8..8, false), (0..11, "1. milk".into(), 4..4));
    // A selection ending where a line starts leaves that line be.
    assert_eq!(bulleted("a\nb\nc", 0..4, false), (0..3, "- a\n- b".into(), 0..8));
}

#[test]
fn a_url_pasted_over_text_links_it() {
    let url = "https://x.io";
    assert_eq!(pasted_link("see docs", 4..8, url), Some((4..8, "[docs](https://x.io)".into(), 24..24)));
    // Not with nothing selected, over a URL, or across lines.
    assert_eq!(pasted_link("see docs", 8..8, url), None);
    assert_eq!(pasted_link("https://a.io", 0..12, url), None);
    assert_eq!(pasted_link("a\nb", 0..3, url), None);
}

#[test]
fn cmd_k_links_the_selection_or_selects_a_links_url() {
    let url = Some("https://x.io");
    // The clipboard's URL comes selected, to type over.
    assert_eq!(link("see docs", 4..8, url), Some((4..8, "[docs](https://x.io)".into(), 11..23)));
    // Without one, the cursor waits where the URL goes.
    assert_eq!(link("see docs", 4..8, None), Some((4..8, "[docs]()".into(), 11..11)));
    // With nothing selected, the cursor goes where the text goes.
    assert_eq!(link("see ", 4..4, url), Some((4..4, "[](https://x.io)".into(), 5..5)));
    // In a link, its URL is selected.
    let text = "a [docs](https://x.io) b";
    assert_eq!(link(text, 4..4, None), Some((4..4, "".into(), 9..21)));
    assert_eq!(link("a\nb", 0..3, url), None);
}

#[test]
fn moving_an_item_takes_its_nested_items_along() {
    // b and b1 go up past a and a1, and back down again.
    let text = "- a\n  - a1\n- b\n  - b1\nend";
    let up = "- b\n  - b1\n- a\n  - a1";
    assert_eq!(moved(text, 13..13, true), Some((0..21, up.into(), 2..2)));
    let text = format!("{up}\nend");
    let down = "- a\n  - a1\n- b\n  - b1";
    assert_eq!(moved(&text, 2..2, false), Some((0..21, down.into(), 13..13)));
    // Plain lines go one at a time, and not past the top or the end.
    assert_eq!(moved("a\nb\n", 2..2, true), Some((0..3, "b\na".into(), 0..0)));
    assert_eq!(moved("a\nb\n", 0..0, true), None);
    assert_eq!(moved("a\nb\n", 2..2, false), None);
    assert_eq!(moved("a\nb\n", 4..4, false), None);
}

#[test]
fn tab_indents_an_item_or_the_lines_selected() {
    // On a list item, wherever the cursor is in it, the whole item.
    assert_eq!(indented("- milk", 3..3, false), Some((0..6, "  - milk".into(), 5..5)));
    assert_eq!(indented("  - milk", 5..5, true), Some((0..8, "- milk".into(), 3..3)));
    // Elsewhere two spaces at the cursor, and ⇧Tab only takes them off.
    assert_eq!(indented("milk", 2..2, false), Some((2..2, "  ".into(), 4..4)));
    assert_eq!(indented("milk", 2..2, true), None);
    assert_eq!(indented("\tmilk", 2..2, true), Some((0..5, "milk".into(), 1..1)));
    // Several lines go together, the blank ones left be.
    assert_eq!(indented("a\n\nb", 0..4, false), Some((0..4, "  a\n\n  b".into(), 0..8)));
}

#[test]
fn enter_goes_on_with_an_item_or_ends_the_list() {
    assert_eq!(next_item("- [x] milk", 10..10), Some((10..10, "\n- [ ] ".into(), 17..17)));
    assert_eq!(next_item("9. eggs", 7..7), Some((7..7, "\n10. ".into(), 12..12)));
    assert_eq!(next_item("a\n- [ ] ", 8..8), Some((2..8, "".into(), 2..2)));
    // Only at an item's end.
    assert_eq!(next_item("- milk", 3..3), None);
    assert_eq!(next_item("milk", 4..4), None);
    assert_eq!(next_item("- milk", 0..6), None);
}

#[test]
fn a_tick_turns_the_box_on_its_line() {
    assert_eq!(tick("a\n- [ ] milk", 12), Some((5..6, "x")));
    assert_eq!(tick("- [X] milk", 0), Some((3..4, " ")));
    assert_eq!(tick("milk", 2), None);
}

#[test]
fn ticked_items_fade_and_their_words_are_struck_through() {
    let text = "- [x] milk  \n- [ ] eggs\n  - [X] bread";
    assert_eq!(ticked(text), vec![(0..10, 6..10), (24..37, 32..37)]);
}

/// Headings are bold, code has a ground, and struck words and ticked
/// items' words are struck through.
#[test]
fn markdown_is_highlighted_as_typed() {
    let k = Kit::neutral(true);
    let look = markdown::look("# Title\nsee `code` and **bold** ~~old~~\n- [x] done", &k);
    let has = |at: std::ops::Range<usize>, what: fn(&rune::Span) -> bool| {
        look.spans.iter().any(|s| (s.start as usize, s.end as usize) == (at.start, at.end) && what(s))
    };
    assert!(has(0..7, |s| s.weight == Some(600.0)), "the heading");
    assert!(has(12..18, |s| s.bg.is_some()), "the code");
    assert!(has(23..31, |s| s.weight == Some(600.0)), "the bold words");
    assert!(has(40..50, |s| s.color.is_some_and(|c| c.a < 1.0)), "the ticked item fades");
    assert_eq!(look.strikes, [34..37, 46..50]);
    // Nothing counts inside code, nor a snake_case name as italics.
    let look = markdown::look("`**not bold**` snake_case_name", &k);
    assert!(look.spans.iter().all(|s| s.weight.is_none() && !s.italic));
}

/// ⌘-clicking a box ticks it, and again unticks it; a plain click only
/// puts the cursor there.
#[test]
fn cmd_click_ticks_a_box() {
    let dir = TempDir::new("notes-ticks");
    notes::save(&notes::file(&dir.join("notes"), "agent", "/srv/a"), "first line\n- [ ] milk").unwrap();
    let (mut rt, _) = window(&dir);
    show(&mut rt, "/srv/a");
    let k = Kit::neutral(true);
    hold_cmd(&mut rt, true);
    let at = rt.find("notes-box-13").expect("the box, while ⌘ is down");
    hold_cmd(&mut rt, false);
    // On the second line, where the field has it.
    let ([_, y, _, _], _) = rt.presented("notes-editor").unwrap();
    assert!((at.1 - y - 27.0).abs() < 6.0, "on the second line: {at:?}, {y}");

    click_at(&mut rt, at);
    assert_eq!(saved(&dir, "/srv/a"), "first line\n- [ ] milk");
    for (now, struck) in [("first line\n- [x] milk", vec![17..21]), ("first line\n- [ ] milk", vec![])] {
        hold_cmd(&mut rt, true);
        click_at(&mut rt, at);
        hold_cmd(&mut rt, false);
        assert_eq!(saved(&dir, "/srv/a"), now);
        assert_eq!(markdown::look(&text(&rt), &k).strikes, struck, "faded and struck through");
    }
    assert!(rt.take_url_requests().is_empty());
}

/// Enter at the end of an item starts the next, numbered on, and on an
/// empty one ends the list; ⇧-Enter is a plain newline, and ⌘-Enter
/// ticks the cursor's line.
#[test]
fn enter_goes_on_with_a_task_list() {
    let dir = TempDir::new("notes-tasks");
    let (mut rt, _) = window(&dir);
    show(&mut rt, "/srv/a");
    click(&mut rt, "notes-editor");

    type_text(&mut rt, "1. [ ] milk");
    press(&mut rt, K::Enter, Mods::default());
    type_text(&mut rt, "eggs");
    press(&mut rt, K::Enter, cmd());
    assert_eq!(text(&rt), "1. [ ] milk\n2. [x] eggs");
    press(&mut rt, K::Enter, Mods::default());
    press(&mut rt, K::Enter, Mods::default());
    type_text(&mut rt, "- [ ] tea");
    press(&mut rt, K::Enter, shift());
    type_text(&mut rt, "done");
    let done = "1. [ ] milk\n2. [x] eggs\n- [ ] tea\ndone";
    assert_eq!(text(&rt), done);
    assert_eq!(saved(&dir, "/srv/a"), done);
}

/// The list buttons bullet the cursor's line, or give it a box, and
/// leave the keyboard in the notes; Enter goes on with plain bullets
/// too.
#[test]
fn list_buttons_bullet_the_cursors_line() {
    let dir = TempDir::new("notes-bullets");
    let (mut rt, _) = window(&dir);
    show(&mut rt, "/srv/a");
    click(&mut rt, "notes-editor");

    click(&mut rt, "notes-bullets");
    type_text(&mut rt, "milk");
    press(&mut rt, K::Enter, Mods::default());
    type_text(&mut rt, "eggs");
    assert_eq!(text(&rt), "- milk\n- eggs");
    click(&mut rt, "notes-boxes");
    type_text(&mut rt, " now");
    press(&mut rt, K::Enter, Mods::default());
    press(&mut rt, K::Enter, Mods::default());
    let done = "- milk\n- [ ] eggs now\n";
    assert_eq!(text(&rt), done);
    assert_eq!(saved(&dir, "/srv/a"), done);
}

/// The notes' own keys, as typed: pasting a URL over text links it,
/// ⇧⌘U ticks, ⇧⌘L makes a checklist, ⌃⌘↑ moves an item, Tab indents
/// one from inside it, and ⌘K in a link selects its URL.
#[test]
fn the_notes_keys() {
    let dir = TempDir::new("notes-keys");
    let (mut rt, clipboard) = window(&dir);
    show(&mut rt, "/srv/a");
    click(&mut rt, "notes-editor");
    let shift_cmd = Mods { shift: true, ..cmd() };

    type_text(&mut rt, "see docs");
    for _ in 0..4 {
        press(&mut rt, K::Left, shift());
    }
    // ⌘V, as the window hands a paste to the field.
    clipboard.set_text("https://x.io\n");
    rt.paste("https://x.io\n");
    frames(&mut rt, 2);
    assert_eq!(text(&rt), "see [docs](https://x.io)");

    press(&mut rt, K::Enter, Mods::default());
    type_text(&mut rt, "- [ ] tea");
    press(&mut rt, K::Command('u'), shift_cmd);
    press(&mut rt, K::Enter, Mods::default());
    press(&mut rt, K::Enter, Mods::default());
    type_text(&mut rt, "milk");
    press(&mut rt, K::Command('l'), shift_cmd);
    press(&mut rt, K::Up, Mods { ctrl: true, ..cmd() });
    assert_eq!(text(&rt), "see [docs](https://x.io)\n- [ ] milk\n- [x] tea");

    press(&mut rt, K::Left, Mods::default());
    press(&mut rt, K::Tab, Mods::default());
    assert_eq!(text(&rt), "see [docs](https://x.io)\n  - [ ] milk\n- [x] tea");

    press(&mut rt, K::Up, cmd());
    for _ in 0..5 {
        press(&mut rt, K::Right, Mods::default());
    }
    press(&mut rt, K::Command('k'), cmd());
    frames(&mut rt, 2);
    type_text(&mut rt, "https://y.io");
    let edited = "see [docs](https://y.io)\n  - [ ] milk\n- [x] tea";
    assert_eq!(text(&rt), edited);
    assert_eq!(saved(&dir, "/srv/a"), edited);
}

fn session(name: &str) -> Session {
    Session {
        name: name.to_string(),
        label: name.to_string(),
        own_label: name.to_string(),
        path: "/tmp".into(),
        idle_secs: 0,
        activity: Activity::Idle,
        waiting_for: None,
        started_claude: false,
        codex: false,
        order: None,
        started: 0,
        claude: None,
        busy: false,
        command: None,
        running: true,
        pid: None,
        shell: None,
        helpers: vec![],
        away: false,
        silent: false,
        issue: None,
    }
}

/// Project 0's worktrees on `local`: each a path, its branch and its threads.
fn snapshot(worktrees: &[(&str, &str, &[&str])]) -> Snapshot {
    let worktrees = worktrees
        .iter()
        .map(|(path, branch, sessions)| Worktree {
            path: path.to_string(),
            branch: Some(branch.to_string()),
            head: None,
            main: false,
            sessions: sessions.iter().map(|s| session(s)).collect(),
            gone: vec![],
        })
        .collect();
    Snapshot {
        hosts: vec![HostView {
            name: "local".into(),
            error: None,
            needs_you: false,
            home: String::new(),
            machine: Default::default(),
            bases: vec![BaseView { index: 0, error: None, worktrees, plain: false }],
            other: vec![],
            gone: vec![],
        }],
    }
}

/// The worktree whose notes show, and whether the panel does.
fn shown(rt: &Runtime<Lemur>) -> (Option<String>, bool) {
    (rt.state.panels.notes.worktree().map(str::to_string), rt.state.right() == Some(Right::Notes))
}

fn api_window(server: &HcServer) -> Runtime<Lemur> {
    let mut cfg = server.config();
    cfg.bases = toml::from_str::<Config>("[[bases]]\nhost = \"local\"\npath = \"/srv/api\"").unwrap().bases;
    window_on(cfg, None).0
}

/// Opening a thread points the notes at its worktree without opening the
/// panel; Notes in a worktree's menu opens it on that one, and opening the
/// thread again takes them back.
#[test]
fn notes_follow_the_opened_thread() {
    let server = HcServer::new("rnotes");
    let mut rt = api_window(&server);
    let snap = snapshot(&[("/srv/api", "main", &[]), ("/srv/api/.claude/worktrees/auth", "worktree-auth", &["api/t"])]);
    rt.update(|s, _| s.set_snapshot(snap.clone()));
    assert_eq!(shown(&rt), (None, false));

    rt.update(|s, _| s.open("local".into(), "api/t".into()));
    assert_eq!(shown(&rt), (Some("/srv/api/.claude/worktrees/auth".into()), false));
    assert_eq!(rt.state.panels.notes.shown.as_ref().map(|s| s.label.as_str()), Some("auth"));
    rt.update(|s, _| s.open_notes("local", "/srv/api", "main"));
    assert_eq!(shown(&rt), (Some("/srv/api".into()), true));
    // A snapshot leaves the ones picked be.
    rt.update(|s, _| s.set_snapshot(snap.clone()));
    assert_eq!(shown(&rt), (Some("/srv/api".into()), true));
    rt.update(|s, _| s.open("local".into(), "api/t".into()));
    assert_eq!(shown(&rt), (Some("/srv/api/.claude/worktrees/auth".into()), true));
}

/// A thread just started isn't listed yet when it shows: the notes go to
/// its worktree once a snapshot lists it.
#[test]
fn the_notes_follow_a_thread_once_its_listed() {
    let server = HcServer::new("rnotes-fresh");
    let mut rt = api_window(&server);
    let web = "/srv/api/.claude/worktrees/web";
    rt.update(|s, _| s.set_snapshot(snapshot(&[("/srv/api", "main", &["api/a"])])));
    rt.update(|s, _| s.open("local".into(), "api/a".into()));
    assert_eq!(shown(&rt).0.as_deref(), Some("/srv/api"));

    rt.update(|s, _| s.open_fresh("local".into(), "api/b".into()));
    assert_eq!(shown(&rt).0.as_deref(), Some("/srv/api"), "not listed yet");
    rt.update(|s, _| s.set_snapshot(snapshot(&[("/srv/api", "main", &["api/a"])])));
    assert_eq!(shown(&rt).0.as_deref(), Some("/srv/api"), "still not");
    rt.update(|s, _| s.set_snapshot(snapshot(&[("/srv/api", "main", &["api/a"]), (web, "worktree-web", &["api/b"])])));
    assert_eq!(rt.state.active.as_ref().map(|k| k.1.as_str()), Some("api/b"));
    assert_eq!(shown(&rt).0.as_deref(), Some(web));
}
