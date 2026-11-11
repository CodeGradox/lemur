//! The Changes panel, on the right as the notes and the agents are, as
//! Herdr's: the files changed in the worktree on show since its last
//! commit, each with its lines added and removed; a click shows its diff,
//! and another hides it, and it scrolls both ways. Its button shows it over
//! the window, nearly as wide and tall.
//! Under them, a message and Commit commit the files listed. Other
//! repositories inside the worktree, as worktrees kept in a project's
//! folder, are neither listed nor committed.

use super::{Lemur, Right, icons, worktree_label};
use crate::config::Host;
use crate::remote::{Remote, quote, sh_path};
use rune::{Cursor, El, ScrollSpec, TextEdit, col, row, text};
use rune_kit::diff::{self, DiffState, DiffView, FileDiff};
use rune_kit::{Button, ButtonVariant, Kit, TextInput};
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

#[derive(Default)]
pub struct ChangesPanel {
    /// The host and worktree shown, and what it's called.
    place: Option<super::panels::Place>,
    files: Vec<Change>,
    /// Why there are none to show, as outside a repository.
    error: Option<String>,
    loading: bool,
    /// The file whose diff shows; its diff as git printed it, and as it shows; or what
    /// shows instead while it's asked for, or when it couldn't be.
    open: Option<String>,
    patch: Option<String>,
    diff: DiffState,
    diff_note: Option<String>,
    /// Files opened so far: each starts its diff at the top, not where the last was scrolled.
    opened: u64,
    /// The diff shown over the window, with its own state, as it scrolls apart from the
    /// panel's.
    over: DiffState,
    over_shown: bool,
    message: TextEdit,
    /// The host and worktree a commit is going in, while it is.
    committing: Option<(String, String)>,
    /// What the last commit did, or why it didn't.
    note: Option<(String, bool)>,
    /// The panel's height, which the list takes at most 40% of while a diff shows.
    height: f32,
}

impl ChangesPanel {
    /// How many files are in conflict: committing them would commit their
    /// markers, and conclude a merge or a step of a rebase with them.
    fn in_conflict(&self) -> usize {
        self.files.iter().filter(|f| f.status == 'U').count()
    }

    /// Whether it shows worktree `on`, by its host and path; with None,
    /// nothing.
    pub(super) fn is_on(&self, on: Option<(&str, &str)>) -> bool {
        super::panels::place_is(self.place.as_ref(), on)
    }

    /// Whether worktree `path` on host `name` shows.
    fn shows(&self, (name, path): &(String, String)) -> bool {
        self.is_on(Some((name, path)))
    }

    /// No file's diff shows, here or over the window.
    fn hide_diff(&mut self) {
        self.open = None;
        self.patch = None;
        self.diff.set_files(vec![]);
        self.diff_note = None;
        self.over_shown = false;
        self.over.set_files(vec![]);
    }

    /// The open file's diff came back: `out` as git printed it, or why it couldn't.
    fn set_diff(&mut self, file: &Change, out: Result<String, String>) {
        match out {
            Err(e) => {
                self.patch = None;
                self.diff.set_files(vec![]);
                self.diff_note = Some(format!("Couldn't read it: {e}"));
            }
            // Asked again and the same: it stays as it's shown, folded or scrolled.
            Ok(out) if self.patch.as_ref() == Some(&out) => {}
            Ok(out) => {
                let shown = to_diff(file, &out);
                if self.over_shown {
                    self.over.set_files(vec![shown.clone()]);
                }
                self.diff.set_files(vec![shown]);
                self.patch = Some(out);
                self.diff_note = None;
            }
        }
        // A new file's lines are counted once it's read.
        let count = self.diff.files().first().filter(|d| file.status == '?' && !d.binary).map(|d| d.additions() as u64);
        if let (Some(count), Some(f)) = (count, self.files.iter_mut().find(|f| f.path == file.path)) {
            f.added = Some(count);
        }
    }

    /// The files listed and the open diff's lines, as a patch writes them, for tests.
    #[cfg(test)]
    pub(super) fn listed(&self) -> (Vec<(String, char)>, Vec<String>) {
        let files = self.files.iter().map(|f| (f.path.clone(), f.status)).collect();
        let lines = match (&self.diff_note, self.diff.files().first()) {
            (Some(note), _) => vec![note.clone()],
            (None, Some(d)) => lines_of(d),
            (None, None) => vec![],
        };
        (files, lines)
    }
}

impl Lemur {
    /// Points the Changes panel at the worktree on show, and asks it.
    pub(super) fn show_changes(&mut self) {
        let place = self.active_worktree().and_then(|(host, w)| Some((self.cfg.host(host)?.clone(), w.path.clone(), worktree_label(w))));
        match place {
            Some((host, path, label)) => self.show_worktree(host, path, label),
            None => self.clear_changes(),
        }
    }

    /// Asks the worktree shown again, every `changes::EVERY` while the panel shows.
    pub(super) fn refresh_changes(&mut self) {
        self.list_changes();
    }

    /// The panel goes by the thread on show (see `Lemur::follow_shown`): with none it says
    /// so, rather than showing, and committing in, the worktree of the one that was.
    pub(super) fn changes_follow_shown(&mut self) {
        if self.right() != Some(Right::Changes) {
            return;
        }
        let on = self.active.is_none();
        let place = self.active_worktree().map(|(h, w)| (h.to_string(), w.path.clone()));
        if on || place.is_some_and(|(h, p)| !self.panels.changes.is_on(Some((&h, &p)))) {
            self.show_changes();
        }
    }

    /// A file's diff shows over the window.
    pub(super) fn diff_overlaid(&self) -> bool {
        self.panels.changes.over_shown
    }

    /// Shows the panel's diff over the window.
    pub(super) fn open_diff_preview(&mut self) {
        let p = &mut self.panels.changes;
        if p.open.is_none() || p.diff.files().is_empty() {
            return;
        }
        p.over = DiffState::new(p.diff.files().to_vec());
        p.over_shown = true;
    }

    /// Closes the diff over the window; true if one showed.
    pub(super) fn close_diff_preview(&mut self) -> bool {
        let p = &mut self.panels.changes;
        let shown = p.over_shown;
        p.over_shown = false;
        p.over.set_files(vec![]);
        shown
    }

    /// A commit message has the keyboard. Rune keeps the field's `focused`, which stays set
    /// while the panel is hidden and the field isn't there to have it.
    pub(super) fn committing(&self) -> bool {
        let p = &self.panels.changes;
        self.right() == Some(Right::Changes) && p.place.is_some() && p.message.focused
    }

    /// Shows worktree `path` on `host`, called `label`, and asks it again.
    fn show_worktree(&mut self, host: Host, path: String, label: String) {
        let p = &mut self.panels.changes;
        let same = p.place.as_ref().is_some_and(|(h, at, _)| h.name == host.name && *at == path);
        if !same {
            p.files.clear();
            // A diff over the window goes now, not once this is asked.
            p.hide_diff();
            p.error = None;
            p.note = None;
            // Enter would commit this worktree's changes, not those of the one typed for.
            p.message.focused = false;
        }
        p.place = Some((host, path, label));
        self.list_changes();
    }

    /// Nothing on show to tell changes of.
    fn clear_changes(&mut self) {
        let p = &mut self.panels.changes;
        if p.place.is_none() {
            return;
        }
        p.place = None;
        p.files.clear();
        p.hide_diff();
        p.message.focused = false;
    }

    /// Asks the worktree for its changes, and the open file's diff.
    fn list_changes(&mut self) {
        let p = &mut self.panels.changes;
        let Some((host, path, _)) = p.place.clone() else { return };
        // A commit going in here changes what there is to list; it asks
        // again once it's in.
        let asked = (host.name.clone(), path.clone());
        if p.loading || p.committing.as_ref() == Some(&asked) {
            return;
        }
        p.loading = true;
        let open = p.open.clone();
        let diffed = open.clone();
        self.in_background(
            move || {
                let files = changes_in(&host, &path);
                let diff = match (&files, &open) {
                    (Ok(files), Some(open)) => files.iter().find(|f| f.path == *open).map(|f| (f.clone(), diff_of(&host, &path, f))),
                    _ => None,
                };
                (files, diff)
            },
            move |s, (files, diff)| {
                let p = &mut s.panels.changes;
                p.loading = false;
                // Another worktree shows now: its changes, not these.
                if !p.shows(&asked) {
                    s.list_changes();
                    return;
                }
                match files {
                    Ok(files) => {
                        p.error = None;
                        if p.open.as_ref().is_some_and(|o| !files.iter().any(|f| f.path == *o)) {
                            p.hide_diff();
                        }
                        p.files = files;
                    }
                    Err(e) => {
                        p.error = Some(e);
                        p.files.clear();
                        p.hide_diff();
                    }
                }
                // Still the file shown: it may have been hidden meanwhile.
                if let Some((file, out)) = diff.filter(|_| p.open == diffed) {
                    p.set_diff(&file, out);
                }
            },
        );
    }

    /// A click on file `path`: shows its diff, or hides it if it's on show.
    fn toggle_file(&mut self, path: String) {
        if self.panels.changes.open.as_ref() == Some(&path) {
            self.panels.changes.hide_diff();
        } else {
            self.open_file(path);
        }
    }

    /// Shows file `path`'s diff.
    fn open_file(&mut self, path: String) {
        let p = &mut self.panels.changes;
        let Some((host, root, _)) = p.place.clone() else { return };
        let Some(file) = p.files.iter().find(|f| f.path == path).cloned() else { return };
        p.hide_diff();
        p.open = Some(path.clone());
        p.opened += 1;
        p.diff_note = Some("Loading…".into());
        let asked = (host.name.clone(), root.clone());
        self.in_background(
            move || {
                let out = diff_of(&host, &root, &file);
                (file, out)
            },
            move |s, (file, out)| {
                let p = &mut s.panels.changes;
                // Still the file shown, in the worktree shown.
                if p.shows(&asked) && p.open.as_ref() == Some(&path) {
                    p.set_diff(&file, out);
                }
            },
        );
    }

    /// Commits every change listed, with the message typed.
    fn commit(&mut self) {
        let p = &mut self.panels.changes;
        let message = p.message.text.trim().to_string();
        let Some((host, path, label)) = p.place.clone() else { return };
        if message.is_empty() || p.files.is_empty() || p.committing.is_some() || p.in_conflict() > 0 {
            return;
        }
        let files = to_add(&p.files);
        let asked = (host.name.clone(), path.clone());
        p.committing = Some(asked.clone());
        p.note = None;
        let typed = message.clone();
        self.in_background(
            move || commit_in(&host, &path, &typed, &files),
            move |s, done| {
                let p = &mut s.panels.changes;
                p.committing = None;
                // Another worktree may show by now: what's typed and shown
                // there is its own, and this says whose this was.
                let here = p.shows(&asked);
                let said = |text: String| if here { text } else { format!("{label}: {text}") };
                match done {
                    Ok(summary) => {
                        p.note = Some((said(summary), false));
                        if here {
                            p.hide_diff();
                            if p.message.text.trim() == message {
                                p.message.clear();
                            }
                        }
                        // The status bar's count is out of date.
                        s.ask_changes();
                    }
                    Err(e) => p.note = Some((said(format!("Commit failed: {e}")), true)),
                }
                s.list_changes();
            },
        );
    }

    /// The panel's height as laid out.
    fn set_changes_height(&mut self, h: f32) {
        if (self.panels.changes.height - h).abs() > 0.5 {
            self.panels.changes.height = h;
        }
    }
}

/// A changed file's row: git's letter for the change, its folder and name, and its lines
/// added and removed. One still there, in worktree `root` on `host`, drags to other apps.
fn file_row(i: usize, f: &Change, shown: bool, (host, root): (&Host, &str), k: &Kit) -> El {
    let c = &k.c;
    let color = match f.status {
        'A' | '?' => c.success.fg,
        'D' | 'U' => c.danger.fg,
        _ => c.warning.fg,
    };
    let (dir, name) = match f.path.rsplit_once('/') {
        Some((dir, name)) => (format!("{dir}/"), name.to_string()),
        None => (String::new(), f.path.clone()),
    };
    let mut r = row()
        .key(format!("change-{i}"))
        .h(24.0)
        .px(8.0)
        .gap(8.0)
        .center_y()
        .radius(k.radius.sm)
        .cursor(Cursor::Pointer)
        .child(text(f.status.to_string()).font(k.ty.small_medium).color(color).w(10.0))
        .child(
            row()
                .grow(1.0)
                .min_w(0.0)
                .child(text(dir).font(k.ty.small).color(c.text_muted).truncate().shrink())
                .child(text(name).font(k.ty.small).color(c.text_base)),
        );
    if let Some(n) = f.added {
        r = r.child(text(format!("+{n}")).font(k.ty.small).color(c.success.fg));
    }
    if let Some(n) = f.removed {
        r = r.child(text(format!("-{n}")).font(k.ty.small).color(c.danger.fg));
    }
    let path = f.path.clone();
    let mut r = k.interactive(r).on_press(move |s: &mut Lemur| s.toggle_file(path.clone()));
    if f.status != 'D' {
        let file = format!("{}/{}", root.trim_end_matches('/'), f.path);
        let carried: super::carry::Carried = (host.name.clone(), None, file.clone());
        let (host, backing) = (host.clone(), c.bg_base);
        r = r
            .on_hover_change(move |s: &mut Lemur, inside, _| {
                if inside {
                    s.ready_to_drag(&host, None, file.clone());
                }
            })
            .drag_out(move |s: &Lemur| s.drag_item(&carried, backing));
    }
    if shown { r.fill(c.pressed) } else { r }
}

pub fn view(s: &Lemur, k: &Kit) -> El {
    let c = &k.c;
    let p = &s.panels.changes;
    let label = p.place.as_ref().map(|(_, _, label)| label.as_str());
    let header = super::panels::header("changes-header", None, "Changes", label, p.files.len(), k);
    let empty = match (&p.place, &p.error) {
        (None, _) => Some("Open a thread in a worktree to see its changes.".to_string()),
        (_, Some(e)) => Some(e.clone()),
        _ if p.files.is_empty() && p.loading => Some("Loading changes…".into()),
        _ if p.files.is_empty() => Some("No changes since the last commit.".into()),
        _ => None,
    };
    let rows = p.files.iter().enumerate().filter_map(|(i, f)| {
        let (host, root, _) = p.place.as_ref()?;
        Some(file_row(i, f, p.open.as_deref() == Some(&f.path), (host, root), k))
    });
    let mut list = col().key("changes-files").w_full().px(8.0).pb(4.0).gap(2.0).children(rows);
    if let Some(e) = empty {
        list = list.child(text(e).font(k.ty.small).color(c.text_muted).wrap().px(4.0).py(4.0));
    }
    let list = col().key("changes-list").w_full().min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(list);
    let mut panel = col()
        .key("changes-panel")
        .w_full()
        .h_full()
        .on_layout(|s: &mut Lemur, [_, _, _, h]| s.set_changes_height(h))
        .child(header);
    if p.open.is_some() {
        let list = if p.height > 0.0 { list.max_h(0.4 * p.height) } else { list };
        let body = match &p.diff_note {
            Some(note) => col().w_full().padding(12.0).child(text(note.clone()).font(k.ty.small).color(c.text_muted).wrap()),
            None => {
                let kc = k.clone();
                let diff = DiffView::new(|s: &Lemur| &s.panels.changes.diff, |s: &mut Lemur| &mut s.panels.changes.diff)
                    .key("changes-diff")
                    // In a row of its own: the header keys what it's given.
                    .header_actions(move |_: &Lemur, _| {
                        let button = icons::button(&icons::MAXIMIZE, "Show it over the window")
                            .on_press(|s: &mut Lemur| s.open_diff_preview())
                            .build(&kc)
                            .key("changes-preview");
                        row().child(button)
                    })
                    .build(s, k);
                // Its own key for each file opened, so another starts at its top.
                col().key(format!("changes-diff-{}", p.opened)).w_full().grow(1.0).min_h(0.0).child(diff)
            }
        };
        let diff = col()
            .key("changes-diff-area")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .border_sides([1.0, 0.0, 0.0, 0.0], c.border_weak)
            .child(body);
        panel = panel.child(list).child(diff);
    } else {
        panel = panel.child(list.grow(1.0));
    }
    if p.place.is_some() {
        panel = panel.child(commit_box(p, k));
    }
    panel
}

/// Under the list: the message, Commit, and what stops a commit or what the last one did.
fn commit_box(p: &ChangesPanel, k: &Kit) -> El {
    let c = &k.c;
    let can_commit = !p.files.is_empty() && p.committing.is_none() && p.in_conflict() == 0 && !p.message.text.trim().is_empty();
    let message = TextInput::new(&p.message)
        .placeholder("Commit message")
        .width(None)
        .on_edit(|s: &mut Lemur| &mut s.panels.changes.message)
        .on_submit(|s: &mut Lemur, _| s.commit())
        .build(k)
        .key("changes-message");
    let commit = Button::new(if p.committing.is_some() { "Committing…" } else { "Commit" })
        .variant(ButtonVariant::Contrast)
        .disabled(!can_commit)
        .help("Commits every change listed")
        .on_press(|s: &mut Lemur| s.commit())
        .build(k)
        .key("commit");
    let mut boxed = col()
        .key("changes-commit")
        .w_full()
        .padding(8.0)
        .gap(6.0)
        .border_sides([1.0, 0.0, 0.0, 0.0], c.border_weak)
        .child(row().w_full().gap(8.0).center_y().child(message.grow(1.0)).child(commit));
    let conflicts = p.in_conflict();
    if conflicts > 0 {
        let said = match conflicts {
            1 => "A file is in conflict: resolve it to commit.".to_string(),
            n => format!("{n} files are in conflict: resolve them to commit."),
        };
        boxed = boxed.child(text(said).key("changes-conflict").font(k.ty.small).color(c.danger.fg).wrap());
    }
    if let Some((note, failed)) = &p.note {
        let color = if *failed { c.danger.fg } else { c.text_muted };
        boxed = boxed.child(text(note.clone()).key("changes-note").font(k.ty.small).color(color).wrap());
    }
    boxed
}

/// The diff shown over the window, nearly as wide and tall: ×, ⌘W, Esc or a click outside
/// closes it.
pub fn diff_overlay(s: &Lemur, k: &Kit, layer: &mut Vec<El>) {
    let c = &k.c;
    if !s.panels.changes.over_shown {
        return;
    }
    let kc = k.clone();
    let diff = DiffView::new(|s: &Lemur| &s.panels.changes.over, |s: &mut Lemur| &mut s.panels.changes.over)
        .key("changes-diff-preview")
        .header_actions(move |_: &Lemur, _| {
            let button = icons::button(&icons::X, "Close")
                .on_press(|s: &mut Lemur| {
                    s.close_diff_preview();
                })
                .build(&kc)
                .key("diff-preview-close");
            row().child(button)
        })
        .build(s, k);
    let card = col()
        .key("diff-preview")
        // Under the row with the window's buttons, which show over anything.
        .pin(Some(16.0), Some(40.0), Some(16.0), Some(16.0))
        .radius(k.radius.lg)
        .border(1.0, c.border_base)
        .fill(c.bg_base)
        .shadow(12.0, 40.0, c.scrim)
        .clip_rounded(k.radius.lg)
        .child(diff);
    super::card::over_window(
        "diff-preview",
        card,
        |s: &mut Lemur| {
            s.close_diff_preview();
        },
        k,
        layer,
    );
}

/// `file`'s diff as `git diff` printed it, or a new file's text with every line added;
/// its first `MOST_LINES` lines.
fn to_diff(file: &Change, out: &str) -> FileDiff {
    let cut: String = out.split_inclusive('\n').take(MOST_LINES).collect();
    if file.status == '?' {
        let mut new = FileDiff::between(None, Some(file.path.clone()), "", if out.trim_end() == "@@binary" { "" } else { &cut });
        new.binary |= out.trim_end() == "@@binary";
        return new;
    }
    // Nothing git can show as text, as a submodule's new commit.
    let mut files = diff::parse(&cut);
    if files.is_empty() { FileDiff::from_texts(file.path.clone(), "", "") } else { files.remove(0) }
}

/// A diff's lines as a patch writes them, for tests.
#[cfg(test)]
fn lines_of(d: &FileDiff) -> Vec<String> {
    use rune_kit::diff::LineKind;
    let mut lines = vec![];
    for h in &d.hunks {
        lines.push(h.header());
        for l in &h.lines {
            let mark = match l.kind {
                LineKind::Added => '+',
                LineKind::Removed => '-',
                LineKind::Context => ' ',
            };
            lines.push(format!("{mark}{}", l.text));
        }
    }
    lines
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
    let out = Remote::new(host).run(&script).map_err(|e| format!("{e:#}"))?;
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
            Change { path, status, unstaged: xy == "??" || &xy[1..] != " ", added, removed }
        })
        .collect())
}

/// `file`'s diff from the last commit as git prints it, or a new file's text; `@@binary`
/// for a new file with no text to show.
fn diff_of(host: &Host, root: &str, file: &Change) -> Result<String, String> {
    let path = quote(&file.path);
    let script = if file.status == '?' {
        format!(
            "cd {} || exit 0\nif grep -Iq . -- {path} 2>/dev/null || ! [ -s {path} ]; then head -c 400000 -- {path}; else echo @@binary; fi\n",
            sh_path(root)
        )
    } else {
        format!(
            "cd {} || exit 0\ngit --literal-pathspecs -c diff.autoRefreshIndex=false diff HEAD --no-ext-diff --no-textconv --no-color -- {path} 2>/dev/null || git --literal-pathspecs -c diff.autoRefreshIndex=false diff --cached --no-ext-diff --no-textconv --no-color -- {path}\n",
            sh_path(root)
        )
    };
    Remote::new(host).run(&script).map_err(|e| format!("{e:#}"))
}

/// The files Commit adds before committing: those with changes not staged.
/// What's staged as it is, a removal or a rename too, `git add` may not
/// even name any more. A file removed from git but kept on disk lists twice,
/// removed and new: adding it would undo the removal.
fn to_add(files: &[Change]) -> Vec<String> {
    let removed: HashSet<&str> = files.iter().filter(|f| f.status == 'D' && !f.unstaged).map(|f| f.path.as_str()).collect();
    files.iter().filter(|f| f.unstaged && !removed.contains(f.path.as_str())).map(|f| f.path.clone()).collect()
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
        let listed: String = files.iter().map(|f| format!("{}\n", xargs_word(f))).collect();
        format!("xargs git --literal-pathspecs add -A -- <<'LEMUR_PATHS' || exit 1\n{listed}LEMUR_PATHS\n")
    };
    let script = format!(
        "cd {} || exit 1\n{add}git commit -q -m {} && git log -1 --format='Committed %h: %s'\n",
        sh_path(path),
        quote(message)
    );
    Remote::new(host).run(&script).map(|out| out.trim().to_string()).map_err(|e| format!("{e:#}"))
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
#[path = "changes_panel_tests.rs"]
mod tests;
