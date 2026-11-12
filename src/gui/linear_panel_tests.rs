//! The Linear panel, driven through Rune's `Runtime` with a stand-in for Linear's API
//! (`FakeLinear`), and a herdr-core of the test's own where a thread is pinned.

use super::super::core::wait_for;
use super::super::links_panel::tests::{click, frames, press, type_text, window};
use crate::gui::tests::{key, open, refreshing};
use super::super::{Lemur, Right};
use super::Account;
use crate::config::{ClaudeThread, Config};
use crate::linear::{Linear, Store};
use crate::snapshot;
use crate::test_support::{FakeLinear, HcServer, Request, TempDir, wait_until};
use rune::{Key as K, Runtime, TextEdit};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

/// A stand-in Linear answering as `answer` says, signed in, its sign-in kept in `dir`.
fn signed_in(dir: &TempDir, answer: impl Fn(&Request) -> (u16, String) + Send + 'static) -> (FakeLinear, Linear) {
    let api = FakeLinear::new(answer);
    let linear = Linear::at(&api.url, Store::File(dir.join("tokens")));
    linear.sign_in_as("a").unwrap();
    (api, linear)
}

/// The window with the Linear panel out, on a stand-in Linear with one issue per search
/// term, signed in.
fn panel(tag: &str) -> (FakeLinear, TempDir, Runtime<Lemur>) {
    let dir = TempDir::new(tag);
    let (api, linear) = signed_in(&dir, |r| {
        let body: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        let term = body["variables"]["term"].as_str().unwrap_or("mine");
        let issue = json!({
            "identifier": "LAFT-1", "title": term, "url": "https://linear.app/x/issue/LAFT-1",
            "branchName": "b", "priorityLabel": "High",
            "state": { "name": "Todo", "color": "#aaaaaa", "type": "unstarted" },
            "assignee": null,
        });
        let query = body["query"].as_str().unwrap();
        let data = if query.contains("searchIssues") {
            json!({ "searchIssues": { "nodes": [issue] } })
        } else if query.contains("assignedIssues") {
            json!({ "viewer": { "assignedIssues": { "nodes": [issue] } } })
        } else {
            json!({ "viewer": { "name": "M" }, "organization": { "name": "L" } })
        };
        (200, json!({ "data": data }).to_string())
    });
    let cfg: Config = toml::from_str("").expect("an empty config");
    let mut rt = window(cfg);
    rt.state.panels.linear.linear = linear;
    rt.update(|s, _| s.toggle_right(Right::Linear));
    assert!(wait_for(&mut rt, 5.0, |s| s.panels.linear.signed_in()), "signed in");
    (api, dir, rt)
}

/// Typed back to what was found, the panel shows what was found, not what a search for
/// the longer term answers.
#[test]
fn typed_back_the_search_shows_what_was_found() {
    let (_api, _dir, mut rt) = panel("linear-search");
    let type_in = |rt: &mut Runtime<Lemur>, text: &str| {
        rt.update(|s, _| {
            s.panels.linear.query = TextEdit::new(text);
            s.linear_search();
        });
    };
    let found = |s: &Lemur| {
        let p = &s.panels.linear;
        let titles = p.found.as_ref().and_then(|(_, f)| f.as_ref().ok()).map(|f| f.iter().map(|i| i.title.clone()).collect::<Vec<_>>());
        (p.searching, titles)
    };
    type_in(&mut rt, "lease");
    assert!(found(&rt.state).0, "searching once typing pauses");
    assert!(wait_for(&mut rt, 5.0, |s| found(s) == (false, Some(vec!["lease".into()]))));
    type_in(&mut rt, "leasex");
    assert!(found(&rt.state).0, "searching");
    type_in(&mut rt, "lease");
    assert_eq!(found(&rt.state), (false, Some(vec!["lease".to_string()])));
    let _ = wait_for(&mut rt, 1.0, |_| false);
    assert_eq!(found(&rt.state), (false, Some(vec!["lease".to_string()])), "not leasex's");
    frames(&mut rt, 4);
    assert!(rt.find("linear-issue-0").is_some());
}

/// Signed out and in again, what Linear said before isn't shown.
#[test]
fn a_new_sign_in_forgets_what_linear_said() {
    let (_api, _dir, mut rt) = panel("linear-forget");
    assert!(wait_for(&mut rt, 5.0, |s| s.panels.linear.mine.is_some()), "yours");
    rt.update(|s, _| s.set_linear_account(Account::SignedOut(None)));
    assert!(rt.state.panels.linear.mine.is_none());
    frames(&mut rt, 4);
    assert!(rt.find("linear-sign-in-button").is_some(), "it asks you to sign in");
}

/// A sign-in started while Lemur still checks the one it kept isn't undone when the check
/// ends: its Cancel stays.
#[test]
fn a_check_ending_leaves_a_sign_in_under_way() {
    let cfg: Config = toml::from_str("").expect("an empty config");
    let mut rt = window(cfg);
    let account = |s: &Lemur| s.panels.linear.account.clone();
    rt.update(|s, _| s.linear_checked(Account::SignedOut(None)));
    assert!(matches!(account(&rt.state), Account::SignedOut(_)));
    rt.update(|s, _| {
        s.panels.linear.account = Account::Checking;
        s.set_linear_account(Account::SigningIn(Arc::new(AtomicBool::new(false))));
        s.linear_checked(Account::SignedOut(None));
    });
    assert!(matches!(account(&rt.state), Account::SigningIn(_)));
}

/// Clicked, the search field has the keyboard; Esc, a click elsewhere or hiding the
/// panel gives it back to the thread on show.
#[test]
fn the_search_field_gives_the_keyboard_back() {
    let (_api, _dir, mut rt) = panel("linear-keys");
    let has_it = |rt: &mut Runtime<Lemur>| {
        frames(rt, 2);
        !rt.state.keyboard_on_terminal()
    };
    click(&mut rt, "linear-search");
    assert!(has_it(&mut rt), "the field has it");
    press(&mut rt, K::Escape);
    assert!(!has_it(&mut rt), "Esc");
    click(&mut rt, "linear-search");
    assert!(has_it(&mut rt));
    click(&mut rt, "linear-head");
    assert!(!has_it(&mut rt), "a click elsewhere");
    click(&mut rt, "linear-search");
    assert!(has_it(&mut rt));
    rt.update(|s, _| s.toggle_right(Right::Linear));
    assert!(!has_it(&mut rt), "hidden");
    rt.update(|s, _| s.toggle_right(Right::Linear));
    assert!(!has_it(&mut rt), "shown again, it doesn't take it");
}

/// The Linear panel lists your issues, and a click pins one to the thread on show:
/// herdr-core keeps it, and the sidebar shows it. Send to agent pastes its link at the
/// thread's prompt; Unpin unpins it. A link to an issue pasted into a thread with none
/// pins that one.
#[test]
fn linear_issues_pin_to_the_thread_on_show() {
    let dir = TempDir::new("linear-gui");
    let (api, linear) = signed_in(&dir, |r| {
        let issue = json!({
            "identifier": "LAFT-1",
            "title": "Fix lease rounding",
            "url": "https://linear.app/laft/issue/LAFT-1/fix",
            "branchName": "magnus/laft-1-fix",
            "priorityLabel": "High",
            "description": "Round *down*.",
            "state": { "name": "Todo", "color": "#e2e2e2", "type": "unstarted" },
            "assignee": { "name": "Magnus" },
        });
        let data = if r.body.contains("assignedIssues") {
            json!({ "viewer": { "assignedIssues": { "nodes": [issue] } } })
        } else if r.body.contains("issue(id") {
            json!({ "issue": issue })
        } else {
            json!({ "viewer": { "name": "Magnus" }, "organization": { "name": "Laft" } })
        };
        (200, json!({ "data": data }).to_string())
    });
    let server = HcServer::new("lingui");
    let t = server.run("/", &["lemur.label=fix"], &["cat"]);
    let mut rt = window(server.config());
    rt.state.panels.linear.linear = linear;
    rt.state.refresh();
    assert!(wait_for(&mut rt, 10.0, |s| s.loaded), "the first snapshot");
    assert!(refreshing(&mut rt, 5.0, |s| s.session_of(&key(&t)).is_some()), "listed");
    open(&mut rt, &t);
    assert!(wait_for(&mut rt, 10.0, |s| s.terminals.get(&key(&t)).is_some_and(|v| v.painted)), "painted");
    // Hidden, it asks Linear nothing; signed in, Lemur asked who for.
    assert!(wait_for(&mut rt, 5.0, |s| s.panels.linear.signed_in()), "signed in");
    let asked = api.requests();
    assert!(asked.iter().any(|r| r.body.contains("viewer { name }")));
    assert!(!asked.iter().any(|r| r.body.contains("assignedIssues")));

    rt.update(|s, _| s.toggle_right(Right::Linear));
    let pinned = |s: &Lemur| s.session_of(&key(&t)).and_then(|x| x.issue.clone());
    let kept = || {
        server.core().terminals().into_iter().find(|x| x.address == t).and_then(|x| x.labels.get(snapshot::ISSUE).cloned())
    };
    assert!(wait_for(&mut rt, 5.0, |s| s.panels.linear.mine.is_some()), "your issues");
    assert_eq!(rt.state.panels.linear.shown().cloned(), Some((key(&t), "fix".to_string(), None)));
    click(&mut rt, "linear-issue-0");
    assert_eq!(pinned(&rt.state).as_deref(), Some("LAFT-1"));
    assert_eq!(rt.state.panels.linear.shown().and_then(|s| s.2.clone()).as_deref(), Some("LAFT-1"));
    assert!(wait_until(Duration::from_secs(5), || kept().as_deref() == Some("LAFT-1")), "herdr-core keeps it");

    assert!(wait_for(&mut rt, 5.0, |s| s.panels.linear.issues.get("LAFT-1").is_some_and(|(_, i)| i.is_ok())), "its card");
    click(&mut rt, "linear-send");
    assert!(
        wait_until(Duration::from_secs(5), || {
            rt.frame(1.0 / 60.0);
            server.hc(&["read", &t]).contains("https://linear.app/laft/issue/LAFT-1/fix")
        }),
        "{}",
        server.hc(&["read", &t])
    );
    // The thread has the keyboard: what's typed shows there.
    assert!(rt.state.keyboard_on_terminal());
    type_text(&mut rt, "typed");
    assert!(
        wait_until(Duration::from_secs(5), || {
            rt.frame(1.0 / 60.0);
            server.hc(&["read", &t]).contains("typed")
        }),
        "{}",
        server.hc(&["read", &t])
    );
    click(&mut rt, "linear-unpin");
    assert_eq!(pinned(&rt.state), None);
    assert!(wait_until(Duration::from_secs(5), || kept().is_none()), "herdr-core lets it go");

    // Pasted: pinned, but not over one pinned already.
    let k = key(&t);
    rt.update(|s, _| s.pasted(&k, "see https://linear.app/laft/issue/LAFT-9/rounding"));
    assert_eq!(pinned(&rt.state).as_deref(), Some("LAFT-9"));
    assert!(wait_until(Duration::from_secs(5), || kept().as_deref() == Some("LAFT-9")));
    rt.update(|s, _| s.pasted(&k, "and https://linear.app/laft/issue/LAFT-10"));
    assert_eq!(pinned(&rt.state).as_deref(), Some("LAFT-9"));
    rt.update(|s, _| s.pasted(&k, "no link"));
    assert_eq!(pinned(&rt.state).as_deref(), Some("LAFT-9"));
    assert!(api.requests().iter().all(|r| r.authorization.as_deref() == Some("Bearer a")));

    // Kept to resume, the thread is pinned again then, Claude running in it or not.
    let thread = ClaudeThread {
        host: "local".into(),
        session: t.clone(),
        path: "/".into(),
        claude_session: crate::actions::random_uuid(),
        label: None,
        issue: None,
    };
    rt.update(|s, _| {
        s.keep_thread(thread);
        s.pin_issue(k.clone(), Some("LAFT-12".into()));
    });
    assert_eq!(rt.state.cfg.threads[0].issue.as_deref(), Some("LAFT-12"));
}

/// Settings' button follows the sign-in. Sign in with Linear opens Linear's page in the
/// browser and waits for you, Cancel ends the wait, and signed in, Sign out forgets it.
#[test]
fn settings_signs_in_and_out() {
    let cfg: Config = toml::from_str("").expect("an empty config");
    let mut rt = window(cfg);
    rt.update(|s, _| s.linear_checked(Account::SignedOut(None)));
    assert_eq!(rt.state.linear_account_status(), ("Not signed in".to_string(), None));
    let (label, run) = rt.state.linear_account_action();
    assert_eq!(label, "Sign in with Linear");
    rt.update(|s, _| run(s));
    frames(&mut rt, 2);
    let opened = rt.take_url_requests();
    assert!(
        opened.len() == 1 && opened[0].starts_with("https://linear.app/oauth/authorize?"),
        "{opened:?}, {:?}",
        rt.state.linear_account_status()
    );
    assert_eq!(rt.state.linear_account_status().0, "Waiting for you in the browser…");

    let (label, run) = rt.state.linear_account_action();
    assert_eq!(label, "Cancel");
    rt.update(|s, _| run(s));
    assert!(wait_for(&mut rt, 5.0, |s| matches!(s.panels.linear.account, Account::SignedOut(None))), "called off");

    let account = crate::linear::Account { name: "M".into(), workspace: "L".into() };
    rt.update(|s, _| s.set_linear_account(Account::SignedIn(Some(account))));
    assert_eq!(rt.state.linear_account_status(), ("M · L".to_string(), None));
    let (label, run) = rt.state.linear_account_action();
    assert_eq!(label, "Sign out");
    rt.update(|s, _| run(s));
    assert_eq!(rt.state.linear_account_status().0, "Not signed in");
}
