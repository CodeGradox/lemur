//! v2's window tests (its `gui/mod.rs`'s first half) that v3 had no test of the same name
//! for, ported to Rune's `Runtime`: the sidebar, threads, tabs, the spotlight, closing,
//! reordering, snapshots. Each says which v2 test it stands for.

use super::core::wait_for;
use super::prompt::Ask;
use super::snackbar::Kind;
use super::tests::{apply, cmd, key, made_up, open, press, refreshing, type_text, window};
use super::{Action, Lemur, List, Right, actions, move_item, moved_to};
use crate::config::{Agent, Base, Config, Host};
use crate::snapshot::{self, Activity};
use crate::test_support::{HcServer, TempDir, git_in, wait_until};
use rune::{ElementInfo, Key as K, KeyEvent, Mods, PhysicalInput, Runtime, TitlebarAction};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn frames(rt: &mut Runtime<Lemur>, n: usize) {
    for _ in 0..n {
        rt.frame(1.0 / 60.0);
    }
}

/// Runs the window for `secs`, as time passing does.
fn settle(rt: &mut Runtime<Lemur>, secs: f64) {
    let _ = wait_for(rt, secs, |_| false);
}

/// The window on `cfg`, once its first snapshot is in.
fn loaded_with(cfg: Config) -> Runtime<Lemur> {
    let mut rt = window(cfg);
    rt.state.refresh();
    assert!(wait_for(&mut rt, 10.0, |s| s.loaded), "the first snapshot");
    frames(&mut rt, 20);
    rt
}

fn loaded(server: &HcServer) -> Runtime<Lemur> {
    loaded_with(server.config())
}

/// The element keyed `key`, or whose path ends with `key`, as of the last frame.
fn info(rt: &Runtime<Lemur>, key: &str) -> Option<ElementInfo> {
    let suffix = format!("/{key}");
    rt.elements().into_iter().find(|e| !e.exiting && (e.key.as_deref() == Some(key) || e.path.ends_with(&suffix)))
}

/// The centre of the element keyed `key` inside the one keyed `within`.
fn find_in(rt: &Runtime<Lemur>, within: &str, key: &str) -> Option<(f32, f32)> {
    let inside = format!("/{within}/");
    let e = rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key) && e.path.contains(&inside))?;
    Some((e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0))
}

fn shows(rt: &Runtime<Lemur>, key: &str) -> bool {
    rt.find(key).is_some()
}

fn click(rt: &mut Runtime<Lemur>, key: &str) {
    frames(rt, 20);
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element keyed {key}"));
    rt.pointer_move(x, y);
    frames(rt, 2);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    frames(rt, 4);
}

/// Presses on `key`, drags it `(dx, dy)` points in steps, and lets go.
fn drag(rt: &mut Runtime<Lemur>, key: &str, dx: f32, dy: f32) {
    frames(rt, 20);
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element keyed {key}"));
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    for step in 1..=10 {
        let f = step as f32 / 10.0;
        rt.pointer_move(x + dx * f, y + dy * f);
        rt.frame(1.0 / 60.0);
    }
    rt.pointer_up(x + dx, y + dy);
    frames(rt, 4);
}

/// What herdr-core keeps of the order of the host's threads outside every project.
fn core_order(cfg: &Config) -> Vec<String> {
    snapshot::fetch(cfg).hosts[0].other.iter().map(|t| t.name.clone()).collect()
}

/// The order the sidebar shows them in.
fn other(s: &Lemur) -> Vec<String> {
    s.snap.hosts.first().map(|h| h.other.iter().map(|t| t.name.clone()).collect()).unwrap_or_default()
}

/// A repository `name` in `dir` on branch `branch`, with a commit, and a worktree for each
/// of `worktrees` at `wt/<name>` on a branch of that name.
fn repo(dir: &Path, name: &str, branch: &str, worktrees: &[&str]) -> PathBuf {
    let repo = dir.join(name);
    std::fs::create_dir_all(&repo).unwrap();
    git_in(&repo, &["init", "-q", "-b", branch]);
    git_in(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    for w in worktrees {
        git_in(&repo, &["worktree", "add", "-q", "-b", w, &format!("wt/{w}")]);
    }
    repo
}

fn path_of(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// `git worktree list` in `repo`.
fn listed(repo: &Path) -> String {
    let out = std::process::Command::new("git").args(["worktree", "list"]).current_dir(repo).output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// v2 `moved_to_follows_move_item`.
#[test]
fn moved_to_follows_move_item() {
    for from in 0..4 {
        for to in 0..4 {
            let mut items: Vec<usize> = (0..4).collect();
            move_item(&mut items, from, to);
            for (place, &item) in items.iter().enumerate() {
                assert_eq!(moved_to(item, from, to), place, "{from} -> {to}");
            }
        }
    }
}

/// v2 `worktrees_are_named_after_their_branch`.
#[test]
fn worktrees_are_named_after_their_branch() {
    let worktree = |branch: Option<&str>, head: Option<&str>| snapshot::Worktree {
        path: "/srv/web/.worktrees/equal-burrow/web".into(),
        branch: branch.map(str::to_string),
        head: head.map(str::to_string),
        main: false,
        sessions: vec![],
        gone: vec![],
    };
    let label = |b, h| super::worktree_label(&worktree(b, h));
    assert_eq!(label(Some("feature/cleaning-app-react"), Some("abc")), "feature/cleaning-app-react");
    assert_eq!(label(Some("worktree-auth"), None), "auth");
    assert_eq!(label(None, Some("0123456789abcdef")), "web (detached)");
    // A bare repository has neither.
    assert_eq!(label(None, None), "web");
}

/// v2 `about_says_the_version`: About Lemur says which version this is.
#[test]
fn about_says_the_version() {
    let mut rt = window(toml::from_str("").unwrap());
    rt.update(|s, _| s.about());
    let title = rt.state.dialog.as_ref().map(|d| d.title.clone());
    assert_eq!(title, Some(format!("Lemur {}", env!("CARGO_PKG_VERSION"))));
}

/// v2 `dragging_a_thread_reorders_its_list`: a thread's row dropped further down its list
/// moves there, in the sidebar at once, and in herdr-core for good; the refresh after
/// agrees.
#[test]
fn dragging_a_thread_reorders_its_list_for_good() {
    let server = HcServer::new("pa-drag");
    let [a, b, c] = ["a", "b", "c"].map(|_| server.run("/", &[], &["sleep", "600"]));
    let mut rt = loaded(&server);
    assert_eq!(other(&rt.state), [a.as_str(), &b, &c]);
    let (_, ya) = rt.find(&format!("row-local-{a}")).unwrap();
    let (_, yb) = rt.find(&format!("row-local-{b}")).unwrap();
    drag(&mut rt, &format!("row-local-{a}"), 0.0, 2.0 * (yb - ya));
    assert_eq!(other(&rt.state), [b.as_str(), &c, &a]);
    let cfg = server.config();
    assert!(wait_for(&mut rt, 15.0, |s| s.ordering.is_empty()), "written");
    assert_eq!(core_order(&cfg), [b.as_str(), &c, &a]);
    rt.update(|s, _| s.refresh());
    settle(&mut rt, 1.5);
    assert_eq!(other(&rt.state), [b.as_str(), &c, &a], "the refresh after the move agrees");
}

/// v2 `two_drags_in_a_row_keep_the_second`: two orders in a row are written one after the
/// other: herdr-core keeps the second's, not a mix of the two.
#[test]
fn two_drags_in_a_row_keep_the_second() {
    let server = HcServer::new("pa-drag2");
    let t: Vec<String> = (0..8).map(|_| server.run("/", &[], &["sleep", "600"])).collect();
    let mut rt = loaded(&server);
    let list: List = ("local".into(), None);
    rt.update(|s, _| {
        s.reorder(list.clone(), 0, 7);
        s.reorder(list.clone(), 0, 7);
    });
    let mut expected = t.clone();
    expected.rotate_left(2);
    assert!(wait_for(&mut rt, 20.0, |s| s.ordering.is_empty()), "written");
    assert_eq!(core_order(&server.config()), expected);
}

/// v2 `dragging_a_tab_reorders_its_threads`: a tab dropped on another takes its place, and
/// its thread in the sidebar with it, in herdr-core for good; it doesn't open another.
#[test]
fn dragging_a_tab_reorders_its_threads() {
    let server = HcServer::new("pa-dragtab");
    let [a, b, c] = ["a", "b", "c"].map(|l| server.run("/", &[&format!("lemur.label={l}")], &["sleep", "600"]));
    let mut rt = loaded(&server);
    open(&mut rt, &a);
    frames(&mut rt, 30);
    assert_eq!(other(&rt.state), [a.as_str(), &b, &c]);
    let (xa, _) = rt.find(&format!("t-{a}")).expect("a's tab");
    let (xc, _) = rt.find(&format!("t-{c}")).expect("c's tab");
    drag(&mut rt, &format!("t-{a}"), xc - xa, 0.0);
    assert_eq!(other(&rt.state), [b.as_str(), &c, &a]);
    assert_eq!(rt.state.active, Some(key(&a)), "dragging it doesn't open another");
    let cfg = server.config();
    assert!(wait_for(&mut rt, 15.0, |s| s.ordering.is_empty()), "written");
    assert_eq!(core_order(&cfg), [b.as_str(), &c, &a]);
}

/// v2 `keys_move_the_tab_on_show`: ⌘⇧← and ⌘⇧→ move the tab on show, stopping at the
/// ends; it stays on show; one put away isn't a tab to move past; herdr-core keeps it.
#[test]
fn keys_move_the_tab_on_show() {
    let server = HcServer::new("pa-movetab");
    let [a, b, c] = ["a", "b", "c"].map(|l| server.run("/", &[&format!("lemur.label={l}")], &["sleep", "600"]));
    let mut rt = loaded(&server);
    open(&mut rt, &b);
    assert_eq!(other(&rt.state), [a.as_str(), &b, &c]);
    let right = Mods { shift: true, ..cmd() };
    press(&mut rt, K::Right, right);
    assert_eq!(other(&rt.state), [a.as_str(), &c, &b]);
    press(&mut rt, K::Right, right);
    assert_eq!(other(&rt.state), [a.as_str(), &c, &b], "it's the last");
    press(&mut rt, K::Left, right);
    press(&mut rt, K::Left, right);
    assert_eq!(other(&rt.state), [b.as_str(), &a, &c]);
    press(&mut rt, K::Left, right);
    assert_eq!(other(&rt.state), [b.as_str(), &a, &c], "it's the first");
    assert_eq!(rt.state.active, Some(key(&b)), "it stays on show");

    // Past one put away, which isn't a tab.
    rt.update(|s, _| {
        s.snap.hosts[0].other[2].away = true;
        s.move_tab(false);
        s.move_tab(false);
    });
    assert_eq!(other(&rt.state), [a.as_str(), &b, &c], "only the tabs' end");
    let cfg = server.config();
    assert!(wait_for(&mut rt, 15.0, |s| s.ordering.is_empty()), "written");
    assert_eq!(core_order(&cfg), [a.as_str(), &b, &c]);
}

/// The first point along `key`'s middle row, left to right, where a press lands on `key`
/// itself rather than on something in it.
fn bare_point(rt: &Runtime<Lemur>, key: &str) -> (f32, f32) {
    let (r, _) = rt.presented(key).unwrap_or_else(|| panic!("no {key}"));
    let y = r[1] + r[3] / 2.0;
    let mut x = r[0] + 2.0;
    while x < r[0] + r[2] {
        if rt.element_at(x, y).is_some_and(|p| p.key() == Some(key)) {
            return (x, y);
        }
        x += 2.0;
    }
    panic!("nowhere bare on {key}");
}

/// Presses at (`x`, `y`) and moves a little, as starting a drag; what the press asked of
/// the window.
fn press_and_move(rt: &mut Runtime<Lemur>, x: f32, y: f32) -> Option<TitlebarAction> {
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    rt.pointer_down(x, y);
    let action = rt.take_titlebar_action();
    rt.pointer_move(x + 3.0, y);
    rt.frame(1.0 / 60.0);
    action
}

/// v2 `dragging_the_top_bar_moves_the_window`: the window has no title bar; the toolbar's
/// top row moves it instead, from where nothing else takes the press.
#[test]
fn dragging_the_top_bar_moves_the_window() {
    let mut rt = window(toml::from_str("").unwrap());
    frames(&mut rt, 30);
    let (x, y) = bare_point(&rt, "toolbar");
    assert_eq!(press_and_move(&mut rt, x, y), Some(TitlebarAction::Drag));
}

/// v2 `dragging_the_sidebars_top_moves_the_window`: the sidebar's top row too, where the
/// window's buttons are, its title with them.
#[test]
fn dragging_the_sidebars_top_moves_the_window() {
    let mut rt = window(toml::from_str("").unwrap());
    frames(&mut rt, 30);
    let (x, y) = bare_point(&rt, "sidebar-head");
    assert_eq!(press_and_move(&mut rt, x, y), Some(TitlebarAction::Drag));
    rt.pointer_up(x + 3.0, y);
    let (x, y) = rt.find("sidebar-title").expect("the title");
    assert_eq!(press_and_move(&mut rt, x, y), Some(TitlebarAction::Drag), "on the title");
}

/// v2 `a_top_bar_button_keeps_its_press`: a press on a top row's button is the button's:
/// moving a little before letting go still clicks it, and doesn't move the window.
#[test]
fn a_top_bar_button_keeps_its_press() {
    let mut rt = window(toml::from_str("").unwrap());
    frames(&mut rt, 30);
    let (x, y) = rt.find("sidebar-toggle").expect("the sidebar's toggle");
    assert_eq!(press_and_move(&mut rt, x, y), None, "not a move");
    rt.pointer_up(x + 3.0, y);
    frames(&mut rt, 4);
    assert!(!rt.state.sidebar.is_open(), "it toggled the sidebar");
}

/// v2 `the_sidebar_follows_the_core`: a terminal started elsewhere joins the sidebar as
/// herdr-core tells of it, well before the next refresh; one that ends leaves it.
#[test]
fn the_sidebar_follows_the_core() {
    let server = HcServer::new("pa-live");
    let mut rt = window(server.config());
    // Out of sight a refresh comes every AWAY ticks (15 s): what comes sooner is the watch.
    rt.update(|s, _| {
        s.looking = false;
        s.start();
    });
    let start = std::time::Instant::now();
    assert!(wait_for(&mut rt, 10.0, |s| s.loaded), "the first snapshot");
    let labels = |s: &Lemur| -> Vec<String> {
        s.snap.hosts.first().map(|h| h.other.iter().map(|t| t.label.clone()).collect()).unwrap_or_default()
    };
    assert!(labels(&rt.state).is_empty());
    let t = server.run("/", &["lemur.label=late"], &["sleep", "600"]);
    assert!(wait_for(&mut rt, 2.5, |s| labels(s) == ["late"]), "{:?}", labels(&rt.state));
    server.hc(&["label", &t, "lemur.label=renamed"]);
    assert!(wait_for(&mut rt, 2.5, |s| labels(s) == ["renamed"]), "{:?}", labels(&rt.state));
    server.hc(&["keep", &t, "--off"]);
    server.hc(&["kill", &t]);
    assert!(wait_for(&mut rt, 2.5, |s| labels(s).is_empty()), "{:?}", labels(&rt.state));
    assert!(start.elapsed() < Duration::from_secs(15), "before the next refresh");
}

/// Drags the grip keyed `grip` `dx` points across.
fn drag_grip(rt: &mut Runtime<Lemur>, grip: &str, dx: f32) {
    let (r, _) = rt.presented(grip).unwrap_or_else(|| panic!("no {grip}"));
    let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    rt.pointer_down(x, y);
    for step in 1..=10 {
        rt.pointer_move(x + dx * step as f32 / 10.0, y);
        rt.frame(1.0 / 60.0);
    }
    rt.pointer_up(x + dx, y);
    frames(rt, 10);
}

/// v2 `dragging_a_side_panels_edge_resizes_it`, for the widths and what the config keeps:
/// dragging a side panel's inner edge resizes it, down to its minimum; the config keeps
/// the widths, but not lemur's own. (v3 folds a panel dragged past half its minimum, as
/// AppKit's split views do, where v2 stopped it at the minimum: that's on purpose.)
#[test]
fn dragging_a_side_panels_edge_resizes_it() {
    let server = HcServer::new("pa-panels");
    let mut rt = window(server.config());
    rt.set_viewport((1000.0, 600.0), 2.0);
    frames(&mut rt, 30);
    let widths = |s: &Lemur| (s.sidebar.width(), s.inspector.width());
    let kept = |s: &Lemur| (s.cfg.sidebar_width, s.cfg.notes_width);

    let w = widths(&rt.state).0;
    drag_grip(&mut rt, "sidebar-grip", 400.4 - w);
    assert_eq!(widths(&rt.state).0, 400.0);
    assert_eq!(kept(&rt.state), (Some(400.0), None));
    // Short of half its minimum, which would fold it.
    drag_grip(&mut rt, "sidebar-grip", super::SIDEBAR.0 * 0.6 - 400.0);
    assert_eq!(widths(&rt.state).0, super::SIDEBAR.0, "its minimum");
    assert!(rt.state.sidebar.is_open());
    drag_grip(&mut rt, "sidebar-grip", super::SIDEBAR_WIDTH - super::SIDEBAR.0);
    assert_eq!(kept(&rt.state), (None, None), "back at lemur's own width");

    rt.update(|s, _| s.toggle_right(Right::Notes));
    frames(&mut rt, 30);
    let w = widths(&rt.state).1;
    // Its edge is its leading one: dragging right narrows it.
    drag_grip(&mut rt, "inspector-grip", w - 130.0);
    assert_eq!(widths(&rt.state).1, 240.0, "its minimum");
    assert_eq!(kept(&rt.state), (None, Some(240.0)));
}

/// v2 `dragging_a_side_panels_edge_resizes_it`, for its other half: a side panel dragged
/// as wide as it goes leaves the terminal its own (v2's `MIN_TERMINAL`, 320 points).
#[test]
fn a_side_panel_dragged_wide_leaves_the_terminal_its_own() {
    let server = HcServer::new("pa-panels-wide");
    let mut rt = window(server.config());
    rt.set_viewport((1000.0, 600.0), 2.0);
    rt.update(|s, _| s.toggle_right(Right::Notes));
    frames(&mut rt, 30);
    let main = |rt: &Runtime<Lemur>| rt.presented("main-pane").expect("the main pane").0[2];
    drag_grip(&mut rt, "inspector-grip", -800.0);
    let right = (rt.state.inspector.width(), main(&rt));
    // The sidebar alone, at the window's smallest.
    rt.update(|s, _| s.toggle_right(Right::Notes));
    rt.set_viewport((720.0, 480.0), 2.0);
    frames(&mut rt, 30);
    drag_grip(&mut rt, "sidebar-grip", 800.0);
    let left = (rt.state.sidebar.width(), main(&rt));
    // The terminal's frame takes a few points of its own.
    assert!(
        right.1 >= 300.0 && left.1 >= 300.0,
        "the right panel at {} leaves the terminal {}; the sidebar at {} leaves it {}",
        right.0,
        right.1,
        left.0,
        left.1
    );
}

/// v2 `dragging_a_project_reorders_the_sidebar`: a project's header dropped on another's
/// place moves it there, whatever its host: in the config and in the snapshot's views,
/// and in what a herdr-core change composes from, before and after a refresh. A project's
/// rows are part of it: dropped on one, it moves too.
#[test]
fn dragging_a_project_reorders_the_sidebar() {
    let server = HcServer::new("pa-project");
    let mut cfg = server.config();
    let mut mac = Host::new("mac", "local");
    mac.hc_session = cfg.hosts[0].hc_session.clone();
    cfg.hosts.push(mac);
    for (host, path) in [("local", "/srv/one"), ("local", "/srv/two"), ("mac", "/srv/three")] {
        cfg.bases.push(Base::new(host, path));
    }
    let mut rt = loaded_with(cfg);
    // The snapshot shown, and the one a herdr-core event would compose from what the last
    // refresh found.
    let consistent = |s: &Lemur| {
        let recomposed = snapshot::compose(&s.cfg, &s.data);
        for h in s.snap.hosts.iter().chain(&recomposed.hosts) {
            assert_eq!(h.bases.len(), if h.name == "mac" { 1 } else { 2 }, "{}", h.name);
            for b in &h.bases {
                assert_eq!(s.cfg.bases[b.index].host, h.name);
            }
        }
        s.cfg.bases.iter().map(|b| b.path.clone()).collect::<Vec<_>>()
    };
    let top = |s: &Lemur, i: usize| s.places[&format!("project-{i}")].0;
    assert_eq!(consistent(&rt.state), ["/srv/one", "/srv/two", "/srv/three"]);

    let dy = top(&rt.state, 0) - top(&rt.state, 2);
    drag(&mut rt, "project-2/section-header", 0.0, dy);
    frames(&mut rt, 10);
    assert_eq!(consistent(&rt.state), ["/srv/three", "/srv/one", "/srv/two"]);
    assert!(top(&rt.state, 0) < top(&rt.state, 1) && top(&rt.state, 1) < top(&rt.state, 2));

    rt.update(|s, _| s.refresh());
    settle(&mut rt, 1.5);
    assert_eq!(consistent(&rt.state), ["/srv/three", "/srv/one", "/srv/two"]);

    // Onto a row under three's name.
    let (y, h) = rt.state.places["project-0"];
    let dy = y + h + 20.0 - (top(&rt.state, 2) + rt.state.places["project-2"].1 / 2.0);
    drag(&mut rt, "project-2/section-header", 0.0, dy);
    assert_eq!(consistent(&rt.state), ["/srv/two", "/srv/three", "/srv/one"]);
}

/// v2 `ctrl_tab_goes_along_the_tabs`, for what `keys_go_through_the_tabs` leaves out:
/// holding ⌃ goes further and back, letting go settles there; from the menu, with no ⌃,
/// each goes one and settles.
#[test]
fn ctrl_tab_settles_when_ctrl_comes_up() {
    let server = HcServer::new("pa-cycle");
    let mut rt = window(server.config());
    let idle = Activity::Idle;
    apply(&mut rt, &[("a", idle), ("b", idle), ("c", idle)]);
    for t in ["a", "b", "c"] {
        open(&mut rt, t);
    }
    let shown = |rt: &Runtime<Lemur>| rt.state.active.clone().map(|(_, s)| s);
    // As the app has Rune send ⌃ going down and up.
    rt.set_modifier_keys(true);
    let ctrl = Mods { ctrl: true, ..Mods::default() };
    let hold = |rt: &mut Runtime<Lemur>, mods: Mods| rt.physical_input(PhysicalInput::Modifiers(mods));
    let let_go = |rt: &mut Runtime<Lemur>| {
        rt.physical_input(PhysicalInput::Modifiers(Mods::default()));
        rt.key(KeyEvent { key: K::Modifier { modifier: rune::Modifier::ControlLeft, pressed: false }, mods: Mods::default() });
        rt.frame(1.0 / 60.0);
    };

    hold(&mut rt, ctrl);
    press(&mut rt, K::Tab, ctrl);
    assert_eq!(shown(&rt).as_deref(), Some("a"), "round to the first tab");
    assert!(rt.state.cycling.is_some(), "⌃ is held");
    press(&mut rt, K::Tab, ctrl);
    assert_eq!(shown(&rt).as_deref(), Some("b"));
    press(&mut rt, K::Tab, Mods { shift: true, ..ctrl });
    assert_eq!(shown(&rt).as_deref(), Some("a"), "back, with ⌃ still held");
    let_go(&mut rt);
    assert!(rt.state.cycling.is_none(), "settled");
    hold(&mut rt, Mods { shift: true, ..ctrl });
    press(&mut rt, K::Tab, Mods { shift: true, ..ctrl });
    assert_eq!(shown(&rt).as_deref(), Some("c"), "back round to the last tab");
    let_go(&mut rt);

    rt.update(|s, tx| actions::run(s, Action::NextThread, tx));
    assert_eq!(shown(&rt).as_deref(), Some("a"));
    rt.update(|s, tx| actions::run(s, Action::PreviousThread, tx));
    assert_eq!(shown(&rt).as_deref(), Some("c"));
    assert!(rt.state.cycling.is_none(), "settled");
}

/// v2 `notes_fade_and_errors_wait_to_be_closed`: a note closes itself; an error stays
/// until its × is clicked.
#[test]
fn notes_fade_and_errors_wait_to_be_closed() {
    let mut rt = window(toml::from_str("").unwrap());
    let time = super::snackbar::NOTE_TIME.as_secs_f64();
    rt.update(|s, _| s.note("saved".into()));
    settle(&mut rt, time / 2.0);
    assert_eq!(rt.state.status, Some(("saved".into(), Kind::Note)));
    assert!(wait_for(&mut rt, time, |s| s.status.is_none()), "it closed itself");

    rt.update(|s, _| s.fail(anyhow::anyhow!("broke")));
    settle(&mut rt, time + 1.0);
    assert_eq!(rt.state.status, Some(("broke".into(), Kind::Error)));
    let close = rt
        .elements()
        .into_iter()
        .find(|e| e.path.contains("/snack-status/") && e.role == "button")
        .expect("its ×");
    let (x, y) = (close.rect[0] + close.rect[2] / 2.0, close.rect[1] + close.rect[3] / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    frames(&mut rt, 4);
    assert_eq!(rt.state.status, None);
}

/// v2 `removing_a_project_keeps_the_sidebar_consistent`: removing a project asks first,
/// then forgets it; the other projects' views keep pointing at their own config entries,
/// and a snapshot fetched before the removal is dropped.
#[test]
fn removing_a_project_keeps_the_sidebar_consistent() {
    let server = HcServer::new("pa-rmproject");
    let mut cfg = server.config();
    cfg.bases.push(Base::new("local", "/srv/one"));
    cfg.bases.push(Base::new("local", "/srv/two"));
    let mut rt = loaded_with(cfg);
    let one = Base::new("local", "/srv/one");
    rt.update(|s, _| s.confirm_remove_project(one.clone()));
    assert!(rt.state.dialog.is_some(), "asked");
    click(&mut rt, "dialog-cancel");
    assert_eq!(rt.state.cfg.bases.len(), 2, "cancelled");

    // A refresh is on its way when the project goes.
    rt.update(|s, _| {
        s.refresh();
        s.remove_project(&one);
    });
    // A herdr-core event before the next refresh lands composes from what the last one
    // found.
    let recomposed = snapshot::compose(&rt.state.cfg, &rt.state.data);
    let views = &recomposed.hosts[0].bases;
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].index, 0);
    settle(&mut rt, 2.0);
    let paths: Vec<&str> = rt.state.cfg.bases.iter().map(|b| b.path.as_str()).collect();
    assert_eq!(paths, ["/srv/two"]);
    let views = &rt.state.snap.hosts[0].bases;
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].index, 0);
}

/// v2 `removing_a_changed_worktree_asks_again`: a worktree with changes isn't removed
/// without asking again, and its thread keeps running until it is; its folder's color goes
/// with it.
#[test]
fn removing_a_changed_worktree_asks_again() {
    let server = HcServer::new("pa-rmdirty");
    let dir = TempDir::new("pa-rmdirty");
    let repo = repo(&dir, "repo", "main", &["draft"]);
    let wt = repo.join("wt/draft");
    std::fs::write(wt.join("notes.txt"), "unsaved").unwrap();
    let path = path_of(&wt);
    let t = server.run(&path, &[], &["sleep", "600"]);
    let mut cfg = server.config();
    cfg.bases.push(Base::new("local", &path_of(&repo)));
    let mut rt = loaded_with(cfg);
    let alive = || server.addresses().contains(&t);
    assert!(refreshing(&mut rt, 10.0, |s| !s.removal(0, path.clone(), "draft".into()).sessions.is_empty()), "listed");
    assert_eq!(rt.state.removal(0, path.clone(), "draft".into()).sessions, std::slice::from_ref(&t));
    let remove = |rt: &mut Runtime<Lemur>, force: bool| {
        rt.update(|s, _| {
            let mut removal = s.removal(0, path.clone(), "draft".into());
            // Only deleting its folder would lose the changes.
            removal.delete = true;
            s.remove_worktree(removal, force);
        });
    };
    remove(&mut rt, false);
    assert!(wait_for(&mut rt, 10.0, |s| s.dialog.is_some()), "asked again: {:?}", rt.state.status);
    assert!(wt.exists() && alive());
    rt.update(|s, _| s.dialog = None);
    assert!(alive());

    rt.update(|s, _| s.set_worktree_color(0, &path, Some("red")));
    remove(&mut rt, true);
    assert!(wait_for(&mut rt, 10.0, |_| !wt.exists()), "{:?}", rt.state.status);
    assert!(wait_until(Duration::from_secs(5), || !alive()));
    assert!(wait_for(&mut rt, 5.0, |s| s.cfg.bases[0].worktree_colors.is_empty()));
}

/// Opens the remove dialog for `worktree` of project 0; returns the threads it names.
fn confirm_remove(rt: &mut Runtime<Lemur>, worktree: &Path) -> Vec<String> {
    let path = path_of(worktree);
    let mut named = vec![];
    rt.update(|s, _| {
        let removal = s.removal(0, path, "x".into());
        named = removal.sessions.clone();
        s.confirm_remove_worktree(removal, None);
    });
    frames(rt, 2);
    assert!(rt.state.dialog.is_some(), "asked");
    named
}

/// v2 `removing_a_worktree_deletes_its_folder_when_ticked`: the remove dialog takes a
/// worktree out of git and keeps its folder, unless its box is ticked; then the folder goes
/// too.
#[test]
fn removing_a_worktree_deletes_its_folder_when_ticked() {
    let server = HcServer::new("pa-rmtick");
    let dir = TempDir::new("pa-rmtick");
    let repo = repo(&dir, "repo", "main", &["kept", "gone"]);
    let (kept, gone) = (repo.join("wt/kept"), repo.join("wt/gone"));
    let mut cfg = server.config();
    cfg.bases.push(Base::new("local", &path_of(&repo)));
    let mut rt = loaded_with(cfg);

    confirm_remove(&mut rt, &kept);
    click(&mut rt, "dialog-ok");
    assert!(wait_for(&mut rt, 10.0, |_| !listed(&repo).contains("wt/kept")), "{:?}", rt.state.status);
    assert!(kept.exists() && !kept.join(".git").exists());

    confirm_remove(&mut rt, &gone);
    click(&mut rt, "dialog-check");
    click(&mut rt, "dialog-ok");
    assert!(wait_for(&mut rt, 10.0, |_| !gone.exists()), "{:?}", rt.state.status);
    assert!(!listed(&repo).contains("wt/gone"));
}

/// v2's dialogs took Return for their button, as its tests agreed to them: here with the
/// remove dialog's.
#[test]
fn return_agrees_to_a_dialog() {
    let server = HcServer::new("pa-return");
    let dir = TempDir::new("pa-return");
    let repo = repo(&dir, "repo", "main", &["kept"]);
    let kept = repo.join("wt/kept");
    let mut cfg = server.config();
    cfg.bases.push(Base::new("local", &path_of(&repo)));
    let mut rt = loaded_with(cfg);
    confirm_remove(&mut rt, &kept);
    frames(&mut rt, 20);
    press(&mut rt, K::Enter, Mods::default());
    assert!(rt.state.dialog.is_none(), "Return agreed");
    assert!(wait_for(&mut rt, 10.0, |_| !listed(&repo).contains("wt/kept")), "{:?}", rt.state.status);
}

/// v2 `removing_a_worktree_closes_its_threads_first`: the remove dialog names the threads
/// running in a worktree. Cancelling keeps them and it; agreeing closes them, then removes
/// it. One the sidebar didn't know of yet keeps it until the host's ask is agreed to.
#[test]
fn removing_a_worktree_closes_its_threads_first() {
    let server = HcServer::new("pa-rmthreads");
    let dir = TempDir::new("pa-rmthreads");
    let repo = repo(&dir, "repo", "main", &["busy", "late"]);
    let (busy, late) = (repo.join("wt/busy"), repo.join("wt/late"));
    let start = |dir: &Path| server.run(&path_of(dir), &[], &["sleep", "600"]);
    let first = start(&busy);
    let mut cfg = server.config();
    cfg.bases.push(Base::new("local", &path_of(&repo)));
    let mut rt = loaded_with(cfg);
    let alive = |name: &str| server.addresses().iter().any(|a| a == name);
    assert!(refreshing(&mut rt, 10.0, |s| s.session_of(&key(&first)).is_some()), "listed");

    assert_eq!(confirm_remove(&mut rt, &busy), std::slice::from_ref(&first));
    press(&mut rt, K::Escape, Mods::default());
    assert!(rt.state.dialog.is_none());
    assert!(alive(&first) && listed(&repo).contains("wt/busy"));

    confirm_remove(&mut rt, &busy);
    click(&mut rt, "dialog-ok");
    assert!(wait_for(&mut rt, 10.0, |_| !listed(&repo).contains("wt/busy")), "{:?}", rt.state.status);
    assert!(wait_until(Duration::from_secs(5), || !alive(&first)));
    // The refresh after it lands before the next starts.
    settle(&mut rt, 1.5);

    // Started after the sidebar last looked: the host asks about it.
    let second = start(&late);
    assert!(wait_until(Duration::from_secs(5), || alive(&second)));
    assert!(confirm_remove(&mut rt, &late).is_empty());
    click(&mut rt, "dialog-ok");
    assert!(wait_for(&mut rt, 10.0, |s| s.dialog.is_some()), "the host asks: {:?}", rt.state.status);
    assert!(alive(&second) && listed(&repo).contains("wt/late"));
    click(&mut rt, "dialog-ok");
    assert!(wait_for(&mut rt, 10.0, |_| !listed(&repo).contains("wt/late")), "{:?}", rt.state.status);
    assert!(wait_until(Duration::from_secs(5), || !alive(&second)));
}

/// v2 `a_new_worktree_gets_the_branch_and_start_asked_for`: a new worktree's prompt offers
/// the host's branches to start from, the default one first; the worktree gets the branch
/// typed, or with none is detached. Picking another branch keeps the prompt open. (v2's
/// "Esc closes the list first" is the native menu's here: a list closed keeps the prompt.)
#[test]
fn a_new_worktree_gets_the_branch_and_start_asked_for() {
    let server = HcServer::new("pa-newwt");
    let dir = TempDir::new("pa-newwt");
    let origin = repo(&dir, "origin", "main", &[]);
    git_in(&dir, &["clone", "-q", &path_of(&origin), "repo"]);
    let repo = dir.join("repo");
    git_in(&repo, &["branch", "side"]);
    let mut cfg = server.config();
    // Typed into the new sessions; nothing to start.
    cfg.hosts[0].claude = "true".into();
    cfg.bases.push(Base::new("local", &path_of(&repo)));
    let mut rt = loaded_with(cfg);
    let ask = |rt: &mut Runtime<Lemur>| {
        rt.update(|s, _| s.ask_name(Ask::NewWorktree { base: 0, agent: Some(Agent::Claude) }));
        assert!(wait_for(rt, 10.0, |s| s.prompt.as_ref().is_some_and(|p| p.from.is_some())), "the branches");
        frames(rt, 20);
    };
    let from = |s: &Lemur| s.prompt.as_ref().and_then(|p| p.from.clone());
    let current = |name: &str| {
        let out = std::process::Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(repo.join(".claude/worktrees").join(name))
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };

    ask(&mut rt);
    assert_eq!(from(&rt.state).as_deref(), Some("origin/main"));
    type_text(&mut rt, "feat");
    assert_eq!(rt.state.prompt.as_ref().unwrap().input.text, "feat");
    click(&mut rt, "branch");
    type_text(&mut rt, "feature/x");
    assert_eq!(rt.state.prompt.as_ref().unwrap().branch.as_ref().unwrap().text, "feature/x");
    press(&mut rt, K::Enter, Mods::default());
    let made = repo.join(".claude/worktrees/feat");
    assert!(wait_for(&mut rt, 10.0, |_| made.exists()), "{:?}", rt.state.status);
    assert_eq!(current("feat"), "feature/x");

    // Another start, picked from the list, and no branch.
    ask(&mut rt);
    type_text(&mut rt, "loose");
    click(&mut rt, "from");
    assert!(rt.popup_menu().is_some(), "the list");
    rt.resolve_popup(None).unwrap();
    frames(&mut rt, 2);
    assert!(rt.state.prompt.is_some(), "closing the list closed the prompt");
    click(&mut rt, "from");
    let other = rt.state.prompt.as_ref().unwrap().branches.iter().find(|b| *b != "origin/main").cloned().expect("another branch");
    rt.resolve_popup(Some(&other)).unwrap();
    frames(&mut rt, 2);
    assert!(rt.state.prompt.is_some(), "still asking");
    assert_eq!(from(&rt.state), Some(other));
    click(&mut rt, "spotlight-input");
    press(&mut rt, K::Enter, Mods::default());
    let made = repo.join(".claude/worktrees/loose");
    assert!(wait_for(&mut rt, 10.0, |_| made.exists()), "{:?}", rt.state.status);
    assert_eq!(current("loose"), "", "detached");
}

/// v2 `gone_threads_resume_their_claude`: a Claude thread lemur starts is kept to resume.
/// Once its terminal is gone, as after the host restarts, it stays under its worktree,
/// dimmed: a click resumes it in a new one, so does Resume all, Forget drops it, and
/// killing a thread forgets it.
#[test]
fn gone_threads_resume_their_claude() {
    let server = HcServer::new("pa-gone");
    let dir = TempDir::new("pa-gone");
    let repo = repo(&dir, "api", "main", &[]);
    let path = path_of(&repo.canonicalize().unwrap());
    let mut cfg = server.config();
    // Prints what Claude would get.
    cfg.hosts[0].claude = "echo claude".into();
    cfg.bases.push(Base::new("local", &path));
    let mut rt = loaded_with(cfg);
    let kept = |s: &Lemur| s.cfg.threads.clone();
    // As the host restarting does, without a shell coming back.
    let restart = |rt: &mut Runtime<Lemur>| {
        let session = kept(&rt.state)[0].session.clone();
        server.hc(&["keep", &session, "--off"]);
        server.hc(&["kill", &session]);
        assert!(wait_until(Duration::from_secs(5), || !server.addresses().contains(&session)));
        let gone = format!("gone-local-{session}");
        assert!(refreshing(rt, 10.0, |s| s.session_of(&key(&session)).is_none()), "gone");
        frames(rt, 20);
        assert!(shows(rt, &gone), "its gone row");
    };
    let typed = |rt: &Runtime<Lemur>, text: &str| {
        let session = kept(&rt.state)[0].session.clone();
        wait_until(Duration::from_secs(5), || server.hc(&["read", &session]).contains(text))
    };

    rt.update(|s, _| s.ask_name(Ask::NewThread { host: "local".into(), base: Some(0), dir: path.clone(), agent: Some(Agent::Claude) }));
    frames(&mut rt, 2);
    type_text(&mut rt, "claude");
    press(&mut rt, K::Enter, Mods::default());
    assert!(wait_for(&mut rt, 10.0, |s| !kept(s).is_empty()), "kept: {:?}", rt.state.status);
    let threads = kept(&rt.state);
    let [thread] = threads.as_slice() else { panic!("kept {threads:?}") };
    let thread = thread.clone();
    assert!(crate::actions::is_uuid(&thread.claude_session));
    assert_eq!(thread.path, path);
    assert_eq!(thread.label.as_deref(), Some("claude"));
    assert!(typed(&rt, &format!("claude --session-id {}", thread.claude_session)));
    let resumed = format!("claude --resume {}", thread.claude_session);

    restart(&mut rt);
    assert!(!shows(&rt, &format!("row-local-{}", thread.session)));
    click(&mut rt, &format!("gone-local-{}", thread.session));
    assert!(wait_for(&mut rt, 10.0, |s| kept(s).first().is_some_and(|t| t.session != thread.session)), "in a new terminal");
    assert!(typed(&rt, &resumed), "{resumed} never ran");
    let again = kept(&rt.state)[0].session.clone();
    assert!(refreshing(&mut rt, 10.0, |s| s.session_of(&key(&again)).is_some()));
    frames(&mut rt, 20);
    assert!(shows(&rt, &format!("row-local-{again}")) && !shows(&rt, &format!("gone-local-{again}")));

    restart(&mut rt);
    rt.update(|s, _| {
        let gone = s.gone_in(0);
        assert_eq!(gone.len(), 1);
        s.resume(gone);
    });
    assert!(wait_for(&mut rt, 10.0, |s| kept(s).first().is_some_and(|t| t.session != again)), "Resume all");
    assert!(typed(&rt, &resumed), "Resume all never ran it");
    let third = kept(&rt.state)[0].session.clone();
    assert!(refreshing(&mut rt, 10.0, |s| s.session_of(&key(&third)).is_some()));
    frames(&mut rt, 20);
    assert!(shows(&rt, &format!("row-local-{third}")));

    // Killed in lemur, it's forgotten.
    rt.update(|s, _| s.close_thread(key(&third)));
    assert!(wait_for(&mut rt, 10.0, |s| kept(s).is_empty()), "forgotten");
    assert!(refreshing(&mut rt, 10.0, |s| s.session_of(&key(&third)).is_none()));
    frames(&mut rt, 20);
    assert!(!shows(&rt, &format!("row-local-{third}")) && !shows(&rt, &format!("gone-local-{third}")));

    // Forgotten, it's gone for good.
    rt.update(|s, _| s.keep_thread(thread.clone()));
    let gone = format!("gone-local-{}", thread.session);
    assert!(refreshing(&mut rt, 10.0, |s| !s.gone_in(0).is_empty()), "back as gone");
    frames(&mut rt, 20);
    assert!(shows(&rt, &gone));
    rt.update(|s, _| s.forget_threads("local", std::slice::from_ref(&thread.session)));
    frames(&mut rt, 20);
    assert!(kept(&rt.state).is_empty() && !shows(&rt, &gone));
    rt.update(|s, _| s.refresh());
    settle(&mut rt, 1.5);
    assert!(!shows(&rt, &gone));
}

/// v2 `refreshes_keep_claude_ids_current`: refreshes keep each running Claude's session ID:
/// a thread lemur didn't start gets kept, `/clear`ing it keeps the new ID, and the
/// conversation going on under another name moves it there.
#[test]
fn refreshes_keep_claude_ids_current() {
    let server = HcServer::new("pa-ids");
    let mut rt = window(server.config());
    let id = |n: u8| format!("0fdfabeb-ca80-4916-a103-275bbb5e86{n:02}");
    let running = |rt: &mut Runtime<Lemur>, runs: &[(&str, String)]| {
        let names: Vec<_> = runs.iter().map(|(s, _)| (*s, Activity::Idle)).collect();
        let mut snap = made_up(&names);
        for (s, (_, id)) in snap.hosts[0].other.iter_mut().zip(runs) {
            s.claude = Some(snapshot::ClaudeRun { id: id.clone(), path: "/srv/api".into() });
            s.label = "What its program calls it".into();
            s.own_label = "claude".into();
        }
        rt.update(|s, _| s.set_snapshot(snap));
    };
    let kept = |s: &Lemur| s.cfg.threads.iter().map(|t| (t.session.clone(), t.claude_session.clone())).collect::<Vec<_>>();

    running(&mut rt, &[("a", id(1)), ("b", "not-an-id".into())]);
    assert_eq!(kept(&rt.state), [("a".into(), id(1))]);
    // Kept as what it's labelled: its program's title isn't a name of yours.
    assert_eq!(rt.state.cfg.threads[0].label.as_deref(), Some("claude"));
    running(&mut rt, &[("a", id(2))]);
    assert_eq!(kept(&rt.state), [("a".into(), id(2))], "after /clear");
    running(&mut rt, &[("c", id(2))]);
    assert_eq!(kept(&rt.state), [("c".into(), id(2))], "renamed by hand");
}

/// v2 `cmd_j_starts_beside_a_thread_outside_every_project`: outside every project, ⌘J
/// starts where the thread on show runs, as it does in a worktree, with the threads there
/// first among those to go to.
#[test]
fn cmd_j_starts_beside_a_thread_outside_every_project() {
    let server = HcServer::new("pa-cmdj");
    let dir = TempDir::new("pa-cmdj");
    let here = path_of(&dir);
    let elsewhere = server.run("/", &["lemur.label=elsewhere"], &["sleep", "600"]);
    let mine = server.run(&here, &["lemur.label=mine"], &["sleep", "600"]);
    let mut rt = loaded(&server);
    assert!(refreshing(&mut rt, 10.0, |s| s.session_of(&key(&mine)).is_some() && s.session_of(&key(&elsewhere)).is_some()));
    open(&mut rt, &mine);

    press(&mut rt, K::Command('j'), cmd());
    let prompt = rt.state.prompt.as_ref().expect("asking");
    let Ask::NewThread { host, base, dir: at, agent } = &prompt.what else { panic!("not a new thread: {:?}", prompt.what) };
    assert_eq!((host.as_str(), *base, at.as_str(), *agent), ("local", None, here.as_str(), Some(Agent::Claude)));
    let listed: Vec<String> = rt.state.matches(prompt).into_iter().map(|m| m.key.1).collect();
    assert_eq!(listed, [mine.clone(), elsewhere.clone()], "those here first");

    press(&mut rt, K::Down, Mods::default());
    press(&mut rt, K::Down, Mods::default());
    type_text(&mut rt, "beside");
    assert_eq!(rt.state.prompt.as_ref().unwrap().input.text, "beside");
    press(&mut rt, K::Enter, Mods::default());
    assert!(wait_for(&mut rt, 10.0, |_| server.addresses().len() == 3), "{:?}", rt.state.status);
    let terminals = server.core().terminals();
    let new = terminals.iter().find(|t| t.address != mine && t.address != elsewhere).unwrap();
    assert_eq!(new.cwd, here);
    let label = |k: &str| new.labels.get(k).map(String::as_str);
    assert_eq!(label(crate::snapshot::LABEL), Some("beside"));
    assert_eq!(label(crate::snapshot::KIND), Some("shell"));
    assert!(wait_for(&mut rt, 10.0, |s| s.active == Some(key(&new.address))), "it shows");
}

/// v2 `names_are_asked_for_in_the_spotlight`: a bad name is explained there rather than in
/// the header, typing clears that, and Esc closes it.
#[test]
fn names_are_asked_for_in_the_spotlight() {
    let mut rt = window(toml::from_str("").unwrap());
    rt.update(|s, _| s.ask_name(Ask::Rename { host: "local".into(), session: "api/claude--calm-heron".into() }));
    frames(&mut rt, 2);
    assert_eq!(rt.state.prompt.as_ref().expect("asking").input.text, "claude");

    // The label starts selected, so typing replaces it.
    type_text(&mut rt, "not ok!");
    press(&mut rt, K::Enter, Mods::default());
    let prompt = rt.state.prompt.as_ref().expect("still asking");
    assert_eq!(prompt.input.text, "not ok!");
    let error = prompt.error.as_ref().map(|(e, _)| e.as_str());
    assert!(error.is_some_and(|e| e.contains("letters")), "{error:?}");
    assert!(rt.state.status.is_none(), "the header stays quiet");
    type_text(&mut rt, "x");
    assert!(rt.state.prompt.as_ref().unwrap().error.is_none(), "typing clears it");
    press(&mut rt, K::Escape, Mods::default());
    assert!(rt.state.prompt.is_none());
}

/// v2 `projects_fold_and_worktrees_take_a_color`: clicking a project's name folds it away
/// and back; a worktree's folder takes a color and gives it up.
#[test]
fn projects_fold_and_worktrees_take_a_color() {
    let server = HcServer::new("pa-fold");
    let mut cfg = server.config();
    cfg.bases.push(Base::new("local", "/srv/api"));
    let mut rt = loaded_with(cfg);
    let folded = |s: &Lemur| s.cfg.bases[0].collapsed;
    click(&mut rt, "project-0/section-header");
    assert!(folded(&rt.state));
    click(&mut rt, "project-0/section-header");
    assert!(!folded(&rt.state));

    rt.update(|s, _| s.set_worktree_color(0, "/srv/api", Some("teal")));
    let colors = |s: &Lemur| s.cfg.bases[0].worktree_colors.clone();
    assert_eq!(colors(&rt.state).get("/srv/api").map(String::as_str), Some("teal"));
    assert!(!colors(&rt.state).contains_key("/srv/api/wt"));
    rt.update(|s, _| s.set_worktree_color(0, "/srv/api", None));
    assert!(colors(&rt.state).is_empty());
}

/// v2 `a_long_worktree_name_stops_before_what_sits_beside_it`: a folded worktree's long
/// name ends before what runs in it, and before its button.
#[test]
fn a_long_worktree_name_stops_before_what_sits_beside_it() {
    let server = HcServer::new("pa-long");
    let dir = TempDir::new("pa-long");
    let branch = "fix/a-branch-name-long-enough-to-reach-what-sits-beside-it";
    let repo = repo(&dir, "api", branch, &[]);
    let path = path_of(&repo);
    // An agent and a shell, as counts wider than its button.
    server.run(&path, &["lemur.kind=claude"], &["sleep", "600"]);
    server.run(&path, &[], &["sleep", "600"]);
    let mut cfg = server.config();
    cfg.bases.push(Base::new("local", &path));
    let mut rt = loaded_with(cfg);
    assert!(refreshing(&mut rt, 10.0, |s| s.snap.hosts[0].bases[0].worktrees.first().is_some_and(|w| w.sessions.len() == 2)));
    let row_key = format!("wt-{path}");
    click(&mut rt, &row_key);
    frames(&mut rt, 30);
    let rect = |what: &str| info(&rt, what).unwrap_or_else(|| panic!("no {what}")).rect;
    let right = |r: [f32; 4]| r[0] + r[2];
    let (row, label, counts, plus) =
        (rect(&row_key), rect(&format!("{row_key}/label")), rect(&format!("{row_key}/detail")), rect(&format!("plus-{path}")));
    assert_eq!(info(&rt, &format!("{row_key}/detail")).and_then(|e| e.text).as_deref(), Some("1 agent, 1 shell"));
    assert!(right(label) <= counts[0] + 0.5, "{label:?} {counts:?}");
    assert!(right(label) <= plus[0] + 0.5, "{label:?} {plus:?}");
    assert!(right(counts) <= right(row) + 0.5 && right(plus) <= right(row) + 0.5, "{counts:?} {plus:?} {row:?}");
}

/// v2 `a_threads_indicators_are_separated`: a small dot separates what a thread's row says
/// of it, as its Linear issue from its age; with only its age there's none.
#[test]
fn a_threads_indicators_are_separated() {
    let server = HcServer::new("pa-sep");
    let pinned = server.run("/", &["lemur.label=pinned", "lemur.issue=ENG-1"], &["sleep", "600"]);
    let plain = server.run("/", &["lemur.label=plain"], &["sleep", "600"]);
    let mut rt = loaded(&server);
    assert!(refreshing(&mut rt, 10.0, |s| s.session_of(&key(&plain)).is_some()));
    frames(&mut rt, 20);
    let said = |t: &str| info(&rt, &format!("row-local-{t}/detail")).and_then(|e| e.text).unwrap_or_default();
    let (p, q) = (said(&pinned), said(&plain));
    assert_eq!(p.matches('·').count(), 1, "{p}");
    assert!(p.starts_with("ENG-1 · "), "{p}");
    assert_eq!(q.matches('·').count(), 0, "{q}");
}

/// v2 `clicking_a_worktree_folds_its_threads`: clicking a worktree folds its threads away
/// and back, saying what runs in it while folded; its + starts a terminal there, or opens
/// its notes, and leaves it open.
#[test]
fn clicking_a_worktree_folds_its_threads() {
    let server = HcServer::new("pa-foldwt");
    let dir = TempDir::new("pa-foldwt");
    let repo = repo(&dir, "api", "main", &[]);
    let path = path_of(&repo);
    let t = server.run(&path, &[], &["sleep", "600"]);
    let mut cfg = server.config();
    cfg.bases.push(Base::new("local", &path));
    let mut rt = loaded_with(cfg);
    assert!(refreshing(&mut rt, 10.0, |s| s.session_of(&key(&t)).is_some()));
    frames(&mut rt, 20);
    let row = format!("row-local-{t}");
    let wt = format!("wt-{path}");
    assert!(shows(&rt, &row));
    assert!(info(&rt, &format!("{wt}/detail")).is_none(), "open, it shows them");
    click(&mut rt, &wt);
    frames(&mut rt, 20);
    assert!(!shows(&rt, &row));
    // Folded, it says what runs in it.
    assert_eq!(info(&rt, &format!("{wt}/detail")).and_then(|e| e.text).as_deref(), Some("1 shell"));
    click(&mut rt, &wt);
    frames(&mut rt, 20);
    assert!(shows(&rt, &row));

    // Its + menu starts a terminal there.
    let before = server.addresses().len();
    click(&mut rt, &format!("plus-{path}"));
    assert!(rt.popup_menu().expect("its menu").find("new-terminal").is_some());
    rt.resolve_popup(Some("new-terminal")).unwrap();
    assert!(wait_for(&mut rt, 10.0, |_| server.addresses().len() > before), "no terminal started: {:?}", rt.state.status);
    let started = server.core().terminals();
    let new = started.iter().find(|x| x.address != t).unwrap();
    assert_eq!(new.cwd, path);
    assert_eq!(new.labels.get("lemur.kind").map(String::as_str), Some("shell"));
    frames(&mut rt, 20);
    assert!(shows(&rt, &row), "still open");

    // Or opens its notes.
    click(&mut rt, &format!("plus-{path}"));
    assert!(rt.popup_menu().expect("its menu").find("notes").is_some());
    rt.resolve_popup(Some("notes")).unwrap();
    frames(&mut rt, 20);
    assert_eq!(rt.state.right(), Some(Right::Notes));
    let header: Vec<String> = rt.elements().into_iter().filter(|e| e.path.contains("/notes-header")).filter_map(|e| e.text).collect();
    assert!(header.iter().any(|t| t.ends_with("· main")), "the notes of main: {header:?}");
}

/// v2 `folds_outlast_their_project_folding`: a folded worktree stays folded, and an open
/// Hidden row open, while their project is folded away and back, and in the next lemur.
#[test]
fn folds_outlast_their_project_folding() {
    let server = HcServer::new("pa-foldkeep");
    let dir = TempDir::new("pa-foldkeep");
    let repo = repo(&dir, "api", "main", &["a"]);
    let path = path_of(&repo);
    let t = server.run(&path, &[], &["sleep", "600"]);
    let mut cfg = server.config();
    cfg.bases.push(Base::new("local", &path));
    let mut rt = loaded_with(cfg);
    assert!(refreshing(&mut rt, 10.0, |s| s.session_of(&key(&t)).is_some()));
    let a = rt.state.snap.hosts[0].bases[0].worktrees[1].path.clone();
    rt.update(|s, _| s.set_worktree_hidden(0, &a, true));
    let threads = |rt: &Runtime<Lemur>| shows(rt, &format!("row-local-{t}"));
    let hidden = |rt: &Runtime<Lemur>| shows(rt, &format!("wt-{a}"));

    click(&mut rt, "hidden-0");
    click(&mut rt, &format!("wt-{path}"));
    frames(&mut rt, 20);
    assert!(hidden(&rt) && !threads(&rt));

    click(&mut rt, "project-0/section-header");
    frames(&mut rt, 20);
    assert!(!hidden(&rt) && !shows(&rt, &format!("wt-{path}")));
    click(&mut rt, "project-0/section-header");
    frames(&mut rt, 20);
    assert!(!threads(&rt), "the worktree stays folded");
    assert!(hidden(&rt), "Hidden stays open");

    let cfg = rt.state.cfg.clone();
    assert_eq!(cfg.bases[0].collapsed_worktrees, [path.as_str()]);
    assert!(cfg.bases[0].hidden_open);
    let mut next = loaded_with(cfg);
    assert!(refreshing(&mut next, 10.0, |s| s.session_of(&key(&t)).is_some()));
    frames(&mut next, 20);
    assert!(shows(&next, &format!("wt-{path}")));
    assert!(!threads(&next), "the next lemur keeps the worktree folded");
    assert!(hidden(&next), "and Hidden open");
}

/// v2 `hidden_worktrees_wait_under_their_project`: hiding a worktree moves it to its
/// project's Hidden row, folded until clicked; the others still drag past it, and unhiding
/// moves it back to its place.
#[test]
fn hidden_worktrees_wait_under_their_project() {
    let server = HcServer::new("pa-hide");
    let dir = TempDir::new("pa-hide");
    let repo = repo(&dir, "api", "main", &["a", "b"]);
    let mut cfg = server.config();
    cfg.bases.push(Base::new("local", &path_of(&repo)));
    let mut rt = loaded_with(cfg);
    let names = |s: &Lemur| -> Vec<String> { s.snap.hosts[0].bases[0].worktrees.iter().map(super::worktree_label).collect() };
    assert_eq!(names(&rt.state), ["main", "a", "b"]);
    let path = |s: &Lemur, i: usize| s.snap.hosts[0].bases[0].worktrees[i].path.clone();
    let (main, a, b) = (path(&rt.state, 0), path(&rt.state, 1), path(&rt.state, 2));
    assert!(!shows(&rt, "hidden-0"));

    rt.update(|s, _| s.set_worktree_hidden(0, &a, true));
    frames(&mut rt, 20);
    assert!(!shows(&rt, &format!("wt-{a}")), "folded away");
    assert!(shows(&rt, &format!("wt-{main}")) && shows(&rt, &format!("wt-{b}")));
    assert_eq!(rt.state.cfg.bases[0].hidden_worktrees, [a.as_str()]);

    // b and main are the rows that drag; a keeps its place behind them.
    let top = |s: &Lemur, p: &str| s.places[&format!("wt-{p}")].0;
    let dy = top(&rt.state, &main) - top(&rt.state, &b);
    drag(&mut rt, &format!("wt-{b}"), 0.0, dy);
    assert_eq!(names(&rt.state), ["b", "main", "a"]);

    click(&mut rt, "hidden-0");
    assert!(shows(&rt, &format!("wt-{a}")), "opened");

    rt.update(|s, _| s.set_worktree_hidden(0, &a, false));
    frames(&mut rt, 20);
    assert!(!shows(&rt, "hidden-0"));
    assert!(shows(&rt, &format!("wt-{a}")));
    assert!(rt.state.cfg.bases[0].hidden_worktrees.is_empty());
    rt.update(|s, _| s.refresh());
    settle(&mut rt, 1.5);
    assert_eq!(names(&rt.state), ["b", "main", "a"]);
}

/// v2 `menu_actions_reach_the_main_window`: ⌘O opens the folder picker, ⌘R toggles the
/// right panel (the notes at first), Refresh refreshes.
#[test]
fn menu_actions_reach_the_main_window() {
    let mut rt = window(toml::from_str("").unwrap());
    press(&mut rt, K::Command('o'), cmd());
    assert!(rt.state.panels.picker.is_some());
    press(&mut rt, K::Escape, Mods::default());
    assert!(rt.state.panels.picker.is_none());

    press(&mut rt, K::Command('r'), cmd());
    assert_eq!(rt.state.right(), Some(Right::Notes));
    press(&mut rt, K::Command('r'), cmd());
    assert_eq!(rt.state.right(), None);

    rt.state.loaded = false;
    rt.update(|s, tx| actions::run(s, Action::Refresh, tx));
    assert!(wait_for(&mut rt, 5.0, |s| s.loaded), "a refresh came in");
}

/// v2 `refreshes_slow_down_out_of_sight`: out of sight, refreshes come every AWAY ticks
/// rather than every tick; back in sight, every tick again.
#[test]
fn refreshes_slow_down_out_of_sight() {
    // No hosts: nothing to watch, and a refresh is quick.
    let mut rt = window(toml::from_str("").unwrap());
    rt.window_focus(false);
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.looking);
    let start = std::time::Instant::now();
    rt.update(|s, _| s.start());
    assert!(wait_for(&mut rt, 1.0, |s| s.loaded), "the first one");
    // The first tick's refresh.
    settle(&mut rt, 1.0);
    rt.state.loaded = false;
    // Ticks at 3, 6, 9 and 12 s don't refresh; the one at 15 s does.
    let at = |secs: f64| secs - start.elapsed().as_secs_f64();
    assert!(!wait_for(&mut rt, at(13.5), |s| s.loaded), "refreshed out of sight after {:?}", start.elapsed());
    assert!(wait_for(&mut rt, at(17.0), |s| s.loaded), "every AWAY ticks");

    rt.window_focus(true);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.looking);
    for _ in 0..2 {
        rt.state.loaded = false;
        assert!(wait_for(&mut rt, 3.5, |s| s.loaded), "every tick in sight");
    }
}

/// How many of the next `n` frames the window asks for another after.
fn frames_asked(rt: &mut Runtime<Lemur>, n: usize) -> usize {
    (0..n).filter(|_| rt.frame(1.0 / 60.0)).count()
}

/// v2 `working_dots_breathe_only_in_the_active_window`: a working thread's dot breathes
/// only while Lemur is the active window; otherwise it holds still, and the window isn't
/// drawn for it.
#[test]
fn working_dots_breathe_only_in_the_active_window() {
    let mut rt = window(toml::from_str("").unwrap());
    rt.window_focus(true);
    apply(&mut rt, &[("busy", Activity::Idle)]);
    frames(&mut rt, 120);
    assert_eq!(frames_asked(&mut rt, 60), 0, "at rest, nothing to draw");
    apply(&mut rt, &[("busy", Activity::Working)]);
    frames(&mut rt, 60);
    assert!(frames_asked(&mut rt, 60) > 0, "it breathes");
    let commits = rt.stats.commits;
    frames(&mut rt, 60);
    assert_eq!(rt.stats.commits, commits, "breathing doesn't run the view again");

    rt.window_focus(false);
    apply(&mut rt, &[("busy", Activity::Idle)]);
    frames(&mut rt, 60);
    assert_eq!(frames_asked(&mut rt, 60), 0, "in the background at rest, nothing to draw");
    apply(&mut rt, &[("busy", Activity::Working)]);
    frames(&mut rt, 60);
    assert_eq!(frames_asked(&mut rt, 60), 0, "still in the background");
    rt.window_focus(true);
    frames(&mut rt, 30);
    assert!(frames_asked(&mut rt, 60) > 0, "breathing again");
}

/// v2 `a_working_tab_doesnt_redraw_the_sidebar_twice`, as it holds in Rune: a working
/// thread on show, its tab and its row both marked, doesn't run the view again for them.
#[test]
fn a_working_tab_doesnt_run_the_view_again() {
    let mut rt = window(toml::from_str("").unwrap());
    rt.window_focus(true);
    apply(&mut rt, &[("busy", Activity::Working), ("idle", Activity::Idle)]);
    rt.state.active = Some(key("busy"));
    rt.update(|_, _| {});
    frames(&mut rt, 120);
    assert!(shows(&rt, "t-busy"), "its tab");
    let commits = rt.stats.commits;
    frames(&mut rt, 120);
    assert_eq!(rt.stats.commits, commits);
}

/// v2 `a_host_out_of_reach_shows_no_load`: a host that can't be reached shows no load,
/// rather than its last.
#[test]
fn a_host_out_of_reach_shows_no_load() {
    let mut rt = window(toml::from_str("").unwrap());
    rt.state.loads.insert("local".into(), (crate::machine::Load::default(), Some(0.5)));
    let mut snap = made_up(&[]);
    snap.hosts[0].error = Some("can't reach it".into());
    rt.update(|s, _| s.set_snapshot(snap));
    assert!(rt.state.loads.is_empty());
}

/// Applies a snapshot in which `local` can't be reached.
fn out_of_reach(rt: &mut Runtime<Lemur>) {
    let mut snap = made_up(&[]);
    snap.hosts[0].error = Some("can't reach it".into());
    rt.update(|s, _| s.set_snapshot(snap));
}

/// v2 `a_host_out_of_reach_keeps_what_its_threads_did`: while its host can't be reached, a
/// thread keeps what it was doing and its done mark: back, it hasn't just changed, and
/// finishing then still counts.
#[test]
fn a_host_out_of_reach_keeps_what_its_threads_did() {
    let mut rt = window(toml::from_str("").unwrap());
    rt.state.active = Some(key("shown"));
    let (working, idle) = (Activity::Working, Activity::Idle);
    apply(&mut rt, &[("shown", working), ("done", working)]);
    apply(&mut rt, &[("shown", working), ("done", idle)]);
    out_of_reach(&mut rt);
    assert!(rt.state.done.contains(&key("done")), "kept");
    apply(&mut rt, &[("shown", idle), ("done", idle)]);
    assert!(rt.state.done.contains(&key("done")));

    apply(&mut rt, &[("shown", working), ("other", working)]);
    out_of_reach(&mut rt);
    apply(&mut rt, &[("shown", working), ("other", idle)]);
    assert!(rt.state.done.contains(&key("other")), "it finished: {:?}", rt.state.done);
}

/// v2 `a_session_that_finishes_elsewhere_is_marked_done`, for what
/// `threads_are_marked_done_and_come_back_when_needed` leaves out: the one on show isn't
/// news, one waiting on you isn't done, and working again clears the mark.
#[test]
fn a_session_that_finishes_elsewhere_is_marked_done() {
    let mut rt = window(toml::from_str("").unwrap());
    rt.state.active = Some(key("shown"));
    let w = Activity::Working;
    apply(&mut rt, &[("shown", w), ("elsewhere", w), ("asking", w)]);
    assert!(rt.state.done.is_empty());
    apply(&mut rt, &[("shown", Activity::Idle), ("elsewhere", Activity::Idle), ("asking", Activity::NeedsInput)]);
    let done = &rt.state.done;
    assert!(done.contains(&key("elsewhere")), "{done:?}");
    assert!(!done.contains(&key("shown")), "the shown session isn't news");
    assert!(!done.contains(&key("asking")), "waiting on you isn't done");
    apply(&mut rt, &[("elsewhere", w)]);
    assert!(rt.state.done.is_empty(), "working again clears it: {:?}", rt.state.done);
}

/// v2 `opening_a_thread_unfolds_its_way`: opening a thread unfolds its project, its
/// worktree and the Hidden row it's under, for good; the header names all three.
#[test]
fn opening_a_thread_unfolds_its_way() {
    let server = HcServer::new("pa-reveal");
    let dir = TempDir::new("pa-reveal");
    let repo = repo(&dir, "api", "main", &["a"]);
    let wt = path_of(&repo.join("wt/a"));
    let thread = server.run(&wt, &["lemur.label=t"], &["sleep", "600"]);
    let mut cfg = server.config();
    cfg.bases.push(Base::new("local", &path_of(&repo)));
    let mut rt = loaded_with(cfg);
    assert!(refreshing(&mut rt, 10.0, |s| s.session_of(&key(&thread)).is_some()));
    let a = rt.state.snap.hosts[0].bases[0].worktrees[1].path.clone();
    rt.update(|s, _| {
        s.set_worktree_hidden(0, &a, true);
        s.toggle_worktree(0, &a);
        s.toggle_project(0);
    });
    frames(&mut rt, 20);
    assert!(!shows(&rt, &format!("wt-{a}")));

    open(&mut rt, &thread);
    frames(&mut rt, 20);
    assert!(shows(&rt, &format!("wt-{a}")));
    assert!(shows(&rt, &format!("row-local-{thread}")));
    let base = &rt.state.cfg.bases[0];
    assert!(!base.collapsed && base.collapsed_worktrees.is_empty() && base.hidden_open);
    assert_eq!(rt.state.titles("local", &thread), (Some("api › a".to_string()), "t".to_string()));
}

/// v2 `tabs_are_the_threads_of_the_worktree_on_show`: the tabs are the threads of the
/// worktree on show: a click opens one, and its × closes it, asking first while its
/// program runs.
#[test]
fn tabs_are_the_threads_of_the_worktree_on_show() {
    let server = HcServer::new("pa-tabs");
    let dir = TempDir::new("pa-tabs");
    let repo = repo(&dir, "api", "main", &["auth"]);
    let wt = path_of(&repo.join("wt/auth"));
    let a = server.run(&wt, &["lemur.label=a"], &["sleep", "600"]);
    let b = server.run(&wt, &["lemur.label=b"], &["sleep", "600"]);
    let c = server.run(&path_of(&repo), &["lemur.label=c"], &["sleep", "600"]);
    let mut cfg = server.config();
    cfg.bases.push(Base::new("local", &path_of(&repo)));
    let mut rt = loaded_with(cfg);
    assert!(refreshing(&mut rt, 10.0, |s| [&a, &b, &c].iter().all(|t| s.session_of(&key(t)).is_some_and(|x| x.busy))));
    open(&mut rt, &a);
    frames(&mut rt, 20);
    let tab = |t: &str| format!("t-{t}");
    assert!(shows(&rt, &tab(&a)) && shows(&rt, &tab(&b)));
    assert!(!shows(&rt, &tab(&c)), "c runs in another worktree");

    click(&mut rt, &tab(&b));
    assert_eq!(rt.state.active, Some(key(&b)));

    // a's ×, which shows under the pointer.
    let close = |rt: &mut Runtime<Lemur>| {
        let (x, y) = rt.find(&tab(&a)).expect("a's tab");
        rt.pointer_move(x, y);
        frames(rt, 20);
        let (x, y) = find_in(rt, &tab(&a), "close").expect("its ×");
        rt.pointer_move(x, y);
        frames(rt, 2);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        frames(rt, 4);
    };
    close(&mut rt);
    assert!(rt.state.dialog.as_ref().is_some_and(|d| d.title.starts_with("Close")), "asked");
    press(&mut rt, K::Escape, Mods::default());
    assert!(rt.state.dialog.is_none());
    assert!(server.addresses().contains(&a), "cancelled");
    close(&mut rt);
    click(&mut rt, "dialog-ok");
    assert!(wait_until(Duration::from_secs(5), || !server.addresses().contains(&a)));
    assert!(refreshing(&mut rt, 10.0, |s| s.session_of(&key(&a)).is_none()));
    frames(&mut rt, 20);
    assert!(!shows(&rt, &tab(&a)) && shows(&rt, &tab(&b)));
}

