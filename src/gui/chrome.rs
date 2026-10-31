//! What frames the terminal: the tabs of the worktree on show, the
//! terminal's own header, the status bar under it, and each host's load at
//! the foot of the sidebar.

use super::settings::Prefs;
use super::sidebar::{status_dot, thread_icon};
use super::terminal::Palette;
use super::{Ask, Key, Lemur, icons, sessions};
use crate::machine::{self, Load};
use crate::snapshot::{Activity, Session, Worktree};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

/// The status bar's height, and the terminal header's.
const STATUS_HEIGHT: Pixels = px(28.0);
const HEADER_HEIGHT: Pixels = px(30.0);

impl Lemur {
    /// The thread behind `key`, if a snapshot lists it.
    pub(super) fn session_of(&self, key: &Key) -> Option<&Session> {
        sessions(&self.snap)
            .find(|(h, s)| *h == key.0 && s.name == key.1)
            .map(|(_, s)| s)
    }

    /// The worktree the thread on show runs in, and the host it's on.
    fn active_worktree(&self) -> Option<(&str, &Worktree)> {
        let (host, session) = self.active.as_ref()?;
        let (_, w) = self.project_of(host, session)?;
        Some((host, w))
    }

    /// The tabs of the threads where the one on show runs: its worktree's,
    /// or its host's outside every worktree. + starts another there.
    pub(super) fn tabs(&self, cx: &mut Context<Self>) -> Div {
        let Some(active) = self.active.clone() else {
            return h_flex().flex_1();
        };
        let tabs: Vec<AnyElement> = self
            .siblings(&active)
            .into_iter()
            .enumerate()
            .filter_map(|(i, key)| {
                let s = self.session_of(&key)?.clone();
                Some(self.tab(i, key, &s, cx))
            })
            .collect();
        let add = Button::new("new-tab")
            .icon(IconName::Plus)
            .ghost()
            .xsmall()
            .tooltip("New thread here (⌘T)")
            .on_click(cx.listener(|this, _, window, cx| this.new_thread_here(window, cx)));
        h_flex()
            .flex_1()
            .min_w_0()
            .gap_1()
            .child(
                h_flex()
                    .id("tabs")
                    .min_w_0()
                    .gap_1()
                    .overflow_x_scroll()
                    .children(tabs),
            )
            .when(self.worktree_of(&active.0, &active.1).is_some(), |bar| {
                bar.child(add)
            })
    }

    fn tab(&self, i: usize, key: Key, s: &Session, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let shown = self.active.as_ref() == Some(&key);
        let arming = self.confirm_kill.as_ref() == Some(&key);
        let (dot, tip) = self.activity(&key, s, cx);
        let working = s.activity == Activity::Working;
        let open = {
            let key = key.clone();
            cx.listener(move |this, event: &ClickEvent, window, cx| {
                let (host, session) = key.clone();
                if event.click_count() >= 2 {
                    this.ask_name(Ask::Rename { host, session }, window, cx);
                } else {
                    this.open(host, session, window, cx);
                }
            })
        };
        let close = Button::new(SharedString::from(format!("close-tab-{i}")))
            .ghost()
            .xsmall()
            .on_click(cx.listener({
                let key = key.clone();
                move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.kill(key.clone(), cx);
                }
            }));
        let close = if arming {
            close.danger().label("Kill?")
        } else {
            close.icon(IconName::Close).tooltip("Kill thread")
        };
        h_flex()
            .id(SharedString::from(format!("tab-{i}")))
            .debug_selector(move || format!("tab-{i}"))
            .flex_shrink_0()
            .h_7()
            .pl_2()
            .pr_1()
            .gap_1p5()
            .items_center()
            .rounded_md()
            .border_1()
            .text_sm()
            .cursor_pointer()
            .map(|tab| {
                if shown {
                    tab.bg(theme.tab_active)
                        .border_color(theme.border)
                        .text_color(theme.tab_active_foreground)
                } else {
                    tab.border_color(transparent_black())
                        .text_color(theme.muted_foreground)
                        .hover(|tab| tab.bg(theme.secondary_hover))
                }
            })
            .on_click(open)
            .child(thread_icon(s).small())
            .child(div().max_w(px(200.0)).truncate().child(s.label.clone()))
            .child(status_dot(&key, dot, working, tip))
            .child(close)
            .into_any_element()
    }

    /// Over the terminal: what runs in it, and where.
    pub(super) fn pane_header(&self, cx: &App) -> Option<Div> {
        let s = self.session_of(self.active.as_ref()?)?;
        let what = if s.runs_claude() {
            "Claude Code"
        } else {
            "Shell"
        };
        Some(
            h_flex()
                .h(HEADER_HEIGHT)
                .flex_shrink_0()
                .px_3()
                .gap_2()
                .items_center()
                .border_b_1()
                .border_color(cx.theme().border)
                .text_sm()
                .child(thread_icon(s).small())
                .child(div().flex_shrink_0().child(what))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(cx.theme().muted_foreground)
                        .child(s.path.clone()),
                ),
        )
    }

    /// The terminal on show in a rounded frame, with its header; the ring
    /// says it has the keyboard.
    pub(super) fn framed(&self, terminal: AnyElement, focused: bool, cx: &App) -> Div {
        let palette = Palette::themed(cx.theme().is_dark(), &Prefs::get(cx).terminal);
        let bg = palette.background;
        let bg = rgb(((bg.r as u32) << 16) | ((bg.g as u32) << 8) | bg.b as u32);
        let ring = if focused {
            cx.theme().ring
        } else {
            cx.theme().border
        };
        div().size_full().p_2().child(
            v_flex()
                .size_full()
                .rounded(px(10.0))
                .border_1()
                .border_color(ring)
                .overflow_hidden()
                .bg(bg)
                .children(self.pane_header(cx))
                .child(div().flex_1().min_h_0().child(terminal)),
        )
    }

    /// Under the terminal: where the thread on show runs (its host, and its
    /// worktree's branch and servers), and how many agents there are.
    pub(super) fn status_bar(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        let item = |icon: Icon, text: SharedString| {
            h_flex()
                .gap_1p5()
                .items_center()
                .child(icon.xsmall())
                .child(text)
        };
        let mut bar = h_flex()
            .h(STATUS_HEIGHT)
            .flex_shrink_0()
            .px_3()
            .gap_4()
            .items_center()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.status_bar)
            .text_xs()
            .text_color(theme.muted_foreground);
        if let Some((host, _)) = &self.active
            && let Some(h) = self.cfg.host(host)
        {
            bar = bar.child(if h.ssh == "local" {
                item(icons::monitor(), "This Mac".into())
            } else {
                item(icons::server(), h.name.clone().into())
            });
        }
        if let Some((host, w)) = self.active_worktree() {
            let branch = match (&w.branch, &w.head) {
                (Some(branch), _) => branch.clone(),
                (None, Some(head)) => format!("detached at {}", &head[..head.len().min(8)]),
                (None, None) => crate::config::leaf(&w.path).to_string(),
            };
            bar = bar.child(item(icons::git_branch(), branch.into()));
            let links = self.links(host, w, cx);
            if !links.is_empty() {
                bar = bar.child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(Icon::new(IconName::Globe).xsmall().mr_1())
                        .children(links),
                );
            }
        }
        let all: Vec<&Session> = sessions(&self.snap)
            .map(|(_, s)| s)
            .filter(|s| s.runs_claude())
            .collect();
        let needs = all
            .iter()
            .filter(|s| s.activity == Activity::NeedsInput)
            .count();
        let agents = format!(
            "{} agent{}",
            all.len(),
            if all.len() == 1 { "" } else { "s" }
        );
        bar.child(
            h_flex()
                .gap_1p5()
                .items_center()
                .child(div().size_1p5().rounded_full().bg(theme.muted_foreground))
                .child(agents)
                .when(needs > 0, |this| {
                    this.child(div().text_color(theme.warning).child(format!(
                        "· {needs} need{} you",
                        if needs == 1 { "s" } else { "" }
                    )))
                }),
        )
    }

    /// The worktree's servers, by port: a link to each one that can be
    /// reached from here. Another host's is reached by its tailnet name.
    fn links(&self, host: &str, w: &Worktree, cx: &App) -> Vec<AnyElement> {
        let Some(view) = self.snap.hosts.iter().find(|h| h.name == host) else {
            return vec![];
        };
        let Some(h) = self.cfg.host(host) else {
            return vec![];
        };
        // A worktree inside this one keeps its own servers.
        let deeper: Vec<&str> = view
            .bases
            .iter()
            .flat_map(|b| b.worktrees.iter())
            .map(|o| o.path.as_str())
            .filter(|p| *p != w.path && p.starts_with(&format!("{}/", w.path)))
            .collect();
        let local = h.ssh == "local";
        let name = match &view.machine.tailnet {
            _ if local => "localhost".to_string(),
            Some(tailnet) => tailnet.clone(),
            None => h.name.clone(),
        };
        let theme = cx.theme().clone();
        machine::servers_in(&view.machine.servers, &w.path, &deeper)
            .into_iter()
            .map(|s| {
                let url = format!("http://{name}:{}", s.port);
                let reachable = local || !s.local_only();
                let tip: SharedString = if reachable {
                    format!("{url} · {}", s.name).into()
                } else {
                    format!(
                        "{} on :{} listens only on {}'s own localhost",
                        s.name, s.port, h.name
                    )
                    .into()
                };
                let port = s.port;
                div()
                    .id(SharedString::from(format!("port-{port}")))
                    .debug_selector(move || format!("port {port}"))
                    .px_1()
                    .rounded_sm()
                    .text_color(if reachable {
                        theme.link
                    } else {
                        theme.muted_foreground
                    })
                    .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                    .when(reachable, |link| {
                        link.cursor_pointer()
                            .hover(|link| link.bg(theme.secondary_hover))
                            .on_click(move |_, _, cx| cx.open_url(&url))
                    })
                    .child(format!(":{}", s.port))
                    .into_any_element()
            })
            .collect()
    }

    /// Follows each host's load: the CPU share is the busy time between two
    /// readings.
    pub(super) fn follow_loads(&mut self) {
        for h in &self.snap.hosts {
            let Some(load) = &h.machine.load else {
                continue;
            };
            let cpu = match self.loads.get(&h.name) {
                Some((before, cpu)) if before.total == load.total => *cpu,
                Some((before, _)) => load.cpu_since(before),
                None => None,
            };
            self.loads.insert(h.name.clone(), (load.clone(), cpu));
        }
        self.loads
            .retain(|name, _| self.snap.hosts.iter().any(|h| h.name == *name));
    }

    /// Each host's CPU and memory, as bars at the foot of the sidebar; none
    /// for hosts without /proc to read them from.
    pub(super) fn machines(&self, cx: &App) -> Option<Div> {
        let mut hosts: Vec<(&String, &(Load, Option<f32>))> = self.loads.iter().collect();
        if hosts.is_empty() {
            return None;
        }
        hosts.sort_by_key(|(name, _)| {
            self.cfg
                .hosts
                .iter()
                .position(|h| h.name == **name)
                .unwrap_or(usize::MAX)
        });
        let theme = cx.theme();
        let gib = |kb: u64| format!("{:.1}", kb as f64 / 1_048_576.0);
        let rows = hosts.into_iter().map(|(name, (load, cpu))| {
            let tip: SharedString = format!(
                "{name}: CPU {}{} · load {} · memory {} of {} GiB{}",
                cpu.map_or("…".into(), |c| format!("{c:.0}%")),
                load.cores
                    .map_or(String::new(), |n| format!(" of {n} cores")),
                load.load.join(" "),
                gib(load.mem_used_kb),
                gib(load.mem_total_kb),
                if load.swap_total_kb > 0 {
                    format!(
                        " · swap {} of {} GiB",
                        gib(load.swap_used_kb),
                        gib(load.swap_total_kb)
                    )
                } else {
                    String::new()
                },
            )
            .into();
            let cpu_text = cpu.map_or("…".to_string(), |c| format!("{c:.0}%"));
            let ram_text = format!("{} / {} GiB", gib(load.mem_used_kb), gib(load.mem_total_kb));
            let selector = format!("load {name}");
            v_flex()
                .id(SharedString::from(format!("load-{name}")))
                .debug_selector(move || selector)
                .gap_0p5()
                .py_1()
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .text_sm()
                        .child(icons::server().xsmall().text_color(theme.muted_foreground))
                        .child(div().flex_1().min_w_0().truncate().child(name.clone()))
                        .children(load.load.first().map(|l| {
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(format!("load {l}"))
                        })),
                )
                .child(bar("CPU", *cpu, cpu_text, cx))
                .child(bar("RAM", load.memory(), ram_text, cx))
        });
        Some(
            v_flex()
                .w_full()
                .px_2()
                .py_1()
                .gap_1()
                .border_t_1()
                .border_color(theme.sidebar_border)
                .children(rows),
        )
    }
}

/// One measure of a host, as a bar: its name, how full, and the number.
/// Nearly full turns it to a warning, then to danger.
fn bar(name: &'static str, percent: Option<f32>, text: String, cx: &App) -> Div {
    let theme = cx.theme();
    let fill = percent.unwrap_or(0.0).clamp(0.0, 100.0);
    let color = match fill {
        90.0.. => theme.danger,
        75.0.. => theme.warning,
        _ => theme.primary,
    };
    h_flex()
        .gap_2()
        .items_center()
        .text_xs()
        .child(
            div()
                .w(px(28.0))
                .flex_shrink_0()
                .text_color(theme.muted_foreground)
                .child(name),
        )
        .child(
            div()
                .flex_1()
                .h_1p5()
                .rounded_full()
                .bg(theme.border)
                .child(
                    div()
                        .h_full()
                        .rounded_full()
                        .bg(color)
                        .w(relative(fill / 100.0)),
                ),
        )
        .child(div().w(px(92.0)).flex_shrink_0().text_right().child(text))
}
