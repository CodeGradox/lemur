//! The window: a sidebar of sessions next to a live terminal, drawn with Rune.
//!
//! Every session you open keeps its own terminal, attached through herdr-core and parsed
//! by Ghostty's terminal core, so switching is instant and scrollback survives. The app's
//! state is one `Lemur`; Rune runs `view` to draw it and the handlers to change it, and
//! work off the main thread comes back through `bg`.

mod actions;
mod agents;
mod away;
mod bg;
mod card;
mod changes;
mod changes_panel;
mod chrome;
mod commands;
mod core;
mod cycle;
mod dictation;
mod gkeys;
mod grid;
mod icons;
pub(crate) mod keys;
mod linear_panel;
mod links_panel;
mod meter;
mod notes;
mod notify;
mod palette;
mod panels;
mod peek;
mod picker;
mod projects;
mod prompt;
mod resume;
mod settings;
mod sidebar;
mod silence;
mod snackbar;
mod term_input;
mod term_links;
mod terminal;
#[cfg(test)]
mod tests;

use crate::attach::Active;
use crate::config::{Config, Host, Language, Terminal, ThemeChoice, ThreadOrder};
use crate::snapshot::{Activity, HostData, Session, Snapshot, Worktree};
use bg::Bg;
use rune::{AppSpec, Chrome, Cx, El, KeyEvent, Tx, Waker, el};
use rune_kit::Kit;
use rune_kit::shell::{PaneState, TabDragState};
use snackbar::Kind;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use terminal::{Metrics, TermView};

pub use actions::Action;

pub type Key = (String, String);
/// A list of threads: a host's worktree, by path, or with `None` the host's sessions
/// outside every base.
pub type List = (String, Option<String>);

const REFRESH: Duration = Duration::from_secs(3);
/// How long changes herdr-core tells of are gathered into one snapshot.
const BURST: Duration = Duration::from_millis(30);
/// What closing a shell with jobs ends.
const JOBS: &str = "The terminal ends, and the jobs its shell started (sent off with &, or stopped with ⌃Z).";

/// What shows on the right of the terminal, one at a time.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Right {
    Notes,
    Linear,
    Changes,
    Links,
    Agents,
}

/// The sidebar's width until its edge is dragged, and the right panel's.
const SIDEBAR_WIDTH: f32 = 255.0;
const NOTES_WIDTH: f32 = 360.0;

/// Lemur's settings, which Settings changes and the config file keeps.
#[derive(Clone)]
pub struct Prefs {
    pub theme: ThemeChoice,
    pub dictation_language: Language,
    pub thread_order: ThreadOrder,
    pub terminal: Terminal,
    pub claude_usage: bool,
    pub codex_usage: bool,
    /// The config file that keeps changes; tests have none.
    file: Option<PathBuf>,
}

impl Prefs {
    pub fn new(cfg: &Config, file: Option<PathBuf>) -> Prefs {
        Prefs {
            theme: cfg.theme,
            dictation_language: cfg.dictation_language,
            thread_order: cfg.thread_order,
            terminal: cfg.terminal.clone(),
            claude_usage: cfg.claude_usage,
            codex_usage: cfg.codex_usage,
            file,
        }
    }

    /// The config file changes are saved to; tests have none.
    pub fn config_file(&self) -> Option<&Path> {
        self.file.as_deref()
    }
}

/// A question over the window, as an alert: what it does, and what OK runs.
pub struct Dialog {
    pub title: String,
    pub description: String,
    pub ok: &'static str,
    /// A box to tick under the description, and whether it is.
    pub check: Option<(String, bool)>,
    pub on_ok: Box<dyn FnOnce(&mut Lemur, bool)>,
}

pub struct Lemur {
    pub cfg: Config,
    pub prefs: Prefs,
    /// Where to remember the session you last opened; tests have none.
    state_file: Option<PathBuf>,
    pub snap: Snapshot,
    /// What the last refresh found on each host, which terminals are filed under as they
    /// change.
    data: Vec<HostData>,
    pub loaded: bool,
    pub terminals: HashMap<Key, TermView>,
    pub active: Option<Key>,
    /// Threads by when they were last opened, the latest first.
    recent: Vec<Key>,
    /// ⌃Tab going through a worktree's threads: in which order, and where it's at. It
    /// ends when ⌃ comes up.
    cycling: Option<(Vec<Key>, usize)>,
    /// The sidebar and the right panel: their widths, and whether they show.
    pub sidebar: PaneState,
    pub inspector: PaneState,
    pub prompt: Option<prompt::Prompt>,
    /// Threads just started and opened, kept open until a snapshot lists them, by how many
    /// more snapshots may leave them out.
    fresh: HashMap<Key, u8>,
    /// Sessions that finished while you were looking at another one.
    pub done: HashSet<Key>,
    /// What each thread was doing at the last snapshot, or before its host couldn't be
    /// reached.
    seen: HashMap<Key, Activity>,
    /// The agents in the order Next and Previous Agent go through them.
    agent_walk: Vec<Key>,
    /// Lists whose dragged order is being written to herdr-core, and the order dragged
    /// meanwhile, written next.
    ordering: HashMap<List, Option<Vec<String>>>,
    /// Terminals setting up a host (`hc machine add`), and the host each sets up.
    setups: HashMap<Key, Host>,
    /// Gone threads being resumed, by host and session.
    resuming: HashSet<Key>,
    /// Hosts a set-up is starting for.
    setting_up: HashSet<String>,
    /// Shown in a snackbar over the terminal.
    pub status: Option<(String, Kind)>,
    /// The session from last time, opened once the first refresh is in.
    restore: Option<Key>,
    /// Each host's last load, and the CPU share between it and the one before.
    loads: HashMap<String, (crate::machine::Load, Option<f32>)>,
    /// Counts changes a snapshot fetched across one would undo (the thread order, a removed
    /// project); such a snapshot is dropped.
    changes: u64,
    /// The panel on the right, shown last; `right()` says whether it's out.
    right_last: Right,
    /// The lists whose "Put away" is open, by their key.
    pub away_open: HashSet<List>,
    /// A thread peeked at, over the one on show.
    pub peek: Option<Key>,
    /// A question over the window.
    pub dialog: Option<Dialog>,
    pub panels: panels::Panels,
    pub bg: Bg,
    /// The app is dark: the theme setting, or macOS's appearance.
    pub dark: bool,
    pub metrics: Metrics,
    /// The terminal's area in the window, which its grid fits.
    term_area: (f32, f32),
    /// The tab bar's drag, and the sidebar's.
    pub tab_drag: TabDragState,
    pub row_drag: rune_kit::shell::Reorder,
    /// The list `row_drag` is for: the one under the pointer as a drag starts.
    pub row_drag_list: List,
    /// A worktree or project being dragged by its row or header, by key, and how far; and
    /// where each of those is laid out (`y`, height), to drop it by.
    pub moving: Option<(String, f32)>,
    pub places: HashMap<String, (f32, f32)>,
    /// When the sidebar's width was last saved.
    saved_widths: (f32, f32),
    /// Each worktree's lines added and removed, for the status bar.
    diffs: changes::Changes,
    /// Refresh ticks so far, for what's asked less often.
    ticks: u64,
    /// The window is in front. Out of sight, where a refresh still costs each host a
    /// `claude agents`, the refresh ticks and the meters ask less often.
    pub looking: bool,
    /// herdr-core tells of changes while this is set.
    watching: bool,
}

/// The cores the config's hosts' terminals run in, each once, and whether every host was
/// reached.
fn watched(cfg: &Config) -> (Vec<std::sync::Arc<crate::hc::Core>>, bool) {
    let mut cores: Vec<std::sync::Arc<crate::hc::Core>> = vec![];
    let mut all = true;
    for host in &cfg.hosts {
        // Not dialing the host: its core tells of it once it's reached, and one being
        // dialled would hold up the others' changes.
        match crate::hc::core_of(host) {
            Ok(core) => {
                if !cores.iter().any(|c| std::sync::Arc::ptr_eq(c, &core)) {
                    cores.push(core);
                }
            }
            Err(_) => all = false,
        }
    }
    (cores, all)
}

pub fn sessions(snap: &Snapshot) -> impl Iterator<Item = (&str, &Session)> {
    snap.hosts.iter().flat_map(|h| {
        let in_bases = h.bases.iter().flat_map(|b| b.worktrees.iter()).flat_map(|w| w.sessions.iter());
        in_bases.chain(h.other.iter()).map(move |s| (h.name.as_str(), s))
    })
}

/// A worktree's name in the sidebar: its branch, which tells worktrees apart even when
/// their folders share a name. `worktree-` is dropped from the branches lemur and
/// `claude -w` made, leaving the name you gave. A detached worktree shows its folder's
/// name, the one you gave lemur.
pub fn worktree_label(w: &Worktree) -> String {
    match (&w.branch, &w.head) {
        (Some(branch), _) => branch.strip_prefix("worktree-").unwrap_or(branch).to_string(),
        (None, Some(_)) => format!("{} (detached)", crate::config::leaf(&w.path)),
        (None, None) => crate::config::leaf(&w.path).to_string(),
    }
}

pub fn key(host: &str, s: &Session) -> Key {
    (host.to_string(), s.name.clone())
}

/// Moves `items[from]` to `to`. False if that's no move, or either is out of range.
pub fn move_item<T>(items: &mut Vec<T>, from: usize, to: usize) -> bool {
    if from == to || from >= items.len() || to >= items.len() {
        return false;
    }
    let moved = items.remove(from);
    items.insert(to, moved);
    true
}

/// Where the item that was at `index` is once `move_item` has moved the one at `from` to
/// `to`.
pub fn moved_to(index: usize, from: usize, to: usize) -> usize {
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

impl Lemur {
    pub fn new(cfg: Config, state_file: Option<PathBuf>, config_file: Option<PathBuf>) -> Lemur {
        // Notes and links live beside the state file, in ~/.lemur; tests have neither.
        let data_dir = state_file.as_deref().and_then(Path::parent).map(Path::to_path_buf);
        let restore = state_file.as_deref().and_then(|p| Active::load_from(p).ok()).map(|a| (a.host, a.session));
        let sidebar_width = cfg.sidebar_width.unwrap_or(SIDEBAR_WIDTH);
        let notes_width = cfg.notes_width.unwrap_or(NOTES_WIDTH);
        let prefs = Prefs::new(&cfg, config_file);
        let metrics = Metrics::new(&prefs.terminal.font_family, prefs.terminal.font_size);
        Lemur {
            panels: panels::Panels::new(data_dir.as_deref()),
            cfg,
            prefs,
            state_file,
            snap: Snapshot::default(),
            data: vec![],
            loaded: false,
            terminals: HashMap::new(),
            active: None,
            recent: vec![],
            cycling: None,
            sidebar: PaneState::new(sidebar_width).range(180.0, 640.0),
            inspector: PaneState::new(notes_width).range(240.0, 900.0).collapsed(),
            prompt: None,
            fresh: HashMap::new(),
            done: HashSet::new(),
            seen: HashMap::new(),
            agent_walk: vec![],
            ordering: HashMap::new(),
            setups: HashMap::new(),
            resuming: HashSet::new(),
            setting_up: HashSet::new(),
            status: None,
            restore,
            loads: HashMap::new(),
            changes: 0,
            right_last: Right::Notes,
            away_open: HashSet::new(),
            peek: None,
            dialog: None,
            bg: Bg::default(),
            dark: true,
            metrics,
            term_area: (900.0, 600.0),
            tab_drag: TabDragState::new(),
            row_drag: rune_kit::shell::Reorder::new().gap(1.0),
            row_drag_list: (String::new(), None),
            moving: None,
            places: HashMap::new(),
            saved_widths: (sidebar_width, notes_width),
            diffs: changes::Changes::default(),
            ticks: 0,
            looking: true,
            watching: false,
        }
    }

    /// Runs what comes back from other threads until the first snapshot is in and the
    /// thread on show has painted, or `wait` passes: for headless runs and tests, whose
    /// frames don't wait for herdr-core.
    pub fn settle(&mut self, wait: Duration) {
        let start = Instant::now();
        while start.elapsed() < wait {
            for job in self.bg.take() {
                job(self);
            }
            self.tick_terminals();
            let painted = self.active.as_ref().and_then(|k| self.terminals.get(k)).is_none_or(|v| v.painted);
            if self.loaded && painted {
                if let Some(v) = self.active.clone().and_then(|k| self.terminals.get_mut(&k))
                    && let Some(g) = v.grid.as_mut()
                {
                    let _ = g.refresh();
                }
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Shows what went wrong, until it's closed.
    pub fn fail(&mut self, e: anyhow::Error) {
        self.status = Some((format!("{e:#}"), Kind::Error));
    }

    /// Shows `text` for a while.
    pub fn note(&mut self, text: String) {
        let shown = Some((text, Kind::Note));
        self.status = shown.clone();
        self.bg.after(snackbar::NOTE_TIME, move |s| {
            if s.status == shown {
                s.status = None;
            }
        });
    }

    /// Says `text` while something runs; returns what it said, for `done` to take away.
    pub fn busy(&mut self, text: String) -> Option<(String, Kind)> {
        self.status = Some((text, Kind::Progress));
        self.status.clone()
    }

    /// What `said` was for is over: it goes, unless something else was said since, so one
    /// job ending doesn't take away another's.
    pub fn done(&mut self, said: &Option<(String, Kind)>) {
        if self.status == *said {
            self.status = None;
        }
    }

    /// Runs `work` off the main thread, then `then` with its result.
    pub fn in_background<R: Send + 'static>(
        &self,
        work: impl FnOnce() -> R + Send + 'static,
        then: impl FnOnce(&mut Lemur, R) + Send + 'static,
    ) {
        self.bg.run(work, then);
    }

    /// The app went light or dark.
    fn set_dark(&mut self, dark: bool) {
        if self.dark != dark {
            self.dark = dark;
            self.recolor_terminals();
        }
    }

    /// The pane the terminal shows in was laid out at `w` by `h`.
    fn set_term_area(&mut self, w: f32, h: f32) {
        if (w, h) != self.term_area {
            self.term_area = (w, h);
            self.fit_terminals();
        }
    }

    /// The terminal font measured `width` for ten cells.
    fn set_cell_width(&mut self, width: f32) {
        let cell_w = width / 10.0;
        if (cell_w - self.metrics.cell_w).abs() > 0.001 {
            self.metrics.cell_w = cell_w;
            self.fit_terminals();
        }
    }

    /// Keeps the side panels' widths in the config file once a drag has changed them.
    fn save_panel_widths(&mut self) {
        if self.sidebar.is_resizing() || self.inspector.is_resizing() {
            return;
        }
        let now = (self.sidebar.width(), self.inspector.width());
        if now == self.saved_widths {
            return;
        }
        self.saved_widths = now;
        let kept = |width: f32, default: f32| (width != default).then_some(width);
        let widths = (kept(now.0, SIDEBAR_WIDTH), kept(now.1, NOTES_WIDTH));
        (self.cfg.sidebar_width, self.cfg.notes_width) = widths;
        if let Some(file) = self.prefs.config_file()
            && let Err(e) = Config::save_panel_widths(file, widths.0, widths.1)
        {
            self.fail(e);
        }
    }
}

/// The view's kit, light or dark as the theme setting says.
fn kit(s: &Lemur, cx: &Cx<Lemur>) -> (Kit, bool) {
    let dark = match s.prefs.theme {
        ThemeChoice::System => cx.system_dark(),
        ThemeChoice::Light => false,
        ThemeChoice::Dark => true,
    };
    (Kit::neutral(dark), dark)
}

fn view(s: &Lemur, cx: &mut Cx<Lemur>) -> El {
    let (k, dark) = kit(s, cx);
    let mut layer = Vec::new();
    let body = chrome::window(s, cx, &k, &mut layer);
    // The app's appearance and the terminal font's width, as laid out: each reports once a
    // new one mounts, so a change reaches the state.
    let probe = el()
        .key(if dark { "appearance-dark" } else { "appearance-light" })
        .size(1.0, 1.0)
        .absolute(0.0, 0.0)
        .opacity(0.0)
        .a11y_hidden()
        .on_layout(move |s: &mut Lemur, _| s.set_dark(dark));
    let m = s.metrics;
    let font = rune::text("0123456789")
        .key(format!("cell-probe-{}-{}", m.family, m.size))
        .font(m.style(false))
        .absolute(0.0, 0.0)
        .opacity(0.0)
        .a11y_hidden()
        .on_layout(|s: &mut Lemur, [_, _, w, _]| s.set_cell_width(w));
    rune_kit::root(body.child(probe).child(font), layer)
        .on_window_focus(|s: &mut Lemur, focused, _| {
            s.looking = focused;
            // And for the notifications.
            s.window_focused(focused);
        })
        // A button or a row clicked keeps Rune's focus, which would have Space press it and
        // Tab move on: unless a field has it, the keyboard stays the terminal's.
        .on_key(|s: &mut Lemur, ev: &KeyEvent, tx: &mut Tx| on_key(s, ev, tx))
}

/// Keys the window gets: the app's shortcuts, then the terminal on show's.
fn on_key(s: &mut Lemur, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if let Some(action) = Action::for_key(ev, tx) {
        actions::run(s, action, tx);
        return true;
    }
    if matches!(ev.key, rune::Key::Escape) && s.close_overlay() {
        return true;
    }
    // ⌃ came up: ⌃Tab settles on the thread on show.
    if let rune::Key::Modifier { modifier: rune::Modifier::ControlLeft | rune::Modifier::ControlRight, pressed: false } = ev.key {
        s.end_cycle();
        return false;
    }
    // A field has it, or something over the window: it's not the terminal's.
    if !s.keyboard_on_terminal() {
        return false;
    }
    if ev.is_command('c') && !ev.mods.shift && s.copy_selection(tx) {
        return true;
    }
    if ev.is_command('v') && !ev.mods.shift {
        s.paste_clipboard();
        return true;
    }
    let front = s.peek.clone().or_else(|| s.active.clone());
    let Some(view) = front.and_then(|k| s.terminals.get_mut(&k)) else { return false };
    let base = tx.key_base();
    match view.grid.as_mut().and_then(|g| g.key(ev, base)) {
        Some(bytes) => {
            view.send(bytes);
            true
        }
        None => false,
    }
}

/// Before each frame: work that came back, terminals' output, and what's due.
fn poll(s: &mut Lemur, _: f64, tx: &mut Tx) -> bool {
    let jobs = s.bg.take();
    let mut changed = !jobs.is_empty();
    for job in jobs {
        job(s);
    }
    let (drawn, again) = s.tick_terminals();
    changed |= drawn;
    if let Some(again) = again {
        tx.wake_after(again);
    }
    s.save_panel_widths();
    changed |= s.panels_poll(tx);
    changed
}

/// The window's set up: background work can wake it now. The first refresh, the watch on
/// herdr-core and the ticks start here.
fn setup(s: &mut Lemur, wake: Waker) {
    s.bg.set_waker(wake);
    s.start();
    // Headless, frames don't wait for herdr-core: what it has to show comes in first.
    if rune::tooling::headless_run().is_some() {
        s.settle(Duration::from_secs(8));
    }
}

/// A held key repeats in the terminal, as it did before: macOS's press-and-hold menu of
/// accented letters would take its place while the terminal takes text from the input
/// method. Registered for this run only, under what the user set themselves.
fn held_keys_repeat() {
    use objc2_foundation::{NSDictionary, NSNumber, NSString, NSUserDefaults};
    let key = NSString::from_str("ApplePressAndHoldEnabled");
    let off = NSNumber::new_bool(false);
    let defaults = NSDictionary::from_slices(&[&*key], &[&*off]);
    // SAFETY: an NSNumber is a property-list object, which is what registerDefaults takes.
    unsafe { NSUserDefaults::standardUserDefaults().registerDefaults(&objc2::rc::Retained::cast_unchecked(defaults)) };
}

pub fn run(cfg: Config) -> anyhow::Result<()> {
    held_keys_repeat();
    let spec = AppSpec {
        on_key: Some(on_key),
        poll: Some(poll),
        setup: Some(setup),
        min_size: Some((720.0, 480.0)),
        ..AppSpec::new("Lemur", (1280.0, 820.0), init, view)
    };
    // No title bar, as in Herdr: the window's buttons sit in the sidebar's top row.
    let chrome = Chrome { full_size_content: true, traffic_lights: Some((16.0, 17.0)), drag_regions: true };
    CONFIG.set(cfg).ok();
    let app = rune::App::new(spec).chrome(chrome).standard_menus().modifier_keys().on_ime(term_input::on_ime);
    let mut app = actions::menus(app.on_modifiers(notes::modifiers));
    notify::add_overlay(&mut app);
    app.run();
    Ok(())
}

/// The config `run` was given, for `init`, which Rune calls without arguments.
static CONFIG: std::sync::OnceLock<Config> = std::sync::OnceLock::new();

fn init() -> Lemur {
    let cfg = CONFIG.get().cloned().expect("run sets the config");
    Lemur::new(cfg, Active::path().ok(), Config::path().ok())
}
