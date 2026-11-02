//! The Changes panel, on the right as the notes and the agents are, as
//! Herdr's: the files changed in the worktree on show since its last
//! commit, each with its lines added and removed; a click shows its diff,
//! and another hides it, and it scrolls both ways. Its button shows it over
//! the window (`peek`).
//! Under them, a message and Commit commit the files listed. Other
//! repositories inside the worktree, as worktrees kept in a project's
//! folder, are neither listed nor committed.

use super::settings::Prefs;
use crate::config::Host;
use crate::remote::{Remote, quote, sh_path};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, StyledExt, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::collections::{HashMap, HashSet};

/// The most diff lines shown of one file.
const MOST_LINES: usize = 3000;

/// A changed file: its path in the worktree, git's word for the change,
/// and its lines added and removed (none known for a new file until it's
/// shown, nor ever for a binary one).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Change {
    pub(super) path: String,
    /// M changed, A added, D deleted, ? new (not tracked yet), U in
    /// conflict.
    pub(super) status: char,
    /// Changed since it was staged, or never staged: Commit adds it. One
    /// staged as it is, a removal or a rename too, is committed as staged.
    pub(super) unstaged: bool,
    pub(super) added: Option<u64>,
    pub(super) removed: Option<u64>,
}

/// A commit went in: the status bar's count is out of date.
pub(super) struct Committed;

impl EventEmitter<Committed> for ChangesPanel {}

/// Its diff is to show over the window.
pub(super) struct Preview;

impl EventEmitter<Preview> for ChangesPanel {}

pub(super) struct ChangesPanel {
    /// The host and worktree shown, and what it's called.
    place: Option<(Host, String, String)>,
    files: Vec<Change>,
    /// Why there are none to show, as outside a repository.
    error: Option<String>,
    loading: bool,
    /// The file whose diff shows, and its lines.
    open: Option<String>,
    lines: Vec<SharedString>,
    pub(super) diff_scroll: UniformListScrollHandle,
    message: Entity<InputState>,
    /// The host and worktree a commit is going in, while it is.
    committing: Option<(String, String)>,
    /// What the last commit did, or why it didn't.
    note: Option<(String, bool)>,
    _message: Subscription,
    /// Asking the worktree for its changes, for a file's diff, and
    /// committing, each apart, so none cancels another.
    _refresh: Option<Task<()>>,
    _diff: Option<Task<()>>,
    _commit: Option<Task<()>>,
}

impl ChangesPanel {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message = cx.new(|cx| InputState::new(window, cx).placeholder("Commit message"));
        let events = cx.subscribe_in(
            &message,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.commit(window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            },
        );
        ChangesPanel {
            place: None,
            files: vec![],
            error: None,
            loading: false,
            open: None,
            lines: vec![],
            diff_scroll: UniformListScrollHandle::new(),
            message,
            committing: None,
            note: None,
            _message: events,
            _refresh: None,
            _diff: None,
            _commit: None,
        }
    }

    /// Whether the commit message has the keyboard.
    pub(super) fn typing(&self, window: &Window, cx: &App) -> bool {
        self.message.focus_handle(cx).is_focused(window)
    }

    #[cfg(test)]
    pub(super) fn focus_message(&self, window: &mut Window, cx: &mut App) {
        self.message.focus_handle(cx).focus(window, cx);
    }

    /// How many files are in conflict: committing them would commit their
    /// markers, and conclude a merge or a step of a rebase with them.
    fn in_conflict(&self) -> usize {
        self.files.iter().filter(|f| f.status == 'U').count()
    }

    /// The file whose diff shows, and its lines.
    pub(super) fn shown_diff(&self) -> Option<(&Change, &[SharedString])> {
        let open = self.open.as_ref()?;
        let file = self.files.iter().find(|f| f.path == *open)?;
        Some((file, &self.lines))
    }

    /// Shows worktree `path` on `host`, called `label`, and asks it again.
    pub(super) fn show(&mut self, host: Host, path: String, label: String, cx: &mut Context<Self>) {
        let same = self
            .place
            .as_ref()
            .is_some_and(|(h, p, _)| h.name == host.name && *p == path);
        if !same {
            self.files.clear();
            self.open = None;
            self.lines.clear();
            self.error = None;
            self.note = None;
            // A diff over the window goes now, not once this is asked.
            cx.notify();
        }
        self.place = Some((host, path, label));
        self.refresh(cx);
    }

    #[cfg(test)]
    pub(super) fn listed(&self) -> (Vec<(String, char)>, Vec<String>) {
        let files = self
            .files
            .iter()
            .map(|f| (f.path.clone(), f.status))
            .collect();
        (files, self.lines.iter().map(|l| l.to_string()).collect())
    }

    /// Whether it shows worktree `on`, by its host and path; with None,
    /// nothing.
    pub(super) fn is_on(&self, on: Option<(&str, &str)>) -> bool {
        let place = self.place.as_ref();
        place.map(|(h, p, _)| (h.name.as_str(), p.as_str())) == on
    }

    /// Nothing on show to tell changes of.
    pub(super) fn clear(&mut self, cx: &mut Context<Self>) {
        if self.place.is_none() {
            return;
        }
        self.place = None;
        self.files.clear();
        self.open = None;
        self.lines.clear();
        cx.notify();
    }

    /// Asks the worktree for its changes, and the open file's diff.
    pub(super) fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some((host, path, _)) = self.place.clone() else {
            return;
        };
        // A commit going in here changes what there is to list; it asks
        // again once it's in.
        let asked = (host.name.clone(), path.clone());
        if self.loading || self.committing.as_ref() == Some(&asked) {
            return;
        }
        self.loading = true;
        let open = self.open.clone();
        let diffed = open.clone();
        self._refresh = Some(cx.spawn(async move |this, cx| {
            let (files, diff) = cx
                .background_spawn(async move {
                    let files = changes_in(&host, &path);
                    let diff = match (&files, &open) {
                        (Ok(files), Some(open)) => files
                            .iter()
                            .find(|f| f.path == *open)
                            .map(|f| diff_of(&host, &path, f)),
                        _ => None,
                    };
                    (files, diff)
                })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                // Another worktree shows now: its changes, not these.
                if !this.shows(&asked) {
                    this.refresh(cx);
                    return;
                }
                match files {
                    Ok(files) => {
                        this.error = None;
                        if this
                            .open
                            .as_ref()
                            .is_some_and(|o| !files.iter().any(|f| f.path == *o))
                        {
                            this.open = None;
                            this.lines.clear();
                        }
                        this.files = files;
                    }
                    Err(e) => {
                        this.error = Some(e);
                        this.files.clear();
                        this.open = None;
                        this.lines.clear();
                    }
                }
                // Still the file shown: it may have been hidden meanwhile.
                if let Some(diff) = diff.filter(|_| this.open == diffed) {
                    this.set_diff(diff);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Whether worktree `path` on host `name` shows.
    fn shows(&self, (name, path): &(String, String)) -> bool {
        self.place
            .as_ref()
            .is_some_and(|(h, p, _)| h.name == *name && p == path)
    }

    fn set_diff(&mut self, (lines, added): (Vec<String>, Option<u64>)) {
        if let (Some(open), Some(added)) = (&self.open, added)
            && let Some(f) = self.files.iter_mut().find(|f| f.path == *open)
        {
            f.added = Some(added);
        }
        self.lines = lines.into_iter().map(SharedString::from).collect();
    }

    /// A click on file `path`: shows its diff, or hides it if it's on show.
    fn toggle_file(&mut self, path: String, cx: &mut Context<Self>) {
        if self.open.as_ref() == Some(&path) {
            self.open = None;
            self.lines.clear();
            cx.notify();
        } else {
            self.open_file(path, cx);
        }
    }

    /// Shows file `path`'s diff.
    fn open_file(&mut self, path: String, cx: &mut Context<Self>) {
        let Some((host, root, _)) = self.place.clone() else {
            return;
        };
        let Some(file) = self.files.iter().find(|f| f.path == path).cloned() else {
            return;
        };
        self.open = Some(path.clone());
        self.lines = vec!["Loading…".into()];
        cx.notify();
        let asked = (host.name.clone(), root.clone());
        self._diff = Some(cx.spawn(async move |this, cx| {
            let diff = cx
                .background_spawn(async move { diff_of(&host, &root, &file) })
                .await;
            this.update(cx, |this, cx| {
                // Still the file shown, in the worktree shown.
                if this.shows(&asked) && this.open.as_ref() == Some(&path) {
                    this.set_diff(diff);
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// Commits every change listed, with the message typed.
    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let message = self.message.read(cx).value().trim().to_string();
        let Some((host, path, label)) = self.place.clone() else {
            return;
        };
        if message.is_empty()
            || self.files.is_empty()
            || self.committing.is_some()
            || self.in_conflict() > 0
        {
            return;
        }
        let files = to_add(&self.files);
        let asked = (host.name.clone(), path.clone());
        self.committing = Some(asked.clone());
        self.note = None;
        cx.notify();
        self._commit = Some(cx.spawn_in(window, async move |this, cx| {
            let typed = message.clone();
            let done = cx
                .background_spawn(async move { commit_in(&host, &path, &typed, &files) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.committing = None;
                // Another worktree may show by now: what's typed and shown
                // there is its own, and this says whose this was.
                let here = this.shows(&asked);
                let said = |text: String| {
                    if here {
                        text
                    } else {
                        format!("{label}: {text}")
                    }
                };
                match done {
                    Ok(summary) => {
                        this.note = Some((said(summary), false));
                        if here {
                            this.open = None;
                            this.lines.clear();
                            if this.message.read(cx).value().trim() == message {
                                this.message.update(cx, |m, cx| m.set_value("", window, cx));
                            }
                        }
                        cx.emit(Committed);
                    }
                    Err(e) => this.note = Some((said(format!("Commit failed: {e}")), true)),
                }
                this.refresh(cx);
                cx.notify();
            })
            .ok();
        }));
    }
}

impl Render for ChangesPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let title = match &self.place {
            Some((_, _, label)) => format!("Changes · {label}"),
            None => "Changes".to_string(),
        };
        let header = h_flex()
            .h_10()
            .flex_shrink_0()
            .px_3()
            .gap_2()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .font_semibold()
                    .child(title),
            )
            .when(!self.files.is_empty(), |h| {
                h.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(self.files.len().to_string()),
                )
            });
        let empty = match (&self.place, &self.error) {
            (None, _) => Some("Open a thread in a worktree to see its changes.".to_string()),
            (_, Some(e)) => Some(e.clone()),
            _ if self.files.is_empty() && self.loading => Some("Loading changes…".into()),
            _ if self.files.is_empty() => Some("No changes since the last commit.".into()),
            _ => None,
        };
        let files = self.files.iter().enumerate().map(|(i, f)| {
            let path = f.path.clone();
            let shown = self.open.as_deref() == Some(&f.path);
            let color = match f.status {
                'A' | '?' => theme.success,
                'D' | 'U' => theme.danger,
                _ => theme.warning,
            };
            let (dir, name) = match f.path.rsplit_once('/') {
                Some((dir, name)) => (format!("{dir}/"), name.to_string()),
                None => (String::new(), f.path.clone()),
            };
            h_flex()
                .id(SharedString::from(format!("change-{i}")))
                .debug_selector(move || format!("change {i}"))
                .px_2()
                .py_0p5()
                .gap_2()
                .items_center()
                .rounded(theme.radius)
                .text_xs()
                .cursor_pointer()
                .map(|row| {
                    if shown {
                        row.bg(theme.tokens.sidebar_accent)
                    } else {
                        row.hover(|row| row.bg(theme.sidebar_accent.opacity(0.5)))
                    }
                })
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_file(path.clone(), cx)))
                .child(div().w_3().text_color(color).child(f.status.to_string()))
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_color(muted).truncate().child(dir))
                        .child(div().flex_shrink_0().child(name)),
                )
                .children(
                    f.added
                        .map(|n| div().text_color(theme.success).child(format!("+{n}"))),
                )
                .children(
                    f.removed
                        .map(|n| div().text_color(theme.danger).child(format!("-{n}"))),
                )
        });
        let list = v_flex()
            .id("changes-files")
            .max_h(relative(0.4))
            .flex_shrink_0()
            .px_2()
            .gap_0p5()
            .overflow_y_scroll()
            .children(files)
            .children(empty.map(|e| div().p_1().text_sm().text_color(muted).child(e)));
        let diff = (!self.lines.is_empty()).then(|| {
            v_flex()
                .flex_1()
                .min_h_0()
                .border_t_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .h_7()
                        .flex_shrink_0()
                        .px_2()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(muted)
                                .child(self.open.clone().unwrap_or_default()),
                        )
                        .child(
                            div().debug_selector(|| "changes-preview".into()).child(
                                Button::new("changes-preview")
                                    .icon(gpui_kit::assets::IconName::Maximize)
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Show it over the window")
                                    .on_click(cx.listener(|_, _, _, cx| cx.emit(Preview))),
                            ),
                        ),
                )
                .child(
                    diff_list("changes-diff", self.lines.clone(), &self.diff_scroll, cx)
                        .flex_1()
                        .min_h_0(),
                )
        });
        let can_commit = !self.files.is_empty()
            && self.committing.is_none()
            && self.in_conflict() == 0
            && !self.message.read(cx).value().trim().is_empty();
        let commit = v_flex()
            .flex_shrink_0()
            .p_2()
            .gap_1()
            .border_t_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.message).small()))
                    .child(
                        div().debug_selector(|| "commit".into()).child(
                            Button::new("commit")
                                .small()
                                .primary()
                                .label(if self.committing.is_some() {
                                    "Committing…"
                                } else {
                                    "Commit"
                                })
                                .disabled(!can_commit)
                                .tooltip("Commits every change listed")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.commit(window, cx)),
                                ),
                        ),
                    ),
            )
            .when(self.in_conflict() > 0, |commit| {
                commit.child(
                    div()
                        .debug_selector(|| "changes-conflict".into())
                        .text_xs()
                        .text_color(theme.danger)
                        .child(match self.in_conflict() {
                            1 => "A file is in conflict: resolve it to commit.".to_string(),
                            n => format!("{n} files are in conflict: resolve them to commit."),
                        }),
                )
            })
            .children(self.note.as_ref().map(|(note, failed)| {
                div()
                    .text_xs()
                    .text_color(if *failed { theme.danger } else { muted })
                    .child(note.clone())
            }));
        v_flex()
            .size_full()
            .child(header)
            .child(list)
            .when_some(diff, |panel, diff| panel.child(diff))
            .when(self.lines.is_empty(), |panel| panel.child(div().flex_1()))
            .when(self.place.is_some(), |panel| panel.child(commit))
    }
}

/// A diff's lines, scrolling both ways: across as far as the longest goes.
pub(super) fn diff_list(
    id: &'static str,
    lines: Vec<SharedString>,
    scroll: &UniformListScrollHandle,
    cx: &App,
) -> UniformList {
    let theme = cx.theme();
    let font = Prefs::get(cx).terminal.font_family.clone();
    let (added, removed, hunk) = (
        theme.success.opacity(0.15),
        theme.danger.opacity(0.15),
        theme.info,
    );
    let longest = lines
        .iter()
        .enumerate()
        .max_by_key(|(_, line)| line.len())
        .map(|(i, _)| i);
    let count = lines.len();
    let row_font = font.clone();
    uniform_list(id, count, move |range, _, _| {
        range
            .map(|i| {
                let line = lines[i].clone();
                // Its own size and font: the list measures a line
                // apart from the panel's text style.
                div()
                    .w_full()
                    .h(px(18.0))
                    .flex()
                    .items_center()
                    .px_2()
                    .text_xs()
                    .font_family(row_font.clone())
                    .whitespace_nowrap()
                    .map(|row| match line.chars().next() {
                        Some('+') => row.bg(added),
                        Some('-') => row.bg(removed),
                        Some('@') => row.text_color(hunk),
                        _ => row,
                    })
                    .child(if line.is_empty() {
                        SharedString::from(" ")
                    } else {
                        line
                    })
            })
            .collect()
    })
    .with_width_from_item(longest)
    .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
    .track_scroll(scroll)
    .debug_selector(move || id.to_string())
    .font_family(font)
    .text_xs()
}

/// The worktree's changes since its last commit, staged or not, new files
/// too, in git's order; other repositories inside it aren't its changes.
/// Without writing the index, which would hold its lock: an agent's `git
/// add` meanwhile would fail.
fn changes_in(host: &Host, path: &str) -> Result<Vec<Change>, String> {
    let script = format!(
        "cd {} 2>/dev/null || {{ echo '@@error There is no such folder'; exit 0; }}\n\
         git rev-parse --show-toplevel >/dev/null 2>&1 || {{ echo '@@error Not a git repository'; exit 0; }}\n\
         echo @@status\n\
         git --no-optional-locks status --porcelain=v1 -z --no-renames --untracked-files=all\n\
         echo; echo @@numstat\n\
         git -c diff.autoRefreshIndex=false diff HEAD --numstat -z --no-renames 2>/dev/null || git -c diff.autoRefreshIndex=false diff --cached --numstat -z --no-renames\n\
         echo; echo @@end\n",
        sh_path(path)
    );
    let out = Remote::new(host)
        .run(&script)
        .map_err(|e| format!("{e:#}"))?;
    parse_changes(&out)
}

/// What the script printed, as changes.
fn parse_changes(out: &str) -> Result<Vec<Change>, String> {
    if let Some(e) = out.lines().find_map(|l| l.strip_prefix("@@error ")) {
        return Err(e.to_string());
    }
    let section = |from: &str, to: &str| -> Option<String> {
        let start = out.find(from)? + from.len();
        let end = out[start..].find(to)? + start;
        Some(out[start..end].trim_matches('\n').to_string())
    };
    let status = section("@@status\n", "@@numstat").ok_or("git didn't answer")?;
    let numstat = section("@@numstat\n", "@@end").unwrap_or_default();
    let mut counts: HashMap<String, (Option<u64>, Option<u64>)> = HashMap::new();
    for record in numstat.split('\0') {
        let mut fields = record.trim_start_matches('\n').splitn(3, '\t');
        if let (Some(a), Some(d), Some(path)) = (fields.next(), fields.next(), fields.next()) {
            counts.insert(path.to_string(), (a.parse().ok(), d.parse().ok()));
        }
    }
    Ok(status
        .split('\0')
        .filter(|e| e.len() > 3)
        // Listing every untracked file, git names a folder only when it's
        // another repository: a worktree kept inside this one, or a clone.
        // Adding it would commit a link to it, not its files.
        .filter(|e| !(e.starts_with("?? ") && e.ends_with('/')))
        .map(|entry| {
            let (xy, path) = (&entry[..2], entry[3..].to_string());
            let status = if xy == "??" {
                '?'
            } else if xy.contains('U') || xy == "AA" || xy == "DD" {
                'U'
            } else if xy.contains('D') {
                'D'
            } else if xy.contains('A') {
                'A'
            } else {
                'M'
            };
            let (added, removed) = counts.get(&path).copied().unwrap_or((None, None));
            Change {
                path,
                status,
                unstaged: xy == "??" || &xy[1..] != " ",
                added,
                removed,
            }
        })
        .collect())
}

/// `file`'s diff from the last commit, a line each, and for a new file its
/// lines counted; a note where there's no text to show.
fn diff_of(host: &Host, root: &str, file: &Change) -> (Vec<String>, Option<u64>) {
    let path = quote(&file.path);
    let script = if file.status == '?' {
        format!(
            "cd {} || exit 0\nif grep -Iq . -- {path} 2>/dev/null || ! [ -s {path} ]; then head -c 400000 -- {path}; else echo @@binary; fi\n",
            sh_path(root)
        )
    } else {
        format!(
            "cd {} || exit 0\ngit --literal-pathspecs -c diff.autoRefreshIndex=false diff HEAD --no-ext-diff --no-textconv --no-color -- {path} 2>/dev/null || git --literal-pathspecs -c diff.autoRefreshIndex=false diff --cached --no-color -- {path}\n",
            sh_path(root)
        )
    };
    let out = match Remote::new(host).run(&script) {
        Ok(out) => out,
        Err(e) => return (vec![format!("Couldn't read it: {e:#}")], None),
    };
    diff_lines(&out, file.status == '?')
}

/// The lines to show of a diff, or of a new file (all added), and how many
/// a new file has.
fn diff_lines(out: &str, new: bool) -> (Vec<String>, Option<u64>) {
    if out.trim_end() == "@@binary" {
        return (vec!["A binary file: no text to show".into()], None);
    }
    if new {
        let lines: Vec<&str> = out.strip_suffix('\n').unwrap_or(out).split('\n').collect();
        let count = if out.is_empty() {
            0
        } else {
            lines.len() as u64
        };
        let mut shown = vec![format!("@@ -0,0 +1,{count} @@")];
        shown.extend(lines.iter().take(MOST_LINES).map(|l| format!("+{l}")));
        return (shown, Some(count));
    }
    let lines: Vec<String> = out
        .lines()
        .skip_while(|l| !l.starts_with("@@"))
        .take(MOST_LINES)
        .map(String::from)
        .collect();
    if lines.is_empty() {
        let note = if out.contains("Binary files") {
            "A binary file changed"
        } else {
            "No text diff to show"
        };
        return (vec![note.into()], None);
    }
    (lines, None)
}

/// The files Commit adds before committing: those with changes not staged.
/// What's staged as it is, a removal or a rename too, `git add` may not
/// even name any more. A file removed from git but kept on disk lists twice,
/// removed and new: adding it would undo the removal.
fn to_add(files: &[Change]) -> Vec<String> {
    let removed: HashSet<&str> = files
        .iter()
        .filter(|f| f.status == 'D' && !f.unstaged)
        .map(|f| f.path.as_str())
        .collect();
    files
        .iter()
        .filter(|f| f.unstaged && !removed.contains(f.path.as_str()))
        .map(|f| f.path.clone())
        .collect()
}

/// Commits what's staged in the worktree with `message`, after adding
/// `files` (those with changes not staged) as they are now; what git said
/// of it.
fn commit_in(host: &Host, path: &str, message: &str, files: &[String]) -> Result<String, String> {
    // Through `xargs`, as many as there are: on the command line, some
    // 15,000 paths are too many, and only git 2.25 on reads them from stdin.
    // Literal, so a file named like `:(glob)*` is that file; each escaped,
    // so no name ends the list early.
    let add = if files.is_empty() {
        String::new()
    } else {
        let listed: String = files
            .iter()
            .map(|f| format!("{}\n", xargs_word(f)))
            .collect();
        format!(
            "xargs git --literal-pathspecs add -A -- <<'LEMUR_PATHS' || exit 1\n{listed}LEMUR_PATHS\n"
        )
    };
    let script = format!(
        "cd {} || exit 1\n{add}git commit -q -m {} && git log -1 --format='Committed %h: %s'\n",
        sh_path(path),
        quote(message)
    );
    Remote::new(host)
        .run(&script)
        .map(|out| out.trim().to_string())
        .map_err(|e| format!("{e:#}"))
}

/// `path` as `xargs` reads one word: a backslash before all it could take
/// for something else, a newline too.
fn xargs_word(path: &str) -> String {
    let mut word = String::new();
    for c in path.chars() {
        if c.is_ascii() && !c.is_ascii_alphanumeric() {
            word.push('\\');
        }
        word.push(c);
    }
    word
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's `test` would stand in for the plain one.
    use super::{changes_in, commit_in, diff_lines, diff_of, parse_changes, to_add};
    use crate::test_support::{TempDir, git_in};

    #[test]
    fn changes_are_read_from_git() {
        let out = "@@status\n M src/a.rs\0A  b.rs\0?? new.txt\0?? .claude/worktrees/x/\0 D gone.rs\0UU both.rs\0\n@@numstat\n\
                   3\t1\tsrc/a.rs\x002\t0\tb.rs\0-\t-\tlogo.png\0\n@@end\n";
        let changes = parse_changes(out).unwrap();
        let brief: Vec<(&str, char, Option<u64>, Option<u64>)> = changes
            .iter()
            .map(|c| (c.path.as_str(), c.status, c.added, c.removed))
            .collect();
        assert_eq!(
            brief,
            [
                ("src/a.rs", 'M', Some(3), Some(1)),
                ("b.rs", 'A', Some(2), Some(0)),
                ("new.txt", '?', None, None),
                ("gone.rs", 'D', None, None),
                ("both.rs", 'U', None, None),
            ]
        );
        assert_eq!(
            parse_changes("@@error Not a git repository\n"),
            Err("Not a git repository".into())
        );
        assert!(
            parse_changes("@@status\n\n@@numstat\n\n@@end\n")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn diffs_start_at_their_first_hunk() {
        let out = "diff --git a/x b/x\nindex 1..2\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-one\n+two\n";
        assert_eq!(diff_lines(out, false).0, ["@@ -1 +1 @@", "-one", "+two"]);
        let (lines, count) = diff_lines("a\nb\n", true);
        assert_eq!(
            (lines, count),
            (
                vec!["@@ -0,0 +1,2 @@".to_string(), "+a".into(), "+b".into()],
                Some(2)
            )
        );
        assert_eq!(
            diff_lines("@@binary\n", true).0,
            ["A binary file: no text to show"]
        );
        assert_eq!(
            diff_lines("Binary files a/x and b/x differ\n", false).0,
            ["A binary file changed"]
        );
    }

    /// Switched to another worktree while it asks the first, the panel
    /// lists the second's changes, not the first's; and a click on a file
    /// while it asks again doesn't stop it asking from then on.
    #[gpui::test]
    fn the_panel_lists_the_worktree_it_shows(cx: &mut gpui::TestAppContext) {
        use super::ChangesPanel;
        use crate::test_support::wait_until;
        use gpui_kit::AppContext;
        use std::time::Duration;
        let server = crate::test_support::HcServer::new("changes-switch");
        let host = server.config().hosts.remove(0);
        let dir = TempDir::new("changes-switch");
        let repo = dir.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git_in(&repo, &["add", "a.txt"]);
        git_in(&repo, &["commit", "-q", "-m", "init"]);
        git_in(&repo, &["worktree", "add", "-q", "-b", "b", "../b"]);
        std::fs::write(repo.join("a.txt"), "two\n").unwrap();
        let b = dir.join("b");
        std::fs::write(b.join("b.txt"), "new\n").unwrap();
        let (a, b_path) = (
            repo.to_string_lossy().into_owned(),
            b.to_string_lossy().into_owned(),
        );
        cx.executor().allow_parking();
        cx.update(gpui_kit::init);
        let mut panel = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| ChangesPanel::new(window, cx));
            panel = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let panel = panel.unwrap();
        let listed = |cx: &mut gpui::VisualTestContext| {
            panel.read_with(cx, |p, _| (p.loading, p.listed().0))
        };
        panel.update(cx, |p, cx| {
            p.show(host.clone(), a.clone(), "main".into(), cx);
            p.show(host.clone(), b_path.clone(), "b".into(), cx);
        });
        let settled = wait_until(Duration::from_secs(10), || {
            cx.run_until_parked();
            let (loading, files) = listed(cx);
            !loading && !files.is_empty()
        });
        assert!(settled, "{:?}", listed(cx));
        assert_eq!(listed(cx).1, [("b.txt".to_string(), '?')]);

        panel.update(cx, |p, cx| {
            p.refresh(cx);
            p.open_file("b.txt".into(), cx);
        });
        std::fs::write(b.join("c.txt"), "more\n").unwrap();
        let again = wait_until(Duration::from_secs(10), || {
            panel.update(cx, |p, cx| p.refresh(cx));
            cx.run_until_parked();
            listed(cx).1.len() == 2
        });
        assert!(again, "{:?}", listed(cx));
    }

    /// A window with just the panel, on the repository at `repo`.
    fn panel_on<'a>(
        repo: &std::path::Path,
        host: crate::config::Host,
        cx: &'a mut gpui::TestAppContext,
    ) -> (
        gpui_kit::Entity<super::ChangesPanel>,
        &'a mut gpui::VisualTestContext,
    ) {
        use gpui_kit::AppContext;
        let path = repo.to_string_lossy().into_owned();
        cx.executor().allow_parking();
        cx.update(gpui_kit::init);
        let mut panel = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| super::ChangesPanel::new(window, cx));
            panel = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let panel = panel.unwrap();
        panel.update(cx, |p, cx| p.show(host, path, "repo".into(), cx));
        (panel, cx)
    }

    /// Waits until the panel has answered, and no commit is under way.
    fn settled(panel: &gpui_kit::Entity<super::ChangesPanel>, cx: &mut gpui::VisualTestContext) {
        let done = crate::test_support::wait_until(std::time::Duration::from_secs(20), || {
            cx.run_until_parked();
            panel.read_with(cx, |p, _| !p.loading && p.committing.is_none())
        });
        assert!(done, "the panel settles");
    }

    /// A click on a file while a commit goes in leaves the commit to finish:
    /// the panel doesn't stay on "Committing…" for good.
    #[gpui::test]
    fn a_click_during_a_commit_leaves_it_to_finish(cx: &mut gpui::TestAppContext) {
        let server = crate::test_support::HcServer::new("changes-click");
        let host = server.config().hosts.remove(0);
        let dir = TempDir::new("changes-click");
        let repo = dir.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        let (panel, cx) = panel_on(&repo, host, cx);
        settled(&panel, cx);
        cx.update(|window, cx| {
            panel.update(cx, |p, cx| {
                p.message.update(cx, |m, cx| m.set_value("Add", window, cx));
                p.commit(window, cx);
                p.open_file("a.txt".into(), cx);
            })
        });
        settled(&panel, cx);
        panel.read_with(cx, |p, cx| {
            assert!(p.committing.is_none());
            assert!(p.files.is_empty(), "{:?}", p.files);
            assert_eq!(p.message.read(cx).value().to_string(), "", "it's in");
            let note = p.note.as_ref().expect("what it did");
            assert!(note.0.ends_with("Add"), "{note:?}");
        });
    }

    /// A commit's answer goes to its own worktree's: another shown meanwhile
    /// lists its changes, and keeps what's typed and shown there.
    #[gpui::test]
    fn a_commit_finishing_leaves_another_worktree_alone(cx: &mut gpui::TestAppContext) {
        let server = crate::test_support::HcServer::new("changes-other");
        let host = server.config().hosts.remove(0);
        let dir = TempDir::new("changes-other");
        let (repo, other) = (dir.join("repo"), dir.join("other"));
        for r in [&repo, &other] {
            std::fs::create_dir(r).unwrap();
            git_in(r, &["init", "-q", "-b", "main"]);
            std::fs::write(r.join("a.txt"), "one\n").unwrap();
        }
        let (panel, cx) = panel_on(&repo, host.clone(), cx);
        settled(&panel, cx);
        let other = other.to_string_lossy().into_owned();
        cx.update(|window, cx| {
            panel.update(cx, |p, cx| {
                p.message.update(cx, |m, cx| m.set_value("Add", window, cx));
                p.commit(window, cx);
                p.show(host, other.clone(), "other".into(), cx);
                assert!(p.loading, "it asks the other, as the commit goes in");
                p.message
                    .update(cx, |m, cx| m.set_value("For the other", window, cx));
            })
        });
        settled(&panel, cx);
        panel.read_with(cx, |p, cx| {
            let note = p.note.as_ref().expect("what it did");
            assert!(note.0.starts_with("repo: Committed "), "{note:?}");
            assert_eq!(p.message.read(cx).value().to_string(), "For the other");
            assert_eq!(p.listed().0, [("a.txt".to_string(), '?')]);
        });
    }

    /// With a file in conflict, Enter doesn't commit its markers, and the
    /// panel says why.
    #[gpui::test]
    fn files_in_conflict_arent_committed(cx: &mut gpui::TestAppContext) {
        let server = crate::test_support::HcServer::new("changes-conflict");
        let host = server.config().hosts.remove(0);
        let dir = TempDir::new("changes-conflict");
        let repo = dir.join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| git_in(&repo, args);
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        git(&["checkout", "-q", "-b", "other"]);
        std::fs::write(repo.join("a.txt"), "theirs\n").unwrap();
        git(&["commit", "-q", "-am", "theirs"]);
        git(&["checkout", "-q", "main"]);
        std::fs::write(repo.join("a.txt"), "ours\n").unwrap();
        git(&["commit", "-q", "-am", "ours"]);
        let merged = std::process::Command::new("git")
            .args(["merge", "other"])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(!merged.status.success(), "it conflicts");
        let head = || {
            let out = std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&repo)
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        let before = head();
        let (panel, cx) = panel_on(&repo, host, cx);
        settled(&panel, cx);
        panel.read_with(cx, |p, _| assert_eq!(p.in_conflict(), 1));
        assert!(cx.debug_bounds("changes-conflict").is_some(), "it says so");
        cx.update(|window, cx| {
            panel.update(cx, |p, cx| {
                p.message
                    .update(cx, |m, cx| m.set_value("Merge", window, cx));
                p.commit(window, cx);
            })
        });
        settled(&panel, cx);
        assert_eq!(head(), before, "nothing went in");
    }

    /// A click on a file shows its diff, and another hides it, for good even
    /// with an ask for that diff under way; one on another file moves it.
    #[gpui::test]
    fn a_click_on_a_file_shows_its_diff_and_another_hides_it(cx: &mut gpui::TestAppContext) {
        use super::ChangesPanel;
        use crate::test_support::wait_until;
        use gpui_kit::{AppContext, Modifiers};
        use std::time::Duration;
        let server = crate::test_support::HcServer::new("changes-toggle");
        let host = server.config().hosts.remove(0);
        let dir = TempDir::new("changes-toggle");
        let repo = dir.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        std::fs::write(repo.join("b.txt"), "one\n").unwrap();
        git_in(&repo, &["add", "a.txt", "b.txt"]);
        git_in(&repo, &["commit", "-q", "-m", "init"]);
        std::fs::write(repo.join("a.txt"), "two\n").unwrap();
        std::fs::write(repo.join("b.txt"), "three\n").unwrap();
        let path = repo.to_string_lossy().into_owned();
        cx.executor().allow_parking();
        cx.update(gpui_kit::init);
        let mut panel = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| ChangesPanel::new(window, cx));
            panel = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let panel = panel.unwrap();
        panel.update(cx, |p, cx| p.show(host, path, "main".into(), cx));
        let settled = |cx: &mut gpui::VisualTestContext| {
            wait_until(Duration::from_secs(10), || {
                cx.run_until_parked();
                panel.read_with(cx, |p, _| !p.loading && !p.files.is_empty())
            })
        };
        assert!(settled(cx));
        let files: Vec<String> =
            panel.read_with(cx, |p, _| p.listed().0.into_iter().map(|f| f.0).collect());
        assert_eq!(files, ["a.txt", "b.txt"]);

        let lines = |cx: &mut gpui::VisualTestContext| panel.read_with(cx, |p, _| p.listed().1);
        let click = |selector: &'static str, cx: &mut gpui::VisualTestContext| {
            let at = cx.debug_bounds(selector).expect(selector).center();
            cx.simulate_click(at, Modifiers::none());
        };
        let shows = |line: &str, cx: &mut gpui::VisualTestContext| {
            wait_until(Duration::from_secs(10), || {
                cx.run_until_parked();
                lines(cx).iter().any(|l| l == line)
            })
        };

        click("change 0", cx);
        assert!(shows("+two", cx), "{:?}", lines(cx));
        click("change 0", cx);
        assert!(lines(cx).is_empty(), "hidden: {:?}", lines(cx));
        click("change 0", cx);
        assert!(shows("+two", cx), "shown again: {:?}", lines(cx));

        click("change 1", cx);
        assert!(shows("+three", cx), "{:?}", lines(cx));
        assert!(!lines(cx).iter().any(|l| l == "+two"), "{:?}", lines(cx));

        panel.update(cx, |p, cx| {
            p.refresh(cx);
            p.toggle_file("b.txt".into(), cx);
        });
        assert!(settled(cx));
        assert!(lines(cx).is_empty(), "came back: {:?}", lines(cx));
    }

    /// What's staged commits as staged: a removal (of an ignored file too)
    /// and a rename, which `git add` can't name any more. What isn't is
    /// added first, whatever its name.
    #[test]
    fn staged_changes_commit_as_staged() {
        let server = crate::test_support::HcServer::new("changes-staged");
        let host = server.config().hosts.remove(0);
        let dir = TempDir::new("changes-staged");
        let repo = dir.join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| git_in(&repo, args);
        git(&["init", "-q", "-b", "main"]);
        for f in ["a.txt", "b.txt", "c.txt", "kept.log"] {
            std::fs::write(repo.join(f), format!("{f}\n")).unwrap();
        }
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        std::fs::write(repo.join(".gitignore"), "*.log\n").unwrap();
        git(&["rm", "-q", "b.txt"]);
        git(&["mv", "c.txt", "e.txt"]);
        git(&["rm", "-q", "--cached", "kept.log"]);
        std::fs::write(repo.join("a.txt"), "staged\n").unwrap();
        git(&["add", "a.txt"]);
        std::fs::write(repo.join("a.txt"), "and more\n").unwrap();
        for name in [
            "new\nline.txt",
            "say \"hi\".txt",
            "back\\slash.txt",
            "it's.txt",
            " spaced .txt",
            "-dash.txt",
            ":(glob)*.txt",
            "æøå.txt",
            "_",
        ] {
            std::fs::write(repo.join(name), "x\n").unwrap();
        }
        let root = repo.to_string_lossy().into_owned();

        let files = to_add(&changes_in(&host, &root).unwrap());
        let said = commit_in(&host, &root, "Tidy", &files).unwrap();
        assert!(said.ends_with("Tidy"), "{said}");
        let tree = std::process::Command::new("git")
            .args(["ls-tree", "-r", "--name-only", "-z", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap();
        let mut tree: Vec<String> = String::from_utf8_lossy(&tree.stdout)
            .split('\0')
            .filter(|f| !f.is_empty())
            .map(String::from)
            .collect();
        tree.sort();
        assert_eq!(
            tree,
            [
                " spaced .txt",
                "-dash.txt",
                ".gitignore",
                ":(glob)*.txt",
                "_",
                "a.txt",
                "back\\slash.txt",
                "e.txt",
                "it's.txt",
                "new\nline.txt",
                "say \"hi\".txt",
                "æøå.txt"
            ]
        );
        let shown = std::process::Command::new("git")
            .args(["show", "HEAD:a.txt"])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&shown.stdout), "and more\n");
        assert!(changes_in(&host, &root).unwrap().is_empty());
    }

    /// A file removed from git but kept on disk is committed as removed,
    /// and goes on listed as new; with nothing else staged, too.
    #[test]
    fn a_file_kept_on_disk_commits_as_removed() {
        let server = crate::test_support::HcServer::new("changes-rm");
        let host = server.config().hosts.remove(0);
        let dir = TempDir::new("changes-rm-cached");
        let repo = dir.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("secrets.yml"), "x\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "init"]);
        git_in(&repo, &["rm", "-q", "--cached", "secrets.yml"]);
        let root = repo.to_string_lossy().into_owned();

        let files = to_add(&changes_in(&host, &root).unwrap());
        assert!(files.is_empty(), "{files:?}");
        let said = commit_in(&host, &root, "Forget it", &files).unwrap();
        assert!(said.ends_with("Forget it"), "{said}");
        let tree = std::process::Command::new("git")
            .args(["ls-tree", "-r", "--name-only", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&tree.stdout), "");
        let left: Vec<(String, char)> = changes_in(&host, &root)
            .unwrap()
            .into_iter()
            .map(|c| (c.path, c.status))
            .collect();
        assert_eq!(left, [("secrets.yml".to_string(), '?')]);
    }

    /// Listing changes and showing a diff leave the index as it is, a file
    /// touched but not changed too: its lock is the one an agent's `git
    /// add` takes.
    #[test]
    fn listing_changes_leaves_the_index_alone() {
        let server = crate::test_support::HcServer::new("changes-no-lock");
        let host = server.config().hosts.remove(0);
        let dir = TempDir::new("changes-no-lock");
        let repo = dir.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        std::fs::write(repo.join("b.txt"), "one\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "init"]);
        std::fs::write(repo.join("b.txt"), "two\n").unwrap();
        let index = repo.join(".git/index");
        let written = || std::fs::metadata(&index).unwrap().modified().unwrap();
        let before = written();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::File::options()
            .write(true)
            .open(repo.join("a.txt"))
            .unwrap()
            .set_modified(std::time::SystemTime::now())
            .unwrap();
        let root = repo.to_string_lossy().into_owned();
        let changes = changes_in(&host, &root).unwrap();
        assert_eq!(changes.len(), 1);
        diff_of(&host, &root, &changes[0]);
        assert_eq!(written(), before, "the index was written");
    }

    /// A file's diff is that file's, though its name reads as a pattern
    /// (`[id]`) that another changed file matches too.
    #[test]
    fn a_diff_is_of_the_file_named() {
        let server = crate::test_support::HcServer::new("changes-literal");
        let host = server.config().hosts.remove(0);
        let dir = TempDir::new("changes-literal");
        let repo = dir.join("repo");
        for d in ["app/[id]", "app/d"] {
            std::fs::create_dir_all(repo.join(d)).unwrap();
            std::fs::write(repo.join(d).join("page.tsx"), "one\n").unwrap();
        }
        git_in(&repo, &["init", "-q", "-b", "main"]);
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "init"]);
        for d in ["app/[id]", "app/d"] {
            std::fs::write(repo.join(d).join("page.tsx"), format!("{d}\n")).unwrap();
        }
        let root = repo.to_string_lossy().into_owned();
        let changes = changes_in(&host, &root).unwrap();
        let id = changes.iter().find(|c| c.path.contains("[id]")).unwrap();
        assert_eq!(
            diff_of(&host, &root, id).0,
            ["@@ -1 +1 @@", "-one", "+app/[id]"]
        );
    }

    /// More new files than a command line holds commit too.
    #[test]
    fn thousands_of_files_commit() {
        let server = crate::test_support::HcServer::new("changes-many");
        let host = server.config().hosts.remove(0);
        let dir = TempDir::new("changes-many");
        let repo = dir.join("repo");
        // Long paths, so fewer files make about 1.3 MB of them: this Mac
        // takes 1 MB of arguments.
        let deep = repo.join(format!("vendor/{}", "package-".repeat(25)));
        std::fs::create_dir_all(&deep).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        for i in 0..6_000 {
            std::fs::write(deep.join(format!("file-{i:05}.txt")), "").unwrap();
        }
        let root = repo.to_string_lossy().into_owned();
        let files: Vec<String> = changes_in(&host, &root)
            .unwrap()
            .into_iter()
            .map(|c| c.path)
            .collect();
        assert_eq!(files.len(), 6_000);
        let said = commit_in(&host, &root, "Vendor", &files).unwrap();
        assert!(said.ends_with("Vendor"), "{said}");
        assert!(changes_in(&host, &root).unwrap().is_empty());
    }

    /// In a real repository, on a test herdr-core: listed, diffed, then
    /// all committed with the message, quotes and all; a worktree kept
    /// inside it, and a clone, are neither.
    #[test]
    fn a_worktree_is_listed_diffed_and_committed() {
        let server = crate::test_support::HcServer::new("changes-panel");
        let host = server.config().hosts.remove(0);
        let dir = TempDir::new("changes-panel");
        let repo = dir.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
        git_in(&repo, &["add", "a.txt"]);
        git_in(&repo, &["commit", "-q", "-m", "init"]);
        git_in(&repo, &["worktree", "add", "-q", ".claude/worktrees/feat"]);
        std::fs::create_dir(repo.join("clone")).unwrap();
        git_in(&repo.join("clone"), &["init", "-q"]);
        std::fs::write(repo.join("clone/x.txt"), "x\n").unwrap();
        std::fs::write(repo.join("a.txt"), "one\n2\n").unwrap();
        std::fs::write(repo.join("it's new.txt"), "x\ny\nz\n").unwrap();
        let root = repo.to_string_lossy().into_owned();

        let changes = changes_in(&host, &root).unwrap();
        let brief: Vec<(&str, char)> = changes
            .iter()
            .map(|c| (c.path.as_str(), c.status))
            .collect();
        assert_eq!(brief, [("a.txt", 'M'), ("it's new.txt", '?')]);
        assert_eq!((changes[0].added, changes[0].removed), (Some(1), Some(1)));
        assert_eq!(
            diff_of(&host, &root, &changes[0]).0,
            ["@@ -1,2 +1,2 @@", " one", "-two", "+2"]
        );
        assert_eq!(diff_of(&host, &root, &changes[1]).1, Some(3));

        let files: Vec<String> = changes.iter().map(|c| c.path.clone()).collect();
        let said = commit_in(&host, &root, "Fix it's \"quotes\"", &files).unwrap();
        assert!(said.starts_with("Committed "), "{said}");
        assert!(said.ends_with("Fix it's \"quotes\""), "{said}");
        assert!(changes_in(&host, &root).unwrap().is_empty());
        let committed = std::process::Command::new("git")
            .args(["ls-tree", "-r", "--name-only", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&committed.stdout),
            "a.txt\nit's new.txt\n"
        );
        assert_eq!(
            changes_in(&host, &dir.to_string_lossy()),
            Err("Not a git repository".into())
        );
    }
}
