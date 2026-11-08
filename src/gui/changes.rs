//! Beside the branch in the status bar: the lines added and removed in the
//! worktree on show since its last commit, staged or not, as Herdr's
//! Changes counts them (`git diff HEAD --numstat`). Files git doesn't track
//! yet aren't counted. Asked when a worktree comes on show, then every few
//! seconds.

use super::Lemur;
use crate::config::Host;
use crate::remote::{Remote, sh_path};
use rune::{El, row, text};
use rune_kit::Kit;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// How often the worktree on show is asked: while Lemur is in front, and
/// when it isn't.
pub(super) const EVERY: Duration = Duration::from_secs(10);
pub(super) const AWAY: Duration = Duration::from_secs(60);

/// A worktree's changes since its last commit.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(super) struct Diff {
    pub(super) files: u64,
    pub(super) added: u64,
    pub(super) removed: u64,
}

/// What's known of each worktree asked, by host and path.
#[derive(Default)]
pub(super) struct Changes {
    known: HashMap<(String, String), Diff>,
    pub(super) asking: HashSet<(String, String)>,
}

impl Lemur {
    /// Asks the worktree on show for its changes, in the background.
    pub(super) fn ask_changes(&mut self) {
        let Some((host, w)) = self.active_worktree() else {
            return;
        };
        let place = (host.to_string(), w.path.clone());
        let Some(host) = self.cfg.host(host).cloned() else {
            return;
        };
        if !self.diffs.asking.insert(place.clone()) {
            return;
        }
        let path = place.1.clone();
        self.in_background(
            move || diff_in(&host, &path),
            move |this, diff| {
                this.diffs.asking.remove(&place);
                // Not a repository, or no answer: nothing to show.
                match diff {
                    Some(diff) => this.diffs.known.insert(place, diff),
                    None => this.diffs.known.remove(&place),
                };
            },
        );
    }

    /// "+12 -3" for the worktree on show, once known and while it has
    /// changes; how many files on hover.
    pub(super) fn changes_item(&self, host: &str, path: &str, k: &Kit) -> Option<El> {
        let diff = *self.diffs.known.get(&(host.to_string(), path.to_string()))?;
        if diff.files == 0 {
            return None;
        }
        let tip = format!(
            "{} file{} changed since the last commit, staged or not. Files git doesn't track yet aren't counted.",
            diff.files,
            if diff.files == 1 { "" } else { "s" }
        );
        Some(
            row()
                .key("changes")
                .gap(4.0)
                .help(tip)
                .child(text(format!("+{}", diff.added)).font(k.ty.small).color(k.c.success.fg))
                .child(text(format!("-{}", diff.removed)).font(k.ty.small).color(k.c.danger.fg)),
        )
    }
}

/// The changes in the worktree at `path` on `host`; None where git can't
/// say, as outside a repository.
fn diff_in(host: &Host, path: &str) -> Option<Diff> {
    let out = Remote::new(host).run(&script(path)).ok()?;
    numstat(&out)
}

/// Prints the worktree's changes from HEAD, one NUL-ended record a file,
/// then `@@end` if git could say; with no commit yet, what's staged, as
/// the Changes panel lists. Without writing the index, which would hold
/// its lock: an agent's `git add` meanwhile would fail.
fn script(path: &str) -> String {
    format!(
        "cd {} 2>/dev/null || exit 0\n\
         {{ git -c diff.autoRefreshIndex=false diff HEAD --numstat -z --no-renames 2>/dev/null \
         || git -c diff.autoRefreshIndex=false diff --cached --numstat -z --no-renames 2>/dev/null; }} \
         && echo @@end\n\
         exit 0\n",
        sh_path(path)
    )
}

/// Sums `git diff --numstat -z`: "added\tremoved\tpath" a file, "-" for a
/// binary file's counts.
fn numstat(out: &str) -> Option<Diff> {
    let (records, _) = out.rsplit_once("@@end")?;
    let mut diff = Diff::default();
    for record in records.split('\0') {
        let mut fields = record.trim_start_matches('\n').splitn(3, '\t');
        let (Some(added), Some(removed), Some(_)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        diff.files += 1;
        diff.added += added.parse::<u64>().unwrap_or(0);
        diff.removed += removed.parse::<u64>().unwrap_or(0);
    }
    Some(diff)
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's `test` would stand in for the plain one.
    use super::{Diff, numstat, script};
    use crate::test_support::{TempDir, git_in};

    #[test]
    fn numstat_is_summed() {
        let out = "12\t3\tsrc/a.rs\0-\t-\tlogo.png\0\u{31}\t0\twith\ttab.txt\0@@end\n";
        assert_eq!(
            numstat(out),
            Some(Diff {
                files: 3,
                added: 13,
                removed: 3
            })
        );
        assert_eq!(numstat("@@end\n"), Some(Diff::default()), "clean");
        assert_eq!(numstat(""), None, "git couldn't say");
    }

    /// Run in a real repository: staged and unstaged changes count, files
    /// git doesn't track don't; outside a repository, nothing.
    #[test]
    fn the_script_counts_changes_since_the_last_commit() {
        let dir = TempDir::new("changes");
        let repo = dir.join("my repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        git_in(&repo, &["add", "a.txt"]);
        git_in(&repo, &["commit", "-q", "-m", "init"]);
        let run = |path: &std::path::Path| {
            let out = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(script(&path.to_string_lossy()))
                .output()
                .unwrap();
            numstat(&String::from_utf8_lossy(&out.stdout))
        };
        assert_eq!(run(&repo), Some(Diff::default()));

        std::fs::write(repo.join("a.txt"), "one\n2\nthree\nfour\n").unwrap();
        std::fs::write(repo.join("b.txt"), "new\n").unwrap();
        git_in(&repo, &["add", "b.txt"]);
        std::fs::write(repo.join("untracked.txt"), "x\ny\n").unwrap();
        assert_eq!(
            run(&repo),
            Some(Diff {
                files: 2,
                added: 3,
                removed: 1
            })
        );
        assert_eq!(run(&dir), None, "not a repository");
        assert_eq!(run(&dir.join("gone")), None);
        // No commit yet: what's staged, as the Changes panel lists.
        let fresh = dir.join("fresh");
        std::fs::create_dir(&fresh).unwrap();
        git_in(&fresh, &["init", "-q", "-b", "main"]);
        std::fs::write(fresh.join("a.txt"), "one\ntwo\n").unwrap();
        git_in(&fresh, &["add", "a.txt"]);
        assert_eq!(
            run(&fresh),
            Some(Diff {
                files: 1,
                added: 2,
                removed: 0
            })
        );

        // A file touched but not changed leaves the index as it is: its lock
        // is the one an agent's `git add` takes.
        git_in(&repo, &["add", "-A"]);
        git_in(&repo, &["commit", "-q", "-m", "two"]);
        let index = repo.join(".git/index");
        let written = || std::fs::metadata(&index).unwrap().modified().unwrap();
        let before = written();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let later = std::time::SystemTime::now();
        std::fs::File::options()
            .write(true)
            .open(repo.join("a.txt"))
            .unwrap()
            .set_modified(later)
            .unwrap();
        run(&repo);
        assert_eq!(written(), before, "the index was written");
    }
}
