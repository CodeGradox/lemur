//! The spotlight: names asked for a new worktree, a new thread or a rename, and threads to
//! go to (⌘P); and ⌘K's sessions, to switch to (`spaces`). What's typed narrows
//! them down; ↑↓ picks one and ⏎ opens it. Also what starts at once, without a name: a
//! terminal (⌘T) or an agent, and the menu offering them.

use super::card;
use super::icons;
use super::snackbar::Kind;
use super::spaces::{Space, SpaceView};
use super::{Key, Lemur, sessions, worktree_label};
use crate::actions;
use crate::config::{Agent, ClaudeThread, leaf};
use crate::snapshot::HOME;
use rune::{El, Key as K, PopupMenu, TextEdit, Tx, col, icon, row, text};
use rune_kit::{Kit, TextInput};

/// How many threads the spotlight lists at most.
const SHOWN: usize = 8;

/// What a new thread can run, as they're listed: an agent, or a shell.
pub(super) const KINDS: [Option<Agent>; 2] = [Some(Agent::Claude), None];

/// What a new thread is called until you name it.
pub(super) fn default_label(agent: Option<Agent>) -> &'static str {
    match agent {
        Some(Agent::Claude) => "claude",
        None => "shell",
    }
}

/// What the name field is asking for.
#[derive(Clone, Debug, PartialEq)]
pub enum Ask {
    /// With `agent` running in it, or with none just a shell.
    NewWorktree { base: usize, agent: Option<Agent> },
    /// `agent`, or with none just a shell, in `dir` on `host`, the folder of session `space`
    /// there (of project `base`, or the host's home); in `folder` of its host's, by its id,
    /// to show beside the thread it was started from.
    NewThread { host: String, base: Option<usize>, dir: String, agent: Option<Agent>, folder: Option<String>, space: String },
    Rename { host: String, session: String },
    /// Nothing new: a thread to go to.
    Find,
    /// A session to show, as Rex's Change Session asks, or a new one.
    Switch,
    /// Folder `id` of `host`'s.
    RenameFolder { host: String, id: String },
}

/// A name being asked for, in the spotlight.
pub struct Prompt {
    pub what: Ask,
    pub input: TextEdit,
    /// For a new worktree: its branch, empty for none, and the branch it starts from,
    /// picked from the host's branches once they've loaded.
    pub branch: Option<TextEdit>,
    pub branches: Vec<String>,
    pub from: Option<String>,
    /// Why the name typed can't be used, and that name.
    pub error: Option<(String, String)>,
    /// The thread picked to go to, among those listed.
    pub go: Option<usize>,
}

/// A thread to go to, as the spotlight lists it.
pub struct Match {
    pub key: Key,
    pub label: String,
    /// `project › worktree`, or its host's name outside every project.
    pub place: String,
}

enum Done {
    /// With a note about how it went, if there's one.
    Started(actions::Started, Option<String>),
    Renamed { host: String, session: String, name: String },
}

/// A field showing `text`, all of it selected so typing replaces it, with the keyboard.
pub(super) fn field(text: &str) -> TextEdit {
    let mut e = TextEdit::new(text);
    e.anchor = 0;
    e.focused = true;
    e
}

impl Lemur {
    /// Opens the name field, selected so typing replaces it. It asks for a label, which
    /// several threads can share: it starts as what the thread runs, a free random name for
    /// a worktree, or the label it has, not what its program calls it.
    pub fn ask_name(&mut self, what: Ask) {
        let name = match &what {
            Ask::NewWorktree { base, .. } => actions::random_name(|n| self.worktree_taken(*base, n)),
            Ask::NewThread { agent, .. } => default_label(*agent).to_string(),
            Ask::Rename { host, session } => sessions(&self.snap)
                .find(|(h, s)| h == host && s.name == *session)
                .map(|(_, s)| s.own_label.clone())
                .unwrap_or_else(|| self.label_of(host, session)),
            Ask::Find | Ask::Switch => String::new(),
            Ask::RenameFolder { host, id } => self.folder_name(host, id),
        };
        let branch = match &what {
            Ask::NewWorktree { base, .. } => {
                let cfg = self.cfg.clone();
                let base = *base;
                self.in_background(
                    move || actions::branches(&cfg, base),
                    |s, branches| {
                        if let (Ok(branches), Some(p)) = (branches, &mut s.prompt) {
                            p.from = branches.first().cloned();
                            p.branches = branches;
                        }
                    },
                );
                let mut b = TextEdit::new("");
                b.focused = false;
                Some(b)
            }
            _ => None,
        };
        self.prompt = Some(Prompt { what, input: field(&name), branch, branches: vec![], from: None, error: None, go: None });
    }

    pub fn close_prompt(&mut self) {
        self.prompt = None;
    }

    /// ⌘J: asks for Claude or a shell in the folder of the session shown, beside the thread
    /// on show if it's in it (in its folder), else in a folder of its own.
    pub fn new_thread_here(&mut self) {
        if self.overlaid() {
            return;
        }
        let Some(space) = self.shown_space() else { return };
        let Some(dir) = self.space_dir(&space) else {
            return self.note(format!("{} hasn't said where {} is yet", space.0, space.1));
        };
        let base = self.space_base(&space);
        let folder = self.folder_in(&space);
        let (host, space) = space;
        self.ask_name(Ask::NewThread { host, base, dir, agent: Some(Agent::Claude), folder, space });
    }

    /// The folder a thread started in session `space` goes in: the thread on show's if it's
    /// in it, else one of its own.
    fn folder_in(&mut self, space: &Space) -> Option<String> {
        let key = self.active.clone().filter(|key| self.space_of(key).as_ref() == Some(space))?;
        self.folder_beside(&key)
    }

    /// ⌘P: the spotlight, to go to any thread.
    pub fn find_thread(&mut self) {
        if !self.overlaid() {
            self.ask_name(Ask::Find);
        }
    }

    /// ⌘K: the spotlight, to switch sessions, as Rex's Change Session does, or start one.
    pub fn switch_session(&mut self) {
        if !self.overlaid() {
            self.ask_name(Ask::Switch);
        }
    }

    /// ⇧⌘N, or the + of the session shown, as Rex starts a session: a shell in the home of
    /// its machine, in a new folder of the session's.
    pub fn new_folder(&mut self) {
        if let Some(space) = self.shown_space() {
            self.new_folder_in(space);
        }
    }

    /// A shell in the home of session `space`'s machine, in a new folder of the session's.
    pub fn new_folder_in(&mut self, space: Space) {
        if self.overlaid() {
            return;
        }
        let Some(home) = self.space_dir(&(space.0.clone(), HOME.into())) else {
            return self.note(format!("{} hasn't said where its home is yet", space.0));
        };
        self.start_now(space.0.clone(), home, None, None, space.1);
    }

    /// The sessions ⌘K lists for what's typed, each word of it in its name, host or folder:
    /// the ones last on show first, the one shown last of all; then New session….
    pub fn switchable(&self, prompt: &Prompt) -> Vec<Option<SpaceView>> {
        if prompt.what != Ask::Switch {
            return vec![];
        }
        let all = self.spaces();
        let shown = self.shown_space();
        let mut order: Vec<Space> = vec![];
        for space in self.recent.iter().filter_map(|key| self.space_of(key)).chain(all.iter().map(|v| v.space.clone())) {
            if !order.contains(&space) && Some(&space) != shown.as_ref() {
                order.push(space);
            }
        }
        order.extend(shown);
        let typed = prompt.input.text.to_lowercase();
        let words: Vec<&str> = typed.split_whitespace().collect();
        let mut listed: Vec<Option<SpaceView>> = order
            .into_iter()
            .filter_map(|space| all.iter().find(|v| v.space == space).cloned())
            .filter(|v| {
                let text = format!("{} {} {}", v.name, v.host, v.space.1).to_lowercase();
                words.iter().all(|w| text.contains(w))
            })
            .map(Some)
            .take(SHOWN)
            .collect();
        listed.push(None);
        listed
    }

    /// Shows the session ⌘K picked, `pick` of those listed; or, past them, starts a new one.
    fn switch_pick(&mut self, pick: usize) {
        let Some(prompt) = &self.prompt else { return };
        let Some(picked) = self.switchable(prompt).into_iter().nth(pick) else { return };
        self.close_prompt();
        match picked {
            Some(v) => self.show_space(v.space),
            None => self.open_picker(),
        }
    }

    /// The text changed: an error about the name it had goes, and the pick starts over.
    fn prompt_changed(&mut self) {
        if let Some(prompt) = &mut self.prompt {
            let text = prompt.input.text.clone();
            if prompt.error.as_ref().is_some_and(|(_, name)| *name != text) {
                prompt.error = None;
            }
            prompt.go = None;
        }
    }

    /// ⏎: a thread or session picked opens; otherwise the name starts or renames.
    pub(super) fn prompt_enter(&mut self) {
        let Some(prompt) = &self.prompt else { return };
        match (&prompt.what, prompt.go) {
            (Ask::Find, go) => {
                self.go_to(go);
            }
            (Ask::Switch, go) => self.switch_pick(go.unwrap_or(0)),
            _ => self.submit(),
        }
    }

    /// The threads ⌘P lists to go to for what's typed: each word of it is in a thread's
    /// name, place or host; those opened last first, the one on show at the end. Nothing
    /// else lists them.
    pub fn matches(&self, prompt: &Prompt) -> Vec<Match> {
        if prompt.what != Ask::Find {
            return vec![];
        }
        let typed = prompt.input.text.to_lowercase();
        let words: Vec<&str> = typed.split_whitespace().collect();
        let mut found: Vec<(usize, Match)> = sessions(&self.snap)
            .map(|(host, s)| {
                let key: Key = (host.to_string(), s.name.clone());
                let (place, label) = self.titles(host, &s.name);
                let place = place.unwrap_or_else(|| host.to_string());
                let rank = match self.active.as_ref() == Some(&key) {
                    // The one on show last: ⏎ goes to the one before it.
                    true => usize::MAX,
                    false => self.recent.iter().position(|k| *k == key).unwrap_or(usize::MAX - 1),
                };
                (rank, Match { key, label, place })
            })
            .filter(|(_, m)| {
                let text = format!("{} {} {}", m.label, m.place, m.key.0).to_lowercase();
                words.iter().all(|w| text.contains(w))
            })
            .collect();
        // Stable: the sidebar's order within a rank.
        found.sort_by_key(|(rank, _)| *rank);
        found.into_iter().map(|(_, m)| m).take(SHOWN).collect()
    }

    /// Moves the pick one down (or with `back` up): for a new thread or worktree, through
    /// what it can run; else through what's listed.
    pub fn step(&mut self, back: bool) {
        let Some(prompt) = &self.prompt else { return };
        let (kinds, count) = match prompt.what {
            Ask::NewWorktree { .. } | Ask::NewThread { .. } => (KINDS.len(), 0),
            Ask::Switch => (0, self.switchable(prompt).len()),
            Ask::RenameFolder { .. } => return,
            _ => (0, self.matches(prompt).len()),
        };
        let total = kinds + count;
        if total == 0 {
            return;
        }
        let now = match (&prompt.what, prompt.go) {
            (_, Some(i)) => kinds + i,
            (Ask::NewThread { agent, .. } | Ask::NewWorktree { agent, .. }, None) => {
                KINDS.iter().position(|k| k == agent).unwrap_or(0)
            }
            _ => 0,
        };
        let next = card::step(now, total, back);
        if next < kinds {
            if let Some(prompt) = &mut self.prompt {
                prompt.go = None;
            }
            self.pick_kind(KINDS[next]);
        } else if let Some(prompt) = &mut self.prompt {
            prompt.go = Some(next - kinds);
        }
    }

    /// Opens the thread picked, or with none picked the first; false if there's none.
    pub fn go_to(&mut self, pick: Option<usize>) -> bool {
        let Some(prompt) = &self.prompt else { return false };
        let Some(m) = self.matches(prompt).into_iter().nth(pick.unwrap_or(0)) else { return false };
        self.close_prompt();
        let (host, session) = m.key;
        self.open(host, session);
        true
    }

    /// Picks what the new thread asked for runs: `agent`, or with none a shell. A thread's
    /// name follows, unless you typed one.
    pub fn pick_kind(&mut self, agent: Option<Agent>) {
        let Some(prompt) = &mut self.prompt else { return };
        let (picked, named) = match &mut prompt.what {
            Ask::NewThread { agent: picked, .. } => (picked, true),
            // Its name is the worktree's.
            Ask::NewWorktree { agent: picked, .. } => (picked, false),
            _ => return,
        };
        prompt.go = None;
        if *picked != agent {
            let typed = prompt.input.text != default_label(*picked);
            *picked = agent;
            if named && !typed {
                prompt.input = field(default_label(agent));
            }
        }
        prompt.input.focused = true;
    }

    fn submit(&mut self) {
        let Some(prompt) = &mut self.prompt else { return };
        let text = prompt.input.text.clone();
        let name = text.trim().to_string();
        let branch = prompt.branch.as_ref().map(|b| b.text.trim().to_string()).filter(|b| !b.is_empty());
        let from = prompt.from.clone();
        let folder = matches!(prompt.what, Ask::RenameFolder { .. });
        let valid = if folder {
            super::folders::validate_folder_name(&name)
        } else {
            actions::validate_name(&name).and_then(|()| branch.as_deref().map_or(Ok(()), actions::validate_branch))
        };
        if let Err(e) = valid {
            prompt.error = Some((format!("{e:#}"), text));
            return;
        }
        let Some(Prompt { what, .. }) = self.prompt.take() else { return };
        // The sidebar stays on the session it starts in till it shows there, as `start_now`
        // has it.
        match &what {
            Ask::NewThread { host, space, .. } => self.space_shown = Some((host.clone(), space.clone())),
            Ask::NewWorktree { base, .. } => {
                let b = &self.cfg.bases[*base];
                self.space_shown = Some((b.host.clone(), b.path.clone()));
            }
            _ => {}
        }
        let cfg = self.cfg.clone();
        let doing = match what {
            Ask::Rename { .. } => "renaming",
            Ask::Find | Ask::Switch => return,
            // Lemur's own: at once.
            Ask::RenameFolder { ref host, ref id } => return self.rename_folder(host, id, name),
            _ => "starting",
        };
        let said = self.busy(format!("{doing} {name}…"));
        self.in_background(
            move || match what {
                // The name is the worktree's: its thread shows what its program calls it.
                Ask::NewWorktree { base, agent } => {
                    actions::new_worktree_thread(&cfg, base, &name, default_label(agent), agent, branch.as_deref(), from.as_deref())
                        .map(|(started, note)| Done::Started(started, note))
                }
                Ask::NewThread { host, dir, agent, folder, space, .. } => {
                    actions::start_in_folder(&cfg, &host, &dir, &name, agent, folder.as_deref(), Some(&space)).map(|started| Done::Started(started, None))
                }
                Ask::Find | Ask::Switch | Ask::RenameFolder { .. } => unreachable!("going to a thread or a session starts nothing, nor does a folder"),
                Ask::Rename { host, session } => {
                    actions::rename_session(&cfg, &host, &session, &name).map(|session| Done::Renamed { host, session, name })
                }
            },
            move |s, done| {
                s.done(&said);
                match done {
                    Ok(Done::Started(started, note)) => {
                        if let Some(note) = note {
                            s.status = Some((note, Kind::Warning));
                        }
                        s.started(started);
                    }
                    Ok(Done::Renamed { host, session, name }) => s.follow_rename(host, session, name),
                    Err(e) => s.fail(e),
                }
                s.refresh();
            },
        );
    }

    /// Shows a thread's new label at once, and keeps it with the thread kept to resume.
    fn follow_rename(&mut self, host: String, session: String, name: String) {
        let kept = self.cfg.threads.iter().find(|t| t.host == host && t.session == session).cloned();
        if let Some(thread) = kept {
            let label = Some(name.clone());
            self.keep_thread(ClaudeThread { label, ..thread });
        }
        let key = (host, session);
        if let Some(s) = self.session_mut(&key) {
            s.label = name.clone();
            s.own_label = name;
        }
    }

    /// A worktree's name as the sidebar shows it.
    fn worktree_name(&self, host: &str, path: &str) -> String {
        self.snap.worktrees(host).find(|w| w.path == path).map(worktree_label).unwrap_or_else(|| leaf(path).to_string())
    }

    /// Whether `base` already has a worktree called `name`.
    fn worktree_taken(&self, base: usize, name: &str) -> bool {
        self.snap
            .hosts
            .iter()
            .flat_map(|h| h.bases.iter())
            .filter(|v| v.index == base)
            .flat_map(|v| v.worktrees.iter())
            .any(|w| leaf(&w.path) == name)
    }

    /// ⌘T: a shell in the folder of the session shown, beside the thread on show if it's in
    /// it, else in a folder of its own.
    pub fn new_terminal_here(&mut self) {
        if self.overlaid() {
            return;
        }
        let Some(space) = self.shown_space() else { return };
        let Some(dir) = self.space_dir(&space) else { return };
        let folder = self.folder_in(&space);
        self.start_now(space.0, dir, None, folder, space.1);
    }

    /// Starts `agent`, or with none a shell, in `dir` on `host`, labelled after what it
    /// runs, in session `space` there and in `folder` of its host's (one of its own with
    /// none), and shows it.
    pub fn start_now(&mut self, host: String, dir: String, agent: Option<Agent>, folder: Option<String>, space: String) {
        let label = default_label(agent);
        let said = self.busy(format!("starting {label}…"));
        let cfg = self.cfg.clone();
        // The sidebar stays on it till the thread shows there.
        self.space_shown = Some((host.clone(), space.clone()));
        self.in_background(
            move || actions::start_in_folder(&cfg, &host, &dir, label, agent, folder.as_deref(), Some(&space)),
            move |s, started| {
                s.done(&said);
                match started {
                    Ok(started) => s.started(started),
                    Err(e) => s.fail(e),
                }
                s.refresh();
            },
        );
    }

    /// A thread was started: it's kept to resume if it's Claude's, and shows.
    fn started(&mut self, started: actions::Started) {
        if let Some(claude) = started.claude {
            self.keep_thread(claude);
        }
        self.open_fresh(started.host, started.session);
    }
}

/// A thread's icon: its agent's, or a terminal.
pub fn agent_icon(agent: Option<Agent>, k: &Kit) -> El {
    let (glyph, ink) = agent_glyph(agent, k);
    row().size(16.0, 16.0).center().child(icon(glyph, 14.0).color(ink))
}

/// A thread's icon: its agent's, or a terminal; given a colour, white on a tile of it, as a
/// project's mark is.
pub fn thread_icon(agent: Option<Agent>, color: Option<&str>, k: &Kit) -> El {
    let Some(fill) = color.and_then(super::sidebar::color_named) else { return agent_icon(agent, k) };
    let (glyph, _) = agent_glyph(agent, k);
    row().key("color-tile").size(18.0, 18.0).radius(5.0).fill(fill).center().child(icon(glyph, 11.0).color(rune::Color::WHITE))
}

/// A thread's glyph and its colour: its agent's, or a terminal.
pub fn agent_glyph(agent: Option<Agent>, k: &Kit) -> (&'static rune::Icon, rune::Color) {
    match agent {
        Some(Agent::Claude) => (&icons::CLAUDE, rune::color::hex("#d97757").unwrap()),
        None => (&icons::SQUARE_TERMINAL, k.c.icon_muted),
    }
}

/// The spotlight, over the window: what it's for, what a new thread runs, the name, the
/// threads to go to, a new worktree's branch and start, and a hint or why the name can't be
/// used.
pub fn card(s: &Lemur, prompt: &Prompt, k: &Kit) -> El {
    let c = &k.c;
    let mut kind = None;
    let mut listed = false;
    let (glyph, what, hint) = match &prompt.what {
        Ask::NewWorktree { base, agent } => {
            kind = Some(*agent);
            (&icons::FOLDER, format!("New worktree in {}", s.cfg.bases[*base].name()), "↑↓ Claude or shell · ⏎ start · esc cancel")
        }
        Ask::NewThread { host, dir, agent, .. } => {
            kind = Some(*agent);
            (&icons::PLUS, format!("New in {}", s.worktree_name(host, dir)), "↑↓ Claude or shell · ⏎ start · esc cancel")
        }
        Ask::Find => {
            listed = true;
            (&icons::SEARCH, "Go to a thread".to_string(), "↑↓ pick · ⏎ go · esc cancel")
        }
        Ask::Switch => (&icons::FOLDER, "Switch session".to_string(), "↑↓ pick · ⏎ show · esc cancel"),
        Ask::Rename { host, session } => (&icons::PENCIL, format!("Rename {}", s.label_of(host, session)), "⏎ rename · esc cancel"),
        Ask::RenameFolder { host, id } => {
            (&icons::PENCIL, format!("Rename {}", s.folder_name(host, id)), "⏎ rename · empty gives it back its own · esc cancel")
        }
    };
    let is_new = matches!(prompt.what, Ask::NewThread { .. } | Ask::NewWorktree { .. });
    let pick_row = |id: String, label: String, detail: Option<String>, lead: El, picked: bool| card::pick_row(id, Some(lead), label, detail, None, picked, k);
    let mut card = card::surface("spotlight", card::WIDTH, k).child(card::title("spotlight-title", glyph, what, k));
    if is_new {
        let mut kinds = col().key("kinds").padding(4.0).gap(2.0);
        for (i, agent) in KINDS.iter().enumerate() {
            let name = match agent {
                Some(Agent::Claude) => "Claude",
                None => "Shell",
            };
            let a = *agent;
            kinds = kinds.child(
                pick_row(format!("kind-{i}"), name.into(), None, agent_icon(a, k), kind == Some(a))
                    .on_press(move |s: &mut Lemur| s.pick_kind(a)),
            );
        }
        card = card.child(kinds).child(card::divider(k));
    }
    let input = TextInput::new(&prompt.input)
        .placeholder(match prompt.what {
            Ask::Find => "Go to a thread…",
            Ask::Switch => "Session…",
            _ => "name",
        })
        .width(None)
        .plain()
        .on_edit(|s: &mut Lemur| &mut s.prompt.as_mut().expect("a prompt").input)
        .on_change(|s: &mut Lemur, _| s.prompt_changed())
        .on_submit(|s: &mut Lemur, _| s.prompt_enter())
        .on_key(|s: &mut Lemur, ev, tx| match ev.key {
            K::Up => {
                s.step(true);
                tx.instant();
                true
            }
            K::Down => {
                s.step(false);
                tx.instant();
                true
            }
            _ => false,
        })
        .build(k)
        .key("spotlight-input");
    card = card.child(card::field(input));
    if listed {
        let matches = s.matches(prompt);
        if !matches.is_empty() {
            let picked = Some(prompt.go.unwrap_or(0));
            let mut rows = col().key("matches").padding(4.0).gap(2.0);
            for (i, m) in matches.into_iter().enumerate() {
                let agent = s.session_of(&m.key).and_then(|s| s.agent());
                rows = rows.child(
                    pick_row(format!("go-{i}"), m.label, Some(m.place), agent_icon(agent, k), picked == Some(i))
                        .on_press(move |s: &mut Lemur| {
                            s.go_to(Some(i));
                        }),
                );
            }
            card = card.child(card::divider(k)).child(rows);
        }
    }
    if prompt.what == Ask::Switch {
        let picked = prompt.go.unwrap_or(0);
        let mut rows = col().key("sessions").padding(4.0).gap(2.0);
        for (i, v) in s.switchable(prompt).into_iter().enumerate() {
            let (glyph, name, detail) = match v {
                Some(v) if v.space.1 == HOME => (&icons::HOUSE, v.name, Some(v.host)),
                Some(v) => (&icons::FOLDER, v.name, Some(v.host)),
                None => (&icons::PLUS, "New session…".to_string(), None),
            };
            let lead = row().size(16.0, 16.0).center().child(icon(glyph, 14.0).color(c.icon_muted));
            rows = rows.child(pick_row(format!("session-{i}"), name, detail, lead, picked == i).on_press(move |s: &mut Lemur| s.switch_pick(i)));
        }
        card = card.child(card::divider(k)).child(rows);
    }
    if let Some(branch) = &prompt.branch {
        let line = |label: &str, field: El| {
            row().gap(8.0).center_y().child(text(label.to_owned()).font(k.ty.small).color(c.text_muted).w(56.0)).child(field.grow(1.0))
        };
        let branch_field = TextInput::new(branch)
            .placeholder("none, to detach")
            .width(None)
            .on_edit(|s: &mut Lemur| s.prompt.as_mut().and_then(|p| p.branch.as_mut()).expect("a branch field"))
            .on_change(|s: &mut Lemur, _| {
                if let Some(p) = &mut s.prompt {
                    p.error = None;
                }
            })
            .on_submit(|s: &mut Lemur, _| s.submit())
            .build(k)
            .key("branch");
        let from = prompt.from.clone().unwrap_or_else(|| "the default branch".into());
        let from_button = rune_kit::Button::new(from)
            .variant(rune_kit::ButtonVariant::Outline)
            .on_press_tx(|s: &mut Lemur, tx: &mut Tx| {
                let Some(p) = &s.prompt else { return };
                let items: Vec<(String, String)> = p.branches.iter().map(|b| (b.clone(), b.clone())).collect();
                let current = p.from.clone().unwrap_or_default();
                let menu = PopupMenu::picker(items.iter().map(|(a, b)| (a.as_str(), b.as_str())), current.as_str());
                tx.popup_menu(menu, "from", |s: &mut Lemur, picked, _| {
                    if let (Some(picked), Some(p)) = (picked, &mut s.prompt) {
                        p.from = Some(picked.to_string());
                    }
                });
            })
            .build(k)
            .key("from");
        card = card.child(col().key("worktree-fields").px(12.0).pb(8.0).gap(8.0).child(line("Branch", branch_field)).child(line("From", from_button)));
    }
    let (footer, error) = match &prompt.error {
        Some((e, _)) => (e.clone(), true),
        None => (hint.to_string(), false),
    };
    card.child(card::footer("spotlight-footer", footer, error, k))
}
