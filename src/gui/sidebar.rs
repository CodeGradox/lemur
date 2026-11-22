//! The sidebar, as Rex's: one host's threads at a time, the thread on show's, in folders
//! (`folders`), every project's and none's, as rows to open, fold, drag and right-click;
//! the host's name over them picks another. The hosts' load at the foot. Projects don't
//! show: ⌘K starts a thread in one.

use super::folders::{Group, folder_of, listed};
use super::prompt::{Ask, agent_glyph, thread_icon};
use super::silence::silence_label;
use super::{Key, Lemur, List, icons, key, worktree_label};
use crate::config::{Agent, COLORS, ClaudeThread};
use crate::snapshot::{Activity, HostView, Session};
use rune::{Color, El, LoopSpec, PointerPress, PopupItem, PopupMenu, Tx, col, el, icon, row, text};
use rune_kit::shell::{Reorder, RowStatus, Sidebar, SidebarRow, TOOLBAR_HEIGHT};
use rune_kit::Kit;

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

/// One of `COLORS`, by its name.
pub(super) fn color_named(name: &str) -> Option<Color> {
    let (_, hex) = COLORS.iter().find(|(n, _)| *n == name)?;
    rune::color::hex(&format!("#{hex:06x}"))
}

/// The header of the host the sidebar shows: its icon and name, which, with other hosts,
/// picks one of them to show instead, as Rex's host filter does; then its +, which starts a
/// shell in its home in a new folder, as ⇧⌘N does, and its menu.
fn host_header(s: &Lemur, name: &str, k: &Kit) -> El {
    let c = &k.c;
    let host = s.cfg.host(name);
    let glyph = host.map_or(&icons::SERVER, super::picker::host_icon);
    let label = host.map_or_else(|| name.to_string(), super::picker::host_label);
    let on = name.to_string();
    let plus = icons::button(&icons::PLUS, "New folder: a shell in its home")
        .on_press(move |s: &mut Lemur| s.new_folder(Some(on.clone())))
        .build(k)
        .key(format!("host-plus-{name}"));
    let (_, any) = host_menu(s, name);
    let menu_button = any.then(|| {
        let host = name.to_string();
        icons::button(&icons::ELLIPSIS, "Host")
            .on_press_tx(move |s: &mut Lemur, tx: &mut Tx| {
                let host = host.clone();
                tx.popup_menu(host_menu(s, &host).0, format!("host-menu-{host}"), move |s: &mut Lemur, picked, _| {
                    if let Some(p) = picked {
                        host_picked(s, p, &host);
                    }
                });
            })
            .build(k)
            .key(format!("host-menu-{name}"))
    });
    let mut switch = row()
        .key("host-switch")
        .min_w(0.0)
        .shrink()
        .h(24.0)
        .px(4.0)
        .gap(6.0)
        .center_y()
        .radius(k.radius.sm)
        .child(icon(glyph, 14.0).color(c.icon_muted).a11y_hidden())
        .child(text(label.clone()).font(k.ty.ui_medium).color(c.text_base).truncate().shrink().a11y_hidden());
    if s.snap.hosts.len() > 1 {
        let shown = name.to_string();
        switch = k
            .interactive(switch.child(icon(&icons::CHEVRON_DOWN, 12.0).color(c.icon_faint).a11y_hidden()))
            .cursor(rune::Cursor::Pointer)
            .role("button")
            .label(format!("{label}: show another host"))
            .on_press_tx(move |s: &mut Lemur, tx: &mut Tx| {
                let hosts: Vec<(String, String)> = s
                    .snap
                    .hosts
                    .iter()
                    .map(|h| (h.name.clone(), s.cfg.host(&h.name).map_or_else(|| h.name.clone(), super::picker::host_label)))
                    .collect();
                let menu = PopupMenu::picker(hosts.iter().map(|(id, name)| (id.as_str(), name.as_str())), &shown);
                tx.popup_menu(menu, "host-switch", |s: &mut Lemur, picked, _| {
                    if let Some(host) = picked {
                        s.show_host(host);
                    }
                });
            });
    }
    row()
        .key("section-header")
        .role("heading")
        .label(label)
        .w_full()
        .h(30.0)
        .pl(4.0)
        .pr(8.0)
        .gap(6.0)
        .center_y()
        .child(switch)
        .child(el().grow(1.0).a11y_hidden())
        .child(plus)
        .children(menu_button)
}

/// A host's menu: a new worktree in one of its projects or removing one, resuming its gone
/// threads, and removing a project; and whether it has anything.
fn host_menu(s: &Lemur, host: &str) -> (PopupMenu, bool) {
    let projects: Vec<usize> = (0..s.cfg.bases.len()).filter(|&i| s.cfg.bases[i].host == host).collect();
    let (mut new, mut remove, mut forget) = (PopupMenu::new(), PopupMenu::new(), PopupMenu::new());
    let (mut repos, mut linked) = (false, false);
    for &index in &projects {
        let name = s.cfg.bases[index].name();
        forget = forget.item(PopupItem::new(format!("remove-project:{index}"), name.clone()));
        // A folder without git has no worktrees.
        let Some(b) = s.project_view(index).filter(|b| !b.plain) else { continue };
        repos = true;
        new = new.item(PopupItem::new(format!("new-worktree:{index}"), name.clone()));
        for w in b.worktrees.iter().filter(|w| !w.main) {
            linked = true;
            remove = remove.item(PopupItem::new(format!("remove-worktree:{index}:{}", w.path), format!("{name} › {}", worktree_label(w))));
        }
    }
    let mut menu = PopupMenu::new();
    if repos {
        menu = menu.submenu("New worktree", new);
    }
    if linked {
        menu = menu.submenu("Remove worktree", remove);
    }
    let gone = s.gone_on(host).len();
    if gone > 0 {
        if repos {
            menu = menu.separator();
        }
        menu = menu.item(PopupItem::new("resume-all", format!("Resume all threads ({gone})")));
    }
    if !projects.is_empty() {
        menu = menu.separator().submenu("Remove project", forget);
    }
    (menu, gone > 0 || !projects.is_empty())
}

fn host_picked(s: &mut Lemur, picked: &str, host: &str) {
    let index = |id: &str| id.parse::<usize>().ok().filter(|&i| s.cfg.bases.get(i).is_some_and(|b| b.host == host));
    if let Some(i) = picked.strip_prefix("new-worktree:").and_then(index) {
        s.ask_name(Ask::NewWorktree { base: i, agent: Some(Agent::Claude) });
    } else if let Some((i, path)) = picked.strip_prefix("remove-worktree:").and_then(|r| r.split_once(':')) {
        let Some(i) = index(i) else { return };
        let label = s.project_view(i).and_then(|b| b.worktrees.iter().find(|w| w.path == path)).map(worktree_label);
        if let Some(label) = label {
            let removal = s.removal(i, path.to_string(), label);
            s.confirm_remove_worktree(removal, None);
        }
    } else if picked == "resume-all" {
        let gone = s.gone_on(host);
        s.resume(gone);
    } else if let Some(i) = picked.strip_prefix("remove-project:").and_then(index) {
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


/// What a right click on a thread offers, and what it runs.
/// A thread's colours to pick from, `current` checked: those of `COLORS`, or none.
pub(super) fn thread_colors(current: Option<&str>) -> PopupMenu {
    let mut colors = PopupMenu::new();
    for (name, _) in COLORS {
        colors = colors.item(PopupItem::new(format!("color:{name}"), title_case(name)).checked(current == Some(name)));
    }
    colors.separator().item(PopupItem::new("color:", "No color").checked(current.is_none()))
}

/// A thread's right-click: renaming it, peeking at it, silencing it, putting it away, its
/// colour; moving it to another of `folders` (by id, with its name), or one of its own.
fn thread_menu(s: &Session, folders: &[(String, String)]) -> PopupMenu {
    let menu = PopupMenu::new()
        .item(PopupItem::new("rename", "Rename…"))
        .item(PopupItem::new("peek", "Peek"))
        .item(PopupItem::new("silence", silence_label(s.silent)))
        .item(if s.away { PopupItem::new("bring-back", "Bring back") } else { PopupItem::new("put-away", "Put away") })
        .submenu("Color", thread_colors(s.color.as_deref()));
    let now = folder_of(s);
    let mut to = PopupMenu::new();
    for (id, name) in folders {
        to = to.item(PopupItem::new(format!("folder:{id}"), name.clone()).checked(now == *id));
    }
    to = to.separator().item(PopupItem::new("new-folder", "New folder"));
    menu.separator().submenu("Move to folder", to)
}

fn thread_picked(s: &mut Lemur, picked: &str, key: Key) {
    let silent = s.session_of(&key).is_some_and(|t| t.silent);
    match picked {
        "rename" => s.ask_name(Ask::Rename { host: key.0, session: key.1 }),
        "peek" => s.peek(key),
        "silence" => s.set_silent(key, !silent),
        "bring-back" => s.bring_back(&key),
        "put-away" => s.put_away(key),
        "new-folder" => s.move_to_folder(&key, None),
        folder if folder.starts_with("folder:") => {
            let id = &folder["folder:".len()..];
            s.move_to_folder(&key, Some(id));
        }
        color if color.starts_with("color:") => {
            let name = &color["color:".len()..];
            s.set_thread_color(key, Some(name).filter(|n| !n.is_empty()));
        }
        _ => {}
    }
}

/// A thread's row, `depth` in: its agent's icon, its name, then the issue pinned to it, what
/// helps it and its age; its dot, a × under the pointer. Its right-click moves it among
/// `folders`, its host's.
fn thread_row(s: &Lemur, host: &str, t: &Session, place: Option<(usize, usize, &List)>, folders: &[(String, String)], k: &Kit) -> El {
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
        .avatar(thread_icon(t.agent(), t.color.as_deref(), k))
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

/// A host's rows: each folder with its threads, then those put away and those gone.
/// Dragged, a thread or a folder takes another place among them (`folders::moved`).
fn host_rows(s: &Lemur, h: &HostView, k: &Kit) -> Vec<El> {
    let host = h.name.as_str();
    let l = listed(h);
    let folded = |id: &str| s.folded(host, id);
    let count = l.items(folded).len();
    let list = Lemur::host_list(host);
    let folders: Vec<(String, String)> = l.groups.iter().map(|g| g.id.clone()).zip(s.folder_titles(h, &l.groups)).collect();
    let mut rows = vec![];
    let mut at = 0;
    for (g, (_, name)) in l.groups.iter().zip(&folders) {
        rows.push(folder_row(s, host, g, name.clone(), (at, count, &list), k));
        at += 1;
        if !folded(&g.id) {
            for t in &g.threads {
                rows.push(thread_row(s, host, t, Some((at, count, &list)), &folders, k));
                at += 1;
            }
        }
    }
    rows.extend(away_rows(s, host, &list, &l.away, &folders, k));
    rows.extend(l.gone.iter().map(|t| gone_row(t, k)));
    rows
}

/// A folder's row, as Rex's session headings are: its name, muted, and, folded, how many of
/// each kind run in it. A click folds or opens it, a right-click renames it; dragged, it
/// takes another place among the folders, its threads with it.
fn folder_row(s: &Lemur, host: &str, g: &Group, name: String, (at, count, list): (usize, usize, &List), k: &Kit) -> El {
    let folded = s.folded(host, &g.id);
    let counts = (folded && !g.threads.is_empty()).then(|| counts(&g.threads, k));
    let trailing = row().gap(6.0).center_y().children(counts).child(super::meter::chevron(folded, k).key("fold"));
    let mut r = SidebarRow::new(name).trailing(trailing);
    if folded && g.threads.iter().any(|t| t.activity == Activity::NeedsInput) {
        r = r.status(RowStatus::Attention);
    }
    let state = if s.row_drag_list == *list { s.row_drag } else { Reorder::new().gap(1.0) };
    let dropped = list.clone();
    r = r.reorder(&state, at, count, |s: &mut Lemur| &mut s.row_drag, move |s, from, to| {
        s.folder_dropped = Some(std::time::Instant::now());
        s.reorder(dropped.clone(), from, to);
    });
    let (toggle, menu_id, host) = (g.id.clone(), g.id.clone(), host.to_string());
    let (fold_host, menu_host) = (host.clone(), host.clone());
    let hovered = list.clone();
    r.on_select(move |s: &mut Lemur, _| {
        // Let go after a drag, it isn't a click.
        if s.folder_dropped.take().is_none_or(|at| at.elapsed() > std::time::Duration::from_millis(500)) {
            s.toggle_folder(&fold_host, &toggle);
        }
    })
    .build(k)
    .key(format!("folder-{host}-{}", g.id))
    .expanded(!folded)
    .on_hover_change(move |s: &mut Lemur, inside, tx: &mut Tx| {
        if inside && s.row_drag.dragging().is_none() {
            s.row_drag_list = hovered.clone();
            tx.live_only();
        }
    })
    .on_secondary_press(move |_: &mut Lemur, press: PointerPress, tx: &mut Tx| {
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
        .child(icons::button(&icons::PLUS, "New thread (⌘K)").on_press(|s: &mut Lemur| s.start_thread()).build(k).key("new-thread"))
        .child(icons::button(&icons::REFRESH, "Refresh").on_press(|s: &mut Lemur| s.refresh()).build(k).key("refresh"))
        .child(toggle(true, k));
    let mut sb = Sidebar::new().label("Threads").row_gap(1.0);
    if !s.loaded {
        sb = sb.child(SidebarRow::new("Loading sessions…").build(&named(k)).key("loading").opacity(0.6));
    }
    // The host shown's section: its folders of threads, or why it couldn't be reached.
    if let Some(h) = s.shown_host() {
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
            // A project it couldn't look at.
            for b in h.bases.iter().filter(|b| b.error.is_some()) {
                let said = format!("{}: {}", s.cfg.bases[b.index].name(), b.error.as_deref().unwrap_or_default());
                rows.push(SidebarRow::new(said).icon(&icons::TRIANGLE_ALERT).build(&named(k)).key(format!("error-{}", b.index)).opacity(0.7));
            }
            rows.extend(host_rows(s, h, k));
        }
        let label = s.cfg.host(&h.name).map_or_else(|| h.name.clone(), super::picker::host_label);
        let section = col().key(format!("host-section-{}", h.name)).role("group").label(label).w_full().gap(2.0).child(host_header(s, &h.name, k));
        sb = sb.child(section.children(rows));
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

    /// Colours thread `key`, or with `None` takes its colour off: it shows at once, herdr-core
    /// keeps it on the terminal, and a thread kept to resume keeps it too.
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
