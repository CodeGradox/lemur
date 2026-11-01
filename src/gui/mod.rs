//! The window: a sidebar of sessions next to a live terminal.
//!
//! Every session you open keeps its own terminal, attached through
//! herdr-core, so switching is instant and scrollback survives.

mod agents;
mod away;
mod changes;
mod changes_panel;
mod chrome;
mod cycle;
mod dictation;
mod folder_picker;
mod goto;
mod icons;
pub(crate) mod keys;
mod linear_panel;
mod links;
mod meter;
mod new;
mod notes;
mod notification;
mod palette;
mod peek;
mod projects;
mod prompt;
mod resume;
mod rows;
mod settings;
mod sidebar;
mod snackbar;
mod spotlight;
mod terminal;
mod terminal_view;

use crate::actions;
use crate::attach::Active;
use crate::config::{Config, Host, ThemeChoice, leaf};
use crate::dictate;
use crate::hc;
use crate::snapshot::{self, Activity, HostData, Session, Snapshot, Worktree};
use dictation::DictationBar;
use folder_picker::FolderPicker;
use futures::{FutureExt, StreamExt};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::InputState;
use gpui_kit::component::resizable::{HandleEdge, resize_handle};
use gpui_kit::component::{
    ActiveTheme, IconName, Selectable, Sizable, Theme, WindowExt, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use notes::NotesPanel;
use notification::{Notification, NotificationEvent};
use settings::Prefs;
use sidebar::{SIDEBAR_WIDTH, SidebarView};
use snackbar::Kind;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;
use terminal_view::{Pasted, StatusChanged, TerminalView};

actions!(
    lemur,
    [
        Quit,
        ToggleSidebar,
        ToggleNotes,
        ToggleAgents,
        ToggleRightPanel,
        NextAgent,
        PreviousAgent,
        CommandPalette,
        PutAway,
        ToggleChanges,
        ToggleLinear,
        UpdateCore,
        OpenSettings,
        AddProject,
        Refresh,
        Hide,
        HideOthers,
        ShowAll,
        NextThread,
        PreviousThread,
        NewThreadHere,
        NewTerminal,
        GoToThread,
        CloseThread,
        Dictate,
        PasteDictation,
        About
    ]
);

/// lemur's keys; menus show them next to their items.
fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
        KeyBinding::new("cmd-o", AddProject, None),
        KeyBinding::new("cmd-b", ToggleSidebar, None),
        // As Zed's right dock; Refresh does without a key.
        KeyBinding::new("cmd-r", ToggleRightPanel, None),
        KeyBinding::new("cmd-shift-a", ToggleAgents, None),
        // As Herdr's; the notes indent with them, as they take them first.
        KeyBinding::new("cmd-]", NextAgent, None),
        KeyBinding::new("cmd-[", PreviousAgent, None),
        KeyBinding::new("cmd-shift-p", CommandPalette, None),
        KeyBinding::new("ctrl-tab", NextThread, None),
        KeyBinding::new("ctrl-shift-tab", PreviousThread, None),
        // A new tab, as in a terminal app; ⌘W closes the one on show.
        KeyBinding::new("cmd-t", NewTerminal, None),
        KeyBinding::new("cmd-j", NewThreadHere, None),
        KeyBinding::new("cmd-w", CloseThread, None),
        KeyBinding::new("cmd-p", GoToThread, None),
        KeyBinding::new("cmd-d", Dictate, None),
        // Text fields take ⌘⏎ themselves: the notes tick a box with it, and
        // the dictation bar pastes too.
        KeyBinding::new("cmd-enter", PasteDictation, None),
        KeyBinding::new("cmd-w", settings::CloseWindow, Some(settings::CONTEXT)),
        // gpui-component's Root moves focus on Tab; in a terminal it belongs to the program.
        KeyBinding::new("tab", NoAction, Some(terminal_view::CONTEXT)),
        KeyBinding::new("shift-tab", NoAction, Some(terminal_view::CONTEXT)),
    ]);
    dictation::bind_keys(cx);
    folder_picker::bind_keys(cx);
    notes::bind_keys(cx);
    prompt::bind_keys(cx);
    palette::bind_keys(cx);
}

const REFRESH: Duration = Duration::from_secs(3);
/// What closing a shell with jobs ends.
const JOBS: &str =
    "The terminal ends, and the jobs its shell started (sent off with &, or stopped with ⌃Z).";
/// How long changes herdr-core tells of are gathered into one snapshot.
const BURST: Duration = Duration::from_millis(30);
/// Out of sight, the sidebar refreshes every this many REFRESH ticks.
const AWAY: u32 = 5;

/// What shows on the right of the terminal, one at a time.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Right {
    Notes,
    Linear,
    Changes,
    Agents,
}

/// The notes panel's width until its edge is dragged.
const NOTES_WIDTH: Pixels = px(360.0);
/// The narrowest a dragged side panel gets, and the room it leaves the
/// terminal.
const MIN_SIDEBAR: Pixels = px(180.0);
const MIN_NOTES: Pixels = px(240.0);
const MIN_TERMINAL: Pixels = px(320.0);

/// A side panel, resized by dragging its inner edge.
#[derive(Clone, Copy)]
enum Panel {
    Sidebar,
    Notes,
}

/// The drag of a side panel's edge; the window follows it and sets the
/// width. It draws nothing at the pointer.
#[derive(Clone)]
struct ResizePanel(Panel);

impl Render for ResizePanel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

type Key = (String, String);
/// A list of threads: a host's worktree, by path, or with `None` the host's
/// sessions outside every base.
type List = (String, Option<String>);

/// What the name field in the header is asking for.
enum Ask {
    NewWorktree {
        base: usize,
    },
    /// Claude, or with `shell` just a shell, in an existing worktree.
    NewThread {
        base: usize,
        worktree: String,
        shell: bool,
    },
    Rename {
        host: String,
        session: String,
    },
    /// Nothing new: a thread to go to.
    Find,
}

/// A name being asked for, in the spotlight.
struct Prompt {
    what: Ask,
    input: Entity<InputState>,
    /// For a new worktree, its branch and what it starts from.
    worktree: Option<prompt::WorktreeFields>,
    /// Why the name typed can't be used, and that name.
    error: Option<(String, String)>,
    /// The thread picked to go to, among those listed.
    go: Option<usize>,
    _events: Subscription,
}

struct Lemur {
    cfg: Config,
    /// Where to remember the session you last opened; tests have none.
    state_file: Option<PathBuf>,
    snap: Snapshot,
    /// What the last refresh found on each host, which terminals are filed
    /// under as they change.
    data: Vec<HostData>,
    loaded: bool,
    terminals: HashMap<Key, Entity<TerminalView>>,
    active: Option<Key>,
    /// Threads by when they were last opened, the latest first.
    recent: Vec<Key>,
    /// ⌃Tab going through a worktree's threads: in which order, and where
    /// it's at. It ends when ⌃ comes up.
    cycling: Option<(Vec<Key>, usize)>,
    collapsed: bool,
    /// The side panels' widths; the config keeps them once a drag ends.
    sidebar_width: Pixels,
    notes_width: Pixels,
    prompt: Option<Prompt>,
    /// Threads just started and opened, kept open until a snapshot lists
    /// them, by how many more snapshots may leave them out.
    fresh: HashMap<Key, u8>,
    /// Sessions that finished while you were looking at another one.
    done: HashSet<Key>,
    /// What each thread was doing at the last snapshot, or before its host
    /// couldn't be reached.
    seen: HashMap<Key, Activity>,
    /// Shown in a snackbar over the terminal.
    status: Option<(String, Kind)>,
    /// The session from last time, opened once the first refresh is in.
    restore: Option<Key>,
    /// The main window, for background work that ends in UI changes.
    window: AnyWindowHandle,
    /// Each host's last load, and the CPU share between it and the one
    /// before.
    loads: HashMap<String, (crate::machine::Load, Option<f32>)>,
    /// How much of Claude's plan is used, at the foot of the sidebar.
    meter: meter::Meter,
    /// Each worktree's lines added and removed, for the status bar.
    diffs: changes::Changes,
    /// Notification popups that are showing, one per session.
    popups: HashMap<Key, (WindowHandle<Notification>, Subscription)>,
    /// Counts changes a snapshot fetched across one would undo (the thread
    /// order, a removed project); such a snapshot is dropped.
    changes: u64,
    /// The notes of the last worktree opened, beside the terminal.
    notes: Entity<NotesPanel>,
    /// Every Claude thread and what it runs.
    agents: Entity<agents::AgentsView>,
    /// The worktree on show's changes, to commit.
    changes_panel: Entity<changes_panel::ChangesPanel>,
    /// The Linear issue pinned to the thread on show, and issues to pin.
    linear_panel: Entity<linear_panel::LinearPanel>,
    /// Which of them shows on the right, if any; one at a time.
    right: Option<Right>,
    /// The one that showed last, which ⌘R brings back.
    right_last: Right,
    /// Adding a folder, over the window.
    picker: Option<(Entity<FolderPicker>, Subscription)>,
    /// Every command by name, over the window.
    palette: Option<palette::Palette>,
    /// The lists whose "Put away" is open, by their key.
    away_open: HashSet<SharedString>,
    /// A thread peeked at, over the one on show.
    peek: Option<Key>,
    /// Lemur is the active window: only then do working dots breathe, as
    /// each breath redraws the window.
    looking: bool,
    /// Dictating, under the terminal.
    dictation: Option<(Entity<DictationBar>, Subscription)>,
    /// lemur-dictate, which dictation runs; tests have a stand-in.
    helper: PathBuf,
    /// The sidebar, drawn by `sidebar()` but as a view of its own, so a
    /// terminal's output doesn't redraw it.
    sidebar: Entity<SidebarView>,
    _refresh: Task<()>,
    _watch: Task<()>,
    _meter: Task<()>,
    _changes: Task<()>,
    _meter_setting: Subscription,
    /// Focused when no terminal is, so keys and menu actions reach the
    /// window.
    focus: FocusHandle,
    _appearance: Subscription,
    _sidebar_follows: Subscription,
    _back: Subscription,
    _lost: Subscription,
}

pub fn run(cfg: Config) -> anyhow::Result<()> {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.set_global(Prefs::new(&cfg, Config::path().ok()));
            settings::apply_theme(None, cx);
            bind_keys(cx);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.on_action(|_: &OpenSettings, cx| settings::open(cx));
            cx.on_action(|_: &Hide, cx| cx.hide());
            cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
            cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
            cx.on_action(|_: &About, cx| about(cx));
            // Menus as macOS apps have them; shortcuts come from bind_keys. The
            // File and View items act on the main window, and are greyed out
            // while another window, like Settings, is in front.
            cx.set_menus([
                Menu::new("Lemur").items([
                    MenuItem::action("About Lemur", About),
                    MenuItem::separator(),
                    MenuItem::action("Settings…", OpenSettings),
                    MenuItem::separator(),
                    MenuItem::os_submenu("Services", SystemMenuType::Services),
                    MenuItem::separator(),
                    MenuItem::action("Hide Lemur", Hide),
                    MenuItem::action("Hide Others", HideOthers),
                    MenuItem::action("Show All", ShowAll),
                    MenuItem::separator(),
                    MenuItem::action("Update herdr-core", UpdateCore),
                    MenuItem::separator(),
                    MenuItem::action("Quit Lemur", Quit),
                ]),
                Menu::new("File").items([
                    MenuItem::action("Add Project…", AddProject),
                    MenuItem::action("New Terminal", NewTerminal),
                    MenuItem::action("New Thread in Worktree…", NewThreadHere),
                    MenuItem::action("Go to Thread…", GoToThread),
                    MenuItem::action("Close Thread", CloseThread),
                    MenuItem::action("Put Thread Away", PutAway),
                    MenuItem::action("Dictate", Dictate),
                    MenuItem::separator(),
                    MenuItem::action("Refresh", Refresh),
                ]),
                Menu::new("View").items([
                    MenuItem::action("Command Palette…", CommandPalette),
                    MenuItem::separator(),
                    MenuItem::action("Toggle Sidebar", ToggleSidebar),
                    MenuItem::action("Toggle Right Panel", ToggleRightPanel),
                    MenuItem::action("Toggle Notes", ToggleNotes),
                    MenuItem::action("Toggle Linear", ToggleLinear),
                    MenuItem::action("Toggle Changes", ToggleChanges),
                    MenuItem::action("Toggle Agents", ToggleAgents),
                    MenuItem::separator(),
                    MenuItem::action("Next Thread in Worktree", NextThread),
                    MenuItem::action("Previous Thread in Worktree", PreviousThread),
                    MenuItem::action("Next Agent", NextAgent),
                    MenuItem::action("Previous Agent", PreviousAgent),
                ]),
            ]);
            let options = WindowOptions {
                // Drawn by the top row instead, as Herdr's; it moves the window.
                titlebar: Some(TitlebarOptions {
                    title: Some("Lemur".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(chrome::WINDOW_BUTTONS_AT),
                }),
                app_owns_titlebar_drag: true,
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1280.0), px(820.0)),
                    cx,
                ))),
                ..Default::default()
            };
            let (main, _) = gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| Lemur::new(cfg, Active::path().ok(), window, cx))
            })
            .expect("failed to open window");
            // Closing the main window quits; notification popups come and go.
            let main = main.window_id();
            cx.on_window_closed(move |cx, closed| {
                if closed == main {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
    Ok(())
}

/// Says which version this is, in an alert over the window in front.
fn about(cx: &mut App) {
    let Some(window) = cx.active_window().or_else(|| cx.windows().first().copied()) else {
        return;
    };
    let version = format!("Lemur {}", env!("CARGO_PKG_VERSION"));
    window
        .update(cx, |_, window, cx| {
            // OK is the only answer; nothing waits for it.
            drop(window.prompt(PromptLevel::Info, &version, None, &["OK"], cx));
        })
        .ok();
}

impl Lemur {
    fn new(
        cfg: Config,
        state_file: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Notes live beside the state file, in ~/.lemur/notes; tests have neither.
        let notes_dir = state_file
            .as_deref()
            .and_then(Path::parent)
            .map(|dir| dir.join("notes"));
        let notes = cx.new(|cx| NotesPanel::new(notes_dir, window, cx));
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let lemur = cx.weak_entity();
        let sidebar = cx.new(|_| SidebarView::new(lemur.clone()));
        let agents = cx.new(|_| agents::AgentsView::new(lemur));
        let changes_panel = cx.new(|cx| changes_panel::ChangesPanel::new(window, cx));
        cx.subscribe(
            &changes_panel,
            |this, _, _: &changes_panel::Committed, cx| this.ask_changes(cx),
        )
        .detach();
        // Tests set their own stand-in for Linear first.
        if cx.try_global::<linear_panel::LinearAccount>().is_none() {
            linear_panel::init(crate::linear::Linear::new(), cx);
        }
        let linear_panel = cx.new(|cx| linear_panel::LinearPanel::new(window, cx));
        cx.subscribe_in(
            &linear_panel,
            window,
            |this, _, event: &linear_panel::LinearEvent, window, cx| match event {
                linear_panel::LinearEvent::Pin(key, issue) => {
                    this.pin_issue(key.clone(), issue.clone(), cx)
                }
                linear_panel::LinearEvent::Send(key, text) => {
                    this.paste_into(key, text, window, cx)
                }
            },
        )
        .detach();
        let sidebar_follows = cx.observe_self(|this, cx| {
            this.sidebar.update(cx, |_, cx| cx.notify());
            if this.right == Some(Right::Agents) {
                this.agents.update(cx, |_, cx| cx.notify());
            }
        });
        let restore = state_file
            .as_deref()
            .and_then(|p| Active::load_from(p).ok())
            .map(|a| (a.host, a.session));
        // Every tick while you look at lemur; out of sight, where a refresh
        // still costs each host a `claude agents`, every AWAY ticks.
        let handle = window.window_handle();
        let refresh = cx.spawn(async move |this, cx| {
            let mut since = AWAY;
            loop {
                let looking = handle
                    .update(cx, |_, window, _| window.is_window_active())
                    .unwrap_or(false);
                if looking || since >= AWAY {
                    let Ok(refreshed) = this.update(cx, |this, cx| this.refresh(cx)) else {
                        break;
                    };
                    refreshed.await;
                    since = 0;
                }
                cx.background_executor().timer(REFRESH).await;
                since += 1;
            }
        });
        // How much of Claude's plan is used, once it's turned on, as
        // `usage::wait` says.
        let asking = cx.spawn(async move |this, cx| {
            loop {
                let looking = handle
                    .update(cx, |_, window, _| window.is_window_active())
                    .unwrap_or(false);
                if this
                    .update(cx, |this, cx| this.tick_meter(looking, cx))
                    .is_err()
                {
                    break;
                }
                cx.background_executor().timer(meter::TICK).await;
            }
        });
        let meter_setting = cx.observe_global::<Prefs>(|this, cx| this.follow_meter_setting(cx));
        // The worktree on show's changes, every `changes::EVERY` while you
        // look at Lemur; out of sight, every `changes::AWAY`.
        let changing = cx.spawn(async move |this, cx| {
            let mut since = changes::AWAY;
            loop {
                let looking = handle
                    .update(cx, |_, window, _| window.is_window_active())
                    .unwrap_or(false);
                if looking || since >= changes::AWAY {
                    if this
                        .update(cx, |this, cx| {
                            if this.loaded {
                                this.ask_changes(cx);
                                if this.right == Some(Right::Changes) {
                                    this.changes_panel.update(cx, |p, cx| p.refresh(cx));
                                }
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                    since = Duration::ZERO;
                }
                cx.background_executor().timer(changes::EVERY).await;
                since += changes::EVERY;
            }
        });
        // herdr-core tells of every change to a terminal as it happens:
        // started, ended, relabelled, what Claude says it's doing.
        let watch = cx.spawn(async move |this, cx| {
            loop {
                let Ok(cfg) = this.read_with(cx, |this, _| this.cfg.clone()) else {
                    break;
                };
                let (cores, all) = cx.background_spawn(async move { watched(&cfg) }).await;
                if cores.is_empty() {
                    cx.background_executor().timer(REFRESH).await;
                    continue;
                }
                // Each core's changes, then word that it's lost; with a host
                // not reached, word to look again in a while. Watching again
                // connects to them, while the other cores' changes go on.
                let mut streams: Vec<futures::stream::LocalBoxStream<'static, bool>> = cores
                    .iter()
                    .map(|c| {
                        c.changes()
                            .map(|()| true)
                            .chain(futures::stream::once(async { false }))
                            .boxed_local()
                    })
                    .collect();
                if !all {
                    let timer = cx.background_executor().timer(REFRESH * AWAY);
                    let again = async move {
                        timer.await;
                        false
                    };
                    streams.push(futures::stream::once(again).boxed_local());
                }
                let mut changes = futures::stream::select_all(streams);
                while let Some(true) = changes.next().await {
                    // A burst of changes makes one new snapshot.
                    cx.background_executor().timer(BURST).await;
                    let mut lost = false;
                    while let Some(Some(changed)) = changes.next().now_or_never() {
                        lost |= !changed;
                    }
                    let Ok(recomposed) = this.update(cx, |this, cx| this.recompose(cx)) else {
                        return;
                    };
                    recomposed.await;
                    if lost {
                        break;
                    }
                }
                // Lost a core: connect again in a while.
                cx.background_executor().timer(REFRESH).await;
            }
        });
        // Follow macOS when it switches between light and dark.
        let appearance = cx.observe_window_appearance(window, |_, window, cx| {
            if Prefs::get(cx).theme == ThemeChoice::System {
                Theme::sync_system_appearance(Some(window), cx);
            }
        });
        // Back in the window, the shown session's popup has had its say, as
        // in Zed.
        let back = cx.observe_window_activation(window, |this, window, cx| {
            this.looking = window.is_window_active();
            cx.notify();
            if window.is_window_active()
                && let Some(key) = this.active.clone()
            {
                this.close_popup(&key, cx);
            }
        });
        // Whatever had the keyboard went from the window, as a closed
        // thread's terminal does, which a closing dialog hands it back to:
        // it goes to the thread on show.
        let lost = cx.on_focus_lost(window, |this, window, cx| this.focus_terminal(window, cx));
        let sidebar_width = cfg.sidebar_width.map_or(SIDEBAR_WIDTH, px).max(MIN_SIDEBAR);
        let notes_width = cfg.notes_width.map_or(NOTES_WIDTH, px).max(MIN_NOTES);
        Lemur {
            cfg,
            state_file,
            snap: Snapshot::default(),
            data: vec![],
            loaded: false,
            terminals: HashMap::new(),
            active: None,
            recent: vec![],
            cycling: None,
            collapsed: false,
            sidebar_width,
            notes_width,
            prompt: None,
            fresh: HashMap::new(),
            done: HashSet::new(),
            seen: HashMap::new(),
            status: None,
            restore,
            window: window.window_handle(),
            popups: HashMap::new(),
            loads: HashMap::new(),
            meter: meter::Meter::new(cx),
            diffs: changes::Changes::default(),
            changes: 0,
            notes,
            agents,
            changes_panel,
            linear_panel,
            right: None,
            right_last: Right::Notes,
            picker: None,
            palette: None,
            away_open: HashSet::new(),
            peek: None,
            looking: window.is_window_active(),
            dictation: None,
            helper: dictate::helper(),
            sidebar,
            _refresh: refresh,
            _meter: asking,
            _changes: changing,
            _meter_setting: meter_setting,
            _watch: watch,
            focus,
            _appearance: appearance,
            _sidebar_follows: sidebar_follows,
            _back: back,
            _lost: lost,
        }
    }

    /// Attaches again each terminal whose stream dropped, once its program
    /// runs again, as Herdr does: after the line to its host or to
    /// herdr-core comes back, or a kept program's new shell starts.
    fn reconnect_terminals(&self, cx: &mut Context<Self>) {
        let running: HashSet<Key> = sessions(&self.snap)
            .flat_map(|(h, s)| {
                let own = s.running.then(|| key(h, s));
                // A helper's terminal, opened or peeked at from under it.
                let helpers = s.helpers.iter().filter(|x| x.terminal);
                own.into_iter()
                    .chain(helpers.map(move |x| (h.to_string(), x.id.clone())))
            })
            .collect();
        for (k, view) in &self.terminals {
            if running.contains(k) && view.read(cx).disconnected() {
                view.update(cx, |v, cx| v.reconnect(cx));
            }
        }
    }

    /// Runs `work` off the main thread, then `then` with its result on the
    /// main window; that's skipped if the window has closed.
    fn in_background<R: Send + 'static>(
        &self,
        cx: &mut Context<Self>,
        work: impl FnOnce() -> R + Send + 'static,
        then: impl FnOnce(&mut Self, R, &mut Window, &mut Context<Self>) + 'static,
    ) -> Task<()> {
        let window = self.window;
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { work() }).await;
            window
                .update(cx, |_, window, cx| {
                    this.update(cx, |this, cx| then(this, result, window, cx))
                        .ok();
                })
                .ok();
        })
    }

    /// Shows what went wrong, until it's closed.
    fn fail(&mut self, e: anyhow::Error, cx: &mut Context<Self>) {
        self.status = Some((format!("{e:#}"), Kind::Error));
        cx.notify();
    }

    /// Shows `text` for a while.
    fn note(&mut self, text: String, cx: &mut Context<Self>) {
        let shown = Some((text, Kind::Note));
        self.status = shown.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(snackbar::NOTE_TIME).await;
            this.update(cx, |this, cx| {
                if this.status == shown {
                    this.status = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Fetches a snapshot in the background and applies it.
    fn refresh(&self, cx: &mut Context<Self>) -> Task<()> {
        let cfg = self.cfg.clone();
        let changes = self.changes;
        self.in_background(
            cx,
            move || {
                let data = snapshot::fetch_data(&cfg);
                let snap = snapshot::compose(&cfg, &data);
                (data, snap)
            },
            move |this, (data, snap), window, cx| {
                if this.changes == changes {
                    this.data = data;
                    this.set_snapshot(snap, window, cx)
                }
            },
        )
    }

    /// Files the terminals as they are now under what the last refresh
    /// found, as herdr-core tells of a change.
    fn recompose(&self, cx: &mut Context<Self>) -> Task<()> {
        if !self.loaded {
            return Task::ready(());
        }
        let (cfg, data) = (self.cfg.clone(), self.data.clone());
        let changes = self.changes;
        self.in_background(
            cx,
            move || snapshot::compose(&cfg, &data),
            move |this, snap, window, cx| {
                if this.changes == changes {
                    this.set_snapshot(snap, window, cx)
                }
            },
        )
    }

    fn set_snapshot(&mut self, snap: Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        let now: HashMap<Key, &Session> = sessions(&snap).map(|(h, s)| (key(h, s), s)).collect();
        // A thread just started may not be listed yet.
        self.fresh.retain(|k, left| {
            *left = left.saturating_sub(1);
            !now.contains_key(k) && *left > 0
        });
        // Forget terminals whose session is gone (killed, or ended elsewhere).
        // A host that couldn't be reached keeps its own: they aren't known
        // to be gone, and reattach once it's back.
        let fresh = &self.fresh;
        let unreached: HashSet<&str> = snap
            .hosts
            .iter()
            .filter(|h| h.error.is_some())
            .map(|h| h.name.as_str())
            .collect();
        // A helper's terminal opened from under its thread.
        let helpers: HashSet<Key> = sessions(&snap)
            .flat_map(|(h, s)| {
                s.helpers
                    .iter()
                    .filter(|x| x.terminal)
                    .map(move |x| (h.to_string(), x.id.clone()))
            })
            .collect();
        let live = |k: &Key| {
            now.contains_key(k)
                || fresh.contains_key(k)
                || helpers.contains(k)
                || unreached.contains(k.0.as_str())
        };
        // What can show instead of a thread that ended: not one put away,
        // nor one only kept for a host that can't be reached.
        let showable = |k: &Key| {
            now.get(k).is_some_and(|s| !s.away) || fresh.contains_key(k) || helpers.contains(k)
        };
        // Whether the thread on show has the keyboard, before its terminal
        // may go.
        let had_keyboard = self
            .active
            .as_ref()
            .and_then(|k| self.terminals.get(k))
            .is_some_and(|v| v.read(cx).focus_handle(cx).is_focused(window));
        self.terminals.retain(|k, _| live(k));
        if self.peek.as_ref().is_some_and(|k| !live(k)) {
            self.peek = None;
        }
        // The thread on show ended: another takes its place.
        let mut instead = None;
        if let Some(gone) = self.active.clone().filter(|a| !live(a)) {
            self.active = None;
            instead = self.successor(&gone, showable);
        }
        // What each thread did last. A host that can't be reached keeps its
        // own: back, its threads haven't just finished or come to need you.
        let before = std::mem::take(&mut self.seen);
        // Working -> idle is "finished"; anything -> waiting is "needs input".
        let mut news = vec![];
        for (k, s) in &now {
            let was = before.get(k).copied().unwrap_or(Activity::Unknown);
            let finished = was == Activity::Working
                && matches!(s.activity, Activity::Idle | Activity::Unknown);
            if finished && self.active.as_ref() != Some(k) {
                self.done.insert(k.clone());
            }
            if finished || (s.activity == Activity::NeedsInput && was != Activity::NeedsInput) {
                news.push((k.clone(), s.activity, s.waiting_for.clone()));
            }
        }
        self.seen = before
            .into_iter()
            .filter(|(k, _)| unreached.contains(k.0.as_str()) && !now.contains_key(k))
            .chain(now.iter().map(|(k, s)| (k.clone(), s.activity)))
            .collect();
        let kept = |k: &Key| unreached.contains(k.0.as_str()) && !now.contains_key(k);
        self.done
            .retain(|k| kept(k) || now.get(k).is_some_and(|s| s.activity != Activity::Working));
        let gone: Vec<Key> = self
            .popups
            .keys()
            .filter(|k| !now.contains_key(*k) && !kept(k))
            .cloned()
            .collect();
        // Not one put away since: it stays away across restarts.
        let restore = self
            .restore
            .take()
            .filter(|k| now.get(k).is_some_and(|s| !s.away) && self.active.is_none());
        drop(now);
        for k in gone {
            self.close_popup(&k, cx);
        }
        self.snap = snap;
        self.bring_back_needed(cx);
        self.reconnect_terminals(cx);
        self.note_claude(cx);
        self.follow_loads();
        // With the hosts known, the meter can ask now, not at its next tick.
        let first = !self.loaded;
        self.loaded = true;
        if first {
            self.tick_meter(window.is_window_active(), cx);
            self.ask_changes(cx);
            self.check_core_updates(cx);
        }
        self.keep_claude_ids(cx);
        self.note_shells(cx);
        self.follow_linear(cx);
        self.notify_news(news, window, cx);
        if let Some((host, session)) = restore {
            self.open(host, session, window, cx);
        } else if let Some((host, session)) = instead {
            // It takes the keyboard only from the one that ended: typing in
            // the notes, a commit message or a spotlight goes on there.
            self.show_thread(host, session, had_keyboard, window, cx);
        }
        self.follow_nothing(cx);
        cx.notify();
    }

    /// With no thread on show, the Changes panel says so, rather than
    /// showing, and committing in, the worktree of the one that was.
    fn follow_nothing(&mut self, cx: &mut Context<Self>) {
        if self.active.is_none() && self.right == Some(Right::Changes) {
            self.show_changes(cx);
        }
    }

    /// What shows once `gone`, the thread on show, has ended: the one opened
    /// last among the threads it was with (its worktree's, or its host's
    /// outside every worktree), else its neighbour there, else the one
    /// opened last anywhere. `live` says what still runs.
    fn successor(&self, gone: &Key, live: impl Fn(&Key) -> bool) -> Option<Key> {
        let with = self.siblings(gone);
        let at = with.iter().position(|k| k == gone).unwrap_or(with.len());
        let after = with.iter().skip(at + 1);
        let before = with.iter().take(at).rev();
        self.recent
            .iter()
            .filter(|k| *k != gone && live(k))
            .find(|k| with.contains(k))
            .or_else(|| after.chain(before).find(|k| live(k)))
            .or_else(|| self.recent.iter().find(|k| *k != gone && live(k)))
            .cloned()
    }

    /// Pops up a notification for each session that finished or needs input,
    /// except the one you're looking at in the active window.
    fn notify_news(
        &mut self,
        news: Vec<(Key, Activity, Option<String>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Decided once: opening a popup mustn't change the answer.
        let looking = window.is_window_active();
        for (key, activity, waiting_for) in news {
            if looking && self.active.as_ref() == Some(&key) {
                continue;
            }
            let Some(screen) = cx.primary_display() else {
                return;
            };
            let host = &key.0;
            let (place, session) = self.titles(host, &key.1);
            let (title, caption, icon) = match activity {
                Activity::NeedsInput => (
                    format!("{session} needs input"),
                    waiting_for.unwrap_or_else(|| host.clone()),
                    IconName::CircleAlert,
                ),
                _ => (
                    format!("{session} finished"),
                    host.clone(),
                    IconName::CircleCheck,
                ),
            };
            let caption = match place {
                Some(place) => format!("{place}  ·  {caption}"),
                None => caption,
            };
            // A newer event for the same session replaces its popup.
            self.close_popup(&key, cx);
            let slot = self.popups.len();
            let Ok(popup) = cx
                .open_window(Notification::window_options(screen, slot), |window, cx| {
                    cx.new(|cx| Notification::new(title, caption, icon, window, cx))
                })
            else {
                continue;
            };
            let Ok(entity) = popup.entity(cx) else {
                continue;
            };
            let target = key.clone();
            let events = cx.subscribe_in(
                &entity,
                window,
                move |this, _, event: &NotificationEvent, window, cx| {
                    if let NotificationEvent::Focused = event {
                        // The keyboard goes back to the main window; the
                        // popup stays until it's answered.
                        window.activate_window();
                        return;
                    }
                    if let NotificationEvent::Show = event {
                        cx.activate(true);
                        window.activate_window();
                        this.open(target.0.clone(), target.1.clone(), window, cx);
                    }
                    this.close_popup(&target, cx);
                },
            );
            self.popups.insert(key, (popup, events));
            window.request_attention();
        }
    }

    fn close_popup(&mut self, key: &Key, cx: &mut App) {
        if let Some((popup, _events)) = self.popups.remove(key) {
            popup.update(cx, |_, window, _| window.remove_window()).ok();
        }
    }

    /// Opens a thread just started, which the sidebar may not list yet.
    fn open_fresh(
        &mut self,
        host: String,
        session: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.fresh.insert((host.clone(), session.clone()), 3);
        self.open(host, session, window, cx);
    }

    /// The terminal view of thread `key` on host `h`, made and attached
    /// the first time, and attached again once its stream ended.
    fn terminal_for(&mut self, key: &Key, h: Host, cx: &mut Context<Self>) -> Entity<TerminalView> {
        let session = key.1.clone();
        let view = self
            .terminals
            .entry(key.clone())
            .or_insert_with(|| {
                let view = cx.new(|cx| TerminalView::new(h, session, cx));
                // Its message shows in the window's snackbars.
                cx.subscribe(&view, |_, _, _: &StatusChanged, cx| cx.notify())
                    .detach();
                let pasted_in = key.clone();
                cx.subscribe(&view, move |this, _, Pasted(text): &Pasted, cx| {
                    this.pasted(&pasted_in, text, cx)
                })
                .detach();
                view
            })
            .clone();
        if view.read(cx).exited() {
            view.update(cx, |v, cx| v.connect(cx));
        }
        view
    }

    fn open(&mut self, host: String, session: String, window: &mut Window, cx: &mut Context<Self>) {
        self.show_thread(host, session, true, window, cx);
    }

    /// Shows a thread, and with `focus` gives it the keyboard.
    fn show_thread(
        &mut self,
        host: String,
        session: String,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(h) = self.cfg.host(&host).cloned() else {
            return;
        };
        let key = (host.clone(), session.clone());
        self.hide_shown(Some(&key), cx);
        let view = self.terminal_for(&key, h, cx);
        // Peeked at, it's shown now.
        if self.peek.as_ref() == Some(&key) {
            self.peek = None;
        }
        self.done.remove(&key);
        self.close_popup(&key, cx);
        self.reveal(&host, &session, cx);
        // ⌃Tab keeps the order it started with until ⌃ comes up.
        if self.cycling.is_none() {
            self.touch(key.clone());
        }
        self.bring_back(&key, cx);
        self.active = Some(key);
        self.ask_changes(cx);
        if self.right == Some(Right::Changes) {
            self.show_changes(cx);
        }
        self.follow_linear(cx);
        if let Some((worktree, label)) = self.worktree_of(&host, &session) {
            self.notes
                .update(cx, |n, cx| n.show(&host, &worktree, &label, window, cx));
        }
        self.remember(host, session);
        if focus {
            view.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    /// Tells the thread on show's program it's no longer shown, unless it's
    /// `next`.
    fn hide_shown(&mut self, next: Option<&Key>, cx: &mut Context<Self>) {
        if let Some(shown) = self.active.as_ref().filter(|k| Some(*k) != next)
            && let Some(view) = self.terminals.get(shown)
        {
            view.update(cx, |v, _| v.hidden());
        }
    }

    /// The project (its place in the config) and worktree a session runs in.
    fn project_of(&self, host: &str, session: &str) -> Option<(usize, &Worktree)> {
        self.snap
            .hosts
            .iter()
            .filter(|h| h.name == host)
            .flat_map(|h| &h.bases)
            .find_map(|b| {
                let w = b
                    .worktrees
                    .iter()
                    .find(|w| w.sessions.iter().any(|s| s.name == session))?;
                Some((b.index, w))
            })
    }

    /// A session as the sidebar shows it: `project › worktree`, none outside
    /// every project, and the thread's name there.
    fn titles(&self, host: &str, session: &str) -> (Option<String>, String) {
        let thread = self.label_of(host, session);
        match self.project_of(host, session) {
            Some((index, w)) => {
                let project = self.cfg.bases[index].name();
                (Some(format!("{project} › {}", worktree_label(w))), thread)
            }
            None => (None, thread),
        }
    }

    /// The worktree a session runs in, and its label.
    fn worktree_of(&self, host: &str, session: &str) -> Option<(String, String)> {
        self.snap
            .worktrees(host)
            .find(|w| w.sessions.iter().any(|s| s.name == session))
            .map(|w| (w.path.clone(), worktree_label(w)))
    }

    /// Opens the notes panel on a worktree's notes.
    fn open_notes(
        &mut self,
        host: &str,
        worktree: &str,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.notes
            .update(cx, |n, cx| n.show(host, worktree, label, window, cx));
        self.right = Some(Right::Notes);
        self.right_last = Right::Notes;
        cx.notify();
    }

    /// Gives the keyboard back to the session on show, after a spotlight.
    fn focus_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active.as_ref().and_then(|k| self.terminals.get(k)) {
            Some(view) => view.read(cx).focus_handle(cx).focus(window, cx),
            None => self.focus.focus(window, cx),
        }
        cx.notify();
    }

    fn remember(&self, host: String, session: String) {
        if let Some(path) = &self.state_file {
            let _ = Active { host, session }.save_to(path);
        }
    }

    /// Moves a thread within its list: the sidebar shows the new order at
    /// once, and herdr-core keeps it.
    fn reorder(&mut self, list: List, from: usize, to: usize, cx: &mut Context<Self>) {
        let Some(sessions) = self.list_mut(&list) else {
            return;
        };
        if !move_item(sessions, from, to) {
            return;
        }
        let names: Vec<String> = sessions.iter().map(|s| s.name.clone()).collect();
        self.changes += 1;
        cx.notify();
        let cfg = self.cfg.clone();
        let host = list.0;
        self.in_background(
            cx,
            move || actions::set_order(&cfg, &host, &names),
            |this, saved, _, cx| {
                // Refreshes that started before herdr-core had the order are stale too.
                this.changes += 1;
                if let Err(e) = saved {
                    this.fail(e, cx);
                }
                this.refresh(cx).detach();
            },
        )
        .detach();
    }

    fn list_mut(&mut self, (host, worktree): &List) -> Option<&mut Vec<Session>> {
        let h = self.snap.hosts.iter_mut().find(|h| &h.name == host)?;
        match worktree {
            None => Some(&mut h.other),
            Some(path) => h
                .bases
                .iter_mut()
                .flat_map(|b| b.worktrees.iter_mut())
                .find(|w| &w.path == path)
                .map(|w| &mut w.sessions),
        }
    }

    /// ⌘W: asks whether to close the thread on show.
    fn confirm_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.prompt.is_some() || self.picker.is_some() {
            return;
        }
        // The peek is in front, not the thread on show under it.
        if self.peek.is_some() {
            self.close_peek(window, cx);
            return;
        }
        if let Some(key) = self.active.clone() {
            self.confirm_close_of(key, window, cx);
        }
    }

    /// Closes the thread `key`, asking first while something other than
    /// its shell runs in it, in front or as a job of the shell's.
    pub(super) fn confirm_close_of(
        &mut self,
        key: Key,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (label, claude) = match self.session_of(&key) {
            Some(s) if !s.busy => {
                // A shell at its prompt closes at once, unless jobs it
                // started still run or wait (`&`, ⌃Z): they'd end too.
                let (cfg, label, pid) = (self.cfg.clone(), s.label.clone(), s.pid);
                let host = key.0.clone();
                self.in_background(
                    cx,
                    move || actions::shell_jobs(&cfg, &host, pid),
                    move |this, jobs, window, cx| match jobs {
                        Ok(0) => this.close_thread(key, cx),
                        _ => this.ask_close(key, label, JOBS, window, cx),
                    },
                )
                .detach();
                return;
            }
            Some(s) => (s.label.clone(), s.runs_claude()),
            None => (self.label_of(&key.0, &key.1), false),
        };
        let description = if claude {
            "Claude quits, and its terminal ends. Lemur forgets the conversation, as it does for a thread killed."
        } else {
            "The terminal ends, and whatever runs in it."
        };
        self.ask_close(key, label, description, window, cx);
    }

    /// Asks whether to close thread `key`, called `label`, saying what that
    /// ends.
    fn ask_close(
        &mut self,
        key: Key,
        label: String,
        description: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (this, key) = (this.clone(), key.clone());
            alert
                .title(format!("Close {label}?"))
                .description(description)
                .show_cancel(true)
                .ok_text("Close")
                .on_ok(move |_, _, cx| {
                    this.update(cx, |this, cx| this.close_thread(key.clone(), cx))
                        .ok();
                    true
                })
        });
    }

    /// Ends a thread's terminal; the worktree stays.
    fn close_thread(&mut self, key: Key, cx: &mut Context<Self>) {
        let cfg = self.cfg.clone();
        let (host, session) = key;
        let killed = (host.clone(), vec![session.clone()]);
        self.in_background(
            cx,
            move || actions::kill_session(&cfg, &host, &session),
            move |this, result, _, cx| {
                match result {
                    // Killed, it's not to come back.
                    Ok(()) => this.forget_threads(&killed.0, &killed.1, cx),
                    Err(e) => this.fail(e, cx),
                }
                this.refresh(cx).detach();
            },
        )
        .detach();
    }

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.collapsed = !self.collapsed;
        cx.notify();
    }

    /// The notes, Linear, the changes and the agents take turns on the
    /// right, as Herdr's Files and Changes do: showing one hides the other,
    /// and showing the one that shows hides it.
    fn toggle_right(&mut self, which: Right, cx: &mut Context<Self>) {
        if self.right == Some(which) {
            self.right = None;
        } else {
            self.right = Some(which);
            self.right_last = which;
            if which == Right::Changes {
                self.show_changes(cx);
            }
            if which == Right::Linear {
                self.follow_linear(cx);
            }
        }
        cx.notify();
    }

    fn toggle_notes(&mut self, cx: &mut Context<Self>) {
        self.toggle_right(Right::Notes, cx);
    }

    fn toggle_agents(&mut self, cx: &mut Context<Self>) {
        self.toggle_right(Right::Agents, cx);
    }

    /// ⌘R: hides the right panel, or brings back the one shown last; the
    /// notes the first time.
    fn toggle_right_panel(&mut self, cx: &mut Context<Self>) {
        match self.right {
            Some(_) => {
                self.right = None;
                cx.notify();
            }
            None => self.toggle_right(self.right_last, cx),
        }
    }

    /// Whether `which` shows on the right.
    #[cfg(test)]
    fn shows(&self, which: Right) -> bool {
        self.right == Some(which)
    }

    /// Says so when a newer herdr-core is out than runs here or on a
    /// machine. Tests don't ask the release site.
    fn check_core_updates(&mut self, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        let cores = crate::core_update::Cores::of(&self.cfg);
        self.in_background(cx, move || crate::core_update::check(&cores), |this, versions, _, cx| {
            let Ok(versions) = versions else { return };
            let outdated = versions.outdated();
            if let (false, Some(latest)) = (outdated.is_empty(), &versions.latest) {
                let note = format!(
                    "herdr-core {latest} is out ({}): ⌘⇧P → Update herdr-core, or Settings → herdr-core",
                    versions.describe(&outdated)
                );
                this.note(note, cx);
            }
        })
        .detach();
    }

    /// Updates herdr-core here, then on each machine behind, as Herdr does;
    /// terminals keep running.
    fn update_core(&mut self, cx: &mut Context<Self>) {
        self.status = Some(("Updating herdr-core…".into(), Kind::Progress));
        cx.notify();
        let cores = crate::core_update::Cores::of(&self.cfg);
        self.in_background(
            cx,
            move || {
                let versions = crate::core_update::check(&cores)?;
                if versions.latest.is_none() {
                    anyhow::bail!("couldn't ask for herdr-core's latest release");
                }
                let outdated = versions.outdated();
                if outdated.is_empty() {
                    return Ok(format!(
                        "herdr-core is up to date ({})",
                        versions.here.unwrap_or_default()
                    ));
                }
                crate::core_update::update(&outdated, &cores)
            },
            |this, done, _, cx| match done {
                Ok(note) => this.note(note, cx),
                Err(e) => this.fail(e, cx),
            },
        )
        .detach();
    }

    /// Points the Changes panel at the worktree on show, and asks it.
    fn show_changes(&mut self, cx: &mut Context<Self>) {
        let place = self.active_worktree().and_then(|(host, w)| {
            let h = self.cfg.host(host)?.clone();
            Some((h, w.path.clone(), worktree_label(w)))
        });
        self.changes_panel.update(cx, |p, cx| match place {
            Some((h, path, label)) => p.show(h, path, label, cx),
            None => p.clear(cx),
        });
    }

    /// Points the Linear panel at the thread on show, while it shows.
    fn follow_linear(&mut self, cx: &mut Context<Self>) {
        if self.right != Some(Right::Linear) {
            return;
        }
        let thread = self.active.clone().map(|key| {
            let (label, issue) = match self.session_of(&key) {
                Some(s) => (s.label.clone(), s.issue.clone()),
                None => (self.label_of(&key.0, &key.1), None),
            };
            (key, label, issue)
        });
        self.linear_panel.update(cx, |p, cx| p.show(thread, cx));
    }

    /// Pins the Linear issue `issue` (its key) to thread `key`, or with
    /// `None` unpins its issue: shown at once, and herdr-core keeps it.
    fn pin_issue(&mut self, key: Key, issue: Option<String>, cx: &mut Context<Self>) {
        if let Some(s) = self.session_mut(&key) {
            s.issue = issue.clone();
        }
        // Kept to resume it, it's pinned again then: whether or not its
        // Claude runs, which keeps the entry current otherwise.
        let kept = self
            .cfg
            .threads
            .iter()
            .find(|t| t.host == key.0 && t.session == key.1)
            .cloned();
        if let Some(thread) = kept {
            let issue = issue.clone();
            self.keep_thread(crate::config::ClaudeThread { issue, ..thread }, cx);
        }
        self.changes += 1;
        self.follow_linear(cx);
        cx.notify();
        let cfg = self.cfg.clone();
        self.in_background(
            cx,
            move || actions::set_issue(&cfg, &key.0, &key.1, issue.as_deref()),
            |this, saved, _, cx| {
                // Refreshes that started before herdr-core had it are stale too.
                this.changes += 1;
                if let Err(e) = saved {
                    this.fail(e, cx);
                }
                this.refresh(cx).detach();
            },
        )
        .detach();
    }

    /// A link to a Linear issue pasted into a thread pins that issue to
    /// it, unless it has one.
    fn pasted(&mut self, key: &Key, text: &str, cx: &mut Context<Self>) {
        let Some(issue) = crate::linear::issue_in(text) else {
            return;
        };
        if self.session_of(key).is_some_and(|s| s.issue.is_none()) {
            self.pin_issue(key.clone(), Some(issue), cx);
        }
    }

    /// Pastes `text` at thread `key`'s prompt, as if typed there, and gives
    /// it the keyboard; Enter is left to you.
    fn paste_into(&mut self, key: &Key, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.terminals.get(key) else {
            return;
        };
        view.read(cx).paste_text(text);
        view.read(cx).focus_handle(cx).focus(window, cx);
    }

    fn session_mut(&mut self, (host, name): &Key) -> Option<&mut Session> {
        let h = self.snap.hosts.iter_mut().find(|h| &h.name == host)?;
        h.bases
            .iter_mut()
            .flat_map(|b| b.worktrees.iter_mut())
            .flat_map(|w| w.sessions.iter_mut())
            .chain(h.other.iter_mut())
            .find(|s| &s.name == name)
    }

    /// Follows the pointer (`x`) dragging a side panel's edge across the
    /// window (`bounds`).
    fn resize_panel(
        &mut self,
        panel: Panel,
        x: Pixels,
        bounds: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let shown = |open: bool, width: Pixels| if open { width } else { px(0.0) };
        match panel {
            Panel::Sidebar => {
                let right = self.right.is_some();
                let room = bounds.size.width - shown(right, self.notes_width) - MIN_TERMINAL;
                self.sidebar_width = panel_width(x - bounds.left(), MIN_SIDEBAR, room);
            }
            Panel::Notes => {
                let room =
                    bounds.size.width - shown(!self.collapsed, self.sidebar_width) - MIN_TERMINAL;
                self.notes_width = panel_width(bounds.right() - x, MIN_NOTES, room);
            }
        }
        cx.notify();
    }

    /// Keeps the side panels' widths in the config file once a drag has
    /// changed them; lemur's own widths are left out of it.
    fn save_panel_widths(&mut self, cx: &mut Context<Self>) {
        let kept = |width: Pixels, default: Pixels| (width != default).then(|| f32::from(width));
        let widths = (
            kept(self.sidebar_width, SIDEBAR_WIDTH),
            kept(self.notes_width, NOTES_WIDTH),
        );
        if widths == (self.cfg.sidebar_width, self.cfg.notes_width) {
            return;
        }
        (self.cfg.sidebar_width, self.cfg.notes_width) = widths;
        if let Some(file) = Prefs::get(cx).config_file()
            && let Err(e) = Config::save_panel_widths(file, widths.0, widths.1)
        {
            self.fail(e, cx);
        }
    }

    fn header(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let toggle = Button::new("toggle-sidebar")
            .icon(if self.collapsed {
                IconName::PanelLeftOpen
            } else {
                IconName::PanelLeftClose
            })
            .ghost()
            .small()
            .tooltip("Toggle sidebar (⌘B)")
            .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx)));
        // With the sidebar shut, the window's buttons are on this row.
        let buttons = self.collapsed && !window.is_fullscreen();
        chrome::title_bar("top bar", window, cx)
            .h_10()
            .px_2()
            .when(buttons, |this| this.pl(chrome::WINDOW_BUTTONS))
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(toggle)
            .child(self.tabs(cx))
            // As Herdr's Files and Changes: each shows its panel, or hides it.
            .child(
                Button::new("toggle-notes")
                    .icon(IconName::FileText)
                    .label("Notes")
                    .ghost()
                    .small()
                    .selected(self.right == Some(Right::Notes))
                    .tooltip("Toggle notes")
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_notes(cx))),
            )
            .child(
                Button::new("toggle-linear")
                    .icon(icons::ticket())
                    .label("Linear")
                    .ghost()
                    .small()
                    .selected(self.right == Some(Right::Linear))
                    .tooltip("Toggle Linear")
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_right(Right::Linear, cx))),
            )
            .child(
                Button::new("toggle-changes")
                    .icon(icons::file_diff())
                    .label("Changes")
                    .ghost()
                    .small()
                    .selected(self.right == Some(Right::Changes))
                    .tooltip("Toggle changes")
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_right(Right::Changes, cx))),
            )
            .child(
                Button::new("toggle-agents")
                    .icon(gpui_kit::assets::IconName::Bot)
                    .label("Agents")
                    .ghost()
                    .small()
                    .selected(self.right == Some(Right::Agents))
                    .tooltip("Toggle agents (⌘⇧A)")
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_agents(cx))),
            )
    }

    /// The window's message above the shown terminal's, at the bottom of
    /// the terminal.
    fn snackbars(&self, cx: &mut Context<Self>) -> Div {
        let mut snacks = vec![];
        if let Some((text, kind)) = &self.status {
            let this = cx.entity().downgrade();
            snacks.push(snackbar::snackbar(
                "status",
                text.clone(),
                *kind,
                move |_, cx| {
                    this.update(cx, |this, cx| {
                        this.status = None;
                        cx.notify();
                    })
                    .ok();
                },
                cx,
            ));
        }
        let terminal = self.active.as_ref().and_then(|k| self.terminals.get(k));
        if let Some(terminal) = terminal
            && let Some((text, kind)) = terminal.read(cx).status()
        {
            let view = terminal.downgrade();
            snacks.push(snackbar::snackbar(
                "terminal-status",
                text.clone(),
                *kind,
                move |_, cx| {
                    view.update(cx, |view, cx| view.dismiss_status(cx)).ok();
                },
                cx,
            ));
        }
        snackbar::stack(snacks)
    }
}

impl Render for Lemur {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let main = match self.active.as_ref().and_then(|k| self.terminals.get(k)) {
            // Cached: sidebar redraws (the breathing dot) don't re-render the
            // terminal; it re-renders on its own output, size or focus change.
            Some(view) => {
                let focused = view.read(cx).focus_handle(cx).is_focused(window);
                let terminal = view
                    .clone()
                    .cached(StyleRefinement::default().size_full())
                    .into_any_element();
                self.framed(terminal, focused, cx)
            }
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child(if self.loaded {
                    "Pick a session, or start one with +"
                } else {
                    "Loading sessions…"
                }),
        };
        let this = cx.entity().downgrade();
        let picker = self.picker.as_ref().map(|(picker, _)| {
            let this = this.clone();
            spotlight::overlay(picker.clone(), move |window, cx| {
                this.update(cx, |this, cx| this.close_picker(window, cx))
                    .ok();
            })
        });
        let prompt = self.prompt.as_ref().map(|prompt| {
            let this = this.clone();
            spotlight::overlay(self.prompt_card(prompt, cx), move |window, cx| {
                this.update(cx, |this, cx| this.close_prompt(window, cx))
                    .ok();
            })
        });
        let peek = self.peek_card(cx);
        let palette = self.palette_card(window, cx).map(|card| {
            let this = this.clone();
            spotlight::overlay(card, move |window, cx| {
                this.update(cx, |this, cx| this.close_palette(window, cx))
                    .ok();
            })
        });
        h_flex()
            .relative()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_sidebar(cx)))
            .on_action(cx.listener(|this, _: &ToggleNotes, _, cx| this.toggle_notes(cx)))
            .on_action(cx.listener(|this, _: &ToggleRightPanel, _, cx| this.toggle_right_panel(cx)))
            .on_action(cx.listener(|this, _: &ToggleAgents, _, cx| this.toggle_agents(cx)))
            .on_action(
                cx.listener(|this, _: &ToggleChanges, _, cx| this.toggle_right(Right::Changes, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ToggleLinear, _, cx| this.toggle_right(Right::Linear, cx)),
            )
            .on_action(cx.listener(|this, _: &UpdateCore, _, cx| this.update_core(cx)))
            .on_action(
                cx.listener(|this, _: &CommandPalette, window, cx| this.open_palette(window, cx)),
            )
            .on_action(cx.listener(|this, _: &PutAway, window, cx| {
                if let Some(k) = this.active.clone() {
                    this.put_away(k, window, cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &NextAgent, window, cx| this.next_agent(false, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &PreviousAgent, window, cx| {
                    this.next_agent(true, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &AddProject, window, cx| this.open_picker(window, cx)))
            .on_action(cx.listener(|this, _: &Refresh, _, cx| this.refresh(cx).detach()))
            .on_action(
                cx.listener(|this, _: &NewThreadHere, window, cx| this.new_thread_here(window, cx)),
            )
            .on_action(cx.listener(|this, _: &NewTerminal, _, cx| this.new_terminal_here(cx)))
            .on_action(cx.listener(|this, _: &GoToThread, window, cx| this.find_thread(window, cx)))
            .on_action(
                cx.listener(|this, _: &CloseThread, window, cx| this.confirm_close(window, cx)),
            )
            .on_action(cx.listener(|this, _: &Dictate, window, cx| this.dictate(window, cx)))
            .on_action(cx.listener(|this, _: &PasteDictation, _, cx| this.paste_dictation(cx)))
            .on_action(
                cx.listener(|this, _: &NextThread, window, cx| {
                    this.cycle_threads(false, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &PreviousThread, window, cx| {
                this.cycle_threads(true, window, cx)
            }))
            .on_modifiers_changed(cx.listener(|this, e: &ModifiersChangedEvent, _, _| {
                if !e.modifiers.control {
                    this.end_cycle();
                }
            }))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                if e.keystroke.key == "escape" && this.prompt.is_some() {
                    this.close_prompt(window, cx);
                    cx.stop_propagation();
                } else if e.keystroke.key == "escape" && this.palette.is_some() {
                    this.close_palette(window, cx);
                    cx.stop_propagation();
                }
            }))
            .on_drag_move(cx.listener(|this, e: &DragMoveEvent<ResizePanel>, _, cx| {
                let panel = e.drag(cx).0;
                this.resize_panel(panel, e.event.position.x, e.bounds, cx);
            }))
            // A drag ends wherever the button comes up: on the handle, which
            // hides the window behind it from the pointer, too.
            .capture_any_mouse_up(cx.listener(|this, _, _, cx| this.save_panel_widths(cx)))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.save_panel_widths(cx)),
            )
            .when(!self.collapsed, |this| {
                this.child(
                    div()
                        .relative()
                        .h_full()
                        .w(self.sidebar_width)
                        .flex_shrink_0()
                        .child(
                            self.sidebar
                                .clone()
                                .cached(StyleRefinement::default().size_full()),
                        )
                        .child(panel_edge(
                            "sidebar-edge",
                            Panel::Sidebar,
                            HandleEdge::Trailing,
                        )),
                )
            })
            .child(
                v_flex()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .child(self.header(window, cx))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_h_0()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .child(
                                        div()
                                            .relative()
                                            .flex_1()
                                            .min_h_0()
                                            .child(main)
                                            .child(self.snackbars(cx)),
                                    )
                                    .children(self.dictation.as_ref().map(|(bar, _)| bar.clone()))
                                    .child(self.status_bar(cx)),
                            )
                            .when_some(self.right, |this, right| {
                                // Cached like the terminal; each redraws on its own changes.
                                let panel: AnyView = match right {
                                    Right::Notes => self.notes.clone().into(),
                                    Right::Agents => self.agents.clone().into(),
                                    Right::Changes => self.changes_panel.clone().into(),
                                    Right::Linear => self.linear_panel.clone().into(),
                                };
                                this.child(
                                    div()
                                        .relative()
                                        .w(self.notes_width)
                                        .h_full()
                                        .flex_shrink_0()
                                        .child(panel.cached(StyleRefinement::default().size_full()))
                                        // Its line is the panel's left border.
                                        .child(panel_edge(
                                            "notes-edge",
                                            Panel::Notes,
                                            HandleEdge::Leading,
                                        )),
                                )
                            }),
                    ),
            )
            .children(picker)
            .children(peek)
            .children(prompt)
            .children(palette)
            // Holds the window's focus with no terminal open. It takes no room
            // and can't be clicked, so clicks elsewhere keep their focus.
            .child(div().absolute().track_focus(&self.focus))
    }
}

/// A dragged side panel's width: `wanted` in whole pixels, at most `room`
/// and at least `min`; in a narrow window `min` wins.
fn panel_width(wanted: Pixels, min: Pixels, room: Pixels) -> Pixels {
    wanted.round().min(room).max(min)
}

/// The band on a side panel's inner `edge` that resizes it.
fn panel_edge(id: &'static str, panel: Panel, edge: HandleEdge) -> impl IntoElement {
    resize_handle(id, Axis::Horizontal)
        .inside(edge)
        .on_drag(ResizePanel(panel), |drag, _, _, cx| {
            cx.new(|_| drag.clone())
        })
}

/// Moves `items[from]` to `to`. False if that's no move, or either is out
/// of range.
fn move_item<T>(items: &mut Vec<T>, from: usize, to: usize) -> bool {
    if from == to || from >= items.len() || to >= items.len() {
        return false;
    }
    let moved = items.remove(from);
    items.insert(to, moved);
    true
}

/// Where the item that was at `index` is once `move_item` has moved the
/// one at `from` to `to`.
fn moved_to(index: usize, from: usize, to: usize) -> usize {
    if index == from {
        to
    } else if from < index && index <= to {
        index - 1
    } else if to <= index && index < from {
        index + 1
    } else {
        index
    }
}

/// The cores the config's hosts' terminals run in, each once, and whether
/// every host was reached.
fn watched(cfg: &Config) -> (Vec<std::sync::Arc<hc::Core>>, bool) {
    let mut cores: Vec<std::sync::Arc<hc::Core>> = vec![];
    let mut all = true;
    for host in &cfg.hosts {
        match hc::for_host(host) {
            Ok((core, _)) => {
                if !cores.iter().any(|c| std::sync::Arc::ptr_eq(c, &core)) {
                    cores.push(core);
                }
            }
            Err(_) => all = false,
        }
    }
    (cores, all)
}

fn sessions(snap: &Snapshot) -> impl Iterator<Item = (&str, &Session)> {
    snap.hosts.iter().flat_map(|h| {
        let in_bases = h
            .bases
            .iter()
            .flat_map(|b| b.worktrees.iter())
            .flat_map(|w| w.sessions.iter());
        in_bases
            .chain(h.other.iter())
            .map(move |s| (h.name.as_str(), s))
    })
}

/// A worktree's name in the sidebar: its branch, which tells worktrees
/// apart even when their folders share a name. `worktree-` is dropped from
/// the branches lemur and `claude -w` made, leaving the name you gave. A
/// detached worktree shows its folder's name, the one you gave lemur.
fn worktree_label(w: &Worktree) -> String {
    match (&w.branch, &w.head) {
        (Some(branch), _) => branch
            .strip_prefix("worktree-")
            .unwrap_or(branch)
            .to_string(),
        (None, Some(_)) => format!("{} (detached)", leaf(&w.path)),
        (None, None) => leaf(&w.path).to_string(),
    }
}

fn key(host: &str, s: &Session) -> Key {
    (host.to_string(), s.name.clone())
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's `test` macro would shadow the built-in one.
    use super::folder_picker::HostChoice;
    use super::settings::Prefs;
    use super::snackbar::Kind;
    use super::{
        Ask, Key, Lemur, MIN_NOTES, MIN_SIDEBAR, MIN_TERMINAL, NOTES_WIDTH, NotificationEvent,
        Right, SIDEBAR_WIDTH, meter, move_item, moved_to,
    };
    use crate::config::{Base, Config, Host};
    use crate::snapshot::{self, Activity, BaseView, HostView, Session, Snapshot, Worktree};
    use crate::test_support::{HcServer, TempDir, fake_dictate, git_in, wait_until};
    use gpui::VisualTestContext;
    use gpui_kit::component::WindowExt;
    use gpui_kit::{
        AppContext, Entity, Modifiers, MouseButton, MouseExitEvent, MouseMoveEvent, Pixels,
        PlatformInput, point, px, size,
    };
    use std::time::Duration;

    fn key(s: &str) -> Key {
        ("local".into(), s.into())
    }

    fn snapshot(sessions: &[(&str, Activity)]) -> Snapshot {
        let other = sessions
            .iter()
            .map(|(name, activity)| Session {
                name: name.to_string(),
                label: name.to_string(),
                path: "/tmp".into(),
                idle_secs: 0,
                activity: *activity,
                waiting_for: None,
                started_claude: false,
                order: None,
                started: 0,
                claude: None,
                busy: false,
                running: true,
                pid: None,
                shell: None,
                helpers: vec![],
                away: false,
                issue: None,
            })
            .collect();
        Snapshot {
            hosts: vec![HostView {
                name: "local".into(),
                error: None,
                home: String::new(),
                machine: Default::default(),
                bases: vec![],
                other,
                gone: vec![],
            }],
        }
    }

    /// Applies a snapshot the way a refresh does.
    fn apply(app: &Entity<Lemur>, sessions: &[(&str, Activity)], cx: &mut VisualTestContext) {
        let snap = snapshot(sessions);
        cx.update(|window, cx| app.update(cx, |app, cx| app.set_snapshot(snap, window, cx)));
    }

    fn open(app: &Entity<Lemur>, session: &str, cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open("local".into(), session.into(), window, cx)
            })
        });
    }

    /// The whole window: sidebar, header and the active terminal. Resizing
    /// the window must resize the terminal inside it.
    #[gpui::test]
    fn the_active_terminal_follows_the_window_size(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("app");
        let t = server.run("/", &[], &["sleep", "600"]);
        // The core's connection is real threads, so this test can't be
        // deterministic.
        let (app, cx) = rooted(server.config(), cx);
        open(&app, &t, cx);
        let view = app.read_with(cx, |app, _| app.terminals[&key(&t)].clone());

        cx.simulate_resize(size(px(900.0), px(600.0)));
        cx.run_until_parked();
        let small = view
            .read_with(cx, |v, _| v.screen_size())
            .expect("attached");
        cx.simulate_resize(size(px(1400.0), px(900.0)));
        cx.run_until_parked();
        let large = view
            .read_with(cx, |v, _| v.screen_size())
            .expect("attached");
        assert!(
            large.cols > small.cols && large.lines > small.lines,
            "{small:?} -> {large:?}"
        );
    }

    /// Dropping a thread on another moves it to that one's place: in the
    /// sidebar at once, and in herdr-core for good.
    #[gpui::test]
    fn dragging_a_thread_reorders_its_list(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("drag");
        let [a, b, c] = ["a", "b", "c"].map(|_| server.run("/", &[], &["sleep", "600"]));
        let (app, cx) = rooted(server.config(), cx);
        cx.simulate_resize(size(px(1000.0), px(600.0)));
        cx.run_until_parked();
        let shown = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, _| {
                app.snap.hosts[0]
                    .other
                    .iter()
                    .map(|s| s.name.clone())
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(shown(cx), [a.as_str(), &b, &c]);

        drag("thread-0", "thread-2", cx);
        assert_eq!(shown(cx), [b.as_str(), &c, &a]);

        cx.run_until_parked();
        let kept: Vec<String> = snapshot::fetch(&server.config()).hosts[0]
            .other
            .iter()
            .map(|s| s.name.clone())
            .collect();
        assert_eq!(kept, [b.as_str(), &c, &a]);
        assert_eq!(
            shown(cx),
            [b.as_str(), &c, &a],
            "the refresh after the move agrees"
        );
    }

    /// Dropping a tab on another moves it to that one's place, and its
    /// thread in the sidebar with it: in herdr-core for good.
    #[gpui::test]
    fn dragging_a_tab_reorders_its_threads(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("drag-tab");
        let [a, b, c] = ["a", "b", "c"]
            .map(|l| server.run("/", &[&format!("lemur.label={l}")], &["sleep", "600"]));
        let (app, cx) = rooted(server.config(), cx);
        cx.simulate_resize(size(px(1200.0), px(700.0)));
        open(&app, &a, cx);
        cx.run_until_parked();
        let shown = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, _| {
                let other = &app.snap.hosts[0].other;
                other.iter().map(|s| s.name.clone()).collect::<Vec<_>>()
            })
        };
        assert_eq!(shown(cx), [a.as_str(), &b, &c]);

        drag("tab-0", "tab-2", cx);
        assert_eq!(shown(cx), [b.as_str(), &c, &a]);
        assert_eq!(
            app.read_with(cx, |app, _| app.active.clone()),
            Some(key(&a)),
            "dragging it doesn't open another"
        );

        cx.run_until_parked();
        let kept: Vec<String> = snapshot::fetch(&server.config()).hosts[0]
            .other
            .iter()
            .map(|s| s.name.clone())
            .collect();
        assert_eq!(kept, [b.as_str(), &c, &a]);
        let_attaches_end(&app, cx);
    }

    /// Presses the mouse on `at` and moves it a little, as starting a drag.
    fn press_and_move(at: gpui::Point<Pixels>, cx: &mut VisualTestContext) {
        cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(
            at + point(px(3.0), px(0.0)),
            MouseButton::Left,
            Modifiers::none(),
        );
    }

    /// The window has no title bar; its top row moves it instead, from
    /// where nothing else takes the press. A test window can't move (its
    /// `start_window_move` is unimplemented), so that panic is the move.
    #[gpui::test]
    #[should_panic(expected = "not implemented")]
    fn dragging_the_top_bar_moves_the_window(cx: &mut gpui::TestAppContext) {
        let (_, cx) = rooted(toml::from_str("").unwrap(), cx);
        let empty = cx.debug_bounds("top bar").expect("the top bar").center();
        press_and_move(empty, cx);
    }

    /// The sidebar's top row too, where the window's buttons are.
    #[gpui::test]
    #[should_panic(expected = "not implemented")]
    fn dragging_the_sidebars_top_moves_the_window(cx: &mut gpui::TestAppContext) {
        let (_, cx) = rooted(toml::from_str("").unwrap(), cx);
        let empty = cx
            .debug_bounds("sidebar top bar")
            .expect("its top")
            .center();
        press_and_move(empty, cx);
    }

    /// A press on a top row's button is the button's: moving a little
    /// before letting go still clicks it, and doesn't move the window.
    #[gpui::test]
    fn a_top_bar_button_keeps_its_press(cx: &mut gpui::TestAppContext) {
        let (app, cx) = rooted(toml::from_str("").unwrap(), cx);
        let bar = cx.debug_bounds("top bar").expect("the top bar");
        // Toggle sidebar, the first thing on it.
        let toggle = point(bar.left() + px(20.0), bar.center().y);
        press_and_move(toggle, cx);
        cx.simulate_mouse_up(
            toggle + point(px(3.0), px(0.0)),
            MouseButton::Left,
            Modifiers::none(),
        );
        app.read_with(cx, |app, _| assert!(app.collapsed));
    }

    /// A terminal started elsewhere joins the sidebar as herdr-core tells
    /// of it, well before the next refresh; one that ends leaves it.
    #[gpui::test]
    fn the_sidebar_follows_the_core(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("live");
        let (app, cx) = rooted(server.config(), cx);
        let shown = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, _| {
                let other = &app.snap.hosts[0].other;
                other.iter().map(|s| s.label.clone()).collect::<Vec<_>>()
            })
        };
        // Changes are gathered for BURST; a refresh waits REFRESH.
        let follows = |want: &[&str], cx: &mut VisualTestContext| {
            for _ in 0..(super::REFRESH.as_millis() / super::BURST.as_millis() - 1) {
                cx.executor().advance_clock(super::BURST);
                cx.run_until_parked();
                if shown(cx) == want {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            false
        };
        assert!(shown(cx).is_empty());

        let t = server.run("/", &["lemur.label=late"], &["sleep", "600"]);
        assert!(follows(&["late"], cx), "{:?}", shown(cx));
        server.hc(&["label", &t, "lemur.label=renamed"]);
        assert!(follows(&["renamed"], cx), "{:?}", shown(cx));
        server.hc(&["keep", &t, "--off"]);
        server.hc(&["kill", &t]);
        assert!(follows(&[], cx), "{:?}", shown(cx));
    }

    /// Dragging a side panel's inner edge resizes it: down to its minimum,
    /// and up to what leaves the terminal its own. The config keeps the
    /// widths, but not lemur's own.
    #[gpui::test]
    fn dragging_a_side_panels_edge_resizes_it(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("resize-panels");
        let (app, cx) = rooted(server.config(), cx);
        cx.simulate_resize(size(px(1000.0), px(600.0)));
        cx.run_until_parked();
        let widths = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, _| (app.sidebar_width, app.notes_width))
        };
        let kept = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, _| (app.cfg.sidebar_width, app.cfg.notes_width))
        };
        // The bands lie just inside the panels' inner edges.
        let across = |from: Pixels, to: Pixels, cx: &mut VisualTestContext| {
            let (from, to) = (point(from, px(300.0)), point(to, px(300.0)));
            let none = Modifiers::none();
            cx.simulate_mouse_down(from, MouseButton::Left, none);
            cx.simulate_mouse_move(from + point(px(0.0), px(6.0)), MouseButton::Left, none);
            cx.simulate_mouse_move(to, MouseButton::Left, none);
            cx.simulate_mouse_up(to, MouseButton::Left, none);
        };

        across(SIDEBAR_WIDTH - px(2.0), px(400.4), cx);
        assert_eq!(widths(cx).0, px(400.0));
        assert_eq!(kept(cx), (Some(400.0), None));
        across(px(398.0), px(20.0), cx);
        assert_eq!(widths(cx).0, MIN_SIDEBAR);
        across(MIN_SIDEBAR - px(2.0), px(900.0), cx);
        assert_eq!(widths(cx).0, px(1000.0) - MIN_TERMINAL);
        across(px(1000.0) - MIN_TERMINAL - px(2.0), SIDEBAR_WIDTH, cx);
        assert_eq!(kept(cx), (None, None), "back at lemur's own width");

        app.update(cx, |app, cx| app.toggle_notes(cx));
        cx.run_until_parked();
        let edge = |width: Pixels| px(1000.0) - width + px(2.0);
        across(edge(NOTES_WIDTH), px(200.0), cx);
        assert_eq!(widths(cx).1, px(1000.0) - SIDEBAR_WIDTH - MIN_TERMINAL);
        across(edge(widths(cx).1), px(990.0), cx);
        assert_eq!(widths(cx).1, MIN_NOTES);
        assert_eq!(kept(cx), (None, Some(f32::from(MIN_NOTES))));
    }

    #[test]
    fn moved_to_follows_move_item() {
        for from in 0..4 {
            for to in 0..4 {
                let mut items: Vec<usize> = (0..4).collect();
                move_item(&mut items, from, to);
                for (place, &item) in items.iter().enumerate() {
                    assert_eq!(moved_to(item, from, to), place, "{from} -> {to}");
                }
            }
        }
    }

    /// Dropping a project's name on another's moves it to that place,
    /// whatever its host: in the sidebar, the config and the snapshot's
    /// views, before and after a refresh. A project's rows are part of it:
    /// dropped on one, it moves too.
    #[gpui::test]
    fn dragging_a_project_reorders_the_sidebar(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("move-project");
        let mut cfg = server.config();
        let mut mac = Host::new("mac", "local");
        mac.hc_session = cfg.hosts[0].hc_session.clone();
        cfg.hosts.push(mac);
        for (host, path) in [
            ("local", "/srv/one"),
            ("local", "/srv/two"),
            ("mac", "/srv/three"),
        ] {
            cfg.bases.push(Base::new(host, path));
        }
        let (app, cx) = rooted(cfg, cx);
        cx.simulate_resize(size(px(1000.0), px(600.0)));
        cx.run_until_parked();
        // The snapshot shown, and the one a herdr-core event would compose
        // from what the last refresh found.
        let consistent = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, _| {
                let recomposed = snapshot::compose(&app.cfg, &app.data);
                for h in app.snap.hosts.iter().chain(&recomposed.hosts) {
                    assert_eq!(h.bases.len(), if h.name == "mac" { 1 } else { 2 });
                    for b in &h.bases {
                        assert_eq!(app.cfg.bases[b.index].host, h.name);
                    }
                }
                app.cfg
                    .bases
                    .iter()
                    .map(|b| b.path.clone())
                    .collect::<Vec<_>>()
            })
        };
        let top = |project: &'static str, cx: &mut VisualTestContext| {
            cx.debug_bounds(project).expect(project).top()
        };
        assert_eq!(consistent(cx), ["/srv/one", "/srv/two", "/srv/three"]);

        drag("project three · mac", "project one · local", cx);
        cx.run_until_parked();
        assert_eq!(consistent(cx), ["/srv/three", "/srv/one", "/srv/two"]);
        assert!(top("project three · mac", cx) < top("project one · local", cx));
        assert!(top("project one · local", cx) < top("project two · local", cx));

        app.update(cx, |app, cx| app.refresh(cx).detach());
        cx.run_until_parked();
        assert_eq!(consistent(cx), ["/srv/three", "/srv/one", "/srv/two"]);

        // Its New worktree row, under its name.
        let from = cx.debug_bounds("project two · local").unwrap().center();
        let three = cx.debug_bounds("project three · mac").unwrap();
        let to = point(three.center().x, three.bottom() + px(20.0));
        let none = Modifiers::none();
        cx.simulate_mouse_down(from, MouseButton::Left, none);
        cx.simulate_mouse_move(from + point(px(0.0), px(6.0)), MouseButton::Left, none);
        cx.simulate_mouse_move(to, MouseButton::Left, none);
        cx.simulate_mouse_up(to, MouseButton::Left, none);
        cx.run_until_parked();
        assert_eq!(consistent(cx), ["/srv/two", "/srv/three", "/srv/one"]);
    }

    /// About Lemur says which version this is.
    #[gpui::test]
    fn about_says_the_version(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("about");
        let (_, cx) = rooted(server.config(), cx);
        cx.cx.update(super::about);
        let version = format!("Lemur {}", env!("CARGO_PKG_VERSION"));
        assert_eq!(cx.pending_prompt(), Some((version, String::new())));
    }

    /// ⌃Tab goes through the threads, the one opened last first: holding ⌃
    /// goes further, letting go settles there, and ⌃⇧Tab goes back. From
    /// the menu, with no ⌃, it goes one and settles.
    #[gpui::test]
    fn ctrl_tab_goes_through_threads_by_when_they_were_opened(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("cycle");
        let [a, b, c] = ["a", "b", "c"].map(|_| server.run("/", &[], &["sleep", "600"]));
        let (app, cx) = rooted(server.config(), cx);
        cx.update(|_, cx| super::bind_keys(cx));
        cx.simulate_resize(size(px(1000.0), px(600.0)));
        cx.run_until_parked();
        for name in [a.as_str(), &b, &c] {
            open(&app, name, cx);
        }
        let shown = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, _| app.active.clone().map(|(_, s)| s))
        };
        let (a, b, c) = (Some(a), Some(b), Some(c));
        // ⌃ goes down before Tab, as on a keyboard, and comes up after.
        let hold = |modifiers: Modifiers, cx: &mut VisualTestContext| {
            cx.simulate_modifiers_change(modifiers)
        };
        let let_go = |cx: &mut VisualTestContext| cx.simulate_modifiers_change(Modifiers::none());

        hold(Modifiers::control(), cx);
        cx.simulate_keystrokes("ctrl-tab");
        assert_eq!(shown(cx), b);
        cx.simulate_keystrokes("ctrl-tab");
        assert_eq!(shown(cx), a);
        let_go(cx);
        // a is the latest now, then c, then b.
        hold(Modifiers::control(), cx);
        cx.simulate_keystrokes("ctrl-tab");
        assert_eq!(shown(cx), c);
        let_go(cx);
        hold(Modifiers::control_shift(), cx);
        cx.simulate_keystrokes("ctrl-shift-tab");
        assert_eq!(shown(cx), b);
        let_go(cx);

        cx.dispatch_action(super::NextThread);
        assert_eq!(shown(cx), c);
        app.read_with(cx, |app, _| assert!(app.cycling.is_none(), "settled"));
    }

    /// Opening a thread points the notes at its worktree without opening
    /// the panel; Notes in a worktree's menu opens it.
    #[gpui::test]
    fn notes_follow_the_opened_thread(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("notes");
        let mut cfg = server.config();
        cfg.bases = toml::from_str::<Config>("[[bases]]\nhost = \"local\"\npath = \"/srv/api\"")
            .unwrap()
            .bases;
        cx.executor().allow_parking();
        cx.update(gpui_kit::init);
        let (app, cx) = cx.add_window_view(|window, cx| Lemur::new(cfg, None, window, cx));
        cx.run_until_parked();
        let worktree = |path: &str, branch: &str, sessions: &[&str]| Worktree {
            path: path.into(),
            branch: Some(branch.into()),
            head: None,
            main: false,
            sessions: snapshot(
                &sessions
                    .iter()
                    .map(|s| (*s, Activity::Idle))
                    .collect::<Vec<_>>(),
            )
            .hosts
            .remove(0)
            .other,
            gone: vec![],
        };
        let snap = Snapshot {
            hosts: vec![HostView {
                name: "local".into(),
                error: None,
                home: String::new(),
                machine: Default::default(),
                bases: vec![BaseView {
                    index: 0,
                    error: None,
                    plain: false,
                    worktrees: vec![
                        worktree("/srv/api", "main", &[]),
                        worktree(
                            "/srv/api/.claude/worktrees/auth",
                            "worktree-auth",
                            &["api/t"],
                        ),
                    ],
                }],
                other: vec![],
                gone: vec![],
            }],
        };
        cx.update(|window, cx| app.update(cx, |app, cx| app.set_snapshot(snap, window, cx)));
        let notes = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, cx| {
                let shown = app.notes.read(cx).worktree().map(str::to_string);
                (shown, app.shows(Right::Notes))
            })
        };
        assert_eq!(notes(cx), (None, false));

        open(&app, "api/t", cx);
        assert_eq!(
            notes(cx),
            (Some("/srv/api/.claude/worktrees/auth".into()), false)
        );
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_notes("local", "/srv/api", "main", window, cx)
            })
        });
        assert_eq!(notes(cx), (Some("/srv/api".into()), true));

        // Let the terminal's attach attempt end before the server cleans up.
        wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            app.read_with(cx, |app, cx| {
                app.terminals.values().all(|t| t.read(cx).exited())
            })
        });
    }

    #[test]
    fn worktrees_are_named_after_their_branch() {
        let worktree = |branch: Option<&str>, head: Option<&str>| super::Worktree {
            path: "/srv/web/.worktrees/equal-burrow/web".into(),
            branch: branch.map(str::to_string),
            head: head.map(str::to_string),
            main: false,
            sessions: vec![],
            gone: vec![],
        };
        let label = |b, h| super::worktree_label(&worktree(b, h));
        assert_eq!(
            label(Some("feature/cleaning-app-react"), Some("abc")),
            "feature/cleaning-app-react"
        );
        assert_eq!(label(Some("worktree-auth"), None), "auth");
        assert_eq!(label(None, Some("0123456789abcdef")), "web (detached)");
        // A bare repository has neither.
        assert_eq!(label(None, None), "web");
    }

    /// The picker closes with Esc; a folder it adds joins the sidebar once.
    #[gpui::test]
    fn an_added_folder_joins_the_sidebar_once(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("add");
        let cfg = server.config();
        let local = HostChoice {
            host: cfg.hosts[0].clone(),
            configured: true,
        };
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            super::folder_picker::bind_keys(cx);
        });
        let (app, cx) = cx.add_window_view(|window, cx| Lemur::new(cfg, None, window, cx));
        cx.update(|window, cx| app.update(cx, |app, cx| app.open_picker(window, cx)));
        cx.run_until_parked();
        cx.simulate_keystrokes("escape");
        app.read_with(cx, |app, _| assert!(app.picker.is_none()));

        for _ in 0..2 {
            app.update(cx, |app, cx| {
                app.add_folder(local.clone(), "/srv/api".into(), cx)
            });
        }
        cx.run_until_parked();
        app.read_with(cx, |app, _| {
            let bases: Vec<&str> = app.cfg.bases.iter().map(|b| b.path.as_str()).collect();
            assert_eq!(bases, ["/srv/api"]);
            assert_eq!(app.cfg.hosts.len(), 1);
            assert_eq!(app.snap.hosts[0].bases.len(), 1, "the refresh lists it");
            let status = app.status.as_ref().map(|(s, _)| s.as_str());
            assert_eq!(status, Some("/srv/api is in the sidebar already"));
        });
    }

    /// A note closes itself; an error stays until its × is clicked.
    #[gpui::test]
    fn notes_fade_and_errors_wait_to_be_closed(cx: &mut gpui::TestAppContext) {
        let (app, cx) = rooted(toml::from_str("").unwrap(), cx);
        let status = |cx: &mut VisualTestContext| app.read_with(cx, |app, _| app.status.clone());
        let wait = |time: Duration, cx: &mut VisualTestContext| {
            cx.executor().advance_clock(time);
            cx.run_until_parked();
        };

        app.update(cx, |app, cx| app.note("saved".into(), cx));
        wait(super::snackbar::NOTE_TIME / 2, cx);
        assert_eq!(status(cx), Some(("saved".into(), Kind::Note)));
        wait(super::snackbar::NOTE_TIME / 2, cx);
        assert_eq!(status(cx), None);

        app.update(cx, |app, cx| app.fail(anyhow::anyhow!("broke"), cx));
        wait(super::snackbar::NOTE_TIME * 2, cx);
        assert_eq!(status(cx), Some(("broke".into(), Kind::Error)));
        let close = cx.debug_bounds("close status").expect("its ×").center();
        cx.simulate_click(close, Modifiers::none());
        assert_eq!(status(cx), None);
    }

    /// Drags one element onto another, by their debug selectors, as a mouse
    /// does: down, a little move to start the drag, then over and up.
    fn drag(from: &'static str, to: &'static str, cx: &mut VisualTestContext) {
        let from = cx.debug_bounds(from).expect(from).center();
        let to = cx.debug_bounds(to).expect(to).center();
        let none = Modifiers::none();
        cx.simulate_mouse_down(from, MouseButton::Left, none);
        cx.simulate_mouse_move(from + point(px(0.0), px(6.0)), MouseButton::Left, none);
        cx.simulate_mouse_move(to, MouseButton::Left, none);
        cx.simulate_mouse_up(to, MouseButton::Left, none);
    }

    /// Wraps the app in Root, as `gpui_kit::open_window` does; dialogs need it.
    fn rooted(
        cfg: Config,
        cx: &mut gpui::TestAppContext,
    ) -> (Entity<Lemur>, &mut VisualTestContext) {
        cx.executor().allow_parking();
        cx.update(gpui_kit::init);
        let mut app = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| Lemur::new(cfg, None, window, cx));
            app = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        cx.run_until_parked();
        (app.unwrap(), cx)
    }

    /// Removing a project asks first, then forgets it; the other projects'
    /// views keep pointing at their own config entries, and a snapshot
    /// fetched before the removal is dropped.
    #[gpui::test]
    fn removing_a_project_keeps_the_sidebar_consistent(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("rm-project");
        let mut cfg = server.config();
        cfg.bases.push(Base::new("local", "/srv/one"));
        cfg.bases.push(Base::new("local", "/srv/two"));
        let (app, cx) = rooted(cfg, cx);

        let one = Base::new("local", "/srv/one");
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.confirm_remove_project(one.clone(), window, cx)
            })
        });
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
        cx.update(|window, cx| window.close_dialog(cx));

        // A refresh is on its way when the project goes.
        app.update(cx, |app, cx| app.refresh(cx).detach());
        app.update(cx, |app, cx| app.remove_project(&one, cx));
        // A herdr-core event before the next refresh lands composes from
        // what the last one found.
        app.read_with(cx, |app, _| {
            let recomposed = snapshot::compose(&app.cfg, &app.data);
            let views = &recomposed.hosts[0].bases;
            assert_eq!(views.len(), 1);
            assert_eq!(views[0].index, 0);
        });
        cx.run_until_parked();
        app.read_with(cx, |app, _| {
            let paths: Vec<&str> = app.cfg.bases.iter().map(|b| b.path.as_str()).collect();
            assert_eq!(paths, ["/srv/two"]);
            let views = &app.snap.hosts[0].bases;
            assert_eq!(views.len(), 1);
            assert_eq!(views[0].index, 0);
        });
    }

    /// A worktree with changes isn't removed without asking again, and its
    /// thread keeps running until it is.
    #[gpui::test]
    fn removing_a_changed_worktree_asks_again(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("rm-dirty");
        let dir = TempDir::new("rm-dirty");
        let repo = dir.join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| git_in(&repo, args);
        git(&["init", "-q"]);
        git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        git(&["worktree", "add", "-q", "-b", "draft", "wt"]);
        let wt = repo.join("wt");
        std::fs::write(wt.join("notes.txt"), "unsaved").unwrap();
        let path = wt.to_string_lossy().into_owned();
        let t = server.run(&path, &[], &["sleep", "600"]);
        let mut cfg = server.config();
        cfg.bases.push(Base::new("local", &repo.to_string_lossy()));
        let (app, cx) = rooted(cfg, cx);
        let alive = || server.addresses().contains(&t);

        let remove = |force: bool, cx: &mut VisualTestContext| {
            app.update(cx, |app, cx| {
                let mut removal = app.removal(0, path.clone(), "draft".into());
                // Only deleting its folder would lose the changes.
                removal.delete = true;
                app.remove_worktree(removal, force, cx)
            });
            cx.run_until_parked();
        };
        assert_eq!(
            app.read_with(cx, |app, _| app
                .removal(0, path.clone(), "draft".into())
                .sessions),
            std::slice::from_ref(&t)
        );
        remove(false, cx);
        assert!(wt.exists() && alive());
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
        cx.update(|window, cx| window.close_dialog(cx));
        assert!(alive());

        // Its folder color goes with it.
        app.update(cx, |app, cx| {
            app.set_worktree_color(0, &path, Some("red"), cx)
        });
        remove(true, cx);
        assert!(!wt.exists());
        assert!(wait_until(Duration::from_secs(5), || !alive()));
        app.read_with(cx, |app, _| {
            assert!(app.cfg.bases[0].worktree_colors.is_empty())
        });
    }

    /// The remove dialog takes a worktree out of git and keeps its folder,
    /// unless its box is ticked; then the folder goes too.
    #[gpui::test]
    fn removing_a_worktree_deletes_its_folder_when_ticked(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("rm-tick");
        let dir = TempDir::new("rm-tick");
        let repo = dir.join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| git_in(&repo, args);
        git(&["init", "-q"]);
        git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        git(&["worktree", "add", "-q", "-b", "kept", "wt/kept"]);
        git(&["worktree", "add", "-q", "-b", "gone", "wt/gone"]);
        let (kept, gone) = (repo.join("wt/kept"), repo.join("wt/gone"));
        let mut cfg = server.config();
        cfg.bases.push(Base::new("local", &repo.to_string_lossy()));
        let (app, cx) = rooted(cfg, cx);
        cx.simulate_resize(size(px(1000.0), px(600.0)));
        cx.run_until_parked();
        let confirm = |worktree: &std::path::Path, cx: &mut VisualTestContext| {
            let path = worktree.to_string_lossy().into_owned();
            cx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    let removal = app.removal(0, path, "x".into());
                    app.confirm_remove_worktree(removal, None, window, cx)
                })
            });
            cx.run_until_parked();
            assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
        };
        let listed = || {
            let out = std::process::Command::new("git")
                .args(["worktree", "list"])
                .current_dir(&repo)
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).into_owned()
        };

        confirm(&kept, cx);
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(wait_until(Duration::from_secs(5), || !listed().contains("wt/kept")));
        assert!(kept.exists() && !kept.join(".git").exists());

        confirm(&gone, cx);
        let tick = cx.debug_bounds("delete-folder").expect("the box").center();
        cx.simulate_click(tick, Modifiers::none());
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(wait_until(Duration::from_secs(5), || !gone.exists()));
        assert!(!listed().contains("wt/gone"));
    }

    /// The remove dialog names the threads running in a worktree. Cancelling
    /// keeps them and it; agreeing closes them, then removes it. One the
    /// sidebar didn't know of yet keeps it until the host's ask is agreed to.
    #[gpui::test]
    fn removing_a_worktree_closes_its_threads_first(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("rm-threads");
        let dir = TempDir::new("rm-threads");
        let repo = dir.join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| git_in(&repo, args);
        git(&["init", "-q"]);
        git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        git(&["worktree", "add", "-q", "-b", "busy", "wt/busy"]);
        git(&["worktree", "add", "-q", "-b", "late", "wt/late"]);
        let (busy, late) = (repo.join("wt/busy"), repo.join("wt/late"));
        let start =
            |dir: &std::path::Path| server.run(&dir.to_string_lossy(), &[], &["sleep", "600"]);
        let first = start(&busy);
        let mut cfg = server.config();
        cfg.bases.push(Base::new("local", &repo.to_string_lossy()));
        let (app, cx) = rooted(cfg, cx);
        cx.simulate_resize(size(px(1000.0), px(600.0)));
        cx.run_until_parked();
        let alive = |name: &str| server.addresses().iter().any(|a| a == name);
        let listed = || {
            let out = std::process::Command::new("git")
                .args(["worktree", "list"])
                .current_dir(&repo)
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        // Opens the remove dialog; returns the threads it names.
        let confirm = |worktree: &std::path::Path, cx: &mut VisualTestContext| {
            let path = worktree.to_string_lossy().into_owned();
            let named = cx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    let removal = app.removal(0, path, "x".into());
                    let named = removal.sessions.clone();
                    app.confirm_remove_worktree(removal, None, window, cx);
                    named
                })
            });
            cx.run_until_parked();
            assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
            named
        };

        assert_eq!(confirm(&busy, cx), std::slice::from_ref(&first));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
        assert!(alive(&first) && listed().contains("wt/busy"));

        confirm(&busy, cx);
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(wait_until(Duration::from_secs(5), || !listed().contains("wt/busy")));
        assert!(wait_until(Duration::from_secs(5), || !alive(&first)));

        // Started after the sidebar last looked: the host asks about it.
        let second = start(&late);
        assert!(wait_until(Duration::from_secs(5), || alive(&second)));
        assert!(confirm(&late, cx).is_empty());
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
        assert!(alive(&second) && listed().contains("wt/late"));
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(wait_until(Duration::from_secs(5), || !listed().contains("wt/late")));
        assert!(wait_until(Duration::from_secs(5), || !alive(&second)));
    }

    /// A new worktree's prompt offers the host's branches to start from,
    /// the default one first; the worktree gets the branch typed, or with
    /// none is detached. Picking another branch keeps the prompt open.
    #[gpui::test]
    fn a_new_worktree_gets_the_branch_and_start_asked_for(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("new-wt-ui");
        let dir = TempDir::new("new-wt-ui");
        let origin = dir.join("origin");
        std::fs::create_dir(&origin).unwrap();
        git_in(&origin, &["init", "-q", "-b", "main"]);
        git_in(&origin, &["commit", "-q", "--allow-empty", "-m", "one"]);
        git_in(&dir, &["clone", "-q", "origin", "repo"]);
        let repo = dir.join("repo");
        git_in(&repo, &["branch", "side"]);
        let mut cfg = server.config();
        // Typed into the new sessions; nothing to start.
        cfg.hosts[0].claude = "true".into();
        cfg.bases.push(Base::new("local", &repo.to_string_lossy()));
        let (app, cx) = rooted(cfg, cx);
        cx.simulate_resize(size(px(1000.0), px(600.0)));
        cx.run_until_parked();
        let ask = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    app.ask_name(Ask::NewWorktree { base: 0 }, window, cx)
                })
            });
            cx.run_until_parked();
        };
        let from = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, cx| {
                let fields = app.prompt.as_ref()?.worktree.as_ref()?;
                fields.from.read(cx).selected_value().cloned()
            })
        };
        let branch_field = |cx: &mut VisualTestContext| {
            let branch = app.read_with(cx, |app, _| {
                app.prompt
                    .as_ref()
                    .unwrap()
                    .worktree
                    .as_ref()
                    .unwrap()
                    .branch
                    .clone()
            });
            cx.update(|window, cx| branch.update(cx, |input, cx| input.focus(window, cx)));
        };
        let current = |name: &str| {
            let out = std::process::Command::new("git")
                .args(["branch", "--show-current"])
                .current_dir(repo.join(".claude/worktrees").join(name))
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };

        ask(cx);
        assert_eq!(from(cx).as_deref(), Some("origin/main"));
        cx.simulate_input("feat");
        branch_field(cx);
        cx.simulate_input("feature/x");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let made = repo.join(".claude/worktrees/feat");
        assert!(wait_until(Duration::from_secs(5), || made.exists()));
        assert_eq!(current("feat"), "feature/x");

        // Another start, picked from the list, and no branch.
        ask(cx);
        cx.simulate_input("loose");
        let picker = cx.debug_bounds("field From").expect("the start").center();
        // Esc closes the list first, not the prompt.
        cx.simulate_click(picker, Modifiers::none());
        cx.simulate_keystrokes("escape");
        app.read_with(cx, |app, _| {
            assert!(app.prompt.is_some(), "Esc closed the prompt")
        });
        cx.simulate_click(picker, Modifiers::none());
        cx.simulate_keystrokes("down enter");
        cx.run_until_parked();
        app.read_with(cx, |app, _| assert!(app.prompt.is_some(), "still asking"));
        let picked = from(cx);
        assert!(
            picked.as_deref().is_some_and(|b| b != "origin/main"),
            "{picked:?}"
        );
        cx.update(|window, cx| {
            let input = app.read(cx).prompt.as_ref().unwrap().input.clone();
            input.update(cx, |input, cx| input.focus(window, cx))
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let made = repo.join(".claude/worktrees/loose");
        assert!(wait_until(Duration::from_secs(5), || made.exists()));
        assert_eq!(current("loose"), "", "detached");
    }

    /// A Claude thread lemur starts is kept to resume. Once its terminal
    /// is gone, as after the host restarts, it stays under its worktree,
    /// dimmed: a click resumes it in a new one, so does Resume all, Forget
    /// drops it, and killing a thread forgets it.
    #[gpui::test]
    fn gone_threads_resume_their_claude(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("gone");
        let dir = TempDir::new("gone");
        let repo = dir.join("api");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        git_in(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let path = repo.canonicalize().unwrap().to_string_lossy().into_owned();
        let mut cfg = server.config();
        // Prints what Claude would get.
        cfg.hosts[0].claude = "echo claude".into();
        cfg.bases.push(Base::new("local", &path));
        let (app, cx) = rooted(cfg, cx);
        cx.update(|_, cx| super::bind_keys(cx));
        cx.simulate_resize(size(px(1000.0), px(600.0)));
        cx.run_until_parked();
        let kept = |cx: &mut VisualTestContext| app.read_with(cx, |app, _| app.cfg.threads.clone());
        let refresh = |cx: &mut VisualTestContext| {
            app.update(cx, |app, cx| app.refresh(cx).detach());
            cx.run_until_parked();
        };
        // As the host restarting does, without a shell coming back.
        let restart = |cx: &mut VisualTestContext| {
            let session = kept(cx)[0].session.clone();
            server.hc(&["keep", &session, "--off"]);
            server.hc(&["kill", &session]);
            assert!(wait_until(Duration::from_secs(5), || !server
                .addresses()
                .contains(&session)));
            refresh(cx);
        };
        let shown = |selector: &'static str, cx: &mut VisualTestContext| {
            cx.debug_bounds(selector).is_some()
        };

        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let ask = Ask::NewThread {
                    base: 0,
                    worktree: path.clone(),
                    shell: false,
                };
                app.ask_name(ask, window, cx)
            })
        });
        cx.run_until_parked();
        // As the other prompts' tests do: Enter on the selected label
        // would replace it here.
        cx.simulate_input("claude");
        cx.simulate_keystrokes("enter");
        wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            !kept(cx).is_empty()
        });
        let threads = kept(cx);
        let [thread] = threads.as_slice() else {
            let status = app.read_with(cx, |app, _| app.status.clone());
            panic!("kept {threads:?}, status {status:?}");
        };
        let thread = thread.clone();
        assert!(crate::actions::is_uuid(&thread.claude_session));
        assert_eq!(thread.path, path);
        assert_eq!(thread.label.as_deref(), Some("claude"));
        let typed = |text: &str, cx: &mut VisualTestContext| {
            let session = kept(cx)[0].session.clone();
            wait_until(Duration::from_secs(5), || {
                server.hc(&["read", &session]).contains(text)
            })
        };
        assert!(typed(
            &format!("claude --session-id {}", thread.claude_session),
            cx
        ));
        let resumed = format!("claude --resume {}", thread.claude_session);

        restart(cx);
        assert!(shown("gone-0", cx) && !shown("thread-0", cx));
        let row = cx.debug_bounds("gone-0").unwrap().center();
        cx.simulate_click(row, Modifiers::none());
        cx.run_until_parked();
        assert!(typed(&resumed, cx), "{resumed} never ran");
        assert_ne!(kept(cx)[0].session, thread.session, "in a new terminal");
        refresh(cx);
        assert!(shown("thread-0", cx) && !shown("gone-0", cx));

        restart(cx);
        app.update(cx, |app, cx| {
            let gone = app.gone_in(0);
            assert_eq!(gone.len(), 1);
            app.resume(gone, cx)
        });
        cx.run_until_parked();
        assert!(typed(&resumed, cx), "Resume all never ran it");
        refresh(cx);
        assert!(shown("thread-0", cx));

        // Killed in lemur, it's forgotten.
        let key: Key = ("local".into(), kept(cx)[0].session.clone());
        app.update(cx, |app, cx| app.close_thread(key.clone(), cx));
        cx.run_until_parked();
        assert!(kept(cx).is_empty());
        assert!(!shown("thread-0", cx) && !shown("gone-0", cx));

        // Forgotten, it's gone for good.
        app.update(cx, |app, cx| app.keep_thread(thread.clone(), cx));
        refresh(cx);
        assert!(shown("gone-0", cx));
        app.update(cx, |app, cx| {
            app.forget_threads("local", std::slice::from_ref(&thread.session), cx)
        });
        cx.run_until_parked();
        assert!(kept(cx).is_empty() && !shown("gone-0", cx));
        refresh(cx);
        assert!(!shown("gone-0", cx));
    }

    /// Refreshes keep each running Claude's session ID: a thread lemur
    /// didn't start gets kept, `/clear`ing it keeps the new ID, and the
    /// conversation going on under another name moves it there.
    #[gpui::test]
    fn refreshes_keep_claude_ids_current(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("ids");
        let (app, cx) = rooted(server.config(), cx);
        let id = |n: u8| format!("0fdfabeb-ca80-4916-a103-275bbb5e86{n:02}");
        let running = |runs: &[(&str, String)], cx: &mut VisualTestContext| {
            let names: Vec<_> = runs.iter().map(|(s, _)| (*s, Activity::Idle)).collect();
            let mut snap = snapshot(&names);
            for (s, (_, id)) in snap.hosts[0].other.iter_mut().zip(runs) {
                s.claude = Some(snapshot::ClaudeRun {
                    id: id.clone(),
                    path: "/srv/api".into(),
                });
            }
            cx.update(|window, cx| app.update(cx, |app, cx| app.set_snapshot(snap, window, cx)));
        };
        let kept = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, _| {
                app.cfg
                    .threads
                    .iter()
                    .map(|t| (t.session.clone(), t.claude_session.clone()))
                    .collect::<Vec<_>>()
            })
        };

        running(&[("a", id(1)), ("b", "not-an-id".into())], cx);
        assert_eq!(kept(cx), [("a".into(), id(1))]);
        running(&[("a", id(2))], cx);
        assert_eq!(kept(cx), [("a".into(), id(2))], "after /clear");
        running(&[("c", id(2))], cx);
        assert_eq!(kept(cx), [("c".into(), id(2))], "renamed by hand");
    }

    /// ⌘J asks for a new thread in the worktree on show: Claude or Shell,
    /// picked with ↑↓ or a click, and its name, which follows the pick until
    /// you type one.
    #[gpui::test]
    fn cmd_j_asks_for_claude_or_a_shell_here(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("cmd-j");
        let mut cfg = server.config();
        cfg.bases.push(Base::new("local", "/srv/api"));
        let (app, cx) = rooted(cfg, cx);
        cx.update(|_, cx| super::bind_keys(cx));
        cx.simulate_resize(size(px(1000.0), px(600.0)));
        let wt = "/srv/api/.claude/worktrees/auth";
        let snap = Snapshot {
            hosts: vec![HostView {
                name: "local".into(),
                error: None,
                home: String::new(),
                machine: Default::default(),
                bases: vec![BaseView {
                    index: 0,
                    error: None,
                    plain: false,
                    worktrees: vec![Worktree {
                        path: wt.into(),
                        branch: Some("worktree-auth".into()),
                        head: None,
                        main: false,
                        sessions: snapshot(&[("api/t", Activity::Idle)]).hosts.remove(0).other,
                        gone: vec![],
                    }],
                }],
                other: vec![],
                gone: vec![],
            }],
        };
        cx.update(|window, cx| app.update(cx, |app, cx| app.set_snapshot(snap, window, cx)));
        // Nothing on show, nowhere to start.
        cx.simulate_keystrokes("cmd-j");
        app.read_with(cx, |app, _| assert!(app.prompt.is_none()));
        open(&app, "api/t", cx);
        cx.run_until_parked();
        let asked = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, cx| {
                let prompt = app.prompt.as_ref().expect("asking");
                let Ask::NewThread {
                    worktree, shell, ..
                } = &prompt.what
                else {
                    panic!("not a new thread");
                };
                assert_eq!(worktree, wt);
                (*shell, prompt.input.read(cx).value().to_string())
            })
        };

        cx.simulate_keystrokes("cmd-j");
        cx.run_until_parked();
        assert_eq!(asked(cx), (false, "claude".into()));
        cx.simulate_keystrokes("down");
        assert_eq!(asked(cx), (true, "shell".into()));
        cx.simulate_input("build");
        cx.simulate_keystrokes("up");
        assert_eq!(asked(cx), (false, "build".into()), "a typed name stays");
        let shell = cx
            .debug_bounds("kind Shell")
            .expect("the Shell row")
            .center();
        cx.simulate_click(shell, Modifiers::none());
        assert_eq!(asked(cx), (true, "build".into()));
        cx.simulate_keystrokes("escape");
        app.read_with(cx, |app, _| assert!(app.prompt.is_none()));

        // Let the terminal's attach attempt end before the server cleans up.
        wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            app.read_with(cx, |app, cx| {
                app.terminals.values().all(|t| t.read(cx).exited())
            })
        });
    }

    /// Names are asked for in the spotlight: a bad one is explained there
    /// rather than in the header, and Esc closes it.
    #[gpui::test]
    fn names_are_asked_for_in_the_spotlight(cx: &mut gpui::TestAppContext) {
        let (app, cx) = rooted(toml::from_str("").unwrap(), cx);
        let rename = Ask::Rename {
            host: "local".into(),
            session: "api/claude--calm-heron".into(),
        };
        cx.update(|window, cx| app.update(cx, |app, cx| app.ask_name(rename, window, cx)));
        cx.run_until_parked();
        app.read_with(cx, |app, cx| {
            let prompt = app.prompt.as_ref().expect("asking");
            assert_eq!(prompt.input.read(cx).value(), "claude");
        });

        // The label starts selected, so typing replaces it.
        cx.simulate_input("not ok!");
        cx.simulate_keystrokes("enter");
        app.read_with(cx, |app, cx| {
            let prompt = app.prompt.as_ref().expect("still asking");
            assert_eq!(prompt.input.read(cx).value(), "not ok!");
            let error = prompt.error.as_ref().map(|(e, _)| e.as_str());
            assert!(error.is_some_and(|e| e.contains("letters")), "{error:?}");
            assert!(app.status.is_none(), "the header stays quiet");
        });
        cx.simulate_input("x");
        app.read_with(cx, |app, _| {
            assert!(
                app.prompt.as_ref().unwrap().error.is_none(),
                "typing clears it"
            )
        });

        cx.simulate_keystrokes("escape");
        app.read_with(cx, |app, _| assert!(app.prompt.is_none()));
    }

    /// Clicking a project's name folds it away and back; a worktree's
    /// folder takes a color and gives it up.
    #[gpui::test]
    fn projects_fold_and_worktrees_take_a_color(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("fold");
        let mut cfg = server.config();
        cfg.bases.push(Base::new("local", "/srv/api"));
        let (app, cx) = rooted(cfg, cx);
        cx.simulate_resize(size(px(1000.0), px(600.0)));
        cx.run_until_parked();
        let folded =
            |cx: &mut VisualTestContext| app.read_with(cx, |app, _| app.cfg.bases[0].collapsed);

        let header = cx
            .debug_bounds("project api · local")
            .expect("header")
            .center();
        cx.simulate_click(header, Modifiers::none());
        assert!(folded(cx));
        cx.run_until_parked();
        cx.simulate_click(header, Modifiers::none());
        assert!(!folded(cx));

        app.update(cx, |app, cx| {
            app.set_worktree_color(0, "/srv/api", Some("teal"), cx)
        });
        app.read_with(cx, |app, _| {
            assert!(super::sidebar::worktree_color(&app.cfg.bases[0], "/srv/api").is_some());
            assert!(super::sidebar::worktree_color(&app.cfg.bases[0], "/srv/api/wt").is_none());
        });
        app.update(cx, |app, cx| {
            app.set_worktree_color(0, "/srv/api", None, cx)
        });
        app.read_with(cx, |app, _| {
            assert!(app.cfg.bases[0].worktree_colors.is_empty())
        });
    }

    /// Clicking a worktree folds its threads away and back; its + starts
    /// a terminal there, or opens its notes, and leaves it open.
    #[gpui::test]
    fn clicking_a_worktree_folds_its_threads(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("fold-wt");
        let dir = TempDir::new("fold-wt");
        let repo = dir.join("api");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| git_in(&repo, args);
        git(&["init", "-q", "-b", "main"]);
        git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        let path = repo.to_string_lossy().into_owned();
        server.run(&path, &[], &["sleep", "600"]);
        let mut cfg = server.config();
        cfg.bases.push(Base::new("local", &path));
        let (app, cx) = rooted(cfg, cx);
        cx.simulate_resize(size(px(1000.0), px(600.0)));
        cx.run_until_parked();
        let shown = |cx: &mut VisualTestContext| cx.debug_bounds("thread-0").is_some();
        assert!(shown(cx));

        let row = cx
            .debug_bounds("worktree-0")
            .expect("the worktree")
            .center();
        assert!(
            cx.debug_bounds("counts main").is_none(),
            "open, it shows them"
        );
        cx.simulate_click(row, Modifiers::none());
        cx.run_until_parked();
        assert!(!shown(cx));
        // Folded, it says what runs in it.
        assert!(cx.debug_bounds("counts main").is_some());
        cx.simulate_click(row, Modifiers::none());
        cx.run_until_parked();
        assert!(shown(cx));

        // + is the last of its buttons; its menu's first item starts a
        // terminal there.
        let before = server.addresses().len();
        let buttons = cx.debug_bounds("buttons main").expect("its buttons");
        let add = point(buttons.right() - px(4.0), buttons.center().y);
        cx.simulate_mouse_move(add, None, Modifiers::none());
        cx.simulate_click(add, Modifiers::none());
        cx.run_until_parked();
        cx.simulate_keystrokes("down enter");
        assert!(
            wait_until(Duration::from_secs(5), || server.addresses().len() > before),
            "no terminal started"
        );
        let started = server.core().terminals();
        let t = started.last().unwrap();
        assert_eq!(t.cwd, path);
        assert_eq!(
            t.labels.get("lemur.kind").map(String::as_str),
            Some("shell")
        );
        assert!(shown(cx));

        // Its last item opens its notes.
        cx.simulate_mouse_move(add, None, Modifiers::none());
        cx.simulate_click(add, Modifiers::none());
        cx.run_until_parked();
        cx.simulate_keystrokes("up enter");
        cx.run_until_parked();
        app.read_with(cx, |app, cx| {
            assert!(app.shows(Right::Notes));
            assert_eq!(app.notes.read(cx).worktree(), Some(path.as_str()));
        });
        let_attaches_end(&app, cx);
    }

    /// ⌘T starts a terminal where the thread on show runs, and shows it.
    #[gpui::test]
    fn cmd_t_opens_a_terminal_beside_the_thread(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("cmd-t");
        let dir = TempDir::new("cmd-t");
        let repo = dir.join("api");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        git_in(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        git_in(&repo, &["worktree", "add", "-q", "-b", "a", "wt/a"]);
        let wt = repo.join("wt/a").to_string_lossy().into_owned();
        let t = server.run(&wt, &["lemur.label=claude"], &["sleep", "600"]);
        let mut cfg = server.config();
        cfg.bases.push(Base::new("local", &repo.to_string_lossy()));
        let (app, cx) = rooted(cfg, cx);
        cx.update(|_, cx| super::bind_keys(cx));
        cx.simulate_resize(size(px(1200.0), px(700.0)));
        open(&app, &t, cx);
        cx.run_until_parked();

        cx.simulate_keystrokes("cmd-t");
        let mut started = None;
        assert!(wait_until(Duration::from_secs(5), || {
            started = server
                .core()
                .terminals()
                .into_iter()
                .find(|x| x.address != t);
            started.is_some()
        }));
        let started = started.unwrap();
        assert_eq!(started.cwd, wt);
        assert_eq!(
            started.labels.get("lemur.label").map(String::as_str),
            Some("shell")
        );
        let shown = wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            app.read_with(cx, |app, _| app.active == Some(key(&started.address)))
        });
        assert!(shown, "the new terminal shows");
        let_attaches_end(&app, cx);
    }

    /// A folded worktree stays folded, and an open Hidden row open, while
    /// their project is folded away and back, and in the next lemur.
    #[gpui::test]
    fn folds_outlast_their_project_folding(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("fold-keep");
        let dir = TempDir::new("fold-keep");
        let repo = dir.join("api");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| git_in(&repo, args);
        git(&["init", "-q", "-b", "main"]);
        git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        git(&["worktree", "add", "-q", "-b", "a", "wt/a"]);
        let path = repo.to_string_lossy().into_owned();
        server.run(&path, &[], &["sleep", "600"]);
        let mut cfg = server.config();
        cfg.bases.push(Base::new("local", &path));
        let (app, cx) = rooted(cfg, cx);
        cx.simulate_resize(size(px(1000.0), px(600.0)));
        cx.run_until_parked();
        let a = app.read_with(cx, |app, _| {
            app.snap.hosts[0].bases[0].worktrees[1].path.clone()
        });
        app.update(cx, |app, cx| app.set_worktree_hidden(0, &a, true, cx));
        cx.run_until_parked();
        let click = |selector: &'static str, cx: &mut VisualTestContext| {
            let at = cx.debug_bounds(selector).expect(selector).center();
            cx.simulate_click(at, Modifiers::none());
            cx.run_until_parked();
        };
        let threads = |cx: &mut VisualTestContext| cx.debug_bounds("thread-0").is_some();
        let hidden = |cx: &mut VisualTestContext| cx.debug_bounds("buttons a").is_some();

        click("hidden", cx);
        click("worktree-0", cx);
        assert!(hidden(cx) && !threads(cx));

        click("project api · local", cx);
        assert!(!hidden(cx) && cx.debug_bounds("worktree-0").is_none());
        click("project api · local", cx);
        assert!(!threads(cx), "the worktree stays folded");
        assert!(hidden(cx), "Hidden stays open");

        let cfg = app.read_with(cx, |app, _| app.cfg.clone());
        assert_eq!(cfg.bases[0].collapsed_worktrees, [path.as_str()]);
        assert!(cfg.bases[0].hidden_open);
        let (_, cx) = rooted(cfg, cx);
        cx.simulate_resize(size(px(1000.0), px(600.0)));
        cx.run_until_parked();
        assert!(cx.debug_bounds("worktree-0").is_some());
        assert!(!threads(cx), "the next lemur keeps the worktree folded");
        assert!(hidden(cx), "and Hidden open");
    }

    /// Dropping a worktree on another moves it there, and the next refresh
    /// keeps that order; the main checkout stays known wherever it is. A
    /// worktree's threads are part of it: dropped on one, it moves too.
    #[gpui::test]
    fn dragging_a_worktree_reorders_its_project(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("drag-wt");
        let dir = TempDir::new("drag-wt");
        let repo = dir.join("api");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| git_in(&repo, args);
        git(&["init", "-q", "-b", "main"]);
        git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        git(&["worktree", "add", "-q", "-b", "a", "wt/a"]);
        git(&["worktree", "add", "-q", "-b", "b", "wt/b"]);
        let wt = repo.join("wt/a").to_string_lossy().into_owned();
        server.run(&wt, &[], &["sleep", "600"]);
        let mut cfg = server.config();
        cfg.bases.push(Base::new("local", &repo.to_string_lossy()));
        let (app, cx) = rooted(cfg, cx);
        cx.simulate_resize(size(px(1000.0), px(600.0)));
        cx.run_until_parked();
        let shown = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, _| {
                app.snap.hosts[0].bases[0]
                    .worktrees
                    .iter()
                    .map(|w| (super::worktree_label(w), w.main))
                    .collect::<Vec<_>>()
            })
        };
        let names = |cx: &mut VisualTestContext| -> Vec<String> {
            shown(cx).into_iter().map(|(name, _)| name).collect()
        };
        assert_eq!(names(cx), ["main", "a", "b"]);

        drag("worktree-0", "worktree-2", cx);
        assert_eq!(names(cx), ["a", "b", "main"]);

        app.update(cx, |app, cx| app.refresh(cx).detach());
        cx.run_until_parked();
        assert_eq!(
            shown(cx),
            [
                ("a".to_string(), false),
                ("b".to_string(), false),
                ("main".to_string(), true)
            ]
        );

        drag("worktree-2", "thread-0", cx);
        assert_eq!(names(cx), ["main", "a", "b"]);
    }

    /// Hiding a worktree moves it to its project's Hidden row, folded until
    /// clicked; the others still drag past it, and unhiding moves it back
    /// to its place.
    #[gpui::test]
    fn hidden_worktrees_wait_under_their_project(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("hide-wt");
        let dir = TempDir::new("hide-wt");
        let repo = dir.join("api");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| git_in(&repo, args);
        git(&["init", "-q", "-b", "main"]);
        git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        git(&["worktree", "add", "-q", "-b", "a", "wt/a"]);
        git(&["worktree", "add", "-q", "-b", "b", "wt/b"]);
        let mut cfg = server.config();
        cfg.bases.push(Base::new("local", &repo.to_string_lossy()));
        let (app, cx) = rooted(cfg, cx);
        cx.simulate_resize(size(px(1000.0), px(600.0)));
        cx.run_until_parked();
        let names = |cx: &mut VisualTestContext| -> Vec<String> {
            app.read_with(cx, |app, _| {
                app.snap.hosts[0].bases[0]
                    .worktrees
                    .iter()
                    .map(super::worktree_label)
                    .collect()
            })
        };
        let a = app.read_with(cx, |app, _| {
            app.snap.hosts[0].bases[0].worktrees[1].path.clone()
        });
        let hide = |hidden: bool, cx: &mut VisualTestContext| {
            app.update(cx, |app, cx| app.set_worktree_hidden(0, &a, hidden, cx));
            cx.run_until_parked();
        };
        assert!(cx.debug_bounds("hidden").is_none());

        hide(true, cx);
        assert!(cx.debug_bounds("buttons a").is_none(), "folded away");
        assert!(
            cx.debug_bounds("buttons main").is_some() && cx.debug_bounds("buttons b").is_some()
        );
        app.read_with(cx, |app, _| {
            assert_eq!(app.cfg.bases[0].hidden_worktrees, [a.as_str()])
        });

        // b and main are the rows that drag; a keeps its place behind them.
        drag("worktree-1", "worktree-0", cx);
        assert_eq!(names(cx), ["b", "main", "a"]);

        let row = cx.debug_bounds("hidden").expect("the Hidden row").center();
        cx.simulate_click(row, Modifiers::none());
        cx.run_until_parked();
        assert!(cx.debug_bounds("buttons a").is_some(), "opened");

        hide(false, cx);
        assert!(cx.debug_bounds("hidden").is_none());
        assert!(cx.debug_bounds("buttons a").is_some());
        app.read_with(cx, |app, _| {
            assert!(app.cfg.bases[0].hidden_worktrees.is_empty())
        });
        app.update(cx, |app, cx| app.refresh(cx).detach());
        cx.run_until_parked();
        assert_eq!(names(cx), ["b", "main", "a"]);
    }

    /// The menus' actions and keys reach the main window: ⌘O opens the
    /// folder picker, ⌘R toggles the right panel (the notes at first),
    /// Refresh refreshes.
    #[gpui::test]
    fn menu_actions_reach_the_main_window(cx: &mut gpui::TestAppContext) {
        let (app, cx) = rooted(toml::from_str("").unwrap(), cx);
        cx.update(|_, cx| super::bind_keys(cx));

        cx.simulate_keystrokes("cmd-o");
        app.read_with(cx, |app, _| assert!(app.picker.is_some()));
        cx.simulate_keystrokes("escape");
        app.read_with(cx, |app, _| assert!(app.picker.is_none()));

        cx.simulate_keystrokes("cmd-r");
        app.read_with(cx, |app, _| assert!(app.shows(Right::Notes)));
        cx.simulate_keystrokes("cmd-r");
        app.read_with(cx, |app, _| assert!(!app.shows(Right::Notes)));

        app.update(cx, |app, _| app.loaded = false);
        cx.dispatch_action(super::Refresh);
        cx.run_until_parked();
        app.read_with(cx, |app, _| assert!(app.loaded, "a refresh came in"));
    }

    /// Out of sight, refreshes come every AWAY ticks rather than every
    /// tick; back in sight, every tick again.
    #[gpui::test]
    fn refreshes_slow_down_out_of_sight(cx: &mut gpui::TestAppContext) {
        // Test windows start inactive.
        let (app, cx) = rooted(toml::from_str("").unwrap(), cx);
        // Whether a refresh came in since the last ask.
        let refreshed = |cx: &mut VisualTestContext| {
            app.update(cx, |app, _| std::mem::replace(&mut app.loaded, false))
        };
        let tick = |cx: &mut VisualTestContext| {
            cx.executor().advance_clock(super::REFRESH);
            cx.run_until_parked();
        };
        assert!(refreshed(cx), "the first one");
        for _ in 1..super::AWAY {
            tick(cx);
            assert!(!refreshed(cx));
        }
        tick(cx);
        assert!(refreshed(cx));

        cx.update(|window, _| window.activate_window());
        for _ in 0..2 {
            tick(cx);
            assert!(refreshed(cx));
        }
    }

    /// Terminal output redraws the terminal alone; the cached sidebar waits
    /// until the app itself changes.
    #[gpui::test]
    fn terminal_output_leaves_the_sidebar_alone(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("cache");
        let t = server.run("/", &[], &["sleep", "600"]);
        let (app, cx) = rooted(server.config(), cx);
        cx.simulate_resize(size(px(1000.0), px(600.0)));
        open(&app, &t, cx);
        cx.run_until_parked();
        let renders =
            |cx: &mut VisualTestContext| app.read_with(cx, |app, cx| app.sidebar.read(cx).renders);
        let before = renders(cx);
        assert!(before > 0, "drawn at least once");

        let terminal = app.read_with(cx, |app, _| app.terminals[&key(&t)].clone());
        terminal.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        assert_eq!(renders(cx), before);

        app.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        assert!(renders(cx) > before);
    }

    /// A working thread's dot breathes, a few breaths' frames a second,
    /// only while Lemur is the active window; otherwise it holds still and
    /// the sidebar isn't redrawn for it.
    #[gpui::test]
    fn working_dots_breathe_only_in_the_active_window(cx: &mut gpui::TestAppContext) {
        // No hosts, so no refresh replaces the snapshot below.
        let (app, cx) = rooted(toml::from_str("").unwrap(), cx);
        cx.simulate_resize(size(px(1000.0), px(600.0)));
        apply(&app, &[("busy", Activity::Working)], cx);
        cx.run_until_parked();
        let renders =
            |cx: &mut VisualTestContext| app.read_with(cx, |app, cx| app.sidebar.read(cx).renders);
        let in_a_second = |cx: &mut VisualTestContext| {
            let before = renders(cx);
            for _ in 0..50 {
                cx.executor().advance_clock(Duration::from_millis(20));
                cx.run_until_parked();
            }
            renders(cx) - before
        };
        // Test windows start inactive.
        assert_eq!(in_a_second(cx), 0, "still in the background");

        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        let breaths = in_a_second(cx);
        assert!((6..=8).contains(&breaths), "{breaths} frames");

        cx.deactivate_window();
        cx.run_until_parked();
        in_a_second(cx);
        assert_eq!(in_a_second(cx), 0, "still again");
    }

    /// While its host can't be reached, a thread keeps what it was doing
    /// and its done mark: back, it hasn't just changed, and finishing then
    /// still counts.
    #[gpui::test]
    fn a_host_out_of_reach_keeps_what_its_threads_did(cx: &mut gpui::TestAppContext) {
        let cfg: Config = toml::from_str("").unwrap();
        cx.update(gpui_kit::init);
        let (app, cx) = cx.add_window_view(|window, cx| Lemur::new(cfg, None, window, cx));
        app.update(cx, |app, _| app.active = Some(key("shown")));
        let out_of_reach = |cx: &mut VisualTestContext| {
            let mut snap = snapshot(&[]);
            snap.hosts[0].error = Some("can't reach it".into());
            cx.update(|window, cx| app.update(cx, |app, cx| app.set_snapshot(snap, window, cx)));
        };
        apply(
            &app,
            &[("shown", Activity::Working), ("done", Activity::Working)],
            cx,
        );
        apply(
            &app,
            &[("shown", Activity::Working), ("done", Activity::Idle)],
            cx,
        );
        out_of_reach(cx);
        app.read_with(cx, |app, _| {
            assert!(app.done.contains(&key("done")), "kept")
        });
        apply(
            &app,
            &[("shown", Activity::Idle), ("done", Activity::Idle)],
            cx,
        );
        app.read_with(cx, |app, _| assert!(app.done.contains(&key("done"))));

        apply(
            &app,
            &[("shown", Activity::Working), ("other", Activity::Working)],
            cx,
        );
        out_of_reach(cx);
        apply(
            &app,
            &[("shown", Activity::Working), ("other", Activity::Idle)],
            cx,
        );
        app.read_with(cx, |app, _| {
            assert!(
                app.done.contains(&key("other")),
                "it finished: {:?}",
                app.done
            )
        });
    }

    #[gpui::test]
    fn a_session_that_finishes_elsewhere_is_marked_done(cx: &mut gpui::TestAppContext) {
        // No hosts, so the background refresh has nothing to fetch.
        let cfg: Config = toml::from_str("").unwrap();
        cx.update(gpui_kit::init);
        let (app, cx) = cx.add_window_view(|window, cx| Lemur::new(cfg, None, window, cx));
        app.update(cx, |app, _| app.active = Some(key("shown")));
        let working = [
            ("shown", Activity::Working),
            ("elsewhere", Activity::Working),
            ("asking", Activity::Working),
        ];
        apply(&app, &working, cx);
        app.read_with(cx, |app, _| assert!(app.done.is_empty()));
        apply(
            &app,
            &[
                ("shown", Activity::Idle),
                ("elsewhere", Activity::Idle),
                ("asking", Activity::NeedsInput),
            ],
            cx,
        );
        app.read_with(cx, |app, _| {
            assert!(app.done.contains(&key("elsewhere")), "{:?}", app.done);
            assert!(
                !app.done.contains(&key("shown")),
                "the shown session isn't news"
            );
            assert!(
                !app.done.contains(&key("asking")),
                "waiting on you isn't done"
            );
        });
        // Working again clears it.
        apply(&app, &[("elsewhere", Activity::Working)], cx);
        app.read_with(cx, |app, _| assert!(app.done.is_empty(), "{:?}", app.done));
    }

    #[gpui::test]
    fn finishing_out_of_sight_pops_up_a_notification(cx: &mut gpui::TestAppContext) {
        // No terminal runs here: Show still opens a terminal view that tries
        // to attach, for real.
        let server = HcServer::new("popup");
        cx.executor().allow_parking();
        cx.update(gpui_kit::init);
        let (app, cx) =
            cx.add_window_view(|window, cx| Lemur::new(server.config(), None, window, cx));
        let windows = |cx: &mut VisualTestContext| cx.update(|_, cx| cx.windows().len());

        // In the active window: the session you're looking at doesn't pop up,
        // one you aren't does. The test platform activates windows
        // asynchronously.
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        app.update(cx, |app, _| app.active = Some(key("shown")));
        apply(
            &app,
            &[("shown", Activity::Working), ("a", Activity::Working)],
            cx,
        );
        apply(
            &app,
            &[("shown", Activity::Idle), ("a", Activity::Idle)],
            cx,
        );
        app.read_with(cx, |app, _| {
            assert!(
                !app.popups.contains_key(&key("shown")),
                "you're looking at it"
            );
            assert!(app.popups.contains_key(&key("a")));
        });
        assert_eq!(windows(cx), 2);

        // Away from the window, even the shown session pops up; needing input
        // does too.
        cx.deactivate_window();
        apply(
            &app,
            &[
                ("shown", Activity::Working),
                ("a", Activity::Idle),
                ("b", Activity::Working),
            ],
            cx,
        );
        apply(
            &app,
            &[
                ("shown", Activity::Idle),
                ("a", Activity::Idle),
                ("b", Activity::NeedsInput),
            ],
            cx,
        );
        assert_eq!(windows(cx), 4);
        let popup = app.read_with(cx, |app, cx| app.popups[&key("a")].0.entity(cx).unwrap());

        // Show opens that session and closes only its popup.
        popup.update(cx, |_, cx| cx.emit(NotificationEvent::Show));
        cx.run_until_parked();
        app.read_with(cx, |app, _| {
            assert_eq!(app.active, Some(key("a")));
            assert!(!app.popups.contains_key(&key("a")));
            assert!(app.popups.contains_key(&key("b")) && app.popups.contains_key(&key("shown")));
        });
        assert_eq!(windows(cx), 3);

        // Opening a session from the sidebar answers its popup too.
        open(&app, "b", cx);
        app.read_with(cx, |app, _| assert!(!app.popups.contains_key(&key("b"))));
        assert_eq!(windows(cx), 2);

        // Let the terminals' attach attempts end before the server cleans up.
        wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            app.read_with(cx, |app, cx| {
                app.terminals.values().all(|t| t.read(cx).exited())
            })
        });
    }

    /// A popup that takes the keyboard without a click, as ⌘Tab back to
    /// lemur can make it, hands it to the main window; back there, the
    /// shown session's popup closes, as Zed's do, and the others wait.
    #[gpui::test]
    fn popups_hand_the_keyboard_back(cx: &mut gpui::TestAppContext) {
        let cfg: Config = toml::from_str("").unwrap();
        cx.update(gpui_kit::init);
        let (app, cx) = cx.add_window_view(|window, cx| Lemur::new(cfg, None, window, cx));
        app.update(cx, |app, _| app.active = Some(key("shown")));
        apply(
            &app,
            &[("shown", Activity::Working), ("a", Activity::Working)],
            cx,
        );
        apply(
            &app,
            &[("shown", Activity::Idle), ("a", Activity::Idle)],
            cx,
        );
        let popup = app.read_with(cx, |app, _| app.popups[&key("a")].0);
        // The test pointer starts at the popup's corner.
        let pointer = |event: PlatformInput, cx: &mut VisualTestContext| {
            cx.update(|_, cx| {
                cx.update_window(popup.into(), |_, window, cx| {
                    window.dispatch_event(event, cx)
                })
            })
            .unwrap();
            cx.run_until_parked();
        };
        let key_to_popup = |cx: &mut VisualTestContext| {
            cx.update(|_, cx| popup.update(cx, |_, window, _| window.activate_window()))
                .unwrap();
            cx.run_until_parked();
        };
        let main_active =
            |cx: &mut VisualTestContext| cx.update(|window, _| window.is_window_active());

        pointer(
            PlatformInput::MouseExited(MouseExitEvent {
                position: point(px(-5.0), px(-5.0)),
                pressed_button: None,
                modifiers: Modifiers::none(),
            }),
            cx,
        );
        key_to_popup(cx);
        assert!(main_active(cx));
        app.read_with(cx, |app, _| {
            assert!(!app.popups.contains_key(&key("shown")), "you're back");
            assert!(app.popups.contains_key(&key("a")), "not answered yet");
        });

        // A click has the pointer over it, and the popup keeps the keyboard.
        pointer(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: point(px(10.0), px(10.0)),
                pressed_button: None,
                modifiers: Modifiers::none(),
            }),
            cx,
        );
        key_to_popup(cx);
        assert!(!main_active(cx));
    }

    /// Opening a thread unfolds its project, its worktree and the Hidden row
    /// it's under, for good; the header names all three.
    #[gpui::test]
    fn opening_a_thread_unfolds_its_way(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("reveal");
        let dir = TempDir::new("reveal");
        let repo = dir.join("api");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| git_in(&repo, args);
        git(&["init", "-q", "-b", "main"]);
        git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        git(&["worktree", "add", "-q", "-b", "a", "wt/a"]);
        let wt = repo.join("wt/a").to_string_lossy().into_owned();
        let thread = server.run(&wt, &["lemur.label=t"], &["sleep", "600"]);
        let thread = thread.as_str();
        let mut cfg = server.config();
        cfg.bases.push(Base::new("local", &repo.to_string_lossy()));
        let (app, cx) = rooted(cfg, cx);
        cx.simulate_resize(size(px(1000.0), px(600.0)));
        cx.run_until_parked();
        let a = app.read_with(cx, |app, _| {
            app.snap.hosts[0].bases[0].worktrees[1].path.clone()
        });
        app.update(cx, |app, cx| {
            app.set_worktree_hidden(0, &a, true, cx);
            app.toggle_worktree(0, &a, cx);
            app.toggle_project(0, cx);
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("buttons a").is_none());

        open(&app, thread, cx);
        cx.run_until_parked();
        assert!(cx.debug_bounds("buttons a").is_some());
        assert!(cx.debug_bounds("thread-0").is_some());
        app.read_with(cx, |app, _| {
            let base = &app.cfg.bases[0];
            assert!(!base.collapsed && base.collapsed_worktrees.is_empty() && base.hidden_open);
            assert_eq!(
                app.titles("local", thread),
                (Some("api › a".to_string()), "t".to_string())
            );
        });
    }

    /// A snapshot of project 0 on `local`: its main checkout, `/srv/api`,
    /// and its worktree `auth` on branch `feature/auth`, which has
    /// `threads`; the host read as `machine`.
    fn project_snapshot(threads: &[&str], machine: crate::machine::Reading) -> Snapshot {
        let sessions = |names: &[&str]| -> Vec<Session> {
            let pairs: Vec<_> = names.iter().map(|n| (*n, Activity::Idle)).collect();
            snapshot(&pairs).hosts.remove(0).other
        };
        let worktree = |path: &str, branch: &str, main: bool, threads: &[&str]| Worktree {
            path: path.into(),
            branch: Some(branch.into()),
            head: None,
            main,
            sessions: sessions(threads),
            gone: vec![],
        };
        Snapshot {
            hosts: vec![HostView {
                name: "local".into(),
                error: None,
                home: String::new(),
                machine,
                bases: vec![BaseView {
                    index: 0,
                    error: None,
                    plain: false,
                    worktrees: vec![
                        worktree("/srv/api", "main", true, &["main-shell"]),
                        worktree(
                            "/srv/api/.claude/worktrees/auth",
                            "feature/auth",
                            false,
                            threads,
                        ),
                    ],
                }],
                other: vec![],
                gone: vec![],
            }],
        }
    }

    /// Lets the terminals' attach attempts end, attached or not, before the
    /// server cleans up.
    fn let_attaches_end(app: &Entity<Lemur>, cx: &mut VisualTestContext) {
        let ended = wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            app.read_with(cx, |app, cx| {
                app.terminals.values().all(|t| !t.read(cx).attaching())
            })
        });
        assert!(ended, "a terminal still attaching");
    }

    /// The tabs are the threads of the worktree on show: a click opens one,
    /// and its × closes it, asking first while its program runs.
    #[gpui::test]
    fn tabs_are_the_threads_of_the_worktree_on_show(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("tabs");
        let dir = TempDir::new("tabs");
        let repo = dir.join("api");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        git_in(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        git_in(&repo, &["worktree", "add", "-q", "-b", "auth", "wt/auth"]);
        let wt = repo.join("wt/auth").to_string_lossy().into_owned();
        let a = server.run(&wt, &["lemur.label=a"], &["sleep", "600"]);
        let b = server.run(&wt, &["lemur.label=b"], &["sleep", "600"]);
        server.run(
            &repo.to_string_lossy(),
            &["lemur.label=c"],
            &["sleep", "600"],
        );
        let mut cfg = server.config();
        cfg.bases.push(Base::new("local", &repo.to_string_lossy()));
        let (app, cx) = rooted(cfg, cx);
        cx.simulate_resize(size(px(1200.0), px(700.0)));
        open(&app, &a, cx);
        cx.run_until_parked();
        let tab = |i: usize, cx: &mut VisualTestContext| {
            cx.debug_bounds(Box::leak(format!("tab-{i}").into_boxed_str()))
        };
        assert!(tab(0, cx).is_some() && tab(1, cx).is_some());
        assert!(tab(2, cx).is_none(), "c runs in another worktree");

        let second = tab(1, cx).unwrap().center();
        cx.simulate_click(second, Modifiers::none());
        let shown = app.read_with(cx, |app, _| app.active.clone());
        assert_eq!(shown, Some(key(&b)));

        // The first tab's ×, at its right end.
        let first = tab(0, cx).unwrap();
        let close = point(first.right() - px(10.0), first.center().y);
        cx.simulate_click(close, Modifiers::none());
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(server.addresses().contains(&a), "cancelled");
        cx.simulate_click(close, Modifiers::none());
        cx.simulate_keystrokes("enter");
        assert!(wait_until(Duration::from_secs(5), || !server
            .addresses()
            .contains(&a)));
        app.update(cx, |app, cx| app.refresh(cx).detach());
        cx.run_until_parked();
        assert!(tab(0, cx).is_some() && tab(1, cx).is_none());
        let_attaches_end(&app, cx);
    }

    /// Beside the branch, the status bar counts the lines added and removed
    /// in the worktree on show since its last commit; nothing while it has
    /// none.
    #[gpui::test]
    fn the_status_bar_counts_the_worktrees_changes(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("changes");
        let dir = TempDir::new("changes-gui");
        let repo = dir.join("api");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
        git_in(&repo, &["add", "a.txt"]);
        git_in(&repo, &["commit", "-q", "-m", "init"]);
        git_in(&repo, &["worktree", "add", "-q", "-b", "auth", "wt/auth"]);
        let wt = repo.join("wt/auth");
        std::fs::write(wt.join("a.txt"), "one\n2\nthree\n").unwrap();
        let clean = server.run(
            &repo.to_string_lossy(),
            &["lemur.label=clean"],
            &["sleep", "600"],
        );
        let changed = server.run(
            &wt.to_string_lossy(),
            &["lemur.label=changed"],
            &["sleep", "600"],
        );
        let mut cfg = server.config();
        cfg.bases.push(Base::new("local", &repo.to_string_lossy()));
        let (app, cx) = rooted(cfg, cx);
        cx.simulate_resize(size(px(1200.0), px(700.0)));
        let listed = wait_until(Duration::from_secs(5), || {
            app.update(cx, |app, cx| app.refresh(cx).detach());
            cx.run_until_parked();
            app.read_with(cx, |app, _| {
                app.project_of("local", &changed).is_some()
                    && app.project_of("local", &clean).is_some()
            })
        });
        assert!(listed);
        let counted = |cx: &mut VisualTestContext| {
            wait_until(Duration::from_secs(5), || {
                cx.run_until_parked();
                app.read_with(cx, |app, _| app.diffs.asking.is_empty())
            });
            cx.debug_bounds("changes").is_some()
        };

        open(&app, &changed, cx);
        assert!(counted(cx), "+2 -1");
        open(&app, &clean, cx);
        assert!(!counted(cx), "the main checkout has none");
        let_attaches_end(&app, cx);
    }

    /// The Linear panel lists your issues, and a click pins one to the
    /// thread on show: herdr-core keeps it, and the sidebar shows it. Send
    /// to agent pastes its link at the thread's prompt; Unpin unpins it. A
    /// link to an issue pasted into a thread with none pins that one.
    #[gpui::test]
    fn linear_issues_pin_to_the_thread_on_show(cx: &mut gpui::TestAppContext) {
        use crate::linear::{Linear, Store};
        use crate::test_support::FakeLinear;
        use gpui_kit::ClipboardItem;
        use serde_json::json;
        let api = FakeLinear::new(|r| {
            let issue = json!({
                "identifier": "LAFT-1",
                "title": "Fix lease rounding",
                "url": "https://linear.app/laft/issue/LAFT-1/fix",
                "branchName": "magnus/laft-1-fix",
                "priorityLabel": "High",
                "description": "Round *down*.",
                "state": { "name": "Todo", "color": "#e2e2e2", "type": "unstarted" },
                "assignee": { "name": "Magnus" },
            });
            let data = if r.body.contains("assignedIssues") {
                json!({ "viewer": { "assignedIssues": { "nodes": [issue] } } })
            } else if r.body.contains("issue(id") {
                json!({ "issue": issue })
            } else {
                json!({ "viewer": { "name": "Magnus" }, "organization": { "name": "Laft" } })
            };
            (200, json!({ "data": data }).to_string())
        });
        let dir = TempDir::new("linear-gui");
        let linear = Linear::at(&api.url, Store::File(dir.join("tokens")));
        linear.sign_in_as("a").unwrap();
        let server = HcServer::new("linear-gui");
        let t = server.run("/", &["lemur.label=fix"], &["cat"]);
        cx.update(|cx| super::linear_panel::init(linear, cx));
        let (app, cx) = rooted(server.config(), cx);
        cx.simulate_resize(size(px(1400.0), px(800.0)));
        assert!(wait_until(Duration::from_secs(5), || {
            app.update(cx, |app, cx| app.refresh(cx).detach());
            cx.run_until_parked();
            app.read_with(cx, |app, _| app.session_of(&key(&t)).is_some())
        }));
        open(&app, &t, cx);
        let view = app.read_with(cx, |app, _| app.terminals[&key(&t)].clone());
        assert!(wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            view.read_with(cx, |v, _| v.screen_size()).is_some()
        }));
        // Hidden, it asks Linear nothing; signed in, Lemur asked who for.
        let asked = api.requests();
        assert!(asked.iter().any(|r| r.body.contains("viewer { name }")));
        assert!(!asked.iter().any(|r| r.body.contains("assignedIssues")));
        cx.dispatch_action(super::ToggleLinear);
        let shows = |selector: &'static str, cx: &mut VisualTestContext| {
            wait_until(Duration::from_secs(5), || {
                cx.run_until_parked();
                cx.debug_bounds(selector).is_some()
            })
        };
        let click = |selector: &'static str, cx: &mut VisualTestContext| {
            let at = cx.debug_bounds(selector).expect(selector).center();
            cx.simulate_click(at, Modifiers::none());
            cx.run_until_parked();
        };
        let pinned = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, _| {
                app.session_of(&key(&t)).and_then(|s| s.issue.clone())
            })
        };
        let kept = || {
            server
                .core()
                .terminals()
                .into_iter()
                .find(|x| x.address == t)
                .and_then(|x| x.labels.get(snapshot::ISSUE).cloned())
        };
        let shown = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, cx| app.linear_panel.read(cx).shown().cloned())
        };
        assert!(shows("issue 0", cx), "your issues");
        assert_eq!(shown(cx), Some((key(&t), "fix".to_string(), None)));
        click("issue 0", cx);
        assert_eq!(pinned(cx).as_deref(), Some("LAFT-1"));
        assert_eq!(shown(cx).and_then(|s| s.2).as_deref(), Some("LAFT-1"));
        assert!(wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            kept().as_deref() == Some("LAFT-1")
        }));

        assert!(shows("send to agent", cx), "its card");
        click("send to agent", cx);
        assert!(
            wait_until(Duration::from_secs(5), || {
                server
                    .hc(&["read", &t])
                    .contains("https://linear.app/laft/issue/LAFT-1/fix")
            }),
            "{}",
            server.hc(&["read", &t])
        );
        click("unpin", cx);
        assert_eq!(pinned(cx), None);
        assert!(wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            kept().is_none()
        }));

        // Pasted: pinned, but not over one pinned already.
        let paste = |text: &str, cx: &mut VisualTestContext| {
            cx.update(|window, cx| app.update(cx, |app, cx| app.focus_terminal(window, cx)));
            cx.write_to_clipboard(ClipboardItem::new_string(text.into()));
            cx.simulate_keystrokes("cmd-v");
            cx.run_until_parked();
        };
        paste("see https://linear.app/laft/issue/LAFT-9/rounding", cx);
        assert_eq!(pinned(cx).as_deref(), Some("LAFT-9"));
        assert!(wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            kept().as_deref() == Some("LAFT-9")
        }));
        paste("and https://linear.app/laft/issue/LAFT-10", cx);
        assert_eq!(pinned(cx).as_deref(), Some("LAFT-9"));
        paste("no link", cx);
        assert_eq!(pinned(cx).as_deref(), Some("LAFT-9"));
        let asked = api.requests();
        assert!(
            asked
                .iter()
                .all(|r| r.authorization.as_deref() == Some("Bearer a"))
        );

        // Kept to resume, the thread is pinned again then, Claude running
        // in it or not.
        let thread = crate::config::ClaudeThread {
            host: "local".into(),
            session: t.clone(),
            path: "/".into(),
            claude_session: crate::actions::random_uuid(),
            label: None,
            issue: None,
        };
        cx.update(|_, cx| app.update(cx, |app, cx| app.keep_thread(thread, cx)));
        cx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.pin_issue(key(&t), Some("LAFT-12".into()), cx)
            })
        });
        let kept = app.read_with(cx, |app, _| app.cfg.threads[0].issue.clone());
        assert_eq!(kept.as_deref(), Some("LAFT-12"));
    }

    /// A program that asks for focus reports hears it lost focus when
    /// another thread shows instead of it, and got it back when it shows
    /// again.
    #[gpui::test]
    fn a_thread_shown_no_more_hears_it_lost_focus(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("focus-out");
        let a = server.run(
            "/",
            &["lemur.label=a"],
            &["sh", "-c", r"printf '\033[?1004h'; exec cat -v"],
        );
        let b = server.run("/", &["lemur.label=b"], &["sleep", "600"]);
        let (app, cx) = rooted(server.config(), cx);
        cx.simulate_resize(size(px(1200.0), px(700.0)));
        let heard = |what: &str, cx: &mut VisualTestContext| {
            wait_until(Duration::from_secs(5), || {
                cx.run_until_parked();
                server.hc(&["read", &a]).contains(what)
            })
        };
        open(&app, &a, cx);
        assert!(heard("^[[I", cx), "{}", server.hc(&["read", &a]));
        open(&app, &b, cx);
        assert!(heard("^[[O", cx), "{}", server.hc(&["read", &a]));
        let_attaches_end(&app, cx);
    }

    /// When the thread on show ends and none shows instead, the Changes
    /// panel no longer lists its worktree's changes, nor commits there.
    #[gpui::test]
    fn the_changes_panel_empties_with_nothing_on_show(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("chpanel-none");
        let dir = TempDir::new("changes-none");
        let repo = dir.join("api");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git_in(&repo, &["add", "a.txt"]);
        git_in(&repo, &["commit", "-q", "-m", "init"]);
        std::fs::write(repo.join("a.txt"), "two\n").unwrap();
        let t = server.run(
            &repo.to_string_lossy(),
            &["lemur.label=t"],
            &["sleep", "600"],
        );
        let mut cfg = server.config();
        cfg.bases.push(Base::new("local", &repo.to_string_lossy()));
        let (app, cx) = rooted(cfg, cx);
        assert!(wait_until(Duration::from_secs(5), || {
            app.update(cx, |app, cx| app.refresh(cx).detach());
            cx.run_until_parked();
            app.read_with(cx, |app, _| app.project_of("local", &t).is_some())
        }));
        open(&app, &t, cx);
        cx.dispatch_action(super::ToggleChanges);
        let files = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, cx| app.changes_panel.read(cx).listed().0)
        };
        assert!(wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            !files(cx).is_empty()
        }));
        server.hc(&["kill", &t]);
        let ended = wait_until(Duration::from_secs(5), || {
            cx.executor().advance_clock(super::BURST);
            app.update(cx, |app, cx| app.refresh(cx).detach());
            cx.run_until_parked();
            app.read_with(cx, |app, _| app.active.is_none())
        });
        assert!(ended);
        assert!(files(cx).is_empty(), "{:?}", files(cx));
    }

    /// The Changes panel lists the worktree on show's changes, and a click
    /// on one shows its diff; it follows the thread on show.
    #[gpui::test]
    fn the_changes_panel_lists_and_diffs(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("chpanel");
        let dir = TempDir::new("changes-gui-panel");
        let repo = dir.join("api");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
        git_in(&repo, &["add", "a.txt"]);
        git_in(&repo, &["commit", "-q", "-m", "init"]);
        git_in(&repo, &["worktree", "add", "-q", "-b", "auth", "wt/auth"]);
        git_in(&repo, &["worktree", "add", "-q", "-b", "docs", "wt/docs"]);
        let wt = repo.join("wt/auth");
        std::fs::write(wt.join("a.txt"), "one\n2\n").unwrap();
        let clean = server.run(
            &repo.join("wt/docs").to_string_lossy(),
            &["lemur.label=clean"],
            &["sleep", "600"],
        );
        let changed = server.run(
            &wt.to_string_lossy(),
            &["lemur.label=changed"],
            &["sleep", "600"],
        );
        let mut cfg = server.config();
        cfg.bases.push(Base::new("local", &repo.to_string_lossy()));
        let (app, cx) = rooted(cfg, cx);
        cx.update(|_, cx| super::bind_keys(cx));
        cx.simulate_resize(size(px(1400.0), px(800.0)));
        assert!(wait_until(Duration::from_secs(5), || {
            app.update(cx, |app, cx| app.refresh(cx).detach());
            cx.run_until_parked();
            app.read_with(cx, |app, _| {
                app.project_of("local", &changed).is_some()
                    && app.project_of("local", &clean).is_some()
            })
        }));
        open(&app, &changed, cx);
        cx.dispatch_action(super::ToggleChanges);
        let listed = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, cx| app.changes_panel.read(cx).listed())
        };
        let files = |cx: &mut VisualTestContext| listed(cx).0;
        assert!(wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            !files(cx).is_empty()
        }));
        assert_eq!(files(cx), [("a.txt".to_string(), 'M')]);
        let at = cx.debug_bounds("change 0").expect("its row").center();
        cx.simulate_click(at, Modifiers::none());
        assert!(wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            listed(cx).1.len() > 1
        }));
        assert_eq!(listed(cx).1, ["@@ -1,2 +1,2 @@", " one", "-two", "+2"]);

        open(&app, &clean, cx);
        assert!(wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            files(cx).is_empty()
        }));
        let_attaches_end(&app, cx);
    }

    /// ⌘W asks before closing the thread on show; closed, or ended any
    /// other way, the one opened before it in its worktree shows instead,
    /// else its neighbour there.
    #[gpui::test]
    fn closing_a_thread_shows_another(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("close");
        let [a, b, c] = ["a", "b", "c"]
            .map(|l| server.run("/", &[&format!("lemur.label={l}")], &["sleep", "600"]));
        let (app, cx) = rooted(server.config(), cx);
        cx.update(|_, cx| super::bind_keys(cx));
        cx.simulate_resize(size(px(1200.0), px(700.0)));
        for t in [&c, &a, &b] {
            open(&app, t, cx);
        }
        let shown = |cx: &mut VisualTestContext| app.read_with(cx, |app, _| app.active.clone());
        let refresh = |cx: &mut VisualTestContext| {
            app.update(cx, |app, cx| app.refresh(cx).detach());
            cx.run_until_parked();
        };

        cx.simulate_keystrokes("cmd-w");
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(server.addresses().contains(&b), "cancelled");
        cx.simulate_keystrokes("cmd-w");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(wait_until(Duration::from_secs(5), || !server
            .addresses()
            .contains(&b)));
        refresh(cx);
        assert_eq!(shown(cx), Some(key(&a)), "the one opened before");
        // As the dialog finishes closing, it gives the keyboard back to the
        // terminal it took it from, which is gone.
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        // It has the keyboard: ⌘J works at once.
        let focused = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                let app = app.read(cx);
                let view = app.active.as_ref().and_then(|k| app.terminals.get(k));
                view.is_some_and(|v| {
                    gpui_kit::Focusable::focus_handle(v.read(cx), cx).is_focused(window)
                })
            })
        };
        assert!(focused(cx), "the terminal shown instead has the keyboard");
        cx.simulate_keystrokes("cmd-j");
        cx.run_until_parked();
        app.read_with(cx, |app, _| assert!(app.prompt.is_some(), "⌘J answered"));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();

        // Ended elsewhere, with none opened before it: its neighbour.
        app.update(cx, |app, _| app.recent.retain(|k| *k == key(&a)));
        server.hc(&["keep", &a, "--off"]);
        server.hc(&["kill", &a]);
        assert!(wait_until(Duration::from_secs(5), || !server
            .addresses()
            .contains(&a)));
        refresh(cx);
        assert_eq!(shown(cx), Some(key(&c)));
        assert!(focused(cx), "the neighbour has the keyboard");
        let_attaches_end(&app, cx);
    }

    /// The agents panel lists each Claude thread with what it runs under
    /// it: Claude's subagents as it reports them (OSC 7501 records), and
    /// terminals its agent started (`hc run`, parent and all). A click opens
    /// a helper's terminal, which stays open across refreshes; a subagent's
    /// shows its thread. The sidebar only counts them.
    #[gpui::test]
    fn the_agents_panel_shows_what_threads_run(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("helpers");
        // Reports a subagent, its task in base64 as Claude Code sends it.
        let report = r#"t=$(printf 'Explore code' | base64); printf '\033]7501;state=working:app=claude-code:id=sub1:title=%s\033\\' "$t"; exec sleep 600"#;
        let a = server.run("/", &["lemur.label=main"], &["sh", "-c", report]);
        let out = server.hc(&[
            "run",
            "--bg",
            "--json",
            "--parent",
            &a,
            "--label",
            "lemur.label=helper",
            "--",
            "sleep",
            "600",
        ]);
        let started: serde_json::Value = serde_json::from_str(&out).unwrap();
        let b = started["address"].as_str().unwrap().to_string();
        let (app, cx) = rooted(server.config(), cx);
        cx.update(|_, cx| super::bind_keys(cx));
        cx.simulate_resize(size(px(1200.0), px(700.0)));
        let helpers = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, _| {
                let threads = &app.snap.hosts[0].other;
                let names: Vec<String> = threads.iter().map(|s| s.name.clone()).collect();
                let helpers: Vec<(String, bool)> = threads
                    .iter()
                    .flat_map(|s| s.helpers.iter().map(|h| (h.label.clone(), h.terminal)))
                    .collect();
                (names, helpers)
            })
        };
        let shown = wait_until(Duration::from_secs(10), || {
            app.update(cx, |app, cx| app.refresh(cx).detach());
            cx.run_until_parked();
            helpers(cx).1.len() == 2
        });
        assert!(shown, "{:?}", helpers(cx));
        assert_eq!(
            helpers(cx),
            (
                vec![a.clone()],
                vec![("Explore code".into(), false), ("helper".into(), true)]
            ),
            "the helper's terminal isn't a thread of its own"
        );
        assert!(
            cx.debug_bounds("helpers main").is_some(),
            "the sidebar counts them"
        );
        assert!(
            cx.debug_bounds("agent-0-0").is_none(),
            "the panel starts hidden"
        );

        cx.simulate_keystrokes("cmd-shift-a");
        cx.run_until_parked();
        let click = |selector: &'static str, cx: &mut VisualTestContext| {
            let at = cx.debug_bounds(selector).expect(selector).center();
            cx.simulate_click(at, Modifiers::none());
            cx.run_until_parked();
        };
        let active = |cx: &mut VisualTestContext| app.read_with(cx, |app, _| app.active.clone());
        let peeked = |cx: &mut VisualTestContext| app.read_with(cx, |app, _| app.peek.clone());
        // A helper's terminal is peeked at, over the one on show.
        click("agent-0-0", cx);
        assert_eq!(active(cx), Some(key(&a)), "a subagent shows its thread");
        click("agent-0-1", cx);
        assert_eq!(peeked(cx), Some(key(&b)));
        assert_eq!(active(cx), Some(key(&a)), "still on show under it");
        assert!(cx.debug_bounds("peek").is_some());
        app.update(cx, |app, cx| app.refresh(cx).detach());
        cx.run_until_parked();
        assert_eq!(peeked(cx), Some(key(&b)), "kept across a refresh");
        // A click outside puts it away again.
        cx.simulate_click(point(px(600.0), px(20.0)), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(peeked(cx), None);
        click("agent-0-1", cx);
        // ↗ shows it instead.
        click("peek-open", cx);
        assert_eq!((active(cx), peeked(cx)), (Some(key(&b)), None));
        app.update(cx, |app, cx| app.refresh(cx).detach());
        cx.run_until_parked();
        assert_eq!(active(cx), Some(key(&b)), "kept open across a refresh");
        let_attaches_end(&app, cx);
    }

    /// A thread put away leaves the sidebar's list and the tabs for "Put
    /// away", still running, and herdr-core keeps it so; on show, another
    /// shows instead. Opened again, it's back; and one that needs you
    /// comes back on its own.
    #[gpui::test]
    fn a_thread_put_away_comes_back_when_it_needs_you(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("away");
        let a = server.run("/", &["lemur.label=a"], &["sleep", "600"]);
        // Asks for you once it's told to.
        let asks =
            r#"read x; printf '\033]7501;state=blocked:app=claude-code\033\\'; exec sleep 600"#;
        let b = server.run("/", &["lemur.label=b"], &["sh", "-c", asks]);
        let (app, cx) = rooted(server.config(), cx);
        cx.update(|_, cx| super::bind_keys(cx));
        cx.simulate_resize(size(px(1200.0), px(700.0)));
        open(&app, &a, cx);
        open(&app, &b, cx);
        cx.run_until_parked();
        let labelled = |t: &str| {
            server
                .core()
                .terminals()
                .into_iter()
                .find(|x| x.address == t)
                .is_some_and(|x| x.labels.contains_key(crate::snapshot::AWAY))
        };
        let listed = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, _| {
                app.snap.hosts[0]
                    .other
                    .iter()
                    .filter(|s| !s.away)
                    .map(|s| s.name.clone())
                    .collect::<Vec<_>>()
            })
        };
        let refresh = |cx: &mut VisualTestContext| {
            app.update(cx, |app, cx| app.refresh(cx).detach());
            cx.run_until_parked();
        };

        cx.dispatch_action(super::PutAway);
        cx.run_until_parked();
        assert_eq!(
            app.read_with(cx, |app, _| app.active.clone()),
            Some(key(&a))
        );
        assert_eq!(listed(cx), std::slice::from_ref(&a));
        assert!(wait_until(Duration::from_secs(5), || labelled(&b)));
        refresh(cx);
        assert_eq!(
            listed(cx),
            std::slice::from_ref(&a),
            "herdr-core keeps it away"
        );
        assert!(cx.debug_bounds("away").is_some(), "under Put away");
        assert!(cx.debug_bounds("tab-1").is_none(), "nor a tab");

        open(&app, &b, cx);
        let back = wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            !labelled(&b)
        });
        assert!(back, "opened, it's back");
        refresh(cx);
        assert_eq!(listed(cx), [a.clone(), b.clone()]);

        // Away again, it asks for you: it's back.
        cx.update(|window, cx| app.update(cx, |app, cx| app.put_away(key(&b), window, cx)));
        assert!(wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            labelled(&b)
        }));
        server.hc(&["send", &b, "go"]);
        let back = wait_until(Duration::from_secs(10), || {
            refresh(cx);
            !labelled(&b)
        });
        assert!(back, "it needs you");
        refresh(cx);
        assert_eq!(listed(cx), [a.clone(), b.clone()]);
        let_attaches_end(&app, cx);
    }

    /// A thread put away doesn't show instead of one that ended, nor when
    /// Lemur starts again: it stays away.
    #[gpui::test]
    fn a_thread_put_away_doesnt_come_back_on_its_own(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("away-stays");
        let [x, y] =
            ["x", "y"].map(|l| server.run("/", &[&format!("lemur.label={l}")], &["sleep", "600"]));
        let (app, cx) = rooted(server.config(), cx);
        cx.simulate_resize(size(px(1200.0), px(700.0)));
        open(&app, &x, cx);
        open(&app, &y, cx);
        cx.update(|window, cx| app.update(cx, |app, cx| app.put_away(key(&x), window, cx)));
        let away = |cx: &mut VisualTestContext| {
            app.update(cx, |app, cx| app.refresh(cx).detach());
            cx.run_until_parked();
            app.read_with(cx, |app, _| {
                app.session_of(&key(&x)).is_some_and(|s| s.away)
            })
        };
        assert!(wait_until(Duration::from_secs(5), || away(cx)));

        server.hc(&["kill", &y]);
        let ended = wait_until(Duration::from_secs(5), || {
            cx.executor().advance_clock(super::BURST);
            app.update(cx, |app, cx| app.refresh(cx).detach());
            cx.run_until_parked();
            app.read_with(cx, |app, _| app.session_of(&key(&y)).is_none())
        });
        assert!(ended);
        app.read_with(cx, |app, _| {
            assert_eq!(app.active, None, "x isn't shown instead")
        });
        assert!(away(cx), "still away");

        // Shown last before Lemur quit, it isn't opened when it starts.
        app.update(cx, |app, _| app.restore = Some(key(&x)));
        assert!(away(cx));
        app.read_with(cx, |app, _| {
            assert_eq!(app.active, None, "nor when Lemur starts")
        });
        let_attaches_end(&app, cx);
    }

    /// A list whose every thread is put away still shows them, under "Put
    /// away", with their way back.
    #[gpui::test]
    fn a_list_of_threads_all_put_away_still_shows_them(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("all-away");
        let a = server.run("/", &["lemur.label=a"], &["sleep", "600"]);
        let (app, cx) = rooted(server.config(), cx);
        cx.simulate_resize(size(px(1200.0), px(700.0)));
        open(&app, &a, cx);
        cx.run_until_parked();
        cx.update(|window, cx| app.update(cx, |app, cx| app.put_away(key(&a), window, cx)));
        let away = wait_until(Duration::from_secs(5), || {
            app.update(cx, |app, cx| app.refresh(cx).detach());
            cx.run_until_parked();
            app.read_with(cx, |app, _| {
                app.session_of(&key(&a)).is_some_and(|s| s.away)
            })
        });
        assert!(away, "put away");
        assert!(cx.debug_bounds("away").is_some(), "under Put away");
        let_attaches_end(&app, cx);
    }

    /// ⌘⇧P lists every command; what's typed narrows them, ↓ picks the
    /// next, and ⏎ runs it as its key would. Esc closes it.
    #[gpui::test]
    fn the_palette_runs_commands_by_name(cx: &mut gpui::TestAppContext) {
        let (app, cx) = rooted(toml::from_str("").unwrap(), cx);
        cx.update(|_, cx| super::bind_keys(cx));
        let open = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, _| {
                (
                    app.palette.is_some(),
                    app.shows(Right::Notes),
                    app.shows(Right::Agents),
                )
            })
        };
        cx.simulate_keystrokes("cmd-shift-p");
        assert_eq!(open(cx), (true, false, false));
        assert!(cx.debug_bounds("command 0").is_some());
        cx.simulate_input("toggle agents");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(open(cx), (false, false, true), "Toggle Agents ran");

        cx.simulate_keystrokes("cmd-shift-p");
        cx.simulate_input("toggle");
        // Toggle Sidebar, Right Panel, Notes: the third.
        cx.simulate_keystrokes("down down enter");
        cx.run_until_parked();
        assert_eq!(open(cx), (false, true, false), "Toggle Notes ran");

        cx.simulate_keystrokes("cmd-shift-p escape");
        assert_eq!(open(cx), (false, true, false));
    }

    /// ⌘] and ⌘[ go through the agents in the panel's order, round and
    /// round, starting from the first.
    #[gpui::test]
    fn cmd_brackets_go_through_the_agents(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("next-agent");
        let (app, cx) = rooted(server.config(), cx);
        cx.update(|_, cx| super::bind_keys(cx));
        apply(
            &app,
            &[
                ("idle", Activity::Idle),
                ("asks", Activity::NeedsInput),
                ("works", Activity::Working),
                ("shell", Activity::Unknown),
            ],
            cx,
        );
        let mut seen = vec![];
        for keys in ["cmd-]", "cmd-]", "cmd-]", "cmd-]", "cmd-[", "cmd-["] {
            cx.simulate_keystrokes(keys);
            seen.push(app.read_with(cx, |app, _| app.active.clone().unwrap().1));
        }
        assert_eq!(seen, ["asks", "works", "idle", "asks", "idle", "works"]);
        let_attaches_end(&app, cx);
    }

    /// The agents panel lists Claude threads, the most urgent first, and
    /// takes turns with the notes on the right.
    #[gpui::test]
    fn agents_come_most_urgent_first(cx: &mut gpui::TestAppContext) {
        let (app, cx) = rooted(toml::from_str("").unwrap(), cx);
        cx.update(|_, cx| super::bind_keys(cx));
        apply(
            &app,
            &[
                ("idle", Activity::Idle),
                ("asks", Activity::NeedsInput),
                ("works", Activity::Working),
                ("shell", Activity::Unknown),
            ],
            cx,
        );
        let listed = app.read_with(cx, |app, _| {
            app.agents()
                .iter()
                .map(|(_, s)| s.name.clone())
                .collect::<Vec<_>>()
        });
        assert_eq!(listed, ["asks", "works", "idle"], "a plain shell isn't one");

        let open = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, _| {
                (app.shows(Right::Notes), app.shows(Right::Agents))
            })
        };
        cx.simulate_keystrokes("cmd-r");
        assert_eq!(open(cx), (true, false), "⌘R: the notes first");
        cx.simulate_keystrokes("cmd-shift-a");
        assert_eq!(open(cx), (false, true), "one or the other");
        cx.simulate_keystrokes("cmd-shift-a");
        assert_eq!(open(cx), (false, false));
        // ⌘R brings back the one shown last, and hides whichever shows.
        cx.simulate_keystrokes("cmd-r");
        assert_eq!(open(cx), (false, true));
        cx.simulate_keystrokes("cmd-r");
        assert_eq!(open(cx), (false, false));
        app.update(cx, |app, cx| app.toggle_notes(cx));
        cx.simulate_keystrokes("cmd-r cmd-r");
        assert_eq!(open(cx), (true, false), "the notes again");
    }

    /// With hosts in two herdr-cores, one lost and back is watched again,
    /// while the other's changes go on: its new threads show without a
    /// refresh.
    #[gpui::test]
    fn a_core_lost_and_back_is_watched_again(cx: &mut gpui::TestAppContext) {
        let a = HcServer::new("watch-a");
        let b = HcServer::new("watch-b");
        let mut cfg = a.config();
        let mut mac = Host::new("mac", "local");
        mac.hc_session = b.config().hosts[0].hc_session.clone();
        cfg.hosts.push(mac);
        let (app, cx) = rooted(cfg, cx);
        let listed = |label: &str, cx: &mut VisualTestContext| {
            app.read_with(cx, |app, _| {
                super::sessions(&app.snap).any(|(_, s)| s.label == label)
            })
        };
        let first = b.run("/", &["lemur.label=first"], &["sleep", "600"]);
        assert!(
            wait_until(Duration::from_secs(5), || {
                cx.executor().advance_clock(super::BURST);
                cx.run_until_parked();
                listed("first", cx)
            }),
            "b's changes reach Lemur"
        );
        assert!(!first.is_empty());

        b.hc(&["server", "stop"]);
        std::thread::sleep(Duration::from_millis(500));
        cx.run_until_parked();
        // Lemur watches again after a while: here one tick, short of the
        // refresh, which comes every 5 while the window is in the back.
        cx.executor().advance_clock(super::REFRESH);
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(500));
        cx.run_until_parked();
        b.run("/", &["lemur.label=later"], &["sleep", "600"]);
        let seen = wait_until(Duration::from_secs(5), || {
            cx.executor().advance_clock(super::BURST);
            cx.run_until_parked();
            listed("later", cx)
        });
        assert!(seen, "b's changes reach Lemur again");
    }

    /// A helper's terminal, peeked at from under its thread, attaches again
    /// on its own too once its stream drops.
    #[gpui::test]
    fn a_dropped_helper_reattaches_on_its_own(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("reattach-helper");
        let a = server.run("/", &["lemur.label=a"], &["sleep", "600"]);
        let out = server.hc(&[
            "run",
            "--bg",
            "--json",
            "--keep",
            "--parent",
            &a,
            "--label",
            "lemur.label=h",
            "--",
            "sh",
        ]);
        let started: serde_json::Value = serde_json::from_str(&out).unwrap();
        let b = started["address"].as_str().unwrap().to_string();
        let (app, cx) = rooted(server.config(), cx);
        cx.simulate_resize(size(px(1200.0), px(700.0)));
        open(&app, &a, cx);
        let listed = wait_until(Duration::from_secs(5), || {
            app.update(cx, |app, cx| app.refresh(cx).detach());
            cx.run_until_parked();
            app.read_with(cx, |app, _| {
                app.session_of(&key(&a))
                    .is_some_and(|s| s.helpers.iter().any(|h| h.id == b))
            })
        });
        assert!(listed, "b is a's helper");
        cx.update(|window, cx| app.update(cx, |app, cx| app.peek(key(&b), window, cx)));
        let view = app.read_with(cx, |app, _| app.terminals[&key(&b)].clone());
        let attached = |cx: &mut VisualTestContext| view.read_with(cx, |v, _| v.is_attached());
        assert!(wait_until(Duration::from_secs(10), || {
            cx.run_until_parked();
            attached(cx)
        }));
        let pid = server
            .core()
            .terminals()
            .into_iter()
            .find(|t| t.address == b)
            .and_then(|t| t.pid)
            .expect("its shell's pid");
        std::process::Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status()
            .unwrap();
        assert!(wait_until(Duration::from_secs(10), || {
            cx.run_until_parked();
            view.read_with(cx, |v, _| v.disconnected())
        }));
        let back = wait_until(Duration::from_secs(20), || {
            app.update(cx, |app, cx| app.refresh(cx).detach());
            cx.run_until_parked();
            attached(cx)
        });
        assert!(back, "attached again on its own");
        let_attaches_end(&app, cx);
    }

    /// A terminal whose stream dropped while its program may still run
    /// attaches again on its own once the program runs, as Herdr's do: here
    /// a kept terminal's shell dies by a signal and herdr-core starts a new
    /// one, as a host's terminals come back with its line. No click.
    #[gpui::test]
    fn a_dropped_terminal_reattaches_on_its_own(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("reattach");
        let a = server.run("/", &["lemur.label=a"], &["sh"]);
        let (app, cx) = rooted(server.config(), cx);
        cx.simulate_resize(size(px(1200.0), px(700.0)));
        open(&app, &a, cx);
        let view = app.read_with(cx, |app, _| app.terminals[&key(&a)].clone());
        let attached = |cx: &mut VisualTestContext| view.read_with(cx, |v, _| v.is_attached());
        assert!(wait_until(Duration::from_secs(10), || {
            cx.run_until_parked();
            attached(cx)
        }));

        let pid = server
            .core()
            .terminals()
            .into_iter()
            .find(|t| t.address == a)
            .and_then(|t| t.pid)
            .expect("its shell's pid");
        let killed = std::process::Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status()
            .unwrap();
        assert!(killed.success());
        assert!(
            wait_until(Duration::from_secs(10), || {
                cx.run_until_parked();
                view.read_with(cx, |v, _| v.disconnected())
            }),
            "its stream dropped"
        );
        let back = wait_until(Duration::from_secs(20), || {
            app.update(cx, |app, cx| app.refresh(cx).detach());
            cx.run_until_parked();
            attached(cx)
        });
        assert!(back, "attached again on its own");
        assert_eq!(
            app.read_with(cx, |app, _| app.active.clone()),
            Some(key(&a))
        );
        assert!(
            view.read_with(cx, |v, _| v.status().is_none()),
            "and says nothing"
        );
        let_attaches_end(&app, cx);
    }

    /// While a host can't be reached, the thread on show stays, terminal
    /// and all: its threads aren't known to be gone. Reached again without
    /// it, it goes.
    #[gpui::test]
    fn an_unreached_host_keeps_its_terminals(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("unreached");
        let a = server.run("/", &["lemur.label=a"], &["sleep", "600"]);
        let (app, cx) = rooted(server.config(), cx);
        cx.simulate_resize(size(px(1200.0), px(700.0)));
        open(&app, &a, cx);
        cx.run_until_parked();
        let shown = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, _| {
                (app.active.clone(), app.terminals.contains_key(&key(&a)))
            })
        };
        assert_eq!(shown(cx), (Some(key(&a)), true));

        let mut unreached = app.read_with(cx, |app, _| app.snap.clone());
        unreached.hosts[0].error = Some("the line dropped".into());
        unreached.hosts[0].other.clear();
        unreached.hosts[0].bases.clear();
        cx.update(|window, cx| app.update(cx, |app, cx| app.set_snapshot(unreached, window, cx)));
        assert_eq!(shown(cx), (Some(key(&a)), true));

        server.hc(&["keep", &a, "--off"]);
        server.hc(&["kill", &a]);
        let gone = wait_until(Duration::from_secs(5), || {
            app.update(cx, |app, cx| app.refresh(cx).detach());
            cx.run_until_parked();
            shown(cx) == (None, false)
        });
        assert!(gone, "{:?}", shown(cx));
        let_attaches_end(&app, cx);
    }

    /// A tab closed with its ×, as herdr-core tells of it, hands the
    /// keyboard to the one shown instead: ⌘J works at once. Its program
    /// running, the × asks first, as ⌘W does.
    #[gpui::test]
    fn the_tab_shown_after_a_close_has_the_keyboard(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("close-x");
        let [a, b] =
            ["a", "b"].map(|l| server.run("/", &[&format!("lemur.label={l}")], &["sleep", "600"]));
        let (app, cx) = rooted(server.config(), cx);
        cx.update(|_, cx| super::bind_keys(cx));
        cx.simulate_resize(size(px(1200.0), px(700.0)));
        open(&app, &a, cx);
        cx.run_until_parked();
        let shown = |cx: &mut VisualTestContext| app.read_with(cx, |app, _| app.active.clone());
        // a's ×; a runs sleep, so it asks.
        let tab = cx.debug_bounds("tab-0").expect("a's tab");
        let close = point(tab.right() - px(10.0), tab.center().y);
        cx.simulate_click(close, Modifiers::none());
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
        cx.simulate_keystrokes("enter");
        assert!(wait_until(Duration::from_secs(5), || !server
            .addresses()
            .contains(&a)));
        let switched = wait_until(Duration::from_secs(5), || {
            cx.executor().advance_clock(super::BURST);
            cx.run_until_parked();
            shown(cx) == Some(key(&b))
        });
        assert!(switched, "{:?}", shown(cx));
        let focused = cx.update(|window, cx| {
            let app = app.read(cx);
            let view = app.active.as_ref().and_then(|k| app.terminals.get(k));
            view.is_some_and(|v| {
                gpui_kit::Focusable::focus_handle(v.read(cx), cx).is_focused(window)
            })
        });
        assert!(focused, "b has the keyboard");
        cx.simulate_keystrokes("cmd-j");
        cx.run_until_parked();
        app.read_with(cx, |app, _| assert!(app.prompt.is_some(), "⌘J answered"));
        let_attaches_end(&app, cx);
    }

    /// The thread on show ending on its own while you type elsewhere, as
    /// in a spotlight, shows another, but leaves the keyboard where it is.
    #[gpui::test]
    fn a_thread_shown_instead_leaves_the_keyboard_where_it_is(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("ended-elsewhere");
        let [a, b] =
            ["a", "b"].map(|l| server.run("/", &[&format!("lemur.label={l}")], &["sleep", "600"]));
        let (app, cx) = rooted(server.config(), cx);
        cx.update(|_, cx| super::bind_keys(cx));
        cx.simulate_resize(size(px(1200.0), px(700.0)));
        open(&app, &a, cx);
        cx.run_until_parked();
        cx.simulate_keystrokes("cmd-p");
        cx.run_until_parked();
        app.read_with(cx, |app, _| assert!(app.prompt.is_some(), "⌘P's spotlight"));

        server.hc(&["kill", &a]);
        let switched = wait_until(Duration::from_secs(5), || {
            cx.executor().advance_clock(super::BURST);
            cx.run_until_parked();
            app.read_with(cx, |app, _| app.active == Some(key(&b)))
        });
        assert!(switched, "b shows instead");
        let b_has_it = cx.update(|window, cx| {
            let view = app.read(cx).terminals[&key(&b)].clone();
            gpui_kit::Focusable::focus_handle(view.read(cx), cx).is_focused(window)
        });
        assert!(!b_has_it, "the spotlight keeps the keyboard");
        cx.simulate_keystrokes("x");
        let typed = app.read_with(cx, |app, cx| {
            app.prompt
                .as_ref()
                .map(|p| p.input.read(cx).value().to_string())
        });
        assert_eq!(typed.as_deref(), Some("x"));
        let_attaches_end(&app, cx);
    }

    /// ⌘W while peeking puts the peek away; the idle shell on show under
    /// it, which ⌘W would close without asking, stays.
    #[gpui::test]
    fn cmd_w_while_peeking_puts_the_peek_away(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("peek-close");
        let [a, b] = ["a", "b"].map(|l| server.run("/", &[&format!("lemur.label={l}")], &["sh"]));
        let (app, cx) = rooted(server.config(), cx);
        cx.update(|_, cx| super::bind_keys(cx));
        cx.simulate_resize(size(px(1200.0), px(700.0)));
        let idle = wait_until(Duration::from_secs(5), || {
            app.update(cx, |app, cx| app.refresh(cx).detach());
            cx.run_until_parked();
            app.read_with(cx, |app, _| {
                [&a, &b]
                    .iter()
                    .all(|s| app.session_of(&key(s)).is_some_and(|s| !s.busy))
            })
        });
        assert!(idle, "both shells at their prompt");
        open(&app, &a, cx);
        cx.update(|window, cx| app.update(cx, |app, cx| app.peek(key(&b), window, cx)));
        cx.run_until_parked();

        cx.simulate_keystrokes("cmd-w");
        cx.run_until_parked();
        app.read_with(cx, |app, _| {
            assert_eq!(app.peek, None, "the peek went");
            assert_eq!(app.active, Some(key(&a)), "a still shows");
        });
        app.update(cx, |app, cx| app.refresh(cx).detach());
        cx.run_until_parked();
        assert!(server.addresses().contains(&a), "a still runs");
        let_attaches_end(&app, cx);
    }

    /// Lemur notes the shell each Claude thread runs in, once, so a thread
    /// whose host restarted is known when it comes back as a fresh one.
    #[gpui::test]
    fn claude_threads_have_their_shell_noted(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("note-shell");
        let t = server.run("/", &["lemur.kind=claude", "lemur.label=c"], &["sh"]);
        let shell = server.run("/", &["lemur.label=s"], &["sh"]);
        let (app, cx) = rooted(server.config(), cx);
        let noted = |address: &str| {
            let t = server.core().terminals();
            let t = t.into_iter().find(|x| x.address == address).unwrap();
            (t.pid, t.labels.get(snapshot::SHELL).cloned())
        };
        let done = wait_until(Duration::from_secs(5), || {
            app.update(cx, |app, cx| app.refresh(cx).detach());
            cx.run_until_parked();
            let (pid, label) = noted(&t);
            pid.is_some() && label == pid.map(|p| p.to_string())
        });
        assert!(done, "{:?}", noted(&t));
        assert_eq!(noted(&shell).1, None, "not a Claude thread");
    }

    /// A shell at its prompt with a job of its own (`&`, or ⌃Z) asks
    /// before ⌘W closes it: the job would end too.
    #[gpui::test]
    fn a_shell_with_a_job_asks_before_closing(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("close-job");
        let b = server.run("/", &["lemur.label=b"], &["sh"]);
        let (app, cx) = rooted(server.config(), cx);
        cx.update(|_, cx| super::bind_keys(cx));
        cx.simulate_resize(size(px(1200.0), px(700.0)));
        open(&app, &b, cx);
        server.hc(&["send", &b, "sleep 600 &"]);
        // Back at its prompt, with the job behind it.
        let idle = wait_until(Duration::from_secs(5), || {
            app.update(cx, |app, cx| app.refresh(cx).detach());
            cx.run_until_parked();
            server.hc(&["read", &b]).contains("[1] ")
                && app.read_with(cx, |app, _| {
                    app.session_of(&key(&b)).is_some_and(|s| !s.busy)
                })
        });
        assert!(idle, "{}", server.hc(&["read", &b]));

        cx.simulate_keystrokes("cmd-w");
        let asked = wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            cx.update(|window, cx| window.has_active_dialog(cx))
        });
        assert!(asked, "⌘W asks");
        assert!(server.addresses().contains(&b), "still running");
        cx.update(|window, cx| window.close_dialog(cx));
        let_attaches_end(&app, cx);
    }

    /// ⌘W closes a shell at its prompt without asking, and the thread shown
    /// instead has the keyboard.
    #[gpui::test]
    fn an_idle_shell_closes_without_asking(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("close-idle");
        let a = server.run("/", &["lemur.label=a"], &["sleep", "600"]);
        let b = server.run("/", &["lemur.label=b"], &["sh"]);
        let (app, cx) = rooted(server.config(), cx);
        cx.update(|_, cx| super::bind_keys(cx));
        cx.simulate_resize(size(px(1200.0), px(700.0)));
        open(&app, &a, cx);
        open(&app, &b, cx);
        // Once herdr-core has read what runs in it.
        let idle = wait_until(Duration::from_secs(5), || {
            app.update(cx, |app, cx| app.refresh(cx).detach());
            cx.run_until_parked();
            app.read_with(cx, |app, _| {
                app.session_of(&key(&b)).is_some_and(|s| !s.busy)
            })
        });
        assert!(idle);

        cx.simulate_keystrokes("cmd-w");
        cx.run_until_parked();
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
        assert!(wait_until(Duration::from_secs(5), || !server
            .addresses()
            .contains(&b)));
        let switched = wait_until(Duration::from_secs(5), || {
            cx.executor().advance_clock(super::BURST);
            cx.run_until_parked();
            app.read_with(cx, |app, _| app.active == Some(key(&a)))
        });
        assert!(switched);
        let focused = cx.update(|window, cx| {
            let app = app.read(cx);
            let view = app.active.as_ref().and_then(|k| app.terminals.get(k));
            view.is_some_and(|v| {
                gpui_kit::Focusable::focus_handle(v.read(cx), cx).is_focused(window)
            })
        });
        assert!(focused, "a has the keyboard");

        // a runs a program of its own: ⌘W asks.
        cx.simulate_keystrokes("cmd-w");
        cx.run_until_parked();
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        let_attaches_end(&app, cx);
    }

    /// A thread's × in the sidebar closes it as ⌘W does: a shell at its
    /// prompt at once, one running a program after asking.
    #[gpui::test]
    fn the_sidebars_close_asks_as_cmd_w_does(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("close-row");
        let a = server.run("/", &["lemur.label=a"], &["sleep", "600"]);
        let b = server.run("/", &["lemur.label=b"], &["sh"]);
        let (app, cx) = rooted(server.config(), cx);
        cx.simulate_resize(size(px(1200.0), px(700.0)));
        // Once herdr-core has read what runs in b.
        let idle = wait_until(Duration::from_secs(5), || {
            app.update(cx, |app, cx| app.refresh(cx).detach());
            cx.run_until_parked();
            app.read_with(cx, |app, _| {
                app.session_of(&key(&b)).is_some_and(|s| !s.busy)
            })
        });
        assert!(idle);
        // A row's × is at its right end.
        let close = |row: &'static str, cx: &mut VisualTestContext| {
            let row = cx.debug_bounds(row).expect(row);
            cx.simulate_click(
                point(row.right() - px(16.0), row.center().y),
                Modifiers::none(),
            );
            cx.run_until_parked();
        };
        let asked =
            |cx: &mut VisualTestContext| cx.update(|window, cx| window.has_active_dialog(cx));

        close("thread-1", cx);
        assert!(!asked(cx), "b's shell is at its prompt");
        assert!(wait_until(Duration::from_secs(5), || !server
            .addresses()
            .contains(&b)));

        close("thread-0", cx);
        assert!(asked(cx), "a runs sleep");
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(server.addresses().contains(&a), "cancelled");
    }

    /// Turned on in Settings, the sidebar's foot shows each limit the
    /// host's Claude command's `/usage` lists; a Claude that can't answer
    /// says so; turned off, it's gone. Off, it never asks.
    #[gpui::test]
    fn the_meter_follows_settings(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("meter");
        let dir = TempDir::new("meter");
        let printed = dir.join("printed.jsonl");
        std::fs::write(&printed, crate::usage::PRINTED).unwrap();
        let ran = dir.join("ran");
        let mut cfg = server.config();
        // Says it ran, and prints what /usage prints; `#` leaves out the
        // arguments Lemur adds.
        cfg.hosts[0].claude = format!("touch {}; cat {} #", ran.display(), printed.display());
        let (app, cx) = rooted(cfg, cx);
        let loaded = wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            app.read_with(cx, |app, _| app.loaded)
        });
        assert!(loaded);
        let found = cx.update(|_, cx| cx.global::<meter::ClaudeFound>().0);
        assert!(!found, "no Claude here yet");
        app.update(cx, |app, cx| app.tick_meter(true, cx));
        cx.run_until_parked();
        assert!(!ran.exists(), "off, it doesn't ask");
        assert!(cx.debug_bounds("usage").is_none());

        let set = |on: bool, cx: &mut VisualTestContext| {
            cx.update(|_, cx| Prefs::update(cx, |p| p.claude_usage = on));
        };
        let shown = |cx: &mut VisualTestContext| {
            wait_until(Duration::from_secs(5), || {
                cx.run_until_parked();
                app.read_with(cx, |app, _| app.meter.shown.is_some())
            });
            app.read_with(cx, |app, _| app.meter.shown.clone())
        };
        set(true, cx);
        let usage = shown(cx).expect("asked at once").expect("its limits");
        assert!(ran.exists());
        assert_eq!(usage.host, "local");
        let labels: Vec<&str> = usage.limits.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(labels, ["Week", "Week · Fable"]);
        assert!(cx.debug_bounds("usage bars").is_some());

        // A click on "Claude" minimizes it to that line, and opens it again.
        let click = |selector: &'static str, cx: &mut VisualTestContext| {
            let at = cx.debug_bounds(selector).expect(selector).center();
            cx.simulate_click(at, Modifiers::none());
            cx.run_until_parked();
        };
        // On the press: it's minimized before the button comes up.
        let header = cx.debug_bounds("usage-header").unwrap().center();
        cx.simulate_mouse_down(header, MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        assert!(cx.debug_bounds("usage bars").is_none());
        cx.simulate_mouse_up(header, MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("usage bars").is_none(),
            "the release does nothing"
        );
        assert!(cx.debug_bounds("usage-header minimized").is_some());
        assert!(app.read_with(cx, |app, _| app.cfg.minimized_usage), "kept");
        click("usage-header minimized", cx);
        assert!(cx.debug_bounds("usage bars").is_some());
        assert!(!app.read_with(cx, |app, _| app.cfg.minimized_usage));

        app.update(cx, |app, cx| {
            app.cfg.hosts[0].claude = "false".into();
            app.meter.shown = None;
            app.ask_usage(cx);
        });
        let failed = shown(cx).expect("asked").unwrap_err();
        assert_eq!(failed, "Claude Code didn't answer (on local)");
        assert!(cx.debug_bounds("usage bars").is_some(), "it says so");
        // A click there asks again.
        app.update(cx, |app, _| {
            app.cfg.hosts[0].claude = format!("cat {} #", printed.display());
        });
        click("usage bars", cx);
        assert!(wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            app.read_with(cx, |app, _| matches!(app.meter.shown, Some(Ok(_))))
        }));

        set(false, cx);
        cx.run_until_parked();
        assert!(cx.debug_bounds("usage").is_none());
        assert!(app.read_with(cx, |app, _| app.meter.shown.is_none()));
    }

    /// Every host is listed in Other, threads or none, and its + starts a
    /// terminal in its home there.
    #[gpui::test]
    fn every_host_starts_threads_from_other(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("host-new");
        let (app, cx) = rooted(server.config(), cx);
        cx.simulate_resize(size(px(1200.0), px(700.0)));
        let listed = wait_until(Duration::from_secs(5), || {
            app.update(cx, |app, cx| app.refresh(cx).detach());
            cx.run_until_parked();
            cx.debug_bounds("host local").is_some()
        });
        assert!(listed, "no threads, still listed");
        let home = std::env::var("HOME").unwrap();
        app.read_with(cx, |app, _| assert_eq!(app.snap.hosts[0].home, home));

        let buttons = cx.debug_bounds("buttons local").expect("its buttons");
        let add = point(buttons.right() - px(4.0), buttons.center().y);
        cx.simulate_mouse_move(add, None, Modifiers::none());
        cx.simulate_click(add, Modifiers::none());
        cx.run_until_parked();
        cx.simulate_keystrokes("down enter");
        let mut started = None;
        assert!(wait_until(Duration::from_secs(5), || {
            started = server.core().terminals().into_iter().next();
            started.is_some()
        }));
        let started = started.unwrap();
        assert_eq!(started.cwd, home);
        assert_eq!(
            started.labels.get("lemur.kind").map(String::as_str),
            Some("shell")
        );
        let shown = wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            app.read_with(cx, |app, _| app.active == Some(key(&started.address)))
        });
        assert!(shown, "the new terminal shows");
        let_attaches_end(&app, cx);
    }

    /// The threads outside every project are in one Other section, each
    /// host's under a row naming it.
    #[gpui::test]
    fn other_threads_are_one_section_by_host(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("other");
        // Not `box`, which another test's stand-in host is called.
        let mut cfg = server.far_config("shed");
        let mut here = Host::new("local", "local");
        here.hc_session = Some(server.session.clone());
        cfg.hosts.insert(0, here);
        server.run("/", &["lemur.label=here"], &["sleep", "600"]);
        let far = [
            "run",
            "--bg",
            "--on",
            "shed",
            "--label",
            "lemur.label=there",
        ];
        server.hc(&[&far[..], &["--", "sleep", "600"]].concat());
        let (app, cx) = rooted(cfg, cx);
        cx.simulate_resize(size(px(1200.0), px(700.0)));
        let listed = wait_until(Duration::from_secs(10), || {
            app.update(cx, |app, cx| app.refresh(cx).detach());
            cx.run_until_parked();
            app.read_with(cx, |app, _| {
                app.snap.hosts.iter().all(|h| h.other.len() == 1)
            })
        });
        assert!(
            listed,
            "{:?}",
            app.read_with(cx, |app, _| app.snap.hosts.clone())
        );
        let top = |selector: &'static str, cx: &mut VisualTestContext| {
            cx.debug_bounds(selector).expect(selector).top()
        };
        let other = top("project Other", cx);
        assert!(other < top("host local", cx));
        assert!(top("host local", cx) < top("host shed", cx));
        assert!(
            cx.debug_bounds("project other").is_none(),
            "no section per host"
        );
    }

    /// The status bar says where the thread on show runs: its host, its
    /// worktree's branch, and the servers running there, as links.
    #[gpui::test]
    fn the_status_bar_links_the_worktrees_servers(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("status");
        let mut cfg = server.config();
        cfg.bases.push(Base::new("local", "/srv/api"));
        let (app, cx) = rooted(cfg, cx);
        cx.simulate_resize(size(px(1200.0), px(700.0)));
        let serve = |port, cwd: &str| crate::machine::Server {
            port,
            addr: "127.0.0.1".into(),
            name: "ruby".into(),
            cwd: cwd.into(),
        };
        let machine = crate::machine::Reading {
            servers: vec![
                serve(3020, "/srv/api/.claude/worktrees/auth"),
                serve(3000, "/srv/api"),
            ],
            ..Default::default()
        };
        let snap = project_snapshot(&["t"], machine);
        cx.update(|window, cx| app.update(cx, |app, cx| app.set_snapshot(snap, window, cx)));
        open(&app, "t", cx);
        cx.run_until_parked();
        assert!(cx.debug_bounds("port 3020").is_some());
        assert!(
            cx.debug_bounds("port 3000").is_none(),
            "the main checkout's"
        );
        let link = cx.debug_bounds("port 3020").unwrap().center();
        cx.simulate_click(link, Modifiers::none());
        assert_eq!(cx.opened_url().as_deref(), Some("http://localhost:3020"));
        let_attaches_end(&app, cx);
    }

    /// The sidebar's foot shows each host's CPU, from two readings, and its
    /// memory.
    #[gpui::test]
    fn hosts_show_their_load(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("load");
        let mut cfg = server.config();
        cfg.bases.push(Base::new("local", "/srv/api"));
        let (app, cx) = rooted(cfg, cx);
        cx.simulate_resize(size(px(1200.0), px(700.0)));
        let read = |busy, total, cx: &mut VisualTestContext| {
            let machine = crate::machine::Reading {
                load: Some(crate::machine::Load {
                    busy,
                    total,
                    mem_total_kb: 4_194_304,
                    mem_used_kb: 1_048_576,
                    load: vec!["0.50".into(), "0.40".into(), "0.30".into()],
                    ..Default::default()
                }),
                ..Default::default()
            };
            let snap = project_snapshot(&[], machine);
            cx.update(|window, cx| app.update(cx, |app, cx| app.set_snapshot(snap, window, cx)));
            cx.run_until_parked();
        };
        read(100, 1000, cx);
        assert!(cx.debug_bounds("load local").is_some());
        // A click on its name minimizes it to that line, and opens it again.
        let click = |selector: &'static str, cx: &mut VisualTestContext| {
            let at = cx.debug_bounds(selector).expect(selector).center();
            cx.simulate_click(at, Modifiers::none());
            cx.run_until_parked();
        };
        let minimized = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, _| app.cfg.minimized_hosts.clone())
        };
        let tall = |cx: &mut VisualTestContext| cx.debug_bounds("load local").unwrap().size.height;
        let open = tall(cx);
        click("load-header-local", cx);
        assert_eq!(minimized(cx), ["local"]);
        assert!(tall(cx) < open, "its bars are gone");
        click("load-header-local minimized", cx);
        assert!(minimized(cx).is_empty());
        assert_eq!(tall(cx), open);
        let cpu = |cx: &mut VisualTestContext| app.read_with(cx, |app, _| app.loads["local"].1);
        assert_eq!(cpu(cx), None, "one reading");
        read(200, 1400, cx);
        assert_eq!(cpu(cx), Some(25.0));
        // The same reading again, as terminals change between refreshes.
        read(200, 1400, cx);
        assert_eq!(cpu(cx), Some(25.0));
    }

    /// ⌘J lists the threads to go to under the new one's name: typing
    /// narrows them, ↓ goes past Claude and Shell to them, and ⏎ opens the
    /// one picked. ⌘P lists them all.
    #[gpui::test]
    fn cmd_j_and_cmd_p_go_to_threads(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("goto");
        let mut cfg = server.config();
        cfg.bases.push(Base::new("local", "/srv/api"));
        let (app, cx) = rooted(cfg, cx);
        cx.update(|_, cx| super::bind_keys(cx));
        cx.simulate_resize(size(px(1200.0), px(700.0)));
        let snap = project_snapshot(&["review", "build"], Default::default());
        cx.update(|window, cx| app.update(cx, |app, cx| app.set_snapshot(snap, window, cx)));
        open(&app, "review", cx);
        cx.run_until_parked();
        let listed = |cx: &mut VisualTestContext| {
            app.read_with(cx, |app, cx| {
                let prompt = app.prompt.as_ref().expect("asking");
                let found = app.matches(prompt, cx);
                found.into_iter().map(|m| m.key.1).collect::<Vec<_>>()
            })
        };
        let shown = |cx: &mut VisualTestContext| app.read_with(cx, |app, _| app.active.clone());

        cx.simulate_keystrokes("cmd-j");
        cx.run_until_parked();
        // The worktree's own first, untouched by the name it starts with.
        assert_eq!(listed(cx), ["review", "build", "main-shell"]);
        cx.simulate_input("bui");
        assert_eq!(listed(cx), ["build"]);
        cx.simulate_keystrokes("down down");
        app.read_with(cx, |app, _| {
            assert_eq!(app.prompt.as_ref().unwrap().go, Some(0))
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(shown(cx), Some(key("build")));
        app.read_with(cx, |app, _| assert!(app.prompt.is_none()));

        cx.simulate_keystrokes("cmd-p");
        cx.run_until_parked();
        // Those opened last first, the one on show at the end.
        assert_eq!(listed(cx), ["review", "main-shell", "build"]);
        cx.simulate_input("main");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(shown(cx), Some(key("main-shell")));
        let_attaches_end(&app, cx);
    }

    /// What the stand-in for lemur-dictate hears: two words while it
    /// listens, and two more once stopped.
    const HEARS: &str = r#"echo '{"listening": true}'
echo '{"finalized": "", "volatile": "Hello"}'
echo '{"finalized": "Hello world.", "volatile": ""}'
cat >/dev/null
echo '{"finalized": "Hello world. Bye.", "volatile": ""}'
echo '{"done": true}'
"#;

    /// Dictation with `HEARS` for lemur-dictate, in threads `a` and `b`
    /// that echo what's pasted into them.
    fn dictating<'a>(
        server: &HcServer,
        dir: &TempDir,
        cx: &'a mut gpui::TestAppContext,
    ) -> ([String; 2], Entity<Lemur>, &'a mut VisualTestContext) {
        let threads = ["a", "b"].map(|_| server.run("/", &[], &["cat"]));
        cx.update(super::bind_keys);
        let (app, cx) = rooted(server.config(), cx);
        let helper = fake_dictate(dir, HEARS);
        app.update(cx, |app, _| app.helper = helper);
        (threads, app, cx)
    }

    /// Opens a thread, and waits for its terminal to attach.
    fn attached(app: &Entity<Lemur>, session: &str, cx: &mut VisualTestContext) {
        open(app, session, cx);
        let attached = wait_until(Duration::from_secs(10), || {
            cx.run_until_parked();
            app.read_with(cx, |app, cx| {
                app.terminals[&key(session)].read(cx).is_attached()
            })
        });
        assert!(attached, "{session} never attached");
    }

    fn dictated(app: &Entity<Lemur>, cx: &mut VisualTestContext) -> Option<(String, bool)> {
        app.read_with(cx, |app, cx| {
            let (bar, _) = app.dictation.as_ref()?;
            let bar = bar.read(cx);
            Some((bar.text(cx), bar.is_listening()))
        })
    }

    fn shows(server: &HcServer, session: &str, text: &str) -> bool {
        wait_until(Duration::from_secs(5), || {
            server.hc(&["read", session]).contains(text)
        })
    }

    /// ⌘D listens and shows what's heard; ⌘⏎ from another thread stops,
    /// waits for the rest, and pastes it all there.
    #[gpui::test]
    fn dictation_pastes_into_the_thread_on_show(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("dictate");
        let dir = TempDir::new("dictate");
        let ([a, b], app, cx) = dictating(&server, &dir, cx);
        attached(&app, &a, cx);
        cx.simulate_keystrokes("cmd-d");
        let heard = wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            dictated(&app, cx) == Some(("Hello world.".into(), true))
        });
        assert!(heard, "{:?}", dictated(&app, cx));

        // Pasted the moment it's opened: it may still be attaching.
        open(&app, &b, cx);
        cx.simulate_keystrokes("cmd-enter");
        assert!(wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            dictated(&app, cx).is_none()
        }));
        assert!(shows(&server, &b, "Hello world. Bye."));
        assert!(!server.hc(&["read", &a]).contains("Hello"));
    }

    /// Stopped, the text can be changed before ⌘⏎ pastes it, and ⌘D adds
    /// to it; Esc throws it away.
    #[gpui::test]
    fn dictation_can_be_corrected_or_thrown_away(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("dictate-edit");
        let dir = TempDir::new("dictate-edit");
        let ([a, _], app, cx) = dictating(&server, &dir, cx);
        attached(&app, &a, cx);
        let wait_for = |text: &str, listening: bool, cx: &mut VisualTestContext| {
            let done = wait_until(Duration::from_secs(5), || {
                cx.run_until_parked();
                dictated(&app, cx) == Some((text.into(), listening))
            });
            assert!(done, "{:?}", dictated(&app, cx));
        };

        cx.simulate_keystrokes("cmd-d");
        wait_for("Hello world.", true, cx);
        // Words typed while it listens would be overwritten, so they aren't taken.
        cx.simulate_input("x");
        cx.simulate_keystrokes("cmd-d");
        wait_for("Hello world. Bye.", false, cx);
        cx.simulate_input(" Ciao.");
        cx.simulate_keystrokes("cmd-d");
        wait_for("Hello world. Bye. Ciao. Hello world.", true, cx);
        cx.simulate_keystrokes("cmd-d");
        wait_for("Hello world. Bye. Ciao. Hello world. Bye.", false, cx);
        cx.simulate_keystrokes("cmd-enter");
        assert!(shows(
            &server,
            &a,
            "Hello world. Bye. Ciao. Hello world. Bye."
        ));
        assert_eq!(dictated(&app, cx), None);

        cx.simulate_keystrokes("cmd-d");
        wait_for("Hello world.", true, cx);
        cx.simulate_keystrokes("escape");
        assert_eq!(dictated(&app, cx), None);
    }
}
