//! The terminal's scroll bar: where its thumb sits, and what dragging it and pressing its track
//! do.

use super::core::wait_for;
use super::term_bar::thumb;
use super::tests::{key, open, window};
use super::{Key, Lemur};
use crate::test_support::HcServer;
use rune::Runtime;

/// The thumb is the share of the rows that show, 24 points at least, placed by how far back
/// the view is.
#[test]
fn the_thumb_is_the_share_of_rows_that_show() {
    assert_eq!(thumb(400, 300, 100, 200.0), (150.0, 50.0), "at the bottom");
    assert_eq!(thumb(400, 0, 100, 200.0), (0.0, 50.0), "at the top");
    assert_eq!(thumb(10_000, 0, 50, 200.0), (0.0, 24.0), "never shorter than 24 points");
    assert_eq!(thumb(10_000, 9_950, 50, 200.0), (176.0, 24.0));
    assert_eq!(thumb(40, 0, 40, 200.0), (0.0, 200.0), "no scrollback: the whole track");
}

/// Where `k`'s view is: all the rows, the first one shown, and how many show.
fn at(rt: &Runtime<Lemur>, k: &Key) -> (usize, usize, usize) {
    rt.state.terminals[k].grid.as_ref().and_then(|g| g.scrollbar()).expect("a scroll bar")
}

fn frames(rt: &mut Runtime<Lemur>, n: usize) {
    for _ in 0..n {
        rt.frame(1.0 / 60.0);
    }
}

/// A shell with scrollback has a scroll bar, unseen at the bottom till the pointer comes to the
/// edge (headless, bars overlay). Dragging its thumb up goes back through the scrollback,
/// and a press on the track below it pages forward a screen; the alternate screen has none.
#[test]
fn the_scroll_bar_drags_and_pages_through_the_scrollback() {
    let server = HcServer::new("tbar");
    let t = server.run("/", &[], &["sh", "-c", "seq 1 500; exec sleep 600"]);
    let mut rt = window(server.config());
    rt.state.refresh();
    assert!(wait_for(&mut rt, 10.0, |s| s.loaded), "the first snapshot");
    open(&mut rt, &t);
    let k = key(&t);
    assert!(wait_for(&mut rt, 10.0, |s| s.terminals.get(&k).and_then(|v| v.grid.as_ref()?.scrollbar()).is_some_and(|(total, ..)| total > 400)), "500 lines");
    frames(&mut rt, 10);
    let (total, offset, len) = at(&rt, &k);
    assert_eq!(offset + len, total, "at the bottom");
    let bar = rt.presented("term-bar").expect("a scroll bar");
    assert_eq!(rt.presented("term-bar-thumb").map(|t| t.1), Some(0.0), "unseen at the bottom");

    // The thumb, dragged up half the track, shows the rows half way back.
    let thumb = rt.presented("term-bar-thumb").expect("its thumb").0;
    let (x, y) = (thumb[0] + thumb[2] - 4.0, thumb[1] + thumb[3] / 2.0);
    let track = bar.0[3];
    rt.pointer_move(x, y);
    frames(&mut rt, 30);
    assert!(rt.presented("term-bar-thumb").is_some_and(|t| t.1 > 0.99), "the pointer at the edge shows it");
    rt.pointer_down(x, y);
    for step in 1..=10 {
        rt.pointer_move(x, y - track / 2.0 * step as f32 / 10.0);
        rt.frame(1.0 / 60.0);
    }
    rt.pointer_up(x, y - track / 2.0);
    frames(&mut rt, 30);
    let (_, back, _) = at(&rt, &k);
    let half = (total - len) / 2;
    assert!(back.abs_diff(half) <= (total - len) / 20, "{back} rows down, about {half}");
    assert!(rt.presented("term-bar-thumb").is_some_and(|t| t.1 > 0.99), "seen while scrolled back");

    // A press on the track under the thumb pages a screen on.
    let thumb = rt.presented("term-bar-thumb").expect("its thumb").0;
    let below = (x, thumb[1] + thumb[3] + 10.0);
    rt.pointer_move(below.0, below.1);
    rt.pointer_down(below.0, below.1);
    rt.pointer_up(below.0, below.1);
    frames(&mut rt, 4);
    assert_eq!(at(&rt, &k).1, back + len, "a screen on");

    // A program on the alternate screen keeps no scrollback.
    rt.update(|s, _| s.terminals.get_mut(&k).unwrap().feed(b"\x1b[?1049h"));
    std::thread::sleep(super::terminal::OUTPUT_EVERY);
    frames(&mut rt, 4);
    assert!(rt.find("term-bar").is_none(), "none on the alternate screen");
}
