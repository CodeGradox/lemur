//! The notes panel: one worktree's notes, always editable, saved as you
//! type. Markdown is highlighted, ⌘-click opens a URL in them, and a
//! task list's boxes tick.
//!
//! The editor is Rune's text field over the Markdown as typed. A field doesn't say what a
//! click landed on, so while ⌘ is down the notes' text lies over it again, laid out as the
//! field lays it out and drawn clear, its URLs and boxes links: a ⌘-click on one is the
//! link's, and anywhere else goes through to the field.

mod edits;
mod markdown;
#[cfg(test)]
mod tests;

use super::{Key, Lemur, Right, icons};
use crate::notes;
use edits::Edit;
use rune::{
    Chord, Color, El, Family, Key as K, KeyEvent, Link, LinkPaint, Mods, ScrollSpec, Span, TextEdit, TextStyle, Tx, col, memo, text,
    text_input,
};
use rune_kit::Kit;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// The links over a URL and a box, by where each starts.
const URL: &str = "notes-url-";
const BOX: &str = "notes-box-";

pub struct Notes {
    /// Where notes are kept (`~/.lemur/notes`); tests have none.
    dir: Option<PathBuf>,
    shown: Option<Shown>,
    edit: TextEdit,
    /// Why the notes couldn't be read or saved.
    error: Option<String>,
    /// A thread shown before a snapshot listed it: its notes show once one does.
    wait: Option<Key>,
    /// ⌘ is down: URLs and boxes are links.
    cmd: bool,
}

struct Shown {
    host: String,
    worktree: String,
    label: String,
    /// None when the notes couldn't be read: saving would overwrite them.
    file: Option<PathBuf>,
}

impl Notes {
    /// What's written in the notes on show.
    #[cfg(test)]
    pub(super) fn text(&self) -> &str {
        &self.edit.text
    }

    pub fn new(data_dir: Option<&Path>) -> Notes {
        Notes {
            dir: data_dir.map(|d| d.join("notes")),
            shown: None,
            edit: TextEdit::new(""),
            error: None,
            wait: None,
            cmd: false,
        }
    }

    /// Shows a worktree's notes; `label` names it in the header.
    fn show(&mut self, host: &str, worktree: &str, label: &str) {
        if let Some(shown) = &mut self.shown
            && shown.host == host
            && shown.worktree == worktree
        {
            shown.label = label.to_string();
            return;
        }
        let file = self.dir.as_ref().map(|d| notes::file(d, host, worktree));
        let (text, file) = match file.as_deref().map(notes::load).transpose() {
            Ok(text) => {
                self.error = None;
                (text.unwrap_or_default(), file)
            }
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                (String::new(), None)
            }
        };
        // Another worktree's notes start at their top, with nothing to undo.
        self.edit.replace_text(text);
        self.edit.set_caret(0);
        self.shown = Some(Shown { host: host.to_string(), worktree: worktree.to_string(), label: label.to_string(), file });
    }

    fn save(&mut self) {
        let Some(file) = self.shown.as_ref().and_then(|s| s.file.as_deref()) else { return };
        self.error = notes::save(file, &self.edit.text).err().map(|e| format!("{e:#}"));
    }

    /// Makes the edit `edit` gives for the notes' text and selection, if any,
    /// as one undo step, and saves. False when there was none.
    fn apply(&mut self, edit: impl FnOnce(&str, Range<usize>) -> Option<Edit>) -> bool {
        let (a, b) = self.edit.range();
        let Some((range, with, selected)) = edit(&self.edit.text, a..b) else { return false };
        self.edit.undo_group(|e| {
            if e.text[range.clone()] != with {
                e.set_selection(range.start, range.end);
                e.insert(&with);
            }
            e.set_selection(selected.start, selected.end);
        });
        self.save();
        true
    }

    /// Ticks the box on the line at byte `at`, or unticks it; false when the
    /// line has none. The selection stays as it was.
    fn tick(&mut self, at: usize) -> bool {
        self.apply(|text, selected| edits::tick(text, at).map(|(mark, tick)| (mark, tick.to_string(), selected)))
    }

    /// The cursor's byte, for the keys that act on its line.
    fn cursor(&self) -> usize {
        self.edit.text.floor_char_boundary(self.edit.focus)
    }

    #[cfg(test)]
    fn worktree(&self) -> Option<&str> {
        self.shown.as_ref().map(|s| s.worktree.as_str())
    }
}

impl Lemur {
    /// Shows a worktree's notes in the notes panel.
    pub(super) fn show_notes(&mut self, host: &str, worktree: &str, label: &str) {
        self.panels.notes.wait = None;
        self.panels.notes.show(host, worktree, label);
    }

    /// A thread came on show: the notes go to its worktree, now or once a
    /// snapshot lists it.
    pub(super) fn notes_follow_thread(&mut self) {
        self.panels.notes.wait = self.active.clone();
        self.notes_follow_shown();
    }

    /// The notes follow the thread on show, once a snapshot lists it.
    pub(super) fn notes_follow_shown(&mut self) {
        let Some((host, session)) = self.panels.notes.wait.clone().filter(|k| self.active.as_ref() == Some(k)) else { return };
        if let Some((worktree, label)) = self.worktree_of(&host, &session) {
            self.show_notes(&host, &worktree, &label);
        }
    }

    /// Another panel took the notes' place on the right: the keyboard leaves
    /// them.
    pub(super) fn notes_hidden(&mut self) {
        self.panels.notes.edit.focused = false;
    }

    /// The editor has the keyboard.
    pub(super) fn notes_editing(&self) -> bool {
        self.right() == Some(Right::Notes) && self.panels.notes.edit.focused
    }

    /// The notes' keys besides the editor's: Enter goes on with a list, Tab
    /// indents, Apple Notes' for a checklist, its boxes and moving items, and
    /// ⌘K for a link.
    fn notes_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let chord = |c: &str| Chord::parse(c).is_ok_and(|c| c.matches_tx(ev, tx));
        let m = ev.mods;
        let n = &mut self.panels.notes;
        match ev.key {
            // ⇧-Enter is a plain newline, the editor's own.
            K::Enter if m.shift || m.alt => false,
            K::Enter if m.cmd => {
                n.tick(n.cursor());
                true
            }
            // Anywhere else Enter is a newline: see `notes_newline`.
            K::Enter => n.apply(edits::next_item),
            K::Tab if !m.cmd && !m.ctrl && !m.alt => {
                n.apply(|text, selected| edits::indented(text, selected, m.shift));
                true
            }
            _ if chord("mod+k") => {
                tx.read_clipboard(|s: &mut Lemur, content, _| {
                    let url = edits::clipboard_url(content.text());
                    s.panels.notes.apply(|text, selected| edits::link(text, selected, url.as_deref()));
                });
                true
            }
            _ if chord("mod+shift+l") => {
                n.apply(|text, selected| Some(edits::bulleted(text, selected, true)));
                true
            }
            _ if chord("mod+shift+u") => {
                n.tick(n.cursor());
                true
            }
            _ if chord("ctrl+mod+up") || chord("ctrl+mod+down") => {
                let up = ev.key == K::Up;
                n.apply(|text, selected| edits::moved(text, selected, up));
                true
            }
            _ => false,
        }
    }

    /// Enter that doesn't go on with a list: a newline, which the field
    /// leaves to whoever has it submit.
    fn notes_newline(&mut self) {
        let n = &mut self.panels.notes;
        n.edit.insert("\n");
        n.save();
    }

    /// Pasting a URL over text on one line makes the text a link to it;
    /// any other paste is the editor's own.
    fn notes_paste(&mut self, pasted: &str) -> bool {
        let Some(url) = edits::clipboard_url(pasted) else { return false };
        self.panels.notes.apply(|text, selected| edits::pasted_link(text, selected, &url))
    }

    /// A list button: bullets the lines the selection touches, or gives them
    /// boxes with `boxed`, and gives the keyboard back to the notes.
    fn notes_bullet(&mut self, boxed: bool) {
        let n = &mut self.panels.notes;
        n.apply(|text, selected| Some(edits::bulleted(text, selected, boxed)));
        n.edit.focused = true;
    }

    /// A URL or a box ⌘-clicked: the URL opens, the box ticks.
    fn notes_link(&mut self, id: &str, tx: &mut Tx) {
        let n = &mut self.panels.notes;
        let at = |prefix: &str| id.strip_prefix(prefix).and_then(|at| at.parse::<usize>().ok());
        if let Some(at) = at(URL) {
            if let Some(url) = edits::urls(&n.edit.text).into_iter().find(|url| url.start == at) {
                let _ = tx.open_url(&n.edit.text[url]);
            }
        } else if let Some(at) = at(BOX).filter(|at| edits::boxes(&n.edit.text).contains(at)) {
            n.tick(at);
        }
    }
}

/// The modifier keys changed (`App::on_modifiers`): while ⌘ is down, the
/// notes' URLs and boxes are links.
pub(super) fn modifiers(s: &mut Lemur, mods: Mods, tx: &mut Tx) {
    if s.panels.notes.cmd == mods.cmd {
        tx.live_only();
    }
    s.panels.notes.cmd = mods.cmd;
}

/// The notes' type: the system's monospaced font, as a code editor's, so
/// lists and indents line up.
fn style(k: &Kit) -> TextStyle {
    TextStyle { family: Family::SystemMono, ..k.ty.mono }
}

pub fn view(s: &Lemur, k: &Kit) -> El {
    let c = &k.c;
    let n = &s.panels.notes;
    // Folded away, the notes leave the keyboard to the terminal.
    let on_show = s.right() == Some(Right::Notes);
    let label = n.shown.as_ref().map(|shown| shown.label.as_str());
    let mut header = super::panels::header("notes-header", Some(&icons::FILE_TEXT), "Notes", label, 0, k);
    if n.shown.is_some() {
        header = header
            .child(list_button("notes-bullets", &icons::LIST, "Bullet list", false, k))
            .child(list_button("notes-boxes", &icons::LIST_TODO, "Checklist (⇧⌘L)", true, k));
    }
    let body = match &n.shown {
        // Built again only when the notes change: the window draws often, as terminals do.
        Some(_) => memo("notes-scroll", (&n.edit, n.cmd, on_show, k.dark), || editor(n, on_show, k)),
        None => col().key("notes-none").w_full().px(12.0).child(
            text("Open a thread, or Notes in a worktree's + menu, to see its notes.").font(k.ty.ui).color(c.text_muted).wrap(),
        ),
    };
    let mut panel = col().key("notes-panel").w_full().h_full().child(header).child(body);
    if let Some(error) = &n.error {
        panel = panel
            .child(col().key("notes-error").w_full().px(12.0).py(8.0).child(text(error.clone()).font(k.ty.small).color(c.danger.fg).wrap()));
    }
    panel
}

fn list_button(key: &str, glyph: &'static rune::Icon, tip: &str, boxed: bool, k: &Kit) -> El {
    icons::button(glyph, tip)
        .on_press(move |s: &mut Lemur| s.notes_bullet(boxed))
        .build(k)
        .key(key)
}

/// The notes as Markdown in a field filling the panel, scrolling; `live`
/// while the panel shows.
fn editor(n: &Notes, live: bool, k: &Kit) -> El {
    let c = &k.c;
    let look = markdown::look(&n.edit.text, k);
    let field = text_input(&n.edit, style(k))
        .key("notes-editor")
        .w_full()
        .color(c.text_base)
        .placeholder("Write here. Markdown works, and ⌘-click opens a link.", c.text_faint)
        .caret(c.text_base, k.selection())
        .spans(look.spans.clone())
        // After the spans, which set the field's own strikes.
        .strikes(look.strikes.iter().map(|r| (r.start, r.end, None)))
        .on_edit(|s: &mut Lemur| &mut s.panels.notes.edit)
        .on_input_key(|s: &mut Lemur, ev, tx| s.notes_key(ev, tx))
        .on_input_change(|s: &mut Lemur, _| s.panels.notes.save())
        .on_input_paste(|s: &mut Lemur, content, _| s.notes_paste(content.text()))
        .on_submit(|s: &mut Lemur, _| s.notes_newline());
    let page = col()
        .key("notes-page")
        .w_full()
        .grow(1.0)
        .px(12.0)
        .pb(16.0)
        // A click under the text puts the cursor in it, at its end.
        .role("textbox")
        .child(col().key("notes-text").w_full().child(field).child(links(n, &look.spans, live, k)));
    let scroll = col().key("notes-scroll").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(page);
    if live { scroll } else { scroll.inert() }
}

/// While ⌘ is down, the notes' text again over the field, laid out as the
/// field lays it out and drawn clear: its URLs and boxes are links, which
/// underline under the pointer. A press anywhere else goes through to the
/// field. With ⌘ up it takes nothing.
fn links(n: &Notes, spans: &[Span], live: bool, k: &Kit) -> El {
    let notes = &n.edit.text;
    // What shapes the text, as in the field; not its colours.
    let shape: Vec<Span> = spans
        .iter()
        .filter(|s| s.weight.is_some() || s.italic)
        .map(|s| Span { color: None, bg: None, underline: None, ..s.clone() })
        .collect();
    let layer = text(notes.clone())
        .key("notes-links")
        .font(style(k))
        .wrap()
        .w_full()
        .absolute(0.0, 0.0)
        .color(Color::TRANSPARENT)
        .spans(shape)
        .a11y_hidden();
    if !(n.cmd && live) {
        return layer.inert();
    }
    let hover = LinkPaint::new().underline(k.c.text_accent);
    let urls = edits::urls(notes).into_iter().map(|url| Link::new(url.clone(), format!("{URL}{}", url.start)).hover(hover));
    let boxes = edits::boxes(notes).into_iter().map(|at| Link::new(at..at + 3, format!("{BOX}{at}")).role("checkbox").hover(hover));
    urls.chain(boxes).fold(layer, El::link).on_link(|s: &mut Lemur, id, tx| s.notes_link(id, tx))
}
