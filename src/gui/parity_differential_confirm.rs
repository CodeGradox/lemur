//! What the v2/v3 differential check found v3 doing other than v2, each as a test of its
//! own: the same stream or key fed to v3's terminal, held to what v2 did.

use super::grid::{Grid, Line, Shape, Size};
use super::palette::DARK;
use rune::{Color, Key, KeyEvent, Mods};

fn grid(cols: u16, lines: u16) -> Grid {
    Grid::new(Size { cols, lines, cell_width: 8, cell_height: 16 }, 100, &DARK).unwrap()
}

fn key(text: &str, shift: bool, alt: bool, ctrl: bool) -> KeyEvent {
    KeyEvent { key: Key::Text(text.into()), mods: Mods { shift, alt, ctrl, ..Default::default() } }
}

fn srgb(c: Color) -> (u8, u8, u8) {
    let b = |v: f32| {
        let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (s * 255.0).round() as u8
    };
    (b(c.r), b(c.g), b(c.b))
}

/// What Option types on the layout reaches a program that turned on Kitty's keyboard
/// protocol, as fish 4 (`CSI = 5 u`) and Claude Code (`CSI > 1 u`) do once the terminal
/// answers `CSI ? u`, which v3 does and v2 didn't: ⌥7 on a Norwegian layout is "|".
#[test]
fn option_characters_reach_a_kitty_protocol_program() {
    for push in [&b"\x1b[=5u"[..], b"\x1b[>1u"] {
        let mut g = grid(80, 24);
        g.write(push);
        for (text, base) in [("|", '7'), ("[", '8'), ("@", '2'), ("å", 'a')] {
            let sent = g.key(&key(text, false, true, false), Some(base)).unwrap();
            assert_eq!(String::from_utf8_lossy(&sent), text, "⌥{base} after {push:?}");
        }
        let sent = g.key(&key("\\", true, true, false), Some('7')).unwrap();
        assert_eq!(sent, b"\\", "⇧⌥7");
    }
}

/// Control chords v2 sent as their C0 byte, as xterm does: ⌃[ is Escape to a vim user,
/// ⌃- and ⌃⇧- are undo (^_) in readline, zsh and fish.
#[test]
fn control_chords_send_their_control_byte() {
    let mut g = grid(80, 24);
    let cases: [(&str, char, bool, &[u8]); 9] = [
        ("[", '[', false, b"\x1b"),
        ("i", 'i', false, b"\t"),
        ("m", 'm', false, b"\r"),
        ("-", '-', false, b"\x1f"),
        ("_", '-', true, b"\x1f"),
        ("@", '2', true, b"\x00"),
        ("^", '6', true, b"\x1e"),
        ("?", '/', true, b"\x7f"),
        ("C", 'c', true, b"\x03"),
    ];
    let mut wrong = vec![];
    for (text, base, shift, want) in cases {
        let sent = g.key(&key(text, shift, false, true), Some(base)).unwrap_or_default();
        if sent != want {
            wrong.push(format!("⌃{}{base}: {sent:?}, not {want:?}", if shift { "⇧" } else { "" }));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Enter, Escape and Tab with a modifier send what v2 sent, not Ghostty's `CSI 27;m;k~`,
/// which a shell types into the line: ⌃Enter runs it.
#[test]
fn modified_enter_escape_and_tab_send_what_shells_read() {
    let mut g = grid(80, 24);
    let mods = |shift, alt, ctrl| Mods { shift, alt, ctrl, ..Default::default() };
    let cases: [(Key, Mods, &[u8]); 7] = [
        (Key::Enter, mods(false, false, true), b"\r"),
        (Key::Enter, mods(true, false, false), b"\x1b\r"),
        (Key::Enter, mods(false, true, false), b"\x1b\r"),
        (Key::Escape, mods(true, false, false), b"\x1b"),
        (Key::Escape, mods(false, false, true), b"\x1b"),
        (Key::Tab, mods(true, false, false), b"\x1b[Z"),
        (Key::Tab, mods(false, false, true), b"\t"),
    ];
    for (k, m, want) in cases {
        let sent = g.key(&KeyEvent { key: k.clone(), mods: m }, None).unwrap_or_default();
        assert_eq!(sent, want, "{k:?} with {m:?}");
    }
}

/// A program setting the terminal's colours (OSC 10, 11) shows it at once, as in v2, and
/// the text already there follows.
#[test]
fn a_programs_default_colours_show_at_once() {
    let mut g = grid(20, 3);
    g.write(b"plain");
    g.refresh().unwrap();
    g.write(b"\x1b]11;#102030\x1b\\\x1b]10;#aabbcc\x1b\\");
    g.refresh().unwrap();
    assert_eq!(srgb(g.frame().bg), (0x10, 0x20, 0x30), "background");
    assert_eq!(srgb(g.frame().rows[0].runs[0].fg), (0xaa, 0xbb, 0xcc), "text in the default colour");
}

/// Cells a program skipped (cursor moved over, a tab) between two underlined words aren't
/// underlined: v2 drew each cell's own underline.
#[test]
fn skipped_cells_between_underlined_words_arent_underlined() {
    let mut g = grid(20, 3);
    g.write(b"\x1b[4ma\x1b[0m\x1b[3C\x1b[4mb\x1b[0m\r\n\x1b[9mx\ty\x1b[0m");
    g.refresh().unwrap();
    let f = g.frame();
    for (y, row) in f.rows.iter().take(2).enumerate() {
        for r in row.runs.iter().filter(|r| r.underline != Line::None || r.strike) {
            assert!(!r.text.contains(' '), "row {y}: {r:?} draws its line over cells it skipped");
        }
    }
}

/// Dim text goes a third of the way to the background, as in v2, not half.
#[test]
fn dim_text_is_a_third_toward_the_background() {
    let mut g = grid(20, 3);
    g.write(b"\x1b[2;38;2;240;240;240mdim");
    g.refresh().unwrap();
    let f = g.frame();
    let (bg, fg) = (srgb(f.bg), srgb(f.rows[0].runs[0].fg));
    let third = |c: u8, b: u8| ((u16::from(c) * 2 + u16::from(b)) / 3) as u8;
    let want = (third(240, bg.0), third(240, bg.1), third(240, bg.2));
    let close = |a: u8, b: u8| a.abs_diff(b) <= 1;
    assert!(close(fg.0, want.0) && close(fg.1, want.1) && close(fg.2, want.2), "{fg:?}, not {want:?}");
}

/// The cursor takes the shape its program asks for (DECSCUSR): vim's bar in insert mode.
#[test]
fn the_cursor_takes_its_programs_shape() {
    let mut g = grid(20, 3);
    for (seq, want) in [(&b"\x1b[6 q"[..], Shape::Bar), (b"\x1b[4 q", Shape::Underline), (b"\x1b[2 q", Shape::Block), (b"\x1b[5 q", Shape::Bar), (b"\x1b[0 q", Shape::Block)] {
        g.write(seq);
        g.refresh().unwrap();
        assert_eq!(g.frame().shape, want, "after {seq:?}");
    }
}

/// Underlines keep their kind: curly (as editors mark errors with) and double.
#[test]
fn underlines_keep_their_kind() {
    let mut g = grid(20, 3);
    g.write(b"\x1b[4:3mwavy\x1b[0m \x1b[21mtwo\x1b[0m \x1b[4mone");
    g.refresh().unwrap();
    let kinds: Vec<(String, Line)> = g.frame().rows[0].runs.iter().filter(|r| r.underline != Line::None).map(|r| (r.text.clone(), r.underline)).collect();
    assert_eq!(kinds, [("wavy".into(), Line::Curly), ("two".into(), Line::Double), ("one".into(), Line::Single)]);
}
