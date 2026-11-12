//! The Changes panel's tests: git's answers read, files listed, diffed and committed on a
//! test herdr-core's host, and the panel driven in a window as a person would.

use super::super::core::wait_for;
use super::super::{Lemur, Right};
use super::{Change, changes_in, commit_in, diff_of, lines_of, parse_changes, to_add, to_diff};
use crate::config::{Base, Config, Host};
use crate::test_support::{HcServer, TempDir, git_in};
use rune::{Key as K, KeyEvent, Mods, Runtime, TextEdit};
use std::path::Path;
use crate::gui::tests::{key, open, press, refreshing, window};

/// The window on `cfg`, once a snapshot lists every thread in `threads`.
fn loaded(cfg: Config, threads: &[&str]) -> Runtime<Lemur> {
    let mut rt = window(cfg);
    let listed = refreshing(&mut rt, 10.0, |s| threads.iter().all(|t| s.session_of(&key(t)).is_some()));
    assert!(listed, "the first snapshot");
    rt
}

/// Waits for `done`, asking the panel again every half second, as the ticks would.
fn asking(rt: &mut Runtime<Lemur>, secs: f64, done: impl Fn(&Lemur) -> bool) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed().as_secs_f64() < secs {
        rt.update(|s, _| s.refresh_changes());
        if wait_for(rt, 0.5, &done) {
            return true;
        }
    }
    false
}

fn click_at(rt: &mut Runtime<Lemur>, x: f32, y: f32) {
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    for _ in 0..4 {
        rt.frame(1.0 / 60.0);
    }
}

fn click(rt: &mut Runtime<Lemur>, key: &str) {
    // What just came in fades in: a press lands once it shows.
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element keyed {key}"));
    click_at(rt, x, y);
}

/// Shows the Changes panel on worktree `path` of `host`.
fn panel_on(rt: &mut Runtime<Lemur>, host: &Host, path: &Path) {
    let (host, path) = (host.clone(), path.to_string_lossy().into_owned());
    rt.update(|s, _| {
        s.toggle_right(Right::Changes);
        s.show_worktree(host, path, "repo".into());
    });
}

/// Waits until the panel has answered, and no commit is under way.
fn settled(rt: &mut Runtime<Lemur>) {
    let done = wait_for(rt, 20.0, |s| !s.panels.changes.loading && s.panels.changes.committing.is_none());
    assert!(done, "the panel settles");
}

fn listed(rt: &Runtime<Lemur>) -> (Vec<(String, char)>, Vec<String>) {
    rt.state.panels.changes.listed()
}

fn files(s: &Lemur) -> Vec<(String, char)> {
    s.panels.changes.listed().0
}

fn shows_line(s: &Lemur, line: &str) -> bool {
    s.panels.changes.listed().1.iter().any(|l| l == line)
}

fn head(repo: &Path) -> String {
    let out = std::process::Command::new("git").args(["rev-parse", "HEAD"]).current_dir(repo).output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A repository at `path`, with one commit.
fn repo_at(path: &Path) -> String {
    committed(path, &[]);
    path.to_string_lossy().into_owned()
}

/// A repository at `path` with one commit, of `files`, each with its text.
fn committed(path: &Path, files: &[(&str, &str)]) {
    std::fs::create_dir_all(path).unwrap();
    git_in(path, &["init", "-q", "-b", "main"]);
    for (name, text) in files {
        std::fs::write(path.join(name), text).unwrap();
    }
    git_in(path, &["add", "."]);
    git_in(path, &["commit", "-q", "--allow-empty", "-m", "init"]);
}

fn change(path: &str, status: char) -> Change {
    Change { path: path.into(), status, unstaged: true, added: None, removed: None }
}

/// `file`'s diff on `host`, as the panel shows it.
fn shown(host: &Host, root: &str, file: &Change) -> rune_kit::diff::FileDiff {
    to_diff(file, &diff_of(host, root, file).unwrap())
}

#[test]
fn changes_are_read_from_git() {
    let out = "@@status\n M src/a.rs\0A  b.rs\0?? new.txt\0?? .claude/worktrees/x/\0 D gone.rs\0UU both.rs\0\n@@numstat\n\
               3\t1\tsrc/a.rs\x002\t0\tb.rs\0-\t-\tlogo.png\0\n@@end\n";
    let changes = parse_changes(out).unwrap();
    let brief: Vec<(&str, char, Option<u64>, Option<u64>)> = changes.iter().map(|c| (c.path.as_str(), c.status, c.added, c.removed)).collect();
    assert_eq!(
        brief,
        [
            ("src/a.rs", 'M', Some(3), Some(1)),
            ("b.rs", 'A', Some(2), Some(0)),
            ("new.txt", '?', None, None),
            ("gone.rs", 'D', None, None),
            ("both.rs", 'U', None, None),
        ]
    );
    assert_eq!(parse_changes("@@error Not a git repository\n"), Err("Not a git repository".into()));
    assert!(parse_changes("@@status\n\n@@numstat\n\n@@end\n").unwrap().is_empty());
}

/// A diff shows from its first hunk; a new file's lines are all added, and counted; a
/// binary file has no lines to show; a long diff shows its first `MOST_LINES`.
#[test]
fn diffs_start_at_their_first_hunk() {
    let changed = change("x", 'M');
    let out = "diff --git a/x b/x\nindex 1..2\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-one\n+two\n";
    assert_eq!(lines_of(&to_diff(&changed, out)), ["@@ -1 +1 @@", "-one", "+two"]);
    let new = change("x", '?');
    let d = to_diff(&new, "a\nb\n");
    assert_eq!(lines_of(&d), ["@@ -0,0 +1,2 @@", "+a", "+b"]);
    assert_eq!(d.additions(), 2);
    assert!(to_diff(&new, "@@binary\n").binary);
    let binary = "diff --git a/x b/x\nindex 1..2 100644\nBinary files a/x and b/x differ\n";
    assert!(to_diff(&changed, binary).binary);
    let added: String = (0..5000).map(|i| format!("+{i}\n")).collect();
    let long = format!("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -0,0 +1,5000 @@\n{added}");
    let lines = lines_of(&to_diff(&changed, &long));
    assert!(lines.len() < 3000 && lines.len() > 2900, "{}", lines.len());
}

/// Switched to another worktree while it asks the first, the panel lists the second's
/// changes, not the first's; and a click on a file while it asks again doesn't stop it
/// asking from then on.
#[test]
fn the_panel_lists_the_worktree_it_shows() {
    let server = HcServer::new("changes-switch");
    let host = server.config().hosts.remove(0);
    let dir = TempDir::new("changes-switch");
    let repo = dir.join("repo");
    committed(&repo, &[("a.txt", "one\n")]);
    git_in(&repo, &["worktree", "add", "-q", "-b", "b", "../b"]);
    std::fs::write(repo.join("a.txt"), "two\n").unwrap();
    let b = dir.join("b");
    std::fs::write(b.join("b.txt"), "new\n").unwrap();
    let mut rt = window(server.config());
    panel_on(&mut rt, &host, &repo);
    panel_on(&mut rt, &host, &b);
    let settled = wait_for(&mut rt, 10.0, |s| !s.panels.changes.loading && !files(s).is_empty());
    assert!(settled, "{:?}", listed(&rt));
    assert_eq!(listed(&rt).0, [("b.txt".to_string(), '?')]);

    rt.update(|s, _| {
        s.refresh_changes();
        s.open_file("b.txt".into());
    });
    std::fs::write(b.join("c.txt"), "more\n").unwrap();
    assert!(asking(&mut rt, 10.0, |s| files(s).len() == 2), "{:?}", listed(&rt));
}

/// A click on a file while a commit goes in leaves the commit to finish: the panel doesn't
/// stay on "Committing…" for good.
#[test]
fn a_click_during_a_commit_leaves_it_to_finish() {
    let server = HcServer::new("changes-click");
    let host = server.config().hosts.remove(0);
    let dir = TempDir::new("changes-click");
    let repo = dir.join("repo");
    std::fs::create_dir(&repo).unwrap();
    git_in(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    let mut rt = window(server.config());
    panel_on(&mut rt, &host, &repo);
    settled(&mut rt);
    rt.update(|s, _| {
        s.panels.changes.message = TextEdit::new("Add");
        s.commit();
        s.open_file("a.txt".into());
    });
    settled(&mut rt);
    let p = &rt.state.panels.changes;
    assert!(p.files.is_empty(), "{:?}", p.files);
    assert_eq!(p.message.text, "", "it's in");
    let note = p.note.as_ref().expect("what it did");
    assert!(note.0.ends_with("Add"), "{note:?}");
}

/// A commit's answer goes to its own worktree's: another shown meanwhile lists its
/// changes, and keeps what's typed and shown there.
#[test]
fn a_commit_finishing_leaves_another_worktree_alone() {
    let server = HcServer::new("changes-other");
    let host = server.config().hosts.remove(0);
    let dir = TempDir::new("changes-other");
    let (repo, other) = (dir.join("repo"), dir.join("other"));
    for r in [&repo, &other] {
        std::fs::create_dir(r).unwrap();
        git_in(r, &["init", "-q", "-b", "main"]);
        std::fs::write(r.join("a.txt"), "one\n").unwrap();
    }
    let mut rt = window(server.config());
    panel_on(&mut rt, &host, &repo);
    settled(&mut rt);
    let other_path = other.to_string_lossy().into_owned();
    rt.update(|s, _| {
        s.panels.changes.message = TextEdit::new("Add");
        s.commit();
        s.show_worktree(host.clone(), other_path, "other".into());
        assert!(s.panels.changes.loading, "it asks the other, as the commit goes in");
        s.panels.changes.message = TextEdit::new("For the other");
    });
    settled(&mut rt);
    let p = &rt.state.panels.changes;
    let note = p.note.as_ref().expect("what it did");
    assert!(note.0.starts_with("repo: Committed "), "{note:?}");
    assert_eq!(p.message.text, "For the other");
    assert_eq!(listed(&rt).0, [("a.txt".to_string(), '?')]);
}

/// With a file in conflict, Enter doesn't commit its markers, and the panel says why; once
/// it's resolved, Enter commits.
#[test]
fn files_in_conflict_arent_committed() {
    let server = HcServer::new("changes-conflict");
    let host = server.config().hosts.remove(0);
    let dir = TempDir::new("changes-conflict");
    let repo = dir.join("repo");
    std::fs::create_dir(&repo).unwrap();
    let git = |args: &[&str]| git_in(&repo, args);
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "init"]);
    git(&["checkout", "-q", "-b", "other"]);
    std::fs::write(repo.join("a.txt"), "theirs\n").unwrap();
    git(&["commit", "-q", "-am", "theirs"]);
    git(&["checkout", "-q", "main"]);
    std::fs::write(repo.join("a.txt"), "ours\n").unwrap();
    git(&["commit", "-q", "-am", "ours"]);
    let merged = std::process::Command::new("git").args(["merge", "other"]).current_dir(&repo).output().unwrap();
    assert!(!merged.status.success(), "it conflicts");
    let before = head(&repo);
    let mut rt = window(server.config());
    panel_on(&mut rt, &host, &repo);
    settled(&mut rt);
    assert_eq!(rt.state.panels.changes.in_conflict(), 1);
    rt.frame(1.0 / 60.0);
    assert!(rt.find("changes-conflict").is_some(), "it says so");
    rt.update(|s, _| s.panels.changes.message = TextEdit::new("Merge"));
    click(&mut rt, "changes-message");
    press(&mut rt, K::Enter, Mods::default());
    settled(&mut rt);
    assert_eq!(head(&repo), before, "nothing went in");
    // Resolved, it goes in.
    std::fs::write(repo.join("a.txt"), "both\n").unwrap();
    git(&["add", "a.txt"]);
    assert!(asking(&mut rt, 10.0, |s| s.panels.changes.in_conflict() == 0));
    press(&mut rt, K::Enter, Mods::default());
    settled(&mut rt);
    assert_ne!(head(&repo), before, "Enter commits");
}

/// A click on a file shows its diff, and another hides it, for good even with an ask for
/// that diff under way; one on another file moves it.
#[test]
fn a_click_on_a_file_shows_its_diff_and_another_hides_it() {
    let server = HcServer::new("changes-toggle");
    let host = server.config().hosts.remove(0);
    let dir = TempDir::new("changes-toggle");
    let repo = dir.join("repo");
    committed(&repo, &[("a.txt", "one\n"), ("b.txt", "one\n")]);
    std::fs::write(repo.join("a.txt"), "two\n").unwrap();
    std::fs::write(repo.join("b.txt"), "three\n").unwrap();
    let mut rt = window(server.config());
    panel_on(&mut rt, &host, &repo);
    assert!(wait_for(&mut rt, 10.0, |s| !s.panels.changes.loading && !files(s).is_empty()));
    let names: Vec<String> = listed(&rt).0.into_iter().map(|f| f.0).collect();
    assert_eq!(names, ["a.txt", "b.txt"]);

    click(&mut rt, "change-0");
    assert!(wait_for(&mut rt, 10.0, |s| shows_line(s, "+two")), "{:?}", listed(&rt));
    click(&mut rt, "change-0");
    assert!(listed(&rt).1.is_empty(), "hidden: {:?}", listed(&rt));
    click(&mut rt, "change-0");
    assert!(wait_for(&mut rt, 10.0, |s| shows_line(s, "+two")), "shown again: {:?}", listed(&rt));

    click(&mut rt, "change-1");
    assert!(wait_for(&mut rt, 10.0, |s| shows_line(s, "+three")), "{:?}", listed(&rt));
    assert!(!listed(&rt).1.iter().any(|l| l == "+two"), "{:?}", listed(&rt));

    rt.update(|s, _| {
        s.refresh_changes();
        s.toggle_file("b.txt".into());
    });
    settled(&mut rt);
    assert!(listed(&rt).1.is_empty(), "came back: {:?}", listed(&rt));
}

/// What's staged commits as staged: a removal (of an ignored file too) and a rename, which
/// `git add` can't name any more. What isn't is added first, whatever its name.
#[test]
fn staged_changes_commit_as_staged() {
    let server = HcServer::new("changes-staged");
    let host = server.config().hosts.remove(0);
    let dir = TempDir::new("changes-staged");
    let repo = dir.join("repo");
    std::fs::create_dir(&repo).unwrap();
    let git = |args: &[&str]| git_in(&repo, args);
    git(&["init", "-q", "-b", "main"]);
    for f in ["a.txt", "b.txt", "c.txt", "kept.log"] {
        std::fs::write(repo.join(f), format!("{f}\n")).unwrap();
    }
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "init"]);
    std::fs::write(repo.join(".gitignore"), "*.log\n").unwrap();
    git(&["rm", "-q", "b.txt"]);
    git(&["mv", "c.txt", "e.txt"]);
    git(&["rm", "-q", "--cached", "kept.log"]);
    std::fs::write(repo.join("a.txt"), "staged\n").unwrap();
    git(&["add", "a.txt"]);
    std::fs::write(repo.join("a.txt"), "and more\n").unwrap();
    for name in ["new\nline.txt", "say \"hi\".txt", "back\\slash.txt", "it's.txt", " spaced .txt", "-dash.txt", ":(glob)*.txt", "æøå.txt", "_"] {
        std::fs::write(repo.join(name), "x\n").unwrap();
    }
    let root = repo.to_string_lossy().into_owned();

    let files = to_add(&changes_in(&host, &root).unwrap());
    let said = commit_in(&host, &root, "Tidy", &files).unwrap();
    assert!(said.ends_with("Tidy"), "{said}");
    let tree = std::process::Command::new("git").args(["ls-tree", "-r", "--name-only", "-z", "HEAD"]).current_dir(&repo).output().unwrap();
    let mut tree: Vec<String> = String::from_utf8_lossy(&tree.stdout).split('\0').filter(|f| !f.is_empty()).map(String::from).collect();
    tree.sort();
    assert_eq!(
        tree,
        [
            " spaced .txt",
            "-dash.txt",
            ".gitignore",
            ":(glob)*.txt",
            "_",
            "a.txt",
            "back\\slash.txt",
            "e.txt",
            "it's.txt",
            "new\nline.txt",
            "say \"hi\".txt",
            "æøå.txt"
        ]
    );
    let shown = std::process::Command::new("git").args(["show", "HEAD:a.txt"]).current_dir(&repo).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&shown.stdout), "and more\n");
    assert!(changes_in(&host, &root).unwrap().is_empty());
}

/// A file removed from git but kept on disk is committed as removed, and goes on listed as
/// new; with nothing else staged, too.
#[test]
fn a_file_kept_on_disk_commits_as_removed() {
    let server = HcServer::new("changes-rm");
    let host = server.config().hosts.remove(0);
    let dir = TempDir::new("changes-rm-cached");
    let repo = dir.join("repo");
    committed(&repo, &[("secrets.yml", "x\n")]);
    git_in(&repo, &["rm", "-q", "--cached", "secrets.yml"]);
    let root = repo.to_string_lossy().into_owned();

    let files = to_add(&changes_in(&host, &root).unwrap());
    assert!(files.is_empty(), "{files:?}");
    let said = commit_in(&host, &root, "Forget it", &files).unwrap();
    assert!(said.ends_with("Forget it"), "{said}");
    let tree = std::process::Command::new("git").args(["ls-tree", "-r", "--name-only", "HEAD"]).current_dir(&repo).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&tree.stdout), "");
    let left: Vec<(String, char)> = changes_in(&host, &root).unwrap().into_iter().map(|c| (c.path, c.status)).collect();
    assert_eq!(left, [("secrets.yml".to_string(), '?')]);
}

/// Listing changes and showing a diff leave the index as it is, a file touched but not
/// changed too: its lock is the one an agent's `git add` takes.
#[test]
fn listing_changes_leaves_the_index_alone() {
    let server = HcServer::new("changes-no-lock");
    let host = server.config().hosts.remove(0);
    let dir = TempDir::new("changes-no-lock");
    let repo = dir.join("repo");
    committed(&repo, &[("a.txt", "one\n"), ("b.txt", "one\n")]);
    std::fs::write(repo.join("b.txt"), "two\n").unwrap();
    let index = repo.join(".git/index");
    let written = || std::fs::metadata(&index).unwrap().modified().unwrap();
    let before = written();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::File::options().write(true).open(repo.join("a.txt")).unwrap().set_modified(std::time::SystemTime::now()).unwrap();
    let root = repo.to_string_lossy().into_owned();
    let changes = changes_in(&host, &root).unwrap();
    assert_eq!(changes.len(), 1);
    diff_of(&host, &root, &changes[0]).unwrap();
    assert_eq!(written(), before, "the index was written");
}

/// Before a repository's first commit, a staged file's diff is as plain as after it,
/// whatever external diff is set up.
#[test]
fn a_diff_before_the_first_commit_is_plain_too() {
    let server = HcServer::new("changes-first");
    let host = server.config().hosts.remove(0);
    let dir = TempDir::new("changes-first");
    let repo = dir.join("repo");
    std::fs::create_dir(&repo).unwrap();
    git_in(&repo, &["init", "-q", "-b", "main"]);
    git_in(&repo, &["config", "diff.external", "echo external"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git_in(&repo, &["add", "."]);
    let root = repo.to_string_lossy().into_owned();
    let changes = changes_in(&host, &root).unwrap();
    assert_eq!(lines_of(&shown(&host, &root, &changes[0])), ["@@ -0,0 +1 @@", "+one"]);
}

/// A file's diff is that file's, though its name reads as a pattern (`[id]`) that another
/// changed file matches too.
#[test]
fn a_diff_is_of_the_file_named() {
    let server = HcServer::new("changes-literal");
    let host = server.config().hosts.remove(0);
    let dir = TempDir::new("changes-literal");
    let repo = dir.join("repo");
    for d in ["app/[id]", "app/d"] {
        std::fs::create_dir_all(repo.join(d)).unwrap();
        std::fs::write(repo.join(d).join("page.tsx"), "one\n").unwrap();
    }
    git_in(&repo, &["init", "-q", "-b", "main"]);
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["commit", "-q", "-m", "init"]);
    for d in ["app/[id]", "app/d"] {
        std::fs::write(repo.join(d).join("page.tsx"), format!("{d}\n")).unwrap();
    }
    let root = repo.to_string_lossy().into_owned();
    let changes = changes_in(&host, &root).unwrap();
    let id = changes.iter().find(|c| c.path.contains("[id]")).unwrap();
    assert_eq!(lines_of(&shown(&host, &root, id)), ["@@ -1 +1 @@", "-one", "+app/[id]"]);
}

/// More new files than a command line holds commit too.
#[test]
fn thousands_of_files_commit() {
    let server = HcServer::new("changes-many");
    let host = server.config().hosts.remove(0);
    let dir = TempDir::new("changes-many");
    let repo = dir.join("repo");
    // Long paths, so fewer files make about 1.3 MB of them: this Mac takes 1 MB of
    // arguments.
    let deep = repo.join(format!("vendor/{}", "package-".repeat(25)));
    std::fs::create_dir_all(&deep).unwrap();
    git_in(&repo, &["init", "-q", "-b", "main"]);
    for i in 0..6_000 {
        std::fs::write(deep.join(format!("file-{i:05}.txt")), "").unwrap();
    }
    let root = repo.to_string_lossy().into_owned();
    let files: Vec<String> = changes_in(&host, &root).unwrap().into_iter().map(|c| c.path).collect();
    assert_eq!(files.len(), 6_000);
    let said = commit_in(&host, &root, "Vendor", &files).unwrap();
    assert!(said.ends_with("Vendor"), "{said}");
    assert!(changes_in(&host, &root).unwrap().is_empty());
}

/// In a real repository, on a test herdr-core: listed, diffed, then all committed with the
/// message, quotes and all; a worktree kept inside it, and a clone, are neither.
#[test]
fn a_worktree_is_listed_diffed_and_committed() {
    let server = HcServer::new("changes-panel");
    let host = server.config().hosts.remove(0);
    let dir = TempDir::new("changes-panel");
    let repo = dir.join("repo");
    committed(&repo, &[("a.txt", "one\ntwo\n")]);
    git_in(&repo, &["worktree", "add", "-q", ".claude/worktrees/feat"]);
    std::fs::create_dir(repo.join("clone")).unwrap();
    git_in(&repo.join("clone"), &["init", "-q"]);
    std::fs::write(repo.join("clone/x.txt"), "x\n").unwrap();
    std::fs::write(repo.join("a.txt"), "one\n2\n").unwrap();
    std::fs::write(repo.join("it's new.txt"), "x\ny\nz\n").unwrap();
    let root = repo.to_string_lossy().into_owned();

    let changes = changes_in(&host, &root).unwrap();
    let brief: Vec<(&str, char)> = changes.iter().map(|c| (c.path.as_str(), c.status)).collect();
    assert_eq!(brief, [("a.txt", 'M'), ("it's new.txt", '?')]);
    assert_eq!((changes[0].added, changes[0].removed), (Some(1), Some(1)));
    assert_eq!(lines_of(&shown(&host, &root, &changes[0])), ["@@ -1,2 +1,2 @@", " one", "-two", "+2"]);
    assert_eq!(shown(&host, &root, &changes[1]).additions(), 3);

    let files: Vec<String> = changes.iter().map(|c| c.path.clone()).collect();
    let said = commit_in(&host, &root, "Fix it's \"quotes\"", &files).unwrap();
    assert!(said.starts_with("Committed "), "{said}");
    assert!(said.ends_with("Fix it's \"quotes\""), "{said}");
    assert!(changes_in(&host, &root).unwrap().is_empty());
    let committed = std::process::Command::new("git").args(["ls-tree", "-r", "--name-only", "HEAD"]).current_dir(&repo).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&committed.stdout), "a.txt\nit's new.txt\n");
    assert_eq!(changes_in(&host, &dir.to_string_lossy()), Err("Not a git repository".into()));
}

/// The Changes panel lists the worktree on show's changes, and a click on one shows its
/// diff; it follows the thread on show.
#[test]
fn the_changes_panel_lists_and_diffs() {
    let server = HcServer::new("chpanel");
    let dir = TempDir::new("changes-gui-panel");
    let repo = dir.join("api");
    committed(&repo, &[("a.txt", "one\ntwo\n")]);
    git_in(&repo, &["worktree", "add", "-q", "-b", "auth", "wt/auth"]);
    git_in(&repo, &["worktree", "add", "-q", "-b", "docs", "wt/docs"]);
    let wt = repo.join("wt/auth");
    std::fs::write(wt.join("a.txt"), "one\n2\n").unwrap();
    let clean = server.run(&repo.join("wt/docs").to_string_lossy(), &["lemur.label=clean"], &["sleep", "600"]);
    let changed = server.run(&wt.to_string_lossy(), &["lemur.label=changed"], &["sleep", "600"]);
    let mut cfg = server.config();
    cfg.bases.push(Base::new("local", &repo.to_string_lossy()));
    let mut rt = loaded(cfg, &[&clean, &changed]);
    assert!(refreshing(&mut rt, 5.0, |s| s.project_of("local", &changed).is_some() && s.project_of("local", &clean).is_some()));
    open(&mut rt, &changed);
    click(&mut rt, "toggle-changes");
    assert!(wait_for(&mut rt, 5.0, |s| !files(s).is_empty()));
    assert_eq!(listed(&rt).0, [("a.txt".to_string(), 'M')]);
    click(&mut rt, "change-0");
    assert!(wait_for(&mut rt, 5.0, |s| s.panels.changes.listed().1.len() > 1));
    assert_eq!(listed(&rt).1, ["@@ -1,2 +1,2 @@", " one", "-two", "+2"]);

    open(&mut rt, &clean);
    assert!(wait_for(&mut rt, 5.0, |s| files(s).is_empty() && !s.panels.changes.loading));
    assert!(rt.state.panels.changes.is_on(Some(("local", &repo.join("wt/docs").to_string_lossy()))));
}

/// When the thread on show ends and none shows instead, the Changes panel no longer lists
/// its worktree's changes, nor commits there.
#[test]
fn the_changes_panel_empties_with_nothing_on_show() {
    let server = HcServer::new("chpanel-none");
    let dir = TempDir::new("changes-none");
    let repo = dir.join("api");
    committed(&repo, &[("a.txt", "one\n")]);
    std::fs::write(repo.join("a.txt"), "two\n").unwrap();
    let t = server.run(&repo.to_string_lossy(), &["lemur.label=t"], &["sleep", "600"]);
    let mut cfg = server.config();
    cfg.bases.push(Base::new("local", &repo.to_string_lossy()));
    let mut rt = loaded(cfg, &[&t]);
    assert!(refreshing(&mut rt, 5.0, |s| s.project_of("local", &t).is_some()));
    open(&mut rt, &t);
    rt.update(|s, _| s.toggle_right(Right::Changes));
    assert!(wait_for(&mut rt, 5.0, |s| !files(s).is_empty()));
    server.hc(&["kill", &t]);
    assert!(refreshing(&mut rt, 5.0, |s| s.active.is_none()));
    assert!(files(&rt.state).is_empty(), "{:?}", listed(&rt));
    assert!(rt.state.panels.changes.is_on(None));
    assert!(rt.find("changes-commit").is_none(), "nothing to commit in");
}

/// A thread just started isn't listed yet when it shows: the Changes panel goes to its
/// worktree once it is.
#[test]
fn the_panel_follows_a_thread_once_its_listed() {
    let server = HcServer::new("just-started");
    let dir = TempDir::new("just-started");
    let (api, web) = (repo_at(&dir.join("api")), repo_at(&dir.join("web")));
    let a = server.run(&api, &["lemur.label=a"], &["sleep", "600"]);
    let mut cfg = server.config();
    cfg.bases = vec![Base::new("local", &api), Base::new("local", &web)];
    let mut rt = loaded(cfg, &[&a]);
    assert!(refreshing(&mut rt, 5.0, |s| s.project_of("local", &a).is_some()));
    open(&mut rt, &a);
    rt.update(|s, _| s.toggle_right(Right::Changes));
    assert!(rt.state.panels.changes.is_on(Some(("local", &api))));

    let b = server.run(&web, &["lemur.label=b"], &["sleep", "600"]);
    rt.update(|s, _| s.open_fresh("local".into(), b.clone()));
    assert!(rt.state.panels.changes.is_on(None), "not listed yet");
    assert!(refreshing(&mut rt, 5.0, |s| s.panels.changes.is_on(Some(("local", &web)))));
}

/// The diff over the window goes as soon as the panel moves to another worktree, not once
/// that answers, which on a hung host could take minutes with keys going nowhere. With no
/// diff, none opens.
#[test]
fn a_diff_over_the_window_goes_with_its_worktree() {
    let server = HcServer::new("preview-moves");
    let dir = TempDir::new("preview-moves");
    let (api, web) = (repo_at(&dir.join("api")), repo_at(&dir.join("web")));
    std::fs::write(dir.join("api/new.txt"), "one\n").unwrap();
    let a = server.run(&api, &["lemur.label=a"], &["sleep", "600"]);
    let mut cfg = server.config();
    cfg.bases = vec![Base::new("local", &api), Base::new("local", &web)];
    let host = cfg.hosts[0].clone();
    let mut rt = loaded(cfg, &[&a]);
    assert!(refreshing(&mut rt, 5.0, |s| s.project_of("local", &a).is_some()));
    open(&mut rt, &a);
    rt.update(|s, _| s.toggle_right(Right::Changes));
    assert!(wait_for(&mut rt, 10.0, |s| !files(s).is_empty()));

    rt.update(|s, _| s.open_diff_preview());
    assert!(!rt.state.diff_overlaid(), "no diff shows yet");

    click(&mut rt, "change-0");
    assert!(wait_for(&mut rt, 10.0, |s| shows_line(s, "+one")));
    click(&mut rt, "changes-preview");
    assert!(rt.state.diff_overlaid());

    rt.update(|s, _| s.show_worktree(host, web.clone(), "web".into()));
    // Nothing has run in the background yet.
    assert!(!rt.state.diff_overlaid(), "it goes at once");
}

/// Where the diff keyed `view` is scrolled to: how far its rows are left of and above its
/// viewport.
fn scrolled(rt: &Runtime<Lemur>, view: &str) -> (f32, f32) {
    let els = rt.elements();
    let at = |end: String| els.iter().find(|e| !e.exiting && e.path.ends_with(&end)).map(|e| e.rect);
    let viewport = at(format!("{view}/scroll")).unwrap_or_else(|| panic!("{view}'s viewport"));
    let rows = at(format!("{view}/scroll/rows")).unwrap_or_else(|| panic!("{view}'s rows"));
    (viewport[0] - rows[0], viewport[1] - rows[1])
}

/// The wheel over `key`, across and then down.
fn wheel(rt: &mut Runtime<Lemur>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element keyed {key}"));
    rt.pointer_move(x, y);
    // One axis at a time: a gesture keeps the axis it starts on.
    rt.wheel_xy(x, y, -200.0, 0.0);
    rt.frame(1.0 / 60.0);
    rt.wheel_xy(x, y, 0.0, -200.0);
    for _ in 0..10 {
        rt.frame(1.0 / 60.0);
    }
}

/// A diff scrolls both ways in the panel, and starts over for another file. Its button
/// shows it over the window, scrolling both ways too; Esc, ⌘W, ×, a click outside or the
/// file's change ending close that, and ⌘W leaves the thread under it alone.
#[test]
fn a_diff_shows_over_the_window_and_scrolls_both_ways() {
    let server = HcServer::new("diff-preview");
    let dir = TempDir::new("diff-preview");
    let repo = dir.join("api");
    committed(&repo, &[("long.txt", "one\n"), ("short.txt", "one\n")]);
    // Wider and taller than the window.
    let line = "x".repeat(1000);
    std::fs::write(repo.join("long.txt"), vec![line; 300].join("\n")).unwrap();
    std::fs::write(repo.join("short.txt"), "two\n").unwrap();
    let path = repo.to_string_lossy().into_owned();
    let t = server.run(&path, &[], &["sleep", "600"]);
    let mut cfg = server.config();
    cfg.bases.push(Base::new("local", &path));
    let mut rt = loaded(cfg, &[&t]);
    rt.set_viewport((1200.0, 800.0), 2.0);
    assert!(refreshing(&mut rt, 5.0, |s| s.project_of("local", &t).is_some()));
    open(&mut rt, &t);
    rt.update(|s, _| s.toggle_right(Right::Changes));
    assert!(wait_for(&mut rt, 10.0, |s| files(s).len() == 2));
    let long_diff = |s: &Lemur| s.panels.changes.listed().1.len() > 100;

    click(&mut rt, "change-0");
    assert!(wait_for(&mut rt, 10.0, long_diff));
    wheel(&mut rt, "changes-diff");
    let (x, y) = scrolled(&rt, "changes-diff");
    assert!(x > 0.0 && y > 0.0, "{:?}", (x, y));
    click(&mut rt, "change-1");
    assert!(wait_for(&mut rt, 10.0, |s| shows_line(s, "+two")));
    rt.frame(1.0 / 60.0);
    assert_eq!(scrolled(&rt, "changes-diff"), (0.0, 0.0), "another file");
    click(&mut rt, "change-0");
    assert!(wait_for(&mut rt, 10.0, long_diff));

    click(&mut rt, "changes-preview");
    assert!(rt.state.diff_overlaid());
    let ([_, top, w, h], _) = rt.presented("diff-preview").expect("the card");
    assert!(w > 1100.0 && h > 700.0, "{:?}", (w, h));
    // Below the window's buttons.
    assert!(top >= 28.0, "{top}");
    wheel(&mut rt, "changes-diff-preview");
    let (x, y) = scrolled(&rt, "changes-diff-preview");
    assert!(x > 0.0 && y > 0.0, "{:?}", (x, y));

    assert!(!rt.state.keyboard_on_terminal(), "not the terminal under it");
    press(&mut rt, K::Escape, Mods::default());
    assert!(!rt.state.diff_overlaid(), "Esc");
    assert!(rt.state.keyboard_on_terminal(), "back to the terminal");

    click(&mut rt, "changes-preview");
    assert!(rt.state.diff_overlaid());
    press(&mut rt, K::Command('w'), Mods { cmd: true, logo: true, ..Mods::default() });
    assert!(!rt.state.diff_overlaid(), "⌘W");
    assert!(rt.state.dialog.is_none(), "the thread under it stays");
    assert!(rt.state.session_of(&key(&t)).is_some());

    click(&mut rt, "changes-preview");
    click(&mut rt, "diff-preview-close");
    assert!(!rt.state.diff_overlaid(), "×");

    click(&mut rt, "changes-preview");
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    click_at(&mut rt, 4.0, 4.0);
    assert!(!rt.state.diff_overlaid(), "a click outside");

    click(&mut rt, "changes-preview");
    assert!(rt.state.diff_overlaid());
    std::fs::write(repo.join("long.txt"), "one\n").unwrap();
    assert!(asking(&mut rt, 10.0, |s| !s.diff_overlaid()), "its change ended");
}

/// The thread on show ending while you type a commit message leaves the keyboard there if
/// the next is in the same worktree. In another, whose changes the panel lists now, Enter
/// would commit those. A click on the terminal takes the keyboard back too.
#[test]
fn a_commit_message_keeps_the_keyboard_only_in_its_worktree() {
    let server = HcServer::new("typing-moves");
    let dir = TempDir::new("typing-moves");
    let (api, web) = (repo_at(&dir.join("api")), repo_at(&dir.join("web")));
    std::fs::write(dir.join("web/new.txt"), "not ready\n").unwrap();
    let [a, c] = ["a", "c"].map(|l| server.run(&api, &[&format!("lemur.label={l}")], &["sh"]));
    let b = server.run(&web, &["lemur.label=b"], &["sh"]);
    let mut cfg = server.config();
    cfg.bases = vec![Base::new("local", &api), Base::new("local", &web)];
    let mut rt = loaded(cfg, &[&a, &b, &c]);
    assert!(refreshing(&mut rt, 5.0, |s| [&a, &b, &c].iter().all(|t| s.project_of("local", t).is_some())));
    for t in [&b, &c, &a] {
        open(&mut rt, t);
    }
    rt.update(|s, _| s.toggle_right(Right::Changes));
    click(&mut rt, "changes-message");
    assert!(rt.state.committing(), "the field has it");
    assert!(!rt.state.keyboard_on_terminal());
    rt.key(KeyEvent { key: K::Text("x".into()), mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.panels.changes.message.text, "x", "typed there");

    server.hc(&["kill", &a]);
    assert!(refreshing(&mut rt, 5.0, |s| s.active == Some(key(&c))), "c shows instead");
    assert!(rt.state.committing(), "in the same worktree, typing goes on");

    server.hc(&["kill", &c]);
    assert!(refreshing(&mut rt, 5.0, |s| s.active == Some(key(&b))), "b shows instead");
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.committing(), "in another, it doesn't");
    assert!(rt.state.keyboard_on_terminal());

    click(&mut rt, "changes-message");
    assert!(rt.state.committing());
    click(&mut rt, "term-area");
    assert!(!rt.state.committing(), "the terminal took it");
    assert!(rt.state.keyboard_on_terminal());
}

/// A changed file drags to other apps once the pointer has been over its row; a deleted one,
/// gone from the worktree, doesn't.
#[test]
fn a_changed_file_drags_out() {
    let server = HcServer::new("changes-drag");
    let host = server.config().hosts.remove(0);
    let dir = TempDir::new("changes-drag");
    let repo = dir.join("repo");
    committed(&repo, &[("a.txt", "one\n"), ("b.txt", "one\n")]);
    std::fs::write(repo.join("a.txt"), "two\n").unwrap();
    std::fs::remove_file(repo.join("b.txt")).unwrap();
    let mut rt = window(server.config());
    panel_on(&mut rt, &host, &repo);
    assert!(wait_for(&mut rt, 10.0, |s| files(s).len() == 2), "{:?}", listed(&rt));
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    let carried = |name: &str| (host.name.clone(), None, format!("{}/{name}", repo.display()));
    for row in ["change-0", "change-1"] {
        let (x, y) = rt.find(row).unwrap_or_else(|| panic!("no {row}"));
        rt.pointer_move(x, y);
        rt.frame(1.0 / 60.0);
    }
    let (a, b) = (carried("a.txt"), carried("b.txt"));
    assert!(wait_for(&mut rt, 10.0, |s| s.drag_item(&a, rune::Color::BLACK).is_some()), "a.txt ready");
    assert_eq!(rt.state.drag_item(&a, rune::Color::BLACK).unwrap().paths(), [repo.join("a.txt")]);
    assert!(rt.state.drag_item(&b, rune::Color::BLACK).is_none(), "b.txt is gone");
}
