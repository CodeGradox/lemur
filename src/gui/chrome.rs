//! What frames the terminal: the tabs of the worktree on show, the
//! terminal's own header, the status bar under it, and each host's load at
//! the foot of the sidebar.

use super::settings::Prefs;
use super::sidebar::thread_icon;
use super::terminal::Palette;
use super::{Ask, CloseThread, Key, Lemur, List, NewTerminal, icons, rows, sessions};
use crate::machine::{self, Load};
use crate::snapshot::{Activity, Session, Worktree};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::rc::Rc;

/// The status bar's height, and the terminal header's.
const STATUS_HEIGHT: Pixels = px(28.0);
const HEADER_HEIGHT: Pixels = px(30.0);

/// The window has no title bar: its buttons sit at the left of the top
/// row, at `WINDOW_BUTTONS_AT`, which leaves this much room for them.
pub(super) const WINDOW_BUTTONS: Pixels = px(80.0);
pub(super) const WINDOW_BUTTONS_AT: Point<Pixels> = point(px(12.0), px(12.0));

/// A top row that does a title bar's work: a drag that starts where no
/// button or tab takes the press moves the window, and a double click
/// zooms it (or does what System Settings says a title bar's does).
pub(super) fn title_bar(id: &'static str, window: &mut Window, cx: &mut App) -> Stateful<Div> {
    // Whether the left button went down on the row itself.
    let pressed =
        window.use_keyed_state(SharedString::from(format!("{id} pressed")), cx, |_, _| {
            false
        });
    let (down, out) = (pressed.clone(), pressed.clone());
    h_flex()
        .id(id)
        .debug_selector(move || id.to_string())
        .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, window, cx| {
            // Buttons and tabs take their presses before the row sees them.
            let own = !window.default_prevented();
            if own && e.click_count == 2 {
                window.titlebar_double_click();
            }
            down.update(cx, |p, _| *p = own && e.click_count == 1);
        })
        .on_mouse_down_out(move |_, _, cx| out.update(cx, |p, _| *p = false))
        .on_mouse_move(move |e: &MouseMoveEvent, window, cx| {
            if pressed.update(cx, |p, _| std::mem::take(p))
                && e.pressed_button == Some(MouseButton::Left)
            {
                window.start_window_move();
            }
        })
}

impl Lemur {
    /// The thread behind `key`, if a snapshot lists it.
    pub(super) fn session_of(&self, key: &Key) -> Option<&Session> {
        sessions(&self.snap)
            .find(|(h, s)| *h == key.0 && s.name == key.1)
            .map(|(_, s)| s)
    }

    /// The worktree the thread on show runs in, and the host it's on.
    pub(super) fn active_worktree(&self) -> Option<(&str, &Worktree)> {
        let (host, session) = self.active.as_ref()?;
        let (_, w) = self.project_of(host, session)?;
        Some((host, w))
    }

    /// Where a thread runs, for what starts beside it: its worktree and the
    /// project it's in, or its folder outside every worktree.
    fn place_of(&self, key: &Key) -> Option<(String, Option<usize>)> {
        match self.project_of(&key.0, &key.1) {
            Some((base, w)) => Some((w.path.clone(), Some(base))),
            None => Some((self.session_of(key)?.path.clone(), None)),
        }
    }

    /// The tabs of the threads where the one on show runs: its worktree's,
    /// or its host's outside every worktree. + offers what starts there.
    /// Dragged, they move in the sidebar too: it's one order.
    pub(super) fn tabs(&self, cx: &mut Context<Self>) -> Div {
        let Some(active) = self.active.clone() else {
            return h_flex().flex_1();
        };
        let list: List = (
            active.0.clone(),
            self.project_of(&active.0, &active.1)
                .map(|(_, w)| w.path.clone()),
        );
        let drag: SharedString = format!(
            "tabs\n{}\n{}",
            list.0,
            list.1.as_deref().unwrap_or_default()
        )
        .into();
        let reorder: rows::Reorder = {
            let this = cx.entity().downgrade();
            Rc::new(move |from, to, _, cx| {
                this.update(cx, |this, cx| this.reorder(list.clone(), from, to, cx))
                    .ok();
            })
        };
        let siblings = self.siblings(&active);
        // The dots of tabs no longer shown go with them.
        self.dots
            .borrow_mut()
            .retain(|(place, key), _| *place != "tab" || siblings.contains(key));
        let tabs: Vec<AnyElement> = siblings
            .into_iter()
            .enumerate()
            .filter_map(|(i, key)| {
                let s = self.session_of(&key)?.clone();
                Some(self.tab(i, key, &s, (drag.clone(), reorder.clone()), cx))
            })
            .collect();
        let add = self.place_of(&active).map(|(dir, base)| {
            let new = self.new_menu(&active.0, &dir, base, cx);
            Button::new("new-tab")
                .icon(IconName::Plus)
                .ghost()
                .xsmall()
                .tooltip("New terminal or Claude here")
                .dropdown_menu(move |m, window, cx| new(m, window, cx))
        });
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
                    // A press here is the tabs', to click or drag, not the
                    // title bar's (see `title_bar`).
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .children(tabs),
            )
            .children(add)
    }

    /// The tab at `i` of the tab bar `drag`, which a drop reorders.
    fn tab(
        &self,
        i: usize,
        key: Key,
        s: &Session,
        (drag, reorder): (SharedString, rows::Reorder),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let menu = self.tab_menu(&key, cx);
        let theme = cx.theme();
        let shown = self.active.as_ref() == Some(&key);
        let (dot, tip) = self.activity(&key, s, cx);
        let breathes = self.breathes(s.activity);
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
            .icon(IconName::Close)
            .tooltip("Close thread")
            .on_click(cx.listener({
                let key = key.clone();
                move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.confirm_close_of(key.clone(), window, cx);
                }
            }));
        let tab = h_flex()
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
            // Filled as the thread on show is in the sidebar, as Herdr's;
            // hovered ones fainter, so they don't pass for it.
            .map(|tab| {
                if shown {
                    tab.bg(theme.tokens.sidebar_accent)
                        .border_color(theme.border)
                        .text_color(theme.tab_active_foreground)
                } else {
                    tab.border_color(transparent_black())
                        .text_color(theme.muted_foreground)
                        .hover(|tab| tab.bg(theme.sidebar_accent.opacity(0.5)))
                }
            })
            .on_click(open)
            .child(thread_icon(s).small())
            .child(div().max_w(px(200.0)).truncate().child(s.label.clone()))
            .children(s.issue.clone().map(|issue| {
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(issue)
            }))
            .child(self.dot_view("tab", &key, dot, breathes, tip, cx))
            .child(close);
        rows::draggable_tab(tab, drag, i, s.label.clone().into(), reorder, cx)
            .context_menu(move |m, window, cx| menu(m, window, cx))
            .into_any_element()
    }

    /// A tab's right-click menu, as Herdr's: rename it, start a terminal
    /// beside it, close it; with the keys that do so for the one on show.
    fn tab_menu(&self, key: &Key, cx: &mut Context<Self>) -> rows::MenuBuilder {
        let this = cx.entity().downgrade();
        let key = key.clone();
        let dir = self.place_of(&key).map(|(dir, _)| dir);
        Rc::new(
            move |menu: PopupMenu, _: &mut Window, _: &mut Context<PopupMenu>| {
                let rename = {
                    let (this, (host, session)) = (this.clone(), key.clone());
                    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                        let ask = Ask::Rename {
                            host: host.clone(),
                            session: session.clone(),
                        };
                        this.update(cx, |this, cx| this.ask_name(ask, window, cx))
                            .ok();
                    }
                };
                let close = {
                    let (this, key) = (this.clone(), key.clone());
                    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                        this.update(cx, |this, cx| {
                            this.confirm_close_of(key.clone(), window, cx)
                        })
                        .ok();
                    }
                };
                let menu = menu.item(PopupMenuItem::new("Rename…").on_click(rename));
                let menu = match &dir {
                    Some(dir) => {
                        let (this, host, dir) = (this.clone(), key.0.clone(), dir.clone());
                        menu.item(
                            PopupMenuItem::new("New terminal")
                                .action(Box::new(NewTerminal))
                                .on_click(move |_, _, cx| {
                                    this.update(cx, |this, cx| {
                                        this.start_now(host.clone(), dir.clone(), true, cx)
                                    })
                                    .ok();
                                }),
                        )
                    }
                    None => menu,
                };
                let away = {
                    let (this, key) = (this.clone(), key.clone());
                    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                        this.update(cx, |this, cx| this.put_away(key.clone(), window, cx))
                            .ok();
                    }
                };
                menu.separator()
                    .item(PopupMenuItem::new("Put away").on_click(away))
                    .item(
                        PopupMenuItem::new("Close")
                            .action(Box::new(CloseThread))
                            .on_click(close),
                    )
            },
        )
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
    /// worktree's branch, its lines added and removed, and its servers),
    /// and how many agents there are.
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
            bar = bar
                .child(item(icons::git_branch(), branch.into()))
                .children(self.changes_item(host, &w.path, cx));
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
        // One that can't be reached shows no load, rather than its last.
        self.loads.retain(|name, _| {
            self.snap
                .hosts
                .iter()
                .any(|h| h.name == *name && h.error.is_none())
        });
    }

    /// Each host's CPU and memory, as bars at the foot of the sidebar; none
    /// for hosts without /proc to read them from. A click on a host's name
    /// minimizes it to that line.
    pub(super) fn machines(&self, cx: &mut Context<Self>) -> Option<Div> {
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
            let minimized = self.cfg.minimized_hosts.contains(name);
            // Minimized, the bars' numbers take the load's place.
            let right = if minimized {
                let ram = load
                    .memory()
                    .map_or("…".to_string(), |m| format!("{m:.0}%"));
                Some(format!("CPU {cpu_text} · RAM {ram}"))
            } else {
                load.load.first().map(|l| format!("load {l}"))
            };
            let toggle = {
                let name = (*name).clone();
                cx.listener(move |this, _, _, cx| this.toggle_host_minimized(&name, cx))
            };
            v_flex()
                .id(SharedString::from(format!("load-{name}")))
                .debug_selector(move || selector)
                .gap_0p5()
                .py_1()
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .child(
                    minimizable_header(
                        SharedString::from(format!("load-header-{name}")),
                        minimized,
                        cx,
                    )
                    .on_mouse_down(MouseButton::Left, toggle)
                    .child(icons::server().xsmall().text_color(theme.muted_foreground))
                    .child(div().flex_1().min_w_0().truncate().child(name.clone()))
                    .children(right.map(|right| {
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(right)
                    }))
                    .child(chevron(minimized)),
                )
                .when(!minimized, |this| {
                    this.child(bar("CPU", *cpu, cpu_text, cx)).child(bar(
                        "RAM",
                        load.memory(),
                        ram_text,
                        cx,
                    ))
                })
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

/// The first line of a block at the sidebar's foot, which a press minimizes
/// to that line, or opens again: on the press, not the release, so it's
/// done before the button comes up.
pub(super) fn minimizable_header(id: SharedString, minimized: bool, cx: &App) -> Stateful<Div> {
    let selector = format!("{id}{}", if minimized { " minimized" } else { "" });
    h_flex()
        .id(id)
        .debug_selector(move || selector)
        .gap_2()
        .items_center()
        .text_sm()
        .cursor_pointer()
        .rounded(cx.theme().radius)
}

/// The caret at a minimizable line's end, as a project's: down while open.
/// A click on it reaches the line, which minimizes.
pub(super) fn chevron(minimized: bool) -> Button {
    Button::new("minimize").xsmall().ghost().icon(
        Icon::new(IconName::ChevronRight)
            .size_4()
            .when(!minimized, |this| this.rotate(percentage(90. / 360.))),
    )
}

impl Lemur {
    /// Minimizes a host's load to its one line, or opens it again.
    pub(super) fn toggle_host_minimized(&mut self, host: &str, cx: &mut Context<Self>) {
        let hosts = &mut self.cfg.minimized_hosts;
        match hosts.iter().position(|h| h == host) {
            Some(i) => {
                hosts.remove(i);
            }
            None => hosts.push(host.to_string()),
        }
        self.save_minimized(cx);
    }

    /// Keeps what's minimized at the sidebar's foot in the config.
    pub(super) fn save_minimized(&mut self, cx: &mut Context<Self>) {
        if let Some(file) = Prefs::get(cx).config_file()
            && let Err(e) = crate::config::Config::save_minimized(
                file,
                self.cfg.minimized_usage,
                &self.cfg.minimized_hosts,
            )
        {
            self.fail(e, cx);
        }
        cx.notify();
    }
}

/// One measure of a host, as a bar: its name, how full, and the number.
/// Nearly full turns it to a warning, then to danger.
fn bar(name: &'static str, percent: Option<f32>, text: String, cx: &App) -> Div {
    bar_sized(name.into(), (px(28.0), px(92.0)), percent, text, cx)
}

/// A bar whose name and number take these widths.
pub(super) fn bar_sized(
    name: SharedString,
    (name_width, text_width): (Pixels, Pixels),
    percent: Option<f32>,
    text: String,
    cx: &App,
) -> Div {
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
                .w(name_width)
                .flex_shrink_0()
                .truncate()
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
        .child(div().w(text_width).flex_shrink_0().text_right().child(text))
}
