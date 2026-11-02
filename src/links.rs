//! Links to Claude artifacts and GitHub pull requests that agents used or
//! posted in a folder, read from Claude Code's session logs on its host:
//! `.claude/projects/<the folder, by another name>/`, subagents' logs too.
//! Only what agents wrote counts, their messages and tool calls: a tool's
//! results name artifacts nobody is working on as well, as the ones it
//! lists. What's known of a pull request comes from `gh`.

use crate::config::Host;
use crate::gh::{PullRequest, State};
use crate::remote::{Remote, sh_path};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// A link found: what the agent called it, if it did, and when it first
/// came up, as the log has it (ISO 8601 in UTC, so its order is the time's).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    pub url: String,
    pub title: Option<String>,
    pub at: String,
}

impl Artifact {
    /// What to call it: its title, or the end of its link.
    pub fn name(&self) -> &str {
        self.title
            .as_deref()
            .unwrap_or_else(|| self.url.rsplit('/').next().unwrap_or(&self.url))
    }
}

/// A folder's links, the latest first; its pull requests, the ones still
/// open first; the links taken off the list, which stay off though the logs
/// still have them; and when its logs were last read, in seconds since the
/// epoch.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Place {
    pub links: Vec<Artifact>,
    #[serde(default)]
    pub prs: Vec<PullRequest>,
    /// Pull requests agents linked to that `gh` hasn't told of yet.
    #[serde(default)]
    pending: Vec<u64>,
    #[serde(default)]
    removed: Vec<String>,
    #[serde(default)]
    pub looked: Option<u64>,
}

impl Place {
    /// Adds what the logs say: a link known already keeps its place, and
    /// takes a title if it had none.
    pub fn add(&mut self, found: Vec<Artifact>) {
        for artifact in found {
            if self.removed.contains(&artifact.url) {
                continue;
            }
            match self.links.iter_mut().find(|l| l.url == artifact.url) {
                Some(known) => {
                    if known.title.is_none() {
                        known.title = artifact.title;
                    }
                    if known.at.is_empty() || (!artifact.at.is_empty() && artifact.at < known.at) {
                        known.at = artifact.at;
                    }
                }
                None => self.links.push(artifact),
            }
        }
        self.links.sort_by(|a, b| b.at.cmp(&a.at));
    }

    /// Takes a link off the list for good.
    pub fn remove(&mut self, url: &str) {
        self.links.retain(|l| l.url != url);
        self.prs.retain(|pr| pr.url != url);
        if !self.removed.iter().any(|r| r == url) {
            self.removed.push(url.to_string());
        }
    }

    /// The pull requests to ask `gh` about every time: those still open, and
    /// those linked to that it hasn't told of yet. Not the merged ones, which
    /// stay merged, or the closed: rarely reopened, and asked again once
    /// agents link to one again.
    pub fn wanted(&self) -> Vec<u64> {
        let open = |pr: &&PullRequest| matches!(pr.state, State::Open | State::Draft);
        let known = self.prs.iter().filter(open);
        let mut wanted: Vec<u64> = known.map(|pr| pr.number).collect();
        wanted.extend(&self.pending);
        wanted
    }

    /// The pull requests not to ask `gh` about though agents link to them:
    /// the merged ones, and those taken off the list.
    pub fn finished(&self) -> Vec<u64> {
        let merged = self.prs.iter().filter(|pr| pr.state == State::Merged);
        let removed = self
            .removed
            .iter()
            .filter_map(|url| url.rsplit_once("/pull/")?.1.parse().ok());
        merged.map(|pr| pr.number).chain(removed).collect()
    }

    /// Notes pull requests agents linked to.
    pub fn link_prs(&mut self, numbers: &[u64]) {
        for &number in numbers {
            let end = format!("/pull/{number}");
            let known = self.pending.contains(&number)
                || self.prs.iter().any(|pr| pr.number == number)
                || self.removed.iter().any(|url| url.ends_with(&end));
            if !known {
                self.pending.push(number);
            }
        }
    }

    /// Takes in what `gh` told: `found`, new or as they are now, unless taken
    /// off the list, and `settled`, no longer waiting for it.
    pub fn add_prs(&mut self, found: Vec<PullRequest>, settled: &[u64]) {
        self.pending.retain(|number| !settled.contains(number));
        for pr in found {
            if self.removed.contains(&pr.url) {
                continue;
            }
            match self.prs.iter_mut().find(|known| known.url == pr.url) {
                Some(known) => *known = pr,
                None => self.prs.push(pr),
            }
        }
        let finished = |pr: &PullRequest| matches!(pr.state, State::Merged | State::Closed);
        self.prs
            .sort_by_key(|pr| (finished(pr), std::cmp::Reverse(pr.number)));
    }

    /// How far back to read at `now`: logs written since the last look, with
    /// room for the host's clock differing; every log the first time.
    pub fn minutes(&self, now: u64) -> Option<u64> {
        self.looked.map(|at| now.saturating_sub(at) / 60 + 5)
    }
}

/// Every folder's links, by host and folder.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Store(BTreeMap<String, Place>);

impl Store {
    /// What's in `path`; nothing if it's missing or unreadable.
    pub fn load(path: &Path) -> Store {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Saves it, whole or not at all: it holds what was taken off the list.
    /// Another Lemur (the app, and one built here) may have saved since: what
    /// that took off the list stays off, and folders only it knows stay.
    pub fn save(&mut self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        for (key, theirs) in Store::load(path).0 {
            match self.0.get_mut(&key) {
                Some(mine) => {
                    for url in &theirs.removed {
                        mine.remove(url);
                    }
                }
                None => {
                    self.0.insert(key, theirs);
                }
            }
        }
        crate::config::write_whole(path, &serde_json::to_string_pretty(self)?)
    }

    pub fn place(&self, host: &str, path: &str) -> Option<&Place> {
        self.0.get(&key(host, path))
    }

    pub fn place_mut(&mut self, host: &str, path: &str) -> &mut Place {
        self.0.entry(key(host, path)).or_default()
    }
}

fn key(host: &str, path: &str) -> String {
    format!("{host}\n{path}")
}

/// What a look at a folder finds.
#[derive(Debug, Default, PartialEq)]
pub struct Scan {
    pub artifacts: Vec<Artifact>,
    /// The GitHub repository of its `origin`, as `gh --repo` takes it.
    pub repo: Option<String>,
    /// The branch checked out, unless it's the repository's default: that
    /// isn't any pull request's.
    pub branch: Option<String>,
    /// The pull requests of `repo` agents linked to, by number.
    pub prs: Vec<u64>,
}

/// What's in the logs of folder `path` on `host`, those written in the last
/// `minutes` if given, and its repository and branch.
pub fn scan(host: &Host, path: &str, minutes: Option<u64>) -> Result<Scan> {
    parse(&Remote::new(host).run_whole(&script(path, minutes))?)
}

/// What Claude Code calls a folder's logs: its path with all but letters
/// and digits made dashes.
fn logs_folder(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Prints the folder's repository and branch; then each link in what agents
/// wrote, as a Markdown link's text and link when it's an artifact's, and
/// the time of each record with one after its links; then `@@end`. Records
/// with none print nothing: a time for each would pass what herdr-core keeps
/// of the output at some 3 GB of logs.
fn script(path: &str, minutes: Option<u64>) -> String {
    let recent = minutes.map_or(String::new(), |m| format!("-mmin -{m} "));
    format!(
        "cd {dir} 2>/dev/null && {{\n\
         echo \"@@origin $(git remote get-url origin 2>/dev/null)\"\n\
         echo \"@@branch $(git symbolic-ref --short HEAD 2>/dev/null)\"\n\
         echo \"@@default $(git symbolic-ref --short refs/remotes/origin/HEAD 2>/dev/null)\"\n\
         }}\n\
         d=\"${{CLAUDE_CONFIG_DIR:-$HOME/.claude}}/projects/{folder}\"\n\
         [ -d \"$d\" ] && find \"$d\" -type f -name '*.jsonl' {recent}\
         -exec grep -h '\"type\":\"assistant\"' {{}} + | \
         grep -E 'claude\\.ai/|github\\.com/' | \
         grep -oE '(\\[[^]\"\\\\]{{1,120}}\\]\\()?(https://)?claude\\.ai/(code/)?artifact/[A-Za-z0-9-]{{16,}}|https://github\\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+/pull/[0-9]+|\"timestamp\":\"[^\"]*\"'\n\
         echo @@end\n",
        dir = sh_path(path),
        folder = logs_folder(path),
    )
}

/// Reads what `script` prints: no `@@end`, and it didn't get to finish.
fn parse(out: &str) -> Result<Scan> {
    let Some((lines, _)) = out.rsplit_once("@@end") else {
        bail!("couldn't read Claude's logs");
    };
    let mut found: Vec<Artifact> = vec![];
    let (mut branch, mut default) = ("", "");
    let mut repo = None;
    let mut prs = vec![];
    // Those at the end of `found` still waiting for their record's time.
    let mut undated = 0;
    for line in lines.lines() {
        if let Some(remote) = line.strip_prefix("@@origin ") {
            repo = crate::gh::repo_of(remote);
        } else if let Some(name) = line.strip_prefix("@@branch ") {
            branch = name;
        } else if let Some(name) = line.strip_prefix("@@default ") {
            default = name.strip_prefix("origin/").unwrap_or(name);
        } else if let Some(link) = line.strip_prefix("https://github.com/") {
            if let Some((name, number)) = link.split_once("/pull/")
                && let Ok(number) = number.parse::<u64>()
            {
                prs.push((name.to_string(), number));
            }
        } else if let Some(at) = line
            .strip_prefix("\"timestamp\":\"")
            .and_then(|l| l.strip_suffix('"'))
        {
            let from = found.len() - undated;
            for artifact in &mut found[from..] {
                artifact.at = at.to_string();
            }
            undated = 0;
        } else if let Some(i) = line.rfind("claude.ai/") {
            // The last: a link's text may name its address too, and that
            // says nothing the address doesn't.
            let title = line
                .strip_prefix('[')
                .and_then(|l| l.split_once("]("))
                .map(|(text, _)| text.replace(['*', '`'], "").trim().to_string())
                .filter(|text| !text.is_empty() && !text.contains("claude.ai/"));
            found.push(Artifact {
                url: format!("https://{}", &line[i..]),
                title,
                at: String::new(),
            });
            undated += 1;
        }
    }
    // Without a word on the default, by what it usually is.
    let is_default = match default {
        "" => matches!(branch, "main" | "master"),
        default => branch == default,
    };
    let mut numbers: Vec<u64> = prs
        .into_iter()
        .filter(|(name, _)| repo.as_ref().is_some_and(|r| r.eq_ignore_ascii_case(name)))
        .map(|(_, number)| number)
        .collect();
    numbers.sort_unstable();
    numbers.dedup();
    Ok(Scan {
        artifacts: found,
        branch: (!branch.is_empty() && !is_default).then(|| branch.to_string()),
        repo,
        prs: numbers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    const ONE: &str = "1hJWkh1hjNLMZjGxLk1zCD";
    const TWO: &str = "2VWjSVh3UGph6KPNmbpbzY";
    const UUID: &str = "3c294ca3-861d-415e-998a-101075a5e23c";

    fn link(id: &str, at: &str) -> Artifact {
        Artifact {
            url: format!("https://claude.ai/artifact/{id}"),
            title: None,
            at: at.into(),
        }
    }

    /// Runs the script on logs in `config`, as a host would.
    fn scanned(config: &Path, path: &str, minutes: Option<u64>) -> Scan {
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(script(path, minutes))
            .env("CLAUDE_CONFIG_DIR", config)
            .output()
            .unwrap();
        parse(&String::from_utf8_lossy(&out.stdout)).unwrap()
    }

    fn run(config: &Path, path: &str, minutes: Option<u64>) -> Vec<Artifact> {
        scanned(config, path, minutes).artifacts
    }

    fn pr(number: u64, state: State) -> PullRequest {
        PullRequest {
            url: format!("https://github.com/o/r/pull/{number}"),
            number,
            title: format!("number {number}"),
            state,
        }
    }

    #[test]
    fn a_folders_logs_go_by_its_path_with_dashes() {
        assert_eq!(
            logs_folder("/Users/me/laft-web/.worktrees/a_b/web"),
            "-Users-me-laft-web--worktrees-a-b-web"
        );
    }

    /// Two Lemurs keep one file: what either took off the list stays off,
    /// whichever saves last.
    #[test]
    fn what_another_lemur_removed_stays_removed() {
        let dir = TempDir::new("links-two");
        let file = dir.join("links.json");
        let mut first = Store::default();
        let place = first.place_mut("agent", "/srv/api");
        place.add(vec![link(ONE, "1"), link(TWO, "2")]);
        first.save(&file).unwrap();
        let mut second = Store::load(&file);

        first
            .place_mut("agent", "/srv/api")
            .remove(&link(ONE, "").url);
        first
            .place_mut("agent", "/srv/web")
            .add(vec![link(TWO, "3")]);
        first.save(&file).unwrap();
        second.save(&file).unwrap();

        let kept = Store::load(&file);
        let api = kept.place("agent", "/srv/api").unwrap();
        assert_eq!(api.links, [link(TWO, "2")]);
        assert!(api.removed.contains(&link(ONE, "").url));
        assert!(
            kept.place("agent", "/srv/web").is_some(),
            "its folder stays"
        );
        assert_eq!(
            second.place("agent", "/srv/api").unwrap().links,
            [link(TWO, "2")],
            "the one that saved last shows it too"
        );
    }

    /// Records with no link print nothing, not even their time.
    #[test]
    fn records_without_a_link_print_nothing() {
        let config = TempDir::new("artifact-quiet");
        let logs = config.join("projects/-srv-api");
        std::fs::create_dir_all(&logs).unwrap();
        let record = |text: String, at: &str| {
            format!("{{\"message\":\"{text}\",\"type\":\"assistant\",\"timestamp\":\"{at}\"}}")
        };
        let log = [
            record("nothing here".into(), "T1"),
            record(format!("see https://claude.ai/artifact/{ONE}"), "T2"),
        ]
        .join("\n");
        std::fs::write(logs.join("a.jsonl"), log + "\n").unwrap();
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(script("/srv/api", None))
            .env("CLAUDE_CONFIG_DIR", &*config)
            .output()
            .unwrap();
        let printed = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(!printed.contains("T1"), "{printed}");
        assert!(printed.contains("T2"), "{printed}");
    }

    /// A Markdown link whose text is its address, or names the site, goes by
    /// its address, and has no title.
    #[test]
    fn a_links_text_can_name_its_address() {
        let out = format!(
            "[https://claude.ai/artifact/{ONE}](https://claude.ai/artifact/{ONE}\n\
             [see claude.ai/artifact docs](https://claude.ai/artifact/{TWO}\n@@end\n"
        );
        let found = parse(&out).unwrap().artifacts;
        assert_eq!(
            found,
            [link(ONE, ""), link(TWO, "")],
            "the addresses, no titles"
        );
    }

    /// As Claude Code writes them: what agents wrote counts, in messages
    /// and tool calls and by subagents, not what people or tools did, nor
    /// what only looks like a link.
    #[test]
    fn links_are_read_from_what_agents_wrote() {
        let config = TempDir::new("artifact-logs");
        let logs = config.join("projects/-srv-api");
        std::fs::create_dir_all(logs.join("session/subagents")).unwrap();
        let record = |kind: &str, content: String, at: &str| {
            format!(
                "{{\"parentUuid\":null,\"message\":{{\"role\":\"x\",\"content\":{content}}},\
                 \"type\":\"{kind}\",\"uuid\":\"u\",\"timestamp\":\"{at}\"}}"
            )
        };
        let log = [
            record(
                "assistant",
                format!(
                    "[{{\"type\":\"text\",\"text\":\"The doc is ready: [**Handover** doc](https://claude.ai/artifact/{ONE}).\\nMore: https://claude.ai/code/artifact/{UUID}#page\"}}]"
                ),
                "2026-11-01T10:00:00.000Z",
            ),
            record(
                "user",
                "[{\"type\":\"tool_result\",\"content\":\"Created from the type https://claude.ai/artifact/QKN21svewxgyPb6SYRqWnd\"}]"
                    .to_string(),
                "2026-11-01T10:30:00.000Z",
            ),
            record(
                "user",
                "\"Look at https://claude.ai/artifact/PastedByAPerson1234567\"".to_string(),
                "2026-11-01T10:40:00.000Z",
            ),
            record(
                "assistant",
                "\"Its links are claude.ai/artifact/{id} and claude.ai/artifact/short\"".to_string(),
                "2026-11-01T10:50:00.000Z",
            ),
        ]
        .join("\n");
        std::fs::write(logs.join("session.jsonl"), log + "\n").unwrap();
        let tool_call = record(
            "assistant",
            format!(
                "[{{\"type\":\"tool_use\",\"name\":\"Artifact\",\"input\":{{\"action\":\"read\",\"url\":\"https://claude.ai/artifact/{TWO}\"}}}}]"
            ),
            "2026-11-02T08:00:00.000Z",
        );
        std::fs::write(logs.join("session/subagents/agent.jsonl"), tool_call + "\n").unwrap();

        let mut place = Place::default();
        place.add(run(&config, "/srv/api", None));
        let got: Vec<(&str, Option<&str>, &str)> = place
            .links
            .iter()
            .map(|a| (a.url.as_str(), a.title.as_deref(), a.at.as_str()))
            .collect();
        let (one, two, uuid) = (
            format!("https://claude.ai/artifact/{ONE}"),
            format!("https://claude.ai/artifact/{TWO}"),
            format!("https://claude.ai/code/artifact/{UUID}"),
        );
        assert_eq!(
            got,
            [
                (two.as_str(), None, "2026-11-02T08:00:00.000Z"),
                (
                    one.as_str(),
                    Some("Handover doc"),
                    "2026-11-01T10:00:00.000Z"
                ),
                (uuid.as_str(), None, "2026-11-01T10:00:00.000Z"),
            ]
        );

        assert!(run(&config, "/srv/other", None).is_empty(), "no such logs");
    }

    /// After the first look, only logs written since are read: they can be
    /// hundreds of megabytes.
    #[test]
    fn only_logs_written_lately_are_read_again() {
        let config = TempDir::new("artifact-recent");
        let logs = config.join("projects/-srv-api");
        std::fs::create_dir_all(&logs).unwrap();
        let log = logs.join("old.jsonl");
        std::fs::write(
            &log,
            format!(
                "{{\"type\":\"assistant\",\"text\":\"https://claude.ai/artifact/{ONE}\",\"timestamp\":\"t\"}}\n"
            ),
        )
        .unwrap();
        assert_eq!(run(&config, "/srv/api", Some(5)).len(), 1, "just written");
        let long_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&log)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        assert!(run(&config, "/srv/api", Some(5)).is_empty());
        assert_eq!(run(&config, "/srv/api", None).len(), 1, "the first look");
    }

    #[test]
    fn a_script_that_didnt_finish_says_so() {
        assert!(parse("").is_err());
        assert_eq!(parse("@@end\n").unwrap(), Scan::default());
    }

    /// A folder's repository and branch come from git, and the pull requests
    /// agents linked to from the logs, only that repository's: an agent
    /// mentioning another project's isn't working on it.
    #[test]
    fn a_folders_repository_and_pull_requests_are_read() {
        let config = TempDir::new("pr-scan");
        let dir = config.join("my repo");
        std::fs::create_dir(&dir).unwrap();
        let git = |args: &[&str]| crate::test_support::git_in(&dir, args);
        git(&["init", "-q", "-b", "main"]);
        git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        let path = dir.to_string_lossy().into_owned();
        let logs = config.join("projects").join(logs_folder(&path));
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(
            logs.join("a.jsonl"),
            "{\"text\":\"See https://github.com/Owner/Api/pull/12/files and [#3](https://github.com/owner/api/pull/3), not https://github.com/zed-industries/zed/pull/38639\",\"type\":\"assistant\",\"timestamp\":\"t\"}\n\
             {\"text\":\"Nor one a person pasted: https://github.com/owner/api/pull/4\",\"type\":\"user\",\"timestamp\":\"t\"}\n\
             {\"text\":\"https://github.com/owner/api/pull/12 again\",\"type\":\"assistant\",\"timestamp\":\"t\"}\n",
        )
        .unwrap();

        let no_remote = scanned(&config, &path, None);
        assert_eq!(no_remote.repo, None);
        assert_eq!(no_remote.prs, [] as [u64; 0], "no repository to be of");
        assert_eq!(no_remote.branch, None, "main, the usual default");

        git(&["remote", "add", "origin", "git@github.com:owner/api.git"]);
        let on_main = scanned(&config, &path, None);
        assert_eq!(on_main.repo.as_deref(), Some("owner/api"));
        assert_eq!(on_main.prs, [3, 12]);
        assert_eq!(on_main.branch, None);

        git(&["checkout", "-q", "-b", "fix/thing"]);
        let on_branch = scanned(&config, &path, None);
        assert_eq!(on_branch.branch.as_deref(), Some("fix/thing"));

        // A repository whose default isn't main or master says so.
        git(&["update-ref", "refs/remotes/origin/trunk", "HEAD"]);
        git(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/trunk",
        ]);
        git(&["checkout", "-q", "-b", "trunk"]);
        assert_eq!(scanned(&config, &path, None).branch, None);
        git(&["checkout", "-q", "main"]);
        let main_here = scanned(&config, &path, None);
        assert_eq!(main_here.branch.as_deref(), Some("main"));

        let gone = scanned(&config, &config.join("gone").to_string_lossy(), None);
        assert_eq!(gone, Scan::default());
    }

    /// A place's pull requests: the ones still open first, as `gh` last told;
    /// a merged one isn't asked after again, and one taken off stays off.
    #[test]
    fn a_place_keeps_its_pull_requests_as_told() {
        let mut place = Place::default();
        place.link_prs(&[7, 8, 9]);
        assert_eq!(place.wanted(), [7, 8, 9]);

        place.add_prs(
            vec![
                pr(7, State::Merged),
                pr(8, State::Draft),
                pr(6, State::Open),
            ],
            &[7, 8],
        );
        let numbers = |place: &Place| place.prs.iter().map(|p| p.number).collect::<Vec<_>>();
        assert_eq!(numbers(&place), [8, 6, 7], "the merged one last");
        assert_eq!(place.wanted(), [8, 6, 9], "9 not told of yet; 7 is merged");

        place.add_prs(vec![pr(8, State::Open)], &[8]);
        assert_eq!(place.prs[0].state, State::Open, "as it is now");
        assert_eq!(place.prs.len(), 3);

        place.remove(&pr(8, State::Open).url);
        assert_eq!(numbers(&place), [6, 7]);
        place.add_prs(vec![pr(8, State::Open)], &[8]);
        assert_eq!(numbers(&place), [6, 7], "it stays off");
        place.link_prs(&[8, 6, 10]);
        assert_eq!(place.wanted(), [6, 9, 10], "known or taken off aren't new");
        assert_eq!(place.finished(), [7, 8], "merged, and taken off");
    }

    /// A link taken off stays off; one known keeps its first time, and takes
    /// a title it comes by later.
    #[test]
    fn a_place_keeps_its_links_as_told() {
        let mut place = Place::default();
        place.add(vec![link(ONE, "2026-11-02"), link(TWO, "2026-11-03")]);
        assert_eq!(place.links[0], link(TWO, "2026-11-03"), "the latest first");

        let mut titled = link(ONE, "2026-11-04");
        titled.title = Some("Handover".into());
        place.add(vec![titled]);
        let one = &place.links[1];
        assert_eq!(one.name(), "Handover");
        assert_eq!(one.at, "2026-11-02");

        place.remove(&place.links[1].url.clone());
        assert_eq!(place.links.len(), 1);
        place.add(vec![link(ONE, "2026-11-05")]);
        assert_eq!(place.links.len(), 1, "it stays off");
        assert_eq!(place.links[0].name(), TWO);
    }

    #[test]
    fn links_are_saved_with_what_was_taken_off() {
        let dir = TempDir::new("artifact-store");
        let file = dir.join("more/artifacts.json");
        let mut store = Store::default();
        let place = store.place_mut("agent", "/srv/api");
        place.add(vec![link(ONE, "a"), link(TWO, "b")]);
        place.remove(&format!("https://claude.ai/artifact/{ONE}"));
        place.looked = Some(600);
        store.save(&file).unwrap();

        let mut back = Store::load(&file);
        assert!(back.place("agent", "/srv/other").is_none());
        let place = back.place_mut("agent", "/srv/api");
        assert_eq!(place.links, [link(TWO, "b")]);
        assert_eq!(place.minutes(1200), Some(15));
        assert_eq!(Place::default().minutes(1200), None);
        place.add(vec![link(ONE, "a")]);
        assert_eq!(place.links.len(), 1);

        assert!(
            Store::load(&dir.join("gone.json"))
                .place("a", "b")
                .is_none()
        );

        // What pull requests' fields came after still reads.
        std::fs::write(
            &file,
            format!(
                "{{\"agent\\n/srv/api\":{{\"links\":[{{\"url\":\"https://claude.ai/artifact/{ONE}\",\"title\":null,\"at\":\"a\"}}]}}}}"
            ),
        )
        .unwrap();
        let old = Store::load(&file);
        let place = old.place("agent", "/srv/api").unwrap();
        assert_eq!(place.links.len(), 1);
        assert!(place.prs.is_empty());

        let mut with_prs = Store::default();
        with_prs
            .place_mut("agent", "/srv/api")
            .add_prs(vec![pr(5, State::Draft)], &[]);
        with_prs.save(&file).unwrap();
        let back = Store::load(&file);
        let told = &back.place("agent", "/srv/api").unwrap().prs;
        assert_eq!(*told, [pr(5, State::Draft)]);
    }
}
