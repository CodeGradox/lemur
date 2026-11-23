//! Sessions, as Rex has them and the sidebar shows one at a time: a folder on a machine,
//! This Mac or a host. Each host's home is one, and each project in the config another
//! (`[[bases]]`, which New session… adds). Claude or a shell started in a session starts in
//! its folder; ⇧⌘N starts a shell in the machine's home, in a new folder of the session's.
//! A thread is in the session it was started in (`snapshot::SESSION`); one started
//! elsewhere, in the project it runs in, else its host's home.
//!
//! In the code a session is a `Space`, as a thread is a `Session`, herdr-core's word.

use super::folders::listed;
use super::{Key, Lemur};
use crate::snapshot::HOME;

/// A session: its host, and its folder as the config has it (a project's `path`), or `HOME`.
pub type Space = (String, String);

/// A session as ⌘K and the sidebar's header list it.
#[derive(Clone, Debug, PartialEq)]
pub struct SpaceView {
    pub space: Space,
    /// Its project's name, or Home.
    pub name: String,
    /// Its host, as it shows: This Mac, or the host's name.
    pub host: String,
}

impl SpaceView {
    /// What tells it apart in a list: an id with its host and folder.
    pub fn id(&self) -> String {
        space_id(&self.space)
    }
}

/// `space` as one string, to pick it by: its host and its folder.
pub fn space_id((host, path): &Space) -> String {
    format!("{host}\n{path}")
}

/// The session `id` names (`space_id`).
pub fn space_of_id(id: &str) -> Option<Space> {
    let (host, path) = id.split_once('\n')?;
    Some((host.to_string(), path.to_string()))
}

impl Lemur {
    /// Every session, host by host in the config's order: its home, then its projects.
    pub(super) fn spaces(&self) -> Vec<SpaceView> {
        let listed = self.snap.hosts.iter().map(|h| h.name.as_str()).filter(|name| self.cfg.host(name).is_none());
        let hosts: Vec<&str> = self.cfg.hosts.iter().map(|h| h.name.as_str()).chain(listed).collect();
        let mut spaces = vec![];
        for host in hosts {
            let shown = self.cfg.host(host).map_or_else(|| host.to_string(), super::picker::host_label);
            spaces.push(SpaceView { space: (host.to_string(), HOME.into()), name: "Home".into(), host: shown.clone() });
            for base in self.cfg.bases.iter().filter(|b| b.host == host) {
                spaces.push(SpaceView { space: (host.to_string(), base.path.clone()), name: base.name(), host: shown.clone() });
            }
        }
        spaces
    }

    /// Session `space` as it's listed, if it's one.
    pub(super) fn space_view(&self, space: &Space) -> Option<SpaceView> {
        self.spaces().into_iter().find(|v| v.space == *space)
    }

    /// The session thread `key` is in, once a snapshot lists it.
    pub(super) fn space_of(&self, key: &Key) -> Option<Space> {
        self.session_of(key).map(|s| (key.0.clone(), s.space.clone()))
    }

    /// The session the sidebar shows: the thread on show's; else the one picked, or that the
    /// thread last on show was in; else the one of the thread opened last, or on show when
    /// Lemur last quit, gone since or not; else the first with threads; else This Mac's home,
    /// or the first host's.
    pub(super) fn shown_space(&self) -> Option<Space> {
        let known = |space: &Space| self.host_view(&space.0).is_some() && self.space_view(space).is_some();
        self.active
            .as_ref()
            .and_then(|key| self.space_of(key))
            .or_else(|| self.space_shown.clone().filter(known))
            .or_else(|| self.recent.iter().find_map(|key| self.space_of(key)))
            .or_else(|| self.remembered.as_ref().and_then(|key| self.space_of(key).or_else(|| self.gone_space(key))))
            .or_else(|| self.spaces().into_iter().map(|v| v.space).find(|space| !self.space_threads(space).is_empty()))
            .or_else(|| {
                let here = self.cfg.hosts.iter().find(|h| crate::remote::Remote::new(h).is_local()).map(|h| &h.name);
                let host = here.or(self.cfg.hosts.first().map(|h| &h.name)).or(self.snap.hosts.first().map(|h| &h.name))?;
                Some((host.clone(), HOME.to_string()))
            })
    }

    /// The session of thread `key` gone, kept to resume: the one of where it ran.
    fn gone_space(&self, (host, name): &Key) -> Option<Space> {
        let h = self.host_view(host)?;
        let gone_in = |w: &crate::snapshot::Worktree| w.gone.iter().any(|t| t.session == *name);
        let base = h.bases.iter().find(|b| b.worktrees.iter().any(gone_in));
        match base {
            Some(b) => Some((host.clone(), b.path.clone())),
            None => h.gone.iter().any(|t| t.session == *name).then(|| (host.clone(), HOME.to_string())),
        }
    }

    /// Shows session `space`: the thread of its opened last, else its first; with none,
    /// nothing, till one starts there.
    pub(super) fn show_space(&mut self, space: Space) {
        self.space_shown = Some(space.clone());
        if self.shown_space().is_some_and(|s| s == space) && self.active.is_some() {
            return;
        }
        let threads = self.space_threads(&space);
        let last = self.recent.iter().find(|k| threads.contains(k)).cloned();
        if let Some((host, session)) = last.or_else(|| threads.first().cloned()) {
            return self.open(host, session);
        }
        self.hide_shown(None);
        self.active = None;
        self.follow_shown();
    }

    /// Every thread of session `space` in the sidebar's order, none put away: each folder's,
    /// folded or not.
    pub(super) fn space_threads(&self, (host, path): &Space) -> Vec<Key> {
        let Some(h) = self.host_view(host) else { return vec![] };
        let l = listed(h, Some(path));
        l.groups.iter().flat_map(|g| &g.threads).map(|s| (host.clone(), s.name.clone())).collect()
    }

    /// The threads the sidebar lists, from the top: every agent's (⌘.); else those of the
    /// sessions it lists, every one or the one on show, in the folders not folded, none put
    /// away.
    pub(super) fn sidebar_threads(&self) -> Vec<Key> {
        if self.sidebar_agents {
            return self.sidebar_agent_threads().into_iter().map(|(h, s)| super::key(h, s)).collect();
        }
        let spaces = if self.prefs.all_sessions { self.spaces().into_iter().map(|v| v.space).collect() } else { self.shown_space().into_iter().collect::<Vec<_>>() };
        let mut threads = vec![];
        for (host, path) in spaces {
            let Some(h) = self.host_view(&host) else { continue };
            let l = listed(h, Some(&path));
            threads.extend(l.groups.iter().filter(|g| !self.folded(&host, &g.id)).flat_map(|g| &g.threads).map(|s| (host.clone(), s.name.clone())));
        }
        threads
    }

    /// The sidebar lists every session, each in a section of its own, or the one on show
    /// alone, as the config keeps it (`all_sessions`).
    pub fn toggle_all_sessions(&mut self) {
        self.prefs.all_sessions = !self.prefs.all_sessions;
        if let Some(file) = self.prefs.config_file()
            && let Err(e) = crate::config::Config::save_all_sessions(file, self.prefs.all_sessions)
        {
            self.fail(e);
        }
    }

    /// Where Claude or a shell started in session `space` starts: its project's own checkout,
    /// or its host's home; none until the host has said.
    pub(super) fn space_dir(&self, (host, path): &Space) -> Option<String> {
        let h = self.host_view(host)?;
        if path == HOME {
            return Some(h.home.clone()).filter(|home| !home.is_empty());
        }
        let b = h.bases.iter().find(|b| b.path == *path)?;
        b.worktrees.iter().find(|w| w.main).or(b.worktrees.first()).map(|w| w.path.clone())
    }

    /// The project session `space` is, by its place in the config; none for a home.
    pub(super) fn space_base(&self, (host, path): &Space) -> Option<usize> {
        self.cfg.bases.iter().position(|b| b.host == *host && b.path == *path)
    }
}
