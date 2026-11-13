//! A thread's terminal: attached through herdr-core, its bytes parsed by Ghostty's core
//! (`grid`), drawn by Rune run by run at their columns. Every opened thread keeps its own,
//! so switching is instant and scrollback survives.

use super::grid::{Grid, Line, Row, Shape, Size};
use super::palette::{Palette, hex};
use super::snackbar::Kind;
use super::{Key, Lemur};
use crate::config::Host;
use crate::hc::{self, Core, Stream};
use futures::StreamExt;
use rune::tok::Family;
use rune::{Color, El, TextEdit, TextStyle, col, el, memo, text};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The least time between attaching again on its own after a drop.
const RETRY: Duration = Duration::from_secs(2);
/// Output is drawn at most this often, about 30 times a second, as Herdr does; right
/// after typing (`TYPING`), at once.
pub const OUTPUT_EVERY: Duration = Duration::from_millis(33);
const TYPING: Duration = Duration::from_millis(200);
/// How long a terminal's stream stays quiet before its first paint counts as over: at
/// least this, however quick the line...
const PAINT_QUIET: Duration = Duration::from_millis(100);
/// ...and at most this, however slow.
const PAINT_QUIET_MOST: Duration = Duration::from_secs(1);
/// How long input waits for it at most.
const PAINT_LONGEST: Duration = Duration::from_secs(5);
/// How often a waiting terminal looks.
pub const PAINT_LOOK: Duration = Duration::from_millis(25);
/// Points between the grid and the frame's edge.
pub const PAD: f32 = 8.0;

/// Where a view is with its terminal's byte stream.
enum Attachment {
    /// Input meanwhile waits in `held`.
    Attaching,
    /// The core, and the channel the terminal's bytes come on.
    Attached(Arc<Core>, u32),
    /// The stream ended: the program exited, or the line dropped.
    Ended,
}

/// Input waiting for the terminal to attach and paint.
enum Held {
    Bytes(Vec<u8>),
    Paste(String),
}

/// The terminal font, and a cell's size in it, in points.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Metrics {
    pub family: &'static str,
    pub size: f32,
    pub cell_w: f32,
    pub cell_h: f32,
}

impl Metrics {
    /// The font at `size`, with Menlo's advance (1233 of 2048 units an em) until a probe of
    /// the real font says otherwise.
    pub fn new(family: &str, size: f64) -> Metrics {
        let size = size as f32;
        // What the font lacks comes from Menlo, as Core Text's fallback (Ghostty's,
        // Terminal's) has it, before the fonts Rune would find: theirs can be wider than the
        // cell, a ◯ two and a half cells wide over its neighbours.
        let family = if family.contains(',') { format!("{family}, Menlo") } else { format!("\"{family}\", Menlo") };
        Metrics {
            family: Box::leak(family.into_boxed_str()),
            size,
            cell_w: size * 1233.0 / 2048.0,
            cell_h: (size * 1.3).round(),
        }
    }

    pub fn style(self, bold: bool) -> TextStyle {
        TextStyle::new(self.size, if bold { 700.0 } else { 400.0 }).family(Family::Named(self.family)).lh(self.cell_h)
    }
}

pub struct TermView {
    pub host: Host,
    pub session: String,
    pub grid: Option<Grid>,
    /// The screen an attach paints, once it's attached: until then the last one shows,
    /// and stays if the attach fails.
    fresh: Option<Grid>,
    attachment: Attachment,
    /// Which attach this view is on: a reader of an earlier one is ignored.
    attach: u64,
    /// The stream dropped while its program may still run.
    lost: bool,
    tried: Option<Instant>,
    held: Vec<Held>,
    /// Attaching again, its program took bracketed pastes: a paste held meanwhile goes as one
    /// even when the new paint hasn't said so yet, as it says it after the scrollback.
    bracketed_before: bool,
    /// The core's first paint is over, which it doesn't say: its stream has gone quiet a
    /// while (see `PAINT_QUIET`).
    pub painted: bool,
    last_output: Option<Instant>,
    attached_at: Option<Instant>,
    quiet: Duration,
    pub typed_at: Option<Instant>,
    pub drawn_at: Option<Instant>,
    /// Wheel travel short of a whole line, kept for the next.
    pub scroll_rest: f32,
    /// The cell a press the program was told about was last at.
    pub pressed: Option<(u16, u16)>,
    /// A selection is being dragged out.
    pub selecting: bool,
    /// The link under the pointer while ⌘ is held: underlined, and a click opens it.
    pub hover: Option<super::term_links::Link>,
    /// What an input method is still composing, shown at the cursor until it's typed.
    pub marked: Option<String>,
    /// Shown in the window's snackbars while this terminal is.
    pub status: Option<(String, Kind)>,
    /// The pointer is over its scroll bar.
    pub bar_hover: bool,
    /// Its scroll bar's thumb is dragged, from the row it showed first at the press.
    pub bar_drag: Option<usize>,
    /// What its find bar (⌘F) looks for, kept while it's closed, and whether it's open.
    pub find: TextEdit,
    pub finding: bool,
    /// Typed in the find bar: the newest match is picked once there's one.
    pub find_pick: bool,
    /// The cells it's shown at, and what the core was last told: the colours and cell size
    /// it's shown in, and whether it has the keyboard.
    pub size: Size,
    told_display: Option<serde_json::Value>,
    told_focus: Option<bool>,
}

impl Drop for TermView {
    fn drop(&mut self) {
        self.detach();
    }
}

impl TermView {
    fn new(host: Host, session: String, size: Size) -> TermView {
        TermView {
            host,
            session,
            grid: None,
            fresh: None,
            attachment: Attachment::Ended,
            attach: 0,
            lost: false,
            tried: None,
            held: Vec::new(),
            bracketed_before: false,
            painted: false,
            last_output: None,
            attached_at: None,
            quiet: PAINT_QUIET,
            typed_at: None,
            drawn_at: None,
            scroll_rest: 0.0,
            pressed: None,
            selecting: false,
            hover: None,
            marked: None,
            status: None,
            bar_hover: false,
            bar_drag: None,
            find: TextEdit::default(),
            finding: false,
            find_pick: false,
            size,
            told_display: None,
            told_focus: None,
        }
    }

    /// Its stream ended, so it shows what it last had.
    pub fn exited(&self) -> bool {
        matches!(self.attachment, Attachment::Ended)
    }

    pub fn is_attached(&self) -> bool {
        matches!(self.attachment, Attachment::Attached(..))
    }

    /// Whether its stream dropped while its program may still run.
    pub fn disconnected(&self) -> bool {
        self.lost && self.exited()
    }

    fn detach(&mut self) {
        if let Attachment::Attached(core, channel) = &self.attachment {
            core.detach(*channel);
        }
        self.attachment = Attachment::Ended;
    }

    /// No longer shown: its program is told it lost focus (vim's FocusLost, Claude's paused
    /// spinner), as it would be shown again.
    pub fn hidden(&mut self) {
        self.tell_focus(false);
    }

    pub fn tell_focus(&mut self, focused: bool) {
        if let Attachment::Attached(core, channel) = &self.attachment
            && self.told_focus != Some(focused)
        {
            core.focus(*channel, focused);
            self.told_focus = Some(focused);
        }
    }

    /// Tells the core the colours and cell size it's shown in, which it answers programs
    /// that ask with, when they change.
    pub fn tell_display(&mut self, display: serde_json::Value) {
        if let Attachment::Attached(core, channel) = &self.attachment
            && self.told_display.as_ref() != Some(&display)
        {
            core.facts(*channel, &display);
            self.told_display = Some(display);
        }
    }

    /// Sends what you typed, or holds it while the terminal attaches and paints.
    pub fn write(&mut self, bytes: Vec<u8>) {
        self.typed_at = Some(Instant::now());
        match &self.attachment {
            Attachment::Attached(core, channel) if self.painted => core.input(*channel, &bytes),
            Attachment::Attaching | Attachment::Attached(..) => self.held.push(Held::Bytes(bytes)),
            Attachment::Ended => {}
        }
    }

    /// Pastes `text` as if typed, bracketed when the program asked for it.
    pub fn paste_text(&mut self, text: &str) {
        let waiting = match &self.attachment {
            Attachment::Attaching => true,
            Attachment::Attached(..) => !self.painted,
            Attachment::Ended => false,
        };
        if waiting {
            self.held.push(Held::Paste(text.to_string()));
            return;
        }
        let bracketed = self.bracketed_before || self.grid.as_ref().is_some_and(Grid::bracketed_paste);
        self.write(super::keys::paste(text, bracketed));
    }

    /// Typed: the view goes back to the bottom, where output shows.
    pub fn send(&mut self, bytes: Vec<u8>) {
        if let Some(g) = &mut self.grid {
            g.scroll_to_bottom();
            g.clear_selection();
        }
        self.write(bytes);
    }

    fn send_held(&mut self) {
        if !self.is_attached() {
            return;
        }
        for input in std::mem::take(&mut self.held) {
            match input {
                Held::Bytes(bytes) => self.write(bytes),
                Held::Paste(text) => self.paste_text(&text),
            }
        }
        self.bracketed_before = false;
    }

    /// Feeds relayed bytes in; false once the stream is over.
    fn ingest(&mut self, batch: Vec<Stream>) -> bool {
        // Attaching again, the stream can start before the attach says it's done: its bytes
        // are the new screen's, not the one still on show.
        let Some(grid) = self.fresh.as_mut().or(self.grid.as_mut()) else { return false };
        let mut open = true;
        // Its program ended: the thread goes from the sidebar, and this with it.
        let mut over = false;
        let mut read = 0;
        for item in batch {
            match item {
                Stream::Data(bytes) => {
                    grid.write(&bytes);
                    read += bytes.len();
                }
                Stream::Closed(why) => {
                    open = false;
                    over = matches!(why["reason"].as_str(), Some("exited" | "killed"));
                }
                Stream::Lost => open = false,
            }
        }
        // Once for the batch: streaming fast, a batch holds hundreds of chunks.
        if read > 0 {
            if let Attachment::Attached(core, channel) = &self.attachment {
                core.credit(*channel, read);
            }
            self.last_output = Some(Instant::now());
        }
        let replies = grid.take_replies();
        if !replies.is_empty() {
            self.write(replies);
        }
        if !open {
            self.attachment = Attachment::Ended;
            self.lost = !over;
            self.status = (!over).then(|| ("disconnected · reconnecting…".into(), Kind::Warning));
        }
        open
    }

    /// Whether the first paint is over, as `PAINT_QUIET` says; it then sends what waited.
    /// True while it's still waiting.
    fn check_paint(&mut self) -> bool {
        if self.painted || !self.is_attached() {
            return false;
        }
        let over = self.last_output.is_some_and(|t| t.elapsed() >= self.quiet)
            || self.attached_at.is_some_and(|t| t.elapsed() >= PAINT_LONGEST);
        if over {
            self.painted = true;
            self.send_held();
        }
        !over
    }

    /// Brings the rows up to the screen, at most every `OUTPUT_EVERY` unless you just
    /// typed. Some(true) when they changed, Some(false) when nothing waits, None when it's
    /// to look again in a moment.
    pub fn draw(&mut self) -> Option<bool> {
        let grid = self.grid.as_mut()?;
        if !grid.stale {
            return Some(false);
        }
        let typing = self.typed_at.is_some_and(|t| t.elapsed() < TYPING);
        let due = self.drawn_at.is_none_or(|t| t.elapsed() >= OUTPUT_EVERY);
        if !typing && !due {
            return None;
        }
        self.drawn_at = Some(Instant::now());
        Some(grid.refresh().unwrap_or(false))
    }
}

impl Lemur {
    /// The size terminals are shown at now, in cells.
    pub(super) fn cell_size(&self) -> Size {
        let m = self.metrics;
        let (w, h) = self.term_area;
        let px = |p: f32| (p * 2.0).round() as u16;
        Size {
            cols: (((w - 2.0 * PAD) / m.cell_w).floor() as u16).max(2),
            lines: (((h - 2.0 * PAD) / m.cell_h).floor() as u16).max(1),
            cell_width: px(m.cell_w),
            cell_height: px(m.cell_h),
        }
    }

    pub(super) fn palette(&self) -> Palette {
        Palette::themed(self.dark, &self.prefs.terminal)
    }

    /// What the core tells programs that ask about colours and cell size.
    fn display(&self) -> serde_json::Value {
        let p = self.palette();
        let size = self.cell_size();
        serde_json::json!({
            "fg": hex(p.foreground),
            "bg": hex(p.background),
            "cursor": hex(p.cursor),
            "cell": [size.cell_width, size.cell_height],
        })
    }

    /// The terminal of thread `key` on host `h`, made and attached the first time, and
    /// attached again once its stream ended.
    pub(super) fn terminal_for(&mut self, key: &Key, h: Host) {
        if !self.terminals.contains_key(key) {
            let view = TermView::new(h, key.1.clone(), self.cell_size());
            self.terminals.insert(key.clone(), view);
            self.connect(key);
        } else if self.terminals.get(key).is_some_and(TermView::exited) {
            self.connect(key);
        }
    }

    /// (Re)attaches `key`'s terminal, e.g. after the line to its host dropped. The core
    /// paints its screen and scrollback first.
    pub(super) fn connect(&mut self, key: &Key) {
        let size = self.cell_size();
        let scrollback = self.prefs.terminal.scrollback;
        let palette = self.palette();
        let display = self.display();
        let Some(view) = self.terminals.get_mut(key) else { return };
        view.detach();
        let Ok(fresh) = Grid::new(size, scrollback, &palette) else { return };
        match view.grid {
            Some(_) => view.fresh = Some(fresh),
            None => view.grid = Some(fresh),
        }
        view.size = size;
        view.attachment = Attachment::Attaching;
        view.attach += 1;
        view.painted = false;
        view.last_output = None;
        view.held.clear();
        view.bracketed_before = view.grid.as_ref().is_some_and(Grid::bracketed_paste);
        view.tried = Some(Instant::now());
        view.told_display = Some(display.clone());
        view.told_focus = None;
        // Reconnecting, it still says so until it's back.
        if !view.lost {
            view.status = None;
        }
        let (host, session, attach) = (view.host.clone(), view.session.clone(), view.attach);
        let back = self.bg.back();
        let key = key.clone();
        let reader_key = key.clone();
        self.bg.run(
            move || {
                // A terminal on a host still being reached can take seconds.
                let (core, _) = hc::for_host(&host)?;
                let attached = core.attach(&session, size.cols, size.lines, scrollback, display)?;
                let channel = attached.channel;
                // Its bytes, a batch at a time, until the stream ends.
                let mut stream = attached.stream;
                std::thread::spawn(move || {
                    while let Some(first) = futures::executor::block_on(stream.next()) {
                        let mut batch = vec![first];
                        while let Ok(more) = stream.try_recv() {
                            batch.push(more);
                        }
                        let key = reader_key.clone();
                        if !back.send(move |s| s.ingest(&key, attach, batch)) {
                            return;
                        }
                    }
                });
                anyhow::Ok((core, channel))
            },
            move |s, attached| s.attached(&key, attach, attached),
        );
    }

    fn attached(&mut self, key: &Key, attach: u64, result: anyhow::Result<(Arc<Core>, u32)>) {
        let size = self.cell_size();
        let active = self.active.as_ref() == Some(key);
        let focused = self.keyboard_on_terminal();
        let Some(view) = self.terminals.get_mut(key).filter(|v| v.attach == attach) else {
            // The view went, or attached again, meanwhile: let this one go.
            if let Ok((core, channel)) = result {
                core.detach(channel);
            }
            return;
        };
        match result {
            Ok((core, channel)) => {
                // The terminal takes the size it's shown at: the attach asked for the size
                // known then, often before the first layout.
                core.resize(channel, size.cols, size.lines);
                view.attachment = Attachment::Attached(core, channel);
                let answered_in = view.tried.map_or(Duration::ZERO, |t| t.elapsed());
                view.quiet = (answered_in * 2).clamp(PAINT_QUIET, PAINT_QUIET_MOST);
                view.attached_at = Some(Instant::now());
                if let Some(mut fresh) = view.fresh.take() {
                    let _ = fresh.resize(size);
                    view.grid = Some(fresh);
                }
                if std::mem::take(&mut view.lost) {
                    view.status = None;
                }
                if active {
                    view.tell_focus(focused);
                }
            }
            Err(e) => {
                // Tried again once the terminal runs.
                view.attachment = Attachment::Ended;
                view.held.clear();
                view.fresh = None;
                view.lost = true;
                view.status = Some((format!("{e:#} · trying again"), Kind::Error));
            }
        }
    }

    fn ingest(&mut self, key: &Key, attach: u64, batch: Vec<Stream>) {
        let Some(view) = self.terminals.get_mut(key).filter(|v| v.attach == attach) else { return };
        view.ingest(batch);
    }

    /// Each poll: terminals waiting for their first paint send what waited once it's over,
    /// and the one on show draws new output. Some(delay) to look again then.
    pub(super) fn tick_terminals(&mut self) -> (bool, Option<Duration>) {
        let mut again = None;
        let shown: Vec<Key> = self.active.iter().chain(self.peek.iter()).cloned().collect();
        let mut changed = false;
        for (k, view) in &mut self.terminals {
            if view.check_paint() {
                again = Some(PAINT_LOOK);
            }
            if shown.contains(k) {
                changed |= view.draw().unwrap_or(false);
                // Output waiting for its turn, or for its program to let go of the screen.
                if view.grid.as_ref().is_some_and(|g| g.stale) {
                    again = Some(again.map_or(OUTPUT_EVERY, |a: Duration| a.min(OUTPUT_EVERY)));
                }
            }
        }
        (changed, again)
    }

    /// Attaches again each terminal whose stream dropped, once its program runs again, as
    /// Herdr does: after the line to its host or to herdr-core comes back, or a kept
    /// program's new shell starts.
    pub(super) fn reconnect_terminals(&mut self) {
        let running: std::collections::HashSet<Key> = super::sessions(&self.snap)
            .flat_map(|(h, s)| {
                let own = s.running.then(|| super::key(h, s));
                let helpers = s.helpers.iter().filter(|x| x.terminal);
                own.into_iter().chain(helpers.map(move |x| (h.to_string(), x.id.clone())))
            })
            .chain(self.setups.keys().cloned())
            .collect();
        let again: Vec<Key> = self
            .terminals
            .iter()
            .filter(|(k, v)| running.contains(*k) && v.disconnected() && v.tried.is_none_or(|t| t.elapsed() >= RETRY))
            .map(|(k, _)| k.clone())
            .collect();
        for k in again {
            self.connect(&k);
        }
    }

    /// The terminals take the size the pane gives them.
    pub(super) fn fit_terminals(&mut self) {
        let size = self.cell_size();
        for view in self.terminals.values_mut() {
            if view.size == size {
                continue;
            }
            view.size = size;
            if let Some(g) = &mut view.grid {
                let _ = g.resize(size);
            }
            if let Attachment::Attached(core, channel) = &view.attachment {
                core.resize(*channel, size.cols, size.lines);
            }
        }
    }

    /// Settings changed the colours, or the app went light or dark.
    pub(super) fn recolor_terminals(&mut self) {
        let palette = self.palette();
        let display = self.display();
        for view in self.terminals.values_mut() {
            for g in view.grid.iter_mut().chain(view.fresh.iter_mut()) {
                let _ = g.set_palette(&palette);
            }
            view.tell_display(display.clone());
        }
    }

    /// The keyboard is the terminal's: nothing else (a field, a spotlight) has it.
    pub(super) fn keyboard_on_terminal(&self) -> bool {
        self.prompt.is_none() && self.dialog.is_none() && !self.panels_overlaid() && !self.editing()
    }

    /// A text field of the window's has the keyboard. Rune keeps each field's `focused`, so
    /// a click elsewhere, which takes the keyboard from it, gives it back to the terminal.
    pub(super) fn editing(&self) -> bool {
        self.prompt.as_ref().is_some_and(|p| p.input.focused || p.branch.as_ref().is_some_and(|b| b.focused))
            || self.panels_editing()
            || self.find_editing()
    }

    /// The peek has the keyboard: nothing's over it.
    pub(super) fn keyboard_on_peek(&self) -> bool {
        self.peek.is_some() && self.keyboard_on_terminal()
    }
}

/// One row: backgrounds, then each run of text at its column.
fn row_el(id: &str, y: usize, row: &Row, m: Metrics) -> El {
    let mut e = el().key(format!("tr-{id}-{y}")).w_full().h(m.cell_h);
    for f in &row.fills {
        e = e.child(el().absolute(f32::from(f.col) * m.cell_w, 0.0).size(f32::from(f.cells) * m.cell_w, m.cell_h).fill(f.color));
    }
    for &(col, c, color) in &row.blocks {
        let Some((rects, opacity)) = super::grid::block(c) else { continue };
        let x = f32::from(col) * m.cell_w;
        for [l, t, r, b] in rects {
            let at = el().absolute(x + l * m.cell_w, t * m.cell_h).size((r - l) * m.cell_w, (b - t) * m.cell_h);
            e = e.child(at.fill(color.alpha(color.a * opacity)));
        }
    }
    for r in &row.runs {
        let mut style = m.style(r.bold);
        if r.italic {
            style = style.italic();
        }
        let x = f32::from(r.col) * m.cell_w;
        e = e.child(text(r.text.clone()).key(format!("tr-{id}-{y}-{}", r.col)).font(style).color(r.fg).absolute(x, 0.0));
        let w = f32::from(r.cells) * m.cell_w;
        let line = |y: f32| el().absolute(x, y).size(w, 1.0).fill(r.fg);
        match r.underline {
            Line::None => {}
            Line::Single => e = e.child(line(m.cell_h - 3.0)),
            Line::Double => e = e.child(line(m.cell_h - 4.0)).child(line(m.cell_h - 2.0)),
            // A line waving 2 points down and back every 6, as Chromium draws its spelling
            // marker: a step a point, the steps at one height drawn as one.
            Line::Curly => {
                const WAVE: [f32; 6] = [0.0, 1.0, 2.0, 2.0, 1.0, 0.0];
                let points = w.ceil() as usize;
                let mut from = 0;
                for i in 1..=points {
                    if i == points || WAVE[i % 6] != WAVE[from % 6] {
                        let y = m.cell_h - 4.0 + WAVE[from % 6];
                        e = e.child(el().absolute(x + from as f32, y).size((i - from) as f32, 1.0).fill(r.fg));
                        from = i;
                    }
                }
            }
        }
        if r.strike {
            e = e.child(line((m.cell_h / 2.0).round()));
        }
    }
    e
}

/// The text of `row` from column `start` up to `end`, run by run: where each part starts,
/// it, and its run. A run of wide characters goes whole or not at all.
fn span_runs(row: &Row, start: u16, end: u16) -> Vec<(u16, String, &super::grid::Run)> {
    row.runs
        .iter()
        .filter(|r| r.col < end && r.col + r.cells > start)
        .filter_map(|r| {
            if r.text.chars().count() == usize::from(r.cells) {
                let from = start.max(r.col);
                let words: String = r.text.chars().skip(usize::from(from - r.col)).take(usize::from(end.min(r.col + r.cells) - from)).collect();
                Some((from, words, r))
            } else {
                (r.col >= start).then(|| (r.col, r.text.clone(), r))
            }
        })
        .collect()
}

/// The character in cell (`x`, `y`), and the run it's in.
fn under_cursor(rows: &[Row], x: u16, y: u16) -> Option<(String, &super::grid::Run)> {
    let r = rows.get(usize::from(y))?.runs.iter().find(|r| (r.col..r.col + r.cells).contains(&x))?;
    // A run of narrow characters has one to a cell; a wide one is a run of its own.
    let c = if r.text.chars().count() == usize::from(r.cells) {
        r.text.chars().nth(usize::from(x - r.col))?.to_string()
    } else {
        (x == r.col).then(|| r.text.clone())?
    };
    (c != " ").then_some((c, r))
}

/// A terminal's screen: its rows and its cursor, a block while it has the keyboard and an
/// outline otherwise.
pub(super) fn screen(view: &TermView, m: Metrics, focused: bool, id: &str) -> El {
    let Some(g) = &view.grid else {
        return col().key(format!("screen-{id}")).w_full().h_full();
    };
    let grid = g.frame();
    let mut rows = col().key(format!("grid-{id}")).w(f32::from(g.size().cols) * m.cell_w);
    for (y, row) in grid.rows.iter().enumerate() {
        // A row Ghostty didn't rebuild keeps the elements built for it last time.
        rows = rows.child(memo(format!("tr-{id}-{y}"), (row.version, m.cell_w.to_bits(), m.cell_h.to_bits()), || row_el(id, y, row, m)));
    }
    // What ⌘F found, on its colour with its text in black over it.
    for (i, &(row, start, end, picked)) in g.found().spans.iter().enumerate() {
        let (x, y, w) = (f32::from(start) * m.cell_w, f32::from(row) * m.cell_h, f32::from(end - start) * m.cell_w);
        let mut found = el().key(format!("found-{id}-{i}")).absolute(x, y).size(w, m.cell_h).fill(super::term_find::match_fill(picked));
        for (col, words, r) in grid.rows.get(usize::from(row)).map(|r| span_runs(r, start, end)).unwrap_or_default() {
            let style = if r.italic { m.style(r.bold).italic() } else { m.style(r.bold) };
            found = found.child(text(words).font(style).color(Color::BLACK).absolute(f32::from(col - start) * m.cell_w, 0.0));
        }
        rows = rows.child(found);
    }
    for (i, &(row, start, end)) in view.hover.iter().flat_map(|link| &link.spans).enumerate() {
        let (x, w) = (f32::from(start) * m.cell_w, f32::from(end - start) * m.cell_w);
        rows = rows.child(el().key(format!("link-{id}-{i}")).absolute(x, f32::from(row + 1) * m.cell_h - 2.0).size(w, 1.0).fill(grid.fg));
    }
    // A file's path held with ⌘ drags to other apps, and a click on it previews it, as on the
    // terminal under it: the path, drawn again over it, is what the pointer lifts.
    for (i, &(row, start, end)) in view.hover.iter().filter(|link| link.file).flat_map(|link| &link.spans).enumerate() {
        let Some(link) = &view.hover else { break };
        let (x, y, w) = (f32::from(start) * m.cell_w, f32::from(row) * m.cell_h, f32::from(end - start) * m.cell_w);
        let mut lifted = el().key(format!("link-drag-{id}-{i}")).absolute(x, y).size(w, m.cell_h).fill(grid.bg).cursor(rune::Cursor::Pointer);
        for (col, words, r) in grid.rows.get(usize::from(row)).map(|r| span_runs(r, start, end)).unwrap_or_default() {
            let style = if r.italic { m.style(r.bold).italic() } else { m.style(r.bold) };
            lifted = lifted.child(text(words).font(style).color(r.fg).absolute(f32::from(col - start) * m.cell_w, 0.0));
        }
        let carried: super::carry::Carried = (view.host.name.clone(), Some(view.session.clone()), link.url.clone());
        let (key, url, backing) = ((view.host.name.clone(), view.session.clone()), link.url.clone(), grid.bg);
        rows = rows.child(
            lifted
                .child(el().absolute(0.0, m.cell_h - 2.0).size(w, 1.0).fill(grid.fg))
                .drag_out(move |s: &Lemur| s.drag_item(&carried, backing))
                .on_press(move |s: &mut Lemur| s.preview(key.clone(), url.clone())),
        );
    }
    if let Some((x, y)) = grid.caret {
        let (cx, cy) = (f32::from(x) * m.cell_w, f32::from(y) * m.cell_h);
        let at = |key: &str| el().key(format!("{key}-{id}")).absolute(cx, cy).size(m.cell_w, m.cell_h);
        // The shape its program asks for: a bar, a line under, or a block, which goes hollow
        // without the keyboard (as v2 drew it).
        let block = matches!(grid.shape, Shape::Block | Shape::Hollow);
        let shaped = |key: &str| match grid.shape {
            Shape::Bar => at(key).size(2.0, m.cell_h),
            Shape::Underline => el().key(format!("{key}-{id}")).absolute(cx, cy + m.cell_h - 2.0).size(m.cell_w, 2.0),
            Shape::Block | Shape::Hollow => at(key),
        };
        let solid = !block || focused && grid.shape == Shape::Block;
        // The input method's window opens at the cursor while the terminal has the keyboard,
        // and stays on while the program hides the cursor to draw its own.
        let drawn = match (grid.cursor.is_some(), solid) {
            (true, true) => Some(shaped("cursor").fill(grid.cursor_color.alpha(0.6))),
            (true, false) => Some(at("cursor").border(1.0, grid.cursor_color)),
            (false, _) => focused.then(|| at("caret")),
        };
        rows = rows.children(drawn.map(|e| if focused { e.ime_cursor() } else { e }));
        // Over a block, the character under it again, so it reads through.
        if grid.cursor.is_some() && solid && block {
            rows = rows.children(under_cursor(&grid.rows, x, y).map(|(c, r)| {
                let style = if r.italic { m.style(r.bold).italic() } else { m.style(r.bold) };
                text(c).key(format!("cursor-text-{id}")).font(style).color(r.fg).absolute(cx, cy)
            }));
        }
        if let Some(marked) = &view.marked {
            let w = marked.chars().count() as f32 * m.cell_w;
            rows = rows
                .child(el().absolute(cx, cy).size(w, m.cell_h).fill(grid.bg))
                .child(text(marked.clone()).key(format!("marked-{id}")).font(m.style(false)).color(grid.fg).absolute(cx, cy))
                .child(el().absolute(cx, cy + m.cell_h - 2.0).size(w, 1.0).fill(grid.fg));
        }
    }
    // The window's terminal has a scroll bar; a peek at one doesn't.
    let bar = (id == "main").then(|| super::term_bar::bar(view, grid.fg)).flatten();
    // A terminal's screen changes in place: no springs, fades or exits.
    col().key(format!("screen-{id}")).w_full().h_full().padding(PAD).fill(grid.bg).child(rows.instant_changes()).children(bar)
}

#[cfg(test)]
impl TermView {
    /// Takes `bytes` in as if the core relayed them, for tests.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.ingest(vec![Stream::Data(bytes.to_vec())]);
    }
}

/// The text of `key`'s screen, for tests.
#[cfg(test)]
pub(super) fn lines(s: &Lemur, key: &Key) -> Vec<String> {
    s.terminals.get(key).and_then(|v| v.grid.as_ref()).map(|g| g.lines()).unwrap_or_default()
}

