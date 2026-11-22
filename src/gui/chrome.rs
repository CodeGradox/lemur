//! The window's frame: the sidebar beside the main pane, the panels' buttons in the
//! toolbar, the terminal in its frame with its header, the status bar under it, and
//! what shows over the window (the spotlight, a dialog, a peek, snackbars).

use super::card;
use super::prompt::thread_icon;
use super::snackbar;
use super::{Key, Lemur, Right, icons, panels, peek, prompt, sessions, sidebar, terminal};
use crate::config::Agent;
use crate::machine;
use crate::snapshot::Activity;
use rune::{Cx, El, Tx, col, el, icon, row, text};
use rune_kit::shell::{SplitView, Toolbar};
use rune_kit::{Button, ButtonVariant, Checkbox, ControlSize, Kit};

/// Points of window around the main pane, and between it and the sidebar: the sidebar's
/// resize grip, as wide, so the gap shows where to drag.
const FRAME: f32 = 6.0;

/// Another thread is on show: it lands at once.
pub(super) fn switched(tx: &mut Tx) {
    tx.instant();
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
            .child(thread_icon(t.agent(), t.color.as_deref(), k))
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
                        .child(terminal::screen(view, s.metrics, focused, "main"))
                        .children(super::term_find::bar(view, key, k)),
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

/// The worktree's servers, and those the thread on show started, by port: a link to each
/// one that can be reached from here. Another host's is reached by its tailnet name.
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
    let name = reach(s, h);
    // And those the thread on show started, in whichever worktree: its core calls its
    // terminal `t7`, lemur `agent/t7`.
    let terminal = s.active.as_ref().and_then(|(_, a)| a.rsplit('/').next()).unwrap_or_default();
    machine::servers_in(&view.machine.servers, &w.path, &deeper, terminal)
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

/// The name that reaches host `h`'s servers from here: `localhost` for this Mac's, else its
/// tailnet name, else its own.
pub(super) fn reach(s: &Lemur, h: &crate::config::Host) -> String {
    let tailnet = s.snap.hosts.iter().find(|v| v.name == h.name).and_then(|v| v.machine.tailnet.clone());
    match tailnet {
        _ if h.ssh == "local" => "localhost".to_string(),
        Some(tailnet) => tailnet,
        None => h.name.clone(),
    }
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
    // Past the traffic lights while the sidebar, which has it otherwise, is away.
    if !s.sidebar.is_open() {
        toolbar = toolbar.leading(sidebar::toggle(false, k));
    }
    for b in panel_buttons(s, k) {
        toolbar = toolbar.trailing(b);
    }
    let split = SplitView::new(main_pane(s, k))
        .sidebar(&s.sidebar, |s: &mut Lemur| &mut s.sidebar, sidebar::sidebar(s, k, cx.titlebar_inset()))
        .inspector(&s.inspector, |s: &mut Lemur| &mut s.inspector, panels::right(s, k))
        .toolbar(toolbar)
        .titlebar_inset(cx.titlebar_inset())
        .frame_inset(FRAME)
        .sidebar_padding(0.0, FRAME)
        .hide_sidebar_toggle()
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

