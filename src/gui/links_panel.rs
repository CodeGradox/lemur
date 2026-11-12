//! The Links panel, on the right as the notes and the changes are: the
//! worktree on show's pull requests, those of its branch and those agents
//! linked to, with what `gh` says of them; and the Claude artifacts agents
//! used or posted there, the latest first (see `links`). A click opens one;
//! its × takes it off the list for good, and leaves what it links to alone.
//! An artifact's pencil gives it a name of your own.

use super::{Lemur, Right, icons, worktree_label};
use crate::config::{Host, leaf};
use crate::gh::{self, Problem, PullRequest, State};
use crate::links::{self, Artifact, Place, Store};
use rune::{El, Key as K, ScrollSpec, TextEdit, Tx, col, icon, row, text};
use rune_kit::shell::SidebarRow;
use rune_kit::{Kit, TextInput};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// How often a worktree's logs are read and `gh` asked while the panel
/// shows: an agent's log can be hundreds of megabytes.
const EVERY: Duration = Duration::from_secs(30);

pub struct LinksPanel {
    /// The list kept, `~/.lemur/links.json`; tests have none.
    file: Option<PathBuf>,
    store: Store,
    /// The host and folder shown, and what it's called.
    place: Option<super::panels::Place>,
    /// The host and folder being read.
    loading: Option<(String, String)>,
    /// Why the logs couldn't be read.
    error: Option<String>,
    /// `gh`, and why it can't say anything.
    gh: PathBuf,
    gh_problem: Option<Problem>,
    /// When they were last asked for.
    asked: Option<Instant>,
    /// The artifact being given a name.
    renaming: Option<Renaming>,
}

/// An artifact being given a name: its link, and the field for it.
struct Renaming {
    url: String,
    input: TextEdit,
}

fn now() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// What reading a folder found: its scan, and what `gh` said of its pull requests.
type Found = anyhow::Result<(links::Scan, Option<gh::Looked>)>;

impl LinksPanel {
    pub fn new(data_dir: Option<&Path>) -> LinksPanel {
        let file = data_dir.map(|d| d.join("links.json"));
        let store = file.as_deref().map(Store::load).unwrap_or_default();
        LinksPanel {
            file,
            store,
            place: None,
            loading: None,
            error: None,
            // Tests have a stand-in, and never the real one.
            gh: PathBuf::from(if cfg!(test) { "/nonexistent/gh" } else { "gh" }),
            gh_problem: None,
            asked: None,
            renaming: None,
        }
    }

    /// Whether it shows folder `on`, by its host and path; with None,
    /// nothing.
    pub(super) fn is_on(&self, on: Option<(&str, &str)>) -> bool {
        super::panels::place_is(self.place.as_ref(), on)
    }

    fn shows(&self, host: &str, path: &str) -> bool {
        self.is_on(Some((host, path)))
    }

    fn save(&mut self) {
        if let Some(file) = &self.file
            && let Err(e) = self.store.save(file)
        {
            self.error = Some(format!("keeping the links: {e:#}"));
        }
    }

    /// What's known of the folder on show.
    fn shown(&self) -> Option<&Place> {
        let (host, path, _) = self.place.as_ref()?;
        self.store.place(&host.name, path)
    }

    /// The artifacts of the folder on show.
    fn links(&self) -> &[Artifact] {
        self.shown().map_or(&[], |place| &place.links)
    }

    /// The pull requests of the folder on show.
    fn prs(&self) -> &[PullRequest] {
        self.shown().map_or(&[], |place| &place.prs)
    }

    /// What to call `artifact`: the name you gave it, else the agent's.
    fn name_of<'a>(&'a self, artifact: &'a Artifact) -> &'a str {
        self.shown().map_or(artifact.name(), |p| p.name_of(artifact))
    }

    #[cfg(test)]
    pub(super) fn listed(&self) -> (bool, Vec<String>) {
        let names = self.links().iter().map(|a| self.name_of(a).to_string()).collect();
        (self.loading.is_some(), names)
    }

    #[cfg(test)]
    fn listed_prs(&self) -> (bool, Vec<(u64, State)>) {
        let prs = self.prs().iter().map(|pr| (pr.number, pr.state)).collect();
        (self.loading.is_some(), prs)
    }
}

impl Lemur {
    /// Points the Links panel at where the thread on show runs: its worktree, or its
    /// folder outside every worktree.
    pub(super) fn show_links(&mut self) {
        let place = self.active.clone().and_then(|key| {
            let (path, _) = self.place_of(&key)?;
            let host = self.cfg.host(&key.0)?.clone();
            let label = match self.project_of(&key.0, &key.1) {
                Some((_, w)) => worktree_label(w),
                None => leaf(&path).to_string(),
            };
            Some((host, path, label))
        });
        match place {
            Some((host, path, label)) => self.links_show(host, path, label),
            None => self.links_clear(),
        }
    }

    /// The panel goes by the thread on show (see `Lemur::follow_shown`).
    pub(super) fn links_follow_shown(&mut self) {
        if self.right() != Some(Right::Links) {
            return;
        }
        let on = self.active.is_none();
        let place = self.active.as_ref().and_then(|k| Some((k.0.clone(), self.place_of(k)?.0)));
        if on || place.is_some_and(|(host, path)| !self.panels.links.is_on(Some((&host, &path)))) {
            self.show_links();
        }
    }

    /// Shows folder `path` on `host`, called `label`, and reads its logs if
    /// that's new, or due.
    fn links_show(&mut self, host: Host, path: String, label: String) {
        let p = &mut self.panels.links;
        let same = p.shows(&host.name, &path);
        p.place = Some((host, path, label));
        if !same {
            p.error = None;
            p.gh_problem = None;
            self.drop_renaming();
        }
        let p = &self.panels.links;
        if !same || p.asked.is_none_or(|at| at.elapsed() >= EVERY) {
            self.refresh_links();
        }
    }

    /// Nothing on show to tell the links of.
    fn links_clear(&mut self) {
        let p = &mut self.panels.links;
        if p.place.take().is_some() {
            p.error = None;
            p.gh_problem = None;
            self.drop_renaming();
        }
    }

    /// Reads the logs of the folder on show, those written since the last
    /// time, and asks `gh` about its pull requests. Another folder being read
    /// is left to ask again when it shows: it may be taking a while, as
    /// with the network down.
    fn refresh_links(&mut self) {
        let p = &mut self.panels.links;
        let Some((host, path, _)) = p.place.clone() else { return };
        let asked = (host.name.clone(), path.clone());
        if p.loading.as_ref() == Some(&asked) {
            return;
        }
        p.loading = Some(asked);
        p.asked = Some(Instant::now());
        let started = now();
        let known = p.store.place(&host.name, &path);
        let minutes = known.and_then(|place| place.minutes(started));
        let wanted = known.map_or(vec![], Place::wanted);
        let finished = known.map_or(vec![], Place::finished);
        let tool = p.gh.clone();
        self.in_background(
            move || {
                let found = links::scan(&host, &path, minutes).map(|scan| {
                    let looked = scan.repo.as_deref().map(|repo| {
                        let mut numbers = wanted;
                        for number in &scan.prs {
                            if !numbers.contains(number) && !finished.contains(number) {
                                numbers.push(*number);
                            }
                        }
                        gh::look(&tool, repo, scan.branch.as_deref(), &numbers)
                    });
                    (scan, looked)
                });
                (host, path, found)
            },
            move |s, (host, path, found)| s.links_found(&host.name, &path, started, found),
        );
    }

    /// What reading folder `path` on `host`, started at `started`, found. It's kept
    /// whichever folder shows by now.
    fn links_found(&mut self, host: &str, path: &str, started: u64, found: Found) {
        let p = &mut self.panels.links;
        if p.loading.as_ref().is_some_and(|(h, l)| h == host && l == path) {
            p.loading = None;
        }
        let shown = p.shows(host, path);
        match found {
            Ok((scan, looked)) => {
                if shown {
                    p.error = None;
                    p.gh_problem = looked.as_ref().and_then(|l| l.problem);
                }
                let place = p.store.place_mut(host, path);
                place.add(scan.artifacts);
                // Noted whatever `gh` says, as these logs aren't read again.
                place.link_prs(&scan.prs);
                if let Some(looked) = looked {
                    place.add_prs(looked.prs, &looked.settled);
                }
                place.looked = Some(started);
                p.save();
            }
            Err(e) if shown => p.error = Some(format!("{e:#}")),
            Err(_) => {}
        }
    }

    /// Takes `url` off the list of the folder on show.
    fn remove_link(&mut self, url: &str) {
        let p = &mut self.panels.links;
        let Some((host, path, _)) = &p.place else { return };
        p.store.place_mut(&host.name, path).remove(url);
        p.save();
    }

    /// Asks for a name for the artifact `url`, in its row. Enter, or the
    /// keyboard going elsewhere, keeps what's typed; Esc doesn't. Once the
    /// field is gone, the keyboard goes back to the thread on show.
    fn start_renaming(&mut self, url: String) {
        // Another's name being given is kept, as the keyboard goes elsewhere.
        self.finish_renaming(true);
        let p = &mut self.panels.links;
        let name = p.links().iter().find(|l| l.url == url).map(|a| p.name_of(a).to_string()).unwrap_or_default();
        // All of it selected, so typing replaces it.
        p.renaming = Some(Renaming { url, input: super::prompt::field(&name) });
    }

    /// Done with the name asked for; with `keep`, it's kept.
    fn finish_renaming(&mut self, keep: bool) {
        let p = &mut self.panels.links;
        let Some(renaming) = p.renaming.take() else { return };
        if keep && let Some((host, path, _)) = &p.place {
            p.store.place_mut(&host.name, path).rename(&renaming.url, &renaming.input.text, now());
            p.save();
        }
    }

    /// The panel goes elsewhere: a name being given is let go.
    fn drop_renaming(&mut self) {
        self.panels.links.renaming = None;
    }

    /// The name being given has the keyboard.
    pub(super) fn links_editing(&self) -> bool {
        self.right() == Some(Right::Links) && self.panels.links.renaming.as_ref().is_some_and(|r| r.input.focused)
    }

    /// Before each frame: the name being given lost the keyboard, to a click elsewhere or
    /// the panel hidden. What's typed is kept, as with Enter. True if that changed anything.
    pub(super) fn links_keyboard(&mut self) -> bool {
        if self.panels.links.renaming.is_none() || self.links_editing() {
            return false;
        }
        self.finish_renaming(true);
        true
    }
}

/// A section's name, over its rows.
fn section(name: &'static str, k: &Kit) -> El {
    text(name).font(k.ty.small).color(k.c.text_muted).px(4.0).pt(8.0).pb(2.0)
}

/// A small button on a row, shown under the pointer.
fn row_button(glyph: &'static rune::Icon, tip: &str, key: String, k: &Kit, press: impl Fn(&mut Lemur) + 'static) -> El {
    icons::button(glyph, tip)
        .on_press(press)
        .build(k)
        .key(key)
        .group_opacity(0.0, 1.0)
}

/// A pull request's row: its state, number and title.
fn pr_row(i: usize, pr: &PullRequest, k: &Kit) -> El {
    let c = &k.c;
    let color = match pr.state {
        State::Open => c.success.fg,
        State::Draft => c.text_muted,
        State::Merged => rune::color::hex("#8250df").unwrap_or(c.info.fg),
        State::Closed => c.danger.fg,
    };
    let state = row()
        .h(18.0)
        .px(6.0)
        .center()
        .radius(k.radius.sm)
        .fill(color.alpha(0.15))
        .child(text(pr.state.name()).font(k.ty.small_medium).color(color));
    let (open, remove) = (pr.url.clone(), pr.url.clone());
    let r = row()
        .key(format!("links-pr-{i}"))
        .w_full()
        .h(28.0)
        .px(8.0)
        .gap(8.0)
        .center_y()
        .radius(k.radius.md)
        .hover_group()
        .cursor(rune::Cursor::Pointer)
        .help(pr.url.clone())
        .child(state)
        .child(text(format!("#{}", pr.number)).font(k.ty.ui).color(c.text_faint))
        .child(text(pr.title.clone()).font(k.ty.ui).color(c.text_muted).truncate().grow(1.0).shrink())
        .child(row_button(&icons::X, "Take it off this list", format!("links-pr-remove-{i}"), k, move |s| s.remove_link(&remove)))
        .on_press_tx(move |_: &mut Lemur, tx: &mut Tx| {
            let _ = tx.open_url(&open);
        });
    k.interactive(r)
}

/// An artifact's row: its name, a pencil to give it one and its ×; or, being named, a
/// field for the name.
fn artifact_row(p: &LinksPanel, i: usize, artifact: &Artifact, k: &Kit) -> El {
    if let Some(r) = p.renaming.as_ref().filter(|r| r.url == artifact.url) {
        // A field of its own rather than the sidebar row's: that one hands the keyboard to
        // the sidebar when it goes, and here it goes back to the thread on show.
        let field = TextInput::new(&r.input)
            .plain()
            .width(None)
            .on_edit(|s: &mut Lemur| &mut s.panels.links.renaming.as_mut().expect("a name being given").input)
            .on_submit(|s: &mut Lemur, _| s.finish_renaming(true))
            .on_key(|s: &mut Lemur, ev, _| {
                if ev.key == K::Escape {
                    s.finish_renaming(false);
                    return true;
                }
                false
            })
            .build(k)
            .key(format!("links-name-{i}"));
        return row()
            .key(format!("links-renaming-{i}"))
            .w_full()
            .h(28.0)
            .px(8.0)
            .gap(6.0)
            .center_y()
            .radius(k.radius.md)
            .fill(k.c.pressed)
            .child(icon(&icons::SHAPES, 16.0).color(k.c.icon_muted))
            .child(field.grow(1.0).shrink());
    }
    let line = SidebarRow::new(p.name_of(artifact)).icon(&icons::SHAPES);
    let (open, rename, remove) = (artifact.url.clone(), artifact.url.clone(), artifact.url.clone());
    let buttons = row()
        .gap(2.0)
        .center_y()
        .child(row_button(&icons::PENCIL, "Give it a name of your own", format!("links-artifact-rename-{i}"), k, move |s| {
            s.start_renaming(rename.clone())
        }))
        .child(row_button(&icons::X, "Take it off this list", format!("links-artifact-remove-{i}"), k, move |s| s.remove_link(&remove)));
    line.trailing(buttons)
        .on_select(move |_: &mut Lemur, tx: &mut Tx| {
            let _ = tx.open_url(&open);
        })
        .build(k)
        .key(format!("links-artifact-{i}"))
        .help(artifact.url.clone())
}

pub fn view(s: &Lemur, k: &Kit) -> El {
    let p = &s.panels.links;
    let c = &k.c;
    let (links, prs) = (p.links(), p.prs());
    let count = links.len() + prs.len();
    let empty = match (&p.place, &p.error) {
        (None, _) => Some("Open a thread to see its worktree's links.".to_string()),
        (_, Some(e)) => Some(e.clone()),
        _ if count == 0 && p.loading.is_some() => Some("Looking for links…".into()),
        _ if count == 0 => Some(
            "No links here yet. This branch's pull requests, and the artifacts and pull requests agents use or post, show up here."
                .into(),
        ),
        _ => None,
    };
    // Why there may be pull requests missing.
    let gh_note = p.gh_problem.map(|problem| match problem {
        Problem::Missing => "Install GitHub's gh to see pull requests here.",
        Problem::SignedOut => "Run gh auth login to see pull requests here.",
    });
    let mut list = col().key("links-list").w_full().grow(1.0).min_h(0.0).px(8.0).pb(8.0).gap(1.0).scroll(ScrollSpec { stick_bottom: false });
    if !prs.is_empty() {
        list = list.child(section("Pull requests", k));
    }
    list = list.children(prs.iter().enumerate().map(|(i, pr)| pr_row(i, pr, k)));
    if let Some(note) = gh_note {
        list = list.child(text(note).key("links-gh-note").font(k.ty.small).color(c.text_muted).wrap().padding(4.0));
    }
    if !links.is_empty() {
        list = list.child(section("Artifacts", k));
    }
    list = list.children(links.iter().enumerate().map(|(i, a)| artifact_row(p, i, a, k)));
    if let Some(e) = empty {
        list = list.child(text(e).key("links-empty").font(k.ty.ui).color(c.text_muted).wrap().padding(4.0));
    }
    col().key("links-panel").w_full().h_full().min_h(0.0).child(super::panels::header("links-head", None, "Links", p.place.as_ref().map(|(_, _, l)| l.as_str()), count, k)).child(list)
}

#[cfg(test)]
#[path = "links_panel_tests.rs"]
pub(super) mod tests;
