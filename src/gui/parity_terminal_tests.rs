//! v2's tests of the terminal (its view and screen, keys, links and colours), checked again
//! on v3: Rune draws the window and takes the input, Ghostty's core keeps the screen and
//! encodes keys and the mouse.

use super::core::wait_for;
use super::grid::{Grid, Size};
use super::palette::{DARK, LIGHT, Palette, Slot, rgb as hex_rgb, set_color};
use super::term_input::on_ime;
use super::term_links::{hyperlinked, link_at};
use super::terminal::{OUTPUT_EVERY, PAD};
use super::tests::{key, open, type_text, window};
use super::{Key, Lemur};
use crate::config::{Terminal, TerminalColors};
use crate::test_support::{HcServer, TempDir};
use libghostty_vt::mouse::{Action as MouseAction, Button};
use rune::{Ime, Key as K, KeyEvent, Mods, PhysicalInput, Runtime};
use std::time::{Duration, Instant};

const NONE: Mods = Mods { shift: false, cmd: false, alt: false, ctrl: false, logo: false };

fn grid(cols: u16, lines: u16) -> Grid {
    Grid::new(Size { cols, lines, cell_width: 16, cell_height: 32 }, 1000, &DARK).unwrap()
}

/// The bytes `g` sends for `key` with `mods`, typing `base` unmodified.
fn sent(g: &mut Grid, key: K, mods: Mods, base: Option<char>) -> Option<Vec<u8>> {
    g.key(&KeyEvent { key, mods }, base)
}

fn t(s: &str) -> K {
    K::Text(s.into())
}

// keys.rs

/// Named keys, and Control chords, as xterm sends them.
#[test]
fn control_and_named_keys() {
    let mut g = grid(80, 24);
    let ctrl = Mods { ctrl: true, ..NONE };
    let shift = Mods { shift: true, ..NONE };
    let alt = Mods { alt: true, ..NONE };
    assert_eq!(sent(&mut g, t("c"), ctrl, Some('c')).unwrap(), [3]);
    assert_eq!(sent(&mut g, t(" "), ctrl, Some(' ')).unwrap(), [0]);
    assert_eq!(sent(&mut g, K::Enter, NONE, None).unwrap(), b"\r");
    assert_eq!(sent(&mut g, K::Enter, shift, None).unwrap(), b"\x1b\r");
    assert_eq!(sent(&mut g, K::Enter, alt, None).unwrap(), b"\x1b\r");
    assert_eq!(sent(&mut g, K::Backspace, NONE, None).unwrap(), b"\x7f");
    // ⌥⌫ deletes a word, as in v2.
    assert_eq!(sent(&mut g, K::Backspace, alt, None).unwrap(), b"\x1b\x7f");
    assert_eq!(sent(&mut g, K::Tab, NONE, None).unwrap(), b"\t");
    assert_eq!(sent(&mut g, K::Tab, shift, None).unwrap(), b"\x1b[Z");
    assert_eq!(sent(&mut g, K::Escape, NONE, None).unwrap(), b"\x1b");
    assert_eq!(sent(&mut g, K::Up, NONE, None).unwrap(), b"\x1b[A");
    assert_eq!(sent(&mut g, K::Left, alt, None).unwrap(), b"\x1b[1;3D");
    assert_eq!(sent(&mut g, K::Home, NONE, None).unwrap(), b"\x1b[H");
    assert_eq!(sent(&mut g, K::Delete, NONE, None).unwrap(), b"\x1b[3~");
    assert_eq!(sent(&mut g, K::PageDown, NONE, None).unwrap(), b"\x1b[6~");
    assert_eq!(sent(&mut g, K::Function(1), NONE, None).unwrap(), b"\x1bOP");
    assert_eq!(sent(&mut g, K::Function(5), NONE, None).unwrap(), b"\x1b[15~");
    assert_eq!(sent(&mut g, K::Function(12), NONE, None).unwrap(), b"\x1b[24~");
    // In the program's cursor key mode (DECCKM).
    g.write(b"\x1b[?1h");
    assert_eq!(sent(&mut g, K::Up, NONE, None).unwrap(), b"\x1bOA");
}

/// What a key types goes as it is, a held key's repeats too (v3 sends both alike): Option
/// composes characters on most layouts, so it's text, not Meta.
#[test]
fn held_keys_type_what_they_type() {
    let mut g = grid(80, 24);
    assert_eq!(sent(&mut g, t("a"), NONE, Some('a')).unwrap(), b"a");
    assert_eq!(sent(&mut g, t("å"), NONE, Some('å')).unwrap(), "å".as_bytes());
    // Option+7 on a Norwegian layout types |.
    let alt = Mods { alt: true, ..NONE };
    assert_eq!(sent(&mut g, t("|"), alt, Some('7')).unwrap(), b"|");
    // Option+f on a US one, ƒ.
    assert_eq!(sent(&mut g, t("ƒ"), alt, Some('f')).unwrap(), "ƒ".as_bytes());
    assert_eq!(sent(&mut g, t("c"), Mods { ctrl: true, ..NONE }, Some('c')).unwrap(), [3]);
}

/// Option types its character in a program that turned on the kitty keyboard protocol, as
/// fish 4 does at its prompt: Option+7 on a Norwegian layout is |, not Alt+7. (v2 offered no
/// kitty protocol, so programs got the text.)
#[test]
fn option_types_its_character_in_a_kitty_keyboard_program() {
    let alt = Mods { alt: true, ..NONE };
    // Disambiguate, and fish's own flags.
    for flags in [&b"\x1b[>1u"[..], b"\x1b[=5u"] {
        let mut g = grid(80, 24);
        g.write(flags);
        assert_eq!(sent(&mut g, t("|"), alt, Some('7')).as_deref(), Some(&b"|"[..]), "Option+7");
        assert_eq!(sent(&mut g, t("@"), alt, Some('2')).as_deref(), Some(&b"@"[..]), "Option+2");
        // Option with nothing composed is still Alt.
        assert_eq!(sent(&mut g, K::Backspace, alt, None).unwrap(), b"\x1b[127;3u");
    }
}

/// Caps Lock capitalizes letters only. macOS puts it in the text a key types (v2's GPUI
/// left it out), so the capital comes as it is, and a Control chord still makes its byte.
#[test]
fn caps_lock_capitalizes_letters() {
    let mut g = grid(80, 24);
    assert_eq!(sent(&mut g, t("A"), NONE, Some('a')).unwrap(), b"A");
    assert_eq!(sent(&mut g, t("Å"), NONE, Some('å')).unwrap(), "Å".as_bytes());
    assert_eq!(sent(&mut g, t("A"), Mods { shift: true, ..NONE }, Some('a')).unwrap(), b"A");
    assert_eq!(sent(&mut g, t("1"), NONE, Some('1')).unwrap(), b"1");
    let ctrl = Mods { ctrl: true, ..NONE };
    assert_eq!(sent(&mut g, t("C"), ctrl, Some('c')).unwrap(), [3]);
}

/// Mouse reports in SGR (1006), for each button, drag and modifier. (The paste half of
/// v2's test is `keys::pastes_are_bracketed_and_cleaned`.)
#[test]
fn mouse_and_paste() {
    let mut g = grid(80, 24);
    // Any motion, SGR.
    g.write(b"\x1b[?1003h\x1b[?1006h");
    let cell = (8.0, 16.0);
    let at = |col: f32, line: f32| ((col + 0.5) * cell.0, (line + 0.5) * cell.1);
    let report = |g: &mut Grid, action, button, (col, line), mods, pressed| String::from_utf8(g.mouse_report(action, Some(button), at(col, line), mods, cell, pressed)).unwrap();
    assert_eq!(report(&mut g, MouseAction::Press, Button::Four, (0.0, 4.0), NONE, false), "\x1b[<64;1;5M");
    assert_eq!(report(&mut g, MouseAction::Motion, Button::Left, (9.0, 0.0), NONE, true), "\x1b[<32;10;1M");
    assert_eq!(report(&mut g, MouseAction::Release, Button::Left, (9.0, 0.0), NONE, false), "\x1b[<0;10;1m");
    assert_eq!(report(&mut g, MouseAction::Press, Button::Middle, (0.0, 0.0), NONE, true), "\x1b[<1;1;1M");
    assert_eq!(report(&mut g, MouseAction::Motion, Button::Right, (1.0, 0.0), NONE, true), "\x1b[<34;2;1M");
    let all = Mods { shift: true, alt: true, ctrl: true, ..NONE };
    assert_eq!(report(&mut g, MouseAction::Press, Button::Five, (0.0, 0.0), all, false), "\x1b[<93;1;1M");
}

// terminal_view.rs: mouse_reports_take_the_encoding_asked_for

/// The encoding a program asked for: X10's, which can't say which button came up nor cells
/// past 223; UTF-8's (1005), which can; SGR's.
#[test]
fn mouse_reports_take_the_encoding_asked_for() {
    let cell = (8.0, 16.0);
    let at = |col: f32| ((col + 0.5) * cell.0, 0.5 * cell.1);
    let report = |g: &mut Grid, button, col: f32, pressed| {
        let action = if pressed { MouseAction::Press } else { MouseAction::Release };
        g.mouse_report(action, Some(button), at(col), NONE, cell, pressed)
    };
    let mut g = grid(400, 4);
    g.write(b"\x1b[?1000h");
    assert_eq!(report(&mut g, Button::Left, 0.0, true), b"\x1b[M !!");
    assert_eq!(report(&mut g, Button::Left, 0.0, false), b"\x1b[M#!!");
    assert_eq!(report(&mut g, Button::Left, 300.0, true), b"", "past 223");
    // That a wheel doesn't come up is the caller's: `term_wheel` sends presses only (see
    // `clicks_reach_a_program_without_sgr`).
    g.write(b"\x1b[?1005h");
    // Column 101: 32 + 101 is U+0085, two bytes in UTF-8.
    assert_eq!(report(&mut g, Button::Left, 100.0, true), b"\x1b[M \xc2\x85!");
    g.write(b"\x1b[?1005l\x1b[?1006h");
    assert_eq!(report(&mut g, Button::Left, 0.0, true), b"\x1b[<0;1;1M");
}

// links.rs

/// A URL the terminal wrapped is one link, from either row.
#[test]
fn a_url_runs_on_across_wrapped_rows() {
    let mut g = grid(30, 6);
    g.write(b"go https://example.com/a/long/path/here now");
    g.refresh().unwrap();
    let link = link_at(&g.line_cells(1), (1, 2)).expect("on the wrapped row");
    assert_eq!(link.url, "https://example.com/a/long/path/here");
    assert_eq!(link.spans, [(0, 3, 30), (1, 0, 9)]);
    assert!(link_at(&g.line_cells(0), (0, 0)).is_none());
}

/// A path printed with its line is a link to its file, underlined with its line.
#[test]
fn a_path_written_out_links_to_its_file() {
    let mut g = grid(30, 6);
    g.write(b"edit src/gui/links.rs:42 now");
    g.refresh().unwrap();
    let link = link_at(&g.line_cells(0), (0, 8)).expect("on the path");
    assert_eq!((link.url.as_str(), link.file, link.place.as_str()), ("src/gui/links.rs", true, ":42"));
    assert_eq!(link.spans, [(0, 5, 24)]);
    assert!(link_at(&g.line_cells(0), (0, 26)).is_none(), "now");
}

/// Text can hide any URI; only a web link's opens, whatever the case of its scheme.
#[test]
fn an_osc_8_link_elsewhere_doesnt_open() {
    let mut g = grid(40, 3);
    g.write(b"\x1b]8;;file:///etc/passwd\x1b\\safe\x1b]8;;\x1b\\ \x1b]8;;HTTPS://x.dev\x1b\\web\x1b]8;;\x1b\\ \x1b]8;;javascript:alert(1)\x1b\\js\x1b]8;;\x1b\\ end");
    g.refresh().unwrap();
    // As `Lemur::link_under` finds it.
    let found = |col: u16| {
        let uri = g.hyperlink(col, 0)?;
        let cells = g.line_cells(0);
        let here = cells.iter().rposition(|&(_, r, c)| (r, c) <= (0, col))?;
        hyperlinked(uri.clone(), &cells, here, |r, c| g.hyperlink(c, r).as_deref() == Some(uri.as_str()))
    };
    assert!(g.hyperlink(1, 0).is_some(), "the program linked it");
    assert!(found(1).is_none(), "a file");
    assert_eq!(found(6).expect("the web link").url, "HTTPS://x.dev");
    assert!(found(10).is_none(), "a script");
}

// terminal.rs

/// Faint text moves toward the background, in a light Lemur as in a dark one.
#[test]
fn light_palette_and_dimming() {
    assert_eq!(Palette::default_for(false), &LIGHT);
    for palette in [&LIGHT, &DARK] {
        let mut g = Grid::new(Size { cols: 20, lines: 2, cell_width: 16, cell_height: 32 }, 100, palette).unwrap();
        g.write(b"plain \x1b[2mfaint");
        g.refresh().unwrap();
        let frame = g.frame();
        assert_eq!(frame.bg, super::grid::rgb(libghostty_vt::style::RgbColor { r: palette.background.r, g: palette.background.g, b: palette.background.b }));
        let (plain, faint) = (frame.rows[0].runs[0].fg, frame.rows[0].runs[1].fg);
        let toward = |c: rune::Color| (c.r - frame.bg.r).abs();
        assert!(toward(faint) < toward(plain), "{plain:?} -> {faint:?} on {:?}", frame.bg);
    }
}

/// Colours set in Settings go over Lemur's own: only what differs is kept, light is left
/// alone, and Lemur's own back empties it again.
#[test]
fn settings_colors_go_over_lemens_own() {
    let mut terminal = Terminal::default();
    assert_eq!(Palette::themed(true, &terminal), DARK);
    set_color(&mut terminal, true, Slot::Background, Some(hex_rgb(0x000000)));
    set_color(&mut terminal, true, Slot::Ansi(9), Some(hex_rgb(0xff0000)));
    let dark = Palette::themed(true, &terminal);
    assert_eq!(dark.background, hex_rgb(0x000000));
    assert_eq!(dark.get(Slot::Ansi(9)), hex_rgb(0xff0000));
    assert_eq!(dark.get(Slot::Ansi(1)), DARK.get(Slot::Ansi(1)));
    assert_eq!(terminal.dark.background.as_deref(), Some("#000000"));
    assert_eq!((terminal.dark.foreground.as_ref(), terminal.dark.ansi.as_ref()), (None, None));
    assert_eq!(terminal.dark.brights.as_ref().map(|b| b[1].as_str()), Some("#ff0000"));
    assert_eq!(Palette::themed(false, &terminal), LIGHT);
    set_color(&mut terminal, true, Slot::Background, None);
    set_color(&mut terminal, true, Slot::Ansi(9), None);
    assert_eq!(terminal.dark, TerminalColors::default());
}

/// The grid takes a new size, and says when it changed.
#[test]
fn the_grid_follows_resizes() {
    let size = Size { cols: 40, lines: 5, cell_width: 8, cell_height: 16 };
    let mut g = Grid::new(size, 100, &DARK).unwrap();
    assert!(!g.resize(size).unwrap(), "no change");
    let larger = Size { cols: 60, lines: 8, ..size };
    assert!(g.resize(larger).unwrap());
    g.refresh().unwrap();
    assert_eq!(g.size(), larger);
    assert_eq!(g.frame().rows.len(), 8);
}

// terminal_view.rs: the screen

/// Rows reach the painter without the blanks after their text.
#[test]
fn trailing_blanks_are_not_drawn() {
    let mut g = grid(40, 3);
    g.write(b"hi");
    g.refresh().unwrap();
    let frame = g.frame();
    assert_eq!(frame.rows[0].text(), "hi");
    assert_eq!(frame.rows[0].runs.iter().map(|r| r.cells).sum::<u16>(), 2);
    assert!(frame.rows[0].fills.is_empty(), "the background is the frame's");
    assert!(frame.rows[1].runs.is_empty());
}

/// Scrolled back so the cursor's row is below the rows shown, the cursor isn't drawn there.
#[test]
fn a_cursor_scrolled_out_of_view_isnt_drawn() {
    let mut g = grid(40, 3);
    g.write(b"1\r\n2\r\n3\r\n4\r\n$ ");
    g.refresh().unwrap();
    assert!(g.frame().cursor.is_some());
    g.scroll(-1);
    g.refresh().unwrap();
    assert_eq!(g.frame().cursor, None);
    g.scroll_to_bottom();
    g.refresh().unwrap();
    assert_eq!(g.frame().cursor, Some((2, 2)));
}

// terminal_view.rs: the view, against a core of the tests' own

/// The window on `server`'s core, once its first snapshot is in.
fn loaded(server: &HcServer) -> Runtime<Lemur> {
    let mut rt = window(server.config());
    rt.state.refresh();
    assert!(wait_for(&mut rt, 10.0, |s| s.loaded), "the first snapshot");
    rt
}

/// A thread running `argv` in `dir`, on show, attached and painted.
fn shown_in(server: &HcServer, dir: &str, argv: &[&str]) -> (Runtime<Lemur>, Key, String) {
    let t = server.run(dir, &[], argv);
    let mut rt = loaded(server);
    open(&mut rt, &t);
    let k = key(&t);
    assert!(wait_for(&mut rt, 10.0, |s| s.terminals.get(&k).is_some_and(|v| v.painted)), "painted");
    (rt, k, t)
}

fn lines(rt: &Runtime<Lemur>, k: &Key) -> Vec<String> {
    super::terminal::lines(&rt.state, k)
}

/// Waits, running the window, for the terminal to show `text`.
fn shows(rt: &mut Runtime<Lemur>, k: &Key, text: &str) -> bool {
    wait_for(rt, 10.0, |s| super::terminal::lines(s, k).iter().any(|l| l.contains(text)))
}

/// What herdr-core has on the program's screen.
fn read(server: &HcServer, t: &str) -> String {
    server.hc(&["read", t])
}

/// Waits, running the window, for herdr-core to have `text` on the program's screen.
fn reads(rt: &mut Runtime<Lemur>, server: &HcServer, t: &str, text: &str) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if read(server, t).contains(text) {
            return true;
        }
        let _ = wait_for(rt, 0.1, |_| false);
    }
    false
}

/// Where (`col`, `row`) cells from the terminal on show's top-left are in the window.
fn cell_point(rt: &Runtime<Lemur>, col: f32, row: f32) -> (f32, f32) {
    let (r, _) = rt.presented("screen-main").expect("the terminal on show");
    let m = rt.state.metrics;
    (r[0] + PAD + col * m.cell_w, r[1] + PAD + row * m.cell_h)
}

fn frames(rt: &mut Runtime<Lemur>, n: usize) {
    for _ in 0..n {
        rt.frame(1.0 / 60.0);
    }
}

fn click_at(rt: &mut Runtime<Lemur>, (x, y): (f32, f32)) {
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

fn tracking(rt: &mut Runtime<Lemur>, k: &Key) -> bool {
    wait_for(rt, 10.0, |s| s.terminals[k].grid.as_ref().is_some_and(|g| g.mouse_tracking()))
}

/// An update shown all at once that never says it's done shows at its deadline, though a
/// second one moved that on.
#[test]
fn an_unfinished_synchronized_update_shows_at_its_deadline() {
    let server = HcServer::new("psyncdl");
    let script = r"sleep 0.3; printf '\033[?2026hAA'; sleep 0.1; printf '\033[?2026hBB'; exec sleep 600";
    let (mut rt, k, _) = shown_in(&server, "/", &["sh", "-c", script]);
    assert!(wait_for(&mut rt, 5.0, |s| super::terminal::lines(s, &k).iter().any(|l| l.contains("AABB"))), "{:?}", lines(&rt, &k));
}

/// Attaching again and failing, as to a host that's down, leaves what the terminal showed.
#[test]
fn a_failed_reattach_leaves_what_showed() {
    let server = HcServer::new("preattach");
    let (mut rt, k, _) = shown_in(&server, "/", &["sh", "-c", "echo hello there; exec sleep 600"]);
    assert!(shows(&mut rt, &k, "hello there"));
    rt.update(|s, _| {
        // A terminal herdr-core doesn't have: the attach fails.
        s.terminals.get_mut(&k).unwrap().session = "no/such".into();
        s.connect(&k);
    });
    assert!(wait_for(&mut rt, 10.0, |s| s.terminals[&k].exited()), "the attach failed");
    frames(&mut rt, 10);
    assert!(rt.state.terminals[&k].status.is_some(), "it says so");
    assert!(lines(&rt, &k).iter().any(|l| l.contains("hello there")), "{:?}", lines(&rt, &k));
}

/// Input given while the terminal attaches, the first time or again, waits for it, then
/// reaches the program; a paste goes as the program's modes say once the core has painted
/// them, here bracketed.
#[test]
fn input_given_while_attaching_reaches_the_program() {
    let server = HcServer::new("pearly");
    let t = server.run("/", &[], &["sh", "-c", "printf '\\033[?2004h'; exec cat -v"]);
    let mut rt = loaded(&server);
    let k = key(&t);
    rt.update(|s, _| {
        s.open("local".into(), t.clone());
        let v = s.terminals.get_mut(&k).expect("its terminal");
        assert!(!v.is_attached(), "still attaching");
        v.paste_text("hello");
        v.write(b"!".to_vec());
    });
    assert!(reads(&mut rt, &server, &t, "^[[200~hello^[[201~!"), "{}", read(&server, &t));
    rt.update(|s, _| {
        // Attaching again, as on a reconnect.
        s.connect(&k);
        let v = s.terminals.get_mut(&k).unwrap();
        assert!(!v.is_attached(), "still attaching");
        v.paste_text("again");
        v.write(b"?".to_vec());
    });
    assert!(reads(&mut rt, &server, &t, "^[[200~again^[[201~?"), "{}", read(&server, &t));
}

/// Input given as the paint comes goes once it's over, however it's split: herdr-core sets a
/// program's modes after its scrollback, so a paste sent after the first part would go out
/// unbracketed, and its newlines act as Enter. It keeps its order with what's typed after it.
#[test]
fn input_waits_for_the_whole_first_paint() {
    let server = HcServer::new("ppaint");
    let (mut rt, k, t) = shown_in(&server, "/", &["sh", "-c", "exec cat -v"]);
    rt.update(|s, _| {
        let v = s.terminals.get_mut(&k).unwrap();
        // As if attached just now, the paint under way: its modes come last.
        v.painted = false;
        v.feed(b"\x1b[?2004l");
        v.paste_text("a\nb");
        v.write(b"!".to_vec());
    });
    // The rest a moment later, sooner than the stream counts as quiet.
    let _ = wait_for(&mut rt, 0.05, |_| false);
    rt.update(|s, _| s.terminals.get_mut(&k).unwrap().feed(b"\x1b[?2004h"));
    assert!(reads(&mut rt, &server, &t, "^[[200~a"), "{}", read(&server, &t));
    assert!(reads(&mut rt, &server, &t, "b^[[201~!"), "{}", read(&server, &t));
}

/// The wheel on the other screen is arrow keys, as the program's cursor key mode has them,
/// unless it turned that off (`?1007l`).
#[test]
fn the_wheel_sends_the_arrow_keys_a_program_asked_for() {
    let server = HcServer::new("pwheelk");
    let script = r"printf '\033[?1049h\033[?1h'; exec cat -v";
    let (mut rt, k, t) = shown_in(&server, "/", &["sh", "-c", script]);
    let wheel = |rt: &mut Runtime<Lemur>, up: bool| {
        let (x, y) = cell_point(rt, 2.5, 2.5);
        let dy = rt.state.metrics.cell_h * if up { 1.0 } else { -1.0 };
        rt.wheel_xy(x, y, 0.0, dy);
        frames(rt, 2);
    };
    wheel(&mut rt, true);
    assert!(reads(&mut rt, &server, &t, "^[OA"), "{}", read(&server, &t));
    rt.update(|s, _| s.terminals.get_mut(&k).unwrap().feed(b"\x1b[?1l"));
    wheel(&mut rt, false);
    assert!(reads(&mut rt, &server, &t, "^[OA^[[B"), "{}", read(&server, &t));
    rt.update(|s, _| s.terminals.get_mut(&k).unwrap().feed(b"\x1b[?1007l"));
    wheel(&mut rt, true);
    // What comes after shows that nothing came before it.
    rt.update(|s, _| s.terminals.get_mut(&k).unwrap().write(b"!".to_vec()));
    assert!(reads(&mut rt, &server, &t, "^[OA^[[B!"), "{}", read(&server, &t));
}

/// Output is drawn at most every `OUTPUT_EVERY`, however much comes meanwhile, and the
/// window wakes for it; right after typing, at once, so the echo doesn't lag.
#[test]
fn output_is_drawn_about_30_times_a_second() {
    let server = HcServer::new("pdraws");
    let (mut rt, k, _) = shown_in(&server, "/", &["sleep", "600"]);
    let _ = wait_for(&mut rt, 0.1, |_| false);
    let drawn = |rt: &Runtime<Lemur>, text: &str| rt.elements().into_iter().any(|e| !e.exiting && e.text.as_deref() == Some(text));
    rt.update(|s, _| {
        let v = s.terminals.get_mut(&k).unwrap();
        v.typed_at = None;
        v.drawn_at = Some(Instant::now());
        for i in 0..50 {
            v.feed(format!("line {i}\r\n").as_bytes());
        }
    });
    rt.frame(1.0 / 60.0);
    assert!(!lines(&rt, &k).iter().any(|l| l == "line 49"), "not yet");
    let clock = rt.clock;
    let wake = rt.wake_deadline().expect("the window is to wake for it");
    assert!(wake - clock <= OUTPUT_EVERY.as_secs_f64() + 0.001, "it wakes in {}", wake - clock);
    std::thread::sleep(OUTPUT_EVERY);
    frames(&mut rt, 2);
    assert!(lines(&rt, &k).iter().any(|l| l == "line 49"), "once for all fifty");
    assert!(drawn(&rt, "line 49"), "and drawn");

    rt.update(|s, _| {
        let v = s.terminals.get_mut(&k).unwrap();
        v.drawn_at = Some(Instant::now());
        v.typed_at = Some(Instant::now());
        v.feed(b"typed");
    });
    rt.frame(1.0 / 60.0);
    assert!(lines(&rt, &k).iter().any(|l| l == "typed"), "typing shows at once: {:?}", lines(&rt, &k));
}

/// Holding ⌘ over a URL shows it, and ⌘-clicking opens it; a plain click doesn't.
#[test]
fn cmd_click_opens_a_link() {
    let server = HcServer::new("plinks");
    let (mut rt, k, _) = shown_in(&server, "/", &["sh", "-c", "printf 'see https://example.com/x ok'; exec sleep 600"]);
    assert!(shows(&mut rt, &k, "example.com/x ok"));
    let row = lines(&rt, &k).iter().position(|l| l.contains("example")).unwrap() as f32;
    // The middle of the URL's third character.
    let at = cell_point(&rt, 6.5, row + 0.5);
    click_at(&mut rt, at);
    assert!(rt.take_url_requests().is_empty(), "a plain click");
    let cmd = Mods { cmd: true, logo: true, ..NONE };
    rt.physical_input(PhysicalInput::Modifiers(cmd));
    rt.pointer_move(at.0 + 1.0, at.1);
    frames(&mut rt, 2);
    let hovered = rt.state.terminals[&k].hover.as_ref().map(|l| l.url.clone());
    assert_eq!(hovered.as_deref(), Some("https://example.com/x"));
    click_at(&mut rt, at);
    assert_eq!(rt.take_url_requests(), ["https://example.com/x"]);
}

/// The mouse lands on the cell drawn under it all the way across, though cells are rarely a
/// whole number of points wide.
#[test]
fn the_mouse_lands_on_the_cell_under_it() {
    let server = HcServer::new("pmousecell");
    let (mut rt, k, t) = shown_in(&server, "/", &["sh", "-c", r"printf '\033[?1000h\033[?1006h'; exec cat -v"]);
    assert!(tracking(&mut rt, &k));
    let size = rt.state.terminals[&k].size;
    let (cols, lines) = (f32::from(size.cols), f32::from(size.lines));
    // The middle of the last cell, as drawn.
    let at = cell_point(&rt, cols - 0.5, lines - 0.5);
    click_at(&mut rt, at);
    let want = format!("^[[<0;{};{}M^[[<0;{};{}m", size.cols, size.lines, size.cols, size.lines);
    assert!(reads(&mut rt, &server, &t, &want), "{want} in {}", read(&server, &t));
    // And its first row's last cell, its last row's first.
    let (a, b) = (cell_point(&rt, cols - 0.5, 0.5), cell_point(&rt, 0.5, lines - 0.5));
    click_at(&mut rt, a);
    click_at(&mut rt, b);
    let want = format!("^[[<0;{};1M^[[<0;{};1m^[[<0;1;{}M", size.cols, size.cols, size.lines);
    assert!(reads(&mut rt, &server, &t, &want), "{want} in {}", read(&server, &t));
}

/// A selection dragged off the terminal follows the pointer to the edge, and ⌘C copies it;
/// typing lets go of it.
#[test]
fn a_drag_off_the_terminal_still_selects() {
    let server = HcServer::new("pdragoff");
    let (mut rt, k, _) = shown_in(&server, "/", &["sh", "-c", "printf 'hello world'; exec sleep 600"]);
    assert!(shows(&mut rt, &k, "hello world"));
    let row = lines(&rt, &k).iter().position(|l| l.contains("hello")).unwrap() as f32;
    let (x0, y) = cell_point(&rt, 0.2, row + 0.5);
    let (x1, _) = cell_point(&rt, 2.5, row + 0.5);
    rt.pointer_move(x0, y);
    rt.pointer_down(x0, y);
    for i in 1..=4 {
        rt.pointer_move(x0 + (x1 - x0) * i as f32 / 4.0, y);
        rt.frame(1.0 / 60.0);
    }
    // Out of the terminal to the right of its row, and out of the window, and let go there.
    let (r, _) = rt.presented("screen-main").unwrap();
    let off = r[0] + r[2] + 40.0;
    rt.pointer_move(off, y);
    rt.frame(1.0 / 60.0);
    rt.pointer_up(off, y);
    rt.frame(1.0 / 60.0);
    let selected = |rt: &Runtime<Lemur>| rt.state.terminals[&k].grid.as_ref().unwrap().selected_text();
    assert_eq!(selected(&rt).as_deref(), Some("hello world"));
    let mut copied = false;
    rt.update(|s, tx| copied = s.copy_selection(tx));
    assert!(copied);
    type_text(&mut rt, "x");
    assert_eq!(selected(&rt), None);
}

/// Hands `ime` to the window as its input method would.
fn ime(rt: &mut Runtime<Lemur>, ime: Ime) {
    let mut took = false;
    rt.update(|s, tx| took = on_ime(s, &ime, tx));
    assert!(took, "the terminal takes {ime:?}");
    rt.frame(1.0 / 60.0);
}

/// Text reaches the program through macOS's input method, which is what composes accents:
/// keys typed, and what it composed once it's done. (v2 also typed what was being composed
/// when macOS asked to keep it as it stood, `unmarkText`; winit drops that, so Lemur can't.)
#[test]
fn text_comes_through_the_input_method() {
    let server = HcServer::new("pime");
    let (mut rt, k, t) = shown_in(&server, "/", &["cat"]);
    type_text(&mut rt, "hi");
    // A dead key's accent shows, unsent, until the next key completes it.
    ime(&mut rt, Ime::Preedit("´".into(), Some((0, 2))));
    assert_eq!(rt.state.terminals[&k].marked.as_deref(), Some("´"));
    assert!(rt.find("marked-main").is_some(), "shown at the cursor");
    ime(&mut rt, Ime::Commit("é".into()));
    assert_eq!(rt.state.terminals[&k].marked, None);
    type_text(&mut rt, "!");
    assert!(reads(&mut rt, &server, &t, "hié!"), "{}", read(&server, &t));
}

/// The input method stays on for the terminal while its program hides the cursor, as
/// Claude Code does to draw its own: v2 took its text whatever the cursor did.
#[test]
fn the_input_method_stays_on_while_the_cursor_is_hidden() {
    let server = HcServer::new("pimehidden");
    let (mut rt, k, _) = shown_in(&server, "/", &["sh", "-c", "printf 'ready'; exec cat"]);
    assert!(shows(&mut rt, &k, "ready"));
    frames(&mut rt, 4);
    assert!(rt.ime_area.is_some(), "on while the cursor shows");
    rt.update(|s, _| s.terminals.get_mut(&k).unwrap().feed(b"\x1b[?25l"));
    frames(&mut rt, 4);
    assert_eq!(rt.state.terminals[&k].grid.as_ref().unwrap().frame().cursor, None, "hidden");
    assert!(rt.ime_area.is_some(), "the input method is off");
}

/// A held key repeats, Caps Lock's capital as macOS types it, Space and Backspace too.
#[test]
fn a_held_key_repeats_with_caps_lock() {
    let server = HcServer::new("pheld");
    let (mut rt, _, addr) = shown_in(&server, "/", &["cat"]);
    let held = |rt: &mut Runtime<Lemur>, key: K, repeats: usize| {
        rt.key(KeyEvent { key: key.clone(), mods: NONE });
        for _ in 0..repeats {
            rt.repeat_key(KeyEvent { key: key.clone(), mods: NONE });
        }
        rt.frame(1.0 / 60.0);
    };
    held(&mut rt, t("A"), 2);
    held(&mut rt, t(" "), 1);
    held(&mut rt, t("B"), 0);
    assert!(reads(&mut rt, &server, &addr, "AAA  B"), "{:?}", read(&server, &addr));
    held(&mut rt, K::Backspace, 2);
    held(&mut rt, t("C"), 0);
    assert!(reads(&mut rt, &server, &addr, "AAAC"), "{:?}", read(&server, &addr));
}

/// A right click on a file's path offers it to preview, open or download, even in a program
/// that takes the mouse, which never hears of that click. Download asks where, puts it there
/// and says so; a file that isn't there says that. A web link offers nothing, so the program
/// gets the click: the first it hears of.
#[test]
fn a_right_click_on_a_path_offers_to_download_it() {
    let server = HcServer::new("pfilemenu");
    let dir = TempDir::new("pfilemenu");
    std::fs::create_dir(dir.join("docs")).unwrap();
    std::fs::write(dir.join("docs/notes.txt"), "what it says\n").unwrap();
    let kept = TempDir::new("pfilemenu-kept");
    // Two paths and a web link, then, in hex, the one report it reads.
    let script = r"printf 'docs/notes.txt\ndocs/gone.txt\nhttps://example.com/a\n\033[?1000h'; stty raw -echo; head -c 6 | xxd -p -c 64";
    let folder = dir.to_string_lossy().into_owned();
    let (mut rt, k, t) = shown_in(&server, &folder, &["sh", "-c", script]);
    assert!(tracking(&mut rt, &k), "the program never asked for mouse reports");
    let right_click = |rt: &mut Runtime<Lemur>, row: f32| {
        let (x, y) = cell_point(rt, 3.5, row);
        rt.secondary_down(x, y);
        rt.secondary_up(x, y);
        frames(rt, 2);
    };

    right_click(&mut rt, 0.5);
    let menu = rt.popup_menu().expect("it offers");
    assert!(["preview", "zed", "download"].iter().all(|id| menu.find(id).is_some()));
    rt.resolve_popup(Some("download")).unwrap();
    frames(&mut rt, 2);
    let (_, options) = rt.file_dialog().expect("it asks where");
    assert_eq!(options.file_name.as_deref(), Some("notes.txt"));
    let to = kept.join("notes.txt");
    rt.resolve_file_dialog(Some(to.clone())).unwrap();
    let said = |s: &Lemur| s.status.as_ref().map(|(text, _)| text.clone()).unwrap_or_default();
    assert!(wait_for(&mut rt, 10.0, |s| said(s).starts_with("downloaded notes.txt to ")), "{:?}", said(&rt.state));
    assert_eq!(std::fs::read_to_string(&to).unwrap(), "what it says\n");

    right_click(&mut rt, 1.5);
    rt.resolve_popup(Some("download")).unwrap();
    frames(&mut rt, 2);
    rt.resolve_file_dialog(Some(kept.join("gone.txt"))).unwrap();
    assert!(wait_for(&mut rt, 10.0, |s| said(s).starts_with("download failed")), "{:?}", said(&rt.state));

    right_click(&mut rt, 2.5);
    assert!(rt.popup_menu().is_none(), "nothing offered");
    // ESC [ M, button 3 pressed, column 4, row 3.
    assert!(reads(&mut rt, &server, &t, "1b5b4d222423"), "it read {:?}", read(&server, &t));
    let fetched = std::env::temp_dir().join("lemur-files/local");
    let _ = std::fs::remove_dir_all(fetched.join(dir.strip_prefix("/").unwrap()));
}

/// Presses, moves and lets go in a program that asked for mouse `mode`; the first `count`
/// bytes it gets are to be `reports`.
fn press_and_move(tag: &str, mode: u32, count: u32, reports: &str, steps: &[f32]) {
    let server = HcServer::new(tag);
    let script = format!(r"printf '\033[?{mode}h'; stty raw -echo; head -c {count} | xxd -p -c 64");
    let (mut rt, k, t) = shown_in(&server, "/", &["sh", "-c", &script]);
    assert!(tracking(&mut rt, &k), "the program never asked for mouse reports");
    let (x, y) = cell_point(&rt, steps[0], 2.5);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.frame(1.0 / 60.0);
    for &col in &steps[1..] {
        let (x, _) = cell_point(&rt, col, 2.5);
        rt.pointer_move(x, y);
        rt.frame(1.0 / 60.0);
    }
    let (x, _) = cell_point(&rt, *steps.last().unwrap(), 2.5);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert!(reads(&mut rt, &server, &t, reports), "{tag} read {:?}", read(&server, &t));
}

/// A press that moves is reported to a program that asked for moves (`?1002`), and not to
/// one that asked only for clicks (`?1000`).
#[test]
fn a_program_asking_only_for_clicks_gets_no_moves() {
    press_and_move("pdrag1000", 1000, 12, "1b5b4d2023231b5b4d232623", &[2.5, 5.5]);
}

#[test]
fn a_program_asking_for_moves_gets_a_presss() {
    press_and_move("pdrag1002", 1002, 18, "1b5b4d2023231b5b4d4026231b5b4d232623", &[2.5, 5.5]);
}

/// A program that asks for the mouse without SGR (1000 alone, as GNU screen or an older
/// curses app) gets the normal reports: a left click, then a right one, then the wheel,
/// which only goes down. (v2's test clicked the middle button too: Rune hears none.)
#[test]
fn clicks_reach_a_program_without_sgr() {
    let server = HcServer::new("px10");
    let script = r"printf '\033[?1000h'; stty raw -echo; head -c 30 | xxd -p -c 64";
    let (mut rt, k, t) = shown_in(&server, "/", &["sh", "-c", script]);
    assert!(tracking(&mut rt, &k), "the program never asked for mouse reports");
    let at = cell_point(&rt, 2.5, 2.5);
    click_at(&mut rt, at);
    rt.secondary_down(at.0, at.1);
    rt.secondary_up(at.0, at.1);
    frames(&mut rt, 2);
    rt.wheel_xy(at.0, at.1, 0.0, rt.state.metrics.cell_h);
    frames(&mut rt, 2);
    // ESC [ M, then 32 + button (3: up, 64: wheel up), column and row from 1.
    let reports = "1b5b4d2023231b5b4d2323231b5b4d2223231b5b4d2323231b5b4d602323";
    assert!(reads(&mut rt, &server, &t, reports), "it read {:?}", read(&server, &t));
}

/// A terminal keeps as many lines to scroll back through as Settings say; fewer only once
/// it attaches again, so typing a number there doesn't drop its history on the way.
#[test]
fn the_terminal_keeps_the_scrollback_set() {
    let server = HcServer::new("pscroll");
    let t = server.run("/", &[], &["sh", "-c", "seq 1 6000; exec sleep 600"]);
    let mut rt = loaded(&server);
    rt.state.prefs.terminal.scrollback = 2000;
    open(&mut rt, &t);
    let k = key(&t);
    let history = |s: &Lemur| s.terminals[&k].grid.as_ref().map_or(0, |g| g.scrollback_rows());
    assert!(wait_for(&mut rt, 10.0, |s| s.terminals[&k].painted && history(s) >= 2000), "kept {}", history(&rt.state));
    let full = history(&rt.state);
    assert!(full <= 3000, "kept {full}");
    // Lowered in Settings, it keeps what it has.
    rt.update(|s, _| s.open_settings());
    super::commands::tests::click(&mut rt, "settings-page-terminal");
    super::commands::tests::click(&mut rt, "settings-scrollback-less");
    assert_eq!(rt.state.prefs.terminal.scrollback, 1000);
    frames(&mut rt, 4);
    assert_eq!(history(&rt.state), full);
    // Attached again, it keeps as few.
    rt.update(|s, _| s.connect(&k));
    assert!(wait_for(&mut rt, 10.0, |s| s.terminals[&k].painted && history(s) < 2000), "kept {}", history(&rt.state));
    assert!(history(&rt.state) >= 1000, "kept {}", history(&rt.state));
}

/// Attaching again, the stream starts on its own thread before the attach says it's done: what
/// comes meanwhile is the new screen's. On the old one, it was lost, and the screen came back
/// blank or in part.
#[test]
fn output_before_an_attach_is_done_goes_to_the_new_screen() {
    let server = HcServer::new("pbird");
    let (mut rt, k, _) = shown_in(&server, "/", &["sleep", "600"]);
    // No frame between them, so nothing has said the attach is done.
    rt.update(|s, _| {
        s.connect(&k);
        s.terminals.get_mut(&k).unwrap().feed(b"early bird");
        s.terminals.get_mut(&k).unwrap().grid.as_mut().unwrap().refresh().unwrap();
    });
    assert!(!lines(&rt, &k).iter().any(|l| l.contains("early bird")), "not the screen on show's: {:?}", lines(&rt, &k));
    assert!(wait_for(&mut rt, 10.0, |s| s.terminals[&k].painted), "attached again");
}

/// A font size from Settings reaches open terminals, and their programs: a bigger font fits
/// fewer columns and lines in the same window.
#[test]
fn the_terminal_follows_the_font_size() {
    let server = HcServer::new("pfont");
    let (mut rt, k, t) = shown_in(&server, "/", &["sleep", "600"]);
    frames(&mut rt, 4);
    let small = rt.state.terminals[&k].size;
    rt.update(|s, _| s.open_settings());
    super::commands::tests::click(&mut rt, "settings-page-terminal");
    for _ in 0..10 {
        super::commands::tests::click(&mut rt, "settings-font-size-more");
    }
    frames(&mut rt, 4);
    let large = rt.state.terminals[&k].size;
    assert!(large.cols < small.cols && large.lines < small.lines, "{small:?} -> {large:?}");
    let core = server.core();
    let seen = || core.terminals().into_iter().find(|x| x.address == t).map(|x| (x.cols, x.rows));
    assert!(
        crate::test_support::wait_until(Duration::from_secs(5), || seen() == Some((large.cols, large.lines))),
        "the core sees {:?} for {large:?}",
        seen()
    );
}

