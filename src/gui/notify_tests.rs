//! Notifications: which threads pop up, what answers a popup, and the overlay's buttons.

use super::super::core::wait_for;
use super::super::{Key, Lemur, sessions};
use super::{GAP, HEIGHT, Popup, Popups, WIDTH, popups_view};
use crate::snapshot::{Activity, HostView, Snapshot};
use crate::test_support::HcServer;
use rune::{Env, Runtime};
use crate::gui::tests::{key, session, window};

/// Applies a made-up snapshot of `sessions` on `local` the way a refresh does.
fn apply(rt: &mut Runtime<Lemur>, sessions: &[(&str, Activity)]) {
    let snap = Snapshot {
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
    };
    rt.update(|s, _| s.set_snapshot(snap));
    rt.frame(1.0 / 60.0);
}

/// The threads whose popup shows, in the order they popped up.
fn popups(rt: &Runtime<Lemur>) -> Vec<Key> {
    rt.state.popups()
}

/// By name: the popups of one snapshot come in no order of their own.
fn sorted(mut keys: Vec<Key>) -> Vec<Key> {
    keys.sort();
    keys
}

#[test]
fn finishing_out_of_sight_pops_up_a_notification() {
    // No terminal runs here: Show still opens a terminal that tries to attach, for real.
    let server = HcServer::new("rpopup");
    let mut rt = window(server.config());
    use Activity::{Idle, NeedsInput, Working};

    // In the active window: the session you're looking at doesn't pop up, one you aren't
    // does.
    rt.window_focus(true);
    rt.frame(1.0 / 60.0);
    rt.update(|s, _| s.active = Some(key("shown")));
    apply(&mut rt, &[("shown", Working), ("a", Working)]);
    apply(&mut rt, &[("shown", Idle), ("a", Idle)]);
    assert_eq!(popups(&rt), [key("a")], "you're looking at the one shown");
    let a = &rt.state.panels.notify.popups[0];
    assert_eq!((a.title.as_str(), a.caption.as_str(), a.asks), ("a finished", "local", false));

    // Away from the window, even the shown session pops up; needing input does too.
    rt.window_focus(false);
    rt.frame(1.0 / 60.0);
    apply(&mut rt, &[("shown", Working), ("a", Idle), ("b", Working)]);
    apply(&mut rt, &[("shown", Idle), ("a", Idle), ("b", NeedsInput)]);
    assert_eq!(popups(&rt)[0], key("a"));
    assert_eq!(sorted(popups(&rt)), [key("a"), key("b"), key("shown")]);
    let b = rt.state.panels.notify.popups.iter().find(|p| p.key == key("b")).unwrap();
    assert_eq!((b.title.as_str(), b.asks), ("b needs input", true));

    // A newer event for the same session replaces its popup, at the end.
    apply(&mut rt, &[("shown", Idle), ("a", Working), ("b", NeedsInput)]);
    apply(&mut rt, &[("shown", Idle), ("a", NeedsInput), ("b", NeedsInput)]);
    assert_eq!(popups(&rt).len(), 3);
    assert_eq!(popups(&rt)[2], key("a"));
    assert_eq!(rt.state.panels.notify.popups[2].title, "a needs input");

    // Show opens that session and closes only its popup.
    rt.update(|s, _| s.show_popup(key("a")));
    assert_eq!(rt.state.active, Some(key("a")));
    assert_eq!(sorted(popups(&rt)), [key("b"), key("shown")]);

    // Opening a session from the sidebar answers its popup too, and one that ends goes.
    rt.update(|s, _| s.open("local".into(), "b".into()));
    assert_eq!(popups(&rt), [key("shown")]);
    apply(&mut rt, &[("a", NeedsInput), ("b", NeedsInput)]);
    assert!(popups(&rt).is_empty(), "{:?}", popups(&rt));

    // Let the terminals' attach attempts end before the server cleans up.
    wait_for(&mut rt, 5.0, |s| s.terminals.values().all(|t| t.exited()));
}

/// Back in the window, the shown session's popup closes, as Zed's do, and the others
/// wait. A popup never has the keyboard to hand back: the overlay never takes it.
#[test]
fn coming_back_answers_the_shown_sessions_popup() {
    let mut rt = window(toml::from_str("").unwrap());
    rt.update(|s, _| s.active = Some(key("shown")));
    rt.window_focus(false);
    rt.frame(1.0 / 60.0);
    apply(&mut rt, &[("shown", Activity::Working), ("a", Activity::Working)]);
    apply(&mut rt, &[("shown", Activity::Idle), ("a", Activity::Idle)]);
    assert_eq!(sorted(popups(&rt)), [key("a"), key("shown")]);
    rt.window_focus(true);
    rt.frame(1.0 / 60.0);
    assert_eq!(popups(&rt), [key("a")], "you're back; a isn't answered yet");
}

fn popup(name: &str) -> Popup {
    Popup { key: key(name), title: format!("{name} finished"), caption: "api › main  ·  local".into(), asks: false }
}

/// The overlay's Show and Dismiss each answer their own popup, which goes at once.
#[test]
fn a_popups_buttons_answer_it() {
    let shown = Popups { shown: vec![popup("a"), popup("b")], dark: true, app: None };
    let env = Env { system_dark: true, scale: 2.0 };
    let mut rt = Runtime::new(shown, popups_view, (WIDTH, 2.0 * HEIGHT + GAP), env);
    let click = |rt: &mut Runtime<Popups>, at: &str| {
        // What just came in fades in: a press lands once it shows.
        for _ in 0..20 {
            rt.frame(1.0 / 60.0);
        }
        let (x, y) = rt.find(at).unwrap_or_else(|| panic!("no element keyed {at}"));
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    };
    rt.frame(1.0 / 60.0);
    assert!(rt.find("popup-title-local-b").is_some());
    click(&mut rt, "popup-dismiss-local-b");
    assert_eq!(rt.state.shown.iter().map(|p| p.key.clone()).collect::<Vec<_>>(), [key("a")]);
    click(&mut rt, "popup-show-local-a");
    assert!(rt.state.shown.is_empty());
}

/// A silenced thread makes no notifications, and a popup it made goes; what it's doing
/// shows as ever. herdr-core keeps that, and it can notify again.
#[test]
fn a_silenced_thread_makes_no_notifications() {
    let server = HcServer::new("rsilence");
    // Needs you when told to, then works, then needs you again. A program of its own, as a
    // shell in front would mean what it said is over.
    let script = r#"BEGIN { s[1] = "blocked"; s[2] = "working"; s[3] = "blocked" }
        { printf "\033]7501;state=%s:app=claude-code\033\\", s[NR]; fflush() }"#;
    let quiet = server.run("/", &["lemur.kind=claude", "lemur.silent=1"], &["awk", script]);
    let loud = server.run("/", &["lemur.kind=claude"], &["awk", script]);
    let mut rt = window(server.config());
    let state = |s: &Lemur, name: &str| sessions(&s.snap).find(|(_, t)| t.name == name).map(|(_, t)| (t.activity, t.silent));
    // Waits for `done`, fetching a snapshot every half second: tests don't tick.
    let until = |rt: &mut Runtime<Lemur>, done: &dyn Fn(&Lemur) -> bool| {
        let start = std::time::Instant::now();
        while start.elapsed().as_secs_f64() < 10.0 {
            rt.update(|s, _| s.refresh());
            if wait_for(rt, 0.5, done) {
                return true;
            }
        }
        false
    };
    let labelled = |t: &str| {
        let terminals = server.core().terminals();
        let t = terminals.iter().find(|x| x.address == t).unwrap();
        t.labels.contains_key(crate::snapshot::SILENT)
    };
    assert!(until(&mut rt, &|s| state(s, &loud).is_some() && state(s, &quiet).is_some()));
    server.hc(&["send", "--force", &quiet, "go"]);
    server.hc(&["send", "--force", &loud, "go"]);
    assert!(until(&mut rt, &|s| {
        state(s, &quiet) == Some((Activity::NeedsInput, true)) && state(s, &loud) == Some((Activity::NeedsInput, false))
    }));
    assert!(popups(&rt).contains(&key(&loud)));
    assert!(!popups(&rt).contains(&key(&quiet)), "silenced");

    // Silenced, its popup goes at once, and herdr-core keeps it.
    rt.update(|s, _| s.set_silent(key(&loud), true));
    assert!(!popups(&rt).contains(&key(&loud)));
    assert!(until(&mut rt, &|_| labelled(&loud)));
    assert_eq!(state(&rt.state, &loud), Some((Activity::NeedsInput, true)));

    // Allowed again, what it does next makes one.
    rt.update(|s, _| s.set_silent(key(&quiet), false));
    assert!(until(&mut rt, &|_| !labelled(&quiet)));
    server.hc(&["send", "--force", &quiet, "go"]);
    assert!(until(&mut rt, &|s| state(s, &quiet) == Some((Activity::Working, false))));
    server.hc(&["send", "--force", &quiet, "go"]);
    let k = key(&quiet);
    assert!(until(&mut rt, &|s| s.popups().contains(&k)));
}
