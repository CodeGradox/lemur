//! The meters: what they say of an answer and when they ask, then the sidebar's foot
//! driven through Rune's `Runtime`, the hosts' load with it.

use super::super::Lemur;
use super::super::agents::tests::{click, loaded};
use crate::gui::tests::{made_up, window};
use super::super::core::wait_for;
use super::{Meter, by_last_claude, describe, until};
use crate::config::Agent;
use crate::test_support::{HcServer, TempDir};
use crate::usage::{self, Limit, Usage};
use rune::Runtime;
use std::time::{Duration, Instant};

#[test]
fn reset_times_read_roughly() {
    assert_eq!(until(-5), "under a minute");
    assert_eq!(until(59), "under a minute");
    assert_eq!(until(45 * 60), "45m");
    assert_eq!(until(2 * 3600 + 13 * 60 + 30), "2h 13m");
    assert_eq!(until(3 * 86400 + 4 * 3600), "3d 4h");
}

#[test]
fn the_meter_shows_each_limit_or_why_not() {
    let now = usage::epoch("2026-11-01T02:47:00Z").unwrap();
    let used = Usage {
        limits: vec![
            Limit { label: "Session".into(), percent: 42.4, resets_at: usage::epoch("2026-11-01T05:00:00Z") },
            Limit { label: "Week · Fable".into(), percent: 81.0, resets_at: None },
        ],
        host: "agent".into(),
    };
    let shown = describe(Agent::Claude, Some(&Ok(used.clone())), now);
    assert_eq!(shown.right, "resets in 2h 13m");
    assert_eq!(shown.rows, [("Session".to_string(), 42.4), ("Week · Fable".to_string(), 81.0)]);
    assert_eq!(shown.brief, "Week · Fable 81%");
    assert_eq!(
        shown.tip,
        "Session: 42% used, resets in 2h 13m\nWeek · Fable: 81% used\n\
         From Claude Code's /usage on agent, every 10 minutes."
    );

    let none = Usage { limits: vec![], host: "agent".into() };
    let none = describe(Agent::Claude, Some(&Ok(none)), now);
    assert_eq!((none.right.as_str(), none.brief.as_str()), ("no limits", "no limits"));
    let failed = describe(Agent::Claude, Some(&Err("Claude Code isn't installed there".into())), now);
    assert_eq!(failed.right, "unavailable");
    assert_eq!(failed.brief, "unavailable");
    assert!(failed.rows.is_empty());
    assert!(failed.tip.contains("isn't installed"), "{}", failed.tip);
    assert_eq!(describe(Agent::Claude, None, now).right, "asking…");
}

#[test]
fn hosts_where_claude_ran_last_are_asked_first() {
    let hosts = vec![("mac".to_string(), None), ("agent".to_string(), Some(30)), ("box".to_string(), None), ("old".to_string(), Some(9000))];
    assert_eq!(by_last_claude(hosts), ["agent", "old", "mac", "box"]);
}

#[test]
fn the_meter_asks_when_its_time() {
    let now = Instant::now();
    let mut m = Meter::default();
    assert!(m.due(true, now, 0), "at first");
    m.asked = Some(now);
    assert!(!m.due(true, now + Duration::from_secs(60), 0));
    assert!(m.due(true, now + usage::EVERY, 0));
    assert!(!m.due(false, now + usage::EVERY, 0), "out of sight it waits longer");
    m.fails = 1;
    assert!(!m.due(true, now + usage::EVERY, 0), "after a failure too");
    m.asking = true;
    assert!(!m.due(true, now + usage::SLOWEST, 0), "one ask at a time");
}

/// Once a limit shown has reset, its use shown is over: asked again.
#[test]
fn the_meter_asks_again_once_a_limit_resets() {
    let now = Instant::now();
    let reset = usage::epoch("2026-11-07T10:00:00Z").unwrap();
    let mut m = Meter {
        shown: Some(Ok(Usage {
            limits: vec![Limit { label: "Week".into(), percent: 95.0, resets_at: Some(reset) }],
                host: "agent".into(),
        })),
        asked: Some(now),
        answered: Some(reset - 600),
        ..Default::default()
    };
    let soon = now + Duration::from_secs(60);
    assert!(!m.due(true, soon, reset - 1));
    assert!(m.due(true, soon, reset + 1), "it has reset");
    m.answered = Some(reset + 1);
    assert!(!m.due(true, soon, reset + 2), "asked since");
}

/// Settings turns a meter on or off, as its switch does.
fn set(rt: &mut Runtime<Lemur>, agent: Agent, on: bool) {
    rt.update(|s, _| {
        match agent {
            Agent::Claude => s.prefs.claude_usage = on,
        }
        s.follow_meter_setting();
    });
    rt.frame(1.0 / 60.0);
}

/// The meter's next answer, once it's in.
fn answer(rt: &mut Runtime<Lemur>, agent: Agent) -> Option<Result<Usage, String>> {
    let meter = move |s: &Lemur| match agent {
        Agent::Claude => s.panels.meters.claude.shown.clone(),
    };
    wait_for(rt, 10.0, |s| meter(s).is_some());
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    meter(&rt.state)
}

/// Asks again now, what it showed forgotten.
fn ask_again(rt: &mut Runtime<Lemur>, agent: Agent) {
    rt.update(|s, _| {
        match agent {
            Agent::Claude => s.panels.meters.claude.shown = None,
        }
        s.ask_usage(agent);
    });
}

/// Turned on in Settings, the sidebar's foot shows each limit the host's Claude command's
/// `/usage` lists; a Claude that can't answer says so; turned off, it's gone. Off, it never
/// asks.
#[test]
fn the_meter_follows_settings() {
    let server = HcServer::new("rmeter");
    let dir = TempDir::new("rmeter");
    let printed = dir.join("printed.jsonl");
    std::fs::write(&printed, usage::PRINTED).unwrap();
    let ran = dir.join("ran");
    let mut cfg = server.config();
    // Says it ran, and prints what /usage prints; `#` leaves out the arguments Lemur adds.
    cfg.hosts[0].claude = format!("touch {}; cat {} #", ran.display(), printed.display());
    let mut rt = loaded(cfg);
    assert!(!rt.state.claude_found(), "no Claude here yet");
    rt.update(|s, _| s.tick_meter());
    wait_for(&mut rt, 0.5, |_| false);
    assert!(!ran.exists(), "off, it doesn't ask");
    assert!(rt.find("usage").is_none());

    set(&mut rt, Agent::Claude, true);
    let usage = answer(&mut rt, Agent::Claude).expect("asked at once").expect("its limits");
    assert!(ran.exists());
    assert_eq!(usage.host, "local");
    let labels: Vec<&str> = usage.limits.iter().map(|l| l.label.as_str()).collect();
    assert_eq!(labels, ["Week", "Week · Fable"]);
    assert!(rt.find("usage-bars").is_some());

    // A click on "Claude" minimizes it to that line, and opens it again.
    click(&mut rt, "usage-header");
    assert!(rt.find("usage-bars").is_none());
    assert!(rt.state.cfg.minimized_usage, "kept");
    click(&mut rt, "usage-header");
    assert!(rt.find("usage-bars").is_some());
    assert!(!rt.state.cfg.minimized_usage);

    rt.update(|s, _| s.cfg.hosts[0].claude = "false".into());
    ask_again(&mut rt, Agent::Claude);
    let failed = answer(&mut rt, Agent::Claude).expect("asked").unwrap_err();
    assert_eq!(failed, "Claude Code didn't answer (on local)");
    assert!(rt.find("usage-bars").is_some(), "it says so");
    // A click there asks again.
    let fixed = format!("cat {} #", printed.display());
    rt.update(|s, _| s.cfg.hosts[0].claude = fixed);
    click(&mut rt, "usage-bars");
    assert!(wait_for(&mut rt, 10.0, |s| matches!(s.panels.meters.claude.shown, Some(Ok(_)))));

    set(&mut rt, Agent::Claude, false);
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.find("usage").is_none());
    assert!(rt.state.panels.meters.claude.shown.is_none());
}

/// Unavailable, the meter's line has a ↻ that asks again, minimized too, and says it's asking
/// meanwhile, the ↻ faint; pressing it leaves the meter as minimized as it was.
#[test]
fn an_unavailable_meter_asks_again_from_its_line() {
    let server = HcServer::new("rmeter-again");
    let dir = TempDir::new("rmeter-again");
    let printed = dir.join("printed.jsonl");
    std::fs::write(&printed, usage::PRINTED).unwrap();
    let mut cfg = server.config();
    cfg.hosts[0].claude = "false".into();
    let mut rt = loaded(cfg);
    let texts = |rt: &Runtime<Lemur>| rt.elements().into_iter().filter(|e| !e.exiting).filter_map(|e| e.text).collect::<Vec<_>>();
    set(&mut rt, Agent::Claude, true);
    assert!(answer(&mut rt, Agent::Claude).expect("asked").is_err());
    assert!(rt.find("usage-again").is_some());
    click(&mut rt, "usage-header");
    assert!(rt.state.cfg.minimized_usage);
    assert!(rt.find("usage-again").is_some(), "minimized too");

    // Slow to answer, so it's seen asking.
    rt.update(|s, _| s.cfg.hosts[0].claude = format!("sleep 1; cat {} #", printed.display()));
    click(&mut rt, "usage-again");
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.state.cfg.minimized_usage, "still minimized");
    assert!(texts(&rt).contains(&"asking…".to_string()), "{:?}", texts(&rt));
    assert!(rt.find("usage-again").is_some(), "it stays put");
    click(&mut rt, "usage-again");
    assert!(rt.state.cfg.minimized_usage, "pressed again while it asks, it doesn't open the meter");
    assert!(wait_for(&mut rt, 10.0, |s| matches!(s.panels.meters.claude.shown, Some(Ok(_)))));
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.find("usage-again").is_none(), "answered");
    assert!(!texts(&rt).contains(&"asking…".to_string()));
}

/// The sidebar's foot shows each host's CPU, from two readings, and its memory. A click on
/// its name minimizes it to that line, and opens it again.
#[test]
fn hosts_show_their_load() {
    let server = HcServer::new("rload");
    let mut rt = window(server.config());
    let read = |rt: &mut Runtime<Lemur>, busy, total| {
        let mut snap = made_up(&[]);
        snap.hosts[0].machine.load = Some(crate::machine::Load {
            busy,
            total,
            mem_total_kb: 4_194_304,
            mem_used_kb: 1_048_576,
            load: vec!["0.50".into(), "0.40".into(), "0.30".into()],
            ..Default::default()
        });
        rt.update(|s, _| s.set_snapshot(snap));
        for _ in 0..20 {
            rt.frame(1.0 / 60.0);
        }
    };
    read(&mut rt, 100, 1000);
    assert!(rt.find("machine-local").is_some());
    let tall = |rt: &Runtime<Lemur>| {
        let e = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("machine-local") && !e.exiting);
        // Where it's going: it eases there.
        e.expect("its load").target[3]
    };
    let open = tall(&rt);
    click(&mut rt, "load-header-local");
    assert_eq!(rt.state.cfg.minimized_hosts, ["local"]);
    assert!(tall(&rt) < open, "its bars are gone");
    assert!(rt.find("cpu-local").is_none());
    click(&mut rt, "load-header-local");
    assert!(rt.state.cfg.minimized_hosts.is_empty());
    assert_eq!(tall(&rt), open);
    let cpu = |rt: &Runtime<Lemur>| rt.state.loads()["local"].1;
    assert_eq!(cpu(&rt), None, "one reading");
    read(&mut rt, 200, 1400);
    assert_eq!(cpu(&rt), Some(25.0));
    // The same reading again, as terminals change between refreshes.
    read(&mut rt, 200, 1400);
    assert_eq!(cpu(&rt), Some(25.0));
}

/// Settings offers the meter once Lemur has seen Claude: running, or kept to resume.
#[test]
fn settings_hears_whether_claude_is_around() {
    let mut rt = window(toml::from_str("").unwrap());
    let found = |rt: &Runtime<Lemur>| rt.state.claude_found();
    rt.update(|s, _| s.set_snapshot(made_up(&[("shell", crate::snapshot::Activity::Unknown)])));
    assert!(!found(&rt));
    rt.update(|s, _| s.set_snapshot(made_up(&[("claude", crate::snapshot::Activity::Idle)])));
    assert!(found(&rt));
}
