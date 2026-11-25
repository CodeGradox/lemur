//! Folders of a session's threads (`spaces`), in the sidebar, as Rex keeps a host's sessions:
//! every thread is in one. A shell started as a new folder (⇧⌘N, the session's +) has one of
//! its own; a thread started beside another (⌘T, ⌘J) goes in that one's. Folder ids are the
//! host's, whichever session. A folder you haven't named goes
//! by two words of its own, as Rex's sessions do (`calm-heron`), or, as Settings has it, by
//! the folder of the worktree its Claude works in. Dragged, a thread takes another place in
//! its folder, never out of it (its right-click moves it to another); a folder another place
//! among the folders, its threads with it (`SidebarDrag`). herdr-core
//! keeps which folder each thread is in (`lemur.folder`) and its place (`lemur.order`);
//! Lemur's config the folders you named or folded.

use super::spaces::Space;
use super::{Key, Lemur, List};
use crate::actions::name_from;
use crate::config::{ClaudeThread, Config, Folder, FolderNames, leaf};
use crate::snapshot::{HOME, HostView, Session, find_worktree, rank};
use std::collections::{HashMap, HashSet};

/// What starts the folder of a thread on its own, before its address: no folder's id does.
const OWN: char = '=';

/// The folder thread `s` is in: its id, or, on its own, `=` and its address.
pub fn folder_of(s: &Session) -> String {
    s.folder.clone().unwrap_or_else(|| format!("{OWN}{}", s.name))
}

/// A folder as the sidebar lists it.
pub struct Group<'a> {
    pub id: String,
    pub threads: Vec<&'a Session>,
}

/// A host's threads as the sidebar lists them.
pub struct Listed<'a> {
    /// Each folder with its threads, none put away, in their order: a folder is where its
    /// first thread is.
    pub groups: Vec<Group<'a>>,
    /// Those put away, and those gone, at the end.
    pub away: Vec<&'a Session>,
    pub gone: Vec<&'a ClaudeThread>,
}

/// Host `h`'s threads in session `space` (a project's path, or `HOME`), or with none in
/// every one, in their order.
pub fn listed<'a>(h: &'a HostView, space: Option<&str>) -> Listed<'a> {
    let worktrees = || h.bases.iter().flat_map(|b| &b.worktrees);
    let in_space = |s: &&Session| space.is_none_or(|p| s.space == p);
    let mut all: Vec<&Session> = worktrees().flat_map(|w| &w.sessions).chain(&h.other).filter(in_space).collect();
    all.sort_by(|x, y| rank(x).cmp(&rank(y)));
    let (away, shown): (Vec<&Session>, Vec<&Session>) = all.into_iter().partition(|s| s.away);
    let mut groups: Vec<Group> = vec![];
    for s in shown {
        let id = folder_of(s);
        match groups.iter_mut().find(|g| g.id == id) {
            Some(g) => g.threads.push(s),
            None => groups.push(Group { id, threads: vec![s] }),
        }
    }
    // A thread kept to resume is in the session of where it ran.
    let gone_in = |p: &str| space.is_none_or(|s| s == p);
    let gone_there = h.bases.iter().filter(|b| gone_in(&b.path)).flat_map(|b| &b.worktrees).flat_map(|w| &w.gone);
    let gone_home = h.gone.iter().filter(|_| gone_in(HOME));
    Listed { groups, away, gone: gone_there.chain(gone_home).collect() }
}

impl Listed<'_> {
    /// Every thread in its order, with its folder: those shown, then those put away, which
    /// keep theirs.
    pub fn placed(&self) -> Vec<(String, String)> {
        let mut placed: Vec<(String, String)> = vec![];
        for g in &self.groups {
            placed.extend(g.threads.iter().map(|s| (s.name.clone(), g.id.clone())));
        }
        placed.extend(self.away.iter().map(|s| (s.name.clone(), folder_of(s))));
        placed
    }
}

/// `placed` with the `from`th thread of folder `folder` moved to its `to`th place: the others
/// keep theirs, and every thread its folder.
fn moved_within(placed: &[(String, String)], folder: &str, from: usize, to: usize) -> Vec<(String, String)> {
    let at: Vec<usize> = placed.iter().enumerate().filter(|(_, (_, f))| f == folder).map(|(i, _)| i).collect();
    let mut out = placed.to_vec();
    if from >= at.len() || to >= at.len() {
        return out;
    }
    let mut threads: Vec<(String, String)> = at.iter().map(|&i| placed[i].clone()).collect();
    let thread = threads.remove(from);
    threads.insert(to, thread);
    for (i, thread) in at.into_iter().zip(threads) {
        out[i] = thread;
    }
    out
}

/// `groups`' threads, each with its folder (`(id, threads)`), the folders in `order`'s order;
/// any it doesn't have keep theirs after those it does.
fn in_order(groups: &[(String, Vec<String>)], order: &[String]) -> Vec<(String, String)> {
    let mut groups: Vec<&(String, Vec<String>)> = groups.iter().collect();
    groups.sort_by_key(|(id, _)| order.iter().position(|o| o == id).unwrap_or(usize::MAX));
    groups.into_iter().flat_map(|(id, threads)| threads.iter().map(move |t| (t.clone(), id.clone()))).collect()
}

/// Points the pointer goes before a press on a row is a drag, as rune-kit's `Reorder` has it.
const SLOP: f32 = 4.0;

/// A drag in the sidebar, in `list`, a session's: of a folder among its folders, its threads
/// with it, or with `folder`, of a thread among that folder's threads, and only there. Unlike
/// rune-kit's `Reorder`, which takes its rows to be one height, a folder is as tall as its
/// threads make it; and what's dragged stops at the list's ends, so a thread never shows out
/// of its folder.
#[derive(Clone, Debug, PartialEq)]
pub struct SidebarDrag {
    pub list: List,
    pub folder: Option<String>,
    /// What it's among, by id (a folder's, or a thread's name), in order, and how far each
    /// starts from the next, as the drag began.
    ids: Vec<String>,
    pitches: Vec<f32>,
    from: usize,
    /// How far the pointer has gone, once past `SLOP`: till then it's a press.
    dy: Option<f32>,
}

impl SidebarDrag {
    /// A press on the `from`th of `rows` (`(id, pitch)`): folders, or with `folder` its threads.
    pub fn new(list: List, folder: Option<String>, rows: &[(String, f32)], from: usize) -> SidebarDrag {
        let (ids, pitches) = rows.iter().cloned().unzip();
        SidebarDrag { list, folder, ids, pitches, from, dy: None }
    }

    /// The pointer is `dy` from the press.
    pub fn follow(&mut self, dy: f32) {
        if self.dy.is_some() || dy.abs() >= SLOP {
            self.dy = Some(dy);
        }
    }

    /// Whether it's a drag, not a press.
    pub fn moved(&self) -> bool {
        self.dy.is_some()
    }

    /// Whether `id` is the one dragged, lifted off the others.
    pub fn lifts(&self, id: &str) -> bool {
        self.moved() && self.ids.get(self.from).is_some_and(|f| f == id)
    }

    /// Where it lands if let go now: past every folder whose middle it has passed.
    fn target(&self) -> usize {
        let Some(dy) = self.dy else { return self.from };
        let (mut to, mut passed) = (self.from, 0.0);
        let passes = |pitch: f32, passed: f32| dy.abs() > passed + pitch / 2.0;
        if dy > 0.0 {
            for j in self.from + 1..self.pitches.len() {
                if !passes(self.pitches[j], passed) {
                    break;
                }
                (to, passed) = (j, passed + self.pitches[j]);
            }
        } else {
            for j in (0..self.from).rev() {
                if !passes(self.pitches[j], passed) {
                    break;
                }
                (to, passed) = (j, passed + self.pitches[j]);
            }
        }
        to
    }

    /// How far `id` sits from its place: the one dragged with the pointer, as far as the
    /// list's ends, and those it passed a step toward the room it left.
    pub fn shift(&self, id: &str) -> f32 {
        let (Some(dy), Some(i)) = (self.dy, self.ids.iter().position(|f| f == id)) else { return 0.0 };
        let (from, to, step) = (self.from, self.target(), self.pitches[self.from]);
        if i == from {
            let (above, below) = (self.pitches[..from].iter().sum::<f32>(), self.pitches[from + 1..].iter().sum::<f32>());
            dy.clamp(-above, below)
        } else if from < i && i <= to {
            -step
        } else if to <= i && i < from {
            step
        } else {
            0.0
        }
    }

    /// Where it goes once let go, if it moved: from its place to another.
    pub fn moves(&self) -> Option<(usize, usize)> {
        let to = self.target();
        (to != self.from).then_some((self.from, to))
    }

    /// The order once let go, by id, if it's another.
    pub fn dropped(&self) -> Option<Vec<String>> {
        let (from, to) = self.moves()?;
        let mut ids = self.ids.clone();
        let id = ids.remove(from);
        ids.insert(to, id);
        Some(ids)
    }
}

/// `placed` with thread `name` moved to the end of folder `folder`, or with `None` to a
/// folder of its own at the end.
fn refiled(placed: &[(String, String)], name: &str, folder: Option<&str>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = placed.iter().filter(|(n, _)| n != name).cloned().collect();
    let to = folder.and_then(|f| out.iter().rposition(|(_, x)| x == f)).map_or(out.len(), |at| at + 1);
    out.insert(to, (name.to_string(), folder.unwrap_or_default().to_string()));
    out
}

/// `threads` with their folders as herdr-core keeps them: a thread with none of its own yet
/// (`=` and its address) stays so alone (`""`), and with others they get one, which
/// `new_id` makes of that.
fn settled(threads: Vec<(String, String)>, mut new_id: impl FnMut(&str) -> String) -> Vec<(String, String)> {
    let count = |f: &str| threads.iter().filter(|(_, x)| x == f).count();
    let mut made: HashMap<String, String> = HashMap::new();
    let folders: Vec<String> = threads
        .iter()
        .map(|(_, f)| match f.starts_with(OWN) {
            true if count(f) == 1 => String::new(),
            true => made.entry(f.clone()).or_insert_with(|| new_id(f)).clone(),
            false => f.clone(),
        })
        .collect();
    threads.into_iter().zip(folders).map(|((name, _), folder)| (name, folder)).collect()
}

/// Whether folder id `id` is two words (and maybe a number) Lemur gave a folder, which it
/// goes by: ids from before were hexadecimal.
fn worded(id: &str) -> bool {
    !id.starts_with(OWN) && id.contains('-')
}

/// The two words each of `groups`' folders goes by, as Rex names its sessions. A folder
/// whose id is two words goes by it; another by the words its id makes (`name_from`), a
/// thread's on its own by its address, so that it keeps them once another joins it and they
/// become its id. No two share them: of those that would, the one with the least id keeps
/// them, and the others make more.
fn word_names(groups: &[Group]) -> Vec<String> {
    let mut taken: HashSet<String> = groups.iter().filter(|g| worded(&g.id)).map(|g| g.id.clone()).collect();
    let mut ids: Vec<&str> = groups.iter().map(|g| g.id.as_str()).filter(|id| !worded(id)).collect();
    ids.sort_unstable();
    let mut made: HashMap<&str, String> = HashMap::new();
    for id in ids {
        let seed = id.trim_start_matches(OWN);
        let words = (0..50)
            .map(|n| if n == 0 { name_from(seed) } else { name_from(&format!("{seed}#{n}")) })
            .find(|w| !taken.contains(w))
            .unwrap_or_else(|| fresh(&name_from(seed), |w| taken.contains(w)));
        taken.insert(words.clone());
        made.insert(id, words);
    }
    groups.iter().map(|g| made.remove(g.id.as_str()).unwrap_or_else(|| g.id.clone())).collect()
}

/// `words`, or, taken, them with the first number after them that isn't.
fn fresh(words: &str, taken: impl Fn(&str) -> bool) -> String {
    (1..).map(|n| if n == 1 { words.to_string() } else { format!("{words}-{n}") }).find(|id| !taken(id)).unwrap_or_default()
}

/// The folder of the worktree thread `s` of host `h`'s works in: its Claude's, which may
/// have gone into another, else the one it started in; else the folder it started in, `~`
/// for the host's home.
fn worktree_title(h: &HostView, s: &Session) -> String {
    let dir = s.claude.as_ref().map_or(s.path.as_str(), |c| c.path.as_str());
    let dir = match find_worktree(&h.bases, dir) {
        Some((b, w)) => h.bases[b].worktrees[w].path.as_str(),
        None => dir,
    };
    if !h.home.is_empty() && dir.trim_end_matches('/') == h.home.trim_end_matches('/') {
        return "~".into();
    }
    match leaf(dir) {
        "" => dir.to_string(),
        leaf => leaf.to_string(),
    }
}

/// What a folder's name may be: nothing, to go by the name Settings gives it again, or
/// something not too long for its row.
pub fn validate_folder_name(name: &str) -> anyhow::Result<()> {
    if name.chars().count() > 60 {
        anyhow::bail!("a folder's name has at most 60 characters");
    }
    Ok(())
}

impl Lemur {
    /// Host `host` as the snapshot has it.
    pub(super) fn host_view(&self, host: &str) -> Option<&HostView> {
        self.snap.hosts.iter().find(|h| h.name == host)
    }

    /// The list session `space`'s rows are, as dragging keys it.
    pub(super) fn space_list((host, path): &Space) -> List {
        (host.clone(), Some(path.clone()))
    }

    /// Whether folder `id` of `host`'s is folded to its row.
    pub(super) fn folded(&self, host: &str, id: &str) -> bool {
        self.cfg.host(host).is_some_and(|h| h.folders.iter().any(|f| f.id == id && f.collapsed))
    }

    /// What `groups`, folders of host `h`'s as listed, are called: the name you gave one, else
    /// as Settings' Folder names has it, its two words or its first Claude thread's worktree's
    /// folder, else its first thread's.
    pub(super) fn folder_titles(&self, h: &HostView, groups: &[Group]) -> Vec<String> {
        let kept = self.cfg.host(&h.name).map(|c| c.folders.as_slice()).unwrap_or_default();
        let named = |id: &str| kept.iter().find(|f| f.id == id).and_then(|f| f.name.clone());
        // Two words none of the host's other folders have, in whichever session.
        let all = listed(h, None).groups;
        let mut words: HashMap<String, String> = all.iter().map(|g| g.id.clone()).zip(word_names(&all)).collect();
        let titles = groups.iter().map(|g| (g, words.remove(&g.id).unwrap_or_else(|| name_from(g.id.trim_start_matches(OWN)))));
        let titles = titles.map(|(g, words)| match self.prefs.folder_names {
            FolderNames::Random => words,
            FolderNames::Worktree => {
                let first = g.threads.iter().find(|s| s.runs_claude()).or(g.threads.first());
                first.map_or(words, |s| worktree_title(h, s))
            }
        });
        groups.iter().zip(titles).map(|(g, title)| named(&g.id).unwrap_or(title)).collect()
    }

    /// What folder `id` of `host`'s is called, as the sidebar shows it.
    pub(super) fn folder_name(&self, host: &str, id: &str) -> String {
        let Some(h) = self.host_view(host) else { return String::new() };
        let groups = listed(h, None).groups;
        let Some(at) = groups.iter().position(|g| g.id == id) else { return String::new() };
        self.folder_titles(h, &groups).swap_remove(at)
    }

    /// The threads of the folder thread `key` is in, none put away.
    pub(super) fn group_of(&self, key: &Key) -> Vec<String> {
        let (Some(h), Some(s)) = (self.host_view(&key.0), self.session_of(key)) else { return vec![] };
        let id = folder_of(s);
        let l = listed(h, Some(&s.space));
        l.groups.iter().find(|g| g.id == id).map(|g| g.threads.iter().map(|s| s.name.clone()).collect()).unwrap_or_default()
    }

    /// The `from`th thread of folder `folder` of `list`, a session's, was dragged to its `to`th
    /// place: the folder's others keep theirs, and the other folders' theirs.
    pub(super) fn reorder_threads(&mut self, list: &List, folder: &str, from: usize, to: usize) {
        let Some(h) = self.host_view(&list.0) else { return };
        let threads = moved_within(&listed(h, list.1.as_deref()).placed(), folder, from, to);
        self.place_threads(list, threads);
    }

    /// The folders of `list`, a session's, go in `order`, by id, each with its threads.
    pub(super) fn reorder_folders(&mut self, list: &List, order: &[String]) {
        let Some(h) = self.host_view(&list.0) else { return };
        let l = listed(h, list.1.as_deref());
        let groups: Vec<(String, Vec<String>)> = l.groups.iter().map(|g| (g.id.clone(), g.threads.iter().map(|s| s.name.clone()).collect())).collect();
        let mut threads = in_order(&groups, order);
        threads.extend(l.away.iter().map(|s| (s.name.clone(), folder_of(s))));
        self.place_threads(list, threads);
    }

    /// Shows the threads of `list`, a session's, in `threads`' order and folders at once, and
    /// has herdr-core keep them.
    fn place_threads(&mut self, list: &List, threads: Vec<(String, String)>) {
        let host = list.0.as_str();
        let words = self.word_names(host);
        // Ids of the host's, in whichever session.
        let others: Vec<String> = self.host_view(host).map(|h| listed(h, None).placed().into_iter().map(|(_, f)| f).collect()).unwrap_or_default();
        let mut taken: Vec<String> = threads.iter().map(|(_, f)| f.clone()).chain(others).collect();
        let threads = settled(threads, |own| {
            let words = words.get(own).cloned().unwrap_or_else(|| name_from(own.trim_start_matches(OWN)));
            let id = fresh(&words, |id| taken.iter().any(|f| f == id));
            taken.push(id.clone());
            id
        });
        for (i, (name, folder)) in threads.iter().enumerate() {
            if let Some(s) = self.session_mut(&(host.to_string(), name.clone())) {
                s.order = Some(i as u32);
                s.folder = (!folder.is_empty()).then(|| folder.clone());
            }
        }
        self.write_places(list.clone(), threads);
    }

    /// Puts thread `key` at the end of folder `folder` of its host's, or with `None` in a
    /// folder of its own, last.
    pub(super) fn move_to_folder(&mut self, key: &Key, folder: Option<&str>) {
        let (Some(h), Some(space)) = (self.host_view(&key.0), self.space_of(key)) else { return };
        let threads = refiled(&listed(h, Some(&space.1)).placed(), &key.1, folder);
        self.place_threads(&Self::space_list(&space), threads);
        self.reveal(&key.0, &key.1);
    }

    /// The folder a thread started beside `key` goes in, by its id: `key`'s, which gets one
    /// of its own if it was on its own.
    pub(super) fn folder_beside(&mut self, key: &Key) -> Option<String> {
        let folder = self.session_of(key)?.folder.clone();
        Some(folder.unwrap_or_else(|| self.own_folder(key)))
    }

    /// The two words each folder of `host`'s goes by, by its id.
    fn word_names(&self, host: &str) -> HashMap<String, String> {
        let Some(h) = self.host_view(host) else { return HashMap::new() };
        let groups = listed(h, None).groups;
        groups.iter().map(|g| g.id.clone()).zip(word_names(&groups)).collect()
    }

    /// Gives thread `key`, on its own, a folder of its own, by the two words its folder goes
    /// by already: it shows at once, and herdr-core keeps it.
    fn own_folder(&mut self, key: &Key) -> String {
        let used: Vec<String> = self.host_view(&key.0).map(|h| listed(h, None).placed().into_iter().map(|(_, f)| f).collect()).unwrap_or_default();
        let own = format!("{OWN}{}", key.1);
        let words = self.word_names(&key.0).remove(&own).unwrap_or_else(|| name_from(&key.1));
        let id = fresh(&words, |id| used.iter().any(|f| f == id));
        if let Some(s) = self.session_mut(key) {
            s.folder = Some(id.clone());
        }
        let (host, session, folder) = (key.0.clone(), key.1.clone(), id.clone());
        self.write_core(move |cfg| crate::actions::set_folder(cfg, &host, &session, &folder));
        id
    }

    /// The id folder `id` of `host`'s keeps: a thread's on its own gets one of its own.
    fn kept_id(&mut self, host: &str, id: &str) -> String {
        match id.strip_prefix(OWN) {
            Some(address) => self.own_folder(&(host.to_string(), address.to_string())),
            None => id.to_string(),
        }
    }

    /// What Lemur keeps of folder `id` of `host`'s, made if there's none.
    fn look(&mut self, host: &str, id: &str) -> Option<&mut Folder> {
        let h = self.cfg.hosts.iter_mut().find(|h| h.name == host)?;
        let at = match h.folders.iter().position(|f| f.id == id) {
            Some(at) => at,
            None => {
                h.folders.push(Folder { id: id.to_string(), name: None, collapsed: false });
                h.folders.len() - 1
            }
        };
        h.folders.get_mut(at)
    }

    /// Names folder `id` of `host`'s `name`, or with an empty one by the name Settings gives
    /// it again.
    pub(super) fn rename_folder(&mut self, host: &str, id: &str, name: String) {
        let id = self.kept_id(host, id);
        if let Some(f) = self.look(host, &id) {
            f.name = Some(name).filter(|n| !n.is_empty());
        }
        self.save_folders(host);
    }

    /// Folds folder `id` of `host`'s to its row, or opens it.
    pub(super) fn toggle_folder(&mut self, host: &str, id: &str) {
        let id = self.kept_id(host, id);
        if let Some(f) = self.look(host, &id) {
            f.collapsed = !f.collapsed;
        }
        self.save_folders(host);
    }

    /// Unfolds the folder that keeps a thread's row out of sight.
    pub(super) fn reveal(&mut self, host: &str, session: &str) {
        let Some(id) = self.session_of(&(host.to_string(), session.to_string())).map(folder_of) else { return };
        if self.folded(host, &id) {
            self.toggle_folder(host, &id);
        }
    }

    /// Keeps what Lemur keeps of `host`'s folders in the config file: only those named or
    /// folded.
    fn save_folders(&mut self, host: &str) {
        let Some(h) = self.cfg.hosts.iter_mut().find(|h| h.name == host) else { return };
        h.folders.retain(|f| f.name.is_some() || f.collapsed);
        let h = h.clone();
        if let Some(file) = self.prefs.config_file()
            && let Err(e) = Config::save_host_folders(file, &h)
        {
            self.fail(e);
        }
    }

    /// Forgets the folders no thread is in any more, put away or not, on the hosts reached.
    pub(super) fn drop_empty_folders(&mut self) {
        let reached: Vec<String> = self.snap.hosts.iter().filter(|h| h.error.is_none()).map(|h| h.name.clone()).collect();
        for host in reached {
            let Some(h) = self.host_view(&host) else { continue };
            let used: Vec<String> = listed(h, None).placed().into_iter().map(|(_, f)| f).collect();
            let Some(h) = self.cfg.hosts.iter_mut().find(|h| h.name == host) else { continue };
            let kept = h.folders.len();
            h.folders.retain(|f| used.contains(&f.id));
            if h.folders.len() != kept {
                self.save_folders(&host);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(n, f)| (n.to_string(), f.to_string())).collect()
    }

    /// A thread dragged takes another place in its folder, never out of it, and the other
    /// threads keep theirs: a folder's, put away too, and other folders'.
    #[test]
    fn a_thread_moves_only_within_its_folder() {
        // A: a b c | B: d | put away: e of A.
        let now = placed(&[("a", "A"), ("b", "A"), ("c", "A"), ("d", "B"), ("e", "A")]);
        assert_eq!(moved_within(&now, "A", 0, 2), placed(&[("b", "A"), ("c", "A"), ("a", "A"), ("d", "B"), ("e", "A")]));
        assert_eq!(moved_within(&now, "A", 2, 0), placed(&[("c", "A"), ("a", "A"), ("b", "A"), ("d", "B"), ("e", "A")]));
        assert_eq!(moved_within(&now, "A", 0, 9), now, "nowhere past its folder");
    }

    /// Folders dropped in a new order take their threads along; one the order lacks stays
    /// after.
    #[test]
    fn folders_take_their_threads_along() {
        let groups = vec![("A".to_string(), vec!["a".to_string(), "b".to_string()]), ("B".to_string(), vec!["c".to_string()]), ("C".to_string(), vec!["e".to_string()])];
        let order = |ids: &[&str]| ids.iter().map(|i| i.to_string()).collect::<Vec<_>>();
        assert_eq!(in_order(&groups, &order(&["B", "A", "C"])), placed(&[("c", "B"), ("a", "A"), ("b", "A"), ("e", "C")]));
        assert_eq!(in_order(&groups, &order(&["C", "A"])), placed(&[("e", "C"), ("a", "A"), ("b", "A"), ("c", "B")]));
    }

    /// A folder dragged passes another once past its middle, however tall each is; those it
    /// passes step aside by its height, toward the room it left. It goes no further than the
    /// list's ends.
    #[test]
    fn a_folder_drags_past_others_by_their_heights() {
        let list: List = ("local".into(), Some("~".into()));
        // A is short, B tall, C short.
        let folders = [("A".to_string(), 30.0), ("B".to_string(), 120.0), ("C".to_string(), 30.0)];
        let mut d = SidebarDrag::new(list, None, &folders, 0);
        d.follow(3.0);
        assert!(!d.moved(), "inside the slop it's a press");
        assert_eq!(d.dropped(), None);
        d.follow(59.0);
        assert_eq!(d.dropped(), None, "not yet past B's middle");
        assert_eq!(d.shift("B"), 0.0);
        d.follow(61.0);
        assert_eq!(d.dropped(), Some(vec!["B".into(), "A".into(), "C".into()]));
        assert_eq!((d.shift("A"), d.shift("B"), d.shift("C")), (61.0, -30.0, 0.0));
        assert!(d.lifts("A") && !d.lifts("B"));
        d.follow(136.0);
        assert_eq!(d.dropped(), Some(vec!["B".into(), "C".into(), "A".into()]), "past C's middle too");
        d.follow(400.0);
        assert_eq!(d.shift("A"), 150.0, "no further than the last place");
        d.follow(-40.0);
        assert_eq!(d.dropped(), None, "nothing above the first");
        assert_eq!(d.shift("A"), 0.0, "nor shows above it");

        let mut d = SidebarDrag::new(("local".into(), None), None, &folders, 2);
        d.follow(-14.0);
        assert_eq!(d.dropped(), None);
        d.follow(-61.0);
        assert_eq!(d.dropped(), Some(vec!["A".into(), "C".into(), "B".into()]));
        assert_eq!(d.shift("B"), 30.0, "it steps down by C's height");
    }

    /// Moved to a folder, a thread goes last in it; to one of its own, last of all.
    #[test]
    fn a_thread_moved_to_a_folder_goes_last() {
        let now = placed(&[("a", "A"), ("b", "A"), ("c", "B"), ("d", "C")]);
        assert_eq!(refiled(&now, "a", Some("B")), placed(&[("b", "A"), ("c", "B"), ("a", "B"), ("d", "C")]));
        assert_eq!(refiled(&now, "c", None), placed(&[("a", "A"), ("b", "A"), ("d", "C"), ("c", "")]));
    }

    /// A thread on its own stays so; joined by another, they get a folder, made of the one
    /// it had.
    #[test]
    fn threads_on_their_own_get_a_folder_once_joined() {
        let threads = placed(&[("t1", "=t1"), ("t2", "=t2"), ("t3", "=t2"), ("t4", "A"), ("t5", "")]);
        let got = settled(threads, |own| format!("made of {own}"));
        assert_eq!(got, placed(&[("t1", ""), ("t2", "made of =t2"), ("t3", "made of =t2"), ("t4", "A"), ("t5", "")]));
    }

    fn group<'a>(id: &str, threads: &[&'a Session]) -> Group<'a> {
        Group { id: id.into(), threads: threads.to_vec() }
    }

    /// A folder goes by two words of its own: those its id is, or those it makes, a thread's
    /// on its own those of its address, which it keeps as its id once joined. No two share
    /// them, whatever order they're in.
    #[test]
    fn every_folder_goes_by_two_words_of_its_own() {
        let words = |groups: &[Group]| word_names(groups);
        let got = words(&[group("calm-heron", &[]), group("=agent/t1", &[]), group("19a8f3c", &[])]);
        assert_eq!(got, ["calm-heron".to_string(), name_from("agent/t1"), name_from("19a8f3c")]);

        // Joined, it keeps them.
        let own = name_from("agent/t1");
        let got = settled(placed(&[("agent/t1", "=agent/t1"), ("agent/t2", "=agent/t1")]), |id| words(&[group(id, &[])])[0].clone());
        assert_eq!(got, placed(&[("agent/t1", &own), ("agent/t2", &own)]));
        assert_eq!(words(&[group(&own, &[])]), std::slice::from_ref(&own));

        // Two that would share them: the least id keeps them, whichever lists first.
        let other = (0..10_000).map(|n| format!("agent/x{n}")).find(|a| name_from(a) == own).expect("1600 names");
        let (first, second) = (format!("={other}"), "=agent/t1".to_string());
        let ab = words(&[group(&first, &[]), group(&second, &[])]);
        let ba = words(&[group(&second, &[]), group(&first, &[])]);
        assert_eq!(ab[1], own, "agent/t1 is the lesser");
        assert_ne!(ab[0], own);
        assert_eq!((ab[0].clone(), ab[1].clone()), (ba[1].clone(), ba[0].clone()), "the same whatever the order");
        // Nor do they take those a folder's id is.
        let got = words(&[group("=agent/t1", &[]), group(&own, &[])]);
        assert_ne!(got[0], own);
        assert_eq!(got[1], own);
    }
}
