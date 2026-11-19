//! The sidebar: each project's threads, from all its worktrees, in folders of yours or
//! none (`folders`), as rows to open, fold, drag and right-click; then Other, each host's
//! threads outside every project; the hosts' load at the foot.

use super::prompt::{Ask, agent_glyph, agent_icon, new_menu, new_picked};
use super::silence::silence_label;
use super::{Key, Lemur, List, icons, key, worktree_label};
use crate::config::{Agent, Base, COLORS, ClaudeThread, Folder};
use crate::snapshot::{Activity, BaseView, Session};
use rune::{Color, El, LoopSpec, PointerPress, PopupItem, PopupMenu, Tx, col, el, icon, row, text};
use rune_kit::shell::{Reorder, RowStatus, Sidebar, SidebarRow, TOOLBAR_HEIGHT};
use rune_kit::Kit;

/// Points the pointer travels before a press on a row becomes a drag.
const SLOP: f32 = 4.0;
/// Points between the traffic lights and the sidebar's title, as Herdr leaves.
const PAST_LIGHTS: f32 = 20.0;
/// Points between the title row's last button and the sidebar's edge: the toggle lines up
/// with the rows' buttons under it, as Herdr's does.
const TITLE_END: f32 = 16.0;

/// The kit a sidebar row is built with: rune-kit mutes the name of a row that isn't picked,
/// and the sidebar's rows have theirs in the text's own colour, as v2 had them.
fn named(k: &Kit) -> Kit {
    let mut k = k.clone();
    k.c.text_muted = k.c.text_base;
    k
}

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
                // Not a drag yet, so nothing changed. Rune makes a drag's moves instant, and
                // the click this may still be would fold or open without its animation.
                tx.live_only();
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

/// One of `COLORS`, by its name.
fn color_named(name: &str) -> Option<Color> {
    let (_, hex) = COLORS.iter().find(|(n, _)| *n == name)?;
    rune::color::hex(&format!("#{hex:06x}"))
}

/// What sets a project apart from the next: a line, as Slack's and Plane's sidebars have.
fn divider(key: String, k: &Kit) -> El {
    el().key(key).w_full().h(1.0).mt(6.0).mb(4.0).fill(k.c.border_base).a11y_hidden()
}

/// A project's header, as Linear's and ClickUp's: a mark in its colour (gray unless one is
/// picked) with its initial, its name and its host, then its +, its menu and the arrow the
/// meters at the foot have, which turns as it folds. A press folds it.
fn project_header(base: &Base, host: &str, index: usize, plus: El, menu: El, k: &Kit) -> El {
    let c = &k.c;
    let name = base.name();
    let color = base.color.as_deref().and_then(color_named).or_else(|| color_named("gray")).unwrap_or(c.icon_muted);
    let initial: String = name.chars().next().map(|ch| ch.to_uppercase().collect()).unwrap_or_default();
    let mark = row().key("project-mark").size(18.0, 18.0).radius(5.0).fill(color).center().child(text(initial).font(k.ty.text_xsmall_medium).color(Color::WHITE));
    let title = row()
        .gap(6.0)
        .center_y()
        .shrink()
        .min_w(0.0)
        .child(mark)
        .child(text(name.clone()).font(k.ty.ui_medium).color(c.text_base).truncate().shrink())
        .child(text(format!("· {host}")).font(k.ty.small).color(c.text_muted))
        .a11y_hidden();
    let header = row()
        .key("section-header")
        .label(format!("{name} · {host}"))
        .w_full()
        .h(30.0)
        // Its end as the rows' under it, so its arrow lines up with theirs.
        .pl(6.0)
        .pr(8.0)
        .gap(6.0)
        .center_y()
        .radius(k.radius.md)
        .child(title)
        .child(el().grow(1.0).a11y_hidden())
        .child(plus)
        .child(menu)
        .child(super::meter::chevron(base.collapsed, k))
        .on_press(move |s: &mut Lemur| s.toggle_project(index));
    // Pointer only, as the kit's section headers: in the list's focus group the arrows
    // step through rows.
    k.interactive(header).role("button").expanded(!base.collapsed).hover_motion(rune::Anim::INSTANT, rune::Anim::INSTANT).focusable(false)
}

/// A group that isn't a project (Other, a host that can't be reached): its name as a
/// project's, over its rows.
fn group(key: String, title: &str, rows: Vec<El>, k: &Kit) -> El {
    let header = row()
        .key("section-header")
        .role("heading")
        .label(title.to_owned())
        .w_full()
        .h(30.0)
        .px(8.0)
        .center_y()
        .child(text(title.to_owned()).font(k.ty.ui_medium).color(k.c.text_base).truncate().a11y_hidden());
    col().key(key).role("group").label(title.to_owned()).w_full().gap(2.0).child(header).children(rows)
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
    /// A thread's dot: its colour, what it says, and whether it breathes (while working, or
    /// while its shell runs a command).
    pub fn activity(&self, key: &Key, s: &Session, k: &Kit) -> (Color, String, bool) {
        let c = &k.c;
        match s.doing() {
            Activity::NeedsInput => (
                c.warning.icon,
                match &s.waiting_for {
                    Some(what) => format!("Needs input: {what}"),
                    None => "Needs input".into(),
                },
                false,
            ),
            // Still while Lemur is in the background: breathing, it would draw every frame.
            Activity::Working => {
                let says = s.shell_runs().map_or_else(|| "Working".into(), |command| format!("Running {command}"));
                (c.success.icon, says, self.looking)
            }
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
/// A thread's right-click: renaming it, peeking at it, silencing it, putting it away; in a
/// project, moving it to one of `folders`, or a new one.
fn thread_menu(s: &Session, folders: Option<&[Folder]>) -> PopupMenu {
    let mut menu = PopupMenu::new()
        .item(PopupItem::new("rename", "Rename…"))
        .item(PopupItem::new("peek", "Peek"))
        .item(PopupItem::new("silence", silence_label(s.silent)))
        .item(if s.away { PopupItem::new("bring-back", "Bring back") } else { PopupItem::new("put-away", "Put away") });
    if let Some(folders) = folders {
        let filed = folders.iter().any(|f| s.folder.as_deref() == Some(f.id.as_str()));
        let mut to = PopupMenu::new();
        for f in folders {
            to = to.item(PopupItem::new(format!("folder:{}", f.id), f.name.clone()).checked(s.folder.as_deref() == Some(f.id.as_str())));
        }
        if !folders.is_empty() {
            to = to.separator();
        }
        to = to.item(PopupItem::new("folder:", "No folder").checked(!filed)).item(PopupItem::new("new-folder", "New folder…"));
        menu = menu.separator().submenu("Move to folder", to);
    }
    menu
}

fn thread_picked(s: &mut Lemur, picked: &str, key: Key) {
    let silent = s.session_of(&key).is_some_and(|t| t.silent);
    match picked {
        "rename" => s.ask_name(Ask::Rename { host: key.0, session: key.1 }),
        "peek" => s.peek(key),
        "silence" => s.set_silent(key, !silent),
        "bring-back" => s.bring_back(&key),
        "put-away" => s.put_away(key),
        "new-folder" => {
            if let Some((base, _)) = s.project_of(&key.0, &key.1) {
                s.ask_name(Ask::NewFolder { base, thread: Some(key) });
            }
        }
        folder if folder.starts_with("folder:") => {
            let id = &folder["folder:".len()..];
            s.move_to_folder(&key, Some(id).filter(|id| !id.is_empty()));
        }
        _ => {}
    }
}

/// A thread's row, `depth` in: its agent's icon, its name, then the issue pinned to it, what
/// helps it and its age; its dot, a × under the pointer.
fn thread_row(s: &Lemur, host: &str, t: &Session, place: Option<(usize, usize, &List)>, depth: u8, k: &Kit) -> El {
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
        .depth(depth)
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
    let folders = s.project_of(host, &t.name).map(|(index, _)| s.cfg.bases[index].folders.as_slice());
    let picked_menu = thread_menu(t, folders);
    let hovered = place.map(|(.., list)| list.clone());
    r.on_select(move |s: &mut Lemur, _| s.open(open.0.clone(), open.1.clone()))
        .on_close(move |s: &mut Lemur, _| s.confirm_close_of(close.clone()))
        .on_double_click(move |s: &mut Lemur, _| s.ask_name(Ask::Rename { host: rename.0.clone(), session: rename.1.clone() }))
        .build(&named(k))
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

/// A thread whose session is gone, dimmed, `depth` in: clicking it or its ▶ resumes it, and
/// its right-click menu can forget it instead.
fn gone_row(t: &ClaudeThread, depth: u8, k: &Kit) -> El {
    let (resume, forget, row_resume) = (t.clone(), t.clone(), t.clone());
    let play = icons::button(&icons::PLAY, "Its session is gone: resume Claude")
        .on_press(move |s: &mut Lemur| s.resume(vec![resume.clone()]))
        .build(k);
    SidebarRow::new(t.shown())
        .avatar(agent_icon(Some(Agent::Claude), k))
        .depth(depth)
        .trailing(play)
        .on_select(move |s: &mut Lemur, _| s.resume(vec![row_resume.clone()]))
        .build(&named(k))
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

/// A host's threads outside every project, then those put away under a row that opens
/// them, then those gone.
fn threads(s: &Lemur, host: &str, sessions: &[Session], gone: &[ClaudeThread], k: &Kit) -> Vec<El> {
    let (away, shown): (Vec<&Session>, Vec<&Session>) = sessions.iter().partition(|t| t.away);
    // Those put away come last in a list, so the rest keep their places for dragging.
    let count = shown.len();
    let list: List = (host.to_string(), None);
    let mut rows: Vec<El> = shown.into_iter().enumerate().map(|(i, t)| thread_row(s, host, t, Some((i, count, &list)), 1, k)).collect();
    rows.extend(away_rows(s, host, &list, &away, 1, k));
    rows.extend(gone.iter().map(|t| gone_row(t, 1, k)));
    rows
}

/// The threads of `list` put away, under a row that opens them, `depth` in.
fn away_rows(s: &Lemur, host: &str, list: &List, away: &[&Session], depth: u8, k: &Kit) -> Vec<El> {
    if away.is_empty() {
        return vec![];
    }
    let open = s.away_open.contains(list);
    let toggle = list.clone();
    let mut rows = vec![
        SidebarRow::new(format!("Put away ({})", away.len()))
            .depth(depth)
            .on_select(move |s: &mut Lemur, _| s.toggle_away_list(toggle.clone()))
            .build(&named(k))
            .key(format!("away-{host}-{}", list.1.as_deref().unwrap_or_default()))
            .opacity(0.7),
    ];
    if open {
        rows.extend(away.iter().map(|t| thread_row(s, host, t, None, depth, k)));
    }
    rows
}

/// A project's rows: its threads in no folder, then each folder with its threads, then
/// those put away and those gone. Dragged, a thread or a folder takes another place among
/// them (`folders::moved`).
fn project_rows(s: &Lemur, host: &str, index: usize, b: &BaseView, k: &Kit) -> Vec<El> {
    let l = super::folders::listed(&s.cfg.bases[index], b);
    let count = l.items().len();
    let list = s.project_list(index);
    let mut rows = vec![];
    let mut at = 0;
    for t in &l.loose {
        rows.push(thread_row(s, host, t, Some((at, count, &list)), 0, k));
        at += 1;
    }
    for (f, threads) in &l.folders {
        rows.push(folder_row(s, index, f, threads, (at, count, &list), k));
        at += 1;
        if !f.collapsed {
            for t in threads {
                rows.push(thread_row(s, host, t, Some((at, count, &list)), 1, k));
                at += 1;
            }
        }
    }
    rows.extend(away_rows(s, host, &list, &l.away, 0, k));
    rows.extend(l.gone.iter().map(|t| gone_row(t, 0, k)));
    rows
}

/// A folder's row: its name and, folded, how many of each kind run in it. A click folds or
/// opens it, a right-click renames or deletes it; dragged, it takes another place among the
/// folders, its threads with it.
fn folder_row(s: &Lemur, index: usize, f: &Folder, threads: &[&Session], (at, count, list): (usize, usize, &List), k: &Kit) -> El {
    let folded = f.collapsed;
    let counts = (folded && !threads.is_empty()).then(|| counts(threads, k));
    let trailing = row().gap(6.0).center_y().children(counts).child(super::meter::chevron(folded, k).key("fold"));
    let mut r = SidebarRow::new(f.name.clone()).icon(&icons::FOLDER).trailing(trailing);
    if folded && threads.iter().any(|t| t.activity == Activity::NeedsInput) {
        r = r.status(RowStatus::Attention);
    }
    let state = if s.row_drag_list == *list { s.row_drag } else { Reorder::new().gap(1.0) };
    let dropped = list.clone();
    r = r.reorder(&state, at, count, |s: &mut Lemur| &mut s.row_drag, move |s, from, to| {
        s.folder_dropped = Some(std::time::Instant::now());
        s.reorder(dropped.clone(), from, to);
    });
    let (toggle, menu_id) = (f.id.clone(), f.id.clone());
    let hovered = list.clone();
    r.on_select(move |s: &mut Lemur, _| {
        // Let go after a drag, it isn't a click.
        if s.folder_dropped.take().is_none_or(|at| at.elapsed() > std::time::Duration::from_millis(500)) {
            s.toggle_folder(index, &toggle);
        }
    })
        .build(&named(k))
        .key(format!("folder-{index}-{}", f.id))
        .expanded(!folded)
        .on_hover_change(move |s: &mut Lemur, inside, tx: &mut Tx| {
            if inside && s.row_drag.dragging().is_none() {
                s.row_drag_list = hovered.clone();
                tx.live_only();
            }
        })
        .on_secondary_press(move |_: &mut Lemur, press: PointerPress, tx: &mut Tx| {
            let id = menu_id.clone();
            let menu = PopupMenu::new().item(PopupItem::new("rename", "Rename…")).item(PopupItem::new("delete", "Delete folder"));
            tx.popup_menu(menu, press, move |s: &mut Lemur, picked, _| match picked {
                Some("rename") => s.ask_name(Ask::RenameFolder { base: index, id: id.clone() }),
                Some("delete") => s.delete_folder(index, &id),
                _ => {}
            });
        })
}

/// What runs in a folded folder: each kind of thread's icon and how many there are.
fn counts(sessions: &[&Session], k: &Kit) -> El {
    let kinds = [(Some(Agent::Claude), "claude"), (None, "shell")];
    row().key("counts").gap(8.0).center_y().a11y_hidden().children(kinds.into_iter().filter_map(|(agent, kind)| {
        let n = sessions.iter().filter(|t| t.agent() == agent).count();
        let (glyph, ink) = agent_glyph(agent, k);
        let count = text(n.to_string()).key(format!("count-{kind}")).font(k.ty.small).color(k.c.text_muted);
        (n > 0).then(|| row().gap(3.0).center_y().child(icon(glyph, 12.0).color(ink)).child(count))
    }))
}

/// A project's menu: a new folder, a new worktree or removing one; resuming its gone
/// threads, its colour, and removing it.
fn project_menu(s: &Lemur, index: usize) -> PopupMenu {
    let gone = s.gone_in(index);
    let mut menu = PopupMenu::new().item(PopupItem::new("new-folder", "New folder…"));
    // A folder without git has no worktrees.
    if let Some((_, b)) = s.project_view(index).filter(|(_, b)| !b.plain) {
        menu = menu.item(PopupItem::new("new-worktree", "New worktree…"));
        let mut linked = PopupMenu::new();
        for w in b.worktrees.iter().filter(|w| !w.main) {
            linked = linked.item(PopupItem::new(format!("remove-worktree:{}", w.path), worktree_label(w)));
        }
        if b.worktrees.iter().any(|w| !w.main) {
            menu = menu.submenu("Remove worktree", linked);
        }
    }
    menu = menu.separator();
    if !gone.is_empty() {
        menu = menu.item(PopupItem::new("resume-all", format!("Resume all threads ({})", gone.len()))).separator();
    }
    // Gray is the mark's own colour.
    let current = s.cfg.bases.get(index).and_then(|b| b.color.clone()).unwrap_or_else(|| "gray".into());
    let mut colors = PopupMenu::new();
    for (name, _) in COLORS {
        colors = colors.item(PopupItem::new(format!("color:{name}"), title_case(name)).checked(current == name));
    }
    menu.submenu("Color", colors).separator().item(PopupItem::new("remove-project", "Remove project…"))
}

fn project_picked(s: &mut Lemur, picked: &str, index: usize) {
    match picked {
        "new-folder" => s.ask_name(Ask::NewFolder { base: index, thread: None }),
        "new-worktree" => s.ask_name(Ask::NewWorktree { base: index, agent: Some(Agent::Claude) }),
        removing if removing.starts_with("remove-worktree:") => {
            let path = &removing["remove-worktree:".len()..];
            let label = s.project_view(index).and_then(|(_, b)| b.worktrees.iter().find(|w| w.path == path)).map(worktree_label);
            if let Some(label) = label {
                let removal = s.removal(index, path.to_string(), label);
                s.confirm_remove_worktree(removal, None);
            }
        }
        "resume-all" => {
            let gone = s.gone_in(index);
            s.resume(gone);
        }
        "remove-project" => {
            if let Some(project) = s.cfg.bases.get(index).cloned() {
                s.confirm_remove_project(project);
            }
        }
        color if color.starts_with("color:") => {
            let name = &color["color:".len()..];
            s.set_project_color(index, Some(name).filter(|n| *n != "gray"));
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
        // Where a folder has its arrow, so the + buttons line up.
        r = r.trailing(row().gap(6.0).center_y().child(plus).child(el().size(12.0, 12.0)));
    }
    r.build(&named(k)).key(format!("host-{name}"))
}

/// The button that folds the sidebar away (`open`) or brings it back: Lucide's, as the
/// buttons beside it, not the kit's.
pub fn toggle(open: bool, k: &Kit) -> El {
    let label = if open { "Hide sidebar (⌘B)" } else { "Show sidebar (⌘B)" };
    icons::button(&icons::PANEL_LEFT, label).on_press(|s: &mut Lemur| s.sidebar.toggle()).build(k).key("sidebar-toggle").expanded(open)
}

/// The sidebar, its title on the title bar's row past the traffic lights, `lights` points
/// from the window's edge (none in full screen).
pub fn sidebar(s: &Lemur, k: &Kit, lights: f32) -> El {
    let c = &k.c;
    // On the title bar's row over the sidebar, as in v2 and Herdr, the sidebar's toggle at
    // its end. Between its buttons it drags the window, as the bar under it.
    // The title shows whole or not at all: a narrow sidebar leaves no stub of it.
    let whole = rune::text::measure("Lemur", &[], k.ty.ui_medium, None).0;
    let title = rune::memo::layout_size("sidebar-title").is_none_or(|[w, _]| w + 0.5 >= whole);
    let header = row()
        .key("sidebar-head")
        .pin(Some(0.0), Some(-TOOLBAR_HEIGHT), Some(0.0), None)
        .h(TOOLBAR_HEIGHT)
        .pl(if lights > 0.0 { lights + PAST_LIGHTS } else { 12.0 })
        .pr(TITLE_END)
        .gap(2.0)
        .center_y()
        .fill(super::look::sidebar(k.dark))
        .window_drag()
        .child(text("Lemur").key("sidebar-title").font(k.ty.ui_medium).color(c.text_base).truncate_fade().grow(1.0).opacity(if title { 1.0 } else { 0.0 }))
        .child(icons::button(&icons::SEARCH, "Go to thread (⌘P)").on_press(|s: &mut Lemur| s.find_thread()).build(k).key("find-thread"))
        .child(icons::button(&icons::PLUS, "Add project (⌘O)").on_press(|s: &mut Lemur| s.open_picker()).build(k).key("add-folder"))
        .child(icons::button(&icons::REFRESH, "Refresh").on_press(|s: &mut Lemur| s.refresh()).build(k).key("refresh"))
        .child(toggle(true, k));
    let mut sb = Sidebar::new().label("Threads").row_gap(1.0);
    if !s.loaded {
        sb = sb.child(SidebarRow::new("Loading sessions…").build(&named(k)).key("loading").opacity(0.6));
    }
    // Projects first, in the config's order whatever their host; then each host's threads
    // outside them, or why it couldn't be reached.
    let mut projects: Vec<_> = s.snap.hosts.iter().flat_map(|h| h.bases.iter().map(move |b| (h, b))).collect();
    projects.sort_by_key(|(_, b)| b.index);
    // A line between each group and the next.
    let mut first = true;
    for (h, b) in projects {
        let index = b.index;
        let base = &s.cfg.bases[index];
        let mut rows = vec![];
        if let Some(e) = &b.error {
            rows.push(SidebarRow::new(e.clone()).icon(&icons::TRIANGLE_ALERT).build(&named(k)).key(format!("error-{index}")).opacity(0.7));
        }
        rows.extend(project_rows(s, &h.name, index, b, k));
        // In its own checkout.
        let (host, dir) = (h.name.clone(), b.worktrees.iter().find(|w| w.main).map_or_else(|| base.path.clone(), |w| w.path.clone()));
        let plus = icons::button(&icons::PLUS, "New terminal or Claude here")
            .on_press_tx(move |_: &mut Lemur, tx: &mut Tx| {
                let (host, dir) = (host.clone(), dir.clone());
                tx.popup_menu(new_menu(), format!("project-plus-{index}"), move |s: &mut Lemur, picked, _| {
                    if let Some(p) = picked {
                        new_picked(s, p, &host, &dir, Some(index));
                    }
                });
            })
            .build(k)
            .key(format!("project-plus-{index}"));
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
        // Dragged by its header to another place among the projects.
        let title = format!("{} · {}", base.name(), h.name);
        let id = format!("project-{index}");
        let header = project_header(base, &h.name, index, plus, menu_button, k);
        let header = drags(note_place(header, id.clone()), id.clone(), move |s, dy| s.drop_project(index, dy));
        let mut section = col().key(id.clone()).role("group").label(title).w_full().gap(2.0).motion(k.motion.disclosure).child(header);
        if !base.collapsed {
            section = section.children(rows);
        }
        if !std::mem::take(&mut first) {
            sb = sb.child(divider(format!("line-{id}"), k));
        }
        sb = sb.child(lift(s, section, &id, k));
    }
    // Then one section for every host, each with its threads outside every project.
    let mut other = vec![];
    for h in &s.snap.hosts {
        if let Some(e) = &h.error {
            let mut rows = vec![SidebarRow::new(e.clone()).icon(&icons::TRIANGLE_ALERT).build(&named(k)).key(format!("host-error-{}", h.name)).opacity(0.7)];
            // herdr-core needs you to add it: in a terminal, as Herdr's.
            if let Some(host) = s.cfg.host(&h.name).filter(|_| h.needs_you).cloned() {
                rows.push(
                    SidebarRow::new(format!("Set up {}…", h.name))
                        .icon(&icons::SQUARE_TERMINAL)
                        .on_select(move |s: &mut Lemur, _| s.set_up_host(host.clone()))
                        .build(&named(k))
                        .key(format!("set-up-{}", h.name)),
                );
            }
            if !std::mem::take(&mut first) {
                sb = sb.child(divider(format!("line-host-{}", h.name), k));
            }
            sb = sb.child(group(format!("host-section-{}", h.name), &h.name, rows, k));
        } else {
            other.push(host_row(s, &h.name, &h.home, k));
            other.extend(threads(s, &h.name, &h.other, &h.gone, k));
        }
    }
    if !other.is_empty() {
        if !first {
            sb = sb.child(divider("line-other".into(), k));
        }
        sb = sb.child(group("other".into(), "Other", other, k));
    }
    let foot = col().key("sidebar-foot").w_full().children(super::panels::sidebar_foot(s, k)).child(s.machines(k));
    sb.footer(foot).build(k).fill(super::look::sidebar(k.dark)).child(header)
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

    /// Opens the notes panel on a worktree's notes, as the tests do: the Notes button opens
    /// those of the thread on show.
    #[cfg(test)]
    pub fn open_notes(&mut self, host: &str, worktree: &str, label: &str) {
        self.show_notes(host, worktree, label);
        self.right_last = super::Right::Notes;
        self.inspector.set_open(true);
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

    /// The list thread `key` is in: its project's, or its host's outside every project.
    pub fn list_of(&self, key: &Key) -> List {
        match self.project_of(&key.0, &key.1) {
            Some((index, _)) => self.project_list(index),
            None => (key.0.clone(), None),
        }
    }
}
