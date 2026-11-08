//! The folder picker, driven through the window as `tests.rs` drives it.

use super::super::core::wait_for;
use super::super::projects::HostChoice;
use super::super::Lemur;
use super::{folder_path, split};
use crate::config::Host;
use crate::test_support::{HcServer, TempDir, git_in};
use rune::{Key as K, Mods, Runtime};
use crate::gui::tests::{cmd, press, type_text, window};

fn frames(rt: &mut Runtime<Lemur>, n: usize) {
    for _ in 0..n {
        rt.frame(1.0 / 60.0);
    }
}

#[test]
fn typed_paths_split_into_folder_and_name() {
    assert_eq!(split("~/projects/la"), ("~/projects/".into(), "la".into()));
    assert_eq!(split("~/"), ("~/".into(), "".into()));
    assert_eq!(split("~"), ("~/".into(), "".into()));
    assert_eq!(split("/"), ("/".into(), "".into()));
    assert_eq!(split("pro"), ("".into(), "pro".into()));
    assert_eq!(folder_path("~/projects/api/"), "~/projects/api");
    assert_eq!(folder_path("~/"), "~");
    assert_eq!(folder_path("/"), "/");
}

fn temp_repos() -> TempDir {
    let dir = TempDir::new("picker");
    for d in ["api/src", "web", "Docs", ".hidden"] {
        std::fs::create_dir_all(dir.join(d)).unwrap();
    }
    git_in(&dir.join("api"), &["init", "-q"]);
    dir
}

/// The picker over a window on `server`'s core, offering `agent` and `local`, this Mac
/// through that core, not the one in use. Its field has the keyboard.
fn picking(server: &HcServer) -> Runtime<Lemur> {
    let local = server.config().hosts.remove(0);
    let hosts = vec![
        HostChoice { host: Host::new("agent", "agent"), configured: true },
        HostChoice { host: local, configured: false },
    ];
    let mut rt = window(server.config());
    rt.update(|s, _| s.show_picker(hosts));
    frames(&mut rt, 4);
    rt
}

fn labels(rt: &Runtime<Lemur>) -> Vec<String> {
    rt.state.panels.picker.as_ref().map(|p| p.row_labels()).unwrap_or_default()
}

/// The folder typed is listed, or couldn't be, and nothing's being checked.
fn listed(rt: &mut Runtime<Lemur>) {
    let done = wait_for(rt, 10.0, |s| {
        s.panels.picker.as_ref().is_none_or(|p| !p.checking && p.listings.contains_key(&split(&p.input.text).0))
    });
    assert!(done, "listed");
}

/// Sets the path, as if typed, and waits for its folder's listing.
fn type_path(rt: &mut Runtime<Lemur>, path: &str) {
    rt.update(|s, _| s.picker_set_text(path));
    listed(rt);
}

/// The folders the window has, by host and path.
fn bases(s: &Lemur) -> Vec<(String, String)> {
    s.cfg.bases.iter().map(|b| (b.host.clone(), b.path.clone())).collect()
}

#[test]
fn picking_a_host_then_a_folder_adds_its_repo() {
    let dir = temp_repos();
    let base = dir.to_string_lossy().into_owned();
    let server = HcServer::new("rpick");
    let mut rt = picking(&server);
    assert_eq!(labels(&rt), ["agent", "local"]);
    // Filter to this Mac, and pick it with Enter.
    type_text(&mut rt, "loc");
    assert_eq!(labels(&rt), ["local"]);
    press(&mut rt, K::Enter, Mods::default());
    assert!(rt.state.panels.picker.as_ref().is_some_and(|p| p.host().is_some()), "this Mac picked");
    assert_eq!(rt.state.panels.picker.as_ref().unwrap().input.text, "~/");

    // Its folders show; hidden ones only when asked for.
    type_path(&mut rt, &format!("{base}/"));
    assert_eq!(labels(&rt), [format!("add {base}"), "api (git)".into(), "Docs".into(), "web".into()]);
    type_text(&mut rt, "w");
    assert_eq!(labels(&rt), [format!("add {base}/w"), "web".into()]);

    // Down selects web; Tab goes into it.
    press(&mut rt, K::Down, Mods::default());
    assert_eq!(rt.state.panels.picker.as_ref().unwrap().selected, 1);
    press(&mut rt, K::Tab, Mods::default());
    listed(&mut rt);
    assert_eq!(labels(&rt), [format!("add {base}/web")]);

    // web has no git, and is added as it is; the picker closes and hands the keyboard back.
    press(&mut rt, K::Enter, Mods::default());
    assert!(wait_for(&mut rt, 10.0, |s| s.panels.picker.is_none()), "closed once added");
    assert!(rt.state.keyboard_on_terminal());
    assert_eq!(bases(&rt.state), [("local".to_string(), format!("{base}/web"))]);

    // A folder inside a repository adds the repository.
    let mut rt = picking(&server);
    press(&mut rt, K::Down, Mods::default());
    press(&mut rt, K::Enter, Mods::default());
    type_path(&mut rt, &format!("{base}/api/src"));
    press(&mut rt, K::Enter, Mods::default());
    assert!(wait_for(&mut rt, 10.0, |s| s.panels.picker.is_none()), "closed once added");
    assert_eq!(bases(&rt.state), [("local".to_string(), format!("{base}/api"))]);

    // A folder that isn't there says so.
    let mut rt = picking(&server);
    press(&mut rt, K::Down, Mods::default());
    press(&mut rt, K::Enter, Mods::default());
    type_path(&mut rt, &format!("{base}/nope"));
    press(&mut rt, K::Enter, Mods::default());
    assert!(wait_for(&mut rt, 10.0, |s| s.panels.picker.as_ref().is_some_and(|p| p.error.is_some())), "an error");
    assert!(rt.find("picker-footer").is_some(), "shown");
    assert!(rt.state.cfg.bases.is_empty());

    press(&mut rt, K::Escape, Mods::default());
    assert!(rt.state.panels.picker.is_none());
}

/// Picked, a host herdr-core needs you to add is set up in a terminal, as Herdr's Add a
/// machine does.
#[test]
fn a_host_that_needs_you_is_set_up() {
    let server = HcServer::new("rpickset");
    server.far_config("pick-box");
    server.core().needed_you("pick-box");
    // What the set-up runs, rather than `hc machine add`.
    crate::actions::SET_UP.lock().unwrap().push(("pick-box".into(), vec!["sleep".into(), "600".into()]));
    let mut rt = picking(&server);
    type_text(&mut rt, "pick-box");
    assert_eq!(labels(&rt), ["pick-box"]);
    press(&mut rt, K::Enter, Mods::default());
    assert!(wait_for(&mut rt, 10.0, |s| s.panels.picker.is_none()), "the picker makes way");
    assert!(wait_for(&mut rt, 10.0, |s| s.peek.as_ref().is_some_and(|k| s.setups.contains_key(k))), "its set-up shows: {:?}", rt.state.status);
    assert_eq!(rt.state.setups.values().map(|h| h.name.as_str()).collect::<Vec<_>>(), ["pick-box"]);
}

#[test]
fn a_typed_destination_offers_to_connect() {
    let server = HcServer::new("rpicktype");
    let mut rt = picking(&server);
    type_text(&mut rt, "me@build.example.com");
    assert_eq!(labels(&rt), ["me@build.example.com"]);
    assert!(rt.find("picker-row-0").is_some());
}

/// ⌘O opens the picker and Esc closes it; a folder it adds joins the sidebar once.
#[test]
fn an_added_folder_joins_the_sidebar_once() {
    let server = HcServer::new("radd");
    let cfg = server.config();
    let local = HostChoice { host: cfg.hosts[0].clone(), configured: true };
    let mut rt = window(cfg);
    press(&mut rt, K::Command('o'), cmd());
    assert!(rt.state.panels.picker.is_some() && !rt.state.keyboard_on_terminal());
    frames(&mut rt, 4);
    press(&mut rt, K::Escape, Mods::default());
    assert!(rt.state.panels.picker.is_none());
    assert!(rt.state.keyboard_on_terminal(), "the keyboard's back");

    for _ in 0..2 {
        rt.update(|s, _| s.add_folder(local.clone(), "/srv/api".into()));
    }
    assert_eq!(bases(&rt.state), [("local".to_string(), "/srv/api".to_string())]);
    assert_eq!(rt.state.cfg.hosts.len(), 1);
    let status = rt.state.status.as_ref().map(|(s, _)| s.as_str());
    assert_eq!(status, Some("/srv/api is in the sidebar already"));
    assert!(wait_for(&mut rt, 10.0, |s| s.snap.hosts.first().is_some_and(|h| h.bases.len() == 1)), "the refresh lists it");
}

/// ⌘O and ⌘D wait under what's over the window, as ⌘P does: the folder picker would open
/// beneath the spotlight and take the keyboard, and dictation would listen with its bar
/// hidden.
#[test]
fn the_picker_and_dictation_wait_under_the_spotlight() {
    let mut rt = window(toml::from_str("").unwrap());
    press(&mut rt, K::Command('p'), cmd());
    frames(&mut rt, 4);
    press(&mut rt, K::Command('o'), cmd());
    press(&mut rt, K::Command('d'), cmd());
    assert!(rt.state.prompt.is_some());
    assert!(rt.state.panels.picker.is_none(), "no picker under it");
    assert!(rt.state.panels.dictation.is_none(), "no dictation under it");
}
