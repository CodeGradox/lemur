//! Finding text in a terminal (⌘F): what Ghostty's search finds and where it shows, and the
//! find bar that drives it.

use super::core::wait_for;
use super::grid::{Grid, Size};
use super::palette::DARK;
use super::tests::{cmd, key, open, press, type_text, window};
use super::{Key, Lemur};
use crate::test_support::HcServer;
use rune::{Key as K, Mods, Runtime};
use std::time::Duration;

fn grid(cols: u16, lines: u16) -> Grid {
    Grid::new(Size { cols, lines, cell_width: 16, cell_height: 32 }, 1000, &DARK).unwrap()
}

/// Looks until the search is done.
fn settle(g: &mut Grid) {
    for _ in 0..100 {
        g.find_more(Duration::from_millis(5));
        if !g.found().searching {
            break;
        }
    }
    g.refresh().unwrap();
}

/// Each match on show covers its cells, ASCII letters in either case; picking goes up from
/// the newest, scrolling to one off the screen, and wraps.
#[test]
fn a_search_finds_matches_through_the_scrollback() {
    let mut g = grid(20, 5);
    for i in 1..=100 {
        g.write(format!("line {i}\r\n").as_bytes());
    }
    g.write(b"Needle here\r\nand a needle");
    g.refresh().unwrap();
    g.find("NEEDLE").unwrap();
    settle(&mut g);
    let found = g.found().clone();
    assert_eq!(found.total, 2);
    assert_eq!(found.picked, None, "none picked till asked");
    // The screen's last rows: "line 100", "Needle here", "and a needle".
    assert_eq!(found.spans.len(), 2, "{found:?}");
    assert!(found.spans.contains(&(3, 0, 6, false)), "{found:?}");
    assert!(found.spans.contains(&(4, 6, 12, false)), "{found:?}");

    assert!(g.find_next(true));
    g.refresh().unwrap();
    assert_eq!(g.found().picked, Some(0), "the newest first");
    assert!(g.found().spans.contains(&(4, 6, 12, true)));

    // Through the scrollback.
    g.find("line 5").unwrap();
    settle(&mut g);
    assert_eq!(g.found().total, 11, "5 and 50 to 59");
    assert!(g.find_next(true));
    g.refresh().unwrap();
    let (_, offset, len) = g.scrollbar().unwrap();
    let picked: Vec<_> = g.found().spans.iter().filter(|s| s.3).collect();
    assert_eq!(picked.len(), 1, "the one picked shows: {:?}", g.found());
    assert!(g.lines()[usize::from(picked[0].0)].starts_with("line 59"), "{:?} at {offset}+{len}", g.lines());
    for _ in 0..10 {
        g.find_next(true);
    }
    g.refresh().unwrap();
    assert_eq!(g.found().picked, Some(10));
    let picked = g.found().spans.iter().find(|s| s.3).copied().expect("on show");
    assert_eq!(g.lines()[usize::from(picked.0)], "line 5");
    g.find_next(true);
    g.refresh().unwrap();
    assert_eq!(g.found().picked, Some(0), "round again");
    g.find_next(false);
    g.refresh().unwrap();
    assert_eq!(g.found().picked, Some(10), "and back down");

    g.find("").unwrap();
    assert_eq!(*g.found(), Default::default(), "nothing looked for");
}

/// A match the terminal wrapped onto the next row covers the end of one and the start of the
/// next.
#[test]
fn a_match_wrapped_onto_the_next_row_shows_on_both() {
    let mut g = grid(10, 3);
    g.write(b"0123456needle");
    g.refresh().unwrap();
    g.find("needle").unwrap();
    settle(&mut g);
    assert_eq!(g.found().total, 1);
    assert_eq!(g.found().spans, vec![(0, 7, 10, false), (1, 0, 3, false)]);
}

fn frames(rt: &mut Runtime<Lemur>, n: usize) {
    for _ in 0..n {
        rt.frame(1.0 / 60.0);
    }
}

/// A shell that printed 1 to 500, its 500 shown at the bottom.
fn counted(tag: &str) -> (HcServer, Runtime<Lemur>, Key) {
    let server = HcServer::new(tag);
    let t = server.run("/", &[], &["sh", "-c", "seq 1 500; exec sleep 600"]);
    let mut rt = window(server.config());
    rt.state.refresh();
    assert!(wait_for(&mut rt, 10.0, |s| s.loaded), "the first snapshot");
    open(&mut rt, &t);
    let k = key(&t);
    assert!(
        wait_for(&mut rt, 10.0, |s| s.terminals.get(&k).and_then(|v| v.grid.as_ref()?.scrollbar()).is_some_and(|(total, ..)| total > 400)),
        "500 lines"
    );
    frames(&mut rt, 10);
    (server, rt, k)
}

fn shows(rt: &Runtime<Lemur>, k: &Key) -> Vec<String> {
    super::terminal::lines(&rt.state, k)
}

fn count(rt: &Runtime<Lemur>) -> String {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some("find-count") && !e.exiting).and_then(|e| e.text).unwrap_or_default()
}

/// The row the match picked shows on, and the text there.
fn picked(rt: &Runtime<Lemur>, k: &Key) -> Option<String> {
    let g = rt.state.terminals[k].grid.as_ref()?;
    let &(row, ..) = g.found().spans.iter().find(|s| s.3)?;
    shows(rt, k).get(usize::from(row)).cloned()
}

/// ⌘F opens the bar with the keyboard; typing looks as it goes, picking the newest match and
/// scrolling back to it, marked on show. Enter goes up to older ones, ⇧Enter and ⇧⌘G back
/// down; Esc closes the bar, the marks go, and the keyboard is the terminal's again.
#[test]
fn cmd_f_finds_through_the_scrollback() {
    let (_server, mut rt, k) = counted("tfind");
    press(&mut rt, K::Command('f'), cmd());
    frames(&mut rt, 10);
    assert!(rt.find("find-bar").is_some(), "the bar");
    assert!(rt.state.find_editing() && !rt.state.keyboard_on_terminal(), "the keyboard's the bar's");

    type_text(&mut rt, "25");
    frames(&mut rt, 10);
    // 25, 125, 225, 250 to 259, 325 and 425.
    assert_eq!(count(&rt), "1 of 15");
    assert_eq!(picked(&rt, &k).as_deref(), Some("425"), "the newest, scrolled back to: {:?}", shows(&rt, &k));
    assert!(rt.find("found-main-0").is_some(), "marked");
    assert_eq!(rt.state.terminals[&k].find.text, "25", "typed in the bar, not the terminal");

    press(&mut rt, K::Enter, Mods::default());
    frames(&mut rt, 4);
    assert_eq!(count(&rt), "2 of 15");
    assert_eq!(picked(&rt, &k).as_deref(), Some("325"));
    press(&mut rt, K::Enter, Mods::default());
    frames(&mut rt, 4);
    assert_eq!(picked(&rt, &k).as_deref(), Some("259"));
    press(&mut rt, K::Enter, Mods { shift: true, ..Mods::default() });
    frames(&mut rt, 4);
    assert_eq!(picked(&rt, &k).as_deref(), Some("325"), "back down");
    press(&mut rt, K::Command('g'), cmd());
    frames(&mut rt, 4);
    assert_eq!(picked(&rt, &k).as_deref(), Some("259"), "⌘G goes up too");
    press(&mut rt, K::Command('g'), Mods { shift: true, ..cmd() });
    frames(&mut rt, 4);
    assert_eq!(count(&rt), "2 of 15", "⇧⌘G down");

    type_text(&mut rt, "x");
    frames(&mut rt, 4);
    assert_eq!(count(&rt), "No matches");
    assert!(rt.find("found-main-0").is_none());

    press(&mut rt, K::Escape, Mods::default());
    frames(&mut rt, 20);
    assert!(rt.find("find-bar").is_none(), "closed");
    assert!(rt.state.keyboard_on_terminal(), "the keyboard's the terminal's again");
    assert!(rt.state.terminals[&k].grid.as_ref().is_some_and(|g| !g.looking()), "not looking any more");

    // Open again, it has what it looked for, all of it selected to type over.
    press(&mut rt, K::Command('f'), cmd());
    frames(&mut rt, 10);
    let find = &rt.state.terminals[&k].find;
    assert_eq!((find.text.as_str(), find.anchor, find.focus), ("25x", 0, 3));
    type_text(&mut rt, "499");
    frames(&mut rt, 10);
    assert_eq!(count(&rt), "1 of 1");
}

/// A press on the bar between its controls doesn't start a selection in the terminal under it,
/// and its buttons step and close it.
#[test]
fn the_find_bars_buttons_step_and_close_it() {
    let (_server, mut rt, k) = counted("tfindb");
    press(&mut rt, K::Command('f'), cmd());
    type_text(&mut rt, "25");
    frames(&mut rt, 20);
    assert_eq!(count(&rt), "1 of 15");
    let click = |rt: &mut Runtime<Lemur>, key: &str| {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no {key}"));
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        frames(rt, 4);
    };
    click(&mut rt, "find-up");
    assert_eq!(count(&rt), "2 of 15");
    click(&mut rt, "find-down");
    assert_eq!(count(&rt), "1 of 15");

    let bar = rt.presented("find-bar").expect("the bar").0;
    let (x, y) = (bar[0] + bar[2] - 2.0, bar[1] + bar[3] / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_move(x - 200.0, y + 100.0);
    frames(&mut rt, 2);
    rt.pointer_up(x - 200.0, y + 100.0);
    frames(&mut rt, 2);
    assert!(rt.state.terminals[&k].grid.as_ref().unwrap().selected_text().is_none(), "nothing selected");

    click(&mut rt, "find-close");
    frames(&mut rt, 20);
    assert!(rt.find("find-bar").is_none(), "closed");
    assert!(rt.find("found-main-0").is_none(), "unmarked");
}
