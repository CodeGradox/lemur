//! The agents panel and Next Agent, driven through Rune's `Runtime` as `tests.rs` drives the
//! window, against snapshots made up here or a herdr-core of the test's own. The meters'
//! tests use these helpers too.

use super::super::core::wait_for;
use super::super::{Action, Lemur, Right, actions};
use crate::config::Config;
use crate::snapshot::{Activity, Session};
use crate::test_support::{HcServer, wait_until};
use rune::{Key as K, KeyEvent, Mods, Runtime};
use std::time::Duration;
use crate::gui::tests::{apply, cmd, key, open, window};

/// The window on `cfg`, once its first snapshot is in.
pub(in crate::gui) fn loaded(cfg: Config) -> Runtime<Lemur> {
    let mut rt = window(cfg);
    rt.state.refresh();
    assert!(wait_for(&mut rt, 10.0, |s| s.loaded), "the first snapshot");
    rt
}

/// A snapshot fetched, and the window run until it's in.
pub(in crate::gui) fn refresh(rt: &mut Runtime<Lemur>) {
    let before = rt.state.snap.hosts.first().map(|h| h.other.len());
    rt.update(|s, _| s.refresh());
    // No sign of when it lands: give it a moment.
    let _ = wait_for(rt, 0.5, |s| s.snap.hosts.first().map(|h| h.other.len()) != before);
}

fn run(rt: &mut Runtime<Lemur>, action: Action) {
    rt.update(|s, tx| actions::run(s, action, tx));
    rt.frame(1.0 / 60.0);
}

/// A press at `x`, `y` once what just came in shows.
pub(in crate::gui) fn click_at(rt: &mut Runtime<Lemur>, x: f32, y: f32) {
    rt.pointer_move(x, y);
    // Under the pointer, what shows on hover does.
    for _ in 0..4 {
        rt.frame(1.0 / 60.0);
    }
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    for _ in 0..4 {
        rt.frame(1.0 / 60.0);
    }
}

pub(in crate::gui) fn click(rt: &mut Runtime<Lemur>, key: &str) {
    // What just came in fades in: a press lands once it shows.
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element keyed {key}"));
    click_at(rt, x, y);
}

/// What the text keyed `key` says, if it shows.
fn says(rt: &Runtime<Lemur>, key: &str) -> Option<String> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting).and_then(|e| e.text)
}

/// The agents panel lists each Claude thread with what it runs under it: Claude's
/// subagents as it reports them (OSC 7501 records), and terminals its agent started (`hc
/// run`, parent and all). A click on a helper's terminal peeks at it, which stays across
/// refreshes; a subagent's shows its thread. The sidebar only counts them.
#[test]
fn the_agents_panel_shows_what_threads_run() {
    let server = HcServer::new("ragents");
    // Reports a subagent, its task in base64 as Claude Code sends it.
    let report = r#"t=$(printf 'Explore code' | base64); printf '\033]7501;state=working:app=claude-code:id=sub1:title=%s\033\\' "$t"; exec sleep 600"#;
    let a = server.run("/", &["lemur.label=main"], &["sh", "-c", report]);
    let out = server.hc(&["run", "--bg", "--json", "--parent", &a, "--label", "lemur.label=helper", "--", "sleep", "600"]);
    let started: serde_json::Value = serde_json::from_str(&out).unwrap();
    let b = started["address"].as_str().unwrap().to_string();
    let mut rt = loaded(server.config());
    let helpers = |s: &Lemur| {
        let threads = &s.snap.hosts[0].other;
        let names: Vec<String> = threads.iter().map(|t| t.name.clone()).collect();
        let helpers: Vec<(String, bool)> = threads.iter().flat_map(|t| t.helpers.iter().map(|h| (h.label.clone(), h.terminal))).collect();
        (names, helpers)
    };
    let start = std::time::Instant::now();
    while helpers(&rt.state).1.len() < 2 && start.elapsed() < Duration::from_secs(10) {
        refresh(&mut rt);
    }
    assert_eq!(
        helpers(&rt.state),
        (vec![a.clone()], vec![("Explore code".into(), false), ("helper".into(), true)]),
        "the helper's terminal isn't a thread of its own"
    );
    let counted = rt.elements().into_iter().any(|e| e.text.is_some_and(|t| t.contains("2 helping")));
    assert!(counted, "the sidebar counts them");
    assert!(rt.find("agent-0-0").is_none(), "the panel starts hidden");

    rt.key(KeyEvent { key: K::Command('a'), mods: Mods { shift: true, ..cmd() } });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.right, Some(Right::Agents), "⌘⇧A");
    click(&mut rt, "agent-0-0");
    assert_eq!(rt.state.active, Some(key(&a)), "a subagent shows its thread");
    click(&mut rt, "agent-0-1");
    assert_eq!(rt.state.peek, Some(key(&b)), "a helper's terminal is peeked at");
    assert_eq!(rt.state.active, Some(key(&a)), "still on show under it");
    assert!(rt.find("peek").is_some());
    refresh(&mut rt);
    assert_eq!(rt.state.peek, Some(key(&b)), "kept across a refresh");
    // × puts it away again.
    click(&mut rt, "peek-close");
    assert_eq!(rt.state.peek, None);
    click(&mut rt, "agent-0-1");
    // ↗ shows it instead.
    click(&mut rt, "peek-open");
    assert_eq!((rt.state.active.clone(), rt.state.peek.clone()), (Some(key(&b)), None));
    refresh(&mut rt);
    assert_eq!(rt.state.active, Some(key(&b)), "kept open across a refresh");
}

/// The panel says which need you, and which finished while you looked away, until you've
/// seen them; nothing moves as that changes.
#[test]
fn the_agents_panel_says_which_need_you_or_are_done() {
    let server = HcServer::new("ragents-say");
    let mut rt = window(server.config());
    rt.update(|s, _| s.toggle_right(Right::Agents));
    let check = |rt: &mut Runtime<Lemur>, statuses: &[(&str, Activity)], want: [Option<&str>; 3]| {
        apply(rt, statuses);
        for _ in 0..20 {
            rt.frame(1.0 / 60.0);
        }
        let order: Vec<String> = rt.state.agents().iter().map(|(_, s)| s.name.clone()).collect();
        assert_eq!(order, ["a", "b", "c"], "they stay put");
        for (i, want) in want.iter().enumerate() {
            assert!(rt.find(&format!("agent-{i}")).is_some(), "agent {i} listed");
            assert_eq!(says(rt, &format!("agent-{i}-says")).as_deref(), *want, "agent {i}");
        }
    };
    let (idle, working, asks) = (Activity::Idle, Activity::Working, Activity::NeedsInput);
    check(&mut rt, &[("a", working), ("b", asks), ("c", idle)], [None, Some("Needs attention"), None]);
    // a finished while you looked away, and b has been answered.
    check(&mut rt, &[("a", idle), ("b", working), ("c", idle)], [Some("Done"), None, None]);
    // Seen, it's just idle.
    open(&mut rt, "a");
    check(&mut rt, &[("a", idle), ("b", working), ("c", idle)], [None; 3]);
}

/// Next Agent goes through the agents done while you looked away one by one, then the
/// rest: opening one doesn't skip the next.
#[test]
fn next_agent_goes_through_each_done_one() {
    let server = HcServer::new("rnext-walk");
    let mut rt = window(server.config());
    apply(&mut rt, &[("a", Activity::Working), ("b", Activity::Working), ("c", Activity::Idle)]);
    apply(&mut rt, &[("a", Activity::Idle), ("b", Activity::Idle), ("c", Activity::Idle)]);
    let mut visited = vec![];
    for _ in 0..3 {
        run(&mut rt, Action::NextAgent);
        visited.push(rt.state.active.clone().unwrap().1);
    }
    assert_eq!(visited, ["a", "b", "c"]);
}

/// Next and Previous Agent go through the agents in the panel's order, round and round,
/// starting from the first. They have no keys.
#[test]
fn next_and_previous_agent_go_through_the_agents() {
    let server = HcServer::new("rnext-agent");
    let mut rt = window(server.config());
    apply(&mut rt, &[("idle", Activity::Idle), ("asks", Activity::NeedsInput), ("works", Activity::Working), ("shell", Activity::Unknown)]);
    for c in [']', '['] {
        rt.key(KeyEvent { key: K::Command(c), mods: cmd() });
    }
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.active, None, "no keys");
    let mut seen = vec![];
    for back in [false, false, false, false, true, true] {
        run(&mut rt, if back { Action::PreviousAgent } else { Action::NextAgent });
        seen.push(rt.state.active.clone().unwrap().1);
    }
    assert_eq!(seen, ["asks", "works", "idle", "asks", "idle", "works"]);

    // One put away is left out, and stays away.
    rt.update(|s, _| s.snap.hosts[0].other[0].away = true);
    let mut seen = vec![];
    for _ in 0..3 {
        run(&mut rt, Action::NextAgent);
        seen.push(rt.state.active.clone().unwrap().1);
    }
    assert!(!seen.contains(&"idle".to_string()), "{seen:?}");
    assert!(rt.state.snap.hosts[0].other[0].away);
}

/// The agents panel lists the agents' threads in their order, whatever they're doing, Next
/// Agent going through them the most urgent first; with none, it says so.
#[test]
fn the_agents_panel_keeps_their_order() {
    let mut rt = window(toml::from_str("").unwrap());
    run(&mut rt, Action::ToggleAgents);
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.find("agents-none").is_some(), "none yet");
    apply(&mut rt, &[("idle", Activity::Idle), ("asks", Activity::NeedsInput), ("works", Activity::Working), ("shell", Activity::Unknown)]);
    let names = |agents: Vec<(&str, &Session)>| agents.iter().map(|(_, s)| s.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(rt.state.agents()), ["idle", "asks", "works"], "a plain shell isn't one");
    assert_eq!(names(rt.state.agents_by_urgency()), ["asks", "works", "idle"]);
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.find("agent-2").is_some() && rt.find("agent-3").is_none());
    assert!(rt.find("agents-none").is_none());
}

/// The eye peeks at an agent from the panel, and the bell silences it: herdr-core keeps
/// that, and the bell stays to say so.
#[test]
fn the_agents_panel_peeks_and_silences() {
    let server = HcServer::new("ragents-eye");
    let [a, b] = ["a", "b"].map(|l| server.run("/", &[&format!("lemur.label={l}"), "lemur.kind=claude"], &["sleep", "600"]));
    let mut rt = loaded(server.config());
    open(&mut rt, &a);
    run(&mut rt, Action::ToggleAgents);
    let listed = |s: &Lemur| s.agents().iter().map(|(_, t)| t.name.clone()).collect::<Vec<_>>();
    assert!(wait_for(&mut rt, 5.0, |s| listed(s) == [a.clone(), b.clone()]), "{:?}", listed(&rt.state));
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.find("agent-0-peek").is_none(), "not the one on show");
    click(&mut rt, "agent-1-peek");
    assert_eq!(rt.state.peek, Some(key(&b)));
    assert_eq!(rt.state.active, Some(key(&a)));
    rt.update(|s, _| s.close_peek());
    click(&mut rt, "agent-1-silence");
    assert!(rt.state.session_of(&key(&b)).is_some_and(|t| t.silent), "at once");
    let cfg = server.config();
    assert!(
        wait_until(Duration::from_secs(10), || crate::snapshot::fetch(&cfg).hosts[0].other.iter().any(|t| t.name == b && t.silent)),
        "herdr-core keeps it"
    );
    assert_eq!(rt.state.active, Some(key(&a)), "the click didn't open it");
}
