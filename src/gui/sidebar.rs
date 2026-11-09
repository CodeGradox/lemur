//! The sidebar: each project's worktrees and their threads, as rows to open, fold and
//! right-click; then Other, each host's threads outside every project; the hosts' load at
//! the foot.

use super::prompt::{Ask, agent_icon, new_menu, new_picked};
use super::silence::silence_label;
use super::{Key, Lemur, List, Right, icons, key, worktree_label};
use crate::config::{Agent, Base, COLORS, ClaudeThread};
use crate::snapshot::{Activity, Session, Worktree};
use rune::{Color, El, LoopSpec, PointerPress, PopupItem, PopupMenu, Tx, col, el, icon, row, text};
use rune_kit::shell::{Reorder, RowStatus, Sidebar, SidebarHeader, SidebarRow, SidebarSection};
use rune_kit::Kit;

/// Points the pointer travels before a press on a row becomes a drag.
const SLOP: f32 = 4.0;

/// What's being dragged: above the rest, following the pointer exactly, and opaque, so what
/// it passes doesn't show through.
fn lifted(e: El, k: &Kit) -> El {
    let (top, under) = (k.c.pressed, k.c.bg_deep);
    let mix = |t: f32, u: f32| t * top.a + u * (1.0 - top.a);
    let fill = Color { r: mix(top.r, under.r), g: mix(top.g, under.g), b: mix(top.b, under.b), a: 1.0 };
    k.elevate(e.raise().offset_motion(rune::Anim::INSTANT).fill(fill), &k.elevation.raised)
}

/// `e`, which drags what's keyed `id` (a worktree, a project) among others of other
/// heights, lifted while it's dragged.
fn lift(s: &Lemur, e: El, id: &str, k: &Kit) -> El {
    match s.moving.as_ref().filter(|m| m.0 == id) {
        Some(m) => lifted(e.offset(0.0, m.1), k),
        None => e,
    }
}

/// Notes where `e` is laid out as `id`'s place, to drop what's dragged by.
fn note_place(e: El, id: String) -> El {
    e.on_layout_tx(move |s: &mut Lemur, [_, y, _, h], tx: &mut Tx| {
        s.places.insert(id.clone(), (y, h));
        // Nothing shows it: the view needn't run again.
        tx.live_only();
    })
}

/// Pressed and dragged, `e` drags what's keyed `id`; let go, `drop` gets how far it went.
/// A press that doesn't move is still a click.
fn drags(e: El, id: String, drop: impl Fn(&mut Lemur, f32) + 'static) -> El {
    e.on_drag(move |s: &mut Lemur, d: &mut rune::Drag, tx: &mut Tx| match d.phase {
        rune::Phase::Down => {}
        rune::Phase::Move => {
            if s.moving.is_none() && d.dy.abs() < SLOP {
                return;
            }
            tx.animated();
            s.moving = Some((id.clone(), d.dy));
        }
        rune::Phase::Up => {
            if s.moving.take().is_some() && !d.cancelled() {
                tx.instant();
                drop(s, d.dy);
            }
        }
    })
}

/// A worktree's folder colour, from its name in the config.
fn worktree_color(base: &Base, worktree: &str) -> Option<Color> {
    let name = base.worktree_colors.get(worktree)?;
    let (_, hex) = COLORS.iter().find(|(n, _)| n == name)?;
    rune::color::hex(&format!("#{hex:06x}"))
}

/// `blue` as `Blue`, for menus.
fn title_case(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

pub fn age(secs: u64) -> String {
    match secs {
        0..60 => "now".into(),
        60..3600 => format!("{}m", secs / 60),
        3600..86400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

impl Lemur {
    /// A thread's dot: its colour, what it says, and whether it breathes (while working).
    pub fn activity(&self, key: &Key, s: &Session, k: &Kit) -> (Color, String, bool) {
        let c = &k.c;
        match s.activity {
            Activity::NeedsInput => (
                c.warning.icon,
                match &s.waiting_for {
                    Some(what) => format!("Needs input: {what}"),
                    None => "Needs input".into(),
                },
                false,
            ),
            // Still while Lemur is in the background: breathing, it would draw every frame.
            Activity::Working => (c.success.icon, "Working".into(), self.looking),
            _ if self.done.contains(key) => (c.info.icon, "Done".into(), false),
            Activity::Idle => (c.text_muted.alpha(0.5), "Idle".into(), false),
            Activity::Unknown => (c.text_muted.alpha(0.5), "No Claude found".into(), false),
        }
    }
}

/// A thread's status dot; it breathes while its agent works.
pub fn status_dot(color: Color, tip: String, breathes: bool) -> El {
    let d = el().size(8.0, 8.0).radius(4.0).fill(color).help(tip);
    if breathes { d.looping(LoopSpec::opacity(1.6, 0.0, 0.35, 1.0)) } else { d }
}


/// What a right click on a thread offers, and what it runs.
fn thread_menu(s: &Session) -> PopupMenu {
    let mut menu = PopupMenu::new().item(PopupItem::new("rename", "Rename…")).item(PopupItem::new("peek", "Peek"));
    // Only an agent has anything to notify of.
    if s.agent().is_some() {
        menu = menu.item(PopupItem::new("silence", silence_label(s.silent)));
    }
    menu.item(if s.away { PopupItem::new("bring-back", "Bring back") } else { PopupItem::new("put-away", "Put away") })
}

fn thread_picked(s: &mut Lemur, picked: &str, key: Key) {
    let silent = s.session_of(&key).is_some_and(|t| t.silent);
    match picked {
        "rename" => s.ask_name(Ask::Rename { host: key.0, session: key.1 }),
        "peek" => s.peek(key),
        "silence" => s.set_silent(key, !silent),
        "bring-back" => s.bring_back(&key),
        "put-away" => s.put_away(key),
        _ => {}
    }
}

/// A thread's row: its agent's icon, its name, then the issue pinned to it, what helps it
/// and its age; its dot, a × under the pointer.
fn thread_row(s: &Lemur, host: &str, t: &Session, place: Option<(usize, usize, &List)>, k: &Kit) -> El {
    let c = &k.c;
    let key = key(host, t);
    let (dot, tip, breathes) = s.activity(&key, t, k);
    let mut said = vec![];
    if let Some(issue) = &t.issue {
        said.push(issue.clone());
    }
    if !t.helpers.is_empty() {
        said.push(format!("{} helping", t.helpers.len()));
    }
    said.push(age(t.idle_secs));
    let mut r = SidebarRow::new(t.label.clone())
        .avatar(agent_icon(t.agent(), k))
        .depth(1)
        .detail(said.join(" · "))
        .status(RowStatus::Other(status_dot(dot, tip, breathes)))
        .selected(s.active.as_ref() == Some(&key));
    if t.silent {
        r = r.trailing(icon(&icons::BELL_OFF, 12.0).color(c.icon_faint).help("Makes no notifications"));
    }
    // The `index`th of `count` in `list`: dragged, it takes another place there.
    if let Some((index, count, list)) = place {
        let state = if s.row_drag_list == *list { s.row_drag } else { Reorder::new().gap(1.0) };
        let list = list.clone();
        r = r.reorder(&state, index, count, |s: &mut Lemur| &mut s.row_drag, move |s, from, to| s.reorder(list.clone(), from, to));
    }
    let (open, close, rename, menu) = (key.clone(), key.clone(), key.clone(), key.clone());
    let picked_menu = thread_menu(t);
    let hovered = place.map(|(.., list)| list.clone());
    r.on_select(move |s: &mut Lemur, _| s.open(open.0.clone(), open.1.clone()))
        .on_close(move |s: &mut Lemur, _| s.confirm_close_of(close.clone()))
        .on_double_click(move |s: &mut Lemur, _| s.ask_name(Ask::Rename { host: rename.0.clone(), session: rename.1.clone() }))
        .build(k)
        .key(format!("row-{}-{}", key.0, key.1))
        .opacity(if t.away { 0.6 } else { 1.0 })
        // The lists share one drag: it goes to the one under the pointer as it starts.
        .on_hover_change(move |s: &mut Lemur, inside, tx: &mut Tx| {
            if let Some(list) = hovered.clone().filter(|_| inside && s.row_drag.dragging().is_none()) {
                s.row_drag_list = list;
                tx.live_only();
            }
        })
        .on_secondary_press(move |_: &mut Lemur, press: PointerPress, tx: &mut Tx| {
            let key = menu.clone();
            tx.popup_menu(picked_menu.clone(), press, move |s: &mut Lemur, picked, _| {
                if let Some(p) = picked {
                    thread_picked(s, p, key);
                }
            });
        })
}

/// A thread whose session is gone, dimmed: clicking it or its ▶ resumes it, and its
/// right-click menu can forget it instead.
fn gone_row(t: &ClaudeThread, k: &Kit) -> El {
    let (resume, forget, row_resume) = (t.clone(), t.clone(), t.clone());
    let play = icons::button(&icons::PLAY, "Its session is gone: resume Claude")
        .on_press(move |s: &mut Lemur| s.resume(vec![resume.clone()]))
        .build(k);
    SidebarRow::new(t.shown())
        .avatar(agent_icon(Some(Agent::Claude), k))
        .depth(1)
        .trailing(play)
        .on_select(move |s: &mut Lemur, _| s.resume(vec![row_resume.clone()]))
        .build(k)
        .key(format!("gone-{}-{}", t.host, t.session))
        .opacity(0.5)
        .on_secondary_press(move |_: &mut Lemur, press: PointerPress, tx: &mut Tx| {
            let t = forget.clone();
            let menu = PopupMenu::new().item(PopupItem::new("resume", "Resume")).item(PopupItem::new("forget", "Forget"));
            tx.popup_menu(menu, press, move |s: &mut Lemur, picked, _| match picked {
                Some("resume") => s.resume(vec![t]),
                Some("forget") => s.forget_threads(&t.host, std::slice::from_ref(&t.session)),
                _ => {}
            });
        })
}

/// A list's threads, then those put away under a row that opens them, then those gone.
fn threads(s: &Lemur, host: &str, worktree: Option<&str>, sessions: &[Session], gone: &[ClaudeThread], k: &Kit) -> Vec<El> {
    let (away, shown): (Vec<&Session>, Vec<&Session>) = sessions.iter().partition(|t| t.away);
    // Those put away come last in a list, so the rest keep their places for dragging.
    let count = shown.len();
    let list: List = (host.to_string(), worktree.map(str::to_string));
    let mut rows: Vec<El> = shown.into_iter().enumerate().map(|(i, t)| thread_row(s, host, t, Some((i, count, &list)), k)).collect();
    if !away.is_empty() {
        let open = s.away_open.contains(&list);
        let toggle = list.clone();
        rows.push(
            SidebarRow::new(format!("Put away ({})", away.len()))
                .depth(1)
                .on_select(move |s: &mut Lemur, _| s.toggle_away_list(toggle.clone()))
                .build(k)
                .key(format!("away-{host}-{}", worktree.unwrap_or_default()))
                .opacity(0.7),
        );
        if open {
            rows.extend(away.into_iter().map(|t| thread_row(s, host, t, None, k)));
        }
    }
    rows.extend(gone.iter().map(|t| gone_row(t, k)));
    rows
}

/// What a right click on a worktree, or its +, offers: what starts there, its notes; its
/// colour, hiding it, and removing a linked one.
fn worktree_menu(base: &Base, w: &Worktree, hidden: bool, full: bool) -> PopupMenu {
    let mut menu = new_menu().separator().item(PopupItem::new("notes", "Notes").symbol("doc.text"));
    if !full {
        return menu;
    }
    let current = base.worktree_colors.get(&w.path).cloned();
    let mut colors = PopupMenu::new();
    for (name, _) in COLORS {
        colors = colors.item(PopupItem::new(format!("color:{name}"), title_case(name)).checked(current.as_deref() == Some(name)));
    }
    colors = colors.separator().item(PopupItem::new("color:", "No color").checked(current.is_none()));
    menu = menu.separator().submenu("Color", colors).item(PopupItem::new("hide", if hidden { "Unhide" } else { "Hide" }));
    // The main checkout and a plain folder are the project itself.
    if !w.main {
        menu = menu.separator().item(PopupItem::new("remove", "Remove worktree…"));
    }
    menu
}

fn worktree_picked(s: &mut Lemur, picked: &str, host: &str, base: usize, path: &str, label: &str) {
    if new_picked(s, picked, host, path, Some(base)) {
        return;
    }
    let hidden = s.cfg.bases[base].hidden_worktrees.iter().any(|p| p == path);
    match picked {
        "notes" => s.open_notes(host, path, label),
        "hide" => s.set_worktree_hidden(base, path, !hidden),
        "remove" => {
            let removal = s.removal(base, path.to_string(), label.to_string());
            s.confirm_remove_worktree(removal, None);
        }
        color if color.starts_with("color:") => {
            let name = &color["color:".len()..];
            s.set_worktree_color(base, path, (!name.is_empty()).then_some(name));
        }
        _ => {}
    }
}

/// A worktree with its threads: its folder or branch, folded to how many run in it.
fn worktree_rows(s: &Lemur, host: &str, index: usize, w: &Worktree, k: &Kit) -> Vec<El> {
    let c = &k.c;
    let base = &s.cfg.bases[index];
    let label = worktree_label(w);
    let folded = base.collapsed_worktrees.contains(&w.path);
    let hidden = base.hidden_worktrees.contains(&w.path);
    let color = worktree_color(base, &w.path).unwrap_or(c.icon_muted);
    let glyph = if w.main { &icons::FOLDER } else { &icons::GIT_BRANCH };
    let lead = row().size(16.0, 16.0).center().child(icon(glyph, 14.0).color(color));
    let path = w.path.clone();
    let (host_s, plus_path, plus_label) = (host.to_string(), w.path.clone(), label.clone());
    let plus_menu = worktree_menu(base, w, hidden, false);
    let plus = icons::button(&icons::PLUS, "New terminal or Claude here, or notes")
        .on_press_tx(move |_: &mut Lemur, tx: &mut Tx| {
            let (host, path, label) = (host_s.clone(), plus_path.clone(), plus_label.clone());
            tx.popup_menu(plus_menu.clone(), format!("plus-{path}"), move |s: &mut Lemur, picked, _| {
                if let Some(p) = picked {
                    worktree_picked(s, p, &host, index, &path, &label);
                }
            });
        })
        .build(k)
        .key(format!("plus-{}", w.path));
    let mut r = SidebarRow::new(label.clone()).avatar(lead).trailing(plus);
    if folded && !w.sessions.is_empty() {
        let agents = w.sessions.iter().filter(|t| t.agent().is_some()).count();
        let shells = w.sessions.len() - agents;
        let say = |n: usize, one: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") };
        let running = match (agents, shells) {
            (0, sh) => say(sh, "shell"),
            (a, 0) => say(a, "agent"),
            (a, sh) => format!("{}, {}", say(a, "agent"), say(sh, "shell")),
        };
        r = r.detail(running);
        if w.sessions.iter().any(|t| t.activity == Activity::NeedsInput) {
            r = r.status(RowStatus::Attention);
        }
    }
    let menu = worktree_menu(base, w, hidden, true);
    let (host_m, path_m, label_m) = (host.to_string(), w.path.clone(), label.clone());
    let id = format!("wt-{}", w.path);
    let drag_path = w.path.clone();
    let built = r.on_select(move |s: &mut Lemur, _| s.toggle_worktree(index, &path)).build(k);
    // Dragged onto another worktree of its project, it moves there; its threads follow.
    let built = drags(note_place(lift(s, built, &id, k), id.clone()), id, move |s, dy| s.drop_worktree(index, &drag_path, dy));
    let mut rows = vec![
        built
            .key(format!("wt-{}", w.path))
            .expanded(!folded)
            .on_secondary_press(move |_: &mut Lemur, press: PointerPress, tx: &mut Tx| {
                let (host, path, label) = (host_m.clone(), path_m.clone(), label_m.clone());
                tx.popup_menu(menu.clone(), press, move |s: &mut Lemur, picked, _| {
                    if let Some(p) = picked {
                        worktree_picked(s, p, &host, index, &path, &label);
                    }
                });
            }),
    ];
    if !folded {
        rows.extend(threads(s, host, Some(&w.path), &w.sessions, &w.gone, k));
    }
    rows
}

/// A project's menu: resuming its gone threads, and removing it.
fn project_menu(s: &Lemur, index: usize) -> PopupMenu {
    let gone = s.gone_in(index);
    let mut menu = PopupMenu::new();
    if !gone.is_empty() {
        menu = menu.item(PopupItem::new("resume-all", format!("Resume all threads ({})", gone.len()))).separator();
    }
    menu.item(PopupItem::new("remove-project", "Remove project…"))
}

fn project_picked(s: &mut Lemur, picked: &str, index: usize) {
    match picked {
        "resume-all" => {
            let gone = s.gone_in(index);
            s.resume(gone);
        }
        "remove-project" => {
            if let Some(project) = s.cfg.bases.get(index).cloned() {
                s.confirm_remove_project(project);
            }
        }
        _ => {}
    }
}

/// A host's row in Other: its name, and + to start a terminal or an agent in its home.
fn host_row(s: &Lemur, name: &str, home: &str, k: &Kit) -> El {
    let host = s.cfg.host(name);
    let glyph = host.map_or(&icons::SERVER, super::picker::host_icon);
    let mut r = SidebarRow::new(host.map_or_else(|| name.to_string(), super::picker::host_label)).icon(glyph);
    // In its home, once its script has said where that is.
    if !home.is_empty() {
        let (host, dir) = (name.to_string(), home.to_string());
        let plus = icons::button(&icons::PLUS, "New terminal or Claude here")
            .on_press_tx(move |_: &mut Lemur, tx: &mut Tx| {
                let (host, dir) = (host.clone(), dir.clone());
                tx.popup_menu(new_menu(), format!("host-plus-{host}"), move |s: &mut Lemur, picked, _| {
                    if let Some(p) = picked {
                        new_picked(s, p, &host, &dir, None);
                    }
                });
            })
            .build(k)
            .key(format!("host-plus-{name}"));
        r = r.trailing(plus);
    }
    r.build(k).key(format!("host-{name}"))
}

pub fn sidebar(s: &Lemur, k: &Kit) -> El {
    let c = &k.c;
    let header = row()
        .key("sidebar-head")
        .w_full()
        .h(28.0)
        .px(4.0)
        .gap(2.0)
        .center_y()
        .child(text("Lemur").font(k.ty.ui_medium).color(c.text_base).grow(1.0))
        .child(icons::button(&icons::SEARCH, "Go to thread (⌘P)").on_press(|s: &mut Lemur| s.find_thread()).build(k).key("find-thread"))
        .child(icons::button(&icons::PLUS, "Add project (⌘O)").on_press(|s: &mut Lemur| s.open_picker()).build(k).key("add-folder"))
        .child(icons::button(&icons::REFRESH, "Refresh").on_press(|s: &mut Lemur| s.refresh()).build(k).key("refresh"));
    let mut sb = Sidebar::new().label("Threads").row_gap(1.0).header(header);
    if !s.loaded {
        sb = sb.child(SidebarRow::new("Loading sessions…").build(k).key("loading").opacity(0.6));
    }
    // Projects first, in the config's order whatever their host; then each host's threads
    // outside them, or why it couldn't be reached.
    let mut projects: Vec<_> = s.snap.hosts.iter().flat_map(|h| h.bases.iter().map(move |b| (h, b))).collect();
    projects.sort_by_key(|(_, b)| b.index);
    for (h, b) in projects {
        let index = b.index;
        let base = &s.cfg.bases[index];
        let mut rows = vec![];
        // A folder without git has no worktrees to add.
        if !b.plain {
            rows.push(
                SidebarRow::new("New worktree")
                    .icon(&icons::PLUS)
                    .on_select(move |s: &mut Lemur, _| s.ask_name(Ask::NewWorktree { base: index, agent: Some(Agent::Claude) }))
                    .build(k)
                    .key(format!("new-worktree-{index}")),
            );
        }
        if let Some(e) = &b.error {
            rows.push(SidebarRow::new(e.clone()).icon(&icons::TRIANGLE_ALERT).build(k).key(format!("error-{index}")).opacity(0.7));
        }
        let mut hidden = vec![];
        for w in &b.worktrees {
            if base.hidden_worktrees.contains(&w.path) {
                hidden.extend(worktree_rows(s, &h.name, index, w, k));
            } else {
                rows.extend(worktree_rows(s, &h.name, index, w, k));
            }
        }
        // Under the rest, in their order.
        if !hidden.is_empty() {
            rows.push(
                SidebarRow::new("Hidden")
                    .trailing(super::meter::chevron(!base.hidden_open, k))
                    .on_select(move |s: &mut Lemur, _| s.toggle_hidden(index))
                    .build(k)
                    .key(format!("hidden-{index}"))
                    .opacity(0.8),
            );
            if base.hidden_open {
                rows.extend(hidden);
            }
        }
        let menu_button = icons::button(&icons::ELLIPSIS, "Project")
            .on_press_tx(move |s: &mut Lemur, tx: &mut Tx| {
                tx.popup_menu(project_menu(s, index), format!("project-menu-{index}"), move |s: &mut Lemur, picked, _| {
                    if let Some(p) = picked {
                        project_picked(s, p, index);
                    }
                });
            })
            .build(k)
            .key(format!("project-menu-{index}"));
        // A section of the kit's, but dragged by its header to another place among the
        // projects.
        let title = format!("{} · {}", base.name(), h.name);
        let id = format!("project-{index}");
        let header = SidebarHeader::new(title.clone())
            .collapsed(base.collapsed)
            .on_toggle(move |s: &mut Lemur, _| s.toggle_project(index))
            .action(menu_button)
            .build(k);
        let header = drags(note_place(header, id.clone()), id.clone(), move |s, dy| s.drop_project(index, dy));
        let mut section = col().key(id.clone()).role("group").label(title).w_full().gap(2.0).motion(k.motion.disclosure).child(header);
        if !base.collapsed {
            section = section.children(rows);
        }
        sb = sb.child(lift(s, section, &id, k));
    }
    // Then one section for every host, each with its threads outside every project.
    let mut other = vec![];
    for h in &s.snap.hosts {
        if let Some(e) = &h.error {
            let mut rows = vec![SidebarRow::new(e.clone()).icon(&icons::TRIANGLE_ALERT).build(k).key(format!("host-error-{}", h.name)).opacity(0.7)];
            // herdr-core needs you to add it: in a terminal, as Herdr's.
            if let Some(host) = s.cfg.host(&h.name).filter(|_| h.needs_you).cloned() {
                rows.push(
                    SidebarRow::new(format!("Set up {}…", h.name))
                        .icon(&icons::SQUARE_TERMINAL)
                        .on_select(move |s: &mut Lemur, _| s.set_up_host(host.clone()))
                        .build(k)
                        .key(format!("set-up-{}", h.name)),
                );
            }
            sb = sb.section(SidebarSection::new(h.name.clone()).key(format!("host-section-{}", h.name)).rows(rows));
        } else {
            other.push(host_row(s, &h.name, &h.home, k));
            other.extend(threads(s, &h.name, None, &h.other, &h.gone, k));
        }
    }
    if !other.is_empty() {
        sb = sb.section(SidebarSection::new("Other").key("other").rows(other));
    }
    let foot = col().key("sidebar-foot").w_full().children(super::panels::sidebar_foot(s, k)).child(s.machines(k));
    sb.footer(foot).build(k).fill(super::look::sidebar(k.dark))
}

impl Lemur {
    /// Each host's CPU and memory, as bars at the foot of the sidebar; none for hosts
    /// without /proc to read them from. A click on a host's name minimizes it to that line.
    fn machines(&self, k: &Kit) -> El {
        let c = &k.c;
        let mut hosts: Vec<_> = self.loads().iter().collect();
        hosts.sort_by_key(|(name, _)| self.cfg.hosts.iter().position(|h| h.name == **name).unwrap_or(usize::MAX));
        let gib = |kb: u64| format!("{:.1}", kb as f64 / 1_048_576.0);
        col().key("machines").w_full().px(8.0).pt(8.0).pb(10.0).gap(8.0).children(hosts.into_iter().map(|(name, (load, cpu))| {
            let percent = |p: Option<f32>| p.map_or("…".to_string(), |p| format!("{p:.0}%"));
            let swap = if load.swap_total_kb > 0 { format!(" · swap {} of {} GiB", gib(load.swap_used_kb), gib(load.swap_total_kb)) } else { String::new() };
            let tip = format!(
                "{name}: CPU {}{} · load {} · memory {} of {} GiB{swap}",
                percent(*cpu),
                load.cores.map_or(String::new(), |n| format!(" of {n} cores")),
                load.load.join(" "),
                gib(load.mem_used_kb),
                gib(load.mem_total_kb),
            );
            // A click on its name minimizes it to that line, the bars' numbers in the load's place.
            let minimized = self.cfg.minimized_hosts.contains(name);
            let right = if minimized {
                Some(format!("CPU {} · RAM {}", percent(*cpu), percent(load.memory())))
            } else {
                load.load.first().map(|l| format!("load {l}"))
            };
            let toggle = name.clone();
            let header = super::meter::minimizable_header(format!("load-header-{name}"), k)
                .on_press(move |s: &mut Lemur| s.toggle_host_minimized(&toggle))
                .child(icon(&icons::SERVER, 12.0).color(c.icon_faint))
                .child(text(name.clone()).font(k.ty.ui_medium).color(c.text_base).truncate().grow(1.0))
                .children(right.map(|r| text(r).font(k.ty.small).color(c.text_muted)))
                .child(super::meter::chevron(minimized, k));
            let machine = col().key(format!("machine-{name}")).gap(6.0).help(tip).child(header);
            if minimized {
                return machine;
            }
            let ram = format!("{} / {} GiB", gib(load.mem_used_kb), gib(load.mem_total_kb));
            machine
                .child(super::meter::bar("CPU".into(), *cpu, percent(*cpu), (28.0, 92.0), k).key(format!("cpu-{name}")))
                .child(super::meter::bar("RAM".into(), load.memory(), ram, (28.0, 92.0), k).key(format!("mem-{name}")))
        }))
    }

    /// Opens the notes panel on a worktree's notes.
    pub fn open_notes(&mut self, host: &str, worktree: &str, label: &str) {
        self.show_notes(host, worktree, label);
        self.right_last = Right::Notes;
        self.inspector.set_open(true);
    }

    /// Worktree `path` of project `index` was dragged `dy` points: it takes the place of the
    /// worktree under the pointer, counting those that aren't hidden.
    fn drop_worktree(&mut self, index: usize, path: &str, dy: f32) {
        let base = &self.cfg.bases[index];
        let shown: Vec<String> = self
            .snap
            .hosts
            .iter()
            .flat_map(|h| h.bases.iter())
            .filter(|b| b.index == index)
            .flat_map(|b| b.worktrees.iter())
            .filter(|w| !base.hidden_worktrees.contains(&w.path))
            .map(|w| w.path.clone())
            .collect();
        let Some(from) = shown.iter().position(|p| p == path) else { return };
        let ids: Vec<String> = shown.iter().map(|p| format!("wt-{p}")).collect();
        if let Some(to) = self.place_at(&ids[from], &ids, dy) {
            self.reorder_worktrees(index, from, to);
        }
    }

    /// Project `index` was dragged `dy` points by its header: it takes the place of the
    /// project under the pointer.
    fn drop_project(&mut self, index: usize, dy: f32) {
        let ids: Vec<String> = (0..self.cfg.bases.len()).map(|i| format!("project-{i}")).collect();
        if let Some(to) = self.place_at(&ids[index], &ids, dy) {
            self.reorder_projects(index, to);
        }
    }

    /// Where what's at `id`'s place lands among `ids`'s, dragged `dy` points: the last of
    /// them that starts above its middle.
    fn place_at(&self, id: &str, ids: &[String], dy: f32) -> Option<usize> {
        let &(y, h) = self.places.get(id)?;
        let at = y + h / 2.0 + dy;
        ids.iter().rposition(|i| self.places.get(i).is_some_and(|(top, _)| *top <= at)).or(Some(0))
    }

    /// Notes the list `list`'s order as the sidebar shows it.
    pub fn list_of(&self, key: &Key) -> List {
        (key.0.clone(), self.project_of(&key.0, &key.1).map(|(_, w)| w.path.clone()))
    }
}
