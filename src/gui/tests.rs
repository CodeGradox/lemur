//! The window, driven as a person would through Rune's `Runtime`: presses at elements and
//! keys, against real herdr-core sessions of the tests' own (`HcServer`), or snapshots made
//! up here.

use super::core::wait_for;
use super::{Action, Key, Lemur, List, Right, actions, on_key, poll, view};
use crate::config::{Agent, Config, ThreadOrder};
use crate::snapshot::{self, Activity, HostView, Session, Snapshot};
use crate::test_support::{HcServer, wait_until};
use rune::{Env, Key as K, KeyEvent, Mods, Runtime};
use std::time::Duration;

pub(super) fn key(s: &str) -> Key {
    ("local".into(), s.into())
}

/// The window on `cfg`, its first snapshot not fetched yet.
pub(super) fn window(cfg: Config) -> Runtime<Lemur> {
    let env = Env { system_dark: true, scale: 2.0 };
    let mut rt = Runtime::new(Lemur::new(cfg, None, None), view, (1280.0, 820.0), env);
    rt.on_key = Some(on_key);
    rt.poll = Some(poll);
    rt.frame(0.0);
    rt
}

/// The window on `server`'s core, once its first snapshot is in.
fn loaded(server: &HcServer) -> Runtime<Lemur> {
    let mut rt = window(server.config());
    rt.state.refresh();
    assert!(wait_for(&mut rt, 10.0, |s| s.loaded), "the first snapshot");
    rt
}

/// Waits for `done`, fetching a snapshot every half second: tests don't tick.
pub(super) fn refreshing(rt: &mut Runtime<Lemur>, secs: f64, done: impl Fn(&Lemur) -> bool) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed().as_secs_f64() < secs {
        rt.update(|s, _| s.refresh());
        if wait_for(rt, 0.5, &done) {
            return true;
        }
    }
    false
}

pub(super) fn session(name: &str, activity: Activity) -> Session {
    Session {
        name: name.to_string(),
        label: name.to_string(),
        own_label: name.to_string(),
        path: "/tmp".into(),
        idle_secs: 0,
        activity,
        waiting_for: None,
        started_claude: false,
        codex: false,
        order: None,
        started: 0,
        claude: None,
        busy: false,
        running: true,
        pid: None,
        shell: None,
        helpers: vec![],
        away: false,
        silent: false,
        issue: None,
    }
}

pub(super) fn made_up(sessions: &[(&str, Activity)]) -> Snapshot {
    Snapshot {
        hosts: vec![HostView {
            name: "local".into(),
            error: None,
            needs_you: false,
            home: String::new(),
            machine: Default::default(),
            bases: vec![],
            other: sessions.iter().map(|(n, a)| session(n, *a)).collect(),
            gone: vec![],
        }],
    }
}

/// Applies a made-up snapshot the way a refresh does.
pub(super) fn apply(rt: &mut Runtime<Lemur>, sessions: &[(&str, Activity)]) {
    let snap = made_up(sessions);
    rt.update(|s, _| s.set_snapshot(snap));
    rt.frame(1.0 / 60.0);
}

pub(super) fn open(rt: &mut Runtime<Lemur>, session: &str) {
    rt.update(|s, _| s.open("local".into(), session.into()));
    rt.frame(1.0 / 60.0);
}

pub(super) fn press(rt: &mut Runtime<Lemur>, key: K, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    rt.frame(1.0 / 60.0);
}

pub(super) fn cmd() -> Mods {
    Mods { cmd: true, logo: true, ..Mods::default() }
}

fn click(rt: &mut Runtime<Lemur>, key: &str) {
    // What just came in fades in: a press lands once it shows.
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

pub(super) fn type_text(rt: &mut Runtime<Lemur>, text: &str) {
    for c in text.chars() {
        let shift = c.is_ascii_uppercase() || "$()*!?:\"<>{}|~^&%@#_+".contains(c);
        rt.key(KeyEvent { key: K::Text(c.to_string()), mods: Mods { shift, ..Mods::default() } });
    }
    rt.frame(1.0 / 60.0);
}

fn screen(rt: &Runtime<Lemur>, k: &Key) -> Vec<String> {
    super::terminal::lines(&rt.state, k)
}

/// The sidebar lists the core's terminals, and a click on one shows it in a terminal of
/// its own, with its tab.
#[test]
fn the_sidebar_follows_the_core_and_a_click_shows_a_thread() {
    let server = HcServer::new("rside");
    let t = server.run("/", &[], &["sh"]);
    let mut rt = loaded(&server);
    let row = format!("row-local-{t}");
    assert!(rt.find(&row).is_some(), "a row for {t}");
    click(&mut rt, &row);
    assert_eq!(rt.state.active, Some(key(&t)));
    assert!(rt.find(&format!("t-{t}")).is_some(), "its tab");
    assert!(wait_for(&mut rt, 10.0, |s| s.terminals.get(&key(&t)).is_some_and(|v| v.is_attached())), "attached");
}

/// What's typed reaches the thread on show's program, and its answer shows.
#[test]
fn typing_reaches_the_thread_on_show() {
    let server = HcServer::new("rtype");
    let t = server.run("/", &[], &["sh"]);
    let mut rt = loaded(&server);
    open(&mut rt, &t);
    assert!(wait_for(&mut rt, 10.0, |s| s.terminals.get(&key(&t)).is_some_and(|v| v.painted)), "painted");
    type_text(&mut rt, "echo $((6*7))");
    press(&mut rt, K::Enter, Mods::default());
    let k = key(&t);
    assert!(
        wait_for(&mut rt, 10.0, |s| super::terminal::lines(s, &k).iter().any(|l| l == "42")),
        "the shell's answer: {:?}",
        screen(&rt, &k)
    );
}

/// The terminal takes the size of the pane it's shown in.
#[test]
fn the_terminal_follows_the_window_size() {
    let server = HcServer::new("rsize");
    let t = server.run("/", &[], &["sleep", "600"]);
    let mut rt = loaded(&server);
    open(&mut rt, &t);
    let size = |rt: &mut Runtime<Lemur>| {
        for _ in 0..4 {
            rt.frame(1.0 / 60.0);
        }
        rt.state.terminals[&key(&t)].size
    };
    rt.set_viewport((900.0, 600.0), 2.0);
    let small = size(&mut rt);
    rt.set_viewport((1400.0, 900.0), 2.0);
    let large = size(&mut rt);
    assert!(large.cols > small.cols && large.lines > small.lines, "{small:?} -> {large:?}");
}

/// Closing the thread on show shows the next tab in its list, or the one before it if it
/// was the last.
#[test]
fn a_thread_that_ends_hands_over_to_its_neighbour() {
    let server = HcServer::new("rnext");
    let mut rt = window(server.config());
    let idle = Activity::Idle;
    apply(&mut rt, &[("a", idle), ("b", idle), ("c", idle)]);
    open(&mut rt, "b");
    apply(&mut rt, &[("a", idle), ("c", idle)]);
    assert_eq!(rt.state.active, Some(key("c")), "the next tab");
    apply(&mut rt, &[("a", idle)]);
    assert_eq!(rt.state.active, Some(key("a")), "the one before");
}

/// ⌃Tab goes along the tabs and round; ⌘2 shows the second.
#[test]
fn keys_go_through_the_tabs() {
    let server = HcServer::new("rtabs");
    let mut rt = window(server.config());
    let idle = Activity::Idle;
    apply(&mut rt, &[("a", idle), ("b", idle), ("c", idle)]);
    open(&mut rt, "a");
    let ctrl = Mods { ctrl: true, ..Mods::default() };
    press(&mut rt, K::Tab, ctrl);
    assert_eq!(rt.state.active, Some(key("b")));
    press(&mut rt, K::Tab, ctrl);
    press(&mut rt, K::Tab, ctrl);
    assert_eq!(rt.state.active, Some(key("a")), "round past the end");
    press(&mut rt, K::Tab, Mods { shift: true, ..ctrl });
    assert_eq!(rt.state.active, Some(key("c")), "and back");
    press(&mut rt, K::Command('2'), cmd());
    assert_eq!(rt.state.active, Some(key("b")));
}

/// ⌃Tab can go by when threads were opened instead, as ⌘Tab goes through apps.
#[test]
fn ctrl_tab_can_go_by_when_threads_were_opened() {
    let server = HcServer::new("rrecent");
    let mut rt = window(server.config());
    rt.state.prefs.thread_order = ThreadOrder::Recent;
    let idle = Activity::Idle;
    apply(&mut rt, &[("a", idle), ("b", idle), ("c", idle)]);
    for t in ["a", "c", "b"] {
        open(&mut rt, t);
    }
    press(&mut rt, K::Tab, Mods { ctrl: true, ..Mods::default() });
    assert_eq!(rt.state.active, Some(key("c")), "the one opened before");
}

/// A thread finishing out of sight is marked done until opened; one put away comes back
/// when it needs you.
#[test]
fn threads_are_marked_done_and_come_back_when_needed() {
    let server = HcServer::new("rdone");
    let mut rt = window(server.config());
    apply(&mut rt, &[("a", Activity::Idle), ("b", Activity::Working)]);
    open(&mut rt, "a");
    apply(&mut rt, &[("a", Activity::Idle), ("b", Activity::Idle)]);
    assert!(rt.state.done.contains(&key("b")));
    open(&mut rt, "b");
    assert!(!rt.state.done.contains(&key("b")), "seen");

    let mut snap = made_up(&[("a", Activity::Idle), ("b", Activity::Idle)]);
    snap.hosts[0].other[0].away = true;
    rt.update(|s, _| s.set_snapshot(snap));
    assert!(rt.state.session_of(&key("a")).unwrap().away);
    let mut snap = made_up(&[("a", Activity::NeedsInput), ("b", Activity::Idle)]);
    snap.hosts[0].other[0].away = true;
    rt.update(|s, _| s.set_snapshot(snap));
    assert!(!rt.state.session_of(&key("a")).unwrap().away, "it needs you: back");
}

/// Dragging a tab, or ⌘⇧→, moves its thread: in the sidebar at once, and in herdr-core
/// for good.
#[test]
fn moving_a_tab_reorders_its_list() {
    let server = HcServer::new("rorder");
    let [a, b, c] = ["a", "b", "c"].map(|_| server.run("/", &[], &["sleep", "600"]));
    let mut rt = loaded(&server);
    open(&mut rt, &a);
    let shown = |s: &Lemur| s.snap.hosts[0].other.iter().map(|t| t.name.clone()).collect::<Vec<_>>();
    press(&mut rt, K::Right, Mods { shift: true, ..cmd() });
    assert_eq!(shown(&rt.state), [b.as_str(), &a, &c]);
    let list: List = ("local".into(), None);
    rt.update(|s, _| s.reorder(list, 0, 2));
    assert_eq!(shown(&rt.state), [a.as_str(), &c, &b]);
    // The second order is written once the first is: the window has to run for that.
    let cfg = server.config();
    let want = [a.clone(), c.clone(), b.clone()];
    assert!(
        wait_for(&mut rt, 15.0, |_| {
            let kept: Vec<String> = snapshot::fetch(&cfg).hosts[0].other.iter().map(|t| t.name.clone()).collect();
            kept == want
        }),
        "herdr-core keeps the order"
    );
}

/// ⌘W on a shell at its prompt closes it at once; on one running something, it asks.
#[test]
fn closing_asks_only_while_something_runs() {
    let server = HcServer::new("rclose");
    let idle = server.run("/", &[], &["sh"]);
    let busy = server.run("/", &[], &["sh", "-c", "sleep 600"]);
    let mut rt = loaded(&server);
    open(&mut rt, &busy);
    assert!(refreshing(&mut rt, 10.0, |s| s.session_of(&key(&busy)).is_some_and(|t| t.busy)), "it runs something");
    press(&mut rt, K::Command('w'), cmd());
    assert!(rt.state.dialog.as_ref().is_some_and(|d| d.title.starts_with("Close")), "asked");
    click(&mut rt, "dialog-cancel");
    assert!(rt.state.dialog.is_none());

    open(&mut rt, &idle);
    assert!(refreshing(&mut rt, 10.0, |s| s.session_of(&key(&idle)).is_some_and(|t| !t.busy)), "at its prompt");
    press(&mut rt, K::Command('w'), cmd());
    assert!(wait_for(&mut rt, 10.0, |s| s.session_of(&key(&idle)).is_none()), "closed without asking");
    assert!(rt.state.dialog.is_none());
}

/// ⌘T starts a terminal where the thread on show runs, and shows it.
#[test]
fn cmd_t_opens_a_terminal_beside_the_thread() {
    let server = HcServer::new("rnewt");
    let t = server.run("/tmp", &[], &["sh"]);
    let mut rt = loaded(&server);
    open(&mut rt, &t);
    press(&mut rt, K::Command('t'), cmd());
    assert!(wait_for(&mut rt, 15.0, |s| s.active.as_ref().is_some_and(|k| k.1 != t)), "a new one shows");
    let shown = rt.state.active.clone().unwrap();
    assert!(wait_for(&mut rt, 10.0, |s| s.session_of(&shown).is_some_and(|x| x.path == "/tmp" || x.path.ends_with("/tmp"))));
}

/// ⌘P lists the threads; typing narrows them, and ⏎ opens the one picked.
#[test]
fn cmd_p_goes_to_a_thread() {
    let server = HcServer::new("rgoto");
    let mut rt = window(server.config());
    let idle = Activity::Idle;
    apply(&mut rt, &[("alpha", idle), ("beta", idle), ("gamma", idle)]);
    open(&mut rt, "alpha");
    press(&mut rt, K::Command('p'), cmd());
    assert!(rt.state.prompt.is_some());
    assert!(rt.find("spotlight-input").is_some());
    rt.update(|s, _| {
        if let Some(p) = &mut s.prompt {
            p.input = rune::TextEdit::new("gam");
        }
    });
    rt.frame(1.0 / 60.0);
    assert!(rt.find("go-0").is_some() && rt.find("go-1").is_none(), "one match");
    rt.update(|s, _| s.prompt_enter());
    assert_eq!(rt.state.active, Some(key("gamma")));
    assert!(rt.state.prompt.is_none());
}

/// ⌘J asks what starts where the thread on show runs, Claude to start with; ↓ picks Codex,
/// then the shell, and a name typed sticks.
#[test]
fn cmd_j_asks_for_claude_or_a_shell_here() {
    let server = HcServer::new("rcmdj");
    let mut rt = window(server.config());
    apply(&mut rt, &[("a", Activity::Idle)]);
    open(&mut rt, "a");
    press(&mut rt, K::Command('j'), cmd());
    let agent = |s: &Lemur| match &s.prompt.as_ref().unwrap().what {
        super::prompt::Ask::NewThread { agent, .. } => *agent,
        _ => panic!("a new thread"),
    };
    assert_eq!(agent(&rt.state), Some(Agent::Claude));
    assert_eq!(rt.state.prompt.as_ref().unwrap().input.text, "claude");
    rt.update(|s, _| s.step(false));
    assert_eq!(agent(&rt.state), Some(Agent::Codex));
    assert_eq!(rt.state.prompt.as_ref().unwrap().input.text, "codex", "the name follows");
    press(&mut rt, K::Escape, Mods::default());
    assert!(rt.state.prompt.is_none(), "Escape cancels");
}

/// Renaming a thread shows its new name at once, and herdr-core keeps it.
#[test]
fn a_thread_renamed_keeps_its_name() {
    let server = HcServer::new("rrename");
    let t = server.run("/", &[], &["sh"]);
    let mut rt = loaded(&server);
    rt.update(|s, _| s.ask_name(super::prompt::Ask::Rename { host: "local".into(), session: t.clone() }));
    rt.update(|s, _| {
        if let Some(p) = &mut s.prompt {
            p.input = rune::TextEdit::new("build");
        }
    });
    rt.update(|s, _| s.prompt_enter());
    assert!(wait_for(&mut rt, 10.0, |s| s.session_of(&key(&t)).is_some_and(|x| x.label == "build")));
    let cfg = server.config();
    assert!(wait_until(Duration::from_secs(10), || snapshot::fetch(&cfg).hosts[0].other.iter().any(|x| x.label == "build")));
}

/// The panels take turns on the right; ⌘R hides the one showing, or brings back the last.
#[test]
fn the_panels_take_turns_on_the_right() {
    let server = HcServer::new("rright");
    let mut rt = window(server.config());
    rt.update(|s, tx| actions::run(s, Action::ToggleChanges, tx));
    assert_eq!(rt.state.right, Some(Right::Changes));
    assert!(rt.state.inspector.is_open());
    rt.update(|s, tx| actions::run(s, Action::ToggleAgents, tx));
    assert_eq!(rt.state.right, Some(Right::Agents));
    press(&mut rt, K::Command('r'), cmd());
    assert_eq!(rt.state.right, None);
    assert!(!rt.state.inspector.is_open());
    press(&mut rt, K::Command('r'), cmd());
    assert_eq!(rt.state.right, Some(Right::Agents), "the last one comes back");
}

/// ⌘B folds the sidebar away, and back.
#[test]
fn cmd_b_toggles_the_sidebar() {
    let server = HcServer::new("rsidebar");
    let mut rt = window(server.config());
    press(&mut rt, K::Command('b'), cmd());
    assert!(!rt.state.sidebar.is_open());
    press(&mut rt, K::Command('b'), cmd());
    assert!(rt.state.sidebar.is_open());
}


/// Where cell (`col`, `row`) of the terminal on show is in the window.
fn cell_point(rt: &Runtime<Lemur>, col: u16, row: u16) -> (f32, f32) {
    let (r, _) = rt.presented("screen-main").expect("the terminal on show");
    let m = rt.state.metrics;
    let pad = super::terminal::PAD;
    (r[0] + pad + (f32::from(col) + 0.5) * m.cell_w, r[1] + pad + (f32::from(row) + 0.5) * m.cell_h)
}

/// A shell thread printing `script`'s output, attached and shown, once the output shows.
fn printed(tag: &str, script: &str, shows: &str) -> (HcServer, Runtime<Lemur>, Key) {
    let server = HcServer::new(tag);
    let t = server.run("/", &[], &["sh", "-c", script]);
    let mut rt = loaded(&server);
    open(&mut rt, &t);
    let k = key(&t);
    assert!(
        wait_for(&mut rt, 10.0, |s| super::terminal::lines(s, &k).iter().any(|l| l.contains(shows))),
        "printed: {:?}",
        screen(&rt, &k)
    );
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    (server, rt, k)
}

fn row_of(rt: &Runtime<Lemur>, k: &Key, text: &str) -> u16 {
    screen(rt, k).iter().position(|l| l.contains(text)).expect("the row") as u16
}

/// Dragging across text selects it, and ⌘C copies it; typing drops the selection.
#[test]
fn dragging_selects_and_cmd_c_copies() {
    let (_server, mut rt, k) = printed("rsel", "echo 'alpha beta gamma'; exec sleep 600", "alpha beta");
    let row = row_of(&rt, &k, "alpha beta");
    let (x0, y) = cell_point(&rt, 6, row);
    // Into the right half of the last cell, which takes it.
    let (x1, _) = cell_point(&rt, 9, row);
    let x1 = x1 + rt.state.metrics.cell_w * 0.4;
    rt.pointer_move(x0, y);
    rt.pointer_down(x0, y);
    for i in 1..=6 {
        rt.pointer_move(x0 + (x1 - x0) * i as f32 / 6.0, y);
        rt.frame(1.0 / 60.0);
    }
    rt.pointer_up(x1, y);
    rt.frame(1.0 / 60.0);
    let selected = |rt: &Runtime<Lemur>| rt.state.terminals[&k].grid.as_ref().unwrap().selected_text();
    assert_eq!(selected(&rt).as_deref(), Some("beta"));
    let mut copied = false;
    rt.update(|s, tx| copied = s.copy_selection(tx));
    assert!(copied);
    // A double click selects the word under it.
    let (xg, _) = cell_point(&rt, 13, row);
    for _ in 0..2 {
        rt.pointer_down(xg, y);
        rt.pointer_up(xg, y);
        rt.frame(0.05);
    }
    assert_eq!(selected(&rt).as_deref(), Some("gamma"));
}

/// A program that asks for the mouse (here SGR reports) gets clicks, not a selection.
#[test]
fn clicks_reach_a_program_that_asks() {
    let script = "printf '\\033[?1000h\\033[?1006hready\\n'; stty -echo -icanon; exec od -c";
    let (_server, mut rt, k) = printed("rmouse", script, "ready");
    assert!(rt.state.terminals[&k].grid.as_ref().unwrap().mouse_tracking());
    let row = row_of(&rt, &k, "ready");
    let (x, y) = cell_point(&rt, 2, row + 3);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    // od prints what it read: the report starts ESC [ <.
    assert!(
        wait_for(&mut rt, 10.0, |s| super::terminal::lines(s, &k).iter().any(|l| l.contains("033   [   <   0"))),
        "a report: {:?}",
        screen(&rt, &k)
    );
    assert!(rt.state.terminals[&k].grid.as_ref().unwrap().selected_text().is_none(), "nothing selected");
}

/// The wheel goes back through the scrollback, and typing comes back to the bottom.
#[test]
fn the_wheel_scrolls_back() {
    let (_server, mut rt, k) = printed("rwheel", "seq 1 200; exec sleep 600", "200");
    let bottom = screen(&rt, &k);
    let (x, y) = cell_point(&rt, 4, 4);
    rt.wheel_xy(x, y, 0.0, 400.0);
    for _ in 0..4 {
        rt.frame(1.0 / 60.0);
    }
    let up = screen(&rt, &k);
    assert_ne!(up, bottom, "it scrolled");
    assert!(!up.iter().any(|l| l == "200"), "the last line is out of view: {up:?}");
}

/// Presses on `key`, drags it `dy` points down in steps, and lets go.
fn drag(rt: &mut Runtime<Lemur>, key: &str, dy: f32) {
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element keyed {key}"));
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    for step in 1..=10 {
        rt.pointer_move(x, y + dy * step as f32 / 10.0);
        rt.frame(1.0 / 60.0);
    }
    rt.pointer_up(x, y + dy);
    for _ in 0..4 {
        rt.frame(1.0 / 60.0);
    }
}

/// A thread's row dragged down the sidebar past two others lands after them.
#[test]
fn dragging_a_row_reorders_its_list() {
    let server = HcServer::new("rdrag");
    let mut rt = window(server.config());
    let idle = Activity::Idle;
    apply(&mut rt, &[("a", idle), ("b", idle), ("c", idle)]);
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    let (_, a) = rt.find("row-local-a").unwrap();
    let (_, b) = rt.find("row-local-b").unwrap();
    drag(&mut rt, "row-local-a", 2.0 * (b - a));
    let shown: Vec<String> = rt.state.snap.hosts[0].other.iter().map(|t| t.name.clone()).collect();
    assert_eq!(shown, ["b", "c", "a"]);
    assert!(rt.state.row_drag.is_none(), "the drag is over");
    // A press that doesn't move is a click.
    click(&mut rt, "row-local-c");
    assert_eq!(rt.state.active, Some(key("c")));
}

/// A worktree's row dragged under the next one swaps with it, and the project keeps
/// that order.
#[test]
fn dragging_a_worktree_reorders_its_project() {
    use crate::config::Base;
    use crate::snapshot::{BaseView, Worktree};
    let server = HcServer::new("rwdrag");
    let mut rt = window(server.config());
    let base: Base = toml::from_str("host = \"local\"\npath = \"/repo\"").unwrap();
    rt.state.cfg.bases = vec![base];
    let worktree = |path: &str, main: bool| Worktree {
        path: path.into(),
        branch: Some(path.trim_start_matches('/').replace('/', "-")),
        head: None,
        main,
        sessions: vec![],
        gone: vec![],
    };
    let mut snap = made_up(&[]);
    snap.hosts[0].bases = vec![BaseView {
        index: 0,
        error: None,
        worktrees: ["/repo", "/repo/one", "/repo/two", "/repo/three"].into_iter().map(|p| worktree(p, p == "/repo")).collect(),
        plain: false,
    }];
    rt.update(|s, _| s.set_snapshot(snap));
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    let (_, one) = rt.find("wt-/repo/one").expect("a row for /repo/one");
    let (_, two) = rt.find("wt-/repo/two").expect("a row for /repo/two");
    drag(&mut rt, "wt-/repo/one", two - one + 4.0);
    let shown: Vec<String> = rt.state.snap.hosts[0].bases[0].worktrees.iter().map(|w| w.path.clone()).collect();
    assert_eq!(shown, ["/repo", "/repo/two", "/repo/one", "/repo/three"]);
    assert_eq!(rt.state.cfg.bases[0].worktree_order, shown, "kept");
    assert!(rt.state.moving.is_none(), "the drag is over");
}

/// A project dragged by its header under the next one swaps with it.
#[test]
fn dragging_a_project_by_its_header_reorders_the_projects() {
    use crate::config::Base;
    use crate::snapshot::{BaseView, Worktree};
    let server = HcServer::new("rpdrag");
    let mut rt = window(server.config());
    let base = |path: &str| -> Base { toml::from_str(&format!("host = \"local\"\npath = \"{path}\"")).unwrap() };
    rt.state.cfg.bases = vec![base("/first"), base("/second"), base("/third")];
    let view = |index: usize, path: &str| BaseView {
        index,
        error: None,
        worktrees: vec![Worktree { path: path.into(), branch: Some("main".into()), head: None, main: true, sessions: vec![], gone: vec![] }],
        plain: false,
    };
    let mut snap = made_up(&[]);
    snap.hosts[0].bases = vec![view(0, "/first"), view(1, "/second"), view(2, "/third")];
    rt.update(|s, _| s.set_snapshot(snap));
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    let header = |rt: &Runtime<Lemur>, i: usize| rt.state.places[&format!("project-{i}")].0;
    let dy = header(&rt, 1) - header(&rt, 0) + 4.0;
    drag(&mut rt, "project-0/section-header", dy);
    let names: Vec<&str> = rt.state.cfg.bases.iter().map(|b| b.path.as_str()).collect();
    assert_eq!(names, ["/second", "/first", "/third"]);
    // A press that doesn't move still folds it.
    let folded = rt.state.cfg.bases[0].collapsed;
    click(&mut rt, "project-0/section-header");
    assert_ne!(rt.state.cfg.bases[0].collapsed, folded);
}

/// A click outside a peek closes it; one on it doesn't.
#[test]
fn a_click_outside_a_peek_closes_it() {
    let server = HcServer::new("rpeek");
    let mut rt = window(server.config());
    let idle = Activity::Idle;
    apply(&mut rt, &[("a", idle), ("b", idle)]);
    open(&mut rt, "a");
    rt.update(|s, _| s.peek(key("b")));
    click(&mut rt, "peek-header");
    assert_eq!(rt.state.peek, Some(key("b")), "a click on it");
    let (_, y) = rt.find("peek-header").unwrap();
    let (x, _) = rt.find("peek").unwrap();
    // Above the card, over the window's tabs.
    rt.pointer_move(x, y - 40.0);
    rt.pointer_down(x, y - 40.0);
    rt.pointer_up(x, y - 40.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.peek, None, "a click outside");
}

/// A click on a button leaves the keyboard with the terminal: Space doesn't press it again.
#[test]
fn a_click_on_a_button_leaves_the_keyboard_with_the_terminal() {
    let server = HcServer::new("rbutton");
    let t = server.run("/", &[], &["sh"]);
    let mut rt = loaded(&server);
    open(&mut rt, &t);
    assert!(wait_for(&mut rt, 10.0, |s| s.terminals.get(&key(&t)).is_some_and(|v| v.painted)), "painted");
    click(&mut rt, "toggle-links");
    assert_eq!(rt.state.right, Some(Right::Links));
    type_text(&mut rt, "echo a b");
    press(&mut rt, K::Enter, Mods::default());
    assert_eq!(rt.state.right, Some(Right::Links), "Space didn't press it");
    let k = key(&t);
    assert!(
        wait_for(&mut rt, 10.0, |s| super::terminal::lines(s, &k).iter().any(|l| l == "a b")),
        "the shell's answer: {:?}",
        screen(&rt, &k)
    );
}

/// What's typed into a field beside the terminal (the notes) is the field's, not the
/// terminal's, from the first key.
#[test]
fn a_field_beside_the_terminal_keeps_what_is_typed() {
    let server = HcServer::new("rfield");
    let t = server.run("/", &[], &["sh"]);
    let mut rt = loaded(&server);
    open(&mut rt, &t);
    assert!(wait_for(&mut rt, 10.0, |s| s.terminals.get(&key(&t)).is_some_and(|v| v.painted)), "painted");
    let dir = crate::test_support::TempDir::new("rfield-notes");
    rt.update(|s, _| {
        s.panels.notes = super::notes::Notes::new(Some(&dir));
        s.open_notes("local", "/", "root");
    });
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    let (x, y) = rt.find("notes-editor").expect("the editor");
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    // At once, before a frame has passed.
    rt.key(KeyEvent { key: K::Text("z".into()), mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    type_text(&mut rt, "q w");
    press(&mut rt, K::Backspace, Mods::default());
    assert_eq!(rt.state.panels.notes.text(), "zq ");
    std::thread::sleep(Duration::from_millis(300));
    let _ = wait_for(&mut rt, 0.5, |_| false);
    assert!(!screen(&rt, &key(&t)).iter().any(|l| l.contains('z') || l.contains('q')), "{:?}", screen(&rt, &key(&t)));
}

/// An attach no terminal took, as when the thread was closed while it attached, lets go of
/// the terminal: the core doesn't go on streaming to it.
#[test]
fn an_attach_no_terminal_took_lets_go() {
    let server = HcServer::new("rorphan");
    let t = server.run("/", &[], &["sleep", "600"]);
    let core = server.core();
    let mut rt = loaded(&server);
    rt.update(|s, _| {
        s.open("local".into(), t.clone());
        s.terminals.remove(&key(&t));
    });
    // Once its attach is in, there's nothing to stream to.
    let _ = wait_for(&mut rt, 2.0, |_| false);
    assert!(wait_until(Duration::from_secs(5), || core.streams() == 0), "streams: {}", core.streams());
    // One taken streams on.
    open(&mut rt, &t);
    assert!(wait_for(&mut rt, 10.0, |s| s.terminals.get(&key(&t)).is_some_and(|v| v.is_attached())), "attached");
    assert_eq!(core.streams(), 1);
}

/// Notes opened from a worktree's menu come back with ⌘R after it hid them.
#[test]
fn notes_opened_from_a_menu_come_back_with_cmd_r() {
    let server = HcServer::new("rnotesr");
    let mut rt = window(server.config());
    rt.update(|s, _| s.toggle_right(Right::Links));
    rt.update(|s, _| s.open_notes("local", "/", "root"));
    press(&mut rt, K::Command('r'), cmd());
    assert_eq!(rt.state.right, None);
    press(&mut rt, K::Command('r'), cmd());
    assert_eq!(rt.state.right, Some(Right::Notes));
}
