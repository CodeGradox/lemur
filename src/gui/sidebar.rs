//! The sidebar, as Rex's: one session's threads at a time (`spaces`), the thread on show's,
//! in folders (`folders`), as rows to open, fold, drag and right-click; the session's name
//! over them switches to another, as ⌘K does. The hosts' load at the foot.

use super::folders::{Group, SidebarDrag, folder_of, listed};
use super::prompt::{Ask, agent_glyph, thread_icon};
use super::silence::silence_label;
use super::spaces::{Space, space_id, space_of_id};
use super::{Key, Lemur, List, icons, key, worktree_label};
use crate::config::{Agent, COLORS, ClaudeThread};
use crate::snapshot::{Activity, HOME, HostView, Session};
use rune::{Anim, Color, Drag, El, LoopSpec, Phase, PointerPress, PopupItem, PopupMenu, Shadow, Tx, col, el, icon, row, text};
use rune_kit::shell::{ROW_HEIGHT, RowStatus, Sidebar, SidebarRow, TOOLBAR_HEIGHT};
use rune_kit::{Combobox, ComboboxOption, IconButtonVariant, Kit, MenuItem, context_menu};

/// Points between the sidebar's rows.
const GAP: f32 = 2.0;
/// A folder's row, which is smaller than a thread's (`ROW_HEIGHT`).
const FOLDER_ROW: f32 = 24.0;

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

/// `row` as Rex has the thread on show's, when `on`: a pill of glass, lit from above and
/// raised a little off the sidebar. Rune has the system's Liquid Glass only in a see-through
/// window, which Lemur's isn't, so this draws its look.
pub(super) fn glass(row: El, on: bool, k: &Kit) -> El {
    if !on {
        return row;
    }
    let (white, black) = (Color::WHITE, Color::BLACK);
    // Its fill, top to bottom; its rim, top to bottom; the edge the glass bends, inside the
    // rim; its shadow; the pointer over it.
    let ((top, bottom), (lit, unlit), edge, shade, hover) = if k.dark {
        let grey = |hex: &str| rune::color::hex(hex).expect("a colour");
        ((grey("#242424"), grey("#1a1a1a")), (white.alpha(0.16), white.alpha(0.05)), white.alpha(0.04), black.alpha(0.6), white.alpha(0.03))
    } else {
        ((white, white), (black.alpha(0.09), black.alpha(0.15)), black.alpha(0.04), black.alpha(0.07), black.alpha(0.02))
    };
    row.radius(ROW_HEIGHT / 2.0)
        .continuous_corners()
        .fill(bottom)
        .gradient(top, bottom)
        .border_gradient(1.0, lit, unlit)
        .box_shadow(Shadow { blur: 6.0, color: edge, inset: true, ..Shadow::default() })
        .shadow(1.0, 3.0, shade)
        .hover(hover)
}

/// One of `COLORS`, by its name.
pub(super) fn color_named(name: &str) -> Option<Color> {
    let (_, hex) = COLORS.iter().find(|(n, _)| *n == name)?;
    rune::color::hex(&format!("#{hex:06x}"))
}

/// `row`, `rows[at]` of `list` (a session's folders, or with `folder` that folder's threads),
/// dragged to another place there: it follows the pointer, as far as the list's ends, and
/// the others step aside. Let go, they keep the new order (`reorder_threads`,
/// `reorder_folders`).
fn draggable(row: El, list: &List, folder: Option<&str>, rows: &[(String, f32)], at: usize) -> El {
    let (list, folder, rows) = (list.clone(), folder.map(str::to_string), rows.to_vec());
    row.on_drag(move |s: &mut Lemur, d: &mut Drag, tx: &mut Tx| match d.phase {
        Phase::Down => s.sidebar_drag = Some(SidebarDrag::new(list.clone(), folder.clone(), &rows, at)),
        Phase::Move => {
            if let Some(drag) = &mut s.sidebar_drag {
                drag.follow(d.dy);
                tx.animated();
            }
        }
        Phase::Up => {
            let Some(drag) = s.sidebar_drag.take() else { return };
            if drag.moved() && drag.folder.is_none() {
                s.folder_dropped = Some(std::time::Instant::now());
            }
            if d.cancelled() {
                return;
            }
            match &drag.folder {
                Some(id) => {
                    if let Some((from, to)) = drag.moves() {
                        s.reorder_threads(&drag.list, id, from, to);
                    }
                }
                None => {
                    if let Some(order) = drag.dropped() {
                        s.reorder_folders(&drag.list, &order);
                    }
                }
            }
        }
    })
}

/// `row`, `id` of the list `drag` is in, where the drag puts it: lifted off the others, over
/// them, following the pointer exactly, or stepped aside on a spring.
fn dragged(row: El, id: &str, drag: Option<&SidebarDrag>, k: &Kit) -> El {
    let Some(drag) = drag else { return row };
    let row = row.offset(0.0, drag.shift(id));
    if !drag.lifts(id) {
        return row;
    }
    k.elevate(row.raise().offset_motion(Anim::INSTANT), &k.elevation.raised)
}

/// What sets a session apart from the next, when the sidebar lists them all: a line.
fn divider(key: String, k: &Kit) -> El {
    el().key(key).w_full().h(1.0).mt(6.0).mb(4.0).fill(k.c.border_base).a11y_hidden()
}

/// The header of a session in the sidebar: its name and machine, which, when it's the `one`
/// shown, opens a list of the sessions to switch to, drawn as Rex's is, or start one
/// (`session_switch`); then its +, which starts a shell in its machine's home in a new
/// folder, as ⇧⌘N does, and its menu.
fn session_header(s: &Lemur, space: &Space, one: bool, k: &Kit, layer: &mut Vec<El>) -> El {
    let c = &k.c;
    let host = s.cfg.host(&space.0);
    let glyph = host.map_or(&icons::SERVER, super::picker::host_icon);
    let view = s.space_view(space);
    let name = view.as_ref().map_or_else(|| space.1.clone(), |v| v.name.clone());
    let machine = view.as_ref().map_or_else(|| space.0.clone(), |v| v.host.clone());
    // Each session's own, with them all listed.
    let (plus_key, menu_key) = match one {
        true => ("session-plus".to_string(), "session-menu".to_string()),
        false => (format!("session-plus-{}-{}", space.0, space.1), format!("session-menu-{}-{}", space.0, space.1)),
    };
    let on = space.clone();
    let plus = icons::button(&icons::PLUS, "New folder (⇧⌘N): a shell in the machine's home")
        .on_press(move |s: &mut Lemur| s.new_folder_in(on.clone()))
        .build(k)
        .key(plus_key);
    let (_, any) = session_menu(s, space);
    let menu_button = any.then(|| {
        let (space, anchor) = (space.clone(), menu_key.clone());
        icons::button(&icons::ELLIPSIS, "Session")
            .on_press_tx(move |s: &mut Lemur, tx: &mut Tx| {
                let space = space.clone();
                tx.popup_menu(session_menu(s, &space).0, anchor.as_str(), move |s: &mut Lemur, picked, _| {
                    if let Some(p) = picked {
                        session_picked(s, p, &space);
                    }
                });
            })
            .build(k)
            .key(menu_key)
    });
    let title = row()
        .min_w(0.0)
        .shrink()
        .h(26.0)
        .px(6.0)
        .gap(6.0)
        .center_y()
        .radius(k.radius.sm)
        .child(icon(glyph, 14.0).color(c.icon_muted).a11y_hidden())
        .child(text(name.clone()).font(k.ty.ui_medium).color(c.text_base).truncate().shrink().a11y_hidden())
        .child(text(machine.clone()).font(k.ty.small).color(c.text_faint).truncate().shrink().a11y_hidden());
    let header = row()
        .key("section-header")
        .role("heading")
        .label(format!("{name} · {machine}"))
        .w_full()
        .h(30.0)
        .pl(2.0)
        .pr(8.0)
        .gap(6.0)
        .center_y();
    if !one {
        return header.child(title).child(el().grow(1.0).a11y_hidden()).child(plus).children(menu_button);
    }
    let trigger = k
        .interactive(title.key("session-switch").child(icon(&icons::CHEVRON_DOWN, 12.0).color(c.icon_faint).a11y_hidden()))
        .cursor(rune::Cursor::Pointer)
        .role("button")
        .label(format!("Session {name} on {machine}: switch (⌘K)"));
    let mut options: Vec<ComboboxOption> = s
        .spaces()
        .into_iter()
        .map(|v| {
            let mut o = ComboboxOption::new(v.id(), v.name.clone());
            o.detail = v.host;
            o
        })
        .collect();
    options.push(ComboboxOption::new(NEW_SESSION, "New session…"));
    let switch = Combobox::new("session-switch-list", &s.session_switch, &options, |s: &mut Lemur| &mut s.session_switch)
        .label("Session")
        .trigger(trigger)
        .current(Some(&space_id(space)))
        .search_placeholder("Session…")
        .width(300.0)
        .max_height(320.0)
        .on_choose(|s: &mut Lemur, id, _| match space_of_id(id) {
            Some(space) => s.show_space(space),
            None => s.open_picker(),
        })
        .build(k, layer);
    header
        .child(switch.shrink().min_w(0.0))
        .child(el().grow(1.0).a11y_hidden())
        .child(plus)
        .children(menu_button)
}

/// The id of the dropdown's last option, which starts a session.
const NEW_SESSION: &str = "new-session";

/// A session's menu: for a project's, a new worktree of it or removing one, and removing
/// the session; resuming its gone threads; and whether it has anything.
fn session_menu(s: &Lemur, space: &Space) -> (PopupMenu, bool) {
    let mut menu = PopupMenu::new();
    let mut any = false;
    let base = s.space_base(space);
    // A folder without git has no worktrees.
    if let Some(b) = base.and_then(|i| s.project_view(i)).filter(|b| !b.plain) {
        menu = menu.item(PopupItem::new("new-worktree", "New worktree…"));
        let mut remove = PopupMenu::new();
        let mut linked = false;
        for w in b.worktrees.iter().filter(|w| !w.main) {
            linked = true;
            remove = remove.item(PopupItem::new(format!("remove-worktree:{}", w.path), worktree_label(w)));
        }
        if linked {
            menu = menu.submenu("Remove worktree", remove);
        }
        any = true;
    }
    let gone = s.gone_in(space).len();
    if gone > 0 {
        if any {
            menu = menu.separator();
        }
        menu = menu.item(PopupItem::new("resume-all", format!("Resume all threads ({gone})")));
        any = true;
    }
    if base.is_some() {
        menu = menu.separator().item(PopupItem::new("remove-session", "Remove session…"));
        any = true;
    }
    (menu, any)
}

fn session_picked(s: &mut Lemur, picked: &str, space: &Space) {
    let Some(i) = s.space_base(space).filter(|_| picked != "resume-all") else {
        if picked == "resume-all" {
            let gone = s.gone_in(space);
            s.resume(gone);
        }
        return;
    };
    if picked == "new-worktree" {
        s.ask_name(Ask::NewWorktree { base: i, agent: Some(Agent::Claude) });
    } else if let Some(path) = picked.strip_prefix("remove-worktree:") {
        let label = s.project_view(i).and_then(|b| b.worktrees.iter().find(|w| w.path == path)).map(worktree_label);
        if let Some(label) = label {
            let removal = s.removal(i, path.to_string(), label);
            s.confirm_remove_worktree(removal, None);
        }
    } else if picked == "remove-session" {
        let project = s.cfg.bases[i].clone();
        s.confirm_remove_project(project);
    }
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


/// The colours to give the thread right-clicked, each as a swatch beside its name, where it
/// was clicked (`color_menu`), drawn by Lemur, as macOS's menus can't colour theirs: those
/// of `COLORS`, its own checked, or none.
fn color_menu(s: &Lemur, k: &Kit, layer: &mut Vec<El>) -> El {
    let point = s.color_menu.as_ref().map(|(_, at)| *at);
    let current = s.color_menu.as_ref().and_then(|(key, _)| s.session_of(key)).and_then(|t| t.color.clone());
    let mut menu = context_menu("thread-colors", el().key("thread-colors-at"), point).on_close(|s: &mut Lemur| s.color_menu = None);
    for (name, _) in COLORS {
        let swatch = el().size(12.0, 12.0).radius(6.0).fill(color_named(name).unwrap_or(k.c.icon_muted));
        let pick = name.to_string();
        let item = MenuItem::new(title_case(name)).lead(swatch).checked(current.as_deref() == Some(name));
        menu = menu.item(item.on_select(move |s: &mut Lemur| s.pick_color(Some(&pick))));
    }
    let none = MenuItem::new("No color").checked(current.is_none()).on_select(|s: &mut Lemur| s.pick_color(None));
    menu.separator().item(none).build(k, layer)
}

/// A thread's right-click: renaming it, peeking at it, silencing it, putting it away, its
/// colour; moving it to another of `folders` (by id, with its name), or one of its own.
fn thread_menu(s: &Session, folders: &[(String, String)]) -> PopupMenu {
    let menu = PopupMenu::new()
        .item(PopupItem::new("rename", "Rename…"))
        .item(PopupItem::new("peek", "Peek"))
        .item(PopupItem::new("silence", silence_label(s.silent)))
        .item(if s.away { PopupItem::new("bring-back", "Bring back") } else { PopupItem::new("put-away", "Put away") })
        .item(PopupItem::new("color", "Color…"));
    let now = folder_of(s);
    let mut to = PopupMenu::new();
    for (id, name) in folders {
        to = to.item(PopupItem::new(format!("folder:{id}"), name.clone()).checked(now == *id));
    }
    to = to.separator().item(PopupItem::new("new-folder", "New folder"));
    menu.separator().submenu("Move to folder", to)
}

/// What thread `key`'s right-click, `at` a place, runs once `picked`.
fn thread_picked(s: &mut Lemur, picked: &str, key: Key, at: PointerPress) {
    let silent = s.session_of(&key).is_some_and(|t| t.silent);
    match picked {
        "rename" => s.ask_name(Ask::Rename { host: key.0, session: key.1 }),
        "peek" => s.peek(key),
        "silence" => s.set_silent(key, !silent),
        "bring-back" => s.bring_back(&key),
        "put-away" => s.put_away(key),
        "new-folder" => s.move_to_folder(&key, None),
        "color" => s.color_menu = Some((key, at)),
        folder if folder.starts_with("folder:") => {
            let id = &folder["folder:".len()..];
            s.move_to_folder(&key, Some(id));
        }
        _ => {}
    }
}

/// Where a thread's row is, to drag it among its folder's: its place, its session's list,
/// its folder's id, and its folder's threads, each with how far it starts from the next.
type Place<'a> = (usize, &'a List, &'a str, &'a [(String, f32)]);

/// A thread's row, `depth` in: its agent's icon, its name, then the issue pinned to it, what
/// helps it and its age; its dot, a × under the pointer. Its right-click moves it among
/// `folders`, its host's.
fn thread_row(s: &Lemur, host: &str, t: &Session, place: Option<Place>, folders: &[(String, String)], k: &Kit) -> El {
    let c = &k.c;
    let key = key(host, t);
    let (dot, tip, breathes) = s.activity(&key, t, k);
    let shown = s.active.as_ref() == Some(&key);
    let mut said = vec![];
    if let Some(issue) = &t.issue {
        said.push(issue.clone());
    }
    if !t.helpers.is_empty() {
        said.push(format!("{} helping", t.helpers.len()));
    }
    said.push(age(t.idle_secs));
    let mut r = SidebarRow::new(t.label.clone())
        .avatar(thread_icon(t.agent(), t.color.as_deref(), k))
        .detail(said.join(" · "))
        .status(RowStatus::Other(status_dot(dot, tip, breathes)))
        .selected(shown);
    if t.silent {
        r = r.trailing(icon(&icons::BELL_OFF, 12.0).color(c.icon_faint).help("Makes no notifications"));
    }
    let (open, close, rename, menu) = (key.clone(), key.clone(), key.clone(), key.clone());
    let picked_menu = thread_menu(t, folders);
    let r = r
        .on_select(move |s: &mut Lemur, _| s.open(open.0.clone(), open.1.clone()))
        .on_close(move |s: &mut Lemur, _| s.confirm_close_of(close.clone()))
        .on_double_click(move |s: &mut Lemur, _| s.ask_name(Ask::Rename { host: rename.0.clone(), session: rename.1.clone() }));
    let mut r = glass(r.build(&named(k)), shown, k);
    // The `at`th of folder `folder`'s threads in `list`: dragged, it takes another place among
    // them, and only there.
    if let Some((at, list, folder, threads)) = place {
        let drag = s.sidebar_drag.as_ref().filter(|d| d.list == *list && d.folder.as_deref() == Some(folder));
        // Opaque, as it's lifted, so the rows it passes don't show through.
        let lifted = drag.is_some_and(|d| d.lifts(&t.name)) && !shown;
        if lifted {
            r = r.fill(super::look::sidebar(k.dark));
        }
        r = dragged(draggable(r, list, Some(folder), threads, at).position_in_set(at + 1, threads.len()), &t.name, drag, k);
    }
    r.key(format!("row-{}-{}", key.0, key.1))
        .opacity(if t.away { 0.6 } else { 1.0 })
        .on_secondary_press(move |_: &mut Lemur, press: PointerPress, tx: &mut Tx| {
            let key = menu.clone();
            tx.popup_menu(picked_menu.clone(), press, move |s: &mut Lemur, picked, _| {
                if let Some(p) = picked {
                    thread_picked(s, p, key, press);
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
        .avatar(thread_icon(Some(Agent::Claude), t.color.as_deref(), k))
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

/// The threads of `list` put away, under a row that opens them.
fn away_rows(s: &Lemur, host: &str, list: &List, away: &[&Session], folders: &[(String, String)], k: &Kit) -> Vec<El> {
    if away.is_empty() {
        return vec![];
    }
    let open = s.away_open.contains(list);
    let toggle = list.clone();
    let mut rows = vec![
        SidebarRow::new(format!("Put away ({})", away.len()))
            .on_select(move |s: &mut Lemur, _| s.toggle_away_list(toggle.clone()))
            .build(&named(k))
            .key(format!("away-{host}"))
            .opacity(0.7),
    ];
    if open {
        rows.extend(away.iter().map(|t| thread_row(s, host, t, None, folders, k)));
    }
    rows
}

/// Session `space`'s rows, on host `h`: each folder with its threads, then those put away
/// and those gone. Dragged, a thread takes another place in its folder, and a folder, its
/// threads with it, among the folders (`SidebarDrag`).
fn space_rows(s: &Lemur, h: &HostView, space: &Space, k: &Kit) -> Vec<El> {
    let host = h.name.as_str();
    let l = listed(h, Some(&space.1));
    let list = Lemur::space_list(space);
    let folders: Vec<(String, String)> = l.groups.iter().map(|g| g.id.clone()).zip(s.folder_titles(h, &l.groups)).collect();
    // How far each folder starts from the next: its row, and its threads unless it's folded.
    let pitches: Vec<(String, f32)> = l
        .groups
        .iter()
        .map(|g| {
            let shown = if s.folded(host, &g.id) { 0 } else { g.threads.len() };
            (g.id.clone(), FOLDER_ROW + shown as f32 * (ROW_HEIGHT + GAP) + GAP)
        })
        .collect();
    let drag = s.sidebar_drag.as_ref().filter(|d| d.list == list && d.folder.is_none());
    let mut rows = vec![];
    for (at, (g, (_, name))) in l.groups.iter().zip(&folders).enumerate() {
        let mut group = col()
            .key(format!("group-{host}-{}", g.id))
            .w_full()
            .gap(GAP)
            .motion(k.motion.control)
            .child(folder_row(s, host, g, name.clone(), (at, &list, &pitches), k));
        if !s.folded(host, &g.id) {
            let threads: Vec<(String, f32)> = g.threads.iter().map(|t| (t.name.clone(), ROW_HEIGHT + GAP)).collect();
            for (i, t) in g.threads.iter().enumerate() {
                group = group.child(thread_row(s, host, t, Some((i, &list, &g.id, &threads)), &folders, k));
            }
        }
        // Opaque, as it's lifted, so the folders it passes don't show through.
        if drag.is_some_and(|d| d.lifts(&g.id)) {
            group = group.fill(super::look::sidebar(k.dark)).radius(k.radius.md);
        }
        rows.push(dragged(group, &g.id, drag, k));
    }
    rows.extend(away_rows(s, host, &list, &l.away, &folders, k));
    rows.extend(l.gone.iter().map(|t| gone_row(t, k)));
    rows
}

/// A folder's row, as Rex's session headings are: its name, small and muted, and, folded, how many of
/// each kind run in it. A click folds or opens it, a right-click renames it; dragged, it
/// takes another place among the folders, its threads with it.
fn folder_row(s: &Lemur, host: &str, g: &Group, name: String, (at, list, pitches): (usize, &List, &[(String, f32)]), k: &Kit) -> El {
    let folded = s.folded(host, &g.id);
    let counts = (folded && !g.threads.is_empty()).then(|| counts(&g.threads, k));
    let trailing = row().gap(6.0).center_y().children(counts).child(super::meter::chevron(folded, k).key("fold"));
    // Small, as Rex's session names are: it says where the threads under it are, not more.
    let mut r = SidebarRow::new(name).title_style(k.ty.small).height(FOLDER_ROW).trailing(trailing);
    if folded && g.threads.iter().any(|t| t.activity == Activity::NeedsInput) {
        r = r.status(RowStatus::Attention);
    }
    let (toggle, menu_id, host) = (g.id.clone(), g.id.clone(), host.to_string());
    let (fold_host, menu_host) = (host.clone(), host.clone());
    let r = r
        .on_select(move |s: &mut Lemur, _| {
            // Let go after a drag, it isn't a click.
            if s.folder_dropped.take().is_none_or(|at| at.elapsed() > std::time::Duration::from_millis(500)) {
                s.toggle_folder(&fold_host, &toggle);
            }
        })
        .build(k)
        .key(format!("folder-{host}-{}", g.id))
        .expanded(!folded);
    // Dragged, it takes its threads along: the group it heads moves (`space_rows`).
    draggable(r, list, None, pitches, at).on_secondary_press(move |_: &mut Lemur, press: PointerPress, tx: &mut Tx| {
        let (host, id) = (menu_host.clone(), menu_id.clone());
        let menu = PopupMenu::new().item(PopupItem::new("rename", "Rename…"));
        tx.popup_menu(menu, press, move |s: &mut Lemur, picked, _| {
            if picked == Some("rename") {
                s.ask_name(Ask::RenameFolder { host: host.clone(), id: id.clone() });
            }
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

/// The button that folds the sidebar away (`open`) or brings it back: Lucide's, as the
/// buttons beside it, not the kit's.
pub fn toggle(open: bool, k: &Kit) -> El {
    let label = if open { "Hide sidebar (⌘B)" } else { "Show sidebar (⌘B)" };
    icons::button(&icons::PANEL_LEFT, label).on_press(|s: &mut Lemur| s.sidebar.toggle()).build(k).key("sidebar-toggle").expanded(open)
}

/// The sidebar, its title on the title bar's row past the traffic lights, `lights` points
/// from the window's edge (none in full screen); its dropdown goes in `layer`.
pub fn sidebar(s: &Lemur, k: &Kit, lights: f32, layer: &mut Vec<El>) -> El {
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
        .child(
            icons::button(&icons::EYE, if s.sidebar_agents { "Show the session (⌘.)" } else { "Show every agent (⌘.)" })
                .variant(if s.sidebar_agents { IconButtonVariant::Neutral } else { IconButtonVariant::GhostMuted })
                .on_press(|s: &mut Lemur| s.toggle_sidebar_agents())
                .build(k)
                .key("sidebar-agents"),
        )
        .child(icons::button(&icons::PLUS, "New Claude or shell (⌘J)").on_press(|s: &mut Lemur| s.new_thread_here()).build(k).key("new-thread"))
        .child(icons::button(&icons::REFRESH, "Refresh").on_press(|s: &mut Lemur| s.refresh()).build(k).key("refresh"))
        .child(toggle(true, k));
    let mut sb = Sidebar::new().label("Threads").row_gap(1.0);
    if !s.loaded {
        sb = sb.child(SidebarRow::new("Loading sessions…").build(&named(k)).key("loading").opacity(0.6));
    }
    // Every agent's thread, as Rex's eye lists what reports activity; else the session shown:
    // its folders of threads, or why its host couldn't be reached.
    if s.sidebar_agents {
        sb = sb.child(col().key("sidebar-agents-list").role("group").label("Agents").w_full().gap(2.0).children(super::agents::sidebar_rows(s, k)));
    } else if s.prefs.all_sessions {
        // Every session, a line between each and the next, as the live version lists projects.
        // A host it couldn't reach says so once, under its home.
        let mut first = true;
        for v in s.spaces() {
            let Some(h) = s.host_view(&v.space.0) else { continue };
            if h.error.is_some() && v.space.1 != HOME {
                continue;
            }
            if !std::mem::take(&mut first) {
                sb = sb.child(divider(format!("line-{}-{}", v.space.0, v.space.1), k));
            }
            sb = sb.child(session_section(s, &v.space, h, false, k, layer));
        }
    } else if let Some((space, h)) = s.shown_space().and_then(|space| Some((space.clone(), s.host_view(&space.0)?))) {
        sb = sb.child(session_section(s, &space, h, true, k, layer));
    }
    let foot = col().key("sidebar-foot").w_full().children(super::panels::sidebar_foot(s, k)).child(s.machines(k));
    sb.footer(foot).build(k).fill(super::look::sidebar(k.dark)).child(header).child(color_menu(s, k, layer))
}

/// Session `space`'s section, on host `h`: its header, then its folders of threads, or why
/// the host couldn't be reached. With `one`, it's the only one the sidebar shows.
fn session_section(s: &Lemur, space: &Space, h: &HostView, one: bool, k: &Kit, layer: &mut Vec<El>) -> El {
    let mut rows = vec![];
    if let Some(e) = &h.error {
        rows.push(SidebarRow::new(e.clone()).icon(&icons::TRIANGLE_ALERT).build(&named(k)).key(format!("host-error-{}", h.name)).opacity(0.7));
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
    } else {
        // Its project, if the host couldn't look at it.
        for b in h.bases.iter().filter(|b| b.error.is_some() && b.path == space.1) {
            let said = format!("{}: {}", s.cfg.bases[b.index].name(), b.error.as_deref().unwrap_or_default());
            rows.push(SidebarRow::new(said).icon(&icons::TRIANGLE_ALERT).build(&named(k)).key(format!("error-{}", b.index)).opacity(0.7));
        }
        rows.extend(space_rows(s, h, space, k));
    }
    let label = s.space_view(space).map_or_else(|| space.1.clone(), |v| format!("{} · {}", v.name, v.host));
    let section = col().key(format!("session-{}-{}", space.0, space.1)).role("group").label(label).w_full().gap(2.0);
    section.child(session_header(s, space, one, k, layer)).children(rows)
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

    /// Colours thread `key`, or with `None` takes its colour off: it shows at once, herdr-core
    /// keeps it on the terminal, and a thread kept to resume keeps it too.
    /// The colour picked among the swatches of the thread right-clicked, or with `None` none.
    fn pick_color(&mut self, color: Option<&str>) {
        if let Some((key, _)) = self.color_menu.take() {
            self.set_thread_color(key, color);
        }
    }

    pub(super) fn set_thread_color(&mut self, key: Key, color: Option<&str>) {
        let color = color.map(str::to_string);
        if let Some(s) = self.session_mut(&key) {
            s.color = color.clone();
        }
        if let Some(thread) = self.cfg.threads.iter().find(|t| t.host == key.0 && t.session == key.1).cloned() {
            self.keep_thread(ClaudeThread { color: color.clone(), ..thread });
        }
        self.write_core(move |cfg| crate::actions::set_color(cfg, &key.0, &key.1, color.as_deref()));
    }
}
