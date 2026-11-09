//! The terminal's pointer and clipboard: selecting, copying and pasting, files and images
//! going up to the host, links opening on a ⌘-click, a right click on a file's path, the
//! wheel, and the mouse for a program that asks for it.

use super::snackbar::Kind;
use super::term_links::{Link, hyperlinked, link_at};
use super::terminal::PAD;
use super::{Key, Lemur};
use crate::actions;
use libghostty_vt::mouse::{Action as MouseAction, Button};
use rune::{Drag, FileDialogOptions, Ime, Phase, PointerEvent, PopupItem, PopupMenu, Tx, WheelEvent};
use std::path::{Path, PathBuf};

impl Lemur {
    /// The terminal at the front: one peeked at, else the one on show.
    fn front(&self) -> Option<Key> {
        self.peek.clone().or_else(|| self.active.clone())
    }

    fn cell(&self) -> (f32, f32) {
        (self.metrics.cell_w, self.metrics.cell_h)
    }

    /// The link under (`x`, `y`) in the terminal at the front.
    fn link_under(&self, x: f32, y: f32) -> Option<Link> {
        let view = self.terminals.get(&self.front()?)?;
        let grid = view.grid.as_ref()?;
        let (col, row) = grid.cell_at(x, y, self.cell());
        let cells = grid.line_cells(row);
        if let Some(uri) = grid.hyperlink(col, row) {
            let here = cells.iter().rposition(|&(_, r, c)| (r, c) <= (row, col))?;
            let same = |r: u16, c: u16| grid.hyperlink(c, r).as_deref() == Some(uri.as_str());
            return hyperlinked(uri.clone(), &cells, here, same);
        }
        link_at(&cells, (row, col))
    }

    /// A press, a drag and a release on the terminal: ⌘-click opens a link; a program that
    /// asked for the mouse gets it (⇧ selects anyway); otherwise it selects, by words on a
    /// double click and lines on a triple one.
    pub(super) fn term_drag(&mut self, d: &Drag, tx: &mut Tx) {
        let Some(key) = self.front() else { return };
        let cell = self.cell();
        let (x, y) = (d.x - PAD, d.y - PAD);
        let mods = tx.mods();
        if d.phase == Phase::Down {
            // A click on the terminal gives it the keyboard back.
            if self.editing() {
                tx.blur();
            }
            if self.terminals.get(&key).is_some_and(|v| v.exited()) {
                self.connect(&key);
                return;
            }
            if mods.cmd
                && let Some(link) = self.link_under(x, y)
            {
                if link.file {
                    self.preview(key, link.url);
                } else {
                    let _ = tx.open_url(&link.url);
                }
                return;
            }
        }
        let Some(view) = self.terminals.get_mut(&key) else { return };
        let Some(grid) = view.grid.as_mut() else { return };
        match d.phase {
            Phase::Down if grid.mouse_tracking() && !mods.shift => {
                let at = grid.cell_at(x, y, cell);
                view.pressed = Some(at);
                let report = grid.mouse_report(MouseAction::Press, Some(Button::Left), (x, y), mods, cell, true);
                view.write(report);
            }
            Phase::Down => {
                let _ = grid.select_press(x, y, cell);
                view.selecting = true;
            }
            Phase::Move if view.selecting => {
                let _ = grid.select_drag(x, y, cell, mods.alt);
            }
            Phase::Move => {
                // Only a program that asked for moves gets them; the encoder knows.
                if let Some(at) = view.pressed {
                    let now = grid.cell_at(x, y, cell);
                    if now != at {
                        view.pressed = Some(now);
                        let report = grid.mouse_report(MouseAction::Motion, Some(Button::Left), (x, y), mods, cell, true);
                        view.write(report);
                    }
                }
            }
            Phase::Up if view.selecting => {
                view.selecting = false;
                let _ = grid.select_release(x, y, cell);
            }
            Phase::Up => {
                if view.pressed.take().is_some() {
                    let report = grid.mouse_report(MouseAction::Release, Some(Button::Left), (x, y), mods, cell, false);
                    view.write(report);
                }
            }
        }
        view.drawn_at = None;
    }

    /// A right click on a file's path offers what can be done with it; a program that asked
    /// for the mouse gets the click otherwise.
    pub(super) fn term_secondary(&mut self, e: PointerEvent, tx: &mut Tx) {
        let Some(key) = self.front() else { return };
        let (x, y) = (e.x - PAD, e.y - PAD);
        if let Some(link) = self.link_under(x, y).filter(|l| l.file) {
            let menu = PopupMenu::new()
                .item(PopupItem::new("preview", "Preview"))
                .item(PopupItem::new("zed", "Open in Zed"))
                .item(PopupItem::new("download", "Download"));
            tx.popup_menu(menu, e.window(), move |s: &mut Lemur, picked, tx| match picked {
                Some("preview") => s.preview(key, link.url),
                Some("zed") => s.open_in_zed(key, link.url, link.place),
                Some("download") => s.download(key, link.url, tx),
                _ => {}
            });
            return;
        }
        let cell = self.cell();
        let mods = tx.mods();
        if let Some(view) = self.terminals.get_mut(&key)
            && let Some(grid) = view.grid.as_mut()
            && grid.mouse_tracking()
        {
            let mut report = grid.mouse_report(MouseAction::Press, Some(Button::Right), (x, y), mods, cell, true);
            report.extend(grid.mouse_report(MouseAction::Release, Some(Button::Right), (x, y), mods, cell, false));
            view.write(report);
        }
    }

    /// While ⌘ is held, the link under the pointer is underlined, and a click opens it.
    pub(super) fn term_hover(&mut self, e: PointerEvent, tx: &mut Tx) {
        let link = if tx.mods().cmd { self.link_under(e.x - PAD, e.y - PAD) } else { None };
        if let Some(view) = self.front().and_then(|k| self.terminals.get_mut(&k))
            && view.hover != link
        {
            view.hover = link;
        }
    }

    /// The wheel: to a program that asked for the mouse; as arrow keys on the alternate
    /// screen of one that asked for that (a pager, an editor); otherwise back through the
    /// scrollback.
    pub(super) fn term_wheel(&mut self, e: &WheelEvent, tx: &mut Tx) -> bool {
        let cell = self.cell();
        let Some(view) = self.front().and_then(|k| self.terminals.get_mut(&k)) else { return false };
        let Some(grid) = view.grid.as_mut() else { return false };
        view.scroll_rest += e.dy / cell.1;
        let lines = view.scroll_rest.trunc();
        if lines == 0.0 {
            return true;
        }
        view.scroll_rest -= lines;
        // Content moving down is the wheel going up, back in time.
        let up = lines > 0.0;
        let notches = lines.abs() as usize;
        let (x, y) = (e.x - PAD, e.y - PAD);
        if grid.mouse_tracking() {
            let button = if up { Button::Four } else { Button::Five };
            let report = grid.mouse_report(MouseAction::Press, Some(button), (x, y), tx.mods(), cell, false);
            view.write(report.repeat(notches));
        } else if grid.wheel_as_arrows() {
            let arrow = grid.arrow(up);
            view.write(arrow.repeat(notches));
        } else {
            grid.scroll(-(lines as isize));
            view.drawn_at = None;
        }
        true
    }

    /// ⌘C on the terminal at the front: what's selected goes to the clipboard.
    pub(super) fn copy_selection(&mut self, tx: &mut Tx) -> bool {
        let Some(text) = self.front().and_then(|k| self.terminals.get(&k)).and_then(|v| v.grid.as_ref()?.selected_text()) else {
            return false;
        };
        tx.copy(text);
        true
    }

    /// ⌘V: files and images go up to the host and paste as their paths there; text pastes
    /// as it is, and a link to a Linear issue pins it.
    pub(super) fn paste_clipboard(&mut self) {
        let Some(key) = self.front() else { return };
        let Ok(mut board) = arboard::Clipboard::new() else { return };
        // Finder puts a file's icon next to its path; the path wins.
        if let Ok(files) = board.get().file_list()
            && !files.is_empty()
        {
            return self.upload(key, files, false);
        }
        if board.get_image().is_ok() {
            return self.upload(key, vec![], true);
        }
        if let Ok(text) = board.get_text() {
            if let Some(view) = self.terminals.get_mut(&key) {
                view.paste_text(&text);
            }
            self.pasted(&key, &text);
        }
    }

    /// Files dropped on the terminal go up to its host, and their paths there are pasted.
    pub(super) fn drop_files(&mut self, files: &[PathBuf]) {
        if let Some(key) = self.front() {
            self.upload(key, files.to_vec(), false);
        }
    }

    /// Copies `files`, and the clipboard's image with `image`, to `key`'s host, then pastes
    /// their paths there.
    fn upload(&mut self, key: Key, mut files: Vec<PathBuf>, image: bool) {
        let Some(view) = self.terminals.get_mut(&key) else { return };
        let host = view.host.clone();
        let count = files.len() + usize::from(image);
        view.status = Some((format!("uploading {count} file(s)…"), Kind::Progress));
        self.in_background(
            move || {
                // An image goes up as a file of its own, removed once it's up.
                let temporary = image
                    .then(|| {
                        let path = std::env::temp_dir().join(format!("lemur-paste-{}.png", std::process::id()));
                        crate::clip::clipboard_png(&path).map(|()| path)
                    })
                    .transpose()?;
                files.extend(temporary.clone());
                let uploaded = actions::upload(&host, &files);
                if let Some(t) = temporary {
                    let _ = std::fs::remove_file(t);
                }
                uploaded
            },
            move |s, result| {
                let Some(view) = s.terminals.get_mut(&key) else { return };
                match result {
                    Ok(paths) => {
                        view.status = None;
                        view.paste_text(&actions::paste_line(&paths));
                    }
                    Err(e) => view.status = Some((format!("upload failed: {e:#}"), Kind::Error)),
                }
            },
        );
    }

    /// Copies a file the session printed the path of from the host, then shows it with
    /// Quick Look.
    fn preview(&mut self, key: Key, path: String) {
        let Some(view) = self.terminals.get_mut(&key) else { return };
        let (host, session) = (view.host.clone(), view.session.clone());
        view.status = Some((format!("downloading {path}…"), Kind::Progress));
        self.in_background(
            move || actions::quick_look(&actions::download(&host, &session, &path)?),
            move |s, result| {
                if let Some(view) = s.terminals.get_mut(&key) {
                    view.status = result.err().map(|e| (format!("download failed: {e:#}"), Kind::Error));
                }
            },
        );
    }

    /// Opens a file the session printed the path of in Zed, at the line it gave.
    fn open_in_zed(&mut self, key: Key, path: String, place: String) {
        let Some(view) = self.terminals.get_mut(&key) else { return };
        let (host, session) = (view.host.clone(), view.session.clone());
        view.status = Some((format!("opening {path} in Zed…"), Kind::Progress));
        self.in_background(
            move || actions::open_in_zed(&host, &session, &path, &place),
            move |s, result| {
                if let Some(view) = s.terminals.get_mut(&key) {
                    view.status = result.err().map(|e| (format!("couldn't open it in Zed: {e:#}"), Kind::Error));
                }
            },
        );
    }

    /// Copies a file the session printed the path of from the host into Downloads, and
    /// says where it went.
    fn download(&mut self, key: Key, path: String, tx: &mut Tx) {
        let options = FileDialogOptions {
            title: "Download".into(),
            directory: actions::downloads_dir().ok(),
            file_name: Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()),
            ..Default::default()
        };
        tx.save_file(options, move |s: &mut Lemur, answer, _| match answer {
            Ok(Some(to)) => s.fetch(key, path, Some(to)),
            Ok(None) => {}
            // No Save dialog to ask with: into Downloads, as before.
            Err(_) => s.fetch(key, path, None),
        });
    }

    /// Copies a file the session printed the path of from the host to `to`, or with none
    /// into Downloads under its own name, numbered if that's taken; says where it went.
    fn fetch(&mut self, key: Key, path: String, to: Option<PathBuf>) {
        let Some(view) = self.terminals.get_mut(&key) else { return };
        let (host, session) = (view.host.clone(), view.session.clone());
        view.status = Some((format!("downloading {path}…"), Kind::Progress));
        self.in_background(
            move || {
                let file = actions::download(&host, &session, &path)?;
                match to {
                    Some(to) => actions::keep_download_at(&file, &to).map(|()| to),
                    None => actions::keep_download(&file),
                }
            },
            move |s, result| {
                if let Some(view) = s.terminals.get_mut(&key) {
                    view.status = None;
                }
                match result {
                    Ok(kept) => {
                        let name = |p: Option<&Path>| p.and_then(Path::file_name).unwrap_or_default().to_string_lossy().into_owned();
                        s.note(format!("downloaded {} to {}", name(Some(&kept)), name(kept.parent())));
                    }
                    Err(e) => s.fail(e.context("download failed")),
                }
            },
        );
    }
}

/// What an input method composes for the terminal (dead keys, Japanese, the emoji picker,
/// dictation): shown at the cursor while composing, typed once committed.
pub fn on_ime(s: &mut Lemur, ime: &Ime, _: &mut Tx) -> bool {
    if !s.keyboard_on_terminal() {
        return false;
    }
    let Some(view) = s.front().and_then(|k| s.terminals.get_mut(&k)) else { return false };
    match ime {
        Ime::Preedit(text, _) => view.marked = (!text.is_empty()).then(|| text.clone()),
        Ime::Commit(text) => {
            view.marked = None;
            view.send(text.as_bytes().to_vec());
        }
        Ime::Enabled | Ime::Disabled => return false,
    }
    true
}
