//! The meters: what they say of an answer and when they ask, then the sidebar's foot
//! driven through Rune's `Runtime`, the hosts' load with it.

use super::super::Lemur;
use super::super::agents::tests::{click, loaded};
use crate::gui::tests::{made_up, window};
use super::super::core::wait_for;
use super::{Meter, by_last_claude, describe, short, until};
use crate::config::Agent;
use crate::test_support::{HcServer, TempDir};
use crate::usage::{self, Limit, Spent, Tokens, Usage};
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
        spent: vec![],
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

    let none = Usage { limits: vec![], spent: vec![], host: "agent".into() };
    let none = describe(Agent::Claude, Some(&Ok(none)), now);
    assert_eq!((none.right.as_str(), none.brief.as_str()), ("no limits", "no limits"));
    let failed = describe(Agent::Claude, Some(&Err("Claude Code isn't installed there".into())), now);
    assert_eq!(failed.right, "unavailable");
    assert_eq!(failed.brief, "unavailable");
    assert!(failed.rows.is_empty());
    assert!(failed.tip.contains("isn't installed"), "{}", failed.tip);
    assert_eq!(describe(Agent::Claude, None, now).right, "asking…");
    // Codex's says it's Codex's.
    let shown = describe(Agent::Codex, Some(&Ok(used)), now);
    assert!(shown.tip.ends_with("From Codex's app server on agent, every 10 minutes."));
    let failed = describe(Agent::Codex, Some(&Err("no plan".into())), now);
    assert_eq!(failed.tip, "Couldn't fetch usage from Codex: no plan.");
}

/// On an API key: the tokens used, with no bars, as it has no limits.
#[test]
fn the_meter_shows_what_a_key_used() {
    let tokens = |input, cached, output| Tokens { input, cached, output };
    let used = Usage {
        limits: vec![],
        spent: vec![
            Spent { label: "24 hours", tokens: tokens(4_588_020, 4_391_280, 32_215) },
            Spent { label: "7 days", tokens: tokens(5_000_000, 0, 1_000) },
        ],
        host: "mac".into(),
    };
    let shown = describe(Agent::Codex, Some(&Ok(used)), 0);
    assert_eq!(shown.right, "API key");
    assert_eq!(shown.brief, "24 hours 229k");
    assert!(shown.rows.is_empty());
    assert_eq!(
        shown.totals,
        [("24 hours".to_string(), "229k + 4.4M cached".to_string()), ("7 days".to_string(), "5.0M".to_string())]
    );
    assert_eq!(
        shown.tip,
        "24 hours: 229k tokens, 197k input (+ 4.4M cached) and 32k output\n\
         7 days: 5.0M tokens, 5.0M input (+ 0 cached) and 1.0k output\n\
         From Codex's session logs on mac, every 10 minutes. An API key has no plan limits to show."
    );
}

#[test]
fn counts_read_roughly() {
    assert_eq!(short(0), "0");
    assert_eq!(short(950), "950");
    assert_eq!(short(1_234), "1.2k");
    assert_eq!(short(229_000), "229k");
    assert_eq!(short(4_620_235), "4.6M");
    assert_eq!(short(21_000_000), "21M");
    assert_eq!(short(1_300_000_000), "1.3B");
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
            spent: vec![],
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
            Agent::Codex => s.prefs.codex_usage = on,
        }
        s.follow_meter_setting();
    });
    rt.frame(1.0 / 60.0);
}

/// The meter's next answer, once it's in.
fn answer(rt: &mut Runtime<Lemur>, agent: Agent) -> Option<Result<Usage, String>> {
    let meter = move |s: &Lemur| match agent {
        Agent::Claude => s.panels.meters.claude.shown.clone(),
        Agent::Codex => s.panels.meters.codex.shown.clone(),
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
            Agent::Codex => s.panels.meters.codex.shown = None,
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

/// Codex's usage has a meter of its own: off, it doesn't ask; on, it asks Codex and shows
/// what it says, beside Claude's, or why it can't.
#[test]
fn the_sidebar_shows_how_much_of_codexs_plan_is_used() {
    let server = HcServer::new("rcodex-meter");
    let dir = TempDir::new("rcodex-meter");
    let printed = dir.join("answer.jsonl");
    let answer_line = r#"{"id":2,"result":{"rateLimits":{"primary":{"usedPercent":25,"windowDurationMins":300,"resetsAt":1793660000},"secondary":{"usedPercent":61,"windowDurationMins":10080,"resetsAt":1794000000}}}}"#;
    std::fs::write(&printed, format!("{answer_line}\n")).unwrap();
    let ran = dir.join("ran");
    let fake = dir.join("fake-codex");
    // Says it ran, prints what Codex answers, and stays until its stdin ends.
    std::fs::write(&fake, format!("#!/bin/sh\ntouch {}\ncat {}\ncat >/dev/null\n", ran.display(), printed.display())).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut cfg = server.config();
    cfg.hosts[0].codex = fake.to_string_lossy().into_owned();
    cfg.hosts[0].claude = "false".into();
    let mut rt = loaded(cfg);
    assert!(!rt.state.codex_found(), "none yet");
    rt.update(|s, _| s.tick_meter());
    wait_for(&mut rt, 0.5, |_| false);
    assert!(!ran.exists(), "off, it doesn't ask");
    assert!(rt.find("codex-usage").is_none());

    set(&mut rt, Agent::Codex, true);
    let usage = answer(&mut rt, Agent::Codex).expect("asked at once").expect("its limits");
    assert!(ran.exists());
    let labels: Vec<&str> = usage.limits.iter().map(|l| l.label.as_str()).collect();
    assert_eq!(labels, ["5 hours", "Week"]);
    assert!(rt.find("codex-usage-bars").is_some());
    assert!(rt.find("usage").is_none(), "Claude's stays off");

    // With both on, Claude's is above.
    set(&mut rt, Agent::Claude, true);
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    let top = |rt: &Runtime<Lemur>, key: &str| rt.find(key).unwrap_or_else(|| panic!("{key}")).1;
    assert!(top(&rt, "usage") < top(&rt, "codex-usage"));
    set(&mut rt, Agent::Claude, false);

    click(&mut rt, "codex-usage-header");
    assert!(rt.find("codex-usage-bars").is_none());
    assert!(rt.state.cfg.minimized_codex_usage);
    assert!(!rt.state.cfg.minimized_usage, "its own");

    // On an API key it has no plan: the tokens in its logs, without bars.
    let logs = server.codex_home().join("sessions");
    std::fs::create_dir_all(&logs).unwrap();
    let at = usage::rfc3339(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64 - 60);
    std::fs::write(
        logs.join("rollout-1.jsonl"),
        format!(
            r#"{{"timestamp":"{at}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"total_tokens":60}},"last_token_usage":{{"input_tokens":50,"cached_input_tokens":20,"output_tokens":10,"total_tokens":60}}}}}}}}"#
        ) + "\n",
    )
    .unwrap();
    std::fs::write(
        &printed,
        r#"{"error":{"code":-32600,"message":"chatgpt authentication required to read rate limits"},"id":2}"#.to_string() + "\n",
    )
    .unwrap();
    // Open again: it's elsewhere, minimized.
    click(&mut rt, "codex-usage-header");
    assert!(!rt.state.cfg.minimized_codex_usage);
    ask_again(&mut rt, Agent::Codex);
    let used = answer(&mut rt, Agent::Codex).expect("asked").expect("what it used");
    assert!(used.limits.is_empty());
    assert_eq!(used.spent[0].tokens.total(), 40);
    assert!(rt.find("codex-usage-bars").is_some(), "it says so");
    assert!(rt.state.status.is_none(), "no failure");

    // A host without it: it says so, and Lemur doesn't fall over.
    rt.update(|s, _| s.cfg.hosts[0].codex = "lemur-has-no-codex".into());
    ask_again(&mut rt, Agent::Codex);
    let failed = answer(&mut rt, Agent::Codex).expect("asked").unwrap_err();
    assert_eq!(failed, "Codex isn't installed there (on local)");

    set(&mut rt, Agent::Codex, false);
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.find("codex-usage").is_none());
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

/// Settings offers a meter once Lemur has seen its agent: Claude running, or kept to resume;
/// Codex running.
#[test]
fn settings_hears_which_agents_are_around() {
    let mut rt = window(toml::from_str("").unwrap());
    let found = |rt: &Runtime<Lemur>| (rt.state.claude_found(), rt.state.codex_found());
    rt.update(|s, _| s.set_snapshot(made_up(&[("shell", crate::snapshot::Activity::Unknown)])));
    assert_eq!(found(&rt), (false, false));
    rt.update(|s, _| s.set_snapshot(made_up(&[("claude", crate::snapshot::Activity::Idle)])));
    assert_eq!(found(&rt), (true, false));
    let mut snap = made_up(&[("codex", crate::snapshot::Activity::Unknown)]);
    snap.hosts[0].other[0].codex = true;
    rt.update(|s, _| s.set_snapshot(snap));
    assert_eq!(found(&rt), (false, true));
}
