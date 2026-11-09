//! The window's frame: the sidebar beside the main pane, the tabs and the panels' buttons
//! in the toolbar, the terminal in its frame with its header, the status bar under it, and
//! what shows over the window (the spotlight, a dialog, a peek, snackbars).

use super::card;
use super::prompt::{Ask, agent_icon, new_menu, new_picked};
use super::silence::silence_label;
use super::snackbar;
use super::{Action, Key, Lemur, Right, icons, panels, peek, prompt, sessions, sidebar, terminal};
use crate::config::Agent;
use crate::machine;
use crate::snapshot::Activity;
use rune::{Cx, El, PopupItem, PopupMenu, Tx, col, el, icon, row, text};
use rune_kit::shell::{SidebarToggle, SplitView, TabItem, TabItemStatus, TabOrientation, TabOverflow, TabReorder, TabStrip, Toolbar};
use rune_kit::{Button, ButtonVariant, Checkbox, ControlSize, Kit};

/// How many tabs have a key (⌘1 to ⌘9).
const NUMBERED_TABS: usize = 9;
/// Points between the tab bar's scroll arrows and its tabs.
const ARROW_GAP: f32 = 2.0;
/// Points between the tabs and +, as the strip keeps before its own trailing action.
const TAB_GAP: f32 = 6.0;
/// Points of window around the main pane, and between it and the sidebar: the sidebar's
/// resize grip, as wide, so the gap shows where to drag.
const FRAME: f32 = 6.0;

/// A tab's right-click menu, as Herdr's: rename it, start a terminal beside it, put it away,
/// close it.
fn tab_menu(s: &Lemur, key: &Key) -> PopupMenu {
    let mut menu = PopupMenu::new().item(PopupItem::new("rename", "Rename…"));
    if s.place_of(key).is_some() {
        let mut new = PopupItem::new("new-terminal", "New terminal");
        if let Some(k) = Action::NewTerminal.key_label().and_then(|k| k.parse().ok()) {
            new = new.key_equivalent(k);
        }
        menu = menu.item(new);
    }
    menu = menu.separator();
    if let Some(t) = s.session_of(key).filter(|t| t.agent().is_some()) {
        menu = menu.item(PopupItem::new("silence", silence_label(t.silent)));
    }
    let mut close = PopupItem::new("close", "Close");
    if let Some(k) = Action::CloseThread.key_label().and_then(|k| k.parse().ok()) {
        close = close.key_equivalent(k);
    }
    menu.item(PopupItem::new("put-away", "Put away")).item(close)
}

fn tab_picked(s: &mut Lemur, picked: &str, key: Key) {
    match picked {
        "rename" => s.ask_name(Ask::Rename { host: key.0, session: key.1 }),
        "new-terminal" => {
            if let Some((dir, _)) = s.place_of(&key) {
                s.start_now(key.0, dir, None);
            }
        }
        "silence" => {
            let silent = s.session_of(&key).is_some_and(|t| t.silent);
            s.set_silent(key, !silent);
        }
        "put-away" => s.put_away(key),
        "close" => s.confirm_close_of(key),
        _ => {}
    }
}

/// The tabs of the threads where the one on show runs: its worktree's, or its host's
/// outside every worktree. + offers what starts there. Dragged, they move in the sidebar
/// too: it's one order.
fn tabs(s: &Lemur, k: &Kit) -> Option<El> {
    let active = s.active.clone()?;
    let siblings = s.siblings(&active);
    let numbered: Vec<String> = siblings.iter().map(|(_, n)| n.clone()).collect();
    let mut ids: Vec<String> = Vec::new();
    let items: Vec<TabItem<'_, Lemur>> = siblings
        .iter()
        .enumerate()
        .filter_map(|(i, key)| {
            let t = s.session_of(key)?;
            ids.push(key.1.clone());
            let shown = s.active.as_ref() == Some(key);
            let status = match t.activity {
                Activity::NeedsInput => Some(TabItemStatus::Attention),
                Activity::Working => Some(TabItemStatus::Busy),
                _ if s.done.contains(key) => Some(TabItemStatus::Unread),
                _ => None,
            };
            let number = (i < NUMBERED_TABS).then(|| format!("{}", i + 1));
            let issue = t.issue.clone();
            let menu = tab_menu(s, key);
            let menu_key = key.clone();
            let label = t.label.clone();
            let mut item = TabItem::new(key.1.as_str(), t.label.as_str())
                .avatar(agent_icon(t.agent(), k))
                .status(status.filter(|st| !shown || *st == TabItemStatus::Busy));
            item = item.decorate(move |tab, _| {
                let label_tip = label;
                let tab = tab.on_secondary_press(move |_: &mut Lemur, press, tx: &mut Tx| {
                    let key = menu_key.clone();
                    tx.popup_menu(menu.clone(), press, move |s: &mut Lemur, picked, _| {
                        if let Some(p) = picked {
                            tab_picked(s, p, key);
                        }
                    });
                });
                // Its name, its key and the issue pinned to it on hover; the tab stays narrow.
                let mut tip = label_tip;
                if let Some(n) = number {
                    tip = format!("{tip}  ⌘{n}");
                }
                if let Some(issue) = issue {
                    tip = format!("{tip}  · {issue}");
                }
                tab.help(tip)
            });
            Some(item)
        })
        .collect();
    let host = active.0.clone();
    let place = s.place_of(&active);
    let plus = place.map(|(dir, base)| {
        let (host, dir) = (host.clone(), dir);
        icons::button(&icons::PLUS, "New terminal or Claude here")
            .on_press_tx(move |_: &mut Lemur, tx: &mut Tx| {
                let (host, dir) = (host.clone(), dir.clone());
                tx.popup_menu(new_menu(), "new-tab", move |s: &mut Lemur, picked, _| {
                    if let Some(p) = picked {
                        new_picked(s, p, &host, &dir, base);
                    }
                });
            })
            .build(k)
            .key("new-tab")
    });
    let list = s.list_of(&active);
    let mut strip = TabStrip::new("tabs", TabOrientation::Horizontal)
        .label("Threads in this worktree")
        .tabs(items)
        .active(Some(active.1.as_str()))
        .overflow(TabOverflow::ShrinkThenScroll { min_width: 44.0, max_width: 180.0, pinned_width: 36.0, fade_width: 24.0 })
        .drag_state(&s.tab_drag, |s: &mut Lemur| &mut s.tab_drag)
        .close_icon(&icons::X)
        .on_select(move |s: &mut Lemur, id, _| s.open(host.clone(), id.to_string()))
        .on_close({
            let host = active.0.clone();
            move |s: &mut Lemur, id, _| s.confirm_close_of((host.clone(), id.to_string()))
        })
        .on_double_click({
            let host = active.0.clone();
            move |s: &mut Lemur, id, _| s.ask_name(Ask::Rename { host: host.clone(), session: id.to_string() })
        })
        .on_reorder(move |s: &mut Lemur, r: TabReorder, _| {
            // The tabs leave out what's put away; the list has it, at its end.
            let Some(sessions) = s.list_mut(&list) else { return };
            let at = |name: &str| sessions.iter().position(|t| t.name == name);
            let (Some(from), Some(to)) = (at(&numbered[r.from]), at(&numbered[r.to])) else { return };
            s.reorder(list.clone(), from, to);
        });
    // In the background a working tab's dot holds still, as the sidebar's does.
    if !s.looking {
        strip = strip.busy_indicator(|k| super::sidebar::status_dot(k.c.success.icon, "Working".into(), false));
    }
    // + after the arrows, as the strip's own trailing action would sit inside them.
    Some(row().key("tab-row").shrink().min_w(0.0).gap(TAB_GAP).center_y().child(scroll_arrows(strip.build(k), &ids, s.tab_aim.as_deref(), k)).children(plus))
}

/// Where a tab is in the tab bar's view: cut off at its start or end, by more than half or
/// less, or wholly in it.
#[derive(Clone, Copy, PartialEq)]
enum InView {
    Before,
    PartBefore,
    In,
    PartAfter,
    After,
}

/// The tab bar between ‹ and ›, while its tabs (`ids`) don't all fit: each scrolls it a tab
/// that way, and dims with none left there. Shortcuts go through the tabs; these show there
/// are more. `aim` is the tab an arrow last scrolled to: pressed again before it's there, an
/// arrow goes on from it.
fn scroll_arrows(strip: El, ids: &[String], aim: Option<&str>, k: &Kit) -> El {
    use rune::memo::{layout_size, window_layout, window_layout_with};
    let width = |key: &str| layout_size(key).map_or(0.0, |[w, _]| w);
    let (list, view) = (width("tabs-list"), width("tabs-scroll"));
    // Judged as if there were none: the arrows take their room from the view, and mustn't
    // be what makes the tabs not fit.
    let arrows = match (layout_size("tabs-back"), layout_size("tabs-forward")) {
        (Some([back, _]), Some([forward, _])) => back + forward + 2.0 * ARROW_GAP,
        _ => 0.0,
    };
    let bar = row().key("tab-bar").shrink().min_w(0.0).gap(ARROW_GAP).center_y();
    if list <= view + arrows + 0.5 {
        return bar.child(strip);
    }
    let [sx, _, sw, _] = window_layout("tabs-scroll").unwrap_or_default();
    let places: Vec<(&String, InView)> = ids
        .iter()
        .filter_map(|id| {
            let at = window_layout_with(&format!("t-{id}"), move |[x, _, w, _]| match () {
                _ if x + w / 2.0 < sx => InView::Before,
                _ if x < sx - 0.5 => InView::PartBefore,
                _ if x + w / 2.0 > sx + sw => InView::After,
                _ if x + w > sx + sw + 0.5 => InView::PartAfter,
                _ => InView::In,
            })?;
            Some((id, at))
        })
        .collect();
    // A tab that way: the first mostly out of view, as one nearly in would scroll the bar a
    // point or two; else one a little out.
    let first = |tabs: &mut dyn Iterator<Item = &(&String, InView)>, most: InView, part: InView| {
        let tabs: Vec<_> = tabs.filter(|(_, at)| *at == most || *at == part).collect();
        tabs.iter().find(|(_, at)| *at == most).or(tabs.first()).map(|(id, _)| (*id).clone())
    };
    let arriving = places.iter().position(|(id, _)| Some(id.as_str()) == aim).map(|i| (i, places[i].1));
    let back = match arriving {
        Some((i, InView::Before | InView::PartBefore)) => i.checked_sub(1).map(|i| places[i].0.clone()),
        _ => first(&mut places.iter().rev(), InView::Before, InView::PartBefore),
    };
    let forward = match arriving {
        Some((i, InView::After | InView::PartAfter)) => places.get(i + 1).map(|(id, _)| (*id).clone()),
        _ => first(&mut places.iter(), InView::After, InView::PartAfter),
    };
    let arrow = |key: &'static str, glyph: &'static rune::Icon, label: &str, to: Option<String>| {
        icons::button(glyph, label)
            .disabled(to.is_none())
            .on_press_tx(move |s: &mut Lemur, tx: &mut Tx| {
                if let Some(id) = &to {
                    tx.scroll_into_view_in("tabs-scroll", format!("t-{id}"));
                    s.tab_aim = Some(id.clone());
                }
            })
            .build(k)
            .key(key)
    };
    bar.child(arrow("tabs-back", &icons::CHEVRON_LEFT, "Scroll the tabs back", back))
        .child(strip)
        .child(arrow("tabs-forward", &icons::CHEVRON_RIGHT, "Scroll the tabs on", forward))
}

/// The panels' buttons, at the toolbar's end: each shows its panel, or hides it.
fn panel_buttons(s: &Lemur, k: &Kit) -> Vec<El> {
    let panel = |which: Right, glyph: &'static rune::Icon, name: &'static str, tip: &'static str| {
        let on = s.right() == Some(which);
        Button::new(name)
            .variant(if on { ButtonVariant::Outline } else { ButtonVariant::Ghost })
            .control_size(ControlSize::Small)
            .icon(glyph)
            .help(tip)
            .on_press(move |s: &mut Lemur| s.toggle_right(which))
            .build(k)
            .key(format!("toggle-{}", name.to_lowercase()))
    };
    vec![
        panel(Right::Notes, &icons::FILE_TEXT, "Notes", "Toggle notes"),
        panel(Right::Linear, &icons::TICKET, "Linear", "Toggle Linear"),
        panel(Right::Changes, &icons::FILE_DIFF, "Changes", "Toggle changes"),
        panel(Right::Links, &icons::LINK, "Links", "Toggle links"),
        panel(Right::Agents, &icons::BOT, "Agents", "Toggle agents (⌘⇧A)"),
    ]
}

/// Over the terminal: what runs in it, its name, and where.
fn pane_header(s: &Lemur, key: &Key, k: &Kit) -> Option<El> {
    let c = &k.c;
    let t = s.session_of(key)?;
    let what = t.agent().map_or("Shell", Agent::name);
    Some(
        row()
            .key("term-header")
            .w_full()
            .h(32.0)
            .px(12.0)
            .gap(8.0)
            .center_y()
            .border_sides([0.0, 0.0, 1.0, 0.0], c.border_weak)
            // On the terminal's own background, as if part of it.
            .fill(rune::color::hex(&super::palette::hex(s.palette().background)).unwrap_or(c.bg_base))
            .child(agent_icon(t.agent(), k))
            .child(text(what).font(k.ty.ui).color(c.text_muted))
            .child(text(t.label.clone()).key("header-name").font(k.ty.ui_medium).color(c.text_base).truncate())
            .child(text(t.path.clone()).font(k.ty.small).color(c.text_faint).truncate_head().grow(1.0)),
    )
}

/// The terminal on show in its frame, with its header, or a hint with none. The pane
/// reports its size, which the terminals fit.
fn main_pane(s: &Lemur, k: &Kit) -> El {
    let c = &k.c;
    let body = match s.active.as_ref().and_then(|key| s.terminals.get(key).map(|v| (key, v))) {
        Some((key, view)) => {
            let focused = s.keyboard_on_terminal() && s.peek.is_none();
            col()
                .key("terminal")
                .w_full()
                .h_full()
                .children(pane_header(s, key, k))
                .child(
                    col()
                        .key("term-area")
                        .w_full()
                        .grow(1.0)
                        .min_h(0.0)
                        .on_layout(|s: &mut Lemur, [_, _, w, h]| s.set_term_area(w, h))
                        .on_wheel(|s: &mut Lemur, e, tx| s.term_wheel(&e, tx))
                        .on_drag(|s: &mut Lemur, d, tx| s.term_drag(d, tx))
                        .on_secondary_press_local(|s: &mut Lemur, e, tx| s.term_secondary(e, tx))
                        .on_pointer_move_local(|s: &mut Lemur, e, tx| s.term_hover(e, tx))
                        .on_drop_files(|s: &mut Lemur, files, _| s.drop_files(files))
                        .cursor(rune::Cursor::Text)
                        .child(terminal::screen(view, s.metrics, focused, "main")),
                )
        }
        None => col()
            .key("empty")
            .w_full()
            .h_full()
            .center()
            .on_layout(|s: &mut Lemur, [_, _, w, h]| s.set_term_area(w, h.max(200.0) - 32.0))
            .child(
                text(if s.loaded { "Pick a session, or start one with +" } else { "Loading sessions…" })
                    .font(k.ty.body)
                    .color(c.text_faint),
            ),
    };
    let mut snacks = vec![];
    if let Some((message, kind)) = &s.status {
        snacks.push(snackbar::snackbar("status", message, *kind, |s: &mut Lemur| s.status = None, k));
    }
    if let Some((message, kind)) = s.active.as_ref().and_then(|key| s.terminals.get(key)).and_then(|v| v.status.as_ref()) {
        snacks.push(snackbar::snackbar(
            "terminal-status",
            message,
            *kind,
            |s: &mut Lemur| {
                if let Some(v) = s.active.clone().and_then(|key| s.terminals.get_mut(&key)) {
                    v.status = None;
                }
            },
            k,
        ));
    }
    let frame = col().key("frame").w_full().grow(1.0).min_h(0.0).child(body).child(snackbar::stack(snacks));
    col().key("pane").w_full().h_full().child(frame).children(panels::under_terminal(s, k)).child(status_bar(s, k))
}

/// Under the terminal: where the thread on show runs (its host, and its worktree's branch,
/// its lines added and removed, and its servers), and how many agents need you.
fn status_bar(s: &Lemur, k: &Kit) -> El {
    let c = &k.c;
    let item = |glyph: &'static rune::Icon, label: String| {
        row().gap(5.0).center_y().child(icon(glyph, 13.0).color(c.icon_muted)).child(text(label).font(k.ty.small).color(c.text_muted))
    };
    let mut bar = row()
        .key("status-bar")
        .w_full()
        .h(26.0)
        .px(12.0)
        .gap(14.0)
        .center_y()
        .fill(super::look::status_bar(k.dark))
        .border_sides([1.0, 0.0, 0.0, 0.0], c.border_weak);
    if let Some((host, _)) = &s.active
        && let Some(h) = s.cfg.host(host)
    {
        bar = bar.child(item(super::picker::host_icon(h), super::picker::host_label(h)));
    }
    if let Some((host, w)) = s.active_worktree() {
        let branch = match (&w.branch, &w.head) {
            (Some(branch), _) => branch.clone(),
            (None, Some(head)) => format!("detached at {}", &head[..head.len().min(8)]),
            (None, None) => crate::config::leaf(&w.path).to_string(),
        };
        bar = bar.child(item(&icons::GIT_BRANCH, branch)).children(s.changes_item(host, &w.path, k));
        let links = servers(s, host, w, k);
        if !links.is_empty() {
            bar = bar.child(row().key("servers").gap(4.0).center_y().children(links));
        }
    }
    bar = bar.child(el().grow(1.0));
    let needs = sessions(&s.snap).filter(|(_, t)| t.agent().is_some() && t.activity == Activity::NeedsInput).count();
    if needs > 0 {
        bar = bar.child(
            text(format!("{needs} need{} you", if needs == 1 { "s" } else { "" }))
                .key("needs-you")
                .font(k.ty.small_medium)
                .color(c.warning.fg),
        );
    }
    bar
}

/// The worktree's servers, by port: a link to each one that can be reached from here.
/// Another host's is reached by its tailnet name.
fn servers(s: &Lemur, host: &str, w: &crate::snapshot::Worktree, k: &Kit) -> Vec<El> {
    let c = &k.c;
    let Some(view) = s.snap.hosts.iter().find(|h| h.name == host) else { return vec![] };
    let Some(h) = s.cfg.host(host) else { return vec![] };
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
    machine::servers_in(&view.machine.servers, &w.path, &deeper)
        .into_iter()
        .map(|server| {
            let url = format!("http://{name}:{}", server.port);
            let reachable = local || !server.local_only();
            let tip = if reachable {
                format!("{url} · {}", server.name)
            } else {
                format!("{} on :{} listens only on {}'s own localhost", server.name, server.port, h.name)
            };
            let port = text(format!(":{}", server.port))
                .key(format!("port-{}", server.port))
                .font(k.ty.small)
                .color(if reachable { c.text_accent } else { c.text_faint })
                .help(tip);
            if reachable {
                port.cursor(rune::Cursor::Pointer).on_press_tx(move |_: &mut Lemur, tx: &mut Tx| {
                    let _ = tx.open_url(&url);
                })
            } else {
                port
            }
        })
        .collect()
}

/// A question over the window, as an alert.
fn dialog(d: &super::Dialog, k: &Kit, layer: &mut Vec<El>) {
    let c = &k.c;
    let mut body = col().gap(10.0).child(text(d.title.clone()).font(k.ty.body_medium).color(c.text_base).wrap()).child(
        text(d.description.clone()).font(k.ty.ui).color(c.text_muted).wrap(),
    );
    if let Some((label, checked)) = &d.check {
        let checked = *checked;
        body = body.child(
            Checkbox::new(checked)
                .label(label.clone())
                .on_toggle(|s: &mut Lemur| {
                    if let Some((_, c)) = s.dialog.as_mut().and_then(|d| d.check.as_mut()) {
                        *c = !*c;
                    }
                })
                .build(k)
                .key("dialog-check"),
        );
    }
    let cancel = Button::new("Cancel")
        .variant(ButtonVariant::Outline)
        .on_press(|s: &mut Lemur| s.dialog = None)
        .build(k)
        .key("dialog-cancel");
    let ok = Button::new(d.ok)
        .variant(ButtonVariant::Contrast)
        .on_press(|s: &mut Lemur| {
            if let Some(d) = s.dialog.take() {
                let checked = d.check.as_ref().is_some_and(|(_, c)| *c);
                (d.on_ok)(s, checked);
            }
        })
        .build(k)
        .key("dialog-ok")
        // Return agrees, as in a Mac's alerts.
        .default_button();
    let buttons = if d.ok == "OK" {
        row().gap(8.0).justify_end().child(ok)
    } else {
        row().gap(8.0).justify_end().child(cancel).child(ok)
    };
    let card = card::surface("dialog", 420.0, k).child(col().padding(20.0).gap(16.0).child(body).child(buttons));
    card::over_window("dialog", card, |s: &mut Lemur| s.dialog = None, k, layer);
}

/// The whole window.
pub fn window(s: &Lemur, cx: &mut Cx<Lemur>, k: &Kit, layer: &mut Vec<El>) -> El {
    let mut toolbar = Toolbar::new();
    if let Some(tabs) = tabs(s, k) {
        toolbar = toolbar.leading(tabs);
    }
    for b in panel_buttons(s, k) {
        toolbar = toolbar.trailing(b);
    }
    let split = SplitView::new(main_pane(s, k))
        .sidebar(&s.sidebar, |s: &mut Lemur| &mut s.sidebar, sidebar::sidebar(s, k))
        .inspector(&s.inspector, |s: &mut Lemur| &mut s.inspector, panels::right(s, k))
        .toolbar(toolbar)
        .titlebar_inset(cx.titlebar_inset())
        .frame_inset(FRAME)
        .sidebar_padding(0.0, FRAME)
        .sidebar_toggle(SidebarToggle::new().in_header().shortcut("⌘B"))
        .reduced_motion(cx.is_reduced_motion())
        .build(k);
    let mut root = el().key("window").w_full().h_full().child(split);
    if let Some(card) = peek::card(s, k) {
        root = root.child(card);
    }
    for over in panels::overlays(s, k, layer) {
        root = root.child(over);
    }
    if let Some(p) = &s.prompt {
        card::over_window("spotlight", prompt::card(s, p, k), |s: &mut Lemur| s.close_prompt(), k, layer);
    }
    if let Some(d) = &s.dialog {
        dialog(d, k, layer);
    }
    root
}

