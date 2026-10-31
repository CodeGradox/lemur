//! Going to a thread from the spotlight: ⌘T lists the threads under the
//! new thread's name, the worktree's first, and ⌘P lists them all. What's
//! typed narrows them down; ↑↓ picks one and ⏎ opens it.

use super::sidebar::thread_icon;
use super::{Ask, Key, Lemur, Prompt, sessions};
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

/// How many threads the spotlight lists at most.
const SHOWN: usize = 8;

/// A thread to go to, as the spotlight lists it.
pub(super) struct Match {
    pub(super) key: Key,
    pub(super) label: String,
    /// `project › worktree`, or its host's name outside every project.
    pub(super) place: String,
}

impl Lemur {
    /// The threads to go to for what's typed in `prompt`: each word of it
    /// is in a thread's name, place or host. ⌘T lists the threads of its
    /// worktree first, then its project's; ⌘P those opened last, the one
    /// on show at the end.
    pub(super) fn matches(&self, prompt: &Prompt, cx: &App) -> Vec<Match> {
        let typed = prompt.input.read(cx).value().to_lowercase();
        let query = match &prompt.what {
            // The name it starts with isn't something you searched for.
            Ask::NewThread { shell, .. } if typed == super::prompt::default_label(*shell) => "",
            _ => typed.trim(),
        };
        let words: Vec<&str> = query.split_whitespace().collect();
        let mut found: Vec<(usize, Match)> = sessions(&self.snap)
            .map(|(host, s)| {
                let key: Key = (host.to_string(), s.name.clone());
                let (place, label) = self.titles(host, &s.name);
                let place = place.unwrap_or_else(|| host.to_string());
                let rank = match &prompt.what {
                    Ask::NewThread { base, worktree, .. } => {
                        let in_project = self
                            .project_of(host, &s.name)
                            .map(|(index, w)| (index == *base, w.path == *worktree));
                        match in_project {
                            Some((_, true)) => 0,
                            Some((true, false)) => 1,
                            _ => 2,
                        }
                    }
                    // The one on show last: ⏎ goes to the one before it.
                    _ if self.active.as_ref() == Some(&key) => usize::MAX,
                    _ => self
                        .recent
                        .iter()
                        .position(|k| *k == key)
                        .unwrap_or(usize::MAX - 1),
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

    /// Moves the pick one down (or with `back` up): for a new thread,
    /// through Claude and Shell, then the threads to go to.
    pub(super) fn step(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = &self.prompt else { return };
        let count = self.matches(prompt, cx).len();
        let kinds = match prompt.what {
            Ask::NewThread { .. } => 2,
            _ => 0,
        };
        let total = kinds + count;
        if total == 0 {
            return;
        }
        let now = match (&prompt.what, prompt.go) {
            (_, Some(i)) => kinds + i,
            (Ask::NewThread { shell, .. }, None) => usize::from(*shell),
            _ => 0,
        };
        let next = if back {
            (now + total - 1) % total
        } else {
            (now + 1) % total
        };
        if next < kinds {
            if let Some(prompt) = &mut self.prompt {
                prompt.go = None;
            }
            self.pick_kind(next == 1, window, cx);
        } else if let Some(prompt) = &mut self.prompt {
            prompt.go = Some(next - kinds);
        }
        cx.notify();
    }

    /// Opens the thread picked, or with none picked the first; false if
    /// there's none to open.
    pub(super) fn go_to(
        &mut self,
        pick: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(prompt) = &self.prompt else {
            return false;
        };
        let matches = self.matches(prompt, cx);
        let Some(m) = matches.into_iter().nth(pick.unwrap_or(0)) else {
            return false;
        };
        self.prompt = None;
        let (host, session) = m.key;
        self.open(host, session, window, cx);
        true
    }

    /// ⌘P: the spotlight, to go to any thread.
    pub(super) fn find_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.prompt.is_none() && self.picker.is_none() {
            self.ask_name(Ask::Find, window, cx);
        }
    }

    /// The threads to go to, under the name field.
    pub(super) fn match_rows(&self, prompt: &Prompt, cx: &Context<Self>) -> Option<Div> {
        let matches = self.matches(prompt, cx);
        if matches.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let picked = match prompt.what {
            Ask::Find => Some(prompt.go.unwrap_or(0)),
            _ => prompt.go,
        };
        let rows = matches.into_iter().enumerate().map(|(i, m)| {
            let s = sessions(&self.snap)
                .find(|(h, s)| *h == m.key.0 && s.name == m.key.1)
                .map(|(_, s)| s.clone());
            h_flex()
                .id(SharedString::from(format!("go-{i}")))
                .debug_selector(move || format!("go {i}"))
                .px_2()
                .py_1()
                .gap_2()
                .rounded(theme.radius)
                .text_sm()
                .cursor_pointer()
                .when(picked == Some(i), |row| {
                    row.bg(theme.accent).text_color(theme.accent_foreground)
                })
                .children(s.map(|s| thread_icon(&s).small()))
                .child(div().flex_shrink_0().child(m.label))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(m.place),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.go_to(Some(i), window, cx);
                }))
        });
        Some(
            v_flex()
                .p_1()
                .gap_0p5()
                .border_t_1()
                .border_color(theme.border)
                .children(rows),
        )
    }
}
