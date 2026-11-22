//! Folders of a host's threads, in the sidebar, as Rex keeps a host's sessions: every thread
//! is in one. A shell started as a new folder (⇧⌘N, a host's +) has one of its own; a thread
//! started beside another (⌘K, ⌘T, ⌘J) goes in that one's. A folder you haven't named goes
//! by two words of its own, as Rex's sessions do (`calm-heron`), or, as Settings has it, by
//! the folder of the worktree its Claude works in. Dragged, a thread takes another place, in another folder or, above them all, one
//! of its own; a folder another place among the folders, its threads with it. herdr-core
//! keeps which folder each thread is in (`lemur.folder`) and its place (`lemur.order`);
//! Lemur's config the folders you named or folded.

use super::{Key, Lemur, List};
use crate::actions::name_from;
use crate::config::{ClaudeThread, Config, Folder, FolderNames, leaf};
use crate::snapshot::{HostView, Session, find_worktree, rank};
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

/// A row a drag moves among: a thread's, by its name, or a folder's, by its id.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Thread(String),
    Folder(String),
}

/// Host `h`'s threads, from every project and none, in their order.
pub fn listed(h: &HostView) -> Listed<'_> {
    let worktrees = || h.bases.iter().flat_map(|b| &b.worktrees);
    let mut all: Vec<&Session> = worktrees().flat_map(|w| &w.sessions).chain(&h.other).collect();
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
    Listed { groups, away, gone: worktrees().flat_map(|w| &w.gone).chain(&h.gone).collect() }
}

impl Listed<'_> {
    /// The rows a drag moves among, in order: each folder's row and, unless `folded`, its
    /// threads.
    pub fn items(&self, folded: impl Fn(&str) -> bool) -> Vec<Item> {
        let mut items = vec![];
        for g in &self.groups {
            items.push(Item::Folder(g.id.clone()));
            if !folded(&g.id) {
                items.extend(g.threads.iter().map(|s| Item::Thread(s.name.clone())));
            }
        }
        items
    }

    /// The threads of the `folded` folders, by folder: no rows of theirs show.
    pub fn hidden(&self, folded: impl Fn(&str) -> bool) -> HashMap<String, Vec<String>> {
        self.groups
            .iter()
            .filter(|g| folded(&g.id))
            .map(|g| (g.id.clone(), g.threads.iter().map(|s| s.name.clone()).collect()))
            .collect()
    }

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

/// Where the threads go once row `from` of `items` is dragged to `to`: each shown thread in
/// its order with its folder, `""` above every folder's row, for one of its own. A thread is
/// in the folder whose row is the nearest above it; `hidden` has those of folded folders,
/// which keep theirs, after its row. A folder dragged takes its threads along.
pub fn moved(items: &[Item], hidden: &HashMap<String, Vec<String>>, from: usize, to: usize) -> Vec<(String, String)> {
    let filed = |rows: &[Item]| {
        let mut folder = String::new();
        let mut out: Vec<(String, String)> = vec![];
        for row in rows {
            match row {
                Item::Folder(id) => {
                    folder = id.clone();
                    out.extend(hidden.get(id).into_iter().flatten().map(|t| (t.clone(), id.clone())));
                }
                Item::Thread(name) => out.push((name.clone(), folder.clone())),
            }
        }
        out
    };
    let mut after = items.to_vec();
    if from >= after.len() || to >= after.len() {
        return filed(items);
    }
    let row = after.remove(from);
    let folder_moved = matches!(row, Item::Folder(_));
    after.insert(to, row);
    if !folder_moved {
        return filed(&after);
    }
    // Its threads go with it, wherever they showed.
    let before = filed(items);
    let mut threads: Vec<(String, String)> = before.iter().filter(|(_, f)| f.is_empty()).cloned().collect();
    for id in after.iter().filter_map(|r| match r {
        Item::Folder(id) => Some(id),
        Item::Thread(_) => None,
    }) {
        threads.extend(before.iter().filter(|(_, f)| f == id).cloned());
    }
    threads
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

    /// The host whose folders the sidebar shows, as Rex shows a host's sessions, one at a
    /// time: the thread on show's, else the one picked or shown last, else the first
    /// reached.
    pub(super) fn shown_host(&self) -> Option<&HostView> {
        let wanted = self.active.as_ref().map(|(h, _)| h).or(self.host_shown.as_ref());
        wanted
            .and_then(|h| self.host_view(h))
            .or_else(|| self.recent.iter().find_map(|(h, _)| self.host_view(h)))
            .or_else(|| self.snap.hosts.iter().find(|h| h.error.is_none()))
            .or(self.snap.hosts.first())
    }

    /// Shows host `host`'s folders in the sidebar, and the thread of its opened last, else
    /// its first; with none, nothing till one starts.
    pub(super) fn show_host(&mut self, host: &str) {
        if self.shown_host().is_some_and(|h| h.name == host) {
            return;
        }
        let Some(h) = self.host_view(host) else { return };
        let threads: Vec<String> = listed(h).groups.iter().flat_map(|g| g.threads.iter().map(|s| s.name.clone())).collect();
        let last = self.recent.iter().find(|(x, s)| x == host && threads.contains(s)).map(|(_, s)| s.clone());
        if let Some(session) = last.or_else(|| threads.first().cloned()) {
            return self.open(host.to_string(), session);
        }
        self.hide_shown(None);
        self.active = None;
        self.host_shown = Some(host.to_string());
        self.follow_shown();
    }

    /// The list a host's rows are, as dragging keys it.
    pub(super) fn host_list(host: &str) -> List {
        (host.to_string(), None)
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
        let titles = groups.iter().zip(word_names(groups)).map(|(g, words)| match self.prefs.folder_names {
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
        let groups = listed(h).groups;
        let Some(at) = groups.iter().position(|g| g.id == id) else { return String::new() };
        self.folder_titles(h, &groups).swap_remove(at)
    }

    /// Thread `name`'s row among the rows a drag moves in `list`, folders' rows counted.
    pub(super) fn row_of(&self, (host, _): &List, name: &str) -> Option<usize> {
        let l = listed(self.host_view(host)?);
        l.items(|id| self.folded(host, id)).iter().position(|i| *i == Item::Thread(name.to_string()))
    }

    /// The threads of the folder thread `key` is in, none put away.
    pub(super) fn group_of(&self, key: &Key) -> Vec<String> {
        let (Some(h), Some(s)) = (self.host_view(&key.0), self.session_of(key)) else { return vec![] };
        let id = folder_of(s);
        let l = listed(h);
        l.groups.iter().find(|g| g.id == id).map(|g| g.threads.iter().map(|s| s.name.clone()).collect()).unwrap_or_default()
    }

    /// Row `from` of `host`'s rows was dragged to `to`: a thread takes its new place, in a
    /// folder or one of its own, or a folder its place among the folders.
    pub(super) fn drop_row(&mut self, host: &str, from: usize, to: usize) {
        let Some(h) = self.host_view(host) else { return };
        let l = listed(h);
        let folded = |id: &str| self.folded(host, id);
        let mut threads = moved(&l.items(folded), &l.hidden(folded), from, to);
        threads.extend(l.away.iter().map(|s| (s.name.clone(), folder_of(s))));
        self.place_threads(host, threads);
    }

    /// Shows `host`'s threads in `threads`' order and folders at once, and has herdr-core
    /// keep them.
    fn place_threads(&mut self, host: &str, threads: Vec<(String, String)>) {
        let words = self.word_names(host);
        let mut taken: Vec<String> = threads.iter().map(|(_, f)| f.clone()).collect();
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
        self.write_places(Self::host_list(host), threads);
    }

    /// Puts thread `key` at the end of folder `folder` of its host's, or with `None` in a
    /// folder of its own, last.
    pub(super) fn move_to_folder(&mut self, key: &Key, folder: Option<&str>) {
        let Some(h) = self.host_view(&key.0) else { return };
        let threads = refiled(&listed(h).placed(), &key.1, folder);
        let host = key.0.clone();
        self.place_threads(&host, threads);
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
        let groups = listed(h).groups;
        groups.iter().map(|g| g.id.clone()).zip(word_names(&groups)).collect()
    }

    /// Gives thread `key`, on its own, a folder of its own, by the two words its folder goes
    /// by already: it shows at once, and herdr-core keeps it.
    fn own_folder(&mut self, key: &Key) -> String {
        let used: Vec<String> = self.host_view(&key.0).map(|h| listed(h).placed().into_iter().map(|(_, f)| f).collect()).unwrap_or_default();
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
            let used: Vec<String> = listed(h).placed().into_iter().map(|(_, f)| f).collect();
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

    fn t(name: &str) -> Item {
        Item::Thread(name.into())
    }

    fn f(id: &str) -> Item {
        Item::Folder(id.into())
    }

    fn placed(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(n, f)| (n.to_string(), f.to_string())).collect()
    }

    /// A thread dragged below a folder's row is in it; above every folder's, in one of its
    /// own. A folded folder's threads keep theirs.
    #[test]
    fn a_thread_is_in_the_folder_above_it() {
        // A: a b | B: c d | C (folded: e)
        let items = [f("A"), t("a"), t("b"), f("B"), t("c"), t("d"), f("C")];
        let hidden = HashMap::from([("C".to_string(), vec!["e".to_string()])]);
        let threads = moved(&items, &hidden, 1, 4);
        assert_eq!(threads, placed(&[("b", "A"), ("c", "B"), ("a", "B"), ("d", "B"), ("e", "C")]));
        let threads = moved(&items, &hidden, 5, 0);
        assert_eq!(threads, placed(&[("d", ""), ("a", "A"), ("b", "A"), ("c", "B"), ("e", "C")]), "one of its own");
        let threads = moved(&items, &hidden, 2, 6);
        assert_eq!(threads, placed(&[("a", "A"), ("c", "B"), ("d", "B"), ("e", "C"), ("b", "C")]), "into a folded one, last");
    }

    /// A folder dragged takes its threads along, wherever it's dropped.
    #[test]
    fn a_folder_takes_its_threads_along() {
        let items = [f("A"), t("a"), f("B"), t("c"), f("C"), t("e")];
        let threads = moved(&items, &HashMap::new(), 2, 0);
        assert_eq!(threads, placed(&[("c", "B"), ("a", "A"), ("e", "C")]));
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
