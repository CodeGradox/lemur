//! Folders of a project's threads, in the sidebar. A project lists its threads from all its
//! worktrees as one list: those in no folder first, then each folder of yours with its own.
//! Dragged, a thread takes another place among them, in a folder or out of one, and a
//! folder another place among the folders; a thread's right-click moves it to one. Lemur
//! keeps the folders in its config, and herdr-core which one each thread is in
//! (`lemur.folder`), with its place (`lemur.order`).

use super::{Key, Lemur, List};
use crate::config::{Base, ClaudeThread, Folder};
use crate::snapshot::{BaseView, Session, rank};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// A project's threads as the sidebar lists them.
pub struct Listed<'a> {
    /// Those in no folder, then each folder with its own; none put away.
    pub loose: Vec<&'a Session>,
    pub folders: Vec<(&'a Folder, Vec<&'a Session>)>,
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

/// Project `base`'s threads in `b`, in their order. One in a folder that's gone is in none.
pub fn listed<'a>(base: &'a Base, b: &'a BaseView) -> Listed<'a> {
    let mut all: Vec<&Session> = b.worktrees.iter().flat_map(|w| &w.sessions).collect();
    all.sort_by(|x, y| rank(x).cmp(&rank(y)));
    let (away, shown): (Vec<&Session>, Vec<&Session>) = all.into_iter().partition(|s| s.away);
    let of = |id: &str| shown.iter().copied().filter(|s| s.folder.as_deref() == Some(id)).collect();
    let folders: Vec<(&Folder, Vec<&Session>)> = base.folders.iter().map(|f| (f, of(&f.id))).collect();
    let filed = |s: &Session| s.folder.as_deref().is_some_and(|id| base.folders.iter().any(|f| f.id == id));
    let loose = shown.iter().copied().filter(|s| !filed(s)).collect();
    Listed { loose, folders, away, gone: b.worktrees.iter().flat_map(|w| &w.gone).collect() }
}

impl Listed<'_> {
    /// The rows a drag moves among, in order: the threads in no folder, then each folder's
    /// row and, open, its threads.
    pub fn items(&self) -> Vec<Item> {
        let mut items: Vec<Item> = self.loose.iter().map(|s| Item::Thread(s.name.clone())).collect();
        for (f, threads) in &self.folders {
            items.push(Item::Folder(f.id.clone()));
            if !f.collapsed {
                items.extend(threads.iter().map(|s| Item::Thread(s.name.clone())));
            }
        }
        items
    }

    /// The threads of the folded folders, by folder: no rows of theirs show.
    pub fn hidden(&self) -> HashMap<String, Vec<String>> {
        self.folders
            .iter()
            .filter(|(f, _)| f.collapsed)
            .map(|(f, threads)| (f.id.clone(), threads.iter().map(|s| s.name.clone()).collect()))
            .collect()
    }

    /// Every thread in its order, with its folder (`""` for none): those shown, then those
    /// put away, which keep theirs.
    pub fn placed(&self) -> Vec<(String, String)> {
        let mut placed: Vec<(String, String)> = self.loose.iter().map(|s| (s.name.clone(), String::new())).collect();
        for (f, threads) in &self.folders {
            placed.extend(threads.iter().map(|s| (s.name.clone(), f.id.clone())));
        }
        placed.extend(self.away.iter().map(|s| (s.name.clone(), s.folder.clone().unwrap_or_default())));
        placed
    }
}

/// Where the threads go once row `from` of `items` is dragged to `to`: each shown thread in
/// its order with its folder (`""` for none), and with a folder dragged, the folders' new
/// order. A thread is in the folder whose row is the nearest above it; `hidden` has those
/// of folded folders, which keep theirs, after its row.
pub fn moved(items: &[Item], hidden: &HashMap<String, Vec<String>>, from: usize, to: usize) -> (Vec<(String, String)>, Option<Vec<String>>) {
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
        return (filed(items), None);
    }
    let row = after.remove(from);
    let folder_moved = matches!(row, Item::Folder(_));
    after.insert(to, row);
    if !folder_moved {
        return (filed(&after), None);
    }
    // Its threads go with it, wherever they showed.
    let before = filed(items);
    let order: Vec<String> = after
        .iter()
        .filter_map(|r| match r {
            Item::Folder(id) => Some(id.clone()),
            Item::Thread(_) => None,
        })
        .collect();
    let mut threads: Vec<(String, String)> = before.iter().filter(|(_, f)| f.is_empty()).cloned().collect();
    for id in &order {
        threads.extend(before.iter().filter(|(_, f)| f == id).cloned());
    }
    (threads, Some(order))
}

/// `placed` with thread `name` moved to the end of `folder` (`""` for none): those in no
/// folder first, then each of `folders`' threads, in their order.
fn refiled(placed: &[(String, String)], folders: &[Folder], name: &str, folder: &str) -> Vec<(String, String)> {
    let mut all: Vec<(String, String)> = placed.iter().filter(|(n, _)| n != name).cloned().collect();
    all.push((name.to_string(), folder.to_string()));
    let mut out: Vec<(String, String)> = all.iter().filter(|(_, f)| !folders.iter().any(|x| x.id == *f)).map(|(n, _)| (n.clone(), String::new())).collect();
    for f in folders {
        out.extend(all.iter().filter(|(_, x)| *x == f.id).cloned());
    }
    out
}

/// A folder id `base` has none of yet.
fn new_id(base: &Base) -> String {
    let mut n = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    loop {
        let id = format!("{n:x}");
        if !base.folders.iter().any(|f| f.id == id) {
            return id;
        }
        n += 1;
    }
}

/// What a folder's name may be: something, and not too long for its row.
pub fn validate_folder_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name.chars().count() > 60 {
        anyhow::bail!("a folder's name has 1 to 60 characters");
    }
    Ok(())
}

impl Lemur {
    /// Project `index` as the snapshot has it, with its config.
    pub(super) fn project_view(&self, index: usize) -> Option<(&Base, &BaseView)> {
        let base = self.cfg.bases.get(index)?;
        let b = self.snap.hosts.iter().filter(|h| h.name == base.host).flat_map(|h| &h.bases).find(|b| b.index == index)?;
        Some((base, b))
    }

    /// Project `index`'s list of threads, as the sidebar keys it: its host and folder.
    pub(super) fn project_list(&self, index: usize) -> List {
        let base = &self.cfg.bases[index];
        (base.host.clone(), Some(base.path.clone()))
    }

    /// The project whose list `list` is, if it's one's.
    pub(super) fn project_at(&self, (host, path): &List) -> Option<usize> {
        let path = path.as_ref()?;
        self.cfg.bases.iter().position(|b| b.host == *host && b.path == *path)
    }

    /// Thread `name`'s row among the rows a drag moves in `list`: a project's, folders' rows
    /// counted, or a host's threads outside every project.
    pub(super) fn row_of(&mut self, list: &List, name: &str) -> Option<usize> {
        match self.project_at(list) {
            Some(index) => {
                let (base, b) = self.project_view(index)?;
                listed(base, b).items().iter().position(|i| *i == Item::Thread(name.to_string()))
            }
            None => self.list_mut(list)?.iter().position(|s| s.name == name),
        }
    }

    /// The threads of the folder thread `key` is in, or of its project's threads in none;
    /// None outside every project.
    pub(super) fn group_of(&self, key: &Key) -> Option<Vec<String>> {
        let (index, _) = self.project_of(&key.0, &key.1)?;
        let (base, b) = self.project_view(index)?;
        let l = listed(base, b);
        let folder = self.session_of(key).and_then(|s| s.folder.clone());
        let threads = l.folders.iter().find(|(f, _)| Some(&f.id) == folder.as_ref()).map_or(&l.loose, |(_, t)| t);
        Some(threads.iter().map(|s| s.name.clone()).collect())
    }

    /// Row `from` of project `index`'s rows was dragged to `to`: a thread takes its new
    /// place, in a folder or none, or a folder its place among the folders.
    pub(super) fn drop_row(&mut self, index: usize, from: usize, to: usize) {
        let Some((base, b)) = self.project_view(index) else { return };
        let l = listed(base, b);
        let away: Vec<(String, String)> = l.away.iter().map(|s| (s.name.clone(), s.folder.clone().unwrap_or_default())).collect();
        let (mut threads, folders) = moved(&l.items(), &l.hidden(), from, to);
        threads.extend(away);
        if let Some(order) = folders {
            let base = &mut self.cfg.bases[index];
            base.folders.sort_by_key(|f| order.iter().position(|id| *id == f.id));
            self.save_look(index);
        }
        self.place_threads(index, threads);
    }

    /// Shows project `index`'s threads in `threads`' order and folders at once, and has
    /// herdr-core keep them.
    fn place_threads(&mut self, index: usize, threads: Vec<(String, String)>) {
        let host = self.cfg.bases[index].host.clone();
        for (i, (name, folder)) in threads.iter().enumerate() {
            if let Some(s) = self.session_mut(&(host.clone(), name.clone())) {
                s.order = Some(i as u32);
                s.folder = (!folder.is_empty()).then(|| folder.clone());
            }
        }
        let list = self.project_list(index);
        self.write_places(list, threads);
    }

    /// Puts thread `key` at the end of folder `folder` of its project, or with `None` at the
    /// end of those in no folder.
    pub(super) fn move_to_folder(&mut self, key: &Key, folder: Option<&str>) {
        let Some((index, _)) = self.project_of(&key.0, &key.1) else { return };
        let Some((base, b)) = self.project_view(index) else { return };
        let threads = refiled(&listed(base, b).placed(), &base.folders, &key.1, folder.unwrap_or_default());
        self.place_threads(index, threads);
        self.reveal(&key.0, &key.1);
    }

    /// Makes folder `name` at the end of project `index`'s, with thread `thread` in it.
    pub(super) fn new_folder(&mut self, index: usize, name: String, thread: Option<Key>) {
        let Some(base) = self.cfg.bases.get_mut(index) else { return };
        let id = new_id(base);
        base.folders.push(Folder { id: id.clone(), name, collapsed: false });
        self.save_look(index);
        if let Some(key) = thread {
            self.move_to_folder(&key, Some(&id));
        }
    }

    /// The folder of its project thread `key` is in, by its id, for a thread started beside
    /// it: none outside every project, or in no folder.
    pub(super) fn folder_beside(&self, key: &Key) -> Option<String> {
        let (index, _) = self.project_of(&key.0, &key.1)?;
        let folder = self.session_of(key)?.folder.clone()?;
        self.cfg.bases[index].folders.iter().any(|f| f.id == folder).then_some(folder)
    }

    /// What folder `id` of project `index` is called.
    pub(super) fn folder_name(&self, index: usize, id: &str) -> Option<String> {
        self.cfg.bases.get(index)?.folders.iter().find(|f| f.id == id).map(|f| f.name.clone())
    }

    pub(super) fn rename_folder(&mut self, index: usize, id: &str, name: String) {
        let Some(f) = self.cfg.bases.get_mut(index).and_then(|b| b.folders.iter_mut().find(|f| f.id == id)) else { return };
        f.name = name;
        self.save_look(index);
    }

    /// Takes folder `id` away; its threads stay, in no folder, after those there.
    pub(super) fn delete_folder(&mut self, index: usize, id: &str) {
        let Some((base, b)) = self.project_view(index) else { return };
        let placed = listed(base, b).placed();
        self.cfg.bases[index].folders.retain(|f| f.id != id);
        self.save_look(index);
        let folders = self.cfg.bases[index].folders.clone();
        // Those in no folder keep their places; the folder's follow them.
        let mut threads: Vec<(String, String)> = placed.iter().filter(|(_, f)| f.is_empty()).cloned().collect();
        threads.extend(placed.iter().filter(|(_, f)| f == id).map(|(n, _)| (n.clone(), String::new())));
        for f in &folders {
            threads.extend(placed.iter().filter(|(_, x)| *x == f.id).cloned());
        }
        self.place_threads(index, threads);
    }

    /// Folds folder `id` of project `index` to its row, or opens it.
    pub(super) fn toggle_folder(&mut self, index: usize, id: &str) {
        let Some(f) = self.cfg.bases.get_mut(index).and_then(|b| b.folders.iter_mut().find(|f| f.id == id)) else { return };
        f.collapsed = !f.collapsed;
        self.save_look(index);
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

    /// A thread dragged below a folder's row is in it; above every folder's, in none. A
    /// folded folder's threads keep theirs.
    #[test]
    fn a_thread_is_in_the_folder_above_it() {
        // a b | A: c d | B (folded: e)
        let items = [t("a"), t("b"), f("A"), t("c"), t("d"), f("B")];
        let hidden = HashMap::from([("B".to_string(), vec!["e".to_string()])]);
        let (threads, folders) = moved(&items, &hidden, 0, 3);
        assert_eq!(threads, placed(&[("b", ""), ("c", "A"), ("a", "A"), ("d", "A"), ("e", "B")]));
        assert_eq!(folders, None);
        let (threads, _) = moved(&items, &hidden, 4, 0);
        assert_eq!(threads, placed(&[("d", ""), ("a", ""), ("b", ""), ("c", "A"), ("e", "B")]), "out of its folder");
        let (threads, _) = moved(&items, &hidden, 1, 5);
        assert_eq!(threads, placed(&[("a", ""), ("c", "A"), ("d", "A"), ("e", "B"), ("b", "B")]), "into a folded one, last");
    }

    /// A folder dragged takes its threads along, wherever it's dropped.
    #[test]
    fn a_folder_takes_its_threads_along() {
        let items = [t("a"), f("A"), t("c"), f("B"), t("e")];
        let (threads, folders) = moved(&items, &HashMap::new(), 1, 4);
        assert_eq!(folders, Some(vec!["B".to_string(), "A".to_string()]));
        assert_eq!(threads, placed(&[("a", ""), ("e", "B"), ("c", "A")]));
    }

    /// Moved to a folder, a thread goes last in it; to none, last of those in none.
    #[test]
    fn a_thread_moved_to_a_folder_goes_last() {
        let folders = [Folder { id: "A".into(), name: "A".into(), collapsed: false }];
        let now = placed(&[("a", ""), ("b", ""), ("c", "A")]);
        assert_eq!(refiled(&now, &folders, "a", "A"), placed(&[("b", ""), ("c", "A"), ("a", "A")]));
        assert_eq!(refiled(&now, &folders, "c", ""), placed(&[("a", ""), ("b", ""), ("c", "")]));
    }
}
