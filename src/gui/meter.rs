//! How much of each agent's plan is used, above the hosts' load at the foot
//! of the sidebar: each limit it lists, asked every few minutes (see
//! `crate::usage`), or that it couldn't be asked; with no plan to measure
//! against, the tokens used. Off until turned on in Settings.

use super::prompt::agent_icon;
use super::{Lemur, Prefs, icons, sessions};
use crate::config::Agent;
use crate::usage::{self, Tokens, Usage};
use rune::{El, col, el, icon, row, text};
use rune_kit::Kit;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Default)]
pub struct Meter {
    /// Turned on in Settings, as last followed.
    on: bool,
    /// The last answer: usage, or why there was none. None until the first.
    shown: Option<Result<Usage, String>>,
    /// Asks in a row that failed.
    fails: u32,
    asked: Option<Instant>,
    /// When the answer shown came, in seconds since 1970.
    answered: Option<i64>,
    asking: bool,
}

impl Meter {
    /// Whether it's time to ask again (see `usage::wait`), or a limit shown
    /// has reset since the answer (`epoch`, seconds since 1970, is now):
    /// its use shown is over.
    fn due(&self, looking: bool, now: Instant, epoch: i64) -> bool {
        let reset = match (&self.shown, self.answered) {
            (Some(Ok(u)), Some(answered)) => u.limits.iter().filter_map(|l| l.resets_at).any(|at| answered < at && at <= epoch),
            _ => false,
        };
        !self.asking && (reset || self.asked.is_none_or(|at| now.duration_since(at) >= usage::wait(looking, self.fails)))
    }
}

/// Claude's meter and Codex's, and what Settings needs to offer them.
#[derive(Default)]
pub struct Meters {
    claude: Meter,
    codex: Meter,
    /// Whether Lemur has seen Claude Code on a host: running there, or kept
    /// to resume. Settings offers the meter once it has.
    claude_found: bool,
    /// The same of Codex: running on a host.
    codex_found: bool,
}

/// Whether Settings has `agent`'s meter on.
fn setting(prefs: &Prefs, agent: Agent) -> bool {
    match agent {
        Agent::Claude => prefs.claude_usage,
        Agent::Codex => prefs.codex_usage,
    }
}

/// Seconds since 1970.
fn epoch_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

const AGENTS: [Agent; 2] = [Agent::Claude, Agent::Codex];

impl Lemur {
    fn meter_of(&mut self, agent: Agent) -> &mut Meter {
        match agent {
            Agent::Claude => &mut self.panels.meters.claude,
            Agent::Codex => &mut self.panels.meters.codex,
        }
    }

    /// Follows Settings, then asks how much of each plan is used once the hosts are
    /// known, when it's due. Turned on, a meter asks at once; off, what it showed is
    /// gone. The refresh ticks call it, and Settings (`follow_meter_setting`) as a switch
    /// changes, so that's at once.
    pub(super) fn tick_meter(&mut self) {
        let looking = self.looking;
        for agent in AGENTS {
            let on = setting(&self.prefs, agent);
            let loaded = self.loaded;
            let meter = self.meter_of(agent);
            if on != meter.on {
                *meter = Meter { on, ..Default::default() };
            }
            if meter.on && loaded && meter.due(looking, Instant::now(), epoch_now()) {
                self.ask_usage(agent);
            }
        }
    }

    /// Follows Settings once it has changed `prefs.claude_usage` or `codex_usage`: turned
    /// on, a meter asks at once; off, it's gone.
    pub(super) fn follow_meter_setting(&mut self) {
        self.tick_meter();
    }

    /// Whether Lemur has seen Claude Code on a host (a kept thread, or one running it), so
    /// Settings offers its usage.
    pub(super) fn claude_found(&self) -> bool {
        self.panels.meters.claude_found
    }

    /// The same of Codex: running on a host.
    pub(super) fn codex_found(&self) -> bool {
        self.panels.meters.codex_found
    }

    /// Tells Settings whether the agents are around, after every snapshot.
    pub(super) fn note_agents(&mut self) {
        let found = !self.cfg.threads.is_empty() || sessions(&self.snap).any(|(_, s)| s.runs_claude());
        self.panels.meters.claude_found = found;
        self.panels.meters.codex_found = sessions(&self.snap).any(|(_, s)| s.agent() == Some(Agent::Codex));
    }

    /// Asks `agent` how much of the plan is used, in the background.
    pub(super) fn ask_usage(&mut self, agent: Agent) {
        let order = self.usage_order(agent);
        let meter = self.meter_of(agent);
        if meter.asking {
            return;
        }
        meter.asking = true;
        let cfg = self.cfg.clone();
        self.in_background(
            move || usage::fetch(&cfg, agent, &order),
            move |s, answer| {
                let meter = s.meter_of(agent);
                meter.asking = false;
                if !meter.on {
                    return;
                }
                meter.asked = Some(Instant::now());
                meter.answered = Some(epoch_now());
                meter.fails = if answer.is_ok() { 0 } else { meter.fails + 1 };
                meter.shown = Some(answer);
            },
        );
    }

    /// The hosts to ask on, the one where `agent` ran last first.
    fn usage_order(&self, agent: Agent) -> Vec<String> {
        let hosts: Vec<(String, Option<u64>)> = self
            .snap
            .hosts
            .iter()
            .filter(|h| h.error.is_none())
            .map(|h| {
                let last = sessions(&self.snap).filter(|(host, s)| *host == h.name && s.agent() == Some(agent)).map(|(_, s)| s.idle_secs).min();
                (h.name.clone(), last)
            })
            .collect();
        by_last_claude(hosts)
    }

    /// Minimizes the meter to its one line, or opens it again.
    pub(super) fn toggle_usage_minimized(&mut self, agent: Agent) {
        let minimized = match agent {
            Agent::Claude => &mut self.cfg.minimized_usage,
            Agent::Codex => &mut self.cfg.minimized_codex_usage,
        };
        *minimized = !*minimized;
        self.save_minimized();
    }

    /// Minimizes a host's load to its one line, or opens it again.
    pub(super) fn toggle_host_minimized(&mut self, host: &str) {
        let hosts = &mut self.cfg.minimized_hosts;
        match hosts.iter().position(|h| h == host) {
            Some(i) => {
                hosts.remove(i);
            }
            None => hosts.push(host.to_string()),
        }
        self.save_minimized();
    }

    /// Keeps what's minimized at the sidebar's foot in the config.
    fn save_minimized(&mut self) {
        if let Some(file) = self.prefs.config_file()
            && let Err(e) = crate::config::Config::save_minimized(file, self.cfg.minimized_usage, self.cfg.minimized_codex_usage, &self.cfg.minimized_hosts)
        {
            self.fail(e);
        }
    }
}

/// How much of each agent's plan is used, at the sidebar's foot: Claude's, then Codex's.
pub fn meters(s: &Lemur, k: &Kit) -> Vec<El> {
    AGENTS.into_iter().filter_map(|agent| usage_meter(s, agent, k)).collect()
}

/// The agent's name and when its next limit starts over, then a bar for
/// each limit, or that it couldn't be asked; what each says on hover. A
/// click on the bars asks again; on its name, it minimizes the meter to
/// that line, the most used limit beside it.
fn usage_meter(s: &Lemur, agent: Agent, k: &Kit) -> Option<El> {
    // As Settings has it, even before the meter has followed: it asks at the next tick.
    if !setting(&s.prefs, agent) {
        return None;
    }
    let c = &k.c;
    let (meter, minimized, name, id) = match agent {
        Agent::Claude => (&s.panels.meters.claude, s.cfg.minimized_usage, "Claude", "usage"),
        Agent::Codex => (&s.panels.meters.codex, s.cfg.minimized_codex_usage, "Codex", "codex-usage"),
    };
    let shown = describe(agent, meter.shown.as_ref(), epoch_now());
    let open = !minimized;
    let failed = matches!(meter.shown, Some(Err(_)));
    let tip = match (&meter.shown, open) {
        (Some(Ok(_)), true) if shown.rows.is_empty() => format!("{}\nClick to ask now.", shown.tip),
        (Some(Ok(_)), true) => format!("{}\nClick a bar to ask now.", shown.tip),
        (Some(Err(_)), true) => format!("{} Click to try again.", shown.tip),
        _ => shown.tip,
    };
    let rows: Vec<El> = shown.rows.into_iter().map(|(label, percent)| bar(label, Some(percent as f32), format!("{percent:.0}%"), (76.0, 36.0), k)).collect();
    let totals: Vec<El> = shown
        .totals
        .into_iter()
        .map(|(label, used)| {
            row()
                .gap(8.0)
                .center_y()
                .child(text(label).font(k.ty.small).color(c.text_faint))
                // Narrow, its end goes first: the hover says it all.
                .child(text(used).font(k.ty.small).color(c.text_muted).truncate().grow(1.0).align(rune::Align::End))
        })
        .collect();
    let right = if minimized { shown.brief } else { shown.right };
    let header = minimizable_header(format!("{id}-header"), k)
        .on_press(move |s: &mut Lemur| s.toggle_usage_minimized(agent))
        .child(agent_icon(Some(agent), k))
        .child(text(name).font(k.ty.ui_medium).color(c.text_base).truncate().grow(1.0))
        .child(text(right).font(k.ty.small).color(c.text_muted))
        .child(chevron(minimized, k));
    let body = (open && (failed || !rows.is_empty() || !totals.is_empty())).then(|| {
        let mut body = col().key(format!("{id}-bars")).gap(6.0).cursor(rune::Cursor::Pointer).on_press(move |s: &mut Lemur| s.ask_usage(agent));
        if failed {
            body = body.child(text(format!("Couldn't fetch usage from {name}.")).font(k.ty.small).color(c.text_muted));
        }
        body.children(rows).children(totals)
    });
    Some(col().key(id).w_full().px(8.0).pt(8.0).pb(4.0).gap(6.0).help(tip).child(header).children(body))
}

/// The first line of a block at the sidebar's foot, which a click minimizes
/// to that line, or opens again.
pub(super) fn minimizable_header(key: String, k: &Kit) -> El {
    row().key(key).w_full().gap(8.0).center_y().radius(k.radius.sm).cursor(rune::Cursor::Pointer)
}

/// The caret at a minimizable line's end, as a project's: down while open.
/// A click on it reaches the line, which minimizes.
pub(super) fn chevron(minimized: bool, k: &Kit) -> El {
    icon(&icons::CHEVRON_DOWN, 12.0).color(k.c.icon_faint).icon_rotation(if minimized { -1.5708 } else { 0.0 })
}

/// One measure as a bar: its name, how full, and the number, which take these
/// widths. Nearly full turns it to a warning, then to danger.
pub(super) fn bar(name: String, percent: Option<f32>, value: String, (name_w, value_w): (f32, f32), k: &Kit) -> El {
    let c = &k.c;
    let frac = percent.unwrap_or(0.0).clamp(0.0, 100.0) / 100.0;
    let ink = match percent.unwrap_or(0.0) {
        p if p >= 90.0 => c.danger.icon,
        p if p >= 75.0 => c.warning.icon,
        _ => c.text_muted,
    };
    let track = row()
        .grow(1.0)
        .h(4.0)
        .radius(2.0)
        .fill(c.layer_03)
        .clip_rounded(2.0)
        .child(el().grow(frac).h_full().fill(ink))
        .child(el().grow(1.0 - frac).h_full());
    row()
        .gap(8.0)
        .center_y()
        .child(text(name).font(k.ty.small).color(c.text_faint).truncate().w(name_w))
        .child(track)
        .child(text(value).font(k.ty.small).color(c.text_muted).w(value_w).align(rune::Align::End))
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
        lines.push(format!("From {name}'s session logs on {}, every 10 minutes. An API key has no plan limits to show.", u.host));
        return Shown {
            right: "API key".into(),
            brief: format!("{} {}", first.label, short(first.tokens.total())),
            rows: vec![],
            totals: u.spent.iter().map(|s| (s.label.to_string(), used(s.tokens))).collect(),
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
        let resets = at.map_or(String::new(), |at| format!(", resets in {}", until(at - now)));
        lines.push(format!("{}: {:.0}% used{resets}", l.label, l.percent));
    }
    if u.limits.is_empty() {
        lines.push(format!("{} listed no limits for your plan.", agent.name()));
    }
    lines.push(format!("From {source} on {}, every 10 minutes.", u.host));
    // The most used limit, for the one line.
    let most = u.limits.iter().max_by(|a, b| a.percent.total_cmp(&b.percent));
    Shown {
        brief: most.map_or("no limits".into(), |l| format!("{} {:.0}%", l.label, l.percent)),
        right: match soonest {
            Some(at) => format!("resets in {}", until(at - now)),
            None if u.limits.is_empty() => "no limits".into(),
            None => String::new(),
        },
        rows: u.limits.iter().map(|l| (l.label.clone(), l.percent)).collect(),
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
#[path = "meter_tests.rs"]
mod tests;
