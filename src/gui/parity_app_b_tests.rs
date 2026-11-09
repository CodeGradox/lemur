//! v2's window tests that v3 had no test of the same name for (the second half of v2's
//! `gui/mod.rs` tests), ported to Rune's `Runtime`, against herdr-cores of their own
//! (`HcServer`) or snapshots made up here.

use super::core::wait_for;
use super::snackbar::Kind;
use super::tests::{apply, cmd, key, made_up, open, press, refreshing, type_text, window};
use super::{Action, Key, Lemur, actions, sessions};
use crate::config::{Agent, Base, ClaudeThread, Config, Host, ThreadOrder};
use crate::snapshot::{self, Activity, BaseView, HostView, Session, Snapshot, Worktree};
use crate::test_support::{HcServer, TempDir, git_in, wait_until};
use rune::{ElementInfo, Key as K, KeyEvent, Mods, Runtime};
use std::path::Path;
use std::time::Duration;

/// The window on `cfg`, once its first snapshot is in.
fn loaded(cfg: Config) -> Runtime<Lemur> {
    let mut rt = window(cfg);
    rt.state.refresh();
    assert!(wait_for(&mut rt, 10.0, |s| s.loaded), "the first snapshot");
    rt
}

fn frames(rt: &mut Runtime<Lemur>, n: usize) {
    for _ in 0..n {
        rt.frame(1.0 / 60.0);
    }
}

/// Runs the window for `secs`, whatever happens.
fn idle(rt: &mut Runtime<Lemur>, secs: f64) {
    let _ = wait_for(rt, secs, |_| false);
}

fn click(rt: &mut Runtime<Lemur>, key: &str) {
    // What just came in fades in: a press lands once it shows.
    frames(rt, 20);
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element keyed {key}"));
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    frames(rt, 4);
}

fn info(rt: &Runtime<Lemur>, key: &str) -> Option<ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting)
}

/// Presses the × inside the element keyed `outer` (a tab, a sidebar row), with the pointer
/// over it first, as the × shows only then.
fn press_close_in(rt: &mut Runtime<Lemur>, outer: &str) {
    frames(rt, 20);
    let (r, _) = rt.presented(outer).unwrap_or_else(|| panic!("no element keyed {outer}"));
    rt.pointer_move(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    frames(rt, 10);
    let inside = |e: &ElementInfo| {
        let (x, y) = (e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
        x >= r[0] && x <= r[0] + r[2] && y >= r[1] && y <= r[1] + r[3]
    };
    let close = rt
        .elements()
        .into_iter()
        .filter(|e| !e.exiting && e.key.as_deref() == Some("close"))
        .find(inside)
        .unwrap_or_else(|| panic!("no × in {outer}"));
    let (x, y) = (close.rect[0] + close.rect[2] / 2.0, close.rect[1] + close.rect[3] / 2.0);
    rt.pointer_move(x, y);
    frames(rt, 10);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    frames(rt, 4);
}

fn ctrl() -> Mods {
    Mods { ctrl: true, ..Mods::default() }
}

fn screen(rt: &Runtime<Lemur>, k: &Key) -> Vec<String> {
    super::terminal::lines(&rt.state, k)
}

/// The core's terminal at `address`, if it runs.
fn terminal(server: &HcServer, address: &str) -> Option<crate::hc::Terminal> {
    server.core().terminals().into_iter().find(|t| t.address == address)
}

fn labelled(server: &HcServer, address: &str, label: &str) -> bool {
    terminal(server, address).is_some_and(|t| t.labels.contains_key(label))
}

/// A repository at `path`, with one commit.
fn repo_at(path: &Path) -> String {
    std::fs::create_dir(path).unwrap();
    git_in(path, &["init", "-q", "-b", "main"]);
    git_in(path, &["commit", "-q", "--allow-empty", "-m", "init"]);
    path.to_string_lossy().into_owned()
}

/// Waits for herdr-core to say whether something other than its shell runs in `k`.
fn busy(rt: &mut Runtime<Lemur>, k: &Key, busy: bool) -> bool {
    refreshing(rt, 10.0, |s| s.session_of(k).is_some_and(|t| t.busy == busy))
}

/// Kills the program of the terminal at `address`, by its pid.
fn kill_program(server: &HcServer, address: &str) {
    let pid = terminal(server, address).and_then(|t| t.pid).expect("its program's pid");
    let killed = std::process::Command::new("kill").args(["-KILL", &pid.to_string()]).status().unwrap();
    assert!(killed.success());
}

/// One project, `/srv/api`, on `local`: its main checkout with `main-shell` in it, and
/// worktree `auth` with `threads`.
fn project_snapshot(threads: &[&str], machine: crate::machine::Reading) -> Snapshot {
    let sessions = |names: &[&str]| -> Vec<Session> {
        let pairs: Vec<_> = names.iter().map(|n| (*n, Activity::Idle)).collect();
        made_up(&pairs).hosts.remove(0).other
    };
    let worktree = |path: &str, branch: &str, main: bool, threads: &[&str]| Worktree {
        path: path.into(),
        branch: Some(branch.into()),
        head: None,
        main,
        sessions: sessions(threads),
        gone: vec![],
    };
    Snapshot {
        hosts: vec![HostView {
            name: "local".into(),
            error: None,
            needs_you: false,
            home: String::new(),
            machine,
            bases: vec![BaseView {
                index: 0,
                error: None,
                plain: false,
                worktrees: vec![
                    worktree("/srv/api", "main", true, &["main-shell"]),
                    worktree("/srv/api/.claude/worktrees/auth", "feature/auth", false, threads),
                ],
            }],
            other: vec![],
            gone: vec![],
        }],
    }
}

/// ⌘1 to ⌘9 show what the tabs at those places show when clicked, from the terminal with
/// the keyboard. With no tab at the place, or the palette up, nothing changes.
#[test]
fn cmd_digits_show_the_tabs_by_their_place() {
    let server = HcServer::new("pdigits");
    let mut rt = window(server.config());
    // Wide enough to show all nine, to click.
    rt.set_viewport((2400.0, 820.0), 2.0);
    let names: Vec<String> = (0..9).map(|i| format!("t{i}")).collect();
    let listed: Vec<(&str, Activity)> = names.iter().map(|n| (n.as_str(), Activity::Idle)).collect();
    apply(&mut rt, &listed);
    open(&mut rt, "t4");
    frames(&mut rt, 20);
    let shown = |rt: &Runtime<Lemur>| rt.state.active.clone().map(|(_, s)| s).unwrap();
    // The tabs from the left, by what a click on each shows.
    let mut along: Vec<(f32, String)> =
        names.iter().map(|n| (rt.find(&format!("t-{n}")).unwrap_or_else(|| panic!("{n}'s tab")).0, n.clone())).collect();
    along.sort_by(|a, b| a.0.total_cmp(&b.0));
    let tabs: Vec<String> = along
        .iter()
        .map(|(_, n)| {
            click(&mut rt, &format!("t-{n}"));
            shown(&rt)
        })
        .collect();
    for n in &names {
        assert!(tabs.contains(n), "{n} isn't on a tab: {tabs:?}");
    }

    for i in [8, 0, 3, 7, 1, 6, 2, 5, 4] {
        press(&mut rt, K::Command(char::from(b'1' + i as u8)), cmd());
        assert_eq!(shown(&rt), tabs[i], "⌘{}", i + 1);
    }

    press(&mut rt, K::Command('p'), Mods { shift: true, ..cmd() });
    assert!(rt.state.panels.commands.is_some(), "the palette");
    press(&mut rt, K::Command('1'), cmd());
    assert_eq!(shown(&rt), tabs[4], "the palette is up");
    press(&mut rt, K::Escape, Mods::default());
    assert!(rt.state.panels.commands.is_none());

    let fewer: Vec<(&str, Activity)> = listed.iter().copied().filter(|(n, _)| *n != tabs[8]).collect();
    apply(&mut rt, &fewer);
    frames(&mut rt, 20);
    assert!(rt.find(&format!("t-{}", tabs[8])).is_none());
    press(&mut rt, K::Command('9'), cmd());
    assert_eq!(shown(&rt), tabs[4], "no ninth tab now");
}

/// The first nine tabs show the number ⌘ goes to them with; a tenth has no key, so no
/// number. (v3 says it on hover, beside the name.)
#[test]
fn the_tabs_show_their_numbers() {
    let server = HcServer::new("pnumbers");
    let mut rt = window(server.config());
    rt.set_viewport((2400.0, 820.0), 2.0);
    let names: Vec<String> = (0..10).map(|i| format!("t{i}")).collect();
    let listed: Vec<(&str, Activity)> = names.iter().map(|n| (n.as_str(), Activity::Idle)).collect();
    apply(&mut rt, &listed);
    open(&mut rt, "t0");
    frames(&mut rt, 20);
    let helps: Vec<String> = rt.elements().into_iter().filter(|e| !e.exiting).filter_map(|e| e.help).collect();
    for (i, n) in names.iter().enumerate().take(9) {
        let want = format!("{n}  ⌘{}", i + 1);
        assert!(helps.contains(&want), "tab {i} says {want:?}: {helps:?}");
    }
    assert!(rt.find("t-t9").is_some(), "a tenth tab");
    assert!(helps.contains(&"t9".to_string()), "named: {helps:?}");
    assert!(!helps.iter().any(|h| h.starts_with("t9  ⌘")), "with no number: {helps:?}");
}

/// Whether the tab of `name` is wholly in view in the tab bar.
fn tab_in_view(rt: &mut Runtime<Lemur>, name: &str) -> bool {
    frames(rt, 40);
    let e = info(rt, &format!("t-{name}")).unwrap_or_else(|| panic!("{name}'s tab"));
    e.rect[2] > 0.0 && e.visible[2] >= e.rect[2] - 1.0
}

/// With more tabs than fit, the one on show scrolls into view: by key, opened some other
/// way, new, or in a narrower window. Scrolled by hand it can leave, and stays out of view
/// until a thread is shown again.
#[test]
fn the_tab_on_show_stays_in_view() {
    let server = HcServer::new("pinview");
    let mut rt = window(server.config());
    // Room for a whole tab between the scroll arrows.
    rt.set_viewport((1000.0, 600.0), 2.0);
    let mut names: Vec<String> = (0..16).map(|i| format!("t{i}")).collect();
    let listed: Vec<(&str, Activity)> = names.iter().map(|n| (n.as_str(), Activity::Idle)).collect();
    apply(&mut rt, &listed);
    open(&mut rt, "t0");
    assert!(tab_in_view(&mut rt, "t0"), "the first tab");
    assert!(!tab_in_view(&mut rt, "t15"), "more tabs than fit");
    let mut out = vec![];
    let mut check = |rt: &mut Runtime<Lemur>, name: &str, how: &str| {
        if !tab_in_view(rt, name) {
            out.push(format!("{how}: {name}"));
        }
    };

    press(&mut rt, K::Command('9'), cmd());
    assert_eq!(rt.state.active, Some(key("t8")));
    check(&mut rt, "t8", "by key");
    press(&mut rt, K::Command('1'), cmd());
    check(&mut rt, "t0", "by key, back");
    press(&mut rt, K::Tab, Mods { shift: true, ..ctrl() });
    rt.key(KeyEvent { key: K::Modifier { modifier: rune::Modifier::ControlLeft, pressed: false }, mods: Mods::default() });
    assert_eq!(rt.state.active, Some(key("t15")));
    check(&mut rt, "t15", "the previous thread, round the end");
    press(&mut rt, K::Command('1'), cmd());
    check(&mut rt, "t0", "by key, back again");
    open(&mut rt, "t15");
    check(&mut rt, "t15", "opened some other way");

    // Scrolled by hand away from the tab on show, the first.
    press(&mut rt, K::Command('1'), cmd());
    if tab_in_view(&mut rt, "t0") {
        let (x, y) = rt.find("t-t0").expect("its tab");
        rt.wheel_xy(x, y, 400.0, 0.0);
        if tab_in_view(&mut rt, "t0") {
            rt.wheel_xy(x, y, -400.0, 0.0);
        }
        assert!(!tab_in_view(&mut rt, "t0"), "scrolled away by hand");
        rt.update(|_, _| {});
        assert!(!tab_in_view(&mut rt, "t0"), "and left there");
    }
    press(&mut rt, K::Command('1'), cmd());
    check(&mut rt, "t0", "shown again, it snaps back");

    rt.set_viewport((1100.0, 600.0), 2.0);
    open(&mut rt, "t15");
    check(&mut rt, "t15", "opened again");
    rt.set_viewport((950.0, 600.0), 2.0);
    check(&mut rt, "t15", "in a narrower window");

    names.push("t16".into());
    let listed: Vec<(&str, Activity)> = names.iter().map(|n| (n.as_str(), Activity::Idle)).collect();
    apply(&mut rt, &listed);
    open(&mut rt, "t16");
    check(&mut rt, "t16", "a new tab");
    assert!(out.is_empty(), "the tab on show out of view: {out:?}");
}

/// With more tabs than fit, ‹ and › stand at the tab bar's ends, so it's plain there are
/// more: › scrolls it a tab on, ‹ a tab back. With them all in view, neither shows.
#[test]
fn arrows_show_there_are_more_tabs() {
    let server = HcServer::new("tabarrows");
    let mut rt = window(server.config());
    rt.set_viewport((1280.0, 800.0), 2.0);
    let arrows = |rt: &Runtime<Lemur>| (info(rt, "tabs-back").is_some(), info(rt, "tabs-forward").is_some());
    apply(&mut rt, &[("t0", Activity::Idle), ("t1", Activity::Idle)]);
    open(&mut rt, "t0");
    frames(&mut rt, 40);
    assert_eq!(arrows(&rt), (false, false), "two tabs fit");

    // Three don't, as the window's other buttons leave the tabs about 500 points.
    let names: Vec<String> = (0..12).map(|i| format!("t{i}")).collect();
    let listed: Vec<(&str, Activity)> = names.iter().map(|n| (n.as_str(), Activity::Idle)).collect();
    apply(&mut rt, &listed);
    frames(&mut rt, 40);
    assert_eq!(arrows(&rt), (true, true), "twelve don't");
    let in_view: Vec<bool> = names.iter().map(|n| tab_in_view(&mut rt, n)).collect();
    let next = in_view.iter().position(|v| !v).expect("a tab out of view");
    assert!(in_view[..next].iter().all(|v| *v), "the first ones in view: {in_view:?}");

    // At the start, ‹ has nowhere to go.
    click(&mut rt, "tabs-back");
    assert!(tab_in_view(&mut rt, "t0"), "still at the start");
    click(&mut rt, "tabs-forward");
    assert!(tab_in_view(&mut rt, &names[next]), "› brings the next tab in");
    assert!(!tab_in_view(&mut rt, "t0"), "and the first goes");
    click(&mut rt, "tabs-forward");
    assert!(tab_in_view(&mut rt, &names[next + 1]), "and the one after");
    click(&mut rt, "tabs-back");
    click(&mut rt, "tabs-back");
    assert!(tab_in_view(&mut rt, "t0"), "‹ goes back to the first");
    // Pressed quickly, before the bar is where the last press sent it: a tab a press still.
    for _ in 0..3 {
        let (x, y) = rt.find("tabs-forward").expect("›");
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        frames(&mut rt, 3);
    }
    assert!(tab_in_view(&mut rt, &names[next + 2]), "three quick presses, three tabs on");
    assert_eq!(rt.state.active, Some(key("t0")), "scrolling shows no other thread");

    // A window wide enough for them all has no arrows.
    rt.set_viewport((3000.0, 600.0), 2.0);
    frames(&mut rt, 40);
    assert_eq!(arrows(&rt), (false, false), "all fit again");
}

/// The status bar doesn't count the agents; it says how many need you, when any do.
#[test]
fn the_status_bar_says_how_many_need_you() {
    let server = HcServer::new("pneeds");
    let mut rt = window(server.config());
    apply(&mut rt, &[("a", Activity::Idle), ("b", Activity::Working)]);
    open(&mut rt, "a");
    frames(&mut rt, 20);
    assert!(rt.find("needs-you").is_none(), "none do");
    apply(&mut rt, &[("a", Activity::Idle), ("b", Activity::NeedsInput)]);
    frames(&mut rt, 20);
    assert_eq!(info(&rt, "needs-you").and_then(|e| e.text).as_deref(), Some("1 needs you"), "one does");
}

/// A tab is as narrow with a long name as with a short one: hovered it says the name, and
/// the terminal's header names the thread on show.
#[test]
fn a_threads_name_is_on_hover_and_in_the_header() {
    let server = HcServer::new("pnames");
    let long = "a-name-long-enough-to-make-a-titled-tab-wide";
    let mut rt = window(server.config());
    rt.set_viewport((1200.0, 700.0), 2.0);
    let mut snap = made_up(&[("a", Activity::Idle), ("b", Activity::Idle)]);
    snap.hosts[0].other[0].label = "ab".into();
    snap.hosts[0].other[1].label = long.into();
    rt.update(|s, _| s.set_snapshot(snap));
    open(&mut rt, "a");
    frames(&mut rt, 30);
    let width = |rt: &Runtime<Lemur>, k: &str| rt.presented(k).unwrap_or_else(|| panic!("{k}")).0[2];
    let (short, wide) = (width(&rt, "t-a"), width(&rt, "t-b"));
    assert!((wide - short).abs() < 4.0, "{short} {wide}");
    let header = |rt: &Runtime<Lemur>| info(rt, "header-name").and_then(|e| e.text);
    assert_eq!(header(&rt).as_deref(), Some("ab"));
    let helps: Vec<String> = rt.elements().into_iter().filter(|e| !e.exiting).filter_map(|e| e.help).collect();
    assert!(helps.iter().any(|h| h.starts_with(long)), "on hover: {helps:?}");
    click(&mut rt, "t-b");
    assert_eq!(rt.state.active, Some(key("b")));
    frames(&mut rt, 20);
    assert_eq!(header(&rt).as_deref(), Some(long));
}

/// Beside the branch, the status bar counts the lines added and removed in the worktree on
/// show since its last commit; nothing while it has none.
#[test]
fn the_status_bar_counts_the_worktrees_changes() {
    let server = HcServer::new("pcounted");
    let dir = TempDir::new("pcounted");
    let repo = dir.join("api");
    std::fs::create_dir(&repo).unwrap();
    git_in(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
    git_in(&repo, &["add", "a.txt"]);
    git_in(&repo, &["commit", "-q", "-m", "init"]);
    git_in(&repo, &["worktree", "add", "-q", "-b", "auth", "wt/auth"]);
    let wt = repo.join("wt/auth");
    std::fs::write(wt.join("a.txt"), "one\n2\nthree\n").unwrap();
    let clean = server.run(&repo.to_string_lossy(), &["lemur.label=clean"], &["sleep", "600"]);
    let changed = server.run(&wt.to_string_lossy(), &["lemur.label=changed"], &["sleep", "600"]);
    let mut cfg = server.config();
    cfg.bases.push(Base::new("local", &repo.to_string_lossy()));
    let mut rt = loaded(cfg);
    rt.set_viewport((1200.0, 700.0), 2.0);
    let listed = refreshing(&mut rt, 10.0, |s| s.project_of("local", &changed).is_some() && s.project_of("local", &clean).is_some());
    assert!(listed);
    let counted = |rt: &mut Runtime<Lemur>| {
        assert!(wait_for(rt, 10.0, |s| s.diffs.asking.is_empty()));
        frames(rt, 20);
        let texts: Vec<String> = rt.elements().into_iter().filter(|e| !e.exiting).filter_map(|e| e.text).collect();
        rt.find("changes").map(|_| (texts.contains(&"+2".to_string()), texts.contains(&"-1".to_string())))
    };
    open(&mut rt, &changed);
    assert_eq!(counted(&mut rt), Some((true, true)), "+2 -1");
    open(&mut rt, &clean);
    assert_eq!(counted(&mut rt), None, "the main checkout has none");
}

/// A program that asks for focus reports hears it lost focus when another thread shows
/// instead of it, and got it back when it shows again.
#[test]
fn a_thread_shown_no_more_hears_it_lost_focus() {
    let server = HcServer::new("pfocus");
    let a = server.run("/", &["lemur.label=a"], &["sh", "-c", r"printf '\033[?1004h'; exec cat -v"]);
    let b = server.run("/", &["lemur.label=b"], &["sleep", "600"]);
    let mut rt = loaded(server.config());
    let heard = |rt: &mut Runtime<Lemur>, what: &str| {
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            if server.hc(&["read", &a]).contains(what) {
                return true;
            }
            idle(rt, 0.2);
        }
        false
    };
    open(&mut rt, &a);
    assert!(heard(&mut rt, "^[[I"), "{}", server.hc(&["read", &a]));
    open(&mut rt, &b);
    assert!(heard(&mut rt, "^[[O"), "{}", server.hc(&["read", &a]));
    open(&mut rt, &a);
    let ins = |text: String| text.matches("^[[I").count();
    let start = std::time::Instant::now();
    while ins(server.hc(&["read", &a])) < 2 && start.elapsed() < Duration::from_secs(10) {
        idle(&mut rt, 0.2);
    }
    assert_eq!(ins(server.hc(&["read", &a])), 2, "back: {}", server.hc(&["read", &a]));
}

/// Along the tabs, as at first, closing the thread on show, with ⌘W or its ×, shows the
/// next tab, or the one before if it was the last. By when they were opened, the next tab
/// wouldn't be the one in either case.
#[test]
fn closing_a_thread_shows_the_next_tab() {
    let server = HcServer::new("pclosetab");
    let [a, b, c, d] = ["a", "b", "c", "d"].map(|l| server.run("/", &[&format!("lemur.label={l}")], &["sleep", "600"]));
    let mut rt = loaded(server.config());
    rt.set_viewport((1200.0, 700.0), 2.0);
    for t in [&c, &b, &a, &d] {
        open(&mut rt, t);
    }
    assert!(busy(&mut rt, &key(&d), true), "d runs sleep");

    // d is the last tab; by when they were opened, a.
    press(&mut rt, K::Command('w'), cmd());
    assert!(rt.state.dialog.is_some(), "asked");
    click(&mut rt, "dialog-ok");
    assert!(refreshing(&mut rt, 10.0, |s| s.active == Some(key(&c))), "{:?}", rt.state.active);

    // Tabs a, b and c, on b; by when they were opened, a.
    open(&mut rt, &a);
    open(&mut rt, &b);
    assert!(busy(&mut rt, &key(&b), true));
    press_close_in(&mut rt, &format!("t-{b}"));
    assert!(rt.state.dialog.is_some(), "its × asks");
    click(&mut rt, "dialog-ok");
    assert!(refreshing(&mut rt, 10.0, |s| s.active == Some(key(&c))), "{:?}", rt.state.active);
}

/// Whatever the order set, a thread put away leaves the one opened before it on show, not
/// the next tab.
#[test]
fn putting_a_thread_away_shows_the_one_opened_before() {
    let server = HcServer::new("pawayord");
    let [a, b, c] = ["a", "b", "c"].map(|l| server.run("/", &[&format!("lemur.label={l}")], &["sleep", "600"]));
    let mut rt = loaded(server.config());
    for t in [&c, &a, &b] {
        open(&mut rt, t);
    }
    rt.update(|s, tx| actions::run(s, Action::PutAway, tx));
    assert_eq!(rt.state.active, Some(key(&a)));
}

/// ⌘W asks before closing the thread on show. By when they were opened, closed or ended any
/// other way, the one opened before it in its worktree shows instead, else its neighbour
/// there.
#[test]
fn closing_a_thread_shows_another() {
    let server = HcServer::new("pcloseoth");
    let [a, b, c] = ["a", "b", "c"].map(|l| server.run("/", &[&format!("lemur.label={l}")], &["sleep", "600"]));
    let mut rt = loaded(server.config());
    rt.state.prefs.thread_order = ThreadOrder::Recent;
    for t in [&c, &a, &b] {
        open(&mut rt, t);
    }
    assert!(busy(&mut rt, &key(&b), true));

    press(&mut rt, K::Command('w'), cmd());
    assert!(rt.state.dialog.is_some(), "asked");
    press(&mut rt, K::Escape, Mods::default());
    assert!(rt.state.dialog.is_none());
    idle(&mut rt, 0.5);
    assert!(server.addresses().contains(&b), "cancelled");
    press(&mut rt, K::Command('w'), cmd());
    click(&mut rt, "dialog-ok");
    assert!(wait_until(Duration::from_secs(5), || !server.addresses().contains(&b)));
    assert!(refreshing(&mut rt, 10.0, |s| s.session_of(&key(&b)).is_none()));
    assert_eq!(rt.state.active, Some(key(&a)), "the one opened before");
    // It has the keyboard: ⌘J works at once.
    assert!(rt.state.keyboard_on_terminal(), "the terminal shown instead has the keyboard");
    press(&mut rt, K::Command('j'), cmd());
    assert!(rt.state.prompt.is_some(), "⌘J answered");
    press(&mut rt, K::Escape, Mods::default());
    assert!(rt.state.prompt.is_none());

    // Ended elsewhere, with none opened before it: its neighbour.
    rt.state.recent.retain(|k| *k == key(&a));
    server.hc(&["keep", &a, "--off"]);
    server.hc(&["kill", &a]);
    assert!(wait_until(Duration::from_secs(5), || !server.addresses().contains(&a)));
    assert!(refreshing(&mut rt, 10.0, |s| s.session_of(&key(&a)).is_none()));
    assert_eq!(rt.state.active, Some(key(&c)));
    assert!(rt.state.keyboard_on_terminal(), "the neighbour has the keyboard");
}

/// Codex shows as Codex: started for it, or found reporting as it (as it does on its own,
/// to herdr-core) with its status, whatever it was started for. A plain terminal isn't an
/// agent.
#[test]
fn codex_threads_show_as_codex() {
    let server = HcServer::new("pcodex");
    let report = |state: &str| format!(r#"printf '\033]7501;state={state}:app=codex\033\\'; exec sleep 600"#);
    let started = server.run("/", &["lemur.kind=codex"], &["sleep", "600"]);
    let by_hand = server.run("/", &[], &["sh", "-c", &report("working")]);
    let asks = server.run("/", &["lemur.kind=claude"], &["sh", "-c", &report("blocked")]);
    let plain = server.run("/", &[], &["sleep", "600"]);
    let mut rt = loaded(server.config());
    let session = |s: &Lemur, name: &str| sessions(&s.snap).find(|(_, t)| t.name == name).map(|(_, t)| (t.agent(), t.activity));
    let reported = refreshing(&mut rt, 10.0, |s| {
        session(s, &asks) == Some((Some(Agent::Codex), Activity::NeedsInput))
            && session(s, &by_hand) == Some((Some(Agent::Codex), Activity::Working))
    });
    assert!(reported, "{:?} {:?}", session(&rt.state, &asks), session(&rt.state, &by_hand));
    assert_eq!(session(&rt.state, &started), Some((Some(Agent::Codex), Activity::Unknown)));
    assert_eq!(session(&rt.state, &plain), Some((None, Activity::Unknown)));

    let listed: Vec<String> = rt.state.agents().iter().map(|(_, s)| s.name.clone()).collect();
    assert!(listed.contains(&started) && listed.contains(&by_hand) && listed.contains(&asks), "{listed:?}");
    assert!(!listed.contains(&plain), "{listed:?}");
    let noted = sessions(&rt.state.snap).filter(|(_, s)| s.codex).filter_map(|(_, s)| s.shell_to_note()).count();
    assert_eq!(noted, 0, "only Claude's are kept to resume");
}

/// Picking an agent the host doesn't have says so where failures show, for Codex and for
/// Claude, a thread and a new worktree alike: nothing starts, and Lemur goes on, a shell
/// starting as ever. With the agent there, Codex starts, labelled for it.
#[test]
fn picking_an_agent_the_host_lacks_says_so() {
    let server = HcServer::new("pnoagent");
    let dir = TempDir::new("pnoagent");
    let repo = dir.join("api");
    std::fs::create_dir(&repo).unwrap();
    git_in(&repo, &["init", "-q", "-b", "main"]);
    git_in(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let path = repo.canonicalize().unwrap().to_string_lossy().into_owned();
    let mut cfg = server.config();
    cfg.hosts[0].claude = "lemur-has-no-claude".into();
    cfg.hosts[0].codex = "lemur-has-no-codex".into();
    cfg.bases.push(Base::new("local", &path));
    let mut rt = loaded(cfg);
    let start = |rt: &mut Runtime<Lemur>, ask: super::prompt::Ask| {
        rt.update(|s, _| s.ask_name(ask));
        rt.update(|s, _| {
            if let Some(p) = &mut s.prompt {
                p.input = rune::TextEdit::new("x");
            }
        });
        rt.update(|s, _| s.prompt_enter());
    };
    let failed = |rt: &mut Runtime<Lemur>| {
        let said = wait_for(rt, 10.0, |s| matches!(&s.status, Some((_, Kind::Error))));
        assert!(said, "{:?}", rt.state.status);
        rt.state.status.take().unwrap().0
    };
    let thread = |agent: Agent| super::prompt::Ask::NewThread { host: "local".into(), base: Some(0), dir: path.clone(), agent: Some(agent) };
    for (agent, said) in [
        (Agent::Codex, "Codex isn't installed on local: its shell can't find `lemur-has-no-codex`"),
        (Agent::Claude, "Claude Code isn't installed on local: its shell can't find `lemur-has-no-claude`"),
    ] {
        start(&mut rt, thread(agent));
        assert_eq!(failed(&mut rt), said);
        start(&mut rt, super::prompt::Ask::NewWorktree { base: 0, agent: Some(agent) });
        assert_eq!(failed(&mut rt), said);
    }
    assert!(server.addresses().is_empty(), "no terminal");
    assert!(!repo.join(".claude/worktrees/x").exists(), "no worktree");
    assert!(rt.state.cfg.threads.is_empty());

    // Lemur goes on: a shell starts.
    start(&mut rt, super::prompt::Ask::NewThread { host: "local".into(), base: Some(0), dir: path.clone(), agent: None });
    assert!(wait_for(&mut rt, 10.0, |s| s.active.is_some()), "{:?}", rt.state.status);

    // And Codex, with it there.
    rt.state.cfg.hosts[0].codex = "sleep 600".into();
    start(&mut rt, thread(Agent::Codex));
    let started =
        refreshing(&mut rt, 10.0, |s| s.active.as_ref().and_then(|k| s.session_of(k)).is_some_and(|t| t.agent() == Some(Agent::Codex)));
    assert!(started, "{:?}", rt.state.status);
    assert!(rt.state.cfg.threads.is_empty(), "no Claude to keep");
}

/// A thread shows what its program calls it until you name it, and a rename starts from its
/// own label, not that title, which isn't a name it could keep.
#[test]
fn a_thread_shows_its_programs_title_until_named() {
    let server = HcServer::new("ptitles");
    let script = r#"printf '\033]2;\342\234\263 Fix the login\033\\'; exec sleep 600"#;
    let t = server.run("/", &["lemur.kind=claude", "lemur.label=claude"], &["sh", "-c", script]);
    let mut rt = loaded(server.config());
    assert!(refreshing(&mut rt, 10.0, |s| s.label_of("local", &t) == "Fix the login"), "{}", rt.state.label_of("local", &t));

    rt.update(|s, _| s.ask_name(super::prompt::Ask::Rename { host: "local".into(), session: t.clone() }));
    assert_eq!(rt.state.prompt.as_ref().unwrap().input.text, "claude");
    rt.update(|s, _| {
        if let Some(p) = &mut s.prompt {
            p.input = rune::TextEdit::new("review");
        }
    });
    rt.update(|s, _| s.prompt_enter());
    assert!(refreshing(&mut rt, 10.0, |s| s.label_of("local", &t) == "review"));
    // Yours stays, herdr-core keeping it, over what the program calls it.
    let kept = || terminal(&server, &t).and_then(|x| x.labels.get(snapshot::LABEL).cloned());
    assert!(wait_until(Duration::from_secs(5), || kept().as_deref() == Some("review")), "{:?}", kept());
    rt.state.refresh();
    idle(&mut rt, 1.5);
    assert_eq!(rt.state.label_of("local", &t), "review");
}

/// A thread put away leaves the sidebar's list and the tabs for "Put away", still running,
/// and herdr-core keeps it so; on show, another shows instead. Opened again, it's back; and
/// one that needs you comes back on its own.
#[test]
fn a_thread_put_away_comes_back_when_it_needs_you() {
    let server = HcServer::new("paway");
    let a = server.run("/", &["lemur.label=a"], &["sleep", "600"]);
    // Asks for you once it's told to.
    let asks = r#"read x; printf '\033]7501;state=blocked:app=claude-code\033\\'; exec sleep 600"#;
    let b = server.run("/", &["lemur.label=b"], &["sh", "-c", asks]);
    let mut rt = loaded(server.config());
    rt.set_viewport((1200.0, 700.0), 2.0);
    open(&mut rt, &a);
    open(&mut rt, &b);
    let listed = |s: &Lemur| s.snap.hosts[0].other.iter().filter(|t| !t.away).map(|t| t.name.clone()).collect::<Vec<_>>();

    rt.update(|s, tx| actions::run(s, Action::PutAway, tx));
    assert_eq!(rt.state.active, Some(key(&a)));
    assert_eq!(listed(&rt.state), std::slice::from_ref(&a));
    assert!(wait_until(Duration::from_secs(5), || labelled(&server, &b, snapshot::AWAY)));
    rt.state.refresh();
    idle(&mut rt, 1.5);
    assert_eq!(listed(&rt.state), std::slice::from_ref(&a), "herdr-core keeps it away");
    frames(&mut rt, 20);
    assert!(rt.find("away-local-").is_some(), "under Put away");
    assert!(rt.find(&format!("t-{b}")).is_none(), "nor a tab");

    open(&mut rt, &b);
    assert!(wait_until(Duration::from_secs(5), || !labelled(&server, &b, snapshot::AWAY)), "opened, it's back");
    rt.state.refresh();
    idle(&mut rt, 1.5);
    assert_eq!(listed(&rt.state), [a.clone(), b.clone()]);

    // Away again, it asks for you: it's back.
    rt.update(|s, _| s.put_away(key(&b)));
    assert!(wait_until(Duration::from_secs(5), || labelled(&server, &b, snapshot::AWAY)));
    server.hc(&["send", &b, "go"]);
    let back = refreshing(&mut rt, 10.0, |_| !labelled(&server, &b, snapshot::AWAY));
    assert!(back, "it needs you");
    rt.state.refresh();
    idle(&mut rt, 1.5);
    assert_eq!(listed(&rt.state), [a.clone(), b.clone()]);
}

/// A thread put away doesn't show instead of one that ended, nor when Lemur starts again: it
/// stays away.
#[test]
fn a_thread_put_away_doesnt_come_back_on_its_own() {
    let server = HcServer::new("pawaystay");
    let [x, y] = ["x", "y"].map(|l| server.run("/", &[&format!("lemur.label={l}")], &["sleep", "600"]));
    let mut rt = loaded(server.config());
    open(&mut rt, &x);
    open(&mut rt, &y);
    rt.update(|s, _| s.put_away(key(&x)));
    assert!(wait_until(Duration::from_secs(5), || labelled(&server, &x, snapshot::AWAY)));
    assert!(refreshing(&mut rt, 5.0, |s| s.session_of(&key(&x)).is_some_and(|t| t.away)));

    server.hc(&["kill", &y]);
    assert!(refreshing(&mut rt, 10.0, |s| s.session_of(&key(&y)).is_none()));
    assert_eq!(rt.state.active, None, "x isn't shown instead");
    assert!(rt.state.session_of(&key(&x)).is_some_and(|t| t.away), "still away");

    // Shown last before Lemur quit, it isn't opened when it starts.
    rt.state.restore = Some(key(&x));
    rt.state.refresh();
    idle(&mut rt, 1.5);
    assert!(rt.state.session_of(&key(&x)).is_some_and(|t| t.away));
    assert_eq!(rt.state.active, None, "nor when Lemur starts");
}

/// A host herdr-core needs you to add is set up in a terminal over the window, as Herdr
/// does: `hc machine add`, where you answer. Put away, it ends, and herdr-core is asked
/// again.
#[test]
fn a_host_that_needs_you_is_set_up_in_a_terminal() {
    let server = HcServer::new("psetup");
    let cfg = server.far_config("pb-set");
    let host = cfg.hosts[0].clone();
    // In place of `hc machine add`, which would dial the host.
    crate::actions::SET_UP
        .lock()
        .unwrap()
        .push(("pb-set".into(), vec!["sh".into(), "-c".into(), "echo pb-set now has hc; exec sleep 600".into()]));
    let mut rt = window(cfg);
    rt.update(|s, _| s.set_up_host(host));
    assert!(wait_for(&mut rt, 10.0, |s| s.peek.is_some()), "its terminal shows: {:?}", rt.state.status);
    let k = rt.state.peek.clone().unwrap();
    assert_eq!(k.0, crate::actions::HERE);
    assert!(wait_until(Duration::from_secs(5), || server.hc(&["read", &k.1]).contains("pb-set now has hc")));
    // A refresh keeps it while it shows.
    rt.state.refresh();
    idle(&mut rt, 1.5);
    assert_eq!(rt.state.peek, Some(k.clone()));
    assert!(rt.state.terminals.contains_key(&k));

    rt.update(|s, _| s.close_peek());
    let ended = wait_until(Duration::from_secs(5), || {
        rt.frame(1.0 / 60.0);
        !server.addresses().contains(&k.1)
    });
    assert!(ended, "its terminal ended");
    assert!(rt.state.setups.is_empty());
}

/// A double click on Set up starts one terminal, whose stream comes back on its own once its
/// program is lost, and which ends as Lemur quits: kept, it would stay with nothing to show
/// it.
#[test]
fn a_hosts_set_up_starts_once_comes_back_and_ends_with_lemur() {
    let server = HcServer::new("psetonce");
    let cfg = server.far_config("pb-once");
    let host = cfg.hosts[0].clone();
    let dir = TempDir::new("psetonce");
    let started = dir.join("started");
    let stand_in = |also: &str| {
        vec!["sh".to_string(), "-c".to_string(), format!("echo {also} >> '{}'; exec sleep 600", started.display())]
    };
    let mut set_up = crate::actions::SET_UP.lock().unwrap();
    set_up.push(("pb-once".into(), stand_in("once")));
    set_up.push(("pb-twice".into(), stand_in("twice")));
    drop(set_up);
    let mut rt = window(cfg);
    rt.update(|s, _| {
        s.set_up_host(host.clone());
        s.set_up_host(host.clone());
    });
    assert!(wait_for(&mut rt, 10.0, |s| s.peek.is_some() && s.setting_up.is_empty()), "its terminal shows");
    let lines = || std::fs::read_to_string(&started).unwrap_or_default();
    assert!(wait_until(Duration::from_secs(5), || !lines().is_empty()));
    idle(&mut rt, 0.5);
    assert_eq!(lines(), "once\n", "only one started");
    assert_eq!(rt.state.setups.len(), 1);

    let k = rt.state.peek.clone().unwrap();
    let attached = |s: &Lemur| s.terminals.get(&k).is_some_and(|v| v.is_attached());
    assert!(wait_for(&mut rt, 10.0, attached));
    kill_program(&server, &k.1);
    assert!(wait_for(&mut rt, 10.0, |s| s.terminals.get(&k).is_some_and(|v| v.disconnected())), "its stream dropped");
    assert!(refreshing(&mut rt, 20.0, attached), "attached again on its own");

    // Another host's takes the card, and the first ends.
    let other = Host::new("pb-twice", "me@pb-twice");
    crate::hc::set_test_route("pb-twice", &server.session);
    rt.update(|s, _| s.set_up_host(other));
    let moved = wait_for(&mut rt, 10.0, |s| s.peek.as_ref().is_some_and(|p| *p != k) && s.setups.len() == 1);
    assert!(moved, "the second shows");
    let gone = wait_until(Duration::from_secs(5), || !server.addresses().contains(&k.1));
    assert!(gone, "the first ended");

    let k = rt.state.peek.clone().unwrap();
    assert!(server.addresses().contains(&k.1));
    // Lemur quits.
    drop(rt);
    let gone = wait_until(Duration::from_secs(5), || !server.addresses().contains(&k.1));
    assert!(gone, "it ended with Lemur");
}

/// A list whose every thread is put away still shows them, under "Put away", with their way
/// back.
#[test]
fn a_list_of_threads_all_put_away_still_shows_them() {
    let server = HcServer::new("pallaway");
    let a = server.run("/", &["lemur.label=a"], &["sleep", "600"]);
    let mut rt = loaded(server.config());
    open(&mut rt, &a);
    rt.update(|s, _| s.put_away(key(&a)));
    assert!(wait_until(Duration::from_secs(5), || labelled(&server, &a, snapshot::AWAY)));
    assert!(refreshing(&mut rt, 5.0, |s| s.session_of(&key(&a)).is_some_and(|t| t.away)), "put away");
    rt.state.refresh();
    idle(&mut rt, 1.5);
    frames(&mut rt, 20);
    assert!(rt.find("away-local-").is_some(), "under Put away");
    click(&mut rt, "away-local-");
    assert!(rt.find(&format!("row-local-{a}")).is_some(), "its row, opened");
}

/// Renaming a thread lists no threads to go to: a click on one would drop the name typed.
#[test]
fn a_rename_lists_no_threads() {
    let server = HcServer::new("prenrows");
    let [a, _b] = ["claude", "claude two"].map(|l| server.run("/", &[&format!("lemur.label={l}")], &["sleep", "600"]));
    let mut rt = loaded(server.config());
    assert!(refreshing(&mut rt, 5.0, |s| s.session_of(&key(&a)).is_some()));
    rt.update(|s, _| s.ask_name(super::prompt::Ask::Rename { host: "local".into(), session: a.clone() }));
    frames(&mut rt, 20);
    assert!(rt.state.prompt.is_some());
    assert!(rt.find("spotlight-input").is_some());
    assert!(rt.find("go-0").is_none(), "no threads listed");
}

/// A refresh that ran as a host's core went down kept that host's error. Once the watch
/// reaches the core again, the host is fetched again and lists its threads, without waiting
/// for the next refresh.
#[test]
fn a_host_a_refresh_failed_lists_again_once_watched() {
    let server = HcServer::new("refetch");
    server.run("/", &["lemur.label=kept"], &["sleep", "600"]);
    // No ticks and no watch: the refreshes here are the test's own.
    let mut rt = window(server.config());
    rt.update(|s, _| s.refresh());
    let listed = |s: &Lemur| sessions(&s.snap).any(|(_, t)| t.label == "kept");
    assert!(wait_for(&mut rt, 10.0, listed), "listed at first");
    rt.update(|s, _| {
        s.data[0].error = Some("herdr-core went away".into());
        s.recompose();
    });
    assert!(wait_for(&mut rt, 5.0, |s| s.snap.hosts[0].error.is_some()), "what that refresh found shows");
    rt.update(|s, _| s.watch_connected());
    assert!(wait_for(&mut rt, 5.0, |s| listed(s) && s.snap.hosts[0].error.is_none()), "{:?}", rt.state.snap.hosts[0].error);
}

/// With hosts in two herdr-cores, one lost and back is watched again, while the other's
/// changes go on: its new threads show without a refresh.
#[test]
fn a_core_lost_and_back_is_watched_again() {
    let a = HcServer::new("pwatcha");
    let b = HcServer::new("pwatchb");
    let mut cfg = a.config();
    let mut mac = Host::new("mac", "local");
    mac.hc_session = b.config().hosts[0].hc_session.clone();
    cfg.hosts.push(mac);
    let mut rt = window(cfg);
    // Out of sight, the ticks refresh only every fifth (`AWAY`): what lists sooner, with no
    // such tick between, the watch brought.
    rt.state.looking = false;
    rt.update(|s, _| s.start());
    assert!(wait_for(&mut rt, 10.0, |s| s.loaded));
    // Both refreshes `start` makes are in: one still fetching as b's core stops would keep
    // that error for b's host until the next, whatever the watch tells.
    idle(&mut rt, 2.0);
    let settled = rt.state.ticks;
    let listed = |s: &Lemur, label: &str| sessions(&s.snap).any(|(_, t)| t.label == label);
    // Starts a thread labelled `label` in `server`'s core: whether the watch lists it, within
    // six seconds and with no refresh meanwhile. One that happens to come meanwhile, it tries
    // again.
    let watched_in = |rt: &mut Runtime<Lemur>, server: &HcServer, label: &str| {
        for attempt in 0..3 {
            let label = format!("{label}{attempt}");
            let before = rt.state.ticks;
            server.run("/", &[&format!("lemur.label={label}")], &["sleep", "600"]);
            let seen = wait_for(rt, 6.0, |s| listed(s, &label));
            // A refresh the tick before may still be fetching.
            let refreshed = (before.saturating_sub(1).max(settled)..rt.state.ticks).any(|t| t % 5 == 0);
            if !seen {
                return false;
            }
            if !refreshed {
                return true;
            }
        }
        false
    };
    assert!(watched_in(&mut rt, &b, "first"), "b's changes reach Lemur");

    b.hc(&["server", "stop"]);
    // The watch connects again after `REFRESH`, which starts b's core, as connecting does.
    idle(&mut rt, 4.5);
    assert!(watched_in(&mut rt, &b, "later"), "b's changes reach Lemur again");
    assert!(watched_in(&mut rt, &a, "meanwhile"), "a's go on");
}

/// A helper's terminal, peeked at from under its thread, attaches again on its own too once
/// its stream drops.
#[test]
fn a_dropped_helper_reattaches_on_its_own() {
    let server = HcServer::new("phelperre");
    let a = server.run("/", &["lemur.label=a"], &["sleep", "600"]);
    let out = server.hc(&["run", "--bg", "--json", "--keep", "--parent", &a, "--label", "lemur.label=h", "--", "sh"]);
    let started: serde_json::Value = serde_json::from_str(&out).unwrap();
    let b = started["address"].as_str().unwrap().to_string();
    let mut rt = loaded(server.config());
    open(&mut rt, &a);
    let listed = refreshing(&mut rt, 5.0, |s| s.session_of(&key(&a)).is_some_and(|t| t.helpers.iter().any(|h| h.id == b)));
    assert!(listed, "b is a's helper");
    rt.update(|s, _| s.peek(key(&b)));
    assert_eq!(rt.state.peek, Some(key(&b)));
    let attached = |s: &Lemur| s.terminals.get(&key(&b)).is_some_and(|v| v.is_attached());
    assert!(wait_for(&mut rt, 10.0, attached));
    kill_program(&server, &b);
    assert!(wait_for(&mut rt, 10.0, |s| s.terminals.get(&key(&b)).is_some_and(|v| v.disconnected())), "its stream dropped");
    assert!(refreshing(&mut rt, 20.0, attached), "attached again on its own");
    assert_eq!(rt.state.peek, Some(key(&b)));
}

/// A terminal whose stream dropped while its program may still run attaches again on its own
/// once the program runs, as Herdr's do: here a kept terminal's shell dies by a signal and
/// herdr-core starts a new one, as a host's terminals come back with its line. No click.
#[test]
fn a_dropped_terminal_reattaches_on_its_own() {
    let server = HcServer::new("reattach-own");
    let a = server.run("/", &["lemur.label=a"], &["sh"]);
    let mut rt = loaded(server.config());
    open(&mut rt, &a);
    let attached = |s: &Lemur| s.terminals.get(&key(&a)).is_some_and(|v| v.is_attached());
    assert!(wait_for(&mut rt, 10.0, attached));
    kill_program(&server, &a);
    assert!(wait_for(&mut rt, 10.0, |s| s.terminals.get(&key(&a)).is_some_and(|v| v.disconnected())), "its stream dropped");
    assert!(refreshing(&mut rt, 20.0, attached), "attached again on its own");
    assert_eq!(rt.state.active, Some(key(&a)));
    assert!(rt.state.terminals[&key(&a)].status.is_none(), "and says nothing: {:?}", rt.state.terminals[&key(&a)].status);
}

/// While a host can't be reached, the thread on show stays, terminal and all: its threads
/// aren't known to be gone. Reached again without it, it goes.
#[test]
fn an_unreached_host_keeps_its_terminals() {
    let server = HcServer::new("punreach");
    let a = server.run("/", &["lemur.label=a"], &["sleep", "600"]);
    let mut rt = loaded(server.config());
    open(&mut rt, &a);
    let shown = |s: &Lemur| (s.active.clone(), s.terminals.contains_key(&key(&a)));
    assert_eq!(shown(&rt.state), (Some(key(&a)), true));

    let mut unreached = rt.state.snap.clone();
    unreached.hosts[0].error = Some("the line dropped".into());
    unreached.hosts[0].other.clear();
    unreached.hosts[0].bases.clear();
    rt.update(|s, _| s.set_snapshot(unreached));
    assert_eq!(shown(&rt.state), (Some(key(&a)), true));

    server.hc(&["keep", &a, "--off"]);
    server.hc(&["kill", &a]);
    assert!(refreshing(&mut rt, 10.0, |s| shown(s) == (None, false)), "{:?}", shown(&rt.state));
}

/// A tab closed with its ×, as herdr-core tells of it, hands the keyboard to the one shown
/// instead: ⌘J works at once. Its program running, the × asks first, as ⌘W does.
#[test]
fn the_tab_shown_after_a_close_has_the_keyboard() {
    let server = HcServer::new("pclosex");
    let [a, b] = ["a", "b"].map(|l| server.run("/", &[&format!("lemur.label={l}")], &["sleep", "600"]));
    let mut rt = loaded(server.config());
    rt.set_viewport((1200.0, 700.0), 2.0);
    open(&mut rt, &a);
    assert!(busy(&mut rt, &key(&a), true));
    // a's ×; a runs sleep, so it asks.
    press_close_in(&mut rt, &format!("t-{a}"));
    assert!(rt.state.dialog.is_some(), "asked");
    click(&mut rt, "dialog-ok");
    assert!(wait_until(Duration::from_secs(5), || !server.addresses().contains(&a)));
    assert!(refreshing(&mut rt, 10.0, |s| s.active == Some(key(&b))), "{:?}", rt.state.active);
    assert!(rt.state.keyboard_on_terminal(), "b has the keyboard");
    press(&mut rt, K::Command('j'), cmd());
    assert!(rt.state.prompt.is_some(), "⌘J answered");
}

/// The thread on show ending on its own while you type elsewhere, as in a spotlight, shows
/// another, but leaves the keyboard where it is.
#[test]
fn a_thread_shown_instead_leaves_the_keyboard_where_it_is() {
    let server = HcServer::new("pendedel");
    let [a, b] = ["a", "b"].map(|l| server.run("/", &[&format!("lemur.label={l}")], &["sleep", "600"]));
    let mut rt = loaded(server.config());
    open(&mut rt, &a);
    press(&mut rt, K::Command('p'), cmd());
    assert!(rt.state.prompt.is_some(), "⌘P's spotlight");

    server.hc(&["kill", &a]);
    assert!(refreshing(&mut rt, 10.0, |s| s.active == Some(key(&b))), "b shows instead");
    assert!(rt.state.prompt.is_some(), "the spotlight stays");
    assert!(!rt.state.keyboard_on_terminal(), "the spotlight keeps the keyboard");
    type_text(&mut rt, "x");
    assert_eq!(rt.state.prompt.as_ref().map(|p| p.input.text.as_str()), Some("x"));
}

/// ⌘W while peeking puts the peek away; the idle shell on show under it, which ⌘W would
/// close without asking, stays.
#[test]
fn cmd_w_while_peeking_puts_the_peek_away() {
    let server = HcServer::new("ppeekclose");
    let [a, b] = ["a", "b"].map(|l| server.run("/", &[&format!("lemur.label={l}")], &["sh"]));
    let mut rt = loaded(server.config());
    assert!(busy(&mut rt, &key(&a), false) && busy(&mut rt, &key(&b), false), "both shells at their prompt");
    open(&mut rt, &a);
    rt.update(|s, _| s.peek(key(&b)));
    assert_eq!(rt.state.peek, Some(key(&b)));

    press(&mut rt, K::Command('w'), cmd());
    assert_eq!(rt.state.peek, None, "the peek went");
    assert_eq!(rt.state.active, Some(key(&a)), "a still shows");
    rt.state.refresh();
    idle(&mut rt, 1.5);
    assert!(server.addresses().contains(&a), "a still runs");
    assert!(server.addresses().contains(&b), "b too");
    assert_eq!(rt.state.active, Some(key(&a)));
}

/// A thread that lands under a spotlight, as one that took a while to start, or a popup's
/// Show, shows under it. The spotlight keeps the keyboard, and hands it to that thread once
/// it's gone.
#[test]
fn a_thread_opened_under_a_spotlight_leaves_it_the_keyboard() {
    let server = HcServer::new("popenunder");
    let a = server.run("/", &["lemur.label=a"], &["sleep", "600"]);
    let b = server.run("/", &["lemur.label=b"], &["cat"]);
    let mut rt = loaded(server.config());
    open(&mut rt, &a);
    press(&mut rt, K::Command('p'), cmd());
    assert!(rt.state.prompt.is_some());
    rt.update(|s, _| s.open_fresh("local".into(), b.clone()));
    assert_eq!(rt.state.active, Some(key(&b)));
    assert!(rt.state.prompt.is_some(), "the spotlight stays");
    assert!(!rt.state.keyboard_on_terminal(), "the spotlight keeps the keyboard");
    type_text(&mut rt, "x");
    assert_eq!(rt.state.prompt.as_ref().map(|p| p.input.text.as_str()), Some("x"));
    press(&mut rt, K::Escape, Mods::default());
    assert!(rt.state.prompt.is_none());
    assert!(rt.state.keyboard_on_terminal(), "handed over once it's gone");
    assert!(wait_for(&mut rt, 10.0, |s| s.terminals.get(&key(&b)).is_some_and(|v| v.painted)), "painted");
    type_text(&mut rt, "hello");
    let k = key(&b);
    assert!(wait_for(&mut rt, 10.0, |s| super::terminal::lines(s, &k).iter().any(|l| l.contains("hello"))), "{:?}", screen(&rt, &k));
}

/// Watching a core doesn't dial its hosts: a host down, or not added yet, doesn't drop what's
/// watched to look for it again.
#[test]
fn watching_doesnt_wait_for_a_host() {
    let server = HcServer::new("pwatchdown");
    let mut cfg = server.config();
    cfg.hosts.push(Host::new("pb-down", "me@pb-down"));
    crate::hc::set_test_route("pb-down", &server.session);
    // What a host that can't be added leaves.
    server.core().needed_you("pb-down");
    let (cores, all) = super::watched(&cfg);
    assert_eq!(cores.len(), 1, "the one core");
    assert!(all, "none to look for again");
}

/// After a core restarts, a host's terminals come back only with its line. Until then its
/// threads aren't known to have ended: the one on show stays, and the host says why it lists
/// none.
#[test]
fn a_hosts_threads_outlast_its_line_being_down() {
    use std::os::unix::fs::PermissionsExt;
    let server = HcServer::new("plinedown");
    let dir = TempDir::new("plinedown");
    let off = dir.join("off");
    let via = dir.join("via");
    std::fs::write(&via, format!("#!/bin/sh\n[ -e '{}' ] && exit 1\nexec sh -c \"$1\"\n", off.display())).unwrap();
    std::fs::set_permissions(&via, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cfg = server.far_config_via("pb-line", &via.to_string_lossy());
    let out = server.hc(&["run", "--bg", "--json", "--keep", "--on", "pb-line", "--", "sleep", "600"]);
    let started: serde_json::Value = serde_json::from_str(&out).unwrap();
    let shown: Key = ("pb-line".into(), started["address"].as_str().unwrap().to_string());
    let mut rt = loaded(cfg);
    assert!(refreshing(&mut rt, 10.0, |s| s.session_of(&shown).is_some()));
    rt.update(|s, _| s.open(shown.0.clone(), shown.1.clone()));
    assert!(wait_for(&mut rt, 10.0, |s| s.terminals.get(&shown).is_some_and(|v| v.is_attached())));

    std::fs::write(&off, "").unwrap();
    server.hc(&["server", "restart"]);
    let start = std::time::Instant::now();
    let mut told = false;
    while !told && start.elapsed() < Duration::from_secs(10) {
        rt.update(|s, _| s.recompose());
        told = wait_for(&mut rt, 0.5, |s| s.snap.hosts[0].error.is_some());
    }
    assert!(told, "{:?}", rt.state.snap.hosts);
    assert_eq!(rt.state.active.as_ref(), Some(&shown), "it stays on show");
    assert!(rt.state.terminals.contains_key(&shown), "its view stays");
}

/// While an order dragged on one host is written, which on a poor line takes a while, the
/// others' threads go on changing; that host's stay as they show, not half written.
#[test]
fn an_order_being_written_holds_up_only_its_host() {
    let server = HcServer::new("porderone");
    let mut cfg = server.far_config("pb-order");
    let mut here = Host::new("local", "local");
    here.hc_session = Some(server.session.clone());
    cfg.hosts.insert(0, here);
    let mut rt = loaded(cfg);
    rt.state.ordering.insert(("pb-order".into(), None), None);
    let here = server.run("/", &["lemur.label=here"], &["sleep", "600"]);
    server.hc(&["run", "--bg", "--on", "pb-order", "--", "sleep", "600"]);
    for refresh in [true, false] {
        let start = std::time::Instant::now();
        let mut listed = false;
        while !listed && start.elapsed() < Duration::from_secs(10) {
            if refresh {
                rt.state.refresh();
            } else {
                rt.update(|s, _| s.recompose());
            }
            listed = wait_for(&mut rt, 0.5, |s| s.session_of(&key(&here)).is_some());
        }
        assert!(listed, "this host's thread lists");
        // Long enough for the far host's to have shown, were it not held.
        idle(&mut rt, 1.0);
        let far = rt.state.snap.hosts.iter().find(|h| h.name == "pb-order").expect("the far host");
        assert!(far.other.is_empty(), "that host's waits: {:?}", far.other.iter().map(|t| &t.name).collect::<Vec<_>>());
    }
}

/// A worktree's removal goes by its project, not where that was in the config: projects
/// moved meanwhile, it removes the right worktree; its project removed, nothing, where asking
/// again would have panicked.
#[test]
fn removing_a_worktree_goes_by_its_project() {
    let server = HcServer::new("premoval");
    let dir = TempDir::new("premoval");
    let (api, web) = (repo_at(&dir.join("api")), repo_at(&dir.join("web")));
    git_in(Path::new(&web), &["worktree", "add", "-q", "-b", "x", "wt/x"]);
    let wt = format!("{web}/wt/x");
    let mut cfg = server.config();
    cfg.bases = vec![Base::new("local", &api), Base::new("local", &web)];
    let mut rt = loaded(cfg);
    let removal = rt.state.removal(1, wt.clone(), "x".into());
    rt.update(|s, _| s.reorder_projects(0, 1));
    rt.update(|s, _| s.remove_worktree(removal.clone(), false));
    let gone = wait_for(&mut rt, 10.0, |_| !Path::new(&wt).join(".git").exists());
    assert!(gone, "web's worktree went: {:?}", rt.state.status);
    assert!(Path::new(&api).join(".git").exists(), "api's stays");

    let project = rt.state.cfg.bases[0].clone();
    assert_eq!(project.path, web);
    rt.update(|s, _| s.remove_project(&project));
    rt.update(|s, _| {
        s.confirm_remove_worktree(removal.clone(), Some(crate::actions::Kept::Changes));
        s.remove_worktree(removal, false);
    });
    frames(&mut rt, 20);
}

/// A double click on a gone thread resumes it once: two Claudes would work on one
/// conversation.
#[test]
fn a_double_click_resumes_a_thread_once() {
    let server = HcServer::new("presume1");
    let dir = TempDir::new("presume1");
    let api = repo_at(&dir.join("api"));
    let mut cfg = server.config();
    cfg.hosts[0].claude = "sh -c 'exec sleep 600' claude".into();
    cfg.bases = vec![Base::new("local", &api)];
    let thread = ClaudeThread {
        host: "local".into(),
        session: "t99".into(),
        path: api.clone(),
        claude_session: "e7f3a2c4-1b5d-4c8e-9f0a-2d3b4c5d6e7f".into(),
        label: Some("gone".into()),
        issue: None,
    };
    cfg.threads.push(thread.clone());
    let mut rt = loaded(cfg);
    let before = server.addresses().len();
    rt.update(|s, _| {
        s.resume(vec![thread.clone()]);
        s.resume(vec![thread.clone()]);
    });
    assert!(wait_for(&mut rt, 20.0, |s| s.resuming.is_empty()));
    idle(&mut rt, 0.5);
    assert_eq!(server.addresses().len(), before + 1, "one terminal");
}

/// A peek, or a host's set-up where a password may be asked for, keeps the keyboard: ⌘1 to
/// ⌘9, Next Agent and ⌃Tab don't show another thread under it, ⌘P doesn't open under it, and
/// whatever hands the keyboard back hands it to the terminal in front.
#[test]
fn a_peek_keeps_the_keyboard() {
    let server = HcServer::new("ppeekkeys");
    let [a, b] = ["a", "b"].map(|l| server.run("/", &[&format!("lemur.label={l}")], &["sleep", "600"]));
    let c = server.run("/", &["lemur.label=c"], &["cat"]);
    let mut rt = loaded(server.config());
    open(&mut rt, &a);
    rt.update(|s, _| s.peek(key(&c)));
    assert!(rt.state.keyboard_on_peek());
    rt.update(|s, tx| actions::run(s, Action::NextAgent, tx));
    assert_eq!(rt.state.active, Some(key(&a)));
    let keys = [
        (K::Command('2'), cmd(), "⌘2"),
        (K::Tab, ctrl(), "⌃Tab"),
        (K::Command('p'), cmd(), "⌘P"),
        (K::Command('t'), cmd(), "⌘T"),
        (K::Command('j'), cmd(), "⌘J"),
    ];
    for (k, mods, name) in keys {
        press(&mut rt, k, mods);
        idle(&mut rt, 0.3);
        assert_eq!(rt.state.active, Some(key(&a)), "{name}");
        assert_eq!(rt.state.peek, Some(key(&c)), "{name}");
        assert!(rt.state.prompt.is_none(), "{name}");
        assert_eq!(rt.state.terminals.len(), 2, "{name}");
        assert!(rt.state.keyboard_on_peek(), "{name}");
    }
    assert!(server.addresses().contains(&b));
    assert_eq!(server.addresses().len(), 3, "nothing started");

    // What's typed goes to the peek, in front.
    assert!(wait_for(&mut rt, 10.0, |s| s.terminals.get(&key(&c)).is_some_and(|v| v.painted)), "painted");
    type_text(&mut rt, "hello");
    let k = key(&c);
    assert!(wait_for(&mut rt, 10.0, |s| super::terminal::lines(s, &k).iter().any(|l| l.contains("hello"))), "{:?}", screen(&rt, &k));
}

/// Lemur notes the shell each Claude thread runs in, once, so a thread whose host restarted
/// is known when it comes back as a fresh one.
#[test]
fn claude_threads_have_their_shell_noted() {
    let server = HcServer::new("pnoteshell");
    let t = server.run("/", &["lemur.kind=claude", "lemur.label=c"], &["sh"]);
    let shell = server.run("/", &["lemur.label=s"], &["sh"]);
    let mut rt = loaded(server.config());
    let noted = |address: &str| {
        let t = terminal(&server, address).unwrap();
        (t.pid, t.labels.get(snapshot::SHELL).cloned())
    };
    let done = refreshing(&mut rt, 10.0, |_| {
        let (pid, label) = noted(&t);
        pid.is_some() && label == pid.map(|p| p.to_string())
    });
    assert!(done, "{:?}", noted(&t));
    assert_eq!(noted(&shell).1, None, "not a Claude thread");
}

/// A shell at its prompt with a job of its own (`&`, or ⌃Z) asks before ⌘W closes it: the job
/// would end too.
#[test]
fn a_shell_with_a_job_asks_before_closing() {
    let server = HcServer::new("pclosejob");
    let b = server.run("/", &["lemur.label=b"], &["sh"]);
    let mut rt = loaded(server.config());
    open(&mut rt, &b);
    server.hc(&["send", &b, "sleep 600 &"]);
    // Back at its prompt, with the job behind it.
    let idle_with_job = refreshing(&mut rt, 10.0, |s| {
        server.hc(&["read", &b]).contains("[1] ") && s.session_of(&key(&b)).is_some_and(|t| !t.busy)
    });
    assert!(idle_with_job, "{}", server.hc(&["read", &b]));

    press(&mut rt, K::Command('w'), cmd());
    assert!(wait_for(&mut rt, 5.0, |s| s.dialog.is_some()), "⌘W asks");
    assert!(server.addresses().contains(&b), "still running");
    rt.state.dialog = None;
}

/// ⌘W closes a shell at its prompt without asking, and the thread shown instead has the
/// keyboard.
#[test]
fn an_idle_shell_closes_without_asking() {
    let server = HcServer::new("pcloseidle");
    let a = server.run("/", &["lemur.label=a"], &["sleep", "600"]);
    let b = server.run("/", &["lemur.label=b"], &["sh"]);
    let mut rt = loaded(server.config());
    open(&mut rt, &a);
    open(&mut rt, &b);
    assert!(busy(&mut rt, &key(&b), false));

    press(&mut rt, K::Command('w'), cmd());
    assert!(rt.state.dialog.is_none());
    assert!(wait_until(Duration::from_secs(5), || {
        rt.frame(1.0 / 60.0);
        !server.addresses().contains(&b)
    }));
    assert!(rt.state.dialog.is_none(), "not asked");
    assert!(refreshing(&mut rt, 10.0, |s| s.active == Some(key(&a))));
    assert!(rt.state.keyboard_on_terminal(), "a has the keyboard");

    // a runs a program of its own: ⌘W asks.
    assert!(busy(&mut rt, &key(&a), true));
    press(&mut rt, K::Command('w'), cmd());
    assert!(rt.state.dialog.is_some());
    press(&mut rt, K::Escape, Mods::default());
    assert!(rt.state.dialog.is_none());
}

/// A thread's × in the sidebar closes it as ⌘W does: a shell at its prompt at once, one
/// running a program after asking.
#[test]
fn the_sidebars_close_asks_as_cmd_w_does() {
    let server = HcServer::new("pcloserow");
    let a = server.run("/", &["lemur.label=a"], &["sleep", "600"]);
    let b = server.run("/", &["lemur.label=b"], &["sh"]);
    let mut rt = loaded(server.config());
    rt.set_viewport((1200.0, 700.0), 2.0);
    assert!(busy(&mut rt, &key(&b), false) && busy(&mut rt, &key(&a), true));

    press_close_in(&mut rt, &format!("row-local-{b}"));
    assert!(rt.state.dialog.is_none(), "b's shell is at its prompt");
    assert!(wait_until(Duration::from_secs(5), || {
        rt.frame(1.0 / 60.0);
        !server.addresses().contains(&b)
    }));

    press_close_in(&mut rt, &format!("row-local-{a}"));
    assert!(rt.state.dialog.is_some(), "a runs sleep");
    press(&mut rt, K::Escape, Mods::default());
    idle(&mut rt, 0.5);
    assert!(server.addresses().contains(&a), "cancelled");
}

/// Every host is listed in Other, threads or none, and its + starts a terminal in its home
/// there.
#[test]
fn every_host_starts_threads_from_other() {
    let server = HcServer::new("phostnew");
    let mut rt = loaded(server.config());
    assert!(refreshing(&mut rt, 5.0, |s| !s.snap.hosts[0].home.is_empty()));
    frames(&mut rt, 20);
    assert!(rt.find("host-local").is_some(), "no threads, still listed");
    let home = std::env::var("HOME").unwrap();
    assert_eq!(rt.state.snap.hosts[0].home, home);

    // Its last item names one in the spotlight, there too.
    click(&mut rt, "host-plus-local");
    assert!(rt.popup_menu().is_some(), "its menu");
    rt.resolve_popup(Some("new-named")).unwrap();
    frames(&mut rt, 2);
    let asked = match rt.state.prompt.as_ref().map(|p| &p.what) {
        Some(super::prompt::Ask::NewThread { host, base, dir, .. }) => Some((host.clone(), *base, dir.clone())),
        _ => None,
    };
    assert_eq!(asked, Some(("local".to_string(), None, home.clone())));
    press(&mut rt, K::Escape, Mods::default());
    assert!(rt.state.prompt.is_none());

    click(&mut rt, "host-plus-local");
    rt.resolve_popup(Some("new-terminal")).unwrap();
    let mut started = None;
    assert!(wait_until(Duration::from_secs(5), || {
        rt.frame(1.0 / 60.0);
        started = server.core().terminals().into_iter().next();
        started.is_some()
    }));
    let started = started.unwrap();
    assert_eq!(started.cwd, home);
    assert_eq!(started.labels.get(snapshot::KIND).map(String::as_str), Some("shell"));
    assert!(wait_for(&mut rt, 10.0, |s| s.active == Some(key(&started.address))), "the new terminal shows");
}

/// The threads outside every project are in one Other section, each host's under a row
/// naming it.
#[test]
fn other_threads_are_one_section_by_host() {
    let server = HcServer::new("pother");
    let mut cfg = server.far_config("pb-shed");
    let mut here = Host::new("local", "local");
    here.hc_session = Some(server.session.clone());
    cfg.hosts.insert(0, here);
    server.run("/", &["lemur.label=here"], &["sleep", "600"]);
    server.hc(&["run", "--bg", "--on", "pb-shed", "--label", "lemur.label=there", "--", "sleep", "600"]);
    let mut rt = loaded(cfg);
    let listed = refreshing(&mut rt, 10.0, |s| s.snap.hosts.len() == 2 && s.snap.hosts.iter().all(|h| h.other.len() == 1));
    assert!(listed, "{:?}", rt.state.snap.hosts.iter().map(|h| (&h.name, h.other.len(), &h.error)).collect::<Vec<_>>());
    frames(&mut rt, 20);
    let top = |rt: &Runtime<Lemur>, k: &str| rt.presented(k).unwrap_or_else(|| panic!("{k}")).0[1];
    assert!(top(&rt, "other") < top(&rt, "host-local"));
    assert!(top(&rt, "host-local") < top(&rt, "host-pb-shed"));
    let here = &rt.state.snap.hosts[0].other[0].name;
    let there = &rt.state.snap.hosts[1].other[0].name;
    assert!(top(&rt, "host-local") < top(&rt, &format!("row-local-{here}")));
    assert!(top(&rt, &format!("row-local-{here}")) < top(&rt, "host-pb-shed"));
    assert!(top(&rt, "host-pb-shed") < top(&rt, &format!("row-pb-shed-{there}")));
    assert!(rt.find("host-section-local").is_none() && rt.find("host-section-pb-shed").is_none(), "no section per host");
}

/// The status bar says where the thread on show runs: its host, its worktree's branch, and
/// the servers running there, as links.
#[test]
fn the_status_bar_links_the_worktrees_servers() {
    let server = HcServer::new("pstatus");
    let mut cfg = server.config();
    cfg.bases.push(Base::new("local", "/srv/api"));
    let mut rt = window(cfg);
    rt.set_viewport((1200.0, 700.0), 2.0);
    let serve = |port, cwd: &str| crate::machine::Server { port, addr: "127.0.0.1".into(), name: "ruby".into(), cwd: cwd.into() };
    let machine = crate::machine::Reading {
        servers: vec![serve(3020, "/srv/api/.claude/worktrees/auth"), serve(3000, "/srv/api")],
        ..Default::default()
    };
    let snap = project_snapshot(&["t"], machine);
    rt.update(|s, _| s.set_snapshot(snap));
    open(&mut rt, "t");
    frames(&mut rt, 20);
    let texts: Vec<String> = rt.elements().into_iter().filter(|e| !e.exiting).filter_map(|e| e.text).collect();
    assert!(texts.contains(&"feature/auth".to_string()), "its branch: {texts:?}");
    assert!(rt.find("port-3020").is_some());
    assert!(rt.find("port-3000").is_none(), "the main checkout's");
    click(&mut rt, "port-3020");
    assert_eq!(rt.take_url_requests(), ["http://localhost:3020"]);
}

/// ⌘J lists the threads to go to under the new one's name: typing narrows them, ↓ goes past
/// Claude and Shell to them, and ⏎ opens the one picked. ⌘P lists them all.
#[test]
fn cmd_j_and_cmd_p_go_to_threads() {
    let server = HcServer::new("pgoto");
    let mut cfg = server.config();
    cfg.bases.push(Base::new("local", "/srv/api"));
    let mut rt = window(cfg);
    let snap = project_snapshot(&["review", "build"], Default::default());
    rt.update(|s, _| s.set_snapshot(snap));
    open(&mut rt, "review");
    let listed = |s: &Lemur| s.matches(s.prompt.as_ref().expect("asking")).into_iter().map(|m| m.key.1).collect::<Vec<_>>();

    press(&mut rt, K::Command('j'), cmd());
    // The worktree's own first, untouched by the name it starts with.
    assert_eq!(listed(&rt.state), ["review", "build", "main-shell"]);
    type_text(&mut rt, "bui");
    assert_eq!(rt.state.prompt.as_ref().unwrap().input.text, "bui");
    assert_eq!(listed(&rt.state), ["build"]);
    for _ in 0..3 {
        press(&mut rt, K::Down, Mods::default());
    }
    assert_eq!(rt.state.prompt.as_ref().unwrap().go, Some(0));
    press(&mut rt, K::Enter, Mods::default());
    assert_eq!(rt.state.active, Some(key("build")));
    assert!(rt.state.prompt.is_none());

    press(&mut rt, K::Command('p'), cmd());
    // Those opened last first, the one on show at the end.
    assert_eq!(listed(&rt.state), ["review", "main-shell", "build"]);
    type_text(&mut rt, "main");
    press(&mut rt, K::Enter, Mods::default());
    assert_eq!(rt.state.active, Some(key("main-shell")));
}

/// The Shortcuts page's keys for the window, the threads and dictation are bound: each does
/// something, by an action's key or one the window takes itself, but for the app menu's
/// (⌘H, ⌥⌘H, ⌘Q, which macOS's standard menus give) and Esc (which closes what's open).
#[test]
fn the_shortcuts_page_shows_bound_keys() {
    use super::settings::shortcuts_for_tests as shortcuts;
    let server = HcServer::new("pshortcut");
    let mut rt = window(server.config());
    let mut unbound = vec![];
    let mut pressed = vec![];
    for (group, keys) in shortcuts() {
        if !["Window", "Threads", "Dictation"].contains(&group) {
            continue;
        }
        for chord in keys {
            if ["escape", "mod+h", "alt+mod+h", "mod+q"].contains(&chord) {
                continue;
            }
            let ev = match chord.strip_prefix("mod+") {
                Some("enter") => KeyEvent { key: K::Enter, mods: cmd() },
                Some(c) if c.chars().count() == 1 => KeyEvent { key: K::Command(c.chars().next().unwrap()), mods: cmd() },
                _ => panic!("a key this test can't press: {chord}"),
            };
            let mut bound = false;
            rt.update(|_, tx| bound = Action::for_key(&ev, tx).is_some());
            pressed.push(chord);
            if !bound {
                unbound.push(format!("{group}: {chord}"));
            }
        }
    }
    assert!(unbound.is_empty(), "not bound: {unbound:?}");
    // ⌘, and ⌘W, ⌘1 and ⌘9, ⌘⏎: the groups are the ones named.
    assert_eq!(pressed, ["mod+,", "mod+w", "mod+1", "mod+9", "mod+enter"]);
}
