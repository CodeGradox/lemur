//! How much of your Claude plan is used, as Claude Code's `/usage` says:
//! Lemur runs `claude -p /usage` on a host and reads the report it prints.
//! Claude Code signs in and asks; Lemur never touches its sign-in. It's no
//! model request, and isn't kept as a session.
//!
//! The report's shape isn't promised: whatever Lemur can't read in it is
//! "couldn't fetch", never a crash.

use crate::config::{Config, Host};
use crate::remote::Remote;
use serde_json::Value;
use std::time::Duration;

/// How often to ask: while Lemur is in front, and when it isn't. Each ask
/// starts Claude Code, which takes a few seconds of CPU. After a failure,
/// twice as long each time, up to `SLOWEST`.
pub const EVERY: Duration = Duration::from_secs(10 * 60);
pub const AWAY: Duration = Duration::from_secs(30 * 60);
pub const SLOWEST: Duration = Duration::from_secs(60 * 60);

/// One of the plan's limits: what it's for, how much is used, and when it
/// starts over.
#[derive(Debug, Clone, PartialEq)]
pub struct Limit {
    /// "Session", "Week", or "Week · Fable" for one model's.
    pub label: String,
    pub percent: f64,
    /// As Claude says it, e.g. `2026-11-07T10:00:00+00:00`.
    pub resets_at: Option<String>,
}

/// The plan's limits, in the order Claude lists them, and the host that
/// answered.
#[derive(Debug, Clone, PartialEq)]
pub struct Usage {
    pub limits: Vec<Limit>,
    pub host: String,
}

/// How long until the next ask, given whether Lemur is in front and how
/// many asks in a row failed.
pub fn wait(looking: bool, fails: u32) -> Duration {
    let base = if looking { EVERY } else { AWAY };
    if fails == 0 {
        return base;
    }
    (EVERY * 2u32.saturating_pow(fails.min(8)))
        .min(SLOWEST)
        .max(base)
}

/// Asks each host in `order` until one answers; the first one's failure
/// if none does.
pub fn fetch(cfg: &Config, order: &[String]) -> Result<Usage, String> {
    let mut first = None;
    for host in order.iter().filter_map(|name| cfg.host(name)) {
        match fetch_from(host) {
            Ok(limits) => {
                return Ok(Usage {
                    limits,
                    host: host.name.clone(),
                });
            }
            Err(e) => {
                first.get_or_insert(format!("{e} (on {})", host.name));
            }
        }
    }
    Err(first.unwrap_or_else(|| "there's no host to ask on".to_string()))
}

fn fetch_from(host: &Host) -> Result<Vec<Limit>, String> {
    let out = Remote::new(host)
        .run(&script(&host.claude))
        .map_err(|e| format!("{e:#}"))?;
    report(&out)
}

/// Runs `/usage` with `claude`, the host's command for Claude Code, away
/// from any project, so no project's settings or hooks load. `@@exit` says
/// how it ended.
fn script(claude: &str) -> String {
    format!(
        "cd \"${{TMPDIR:-/tmp}}\" 2>/dev/null || cd /\n\
         {claude} -p /usage --no-session-persistence --output-format stream-json --verbose \
         </dev/null 2>/dev/null\n\
         echo \"@@exit $?\"\n"
    )
}

/// The limits in what the script printed, or why there are none.
fn report(out: &str) -> Result<Vec<Limit>, String> {
    let code = out
        .lines()
        .rev()
        .find_map(|l| l.strip_prefix("@@exit "))
        .map(str::trim);
    let lines: Vec<Value> = out
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    if let Some(report) = lines.iter().find_map(|v| v.get("usage_report")) {
        let limits = report
            .get("rate_limits")
            .and_then(|r| r.get("limits"))
            .and_then(Value::as_array)
            .ok_or("Claude Code's report had no plan limits (is it signed in to a Claude plan?)")?;
        return Ok(limits.iter().filter_map(limit).collect());
    }
    if code == Some("127") {
        return Err("Claude Code isn't installed there".into());
    }
    // What Claude Code said instead, when it says why.
    let said = lines
        .iter()
        .find(|v| v["type"] == "result" && v["is_error"] == true)
        .and_then(|v| v["result"].as_str())
        .map(|s| s.chars().take(160).collect::<String>());
    Err(match said {
        Some(said) => format!("Claude Code said: {said}"),
        None if lines.is_empty() => "Claude Code didn't answer".into(),
        None => "Claude Code's report wasn't one Lemur knows".into(),
    })
}

/// One limit, if it has a share used; labelled by its kind and what it's
/// scoped to, as Claude Code classifies rows.
fn limit(v: &Value) -> Option<Limit> {
    let percent = v
        .get("percent")?
        .as_f64()
        .filter(|p| p.is_finite() && *p >= 0.0)?;
    let text = |key: &str| v.get(key).and_then(Value::as_str);
    let base = match text("kind").or(text("group")) {
        Some("session") => "Session".to_string(),
        Some(k) if k.starts_with("weekly") => "Week".to_string(),
        Some(k) if !k.is_empty() => {
            let k = k.replace('_', " ");
            let mut chars = k.chars();
            chars
                .next()
                .map_or(k.clone(), |c| c.to_uppercase().chain(chars).collect())
        }
        _ => "Limit".to_string(),
    };
    let scope = v.get("scope");
    let scoped = ["model", "surface"].into_iter().find_map(|key| {
        scope?
            .get(key)?
            .get("display_name")?
            .as_str()
            .filter(|s| !s.is_empty())
    });
    Some(Limit {
        label: match scoped {
            Some(name) => format!("{base} · {name}"),
            None => base,
        },
        percent,
        resets_at: text("resets_at").map(String::from),
    })
}

/// Seconds since 1970 for an RFC 3339 time, as `resets_at` is.
pub fn epoch(time: &str) -> Option<i64> {
    let b = time.as_bytes();
    let num = |from: usize, len: usize| -> Option<i64> {
        let s = time.get(from..from + len)?;
        s.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| s.parse().ok())?
    };
    let (y, mo, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let (h, mi, s) = (num(11, 2)?, num(14, 2)?, num(17, 2)?);
    if b.get(4) != Some(&b'-') || b.get(7) != Some(&b'-') || b.get(13) != Some(&b':') {
        return None;
    }
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    // Past the seconds: a fraction, then Z or ±HH:MM.
    let mut rest = time.get(19..)?;
    if let Some(r) = rest.strip_prefix('.') {
        rest = r.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    let offset = match rest.as_bytes().first() {
        None | Some(b'Z' | b'z') => 0,
        Some(sign @ (b'+' | b'-')) => {
            let r = &rest[1..];
            let oh: i64 = r.get(0..2)?.parse().ok()?;
            let om: i64 = r.get(3..5)?.parse().ok()?;
            let o = oh * 3600 + om * 60;
            if *sign == b'-' { -o } else { o }
        }
        _ => return None,
    };
    // Days since 1970 for a date, after Howard Hinnant's days_from_civil.
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + h * 3600 + mi * 60 + s - offset)
}

/// What `claude -p /usage` prints on a Claude plan, cut down: its report
/// rides on the assistant message beside the text.
#[cfg(test)]
pub const PRINTED: &str = r#"{"type":"system","subtype":"init","cwd":"/tmp"}
{"type":"assistant","message":{"content":[{"type":"text","text":"Current week (all models): 8% used"}]},"usage_report":{"session":{"total_cost_usd":0},"rate_limits":{"limits":[{"kind":"weekly_all","group":"weekly","percent":8,"resets_at":"2026-11-07T10:00:00.390555+00:00","scope":null,"severity":"normal","is_active":true},{"kind":"weekly_scoped","group":"weekly","percent":0,"resets_at":"2026-11-07T10:00:00+00:00","scope":{"model":{"display_name":"Fable"},"surface":null},"severity":"normal","is_active":false}],"extra_usage":{"is_enabled":false}}}}
{"type":"result","subtype":"success","is_error":false,"num_turns":0,"total_cost_usd":0}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_are_read_as_claude_lists_them() {
        let limits = report(PRINTED).unwrap();
        assert_eq!(
            limits,
            [
                Limit {
                    label: "Week".into(),
                    percent: 8.0,
                    resets_at: Some("2026-11-07T10:00:00.390555+00:00".into()),
                },
                Limit {
                    label: "Week · Fable".into(),
                    percent: 0.0,
                    resets_at: Some("2026-11-07T10:00:00+00:00".into()),
                },
            ]
        );
        // A session window, and kinds Lemur doesn't know, are read too.
        let row = |json: &str| limit(&serde_json::from_str(json).unwrap());
        let label = |json: &str| row(json).unwrap().label;
        assert_eq!(label(r#"{"kind":"session","percent":42}"#), "Session");
        assert_eq!(
            label(r#"{"kind":"monthly_all","percent":1}"#),
            "Monthly all"
        );
        assert_eq!(label(r#"{"group":"weekly","percent":1}"#), "Week");
        assert_eq!(label(r#"{"percent":1}"#), "Limit");
        assert_eq!(
            label(
                r#"{"kind":"weekly_scoped","percent":1,"scope":{"surface":{"display_name":"Claude Code"}}}"#
            ),
            "Week · Claude Code"
        );
        // A row without a share used is left out, not guessed at.
        for json in [
            r#"{"kind":"session"}"#,
            r#"{"kind":"session","percent":"42"}"#,
            r#"{"kind":"session","percent":-1}"#,
            r#"{"kind":"session","percent":null}"#,
        ] {
            assert_eq!(row(json), None, "{json}");
        }
    }

    #[test]
    fn whatever_else_is_printed_says_why_it_failed() {
        let failed = |out: &str| report(out).unwrap_err();
        assert!(failed("@@exit 127\n").contains("isn't installed"));
        assert!(failed("").contains("didn't answer"));
        assert!(failed("garbage\n@@exit 1\n").contains("didn't answer"));
        let not_in =
            r#"{"type":"result","is_error":true,"result":"Not logged in · Please run /login"}"#;
        assert!(failed(not_in).contains("Not logged in"));
        let text_only = r#"{"type":"result","is_error":false,"result":"text only"}"#;
        assert!(failed(text_only).contains("wasn't one"));
        assert!(failed(r#"{"usage_report":{"rate_limits":null}}"#).contains("no plan limits"));
        let other_shape = r#"{"usage_report":{"rate_limits":{"five_hour":{}}}}"#;
        assert!(failed(other_shape).contains("no plan limits"));
        // Limits in a shape Lemur doesn't know: none read, and no crash.
        let odd = r#"{"usage_report":{"rate_limits":{"limits":[1,"x",{}]}}}"#;
        assert_eq!(report(odd), Ok(vec![]));
    }

    #[test]
    fn waits_longer_after_failures_and_out_of_sight() {
        let min = |m: u64| Duration::from_secs(m * 60);
        assert_eq!(wait(true, 0), min(10));
        assert_eq!(wait(false, 0), min(30));
        assert_eq!(wait(true, 1), min(20));
        assert_eq!(wait(true, 2), min(40));
        assert_eq!(wait(true, 3), min(60));
        assert_eq!(wait(true, 40), min(60));
        assert_eq!(wait(false, 1), min(30));
    }

    #[test]
    fn reset_times_are_read() {
        assert_eq!(epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch("2025-11-01T05:00:00+00:00"), Some(1_761_973_200));
        assert_eq!(
            epoch("2025-11-01T05:00:00.123456+00:00"),
            Some(1_761_973_200)
        );
        assert_eq!(epoch("2025-11-01T06:30:00+01:30"), Some(1_761_973_200));
        assert_eq!(epoch("2024-02-29T12:00:00-05:00"), Some(1_709_226_000));
        for bad in [
            "",
            "soon",
            "2025-13-01T00:00:00Z",
            "2025-11-01 05:00",
            "2025-11-01T05:00:00+0",
        ] {
            assert_eq!(epoch(bad), None, "{bad}");
        }
    }

    /// The script runs the host's Claude command with `/usage`, in print
    /// mode, kept as no session, and its report comes back.
    #[test]
    fn the_script_runs_usage_with_the_hosts_claude() {
        let dir = crate::test_support::TempDir::new("usage-script");
        let fixture = dir.join("printed.jsonl");
        std::fs::write(&fixture, PRINTED).unwrap();
        let args = dir.join("args");
        let claude = format!(
            "sh -c 'printf \"%s\\n\" \"$@\" > {}; cat {}' claude",
            args.display(),
            fixture.display()
        );
        let run = |claude: &str| {
            let out = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(script(claude))
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        let out = run(&claude);
        assert_eq!(report(&out).unwrap().len(), 2, "{out}");
        assert!(out.ends_with("@@exit 0\n"), "{out}");
        assert_eq!(
            std::fs::read_to_string(args).unwrap(),
            "-p\n/usage\n--no-session-persistence\n--output-format\nstream-json\n--verbose\n"
        );

        let missing = run("lemur-no-such-claude");
        assert!(
            report(&missing).unwrap_err().contains("isn't installed"),
            "{missing}"
        );
    }
}
