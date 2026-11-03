//! How much of your plan is used, as the agent itself says. For Claude, Lemur
//! runs `claude -p /usage` on a host and reads the report it prints. For Codex,
//! it starts `codex app-server` there and asks it over stdio, which is documented
//! for clients. The agent signs in and asks; Lemur never touches its sign-in. It's
//! no model request, and isn't kept as a session.
//!
//! Codex only answers for a ChatGPT sign-in: an API key has no plan. Lemur then
//! adds up the tokens in Codex's session logs on the host, and reads nothing else
//! there.
//!
//! The report's shape isn't promised: whatever Lemur can't read in it is
//! "couldn't fetch", never a crash.

use crate::config::{Agent, Config, Host};
use crate::remote::Remote;
use serde_json::Value;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How often to ask: while Lemur is in front, and when it isn't. Each ask
/// starts the agent, which takes a few seconds of CPU. After a failure,
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
    /// In seconds since 1970.
    pub resets_at: Option<i64>,
}

/// Tokens used, as Codex logs them.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Tokens {
    /// Input, those read from its cache among them.
    pub input: u64,
    pub cached: u64,
    pub output: u64,
}

impl Tokens {
    /// The input that wasn't cached.
    pub fn fresh(self) -> u64 {
        self.input.saturating_sub(self.cached)
    }

    /// What it worked through anew, as Codex totals it: not the cached.
    pub fn total(self) -> u64 {
        self.fresh() + self.output
    }
}

/// What was used over a stretch of time, with no plan to measure it against.
#[derive(Debug, Clone, PartialEq)]
pub struct Spent {
    pub label: &'static str,
    pub tokens: Tokens,
}

/// The plan's limits, in the order the agent lists them, or with no plan what
/// was used, and the host that answered.
#[derive(Debug, Clone, PartialEq)]
pub struct Usage {
    pub limits: Vec<Limit>,
    pub spent: Vec<Spent>,
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
pub fn fetch(cfg: &Config, agent: Agent, order: &[String]) -> Result<Usage, String> {
    let mut first = None;
    for host in order.iter().filter_map(|name| cfg.host(name)) {
        match fetch_from(host, agent) {
            Ok((limits, spent)) => {
                return Ok(Usage {
                    limits,
                    spent,
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

fn fetch_from(host: &Host, agent: Agent) -> Result<(Vec<Limit>, Vec<Spent>), String> {
    let remote = Remote::new(host);
    let run = |script: &str| remote.run(script).map_err(|e| format!("{e:#}"));
    match agent {
        Agent::Claude => Ok((report(&run(&script(&host.claude))?)?, vec![])),
        Agent::Codex => match codex_report(&run(&codex_script(&host.codex))?)? {
            Some(limits) => Ok((limits, vec![])),
            None => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs() as i64);
                Ok((vec![], spent_report(&run(&spent_script(now))?)?))
            }
        },
    }
}

/// Runs `/usage` with `claude`, the host's command for Claude Code, away
/// from any project, and with no settings or MCP servers: it needs none,
/// and each ask would run your hooks and start your MCP servers. `@@exit`
/// says how it ended.
fn script(claude: &str) -> String {
    format!(
        "cd \"${{TMPDIR:-/tmp}}\" 2>/dev/null || cd /\n\
         {claude} -p /usage --no-session-persistence --setting-sources '' --strict-mcp-config \
         --output-format stream-json --verbose </dev/null 2>/dev/null\n\
         echo \"@@exit $?\"\n"
    )
}

/// Asks `codex`, the host's command for Codex, for its plan's limits, away from
/// any project. It answers on stdout, and ends with its stdin: that's held open
/// until the answer comes, or for 30 seconds if it doesn't. `@@exit 127` says
/// there's no such program.
fn codex_script(codex: &str) -> String {
    let version = env!("CARGO_PKG_VERSION");
    format!(
        r#"cd "${{TMPDIR:-/tmp}}" 2>/dev/null || cd /
set -- {codex}
command -v "$1" >/dev/null 2>&1 || {{ echo '@@exit 127'; exit 0; }}
waiting=$(mktemp) || exit 1
{{
  printf '%s\n' '{{"id":1,"method":"initialize","params":{{"clientInfo":{{"name":"lemur","title":"Lemur","version":"{version}"}}}}}}' \
    '{{"method":"initialized"}}' \
    '{{"id":2,"method":"account/rateLimits/read"}}'
  n=0
  while [ -e "$waiting" ] && [ $n -lt 120 ]; do sleep 0.25; n=$((n+1)); done
}} | {codex} app-server 2>/dev/null | {{ sed -n '/"id":2/{{p;q;}}'; rm -f "$waiting"; }}
"#
    )
}

/// The limits in Codex's answer to `codex_script`, None if it has no plan to
/// report on, or why it couldn't say.
fn codex_report(out: &str) -> Result<Option<Vec<Limit>>, String> {
    if out.lines().any(|l| l.trim() == "@@exit 127") {
        return Err("Codex isn't installed there".into());
    }
    let answer: Value = out
        .lines()
        .find_map(|l| serde_json::from_str(l).ok())
        .ok_or("Codex didn't answer")?;
    if let Some(said) = answer["error"]["message"].as_str() {
        if said.contains("chatgpt authentication required") {
            return Ok(None);
        }
        return Err(format!(
            "Codex said: {}",
            said.chars().take(160).collect::<String>()
        ));
    }
    let result = &answer["result"];
    if !result.is_object() {
        return Err("Codex's answer wasn't one Lemur knows".into());
    }
    // Its own limit first, then any others (a model's), by name; the one
    // bucket it has always given, without those.
    let mut buckets: Vec<(&String, &Value)> = result["rateLimitsByLimitId"]
        .as_object()
        .map(|by_id| by_id.iter().collect())
        .unwrap_or_default();
    buckets.sort_by_key(|(id, _)| (id.as_str() != "codex", id.as_str()));
    let mut buckets: Vec<&Value> = buckets.into_iter().map(|(_, bucket)| bucket).collect();
    if buckets.is_empty() {
        buckets.push(&result["rateLimits"]);
    }
    Ok(Some(
        buckets
            .into_iter()
            .flat_map(|bucket| {
                let name = bucket["limitName"].as_str().filter(|n| !n.is_empty());
                [&bucket["primary"], &bucket["secondary"]]
                    .into_iter()
                    .filter_map(move |window| codex_limit(window, name))
            })
            .collect(),
    ))
}

/// Adds up `grep -H`'s lines of Codex's `token_count` events, from `last_token_usage`
/// (what one request used), for those since `day` and since `week`. An event
/// repeating a session's running total used nothing more.
const ADD_UP: &str = r#"
function num(key, s) {
    if (!match(s, "\"" key "\":[0-9]+")) return 0
    return substr(s, RSTART + length(key) + 3, RLENGTH - length(key) - 3) + 0
}
function part(name) {
    if (!match($0, "\"" name "\":\\{[^}]*\\}")) return ""
    return substr($0, RSTART, RLENGTH)
}
{
    file = substr($0, 1, index($0, ":{") - 1)
    if (!match($0, /"timestamp":"[^"]*"/)) next
    at = substr($0, RSTART + 13, RLENGTH - 14)
    total = num("total_tokens", part("total_token_usage"))
    if (total == seen[file]) next
    seen[file] = total
    last = part("last_token_usage")
    i = num("input_tokens", last)
    c = num("cached_input_tokens", last)
    o = num("output_tokens", last)
    if (at >= week) { wi += i; wc += c; wo += o }
    if (at >= day) { di += i; dc += c; dp += o }
}
END { printf "@@spent %.0f %.0f %.0f %.0f %.0f %.0f\n", di, dc, dp, wi, wc, wo }
"#;

/// Totals the tokens in Codex's session logs on the host over the last day and
/// the last 7, `now` being seconds since 1970. Only numbers come back.
pub(crate) fn spent_script(now: i64) -> String {
    let day = rfc3339(now - 86400);
    let week = rfc3339(now - 7 * 86400);
    format!(
        r#"dir="${{CODEX_HOME:-$HOME/.codex}}/sessions"
[ -d "$dir" ] || {{ echo '@@spent 0 0 0 0 0 0'; exit 0; }}
find "$dir" -name 'rollout-*.jsonl' -mtime -8 -exec grep -H '"type":"token_count"' {{}} + 2>/dev/null |
  awk -v day={day} -v week={week} '{ADD_UP}'
"#
    )
}

/// What `spent_script` totalled, or that it couldn't.
pub(crate) fn spent_report(out: &str) -> Result<Vec<Spent>, String> {
    let numbers: Vec<u64> = out
        .lines()
        .find_map(|l| l.strip_prefix("@@spent "))
        .map(|l| {
            l.split_whitespace()
                .filter_map(|n| n.parse().ok())
                .collect()
        })
        .unwrap_or_default();
    let [di, dc, dp, wi, wc, wo] = numbers[..] else {
        return Err("Codex's session logs couldn't be read".into());
    };
    let tokens = |input, cached, output| Tokens {
        input,
        cached,
        output,
    };
    Ok(vec![
        Spent {
            label: "24 hours",
            tokens: tokens(di, dc, dp),
        },
        Spent {
            label: "7 days",
            tokens: tokens(wi, wc, wo),
        },
    ])
}

/// One of Codex's windows, if it has a share used, by how long it is.
fn codex_limit(window: &Value, name: Option<&str>) -> Option<Limit> {
    let percent = window["usedPercent"]
        .as_f64()
        .filter(|p| p.is_finite() && *p >= 0.0)?;
    let base = match window["windowDurationMins"].as_u64() {
        Some(10080) => "Week".to_string(),
        Some(1440) => "Day".to_string(),
        Some(60) => "Hour".to_string(),
        Some(mins) if mins % 1440 == 0 && mins > 0 => format!("{} days", mins / 1440),
        Some(mins) if mins % 60 == 0 && mins > 0 => format!("{} hours", mins / 60),
        Some(mins) if mins > 0 => format!("{mins} minutes"),
        _ => "Limit".to_string(),
    };
    Some(Limit {
        label: match name {
            Some(name) => format!("{base} · {name}"),
            None => base,
        },
        percent,
        resets_at: window["resetsAt"].as_i64(),
    })
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
        resets_at: text("resets_at").and_then(epoch),
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

/// An RFC 3339 time for seconds since 1970, `epoch`'s inverse; after Howard
/// Hinnant's civil_from_days.
pub(crate) fn rfc3339(secs: i64) -> String {
    let (days, rest) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
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
                    resets_at: epoch("2026-11-07T10:00:00.390555+00:00"),
                },
                Limit {
                    label: "Week · Fable".into(),
                    percent: 0.0,
                    resets_at: epoch("2026-11-07T10:00:00+00:00"),
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
            "-p\n/usage\n--no-session-persistence\n--setting-sources\n\n--strict-mcp-config\n\
             --output-format\nstream-json\n--verbose\n",
            "no settings (hooks) or MCP servers"
        );

        let missing = run("lemur-no-such-claude");
        assert!(
            report(&missing).unwrap_err().contains("isn't installed"),
            "{missing}"
        );
    }

    /// Codex's answer, as its protocol's schema and docs give it: two windows
    /// of its own limit, and another's.
    const CODEX_ANSWER: &str = r#"{"id":2,"result":{"ordinaryUsageAllowed":true,"rateLimits":{"limitId":"codex","limitName":null,"primary":{"usedPercent":25,"windowDurationMins":300,"resetsAt":1793660000},"secondary":{"usedPercent":61.5,"windowDurationMins":10080,"resetsAt":1794000000},"credits":null,"planType":"plus"},"rateLimitsByLimitId":{"codex_other":{"limitId":"codex_other","limitName":"Spark","primary":{"usedPercent":3,"windowDurationMins":10080,"resetsAt":null},"secondary":null},"codex":{"limitId":"codex","limitName":null,"primary":{"usedPercent":25,"windowDurationMins":300,"resetsAt":1793660000},"secondary":{"usedPercent":61.5,"windowDurationMins":10080,"resetsAt":1794000000}}}}}
"#;

    #[test]
    fn codexs_limits_are_read_as_its_protocol_gives_them() {
        let read = codex_report(CODEX_ANSWER).unwrap().unwrap();
        let shown: Vec<(&str, f64, Option<i64>)> = read
            .iter()
            .map(|l| (l.label.as_str(), l.percent, l.resets_at))
            .collect();
        assert_eq!(
            shown,
            [
                ("5 hours", 25.0, Some(1793660000)),
                ("Week", 61.5, Some(1794000000)),
                ("Week · Spark", 3.0, None),
            ]
        );
        // The older, one-bucket answer alone, and windows that say little.
        let one = r#"{"id":2,"result":{"rateLimits":{"primary":{"usedPercent":7,"windowDurationMins":15,"resetsAt":1},"secondary":null},"rateLimitsByLimitId":null}}"#;
        let read = codex_report(one).unwrap().unwrap();
        assert_eq!(read[0].label, "15 minutes");
        assert_eq!(read.len(), 1);
        let odd = r#"{"id":2,"result":{"rateLimits":{"primary":{"usedPercent":"7"},"secondary":{"usedPercent":1}}}}"#;
        let read = codex_report(odd).unwrap().unwrap();
        assert_eq!(read[0].label, "Limit");
        assert_eq!(read.len(), 1);
    }

    #[test]
    fn why_codex_has_no_limits_is_said() {
        let said = |out: &str| codex_report(out).unwrap_err();
        assert_eq!(said("@@exit 127\n"), "Codex isn't installed there");
        assert_eq!(said(""), "Codex didn't answer");
        assert_eq!(said("not json\n"), "Codex didn't answer");
        assert_eq!(
            said("{\"id\":2}\n"),
            "Codex's answer wasn't one Lemur knows"
        );
        assert_eq!(
            codex_report(
                r#"{"error":{"code":-32600,"message":"chatgpt authentication required to read rate limits"},"id":2}"#
            ),
            Ok(None),
            "no plan, which isn't a failure"
        );
        assert_eq!(
            said(r#"{"error":{"code":-1,"message":"down"},"id":2}"#),
            "Codex said: down"
        );
    }

    /// The script against a stand-in for `codex app-server`: it reads the
    /// handshake, answers only once asked, and stays until its stdin ends, as
    /// the real one does. The script doesn't wait out the time it allows.
    #[test]
    fn codex_is_asked_and_let_go_of_once_it_answers() {
        let dir = crate::test_support::TempDir::new("codex-script");
        let fake = dir.join("fake-codex");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\n[ \"$1\" = app-server ] || exit 9\nwhile read -r line; do\n  case $line in\n    *'\"id\":1'*) echo '{{\"id\":1,\"result\":{{}}}}' ;;\n    *rateLimits/read*) cat <<'EOF'\n{CODEX_ANSWER}EOF\n ;;\n  esac\ndone\n"
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let run = |codex: &str| {
            std::process::Command::new("sh")
                .args(["-c", &codex_script(codex)])
                .output()
                .unwrap()
        };
        let at = std::time::Instant::now();
        let out = run(&fake.to_string_lossy());
        assert!(at.elapsed() < Duration::from_secs(10), "{:?}", at.elapsed());
        let read = codex_report(&String::from_utf8_lossy(&out.stdout))
            .unwrap()
            .unwrap();
        assert_eq!(read.len(), 3);
        let out = run("lemur-has-no-codex");
        assert_eq!(
            codex_report(&String::from_utf8_lossy(&out.stdout)).unwrap_err(),
            "Codex isn't installed there"
        );
    }

    #[test]
    fn times_are_written_as_they_are_read() {
        for time in [
            "1970-01-01T00:00:00Z",
            "2000-02-29T23:59:59Z",
            "2026-11-02T22:06:20Z",
            "2028-12-31T00:00:01Z",
        ] {
            assert_eq!(rfc3339(epoch(time).unwrap()), time);
        }
    }

    /// One `token_count` event, as Codex's logs have them.
    fn event(at: &str, total: u64, input: u64, cached: u64, output: u64) -> String {
        format!(
            r#"{{"timestamp":"{at}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":1,"cached_input_tokens":1,"output_tokens":1,"reasoning_output_tokens":0,"total_tokens":{total}}},"last_token_usage":{{"input_tokens":{input},"cached_input_tokens":{cached},"cache_write_input_tokens":5,"output_tokens":{output},"reasoning_output_tokens":2,"total_tokens":{}}},"model_context_window":258400}},"rate_limits":null}}}}"#,
            input + output
        )
    }

    /// The script against logs like Codex's: only what's in each window counts,
    /// once, from every session, and what was said isn't taken for a count.
    #[test]
    fn what_an_api_key_used_is_added_up_from_codexs_logs() {
        let home = crate::test_support::TempDir::new("codex-logs");
        let run = || {
            let now = epoch("2026-11-02T12:00:00Z").unwrap();
            let out = std::process::Command::new("sh")
                .args(["-c", &spent_script(now)])
                .env("CODEX_HOME", &*home)
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        // Codex never ran here.
        let none = spent_report(&run()).unwrap();
        assert_eq!(none[0].tokens, Tokens::default());
        assert_eq!(none[1].tokens, Tokens::default());

        let day = home.join("sessions/2026/11/02");
        std::fs::create_dir_all(&day).unwrap();
        let said = r#"{"timestamp":"2026-11-02T11:00:00.000Z","type":"response_item","payload":{"type":"message","text":"\"type\":\"token_count\" \"last_token_usage\":{\"input_tokens\":999}"}}"#;
        let one = [
            event("2026-10-20T10:00:00.000Z", 10, 100000, 0, 100000),
            event("2026-10-30T10:00:00.000Z", 20, 100, 40, 10),
            event("2026-11-02T10:00:00.000Z", 30, 1000, 900, 50),
            // Only the rate limits changed.
            event("2026-11-02T10:00:01.000Z", 30, 1000, 900, 50),
            said.to_string(),
        ]
        .join("\n");
        std::fs::write(day.join("rollout-one.jsonl"), one + "\n").unwrap();
        std::fs::write(
            day.join("rollout-two.jsonl"),
            event("2026-11-02T11:30:00.000Z", 30, 7, 0, 3) + "\n",
        )
        .unwrap();
        std::fs::write(
            day.join("other.jsonl"),
            event("2026-11-02T11:30:00.000Z", 1, 5, 5, 5),
        )
        .unwrap();

        let read = spent_report(&run()).unwrap();
        assert_eq!(read[0].label, "24 hours");
        assert_eq!(
            read[0].tokens,
            Tokens {
                input: 1007,
                cached: 900,
                output: 53,
            }
        );
        assert_eq!(read[0].tokens.fresh(), 107);
        assert_eq!(read[0].tokens.total(), 160);
        assert_eq!(read[1].label, "7 days");
        assert_eq!(
            read[1].tokens,
            Tokens {
                input: 1107,
                cached: 940,
                output: 63,
            }
        );

        assert!(spent_report("").is_err());
        assert!(spent_report("@@spent 1 2\n").is_err());
        assert!(spent_report("@@spent a b c d e f\n").is_err());
    }
}
