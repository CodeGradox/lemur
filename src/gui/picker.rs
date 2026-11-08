//! Adding a folder, as in Zed's remote project picker: pick a host
//! (lemur's own, an SSH alias, or this Mac), then type or pick a path while
//! the folders on that host are suggested.

use super::projects::{HostChoice, host_choices};
use super::{Lemur, icons, peek};
use crate::browse::{self, Folder};
use crate::config::Host;
use rune::{El, Icon, Key as K, ScrollSpec, TextEdit, Tx, col, icon, row, text};
use rune_kit::{Kit, TextInput};
use std::collections::HashMap;
use std::time::Duration;

/// Where a path starts: the home folder.
const HOME: &str = "~/";

/// Typing a name doesn't list at every key: a listing waits this long for the next one.
const PAUSE: Duration = Duration::from_millis(80);

enum Stage {
    Host,
    Path(HostChoice),
}

/// What a row does when picked.
enum Row {
    Host(HostChoice),
    /// Adds the path as typed.
    Add(String),
    Folder(Folder),
}

pub struct Picker {
    hosts: Vec<HostChoice>,
    stage: Stage,
    pub input: TextEdit,
    selected: usize,
    /// Each listed folder of the picked host, by the path typed for it.
    listings: HashMap<String, Result<Vec<Folder>, String>>,
    /// Counts the changes to what's typed: a listing that waited out `PAUSE` goes ahead only
    /// if nothing was typed since.
    typed: u64,
    /// The change whose folder is being listed.
    listing: Option<u64>,
    /// Finding the folder to add: a git repository's top folder.
    checking: bool,
    error: Option<String>,
}

impl Picker {
    fn new(hosts: Vec<HostChoice>) -> Picker {
        let mut input = TextEdit::new("");
        input.focused = true;
        Picker { hosts, stage: Stage::Host, input, selected: 0, listings: HashMap::new(), typed: 0, listing: None, checking: false, error: None }
    }

    /// The host picked, once it is.
    fn host(&self) -> Option<&HostChoice> {
        match &self.stage {
            Stage::Host => None,
            Stage::Path(choice) => Some(choice),
        }
    }

    fn rows(&self) -> Vec<Row> {
        let text = &self.input.text;
        match &self.stage {
            Stage::Host => {
                let query = text.trim().to_lowercase();
                let mut rows: Vec<Row> = self
                    .hosts
                    .iter()
                    .filter(|c| c.host.name.to_lowercase().contains(&query) || c.host.ssh.to_lowercase().contains(&query))
                    .cloned()
                    .map(Row::Host)
                    .collect();
                // A host no one knows, like `me@build.example.com`, can be typed in full.
                if rows.is_empty() && !query.is_empty() && !query.contains(char::is_whitespace) {
                    let host = Host::new(text.trim(), text.trim());
                    rows.push(Row::Host(HostChoice { host, configured: false }));
                }
                rows
            }
            Stage::Path(_) => {
                let (dir, prefix) = split(text);
                let prefix = prefix.to_lowercase();
                let mut rows = vec![Row::Add(folder_path(text))];
                if let Some(Ok(folders)) = self.listings.get(&dir) {
                    rows.extend(
                        folders
                            .iter()
                            // Hidden folders only when you start typing one.
                            .filter(|f| !f.name.starts_with('.') || prefix.starts_with('.'))
                            .filter(|f| f.name.to_lowercase().starts_with(&prefix))
                            .cloned()
                            .map(Row::Folder),
                    );
                }
                rows
            }
        }
    }

    /// What it's doing, under the rows: checking the folder to add, or listing the one typed,
    /// or why that couldn't be listed.
    fn note(&self) -> Option<String> {
        if self.checking {
            return Some("Checking…".into());
        }
        self.host()?;
        let (dir, _) = split(&self.input.text);
        match self.listings.get(&dir) {
            Some(Err(e)) => Some(e.clone()),
            None if self.listing.is_some() => Some("Listing folders…".into()),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn row_labels(&self) -> Vec<String> {
        self.rows()
            .iter()
            .map(|row| match row {
                Row::Host(c) => c.host.name.clone(),
                Row::Add(path) => format!("add {path}"),
                Row::Folder(f) if f.repo => format!("{} (git)", f.name),
                Row::Folder(f) => f.name.clone(),
            })
            .collect()
    }
}

impl Lemur {
    /// ⌘O, or the sidebar's +: the folder picker, over the window. Its field has the keyboard.
    pub(super) fn open_picker(&mut self) {
        if self.overlaid() {
            return;
        }
        self.show_picker(host_choices(&self.cfg));
    }

    /// The picker, offering `hosts`.
    pub(super) fn show_picker(&mut self, hosts: Vec<HostChoice>) {
        self.panels.picker = Some(Picker::new(hosts));
        self.typing = true;
    }

    pub(super) fn close_picker(&mut self) {
        self.panels.picker = None;
        self.typing = false;
    }

    /// What's typed changed: the pick starts over, an error about it goes, and the folder it's
    /// in is listed, unless it's listed already.
    fn picker_changed(&mut self) {
        let Some(p) = &mut self.panels.picker else { return };
        p.selected = 0;
        p.error = None;
        p.typed += 1;
        let Stage::Path(choice) = &p.stage else { return };
        let (dir, _) = split(&p.input.text);
        if p.listings.contains_key(&dir) {
            return;
        }
        let (host, typed) = (choice.host.clone(), p.typed);
        p.listing = Some(typed);
        self.bg.after(PAUSE, move |s| s.list_folders(host, dir, typed));
    }

    /// Lists `dir` on `host`, unless more was typed while it waited.
    fn list_folders(&mut self, host: Host, dir: String, typed: u64) {
        if self.panels.picker.as_ref().is_none_or(|p| p.typed != typed) {
            return;
        }
        let (on, path) = (host.clone(), dir.clone());
        self.in_background(
            move || browse::list_folders(&on, &path),
            move |s, listed| {
                // Gone, or back to picking a host, since.
                let Some(p) = s.panels.picker.as_mut().filter(|p| p.host().is_some_and(|c| c.host == host)) else { return };
                // As Herdr's Add a machine: herdr-core asks in a terminal.
                if listed.as_ref().err().is_some_and(crate::hc::needs_you) {
                    s.close_picker();
                    s.set_up_host(host);
                    return;
                }
                p.listings.insert(dir, listed.map_err(|e| format!("{e:#}")));
                if p.listing == Some(typed) {
                    p.listing = None;
                }
            },
        );
    }

    /// ⏎: picks the selected row.
    fn picker_submit(&mut self) {
        if let Some(p) = &self.panels.picker {
            self.picker_pick(p.selected);
        }
    }

    /// Picks row `index`: a host, then a folder to go into, or the path to add.
    fn picker_pick(&mut self, index: usize) {
        let Some(p) = &mut self.panels.picker else { return };
        let Some(row) = p.rows().into_iter().nth(index) else { return };
        match row {
            Row::Host(choice) => {
                p.stage = Stage::Path(choice);
                self.picker_set_text(HOME);
            }
            Row::Folder(folder) => self.picker_enter(&folder),
            Row::Add(path) => self.picker_add(path),
        }
    }

    /// Goes into a folder: its path, then a slash, so its folders show.
    fn picker_enter(&mut self, folder: &Folder) {
        let Some(p) = &self.panels.picker else { return };
        let (dir, _) = split(&p.input.text);
        self.picker_set_text(&format!("{dir}{}/", folder.name));
    }

    fn picker_set_text(&mut self, text: &str) {
        let Some(p) = &mut self.panels.picker else { return };
        p.input.replace_text(text);
        self.picker_changed();
    }

    /// Adds the folder; one inside a git repository adds the repository's top folder.
    fn picker_add(&mut self, path: String) {
        let Some(p) = &mut self.panels.picker else { return };
        let Some(choice) = p.host().cloned() else { return };
        p.error = None;
        p.checking = true;
        let host = choice.host.clone();
        self.in_background(
            move || browse::folder_to_add(&host, &path),
            move |s, root| {
                let Some(p) = s.panels.picker.as_mut().filter(|p| p.checking) else { return };
                p.checking = false;
                match root {
                    Ok(path) => {
                        s.close_picker();
                        s.add_folder(choice, path);
                    }
                    Err(e) => p.error = Some(format!("{e:#}")),
                }
            },
        );
    }

    /// ↑ or ↓: selects the row above or below, round at the ends, and keeps it in view.
    fn picker_step(&mut self, up: bool, tx: &mut Tx) {
        let Some(p) = &mut self.panels.picker else { return };
        let len = p.rows().len().max(1);
        p.selected = if up { (p.selected + len - 1) % len } else { (p.selected + 1) % len };
        tx.scroll_into_view(format!("picker-row-{}", p.selected));
    }

    /// Tab: goes into the selected folder, or picks the selected host.
    fn picker_complete(&mut self) {
        let Some(p) = &self.panels.picker else { return };
        let selected = p.selected;
        match p.rows().into_iter().nth(selected) {
            Some(Row::Folder(folder)) => self.picker_enter(&folder),
            Some(Row::Host(_)) => self.picker_pick(selected),
            _ => {}
        }
    }
}

/// Splits a typed path into the folder it's in and the start of a name: `~/projects/la` is
/// `~/projects/` and `la`. A bare name is in home.
fn split(text: &str) -> (String, String) {
    match text.rsplit_once('/') {
        Some((dir, name)) => (format!("{dir}/"), name.to_string()),
        None if text == "~" => (HOME.into(), String::new()),
        None => (String::new(), text.to_string()),
    }
}

/// The folder a typed path names, without a trailing slash.
fn folder_path(text: &str) -> String {
    let path = text.trim().trim_end_matches('/');
    match path {
        "" if text.starts_with('/') => "/".into(),
        "" => "~".into(),
        path => path.into(),
    }
}

fn host_icon(host: &Host) -> &'static Icon {
    if host.ssh == "local" { &icons::MONITOR } else { &icons::SERVER }
}

fn host_label(host: &Host) -> String {
    if host.ssh == "local" { "This Mac".into() } else { host.name.clone() }
}

/// The picker over the window, as the spotlight is: the host picked, the field, the rows, and
/// what it's doing or why it can't.
pub fn overlay(s: &Lemur, k: &Kit) -> Option<El> {
    let p = s.panels.picker.as_ref()?;
    let c = &k.c;
    let mut card = col()
        .key("picker")
        .w(560.0)
        .radius(k.radius.lg)
        .fill(c.surface_raised)
        .border(1.0, c.border_base)
        .shadow(12.0, 40.0, c.scrim)
        .clip_rounded(k.radius.lg);
    if let Some(choice) = p.host() {
        card = card.child(
            row()
                .key("picker-host")
                .px(12.0)
                .h(34.0)
                .gap(8.0)
                .center_y()
                .child(icon(host_icon(&choice.host), 14.0).color(c.icon_muted))
                .child(text(host_label(&choice.host)).font(k.ty.ui_medium).color(c.text_base).truncate()),
        );
    }
    let input = TextInput::new(&p.input)
        .placeholder(if p.host().is_some() { "Path" } else { "Search hosts" })
        .width(None)
        .plain()
        .on_edit(|s: &mut Lemur| &mut s.panels.picker.as_mut().expect("a picker").input)
        .on_change(|s: &mut Lemur, _| s.picker_changed())
        .on_submit(|s: &mut Lemur, _| s.picker_submit())
        .on_key(|s: &mut Lemur, ev, tx| match ev.key {
            K::Up => {
                s.picker_step(true, tx);
                true
            }
            K::Down => {
                s.picker_step(false, tx);
                true
            }
            K::Tab if !ev.mods.shift => {
                s.picker_complete();
                true
            }
            K::Escape => {
                s.close_picker();
                true
            }
            _ => false,
        })
        .build(k)
        .key("picker-input");
    card = card.child(row().px(8.0).py(6.0).border_sides([0.0, 0.0, 1.0, 0.0], c.border_weak).child(input));
    let rows = p.rows();
    let selected = p.selected.min(rows.len().saturating_sub(1));
    let mut list = col().key("picker-rows").w_full().max_h(320.0).padding(4.0).gap(2.0).scroll(ScrollSpec { stick_bottom: false });
    for (i, r) in rows.into_iter().enumerate() {
        let (glyph, label, detail) = match r {
            Row::Host(choice) => {
                let detail = if !choice.configured && !p.hosts.contains(&choice) {
                    Some("connect".to_string())
                } else {
                    (choice.host.ssh != choice.host.name && choice.host.ssh != "local").then(|| choice.host.ssh.clone())
                };
                (host_icon(&choice.host), host_label(&choice.host), detail)
            }
            Row::Add(path) => (&icons::PLUS, format!("Add {path}"), None),
            Row::Folder(f) => (&icons::FOLDER, f.name, f.repo.then(|| "git".into())),
        };
        let picked = i == selected;
        let ink = if picked { c.text_contrast } else { c.text_base };
        let mut item = row()
            .key(format!("picker-row-{i}"))
            .px(8.0)
            .h(28.0)
            .gap(8.0)
            .center_y()
            .radius(k.radius.sm)
            .cursor(rune::Cursor::Pointer)
            .child(icon(glyph, 14.0).color(if picked { c.text_contrast } else { c.icon_muted }))
            .child(text(label).font(k.ty.ui).color(ink).truncate().grow(1.0));
        if let Some(d) = detail {
            item = item.child(text(d).font(k.ty.small).color(if picked { c.text_contrast } else { c.text_faint }));
        }
        let item = if picked { item.fill(c.bg_accent) } else { k.interactive(item) };
        list = list.child(item.on_press(move |s: &mut Lemur| s.picker_pick(i)));
    }
    card = card.child(list);
    let line = p.error.clone().map(|e| (e, true)).or_else(|| p.note().map(|n| (n, false)));
    if let Some((line, error)) = line {
        card = card.child(
            row()
                .key("picker-footer")
                .px(12.0)
                .py(6.0)
                .min_h(28.0)
                .center_y()
                .fill(c.layer_01)
                .child(text(line).font(k.ty.small).color(if error { c.danger.fg } else { c.text_faint }).wrap()),
        );
    }
    let place = |card: El| peek::centred(col().pt(80.0).child(card));
    Some(peek::over_window("picker", card, place, |s: &mut Lemur| s.close_picker(), k))
}

#[cfg(test)]
#[path = "picker_tests.rs"]
mod tests;
