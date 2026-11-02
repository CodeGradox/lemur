//! Names asked for in the spotlight: for a new worktree, a new thread or
//! a rename, and what happens once one is given.

use super::sidebar::{claude_icon, codex_icon};
use super::snackbar::Kind;
use super::{Ask, Lemur, Prompt, sessions, spotlight, worktree_label};
use crate::actions;
use crate::config::{Agent, ClaudeThread, leaf};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{SearchableVec, Select, SelectState};
use gpui_kit::component::{ActiveTheme, Icon, IconName, IndexPath, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

actions!(new_thread, [Up, Down]);

/// Key context of the spotlight for a new thread, or one to go to.
const NEW_THREAD: &str = "NewThread";

/// ↑↓ pick what a new thread runs, or a thread to go to. The
/// name field goes first and passes on the keys it has no use for.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", Up, Some(NEW_THREAD)),
        KeyBinding::new("down", Down, Some(NEW_THREAD)),
    ]);
}

/// What a new thread can run, as they're listed: an agent, or a shell.
pub(super) const KINDS: [Option<Agent>; 3] = [Some(Agent::Claude), Some(Agent::Codex), None];

/// What a new thread is called until you name it.
pub(super) fn default_label(agent: Option<Agent>) -> &'static str {
    match agent {
        Some(Agent::Claude) => "claude",
        Some(Agent::Codex) => "codex",
        None => "shell",
    }
}

/// A new worktree's branch, empty for none, and the branch it starts
/// from, picked from the host's branches once they've loaded.
pub(super) struct WorktreeFields {
    pub(super) branch: Entity<InputState>,
    pub(super) from: Entity<SelectState<SearchableVec<String>>>,
    _events: Subscription,
    _load: Task<()>,
}

enum Done {
    /// With a note about how it went, if there's one.
    Started(actions::Started, Option<String>),
    Renamed {
        host: String,
        session: String,
        name: String,
    },
}

impl Lemur {
    /// Opens the name field, selected so typing replaces it. It asks for a
    /// label, which several threads can share: it starts as what the thread
    /// runs, a free random name for a worktree, or the current label.
    pub(super) fn ask_name(&mut self, what: Ask, window: &mut Window, cx: &mut Context<Self>) {
        let name = match &what {
            Ask::NewWorktree { base, .. } => {
                actions::random_name(|n| self.worktree_taken(*base, n))
            }
            Ask::NewThread { agent, .. } => default_label(*agent).to_string(),
            Ask::Rename { host, session } => self.label_of(host, session),
            Ask::Find => String::new(),
        };
        let placeholder = match &what {
            Ask::Find => "Go to a thread…",
            _ => "name",
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(name)
        });
        let events =
            cx.subscribe_in(
                &input,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    // A thread picked to go to opens; otherwise the name
                    // starts or renames.
                    InputEvent::PressEnter { .. } => match this.prompt.as_ref() {
                        Some(Prompt {
                            what: Ask::Find,
                            go,
                            ..
                        }) => {
                            let go = *go;
                            this.go_to(go, window, cx);
                        }
                        Some(Prompt { go: Some(i), .. }) => {
                            let i = *i;
                            this.go_to(Some(i), window, cx);
                        }
                        _ => this.submit(cx),
                    },
                    // Enter also reports a change; only new text answers the error.
                    InputEvent::Change => {
                        if let Some(prompt) = &mut this.prompt {
                            let text = prompt.input.read(cx).value();
                            if prompt.error.as_ref().is_some_and(|(_, name)| *name != text) {
                                prompt.error = None;
                            }
                            // What's listed changed, so the pick starts over.
                            prompt.go = None;
                        }
                        cx.notify();
                    }
                    _ => {}
                },
            );
        input.update(cx, |i, cx| {
            i.focus(window, cx);
            i.select_all(window, cx);
        });
        let worktree = match &what {
            Ask::NewWorktree { base, .. } => Some(self.worktree_fields(*base, window, cx)),
            _ => None,
        };
        self.prompt = Some(Prompt {
            what,
            input,
            worktree,
            error: None,
            go: None,
            _events: events,
        });
        cx.notify();
    }

    /// The branch field, empty, and the branches of `base` to start from,
    /// the default one picked once they've loaded. Without them, the host
    /// picks its default branch.
    fn worktree_fields(
        &self,
        base: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> WorktreeFields {
        let branch = cx.new(|cx| InputState::new(window, cx).placeholder("none, to detach"));
        let from = cx.new(|cx| {
            SelectState::new(SearchableVec::new(Vec::<String>::new()), None, window, cx)
                .searchable(true)
        });
        let events =
            cx.subscribe_in(
                &branch,
                window,
                |this, _, event: &InputEvent, _, cx| match event {
                    InputEvent::PressEnter { .. } => this.submit(cx),
                    InputEvent::Change => {
                        if let Some(prompt) = &mut this.prompt {
                            prompt.error = None;
                        }
                        cx.notify();
                    }
                    _ => {}
                },
            );
        let cfg = self.cfg.clone();
        let load = self.in_background(
            cx,
            move || actions::branches(&cfg, base),
            |this, branches, window, cx| {
                let fields = this.prompt.as_ref().and_then(|p| p.worktree.as_ref());
                if let (Ok(branches), Some(fields)) = (branches, fields) {
                    fields.from.update(cx, |from, cx| {
                        from.set_items(SearchableVec::new(branches), window, cx);
                        from.set_selected_index(Some(IndexPath::default()), window, cx);
                    });
                }
            },
        );
        WorktreeFields {
            branch,
            from,
            _events: events,
            _load: load,
        }
    }

    /// The name field, in the spotlight: what it's for, the name, and a
    /// hint or why the name can't be used. What a new thread runs is picked
    /// above the name.
    pub(super) fn prompt_card(&self, prompt: &Prompt, cx: &Context<Self>) -> Div {
        let mut kind = None;
        let mut listed = false;
        let (icon, what, hint) = match &prompt.what {
            Ask::NewWorktree { base, agent } => {
                kind = Some(*agent);
                (
                    IconName::Folder,
                    format!("New worktree in {}", self.cfg.bases[*base].name()),
                    "↑↓ Claude, Codex or shell · ⏎ start · esc cancel",
                )
            }
            Ask::NewThread {
                host, dir, agent, ..
            } => {
                // While a thread is picked, what to start isn't.
                kind = Some(*agent).filter(|_| prompt.go.is_none());
                listed = true;
                (
                    IconName::Plus,
                    format!("New in {}, or go to", self.worktree_name(host, dir)),
                    "↑↓ Claude, Codex, shell or a thread · ⏎ start or go · esc cancel",
                )
            }
            Ask::Find => {
                listed = true;
                (
                    IconName::Search,
                    "Go to a thread".to_string(),
                    "↑↓ pick · ⏎ go · esc cancel",
                )
            }
            Ask::Rename { host, session } => (
                IconName::CaseSensitive,
                format!("Rename {}", self.label_of(host, session)),
                "⏎ rename · esc cancel",
            ),
        };
        let footer = match &prompt.error {
            Some((error, _)) => spotlight::footer(error.clone(), true, cx),
            None => spotlight::footer(hint, false, cx),
        };
        let is_new = matches!(prompt.what, Ask::NewThread { .. } | Ask::NewWorktree { .. });
        spotlight::card(cx)
            .child(spotlight::title(icon, what, cx))
            .when(listed || is_new, |card| {
                card.key_context(NEW_THREAD)
                    .on_action(cx.listener(|this, _: &Up, window, cx| this.step(true, window, cx)))
                    .on_action(
                        cx.listener(|this, _: &Down, window, cx| this.step(false, window, cx)),
                    )
            })
            .when(is_new, |card| card.child(kinds(kind, cx)))
            .child(
                div()
                    .p_1()
                    .child(Input::new(&prompt.input).appearance(false)),
            )
            // A rename or a new worktree's name isn't a thread to go to:
            // a click on one would drop what's typed.
            .when(listed, |card| card.children(self.match_rows(prompt, cx)))
            .when_some(prompt.worktree.as_ref(), |card, fields| {
                card.child(
                    v_flex()
                        .px_3()
                        .pb_2()
                        .gap_2()
                        .child(field("Branch", Input::new(&fields.branch).small(), cx))
                        .child(field(
                            "From",
                            Select::new(&fields.from)
                                .small()
                                .placeholder("the default branch")
                                .search_placeholder("Search branches"),
                            cx,
                        )),
                )
            })
            .child(footer)
    }

    /// Picks what the new thread asked for runs: `agent`, or with none a
    /// shell. A thread's name follows, unless you typed one.
    pub(super) fn pick_kind(
        &mut self,
        agent: Option<Agent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(prompt) = &mut self.prompt else {
            return;
        };
        let (picked, named) = match &mut prompt.what {
            Ask::NewThread { agent: picked, .. } => (picked, true),
            // Its name is the worktree's.
            Ask::NewWorktree { agent: picked, .. } => (picked, false),
            _ => return,
        };
        prompt.go = None;
        let input = &prompt.input;
        if *picked != agent {
            let typed = input.read(cx).value() != default_label(*picked);
            *picked = agent;
            if named && !typed {
                input.update(cx, |i, cx| {
                    i.set_value(default_label(agent), window, cx);
                    i.select_all(window, cx);
                });
            }
        }
        // A click on a kind took the keyboard.
        input.update(cx, |i, cx| i.focus(window, cx));
        cx.notify();
    }

    /// ⌘J: asks for a new agent's thread or shell where the one on show runs:
    /// its worktree, or its folder outside every worktree.
    pub(super) fn new_thread_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlaid() {
            return;
        }
        let Some(key) = self.active.clone() else {
            return;
        };
        let (dir, base) = match self.project_of(&key.0, &key.1) {
            Some((base, worktree)) => (worktree.path.clone(), Some(base)),
            None => match self.session_of(&key) {
                Some(s) => (s.path.clone(), None),
                // Not listed yet: there's no telling where it runs.
                None => return self.ask_name(Ask::Find, window, cx),
            },
        };
        let ask = Ask::NewThread {
            host: key.0,
            base,
            dir,
            agent: Some(Agent::Claude),
        };
        self.ask_name(ask, window, cx);
    }

    pub(super) fn close_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt = None;
        self.focus_terminal(window, cx);
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = &mut self.prompt else {
            return;
        };
        let text = prompt.input.read(cx).value().to_string();
        let name = text.trim().to_string();
        let (branch, from) = match &prompt.worktree {
            Some(fields) => {
                let branch = fields.branch.read(cx).value().trim().to_string();
                let from = fields.from.read(cx).selected_value().cloned();
                ((!branch.is_empty()).then_some(branch), from)
            }
            None => (None, None),
        };
        let valid = actions::validate_name(&name)
            .and_then(|()| branch.as_deref().map_or(Ok(()), actions::validate_branch));
        if let Err(e) = valid {
            prompt.error = Some((format!("{e:#}"), text));
            cx.notify();
            return;
        }
        let Some(Prompt { what, .. }) = self.prompt.take() else {
            return;
        };
        let cfg = self.cfg.clone();
        let doing = match &what {
            Ask::Rename { .. } => "renaming",
            Ask::Find => return,
            _ => "starting",
        };
        self.status = Some((format!("{doing} {name}…"), Kind::Progress));
        cx.notify();
        self.in_background(
            cx,
            move || match what {
                Ask::NewWorktree { base, agent } => actions::new_worktree_thread(
                    &cfg,
                    base,
                    &name,
                    &name,
                    agent,
                    branch.as_deref(),
                    from.as_deref(),
                )
                .map(|(started, note)| Done::Started(started, note)),
                Ask::NewThread {
                    host, dir, agent, ..
                } => actions::start_in(&cfg, &host, &dir, &name, agent)
                    .map(|started| Done::Started(started, None)),
                Ask::Find => unreachable!("going to a thread starts nothing"),
                Ask::Rename { host, session } => {
                    actions::rename_session(&cfg, &host, &session, &name).map(|session| {
                        Done::Renamed {
                            host,
                            session,
                            name,
                        }
                    })
                }
            },
            |this, done, window, cx| {
                match done {
                    Ok(Done::Started(started, note)) => {
                        this.status = note.map(|note| (note, Kind::Warning));
                        if let Some(claude) = started.claude {
                            this.keep_thread(claude, cx);
                        }
                        this.open_fresh(started.host, started.session, window, cx);
                    }
                    Ok(Done::Renamed {
                        host,
                        session,
                        name,
                    }) => {
                        this.status = None;
                        this.follow_rename(host, session, name, cx);
                    }
                    Err(e) => this.fail(e, cx),
                }
                this.refresh(cx).detach();
            },
        )
        .detach();
    }

    /// Shows a thread's new label at once, and keeps it with the thread
    /// kept to resume.
    fn follow_rename(
        &mut self,
        host: String,
        session: String,
        name: String,
        cx: &mut Context<Self>,
    ) {
        let kept = self
            .cfg
            .threads
            .iter()
            .find(|t| t.host == host && t.session == session)
            .cloned();
        if let Some(thread) = kept {
            let label = Some(name.clone());
            self.keep_thread(ClaudeThread { label, ..thread }, cx);
        }
        let shown = self
            .snap
            .hosts
            .iter_mut()
            .filter(|h| h.name == host)
            .flat_map(|h| {
                let in_bases = h.bases.iter_mut().flat_map(|b| b.worktrees.iter_mut());
                in_bases
                    .flat_map(|w| w.sessions.iter_mut())
                    .chain(h.other.iter_mut())
            });
        for s in shown.filter(|s| s.name == session) {
            s.label = name.clone();
        }
        cx.notify();
    }

    /// What the sidebar calls a thread.
    pub(super) fn label_of(&self, host: &str, session: &str) -> String {
        sessions(&self.snap)
            .find(|(h, s)| *h == host && s.name == session)
            .map(|(_, s)| s.label.clone())
            .unwrap_or_else(|| actions::label(leaf(session)).to_string())
    }

    /// A worktree's name as the sidebar shows it.
    fn worktree_name(&self, host: &str, path: &str) -> String {
        self.snap
            .worktrees(host)
            .find(|w| w.path == path)
            .map(worktree_label)
            .unwrap_or_else(|| leaf(path).to_string())
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
}

/// What a new thread can run, to pick from: `picked` is highlighted, and with
/// none nothing is.
fn kinds(picked: Option<Option<Agent>>, cx: &Context<Lemur>) -> Div {
    let theme = cx.theme();
    let row = |label: &'static str, icon: Icon, kind: Option<Agent>| {
        h_flex()
            .id(label)
            .debug_selector(move || format!("kind {label}"))
            .px_2()
            .py_1()
            .gap_2()
            .rounded(theme.radius)
            .text_sm()
            .cursor_pointer()
            .when(picked == Some(kind), |row| {
                row.bg(theme.accent).text_color(theme.accent_foreground)
            })
            .child(icon.small())
            .child(label)
            .on_click(cx.listener(move |this, _, window, cx| this.pick_kind(kind, window, cx)))
    };
    v_flex()
        .p_1()
        .gap_0p5()
        .border_b_1()
        .border_color(theme.border)
        .child(row("Claude", claude_icon(), Some(Agent::Claude)))
        .child(row("Codex", codex_icon(), Some(Agent::Codex)))
        .child(row("Shell", Icon::new(IconName::SquareTerminal), None))
}

/// A line of the new worktree's fields: what it is, then the field.
fn field(label: &str, input: impl IntoElement, cx: &App) -> Div {
    let selector = format!("field {label}");
    h_flex()
        .debug_selector(move || selector)
        .gap_2()
        .child(
            div()
                .w(px(56.0))
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_string()),
        )
        .child(div().flex_1().child(input))
}
