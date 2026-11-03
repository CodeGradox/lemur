//! How much of each agent's plan is used, above the hosts' load at the foot
//! of the sidebar: each limit it lists, asked every few minutes (see
//! `crate::usage`), or that it couldn't be asked; with no plan to measure
//! against, the tokens used. Off until turned on in Settings.

use super::chrome::{bar_sized, chevron, minimizable_header};
use super::settings::Prefs;
use super::sidebar::{claude_icon, codex_icon};
use super::{Lemur, sessions};
use crate::config::Agent;
use crate::usage::{self, Tokens, Usage};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How often the meter checks whether it's time to ask.
pub(super) const TICK: Duration = Duration::from_secs(30);

/// Whether Lemur has seen Claude Code on a host: running there, or kept
/// to resume. Settings offers the meter once it has.
pub(super) struct ClaudeFound(pub(super) bool);

impl Global for ClaudeFound {}

/// The same of Codex: running on a host.
pub(super) struct CodexFound(pub(super) bool);

impl Global for CodexFound {}

#[derive(Default)]
pub(super) struct Meter {
    /// Turned on in Settings.
    pub(super) on: bool,
    /// The last answer: usage, or why there was none. None until the first.
    pub(super) shown: Option<Result<Usage, String>>,
    /// Asks in a row that failed.
    fails: u32,
    asked: Option<Instant>,
    /// When the answer shown came, in seconds since 1970.
    answered: Option<i64>,
    asking: bool,
}

impl Meter {
    pub(super) fn new(agent: Agent, cx: &App) -> Self {
        Meter {
            on: setting(agent, cx),
            ..Default::default()
        }
    }

    /// Whether it's time to ask again (see `usage::wait`), or a limit shown
    /// has reset since the answer (`epoch`, seconds since 1970, is now):
    /// its use shown is over.
    fn due(&self, looking: bool, now: Instant, epoch: i64) -> bool {
        let reset = match (&self.shown, self.answered) {
            (Some(Ok(u)), Some(answered)) => u
                .limits
                .iter()
                .filter_map(|l| l.resets_at)
                .any(|at| answered < at && at <= epoch),
            _ => false,
        };
        !self.asking
            && (reset
                || self
                    .asked
                    .is_none_or(|at| now.duration_since(at) >= usage::wait(looking, self.fails)))
    }
}

/// Whether Settings has `agent`'s meter on.
fn setting(agent: Agent, cx: &App) -> bool {
    match agent {
        Agent::Claude => Prefs::get(cx).claude_usage,
        Agent::Codex => Prefs::get(cx).codex_usage,
    }
}

/// Seconds since 1970.
fn epoch_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

const AGENTS: [Agent; 2] = [Agent::Claude, Agent::Codex];

impl Lemur {
    fn meter_of(&mut self, agent: Agent) -> &mut Meter {
        match agent {
            Agent::Claude => &mut self.meter,
            Agent::Codex => &mut self.codex_meter,
        }
    }

    /// Asks, once the hosts are known, when it's time.
    pub(super) fn tick_meter(&mut self, looking: bool, cx: &mut Context<Self>) {
        for agent in AGENTS {
            let loaded = self.loaded;
            let meter = self.meter_of(agent);
            if meter.on && loaded && meter.due(looking, Instant::now(), epoch_now()) {
                self.ask_usage(agent, cx);
            }
        }
    }

    /// Follows Settings: turned on, it asks at once; off, it's gone.
    pub(super) fn follow_meter_setting(&mut self, cx: &mut Context<Self>) {
        for agent in AGENTS {
            let on = setting(agent, cx);
            let meter = self.meter_of(agent);
            if on == meter.on {
                continue;
            }
            *meter = Meter {
                on,
                ..Default::default()
            };
            if on {
                self.ask_usage(agent, cx);
            }
            cx.notify();
        }
    }

    /// Tells Settings whether the agents are around, when that changes.
    pub(super) fn note_agents(&self, cx: &mut App) {
        let found =
            !self.cfg.threads.is_empty() || sessions(&self.snap).any(|(_, s)| s.runs_claude());
        if cx.try_global::<ClaudeFound>().map(|f| f.0) != Some(found) {
            cx.set_global(ClaudeFound(found));
        }
        let found = sessions(&self.snap).any(|(_, s)| s.agent() == Some(Agent::Codex));
        if cx.try_global::<CodexFound>().map(|f| f.0) != Some(found) {
            cx.set_global(CodexFound(found));
        }
    }

    /// Asks `agent` how much of the plan is used, in the background.
    pub(super) fn ask_usage(&mut self, agent: Agent, cx: &mut Context<Self>) {
        let order = self.usage_order(agent);
        let meter = self.meter_of(agent);
        if meter.asking {
            return;
        }
        meter.asking = true;
        let cfg = self.cfg.clone();
        self.in_background(
            cx,
            move || usage::fetch(&cfg, agent, &order),
            move |this, answer, _, cx| {
                let meter = this.meter_of(agent);
                meter.asking = false;
                if !meter.on {
                    return;
                }
                meter.asked = Some(Instant::now());
                meter.answered = Some(epoch_now());
                meter.fails = if answer.is_ok() { 0 } else { meter.fails + 1 };
                meter.shown = Some(answer);
                cx.notify();
            },
        )
        .detach();
    }

    /// The hosts to ask on, the one where `agent` ran last first.
    fn usage_order(&self, agent: Agent) -> Vec<String> {
        let hosts: Vec<(String, Option<u64>)> = self
            .snap
            .hosts
            .iter()
            .filter(|h| h.error.is_none())
            .map(|h| {
                let last = sessions(&self.snap)
                    .filter(|(host, s)| *host == h.name && s.agent() == Some(agent))
                    .map(|(_, s)| s.idle_secs)
                    .min();
                (h.name.clone(), last)
            })
            .collect();
        by_last_claude(hosts)
    }

    /// The agent's name and when its next limit starts over, then a bar for
    /// each limit, or that it couldn't be asked; what each says on hover. A
    /// click on the bars asks again; on its name, it minimizes the meter to
    /// that line, the most used limit beside it.
    pub(super) fn usage_meter(&self, agent: Agent, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (meter, minimized, name, icon, ids) = match agent {
            Agent::Claude => (
                &self.meter,
                self.cfg.minimized_usage,
                "Claude",
                claude_icon(),
                ("usage", "usage-header", "usage-bars", "usage bars"),
            ),
            Agent::Codex => (
                &self.codex_meter,
                self.cfg.minimized_codex_usage,
                "Codex",
                codex_icon(),
                (
                    "codex usage",
                    "codex-usage-header",
                    "codex-usage-bars",
                    "codex usage bars",
                ),
            ),
        };
        let (id, header_id, bars_id, bars_selector) = ids;
        if !meter.on {
            return None;
        }
        let now = epoch_now();
        let shown = describe(agent, meter.shown.as_ref(), now);
        let open = !minimized;
        let failed = matches!(meter.shown, Some(Err(_)));
        let tip = match (&meter.shown, open) {
            (Some(Ok(_)), true) if shown.rows.is_empty() => {
                format!("{}\nClick to ask now.", shown.tip)
            }
            (Some(Ok(_)), true) => format!("{}\nClick a bar to ask now.", shown.tip),
            (Some(Err(_)), true) => format!("{} Click to try again.", shown.tip),
            _ => shown.tip,
        };
        let tip = SharedString::from(tip);
        let rows: Vec<Div> = shown
            .rows
            .into_iter()
            .map(|(label, percent)| {
                let text = format!("{percent:.0}%");
                bar_sized(
                    label.into(),
                    (px(76.0), px(36.0)),
                    Some(percent as f32),
                    text,
                    cx,
                )
            })
            .collect();
        let theme = cx.theme();
        let (border, muted) = (theme.sidebar_border, theme.muted_foreground);
        let totals: Vec<Div> = shown
            .totals
            .into_iter()
            .map(|(label, text)| {
                h_flex()
                    .gap_2()
                    .text_xs()
                    .child(div().flex_shrink_0().text_color(muted).child(label))
                    // Narrow, its end goes first: the hover says it all.
                    .child(div().flex_1().min_w_0().truncate().text_right().child(text))
            })
            .collect();
        let right = if minimized { shown.brief } else { shown.right };
        let header = minimizable_header(header_id.into(), minimized, cx)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| this.toggle_usage_minimized(agent, cx)),
            )
            .child(icon.xsmall())
            .child(div().flex_1().min_w_0().truncate().child(name))
            .child(div().text_xs().text_color(muted).child(right))
            .child(chevron(minimized));
        let body = (open && (failed || !rows.is_empty() || !totals.is_empty())).then(|| {
            v_flex()
                .id(bars_id)
                .debug_selector(move || bars_selector.into())
                .gap_0p5()
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.ask_usage(agent, cx)))
                .when(failed, |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(format!("Couldn't fetch usage from {name}.")),
                    )
                })
                .children(rows)
                .children(totals)
        });
        Some(
            v_flex()
                .w_full()
                .px_2()
                .py_1()
                .border_t_1()
                .border_color(border)
                .child(
                    v_flex()
                        .id(id)
                        .debug_selector(move || id.into())
                        .gap_0p5()
                        .py_1()
                        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                        .child(header)
                        .children(body),
                )
                .into_any_element(),
        )
    }

    /// Minimizes the meter to its one line, or opens it again.
    pub(super) fn toggle_usage_minimized(&mut self, agent: Agent, cx: &mut Context<Self>) {
        let minimized = match agent {
            Agent::Claude => &mut self.cfg.minimized_usage,
            Agent::Codex => &mut self.cfg.minimized_codex_usage,
        };
        *minimized = !*minimized;
        self.save_minimized(cx);
    }
}

/// Hosts by how long ago the agent last did something on them, if it runs
/// there at all, most recent first; the rest in their order after.
fn by_last_claude(mut hosts: Vec<(String, Option<u64>)>) -> Vec<String> {
    hosts.sort_by_key(|(_, idle)| (idle.is_none(), idle.unwrap_or(0)));
    hosts.into_iter().map(|(name, _)| name).collect()
}

/// What the meter shows: beside the agent's name, open and minimized, a bar
/// for each limit, a total for what was used with no limits, and on hover.
struct Shown {
    right: String,
    brief: String,
    rows: Vec<(String, f64)>,
    totals: Vec<(String, String)>,
    tip: String,
}

/// The meter for the last answer, if any; `now` in seconds since 1970.
fn describe(agent: Agent, shown: Option<&Result<Usage, String>>, now: i64) -> Shown {
    let (name, source) = match agent {
        Agent::Claude => ("Claude", "Claude Code's /usage"),
        Agent::Codex => ("Codex", "Codex's app server"),
    };
    let u = match shown {
        None => {
            return Shown {
                right: "asking…".into(),
                brief: "asking…".into(),
                rows: vec![],
                totals: vec![],
                tip: format!("Asking {} how much of your plan is used…", agent.name()),
            };
        }
        Some(Err(why)) => {
            return Shown {
                right: "unavailable".into(),
                brief: "unavailable".into(),
                rows: vec![],
                totals: vec![],
                tip: format!("Couldn't fetch usage from {name}: {why}."),
            };
        }
        Some(Ok(u)) => u,
    };
    if let Some(first) = u.spent.first() {
        let line = |label: &str, t: Tokens| {
            format!(
                "{label}: {} tokens, {} input (+ {} cached) and {} output",
                short(t.total()),
                short(t.fresh()),
                short(t.cached),
                short(t.output)
            )
        };
        let mut lines: Vec<String> = u.spent.iter().map(|s| line(s.label, s.tokens)).collect();
        lines.push(format!(
            "From {name}'s session logs on {}, every 10 minutes. An API key has no plan limits to show.",
            u.host
        ));
        return Shown {
            right: "API key".into(),
            brief: format!("{} {}", first.label, short(first.tokens.total())),
            rows: vec![],
            totals: u
                .spent
                .iter()
                .map(|s| (s.label.to_string(), used(s.tokens)))
                .collect(),
            tip: lines.join("\n"),
        };
    }
    let mut lines = vec![];
    let mut soonest: Option<i64> = None;
    for l in &u.limits {
        let at = l.resets_at;
        if let Some(at) = at.filter(|at| *at > now) {
            soonest = Some(soonest.map_or(at, |s| s.min(at)));
        }
        let resets = at.map_or(String::new(), |at| {
            format!(", resets in {}", until(at - now))
        });
        lines.push(format!("{}: {:.0}% used{resets}", l.label, l.percent));
    }
    if u.limits.is_empty() {
        lines.push(format!("{} listed no limits for your plan.", agent.name()));
    }
    lines.push(format!("From {source} on {}, every 10 minutes.", u.host));
    // The most used limit, for the one line.
    let most = u
        .limits
        .iter()
        .max_by(|a, b| a.percent.total_cmp(&b.percent));
    Shown {
        brief: most.map_or("no limits".into(), |l| {
            format!("{} {:.0}%", l.label, l.percent)
        }),
        right: match soonest {
            Some(at) => format!("resets in {}", until(at - now)),
            None if u.limits.is_empty() => "no limits".into(),
            None => String::new(),
        },
        rows: u
            .limits
            .iter()
            .map(|l| (l.label.clone(), l.percent))
            .collect(),
        totals: vec![],
        tip: lines.join("\n"),
    }
}

/// What a row says of tokens used: those worked through anew, and those from
/// the cache, which are usually billed too, and so aren't left to the hover.
fn used(t: Tokens) -> String {
    match t.cached {
        0 => short(t.total()),
        cached => format!("{} + {} cached", short(t.total()), short(cached)),
    }
}

/// A count, roughly: "950", "1.2k", "229k", "4.6M", "21M", "1.3B".
fn short(n: u64) -> String {
    let n = n as f64;
    match n {
        ..1e3 => format!("{n:.0}"),
        ..1e4 => format!("{:.1}k", n / 1e3),
        ..1e6 => format!("{:.0}k", n / 1e3),
        ..1e7 => format!("{:.1}M", n / 1e6),
        ..1e9 => format!("{:.0}M", n / 1e6),
        _ => format!("{:.1}B", n / 1e9),
    }
}

/// How long `secs` is, roughly: "45m", "2h 13m", "3d 4h".
fn until(secs: i64) -> String {
    let m = secs.max(0) / 60;
    match m {
        0 => "under a minute".into(),
        1..60 => format!("{m}m"),
        60..1440 => format!("{}h {}m", m / 60, m % 60),
        _ => format!("{}d {}h", m / 1440, m % 1440 / 60),
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's `test` would stand in for the plain one.
    use super::{Meter, by_last_claude, describe, short, until, usage};
    use crate::config::Agent;
    use crate::usage::{Limit, Spent, Tokens, Usage};
    use std::time::{Duration, Instant};

    #[test]
    fn reset_times_read_roughly() {
        assert_eq!(until(-5), "under a minute");
        assert_eq!(until(59), "under a minute");
        assert_eq!(until(45 * 60), "45m");
        assert_eq!(until(2 * 3600 + 13 * 60 + 30), "2h 13m");
        assert_eq!(until(3 * 86400 + 4 * 3600), "3d 4h");
    }

    #[test]
    fn the_meter_shows_each_limit_or_why_not() {
        let now = usage::epoch("2026-11-01T02:47:00Z").unwrap();
        let used = Usage {
            limits: vec![
                Limit {
                    label: "Session".into(),
                    percent: 42.4,
                    resets_at: usage::epoch("2026-11-01T05:00:00Z"),
                },
                Limit {
                    label: "Week · Fable".into(),
                    percent: 81.0,
                    resets_at: None,
                },
            ],
            spent: vec![],
            host: "agent".into(),
        };
        let shown = describe(Agent::Claude, Some(&Ok(used.clone())), now);
        assert_eq!(shown.right, "resets in 2h 13m");
        assert_eq!(
            shown.rows,
            [
                ("Session".to_string(), 42.4),
                ("Week · Fable".to_string(), 81.0)
            ]
        );
        assert_eq!(shown.brief, "Week · Fable 81%");
        assert_eq!(
            shown.tip,
            "Session: 42% used, resets in 2h 13m\nWeek · Fable: 81% used\n\
             From Claude Code's /usage on agent, every 10 minutes."
        );

        let none = Usage {
            limits: vec![],
            spent: vec![],
            host: "agent".into(),
        };
        let none = describe(Agent::Claude, Some(&Ok(none)), now);
        assert_eq!(
            (none.right.as_str(), none.brief.as_str()),
            ("no limits", "no limits")
        );
        let failed = describe(
            Agent::Claude,
            Some(&Err("Claude Code isn't installed there".into())),
            now,
        );
        assert_eq!(failed.right, "unavailable");
        assert_eq!(failed.brief, "unavailable");
        assert!(failed.rows.is_empty());
        assert!(failed.tip.contains("isn't installed"), "{}", failed.tip);
        assert_eq!(describe(Agent::Claude, None, now).right, "asking…");
        // Codex's says it's Codex's.
        let shown = describe(Agent::Codex, Some(&Ok(used)), now);
        assert!(
            shown
                .tip
                .ends_with("From Codex's app server on agent, every 10 minutes.")
        );
        let failed = describe(Agent::Codex, Some(&Err("no plan".into())), now);
        assert_eq!(failed.tip, "Couldn't fetch usage from Codex: no plan.");
    }

    /// On an API key: the tokens used, with no bars, as it has no limits.
    #[test]
    fn the_meter_shows_what_a_key_used() {
        let tokens = |input, cached, output| Tokens {
            input,
            cached,
            output,
        };
        let used = Usage {
            limits: vec![],
            spent: vec![
                Spent {
                    label: "24 hours",
                    tokens: tokens(4_588_020, 4_391_280, 32_215),
                },
                Spent {
                    label: "7 days",
                    tokens: tokens(5_000_000, 0, 1_000),
                },
            ],
            host: "mac".into(),
        };
        let shown = describe(Agent::Codex, Some(&Ok(used)), 0);
        assert_eq!(shown.right, "API key");
        assert_eq!(shown.brief, "24 hours 229k");
        assert!(shown.rows.is_empty());
        assert_eq!(
            shown.totals,
            [
                ("24 hours".to_string(), "229k + 4.4M cached".to_string()),
                ("7 days".to_string(), "5.0M".to_string())
            ]
        );
        assert_eq!(
            shown.tip,
            "24 hours: 229k tokens, 197k input (+ 4.4M cached) and 32k output\n\
             7 days: 5.0M tokens, 5.0M input (+ 0 cached) and 1.0k output\n\
             From Codex's session logs on mac, every 10 minutes. An API key has no plan limits to show."
        );
    }

    #[test]
    fn counts_read_roughly() {
        assert_eq!(short(0), "0");
        assert_eq!(short(950), "950");
        assert_eq!(short(1_234), "1.2k");
        assert_eq!(short(229_000), "229k");
        assert_eq!(short(4_620_235), "4.6M");
        assert_eq!(short(21_000_000), "21M");
        assert_eq!(short(1_300_000_000), "1.3B");
    }

    #[test]
    fn hosts_where_claude_ran_last_are_asked_first() {
        let hosts = vec![
            ("mac".to_string(), None),
            ("agent".to_string(), Some(30)),
            ("box".to_string(), None),
            ("old".to_string(), Some(9000)),
        ];
        assert_eq!(by_last_claude(hosts), ["agent", "old", "mac", "box"]);
    }

    #[test]
    fn the_meter_asks_when_its_time() {
        let now = Instant::now();
        let mut m = Meter::default();
        assert!(m.due(true, now, 0), "at first");
        m.asked = Some(now);
        assert!(!m.due(true, now + Duration::from_secs(60), 0));
        assert!(m.due(true, now + usage::EVERY, 0));
        assert!(
            !m.due(false, now + usage::EVERY, 0),
            "out of sight it waits longer"
        );
        m.fails = 1;
        assert!(!m.due(true, now + usage::EVERY, 0), "after a failure too");
        m.asking = true;
        assert!(!m.due(true, now + usage::SLOWEST, 0), "one ask at a time");
    }

    /// Once a limit shown has reset, its use shown is over: asked again.
    #[test]
    fn the_meter_asks_again_once_a_limit_resets() {
        let now = Instant::now();
        let reset = usage::epoch("2026-11-07T10:00:00Z").unwrap();
        let mut m = Meter {
            shown: Some(Ok(Usage {
                limits: vec![Limit {
                    label: "Week".into(),
                    percent: 95.0,
                    resets_at: Some(reset),
                }],
                spent: vec![],
                host: "agent".into(),
            })),
            asked: Some(now),
            answered: Some(reset - 600),
            ..Default::default()
        };
        let soon = now + Duration::from_secs(60);
        assert!(!m.due(true, soon, reset - 1));
        assert!(m.due(true, soon, reset + 1), "it has reset");
        m.answered = Some(reset + 1);
        assert!(!m.due(true, soon, reset + 2), "asked since");
    }
}
