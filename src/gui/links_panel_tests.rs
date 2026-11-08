//! The Links panel, driven through Rune's `Runtime` as `tests.rs` drives the window,
//! against a herdr-core of the test's own, whose host's Claude Code logs the tests write.
//! The Linear panel's tests use these helpers too.

use super::super::core::wait_for;
use super::super::{Lemur, Right, on_key, poll, view};
use super::LinksPanel;
use crate::config::{Base, Config};
use crate::gh::State;
use crate::test_support::{HcServer, TempDir, fake_gh, git_in};
use rune::{Env, Key as K, KeyEvent, Mods, Runtime};
use std::path::Path;
use crate::gui::tests::{key, open, refreshing};

const ONE: &str = "1hJWkh1hjNLMZjGxLk1zCD";
const TWO: &str = "2VWjSVh3UGph6KPNmbpbzY";

/// The window on `cfg`, its first snapshot not fetched yet.
pub(in crate::gui) fn window(cfg: Config) -> Runtime<Lemur> {
    let env = Env { system_dark: true, scale: 2.0 };
    let mut rt = Runtime::new(Lemur::new(cfg, None, None), view, (1400.0, 900.0), env);
    rt.on_key = Some(on_key);
    rt.poll = Some(poll);
    rt.frame(0.0);
    rt
}

/// The window on `cfg`, once its first snapshot is in.
pub(in crate::gui) fn loaded(cfg: Config) -> Runtime<Lemur> {
    let mut rt = window(cfg);
    rt.state.refresh();
    assert!(wait_for(&mut rt, 10.0, |s| s.loaded), "the first snapshot");
    rt
}

pub(in crate::gui) fn frames(rt: &mut Runtime<Lemur>, n: usize) {
    for _ in 0..n {
        rt.frame(1.0 / 60.0);
    }
}

pub(in crate::gui) fn click(rt: &mut Runtime<Lemur>, key: &str) {
    // What just came in fades in: a press lands once it shows.
    frames(rt, 20);
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element keyed {key}"));
    rt.pointer_move(x, y);
    frames(rt, 2);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    frames(rt, 4);
}

pub(in crate::gui) fn press(rt: &mut Runtime<Lemur>, key: K) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    frames(rt, 2);
}

pub(in crate::gui) fn type_text(rt: &mut Runtime<Lemur>, text: &str) {
    for c in text.chars() {
        let shift = c.is_ascii_uppercase();
        rt.key(KeyEvent { key: K::Text(c.to_string()), mods: Mods { shift, ..Mods::default() } });
    }
    frames(rt, 2);
}

/// A log line of an agent's linking to artifact `id` as `title`, at `at`.
fn line(id: &str, title: &str, at: &str) -> String {
    format!("{{\"message\":{{\"content\":\"[{title}](https://claude.ai/artifact/{id})\"}},\"type\":\"assistant\",\"timestamp\":\"{at}\"}}\n")
}

/// Where Claude Code keeps the logs of folder `path` on the test's host.
fn logs_of(server: &HcServer, path: &str) -> std::path::PathBuf {
    let folder: String = path.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let logs = server.claude_config().join("projects").join(folder);
    std::fs::create_dir_all(&logs).unwrap();
    logs
}

/// The window with the Links panel out, keeping its list in `dir`.
fn with_links(server: &HcServer, dir: &Path) -> Runtime<Lemur> {
    let mut rt = window(server.config());
    rt.state.panels.links = LinksPanel::new(Some(dir));
    rt.update(|s, _| s.toggle_right(Right::Links));
    rt
}

/// Shows folder `path` of the test's host in the panel, called `label`.
fn show(rt: &mut Runtime<Lemur>, server: &HcServer, path: &str, label: &str) {
    let host = server.config().hosts.remove(0);
    rt.update(|s, _| s.links_show(host, path.into(), label.into()));
}

/// Waits for the panel to have read its folder, and to list `names`.
fn lists(rt: &mut Runtime<Lemur>, names: &[&str]) -> bool {
    wait_for(rt, 10.0, |s| {
        let (loading, got) = s.panels.links.listed();
        !loading && got == names
    })
}

fn repo_at(path: &Path) -> String {
    std::fs::create_dir(path).unwrap();
    git_in(path, &["init", "-q", "-b", "main"]);
    git_in(path, &["commit", "-q", "--allow-empty", "-m", "init"]);
    path.to_string_lossy().into_owned()
}

/// An artifact's pencil asks for a name in its row: Esc leaves it as it was, Enter keeps
/// what's typed, and emptied it goes back to what the agent called it.
#[test]
fn an_artifact_can_be_given_a_name() {
    let server = HcServer::new("lkname");
    let dir = TempDir::new("lkname");
    std::fs::write(logs_of(&server, "/srv/api").join("a.jsonl"), line(ONE, "Handover", "2026-11-01")).unwrap();
    let mut rt = with_links(&server, &dir);
    show(&mut rt, &server, "/srv/api", "api");
    assert!(lists(&mut rt, &["Handover"]), "{:?}", rt.state.panels.links.listed());

    click(&mut rt, "links-artifact-rename-0");
    assert!(rt.find("links-renaming-0").is_some(), "it asks");
    type_text(&mut rt, "Not this");
    press(&mut rt, K::Escape);
    assert!(rt.find("links-renaming-0").is_none());
    assert!(lists(&mut rt, &["Handover"]));

    // What's typed takes the place of what was there.
    click(&mut rt, "links-artifact-rename-0");
    type_text(&mut rt, "My doc");
    press(&mut rt, K::Enter);
    assert!(lists(&mut rt, &["My doc"]), "{:?}", rt.state.panels.links.listed());
    let kept = std::fs::read_to_string(dir.join("links.json")).unwrap();
    assert!(kept.contains("My doc"), "{kept}");

    // Emptied, it goes back to what the agent called it.
    click(&mut rt, "links-artifact-rename-0");
    press(&mut rt, K::Backspace);
    press(&mut rt, K::Enter);
    assert!(lists(&mut rt, &["Handover"]), "{:?}", rt.state.panels.links.listed());
}

#[test]
fn another_folder_doesnt_wait_for_the_one_being_read() {
    let server = HcServer::new("lkwait");
    let dir = TempDir::new("lkwait");
    let mut rt = with_links(&server, &dir);
    show(&mut rt, &server, "/srv/slow", "slow");
    show(&mut rt, &server, "/srv/next", "next");
    let reading = rt.state.panels.links.loading.as_ref().map(|(_, path)| path.clone());
    assert_eq!(reading.as_deref(), Some("/srv/next"), "asked at once");
}

/// Links in a worktree's logs on its host list by themselves, each to its own worktree,
/// and open with a click. One taken off stays off, the next time Lemur starts too, though
/// the log still has it.
#[test]
fn links_list_by_themselves_and_stay_off_once_removed() {
    let server = HcServer::new("lkpanel");
    let dir = TempDir::new("lkpanel");
    let logs = logs_of(&server, "/srv/api");
    std::fs::write(logs.join("a.jsonl"), line(ONE, "Handover", "2026-11-01")).unwrap();
    let mut rt = with_links(&server, &dir);
    show(&mut rt, &server, "/srv/api", "api");
    assert!(lists(&mut rt, &["Handover"]), "{:?}", rt.state.panels.links.listed());

    // A click on it opens it.
    click(&mut rt, "links-artifact-0");
    assert_eq!(rt.take_url_requests(), [format!("https://claude.ai/artifact/{ONE}")]);

    // A new link, in the log since the last look, goes on top.
    std::fs::OpenOptions::new()
        .append(true)
        .open(logs.join("a.jsonl"))
        .and_then(|mut log| std::io::Write::write_all(&mut log, line(TWO, "Report", "2026-11-02").as_bytes()))
        .unwrap();
    rt.update(|s, _| s.refresh_links());
    assert!(lists(&mut rt, &["Report", "Handover"]), "{:?}", rt.state.panels.links.listed());

    // On its ×, it goes, and doesn't open.
    click(&mut rt, "links-artifact-remove-0");
    assert_eq!(rt.state.panels.links.listed().1, ["Handover"]);
    assert!(rt.take_url_requests().is_empty(), "it didn't open");

    // Read from the start again, and by a new Lemur: it stays off.
    rt.update(|s, _| {
        s.panels.links.store.place_mut("local", "/srv/api").looked = None;
        s.refresh_links();
    });
    assert!(lists(&mut rt, &["Handover"]), "{:?}", rt.state.panels.links.listed());
    let mut again = with_links(&server, &dir);
    show(&mut again, &server, "/srv/api", "api");
    assert!(lists(&mut again, &["Handover"]), "{:?}", again.state.panels.links.listed());

    // Another worktree has its own.
    show(&mut rt, &server, "/srv/other", "other");
    assert!(lists(&mut rt, &[]), "{:?}", rt.state.panels.links.listed());
}

/// A worktree's pull requests list with what `gh` says of them, and change as it does:
/// those of its branch, and those agents linked to, of its repository only. A click opens
/// one, and × takes it off for good. A merged one isn't asked after again. Without `gh`, or
/// signed out, the panel says so, and links agents posted meanwhile aren't lost.
#[test]
fn pull_requests_list_with_what_gh_says() {
    let server = HcServer::new("lkprs");
    let dir = TempDir::new("lkprs");
    let repo = dir.join("api");
    let path = repo_at(&repo);
    git_in(&repo, &["remote", "add", "origin", "git@github.com:owner/api.git"]);
    git_in(&repo, &["checkout", "-q", "-b", "fix/thing"]);
    let logs = logs_of(&server, &path);
    std::fs::write(
        logs.join("a.jsonl"),
        "{\"text\":\"https://github.com/owner/api/pull/40 and https://github.com/zed-industries/zed/pull/38639\",\"type\":\"assistant\",\"timestamp\":\"t\"}\n",
    )
    .unwrap();
    let told = |number: u64, state: &str, draft: bool, title: &str| {
        format!(
            "{{\"isDraft\":{draft},\"number\":{number},\"state\":\"{state}\",\"title\":\"{title}\",\"url\":\"https://github.com/owner/api/pull/{number}\"}}"
        )
    };
    // What it says is in files, so it can change.
    let branch_says = dir.join("branch.json");
    std::fs::write(&branch_says, format!("[{}]", told(41, "OPEN", true, "Fix the thing"))).unwrap();
    let forty_says = dir.join("forty.json");
    std::fs::write(&forty_says, told(40, "MERGED", false, "An earlier fix")).unwrap();
    let gh = fake_gh(
        &dir,
        &format!(
            r#"case "$*" in
  "pr list --repo owner/api --head fix/thing "*) cat '{}' ;;
  "pr view 40 --repo owner/api "*) cat '{}' ;;
  *) echo "asked of: $*" >&2; exit 1 ;;
esac"#,
            branch_says.display(),
            forty_says.display()
        ),
    );

    let mut rt = with_links(&server, &dir);
    rt.state.panels.links.gh = gh;
    let prs = |s: &Lemur| s.panels.links.listed_prs();
    let shows = |rt: &mut Runtime<Lemur>, want: &[(u64, State)]| {
        wait_for(rt, 10.0, |s| {
            let (loading, got) = prs(s);
            !loading && got == want
        })
    };
    show(&mut rt, &server, &path, "api");
    assert!(shows(&mut rt, &[(41, State::Draft), (40, State::Merged)]), "the open one first, not another project's: {:?}", prs(&rt.state));
    assert!(rt.find("links-pr-0").is_some());
    assert!(rt.find("links-gh-note").is_none());

    // It's marked ready: so says the panel, and no one asks after 40.
    std::fs::write(&branch_says, format!("[{}]", told(41, "OPEN", false, "Fix the thing"))).unwrap();
    let calls = dir.join("calls");
    std::fs::remove_file(&calls).unwrap();
    rt.update(|s, _| s.refresh_links());
    assert!(shows(&mut rt, &[(41, State::Open), (40, State::Merged)]), "{:?}", prs(&rt.state));
    let asked = std::fs::read_to_string(&calls).unwrap();
    assert!(!asked.contains("pr view 40"), "{asked}");

    click(&mut rt, "links-pr-0");
    assert_eq!(rt.take_url_requests(), ["https://github.com/owner/api/pull/41"]);

    // Taken off, it stays off, though it's still the branch's.
    click(&mut rt, "links-pr-remove-0");
    assert!(rt.take_url_requests().is_empty(), "it didn't open");
    rt.update(|s, _| s.refresh_links());
    assert!(shows(&mut rt, &[(40, State::Merged)]), "{:?}", prs(&rt.state));

    // Signed out: it says so, and a link posted meanwhile waits for `gh`.
    std::fs::OpenOptions::new()
        .append(true)
        .open(logs.join("a.jsonl"))
        .and_then(|mut log| {
            std::io::Write::write_all(
                &mut log,
                b"{\"text\":\"https://github.com/owner/api/pull/42\",\"type\":\"assistant\",\"timestamp\":\"t\"}\n",
            )
        })
        .unwrap();
    let signed_out = TempDir::new("lkprs-out");
    rt.state.panels.links.gh = fake_gh(&signed_out, "echo 'To get started with GitHub CLI, please run:  gh auth login' >&2; exit 4");
    rt.update(|s, _| s.refresh_links());
    assert!(wait_for(&mut rt, 10.0, |s| s.panels.links.gh_problem.is_some()), "it says so");
    frames(&mut rt, 2);
    assert!(rt.find("links-gh-note").is_some());

    // By now that log is too old to be read again: only the note of it is left.
    let long_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::File::options().write(true).open(logs.join("a.jsonl")).unwrap().set_modified(long_ago).unwrap();
    let forty_two = dir.join("forty-two");
    std::fs::write(&forty_two, told(42, "OPEN", false, "Posted meanwhile")).unwrap();
    rt.state.panels.links.gh = fake_gh(&dir, &format!("cat '{}'", forty_two.display()));
    rt.update(|s, _| s.refresh_links());
    assert!(wait_for(&mut rt, 10.0, |s| prs(s).1.iter().any(|(number, _)| *number == 42)), "{:?}", prs(&rt.state));
    frames(&mut rt, 2);
    assert!(rt.find("links-gh-note").is_none(), "it can say now");
}

/// Two repositories, each with a thread in it and an artifact in its logs.
fn two_worktrees(server: &HcServer, dir: &Path) -> (Config, Vec<String>, Vec<String>) {
    let mut threads = vec![];
    let mut paths = vec![];
    let mut cfg = server.config();
    for (name, id, title) in [("api", ONE, "Handover"), ("web", TWO, "Report")] {
        let path = repo_at(&dir.join(name));
        threads.push(server.run(&path, &[&format!("lemur.label={name}")], &["sleep", "600"]));
        cfg.bases.push(Base::new("local", &path));
        let log = format!("{{\"text\":\"[{title}](https://claude.ai/artifact/{id})\",\"type\":\"assistant\",\"timestamp\":\"t\"}}\n");
        std::fs::write(logs_of(server, &path).join("a.jsonl"), log).unwrap();
        paths.push(path);
    }
    (cfg, threads, paths)
}

/// **Links** points its panel at the worktree on show, and at another's once a thread
/// there shows.
#[test]
fn the_links_panel_follows_the_worktree_on_show() {
    let server = HcServer::new("lkgui");
    let dir = TempDir::new("lkgui");
    let (cfg, threads, _) = two_worktrees(&server, &dir);
    let mut rt = loaded(cfg);
    assert!(refreshing(&mut rt, 10.0, |s| threads.iter().all(|t| s.project_of("local", t).is_some())), "listed");
    open(&mut rt, &threads[0]);
    rt.update(|s, _| s.toggle_right(Right::Links));
    assert!(lists(&mut rt, &["Handover"]));
    frames(&mut rt, 4);
    assert!(rt.find("links-artifact-0").is_some());
    open(&mut rt, &threads[1]);
    assert!(lists(&mut rt, &["Report"]));
    rt.update(|s, _| s.toggle_right(Right::Links));
    frames(&mut rt, 30);
    assert!(rt.find("links-artifact-0").is_none(), "hidden");
}

/// Naming an artifact ends with the keyboard back on the thread on show: with Enter, Esc,
/// or a click elsewhere, which keeps what's typed as Enter does, as hiding the panel does.
#[test]
fn the_keyboard_comes_back_from_naming_an_artifact() {
    let server = HcServer::new("lkkeys");
    let dir = TempDir::new("lkkeys");
    let (cfg, threads, _) = two_worktrees(&server, &dir);
    let mut rt = loaded(cfg);
    assert!(refreshing(&mut rt, 10.0, |s| s.project_of("local", &threads[0]).is_some()), "listed");
    open(&mut rt, &threads[0]);
    let shown = key(&threads[0]);
    assert!(wait_for(&mut rt, 10.0, |s| s.terminals.get(&shown).is_some_and(|v| v.painted)), "painted");
    rt.update(|s, _| s.toggle_right(Right::Links));
    assert!(lists(&mut rt, &["Handover"]));
    // What's typed now shows in the terminal, which echoes it.
    let reaches = |rt: &mut Runtime<Lemur>, word: &str| {
        assert!(rt.state.keyboard_on_terminal(), "the terminal has it, before {word}");
        type_text(rt, word);
        wait_for(rt, 5.0, |s| super::super::terminal::lines(s, &shown).iter().any(|l| l.contains(word)))
    };

    click(&mut rt, "links-artifact-rename-0");
    assert!(!rt.state.keyboard_on_terminal(), "the field has it");
    type_text(&mut rt, "Mine");
    press(&mut rt, K::Enter);
    assert!(lists(&mut rt, &["Mine"]));
    assert!(reaches(&mut rt, "first"), "{:?}", super::super::terminal::lines(&rt.state, &shown));

    // A click on the terminal keeps what's typed, and gives it the keyboard.
    click(&mut rt, "links-artifact-rename-0");
    type_text(&mut rt, "Ours");
    click(&mut rt, "term-area");
    assert!(lists(&mut rt, &["Ours"]), "{:?}", rt.state.panels.links.listed());
    assert!(reaches(&mut rt, "second"), "{:?}", super::super::terminal::lines(&rt.state, &shown));

    // Esc too, leaving the name as it was.
    click(&mut rt, "links-artifact-rename-0");
    type_text(&mut rt, "Not this");
    press(&mut rt, K::Escape);
    assert_eq!(rt.state.panels.links.listed().1, ["Ours"]);
    assert!(reaches(&mut rt, "third"), "{:?}", super::super::terminal::lines(&rt.state, &shown));

    // Hiding the panel keeps what's typed, and the field lets go. (Where Rune's focus goes
    // then, the sidebar's toggle, is the window's.)
    click(&mut rt, "links-artifact-rename-0");
    type_text(&mut rt, "Theirs");
    rt.update(|s, _| s.toggle_right(Right::Links));
    frames(&mut rt, 4);
    assert_eq!(rt.state.panels.links.listed().1, ["Theirs"]);
    assert!(rt.state.keyboard_on_terminal());
}

/// A thread just started isn't listed yet when it shows: the panel goes to its worktree
/// once it is, and with nothing left on show, it says so.
#[test]
fn the_links_panel_follows_a_thread_once_its_listed() {
    let server = HcServer::new("lklisted");
    let dir = TempDir::new("lklisted");
    let (api, web) = (repo_at(&dir.join("api")), repo_at(&dir.join("web")));
    let a = server.run(&api, &["lemur.label=a"], &["sleep", "600"]);
    let mut cfg = server.config();
    cfg.bases = vec![Base::new("local", &api), Base::new("local", &web)];
    let mut rt = loaded(cfg);
    assert!(refreshing(&mut rt, 10.0, |s| s.project_of("local", &a).is_some()), "listed");
    open(&mut rt, &a);
    rt.update(|s, _| s.toggle_right(Right::Links));
    assert!(rt.state.panels.links.is_on(Some(("local", &api))));

    let b = server.run(&web, &["lemur.label=b"], &["sleep", "600"]);
    rt.update(|s, _| s.open_fresh("local".into(), b.clone()));
    assert!(refreshing(&mut rt, 10.0, |s| s.panels.links.is_on(Some(("local", &web)))), "once it's listed");

    // Nothing on show.
    let mut snap = rt.state.snap.clone();
    snap.hosts[0].bases.iter_mut().for_each(|b| b.worktrees.iter_mut().for_each(|w| w.sessions.clear()));
    snap.hosts[0].other.clear();
    rt.update(|s, _| s.set_snapshot(snap));
    assert_eq!(rt.state.active, None);
    assert!(rt.state.panels.links.is_on(None), "nothing on show");
    frames(&mut rt, 4);
    assert!(rt.find("links-empty").is_some());
}
