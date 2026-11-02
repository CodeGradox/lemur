//! Keeping herdr-core up to date, as Herdr does: which version runs on this
//! Mac and on each machine it reaches, which is the latest release, and
//! `hc update` and `hc machine update` to bring them up to it. Terminals
//! keep running through an update.

use crate::config::Config;
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

/// The herdr-cores the config has Lemur use on this Mac: the local host's
/// session (`hc_session`; herdr-core's default without one), and the
/// default one, which reaches the other hosts, if there are any.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cores {
    pub here: Option<String>,
    pub machines: bool,
}

impl Cores {
    pub fn of(cfg: &Config) -> Cores {
        let (local, far): (Vec<_>, Vec<_>) = cfg.hosts.iter().partition(|h| h.ssh == "local");
        Cores {
            here: local.first().and_then(|h| h.hc_session.clone()),
            machines: !far.is_empty(),
        }
    }
}

/// Asks this Mac's herdr-core and the release site what runs and what's
/// out. The latest is left unknown when the site can't be reached.
pub fn check(cores: &Cores) -> Result<Versions> {
    let mut versions = installed(cores)?;
    versions.latest = latest().ok();
    Ok(versions)
}

/// What runs: asking only the cores Lemur uses, as asking one starts it.
fn installed(cores: &Cores) -> Result<Versions> {
    let core = hc::core(cores.here.as_deref())?;
    let status = core.call("server.status", serde_json::json!({}))?;
    if !cores.machines {
        return Ok(read(&status, &serde_json::json!({})));
    }
    let default = hc::core(None)?;
    let machines = default.call("machine.list", serde_json::json!({}))?;
    let mut versions = read(&status, &machines);
    // The local host in a session of its own: this Mac runs two, and is as
    // up to date as the older.
    if cores.here.is_some() {
        let other = default.call("server.status", serde_json::json!({}))?;
        versions.here = older(versions.here, read(&other, &machines).here);
    }
    Ok(versions)
}

/// The older of two versions, or the one known.
fn older(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if newer(&a, &b) { b } else { a }),
        (a, b) => a.or(b),
    }
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
pub fn update(targets: &[Target], cores: &Cores) -> Result<String> {
    // One at a time: Settings' Update and the menu's each start one.
    static UPDATING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    struct Updating;
    impl Drop for Updating {
        fn drop(&mut self) {
            UPDATING.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
    if UPDATING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        bail!("herdr-core is being updated already");
    }
    let _updating = Updating;
    let mut done = vec![];
    for target in targets {
        let (args, what): (Vec<&str>, String) = match target {
            Target::Here => (vec!["update"], "this Mac".into()),
            Target::Machine(name) => (vec!["machine", "update", name], name.clone()),
        };
        for session in sessions_for(target, cores) {
            let mut command = Command::new(hc::binary());
            if let Some(session) = session {
                command.env("HC_SESSION", session);
            }
            let out = command
                .args(&args)
                .output()
                .with_context(|| format!("running hc {}", args.join(" ")))?;
            if !out.status.success() {
                let said = String::from_utf8_lossy(&out.stderr).trim().to_string();
                bail!("updating herdr-core on {what}: {said}");
            }
        }
        done.push(what);
    }
    Ok(format!("Updated herdr-core on {}", done.join(" and ")))
}

/// The sessions to run `target`'s update in. `hc update` restarts the core
/// of its session, and any other that's older only when run in that one: with
/// the local host in a session of its own, its own, then the default that
/// reaches the other hosts, or those would go on through the old build.
fn sessions_for<'a>(target: &Target, cores: &'a Cores) -> Vec<Option<&'a str>> {
    match (target, cores.here.as_deref()) {
        (Target::Here, Some(session)) if cores.machines => vec![Some(session), None],
        (Target::Here, Some(session)) => vec![Some(session)],
        _ => vec![None],
    }
}

/// Whether version `a` is newer than `b`: `0.1.16` over `0.1.9`, and over
/// `0.1.16-rc.1`. What follows a `+` (a build's) doesn't count, and `0.2`
/// is `0.2.0`.
pub fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> (Vec<u64>, bool) {
        let v = v.trim_start_matches("hc ");
        let v = v.split('+').next().unwrap_or(v);
        let (numbers, release) = match v.split_once('-') {
            Some((numbers, _)) => (numbers, false),
            None => (v, true),
        };
        let mut numbers: Vec<u64> = numbers.split('.').map(|p| p.parse().unwrap_or(0)).collect();
        if numbers.len() < 3 {
            numbers.resize(3, 0);
        }
        (numbers, release)
    };
    parts(a) > parts(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Lemur asks only the cores it uses, as asking one starts it: with
    /// this Mac its one host, in a session of its own, that session's, not
    /// herdr-core's default.
    #[test]
    fn only_the_cores_lemur_uses_are_asked() {
        let server = crate::test_support::HcServer::new("update-cores");
        let cfg = server.config();
        let cores = Cores::of(&cfg);
        assert_eq!(
            cores,
            Cores {
                here: Some(server.session.clone()),
                machines: false
            }
        );
        let v = installed(&cores).unwrap();
        assert!(v.here.is_some(), "{v:?}");
        assert!(v.machines.is_empty());

        let far = server.far_config("update-box");
        assert_eq!(
            Cores::of(&far),
            Cores {
                here: None,
                machines: true
            }
        );
    }

    /// With the local host in a session of its own and other hosts too, both
    /// cores are updated, and this Mac is as up to date as the older.
    #[test]
    fn two_cores_here_are_updated_together() {
        let both = Cores {
            here: Some("lemur".into()),
            machines: true,
        };
        assert_eq!(
            sessions_for(&Target::Here, &both),
            [Some("lemur"), None],
            "its own, then the default"
        );
        let only = Cores {
            here: Some("lemur".into()),
            machines: false,
        };
        assert_eq!(sessions_for(&Target::Here, &only), [Some("lemur")]);
        let default = Cores {
            here: None,
            machines: true,
        };
        assert_eq!(sessions_for(&Target::Here, &default), [None]);
        let machine = Target::Machine("agent".into());
        assert_eq!(sessions_for(&machine, &both), [None]);

        let v = |s: &str| Some(s.to_string());
        assert_eq!(older(v("0.1.16"), v("0.1.15")), v("0.1.15"));
        assert_eq!(older(v("0.1.15"), v("0.1.16")), v("0.1.15"));
        assert_eq!(older(None, v("0.1.16")), v("0.1.16"));
        assert_eq!(older(v("0.1.16"), None), v("0.1.16"));
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(newer("0.1.16", "0.1.9"));
        assert!(newer("0.2.0", "0.1.99"));
        assert!(!newer("0.1.16", "0.1.16"));
        assert!(!newer("0.1.15", "0.1.16"));
        assert!(newer("0.1.17", "0.1.17-rc.1"));
        assert!(!newer("0.1.17-rc.1", "0.1.17"));
        assert!(!newer("0.1.1", "0.1.16+abc"));
        assert!(!newer("0.2.0", "0.2"));
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
