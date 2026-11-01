//! Keeping herdr-core up to date, as Herdr does: which version runs on this
//! Mac and on each machine it reaches, which is the latest release, and
//! `hc update` and `hc machine update` to bring them up to it. Terminals
//! keep running through an update.

use crate::hc;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::process::Command;

/// Where herdr-core says its latest release is.
const LATEST: &str = "https://releases.herdr.dev/core/latest.json";

/// The protocol Lemur speaks to herdr-core; a release of another one isn't
/// offered.
const GENERATION: u64 = 1;

/// What runs where, and what's out.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Versions {
    /// This Mac's herdr-core.
    pub here: Option<String>,
    /// Installed by a package manager, which updates it instead.
    pub managed: Option<String>,
    /// Each machine this Mac's herdr-core reaches, by name.
    pub machines: Vec<(String, Option<String>)>,
    /// The latest release, when it could be asked.
    pub latest: Option<String>,
}

impl Versions {
    /// What's older than the latest release: this Mac first, then the
    /// machines, by name. Nothing when the latest isn't known.
    pub fn outdated(&self) -> Vec<Target> {
        let Some(latest) = &self.latest else {
            return vec![];
        };
        let behind = |v: &Option<String>| v.as_deref().is_some_and(|v| newer(latest, v));
        let mut out = vec![];
        if self.managed.is_none() && behind(&self.here) {
            out.push(Target::Here);
        }
        // A machine gets this Mac's: ahead of it there's nothing to give.
        let given = if out.is_empty() {
            self.here.clone()
        } else {
            Some(latest.clone())
        };
        for (name, v) in &self.machines {
            if behind(v)
                && given
                    .as_deref()
                    .is_some_and(|g| v.as_deref().is_some_and(|v| newer(g, v)))
            {
                out.push(Target::Machine(name.clone()));
            }
        }
        out
    }

    /// "this Mac 0.1.15, agent 0.1.14": the versions out of date.
    pub fn describe(&self, targets: &[Target]) -> String {
        targets
            .iter()
            .map(|t| match t {
                Target::Here => format!("this Mac {}", self.here.as_deref().unwrap_or("?")),
                Target::Machine(name) => {
                    let v = self.machines.iter().find(|(n, _)| n == name);
                    format!(
                        "{name} {}",
                        v.and_then(|(_, v)| v.as_deref()).unwrap_or("?")
                    )
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Here,
    Machine(String),
}

/// Asks this Mac's herdr-core and the release site what runs and what's
/// out. The latest is left unknown when the site can't be reached.
pub fn check() -> Result<Versions> {
    let core = hc::core(None)?;
    let status = core.call("server.status", serde_json::json!({}))?;
    let machines = core.call("machine.list", serde_json::json!({}))?;
    let mut versions = read(&status, &machines);
    versions.latest = latest().ok();
    Ok(versions)
}

/// This Mac's version and the machines', from herdr-core's answers.
fn read(status: &Value, machines: &Value) -> Versions {
    let text = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(String::from);
    let list = machines
        .get("machines")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Versions {
        here: text(&status["version"]),
        managed: text(&status["installed_by"]),
        machines: list
            .iter()
            .filter_map(|m| Some((text(&m["name"])?, text(&m["version"]))))
            .collect(),
        latest: None,
    }
}

/// The latest release of the protocol Lemur speaks.
fn latest() -> Result<String> {
    let out = Command::new("/usr/bin/curl")
        .args(["-fsSL", "--max-time", "10", LATEST])
        .output()
        .context("asking for herdr-core's latest release")?;
    if !out.status.success() {
        bail!("couldn't reach {LATEST}");
    }
    latest_in(&String::from_utf8_lossy(&out.stdout))
}

fn latest_in(json: &str) -> Result<String> {
    let v: Value = serde_json::from_str(json).context("herdr-core's latest release")?;
    if v["generation"].as_u64().is_some_and(|g| g != GENERATION) {
        bail!("the latest herdr-core speaks another protocol");
    }
    v["version"]
        .as_str()
        .map(String::from)
        .context("herdr-core's latest release has no version")
}

/// Updates each of `targets` in turn, this Mac first: what it did.
pub fn update(targets: &[Target]) -> Result<String> {
    let mut done = vec![];
    for target in targets {
        let (args, what): (Vec<&str>, String) = match target {
            Target::Here => (vec!["update"], "this Mac".into()),
            Target::Machine(name) => (vec!["machine", "update", name], name.clone()),
        };
        let out = Command::new(hc::binary())
            .args(&args)
            .output()
            .with_context(|| format!("running hc {}", args.join(" ")))?;
        if !out.status.success() {
            let said = String::from_utf8_lossy(&out.stderr).trim().to_string();
            bail!("updating herdr-core on {what}: {said}");
        }
        done.push(what);
    }
    Ok(format!("Updated herdr-core on {}", done.join(" and ")))
}

/// Whether version `a` is newer than `b`: `0.1.16` over `0.1.9`.
pub fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.trim_start_matches("hc ")
            .split(['.', '-'])
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    parts(a) > parts(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn versions_compare_by_number() {
        assert!(newer("0.1.16", "0.1.9"));
        assert!(newer("0.2.0", "0.1.99"));
        assert!(!newer("0.1.16", "0.1.16"));
        assert!(!newer("0.1.15", "0.1.16"));
    }

    #[test]
    fn what_is_out_of_date_is_updated_this_mac_first() {
        let v = read(
            &json!({"version": "0.1.15", "installed_by": null}),
            &json!({"machines": [
                {"name": "agent", "version": "0.1.14"},
                {"name": "box", "version": "0.1.16"},
                {"name": "new"}
            ]}),
        );
        assert_eq!(v.here.as_deref(), Some("0.1.15"));
        assert_eq!(v.outdated(), [], "the latest isn't known");
        let v = Versions {
            latest: Some("0.1.16".into()),
            ..v
        };
        assert_eq!(
            v.outdated(),
            [Target::Here, Target::Machine("agent".into())]
        );
        assert_eq!(v.describe(&v.outdated()), "this Mac 0.1.15, agent 0.1.14");
        // Up to date here: a machine behind gets this Mac's.
        let here = Versions {
            here: Some("0.1.16".into()),
            ..v.clone()
        };
        assert_eq!(here.outdated(), [Target::Machine("agent".into())]);
        // A package manager updates this Mac's; a machine behind this Mac's
        // still gets it.
        let managed = Versions {
            managed: Some("brew".into()),
            ..v
        };
        assert_eq!(managed.outdated(), [Target::Machine("agent".into())]);
    }

    #[test]
    fn the_latest_release_is_read_for_this_protocol() {
        let latest = |json: &str| latest_in(json).ok();
        assert_eq!(
            latest(r#"{"version":"0.1.16","generation":1}"#).as_deref(),
            Some("0.1.16")
        );
        assert_eq!(latest(r#"{"version":"0.1.16"}"#).as_deref(), Some("0.1.16"));
        assert_eq!(latest(r#"{"version":"1.0.0","generation":2}"#), None);
        assert_eq!(latest("<html>"), None);
    }
}
