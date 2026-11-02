//! Links to Claude artifacts that agents used or posted in a folder, read
//! from Claude Code's session logs on its host: `.claude/projects/<the
//! folder, by another name>/`, subagents' logs too. Only what agents wrote
//! counts, their messages and tool calls: a tool's results name artifacts
//! nobody is working on as well, as the ones it lists.

use crate::config::Host;
use crate::remote::Remote;
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

/// A folder's links, the latest first; the ones taken off the list, which
/// stay off though the logs still have them; and when its logs were last
/// read, in seconds since the epoch.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Place {
    pub links: Vec<Artifact>,
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
        if !self.removed.iter().any(|r| r == url) {
            self.removed.push(url.to_string());
        }
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

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Whole or not at all: it holds what was taken off the list.
        let partial = path.with_extension("json.partial");
        std::fs::write(&partial, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&partial, path)?;
        Ok(())
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

/// The links in the logs of folder `path` on `host`, those written in the
/// last `minutes` if given.
pub fn scan(host: &Host, path: &str, minutes: Option<u64>) -> Result<Vec<Artifact>> {
    parse(&Remote::new(host).run(&script(path, minutes))?)
}

/// What Claude Code calls a folder's logs: its path with all but letters
/// and digits made dashes.
fn logs_folder(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Prints each link in what agents wrote, as a Markdown link's text and
/// link when it's one, and each record's time after its links; then `@@end`.
fn script(path: &str, minutes: Option<u64>) -> String {
    let recent = minutes.map_or(String::new(), |m| format!("-mmin -{m} "));
    format!(
        "d=\"${{CLAUDE_CONFIG_DIR:-$HOME/.claude}}/projects/{folder}\"\n\
         [ -d \"$d\" ] && find \"$d\" -type f -name '*.jsonl' {recent}\
         -exec grep -h '\"type\":\"assistant\"' {{}} + | \
         grep -oE '(\\[[^]\"\\\\]{{1,120}}\\]\\()?(https://)?claude\\.ai/(code/)?artifact/[A-Za-z0-9-]{{16,}}|\"timestamp\":\"[^\"]*\"'\n\
         echo @@end\n",
        folder = logs_folder(path),
    )
}

/// Reads what `script` prints: no `@@end`, and it didn't get to finish.
fn parse(out: &str) -> Result<Vec<Artifact>> {
    let Some((lines, _)) = out.rsplit_once("@@end") else {
        bail!("couldn't read Claude's logs");
    };
    let mut found: Vec<Artifact> = vec![];
    // Those at the end of `found` still waiting for their record's time.
    let mut undated = 0;
    for line in lines.lines() {
        if let Some(at) = line
            .strip_prefix("\"timestamp\":\"")
            .and_then(|l| l.strip_suffix('"'))
        {
            let from = found.len() - undated;
            for artifact in &mut found[from..] {
                artifact.at = at.to_string();
            }
            undated = 0;
        } else if let Some(i) = line.find("claude.ai/") {
            let title = line
                .strip_prefix('[')
                .and_then(|l| l.split_once("]("))
                .map(|(text, _)| text.replace(['*', '`'], "").trim().to_string())
                .filter(|text| !text.is_empty());
            found.push(Artifact {
                url: format!("https://{}", &line[i..]),
                title,
                at: String::new(),
            });
            undated += 1;
        }
    }
    Ok(found)
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
    fn run(config: &Path, path: &str, minutes: Option<u64>) -> Vec<Artifact> {
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(script(path, minutes))
            .env("CLAUDE_CONFIG_DIR", config)
            .output()
            .unwrap();
        parse(&String::from_utf8_lossy(&out.stdout)).unwrap()
    }

    #[test]
    fn a_folders_logs_go_by_its_path_with_dashes() {
        assert_eq!(
            logs_folder("/Users/me/laft-web/.worktrees/a_b/web"),
            "-Users-me-laft-web--worktrees-a-b-web"
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
        assert_eq!(parse("@@end\n").unwrap(), []);
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
    }
}
