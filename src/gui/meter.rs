//! How much of your Claude plan is used, above the hosts' load at the foot
//! of the sidebar: each limit Claude Code's `/usage` lists, asked every
//! few minutes (see `crate::usage`), or that it couldn't be asked. Off
//! until turned on in Settings.

use super::chrome::{bar_sized, chevron, minimizable_header};
use super::settings::Prefs;
use super::sidebar::claude_icon;
use super::{Lemur, sessions};
use crate::usage::{self, Usage};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Sizable, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How often the meter checks whether it's time to ask.
pub(super) const TICK: Duration = Duration::from_secs(30);

/// Whether Lemur has seen Claude Code on a host: running there, or kept
/// to resume. Settings offers the meter once it has.
pub(super) struct ClaudeFound(pub(super) bool);

impl Global for ClaudeFound {}

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
    pub(super) fn new(cx: &App) -> Self {
        Meter {
            on: Prefs::get(cx).claude_usage,
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
                .filter_map(|l| l.resets_at.as_deref().and_then(usage::epoch))
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

/// Seconds since 1970.
fn epoch_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl Lemur {
    /// Asks, once the hosts are known, when it's time.
    pub(super) fn tick_meter(&mut self, looking: bool, cx: &mut Context<Self>) {
        if self.meter.on && self.loaded && self.meter.due(looking, Instant::now(), epoch_now()) {
            self.ask_usage(cx);
        }
    }

    /// Follows Settings: turned on, it asks at once; off, it's gone.
    pub(super) fn follow_meter_setting(&mut self, cx: &mut Context<Self>) {
        let on = Prefs::get(cx).claude_usage;
        if on == self.meter.on {
            return;
        }
        self.meter = Meter {
            on,
            ..Default::default()
        };
        if on {
            self.ask_usage(cx);
        }
        cx.notify();
    }

    /// Tells Settings whether Claude Code is around, when that changes.
    pub(super) fn note_claude(&self, cx: &mut App) {
        let found =
            !self.cfg.threads.is_empty() || sessions(&self.snap).any(|(_, s)| s.runs_claude());
        if cx.try_global::<ClaudeFound>().map(|f| f.0) != Some(found) {
            cx.set_global(ClaudeFound(found));
        }
    }

    /// Asks Claude Code how much of the plan is used, in the background.
    pub(super) fn ask_usage(&mut self, cx: &mut Context<Self>) {
        if self.meter.asking {
            return;
        }
        self.meter.asking = true;
        let cfg = self.cfg.clone();
        let order = self.usage_order();
        self.in_background(
            cx,
            move || usage::fetch(&cfg, &order),
            |this, answer, _, cx| {
                let meter = &mut this.meter;
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

    /// The hosts to ask on, the one where Claude ran last first.
    fn usage_order(&self) -> Vec<String> {
        let hosts: Vec<(String, Option<u64>)> = self
            .snap
            .hosts
            .iter()
            .filter(|h| h.error.is_none())
            .map(|h| {
                let last = sessions(&self.snap)
                    .filter(|(host, s)| *host == h.name && s.runs_claude())
                    .map(|(_, s)| s.idle_secs)
                    .min();
                (h.name.clone(), last)
            })
            .collect();
        by_last_claude(hosts)
    }

    /// "Claude" and when its next limit starts over, then a bar for each
    /// limit, or that Claude couldn't be asked; what each says on hover. A
    /// click on the bars asks again; on "Claude", it minimizes the meter to
    /// that line, the most used limit beside it.
    pub(super) fn usage_meter(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.meter.on {
            return None;
        }
        let now = epoch_now();
        let shown = describe(self.meter.shown.as_ref(), now);
        let minimized = self.cfg.minimized_usage;
        let open = !minimized;
        let failed = matches!(self.meter.shown, Some(Err(_)));
        let tip = match (&self.meter.shown, open) {
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
        let right = if minimized { shown.brief } else { shown.right };
        let header = minimizable_header("usage-header".into(), minimized, cx)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.toggle_usage_minimized(cx)),
            )
            .child(claude_icon().xsmall())
            .child(div().flex_1().min_w_0().truncate().child("Claude"))
            .child(div().text_xs().text_color(muted).child(right))
            .child(chevron(minimized));
        let body = (open && (failed || !rows.is_empty())).then(|| {
            v_flex()
                .id("usage-bars")
                .debug_selector(|| "usage bars".into())
                .gap_0p5()
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| this.ask_usage(cx)))
                .when(failed, |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("Couldn't fetch usage from Claude."),
                    )
                })
                .children(rows)
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
                        .id("usage")
                        .debug_selector(|| "usage".into())
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
    pub(super) fn toggle_usage_minimized(&mut self, cx: &mut Context<Self>) {
        self.cfg.minimized_usage = !self.cfg.minimized_usage;
        self.save_minimized(cx);
    }
}

/// Hosts by how long ago Claude last did something on them, if it runs
/// there at all, most recent first; the rest in their order after.
fn by_last_claude(mut hosts: Vec<(String, Option<u64>)>) -> Vec<String> {
    hosts.sort_by_key(|(_, idle)| (idle.is_none(), idle.unwrap_or(0)));
    hosts.into_iter().map(|(name, _)| name).collect()
}

/// What the meter shows: beside "Claude", open and minimized, a bar for
/// each limit, and on hover.
struct Shown {
    right: String,
    brief: String,
    rows: Vec<(String, f64)>,
    tip: String,
}

/// The meter for the last answer, if any; `now` in seconds since 1970.
fn describe(shown: Option<&Result<Usage, String>>, now: i64) -> Shown {
    let u = match shown {
        None => {
            return Shown {
                right: "asking…".into(),
                brief: "asking…".into(),
                rows: vec![],
                tip: "Asking Claude Code how much of your plan is used…".into(),
            };
        }
        Some(Err(why)) => {
            return Shown {
                right: "unavailable".into(),
                brief: "unavailable".into(),
                rows: vec![],
                tip: format!("Couldn't fetch usage from Claude: {why}."),
            };
        }
        Some(Ok(u)) => u,
    };
    let mut lines = vec![];
    let mut soonest: Option<i64> = None;
    for l in &u.limits {
        let at = l.resets_at.as_deref().and_then(usage::epoch);
        if let Some(at) = at.filter(|at| *at > now) {
            soonest = Some(soonest.map_or(at, |s| s.min(at)));
        }
        let resets = at.map_or(String::new(), |at| {
            format!(", resets in {}", until(at - now))
        });
        lines.push(format!("{}: {:.0}% used{resets}", l.label, l.percent));
    }
    if u.limits.is_empty() {
        lines.push("Claude Code listed no limits for your plan.".into());
    }
    lines.push(format!(
        "From Claude Code's /usage on {}, every 10 minutes.",
        u.host
    ));
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
        tip: lines.join("\n"),
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
    use super::{Meter, by_last_claude, describe, until, usage};
    use crate::usage::{Limit, Usage};
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
                    resets_at: Some("2026-11-01T05:00:00Z".into()),
                },
                Limit {
                    label: "Week · Fable".into(),
                    percent: 81.0,
                    resets_at: Some("not a time".into()),
                },
            ],
            host: "agent".into(),
        };
        let shown = describe(Some(&Ok(used)), now);
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
            host: "agent".into(),
        };
        let none = describe(Some(&Ok(none)), now);
        assert_eq!(
            (none.right.as_str(), none.brief.as_str()),
            ("no limits", "no limits")
        );
        let failed = describe(Some(&Err("Claude Code isn't installed there".into())), now);
        assert_eq!(failed.right, "unavailable");
        assert_eq!(failed.brief, "unavailable");
        assert!(failed.rows.is_empty());
        assert!(failed.tip.contains("isn't installed"), "{}", failed.tip);
        assert_eq!(describe(None, now).right, "asking…");
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
                    resets_at: Some("2026-11-07T10:00:00Z".into()),
                }],
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
