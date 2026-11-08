//! A terminal's screen: the bytes herdr-core relays, parsed by Ghostty's terminal core
//! (libghostty-vt), and kept as rows of styled runs for the view to place at their columns.
//! Knows nothing about herdr-core or drawing.

use super::palette::{Palette, Rgb};
use libghostty_vt::fmt::Format;
use libghostty_vt::key;
use libghostty_vt::mouse;
use libghostty_vt::render::{CellIteration, CellIterator, Colors, Dirty, RenderState, RowId, RowIterator};
use libghostty_vt::screen::{CellWide, Screen};
use libghostty_vt::selection::FormatOptions;
use libghostty_vt::selection::gesture::{DragEvent, Gesture, Geometry, PressEvent, ReleaseEvent};
use libghostty_vt::style::{self, RgbColor, Underline};
use libghostty_vt::terminal::{Mode, Point, PointCoordinate, ScrollViewport, SizeReportSize, Terminal};
use rune::{Color, Key, KeyEvent};
use std::cell::{Cell, Ref, RefCell};
use std::collections::HashMap;
use std::error::Error;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Pointer positions go to Ghostty in hundredths of a point, which it takes as pixels: whole
/// numbers, and a cell's width (7.83 points in Menlo at 13) stays exact enough.
const UNIT: f32 = 100.0;
/// The longest a program may hold the screen (synchronized output) before what it drew
/// meanwhile shows anyway: one that crashed or forgot to let go would freeze it.
const HOLD: Duration = Duration::from_secs(1);
/// Ghostty drops scrollback a page at a time, and at 80 columns a page holds about 500 rows:
/// asked to keep this many more, it keeps at least as many lines as set.
const PAGE_SLACK: u32 = 1000;

/// The line limit to give Ghostty for `lines` of scrollback.
fn scrollback_limit(lines: u32) -> Option<usize> {
    Some(if lines == 0 { 0 } else { (lines + PAGE_SLACK) as usize })
}

pub type Res<T> = Result<T, Box<dyn Error>>;

/// Text in one style, starting at a column. Plain ASCII runs span many cells; anything else
/// (box drawing, CJK, emoji) is a run of its own, so a glyph from a fallback font, whatever
/// its advance, can't push the cells after it off their columns.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub col: u16,
    pub cells: u16,
    pub text: String,
    pub fg: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

/// A background over `cells` cells from `col`.
#[derive(Clone, Debug, PartialEq)]
pub struct Fill {
    pub col: u16,
    pub cells: u16,
    pub color: Color,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    pub runs: Vec<Run>,
    pub fills: Vec<Fill>,
    /// Block elements, painted rather than typed (see [`block`]), by column.
    pub blocks: Vec<(u16, char, Color)>,
    /// Bumped each time the row is rebuilt, so the view can keep a row it built before.
    pub version: u64,
}

impl Row {
    /// The row's text with its runs at their columns, without trailing blanks.
    #[cfg(test)]
    pub fn text(&self) -> String {
        let mut s = String::new();
        let mut at = 0u16;
        for r in &self.runs {
            while at < r.col {
                s.push(' ');
                at += 1;
            }
            s.push_str(&r.text);
            at = r.col + r.cells;
        }
        s
    }
}

/// The size a program asks for (`CSI 18 t`): cells, and a cell's pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub cols: u16,
    pub lines: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

/// What the view shows of the terminal: its rows, where the cursor is, and its colours.
pub struct Frame {
    pub rows: Vec<Row>,
    pub cursor: Option<(u16, u16)>,
    pub fg: Color,
    pub bg: Color,
    pub cursor_color: Color,
}

/// Builds the [`Frame`] from Ghostty's render state. Shared with the terminal's hold
/// callback, which paints the frame a program wants left on screen as its hold begins.
struct Painter {
    render: RenderState<'static>,
    row_it: RowIterator<'static>,
    cell_it: CellIterator<'static>,
    scratch: String,
    ids: Vec<RowId>,
    version: u64,
    frame: Frame,
    /// What's selected shows on this, in the palette's colours.
    selection: RgbColor,
    /// The frame changed since the view last looked.
    changed: bool,
}

pub struct Grid {
    terminal: Terminal<'static, 'static>,
    painter: Rc<RefCell<Painter>>,
    /// When the program's hold on the screen began, while it holds it.
    held: Rc<Cell<Option<Instant>>>,
    keys: key::Encoder<'static>,
    key_event: key::Event<'static>,
    mouse: mouse::Encoder<'static>,
    mouse_event: mouse::Event<'static>,
    /// Selecting with the pointer: a click selects nothing, two a word, three a line; a drag
    /// grows the selection by what the click selected.
    gesture: Gesture<'static>,
    press: PressEvent<'static>,
    release: ReleaseEvent<'static>,
    drag: DragEvent<'static>,
    epoch: Instant,
    /// What the terminal answers its program with (cursor position, device attributes),
    /// for the view to send on.
    replies: Rc<RefCell<Vec<u8>>>,
    /// The size it tells a program that asks.
    told: Rc<Cell<Size>>,
    /// Output came since the rows were last brought up to date, or they wait for a hold
    /// to end.
    pub stale: bool,
}

fn ghostty_rgb(c: Rgb) -> RgbColor {
    RgbColor { r: c.r, g: c.g, b: c.b }
}

/// An sRGB byte colour as Rune's linear [`Color`].
pub fn rgb(c: RgbColor) -> Color {
    let lin = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    Color { r: lin(c.r), g: lin(c.g), b: lin(c.b), a: 1.0 }
}

impl Grid {
    /// A screen `size` big keeping `scrollback` lines, in `palette`'s colours.
    pub fn new(size: Size, scrollback: u32, palette: &Palette) -> Res<Grid> {
        let mut t = Terminal::new(size.cols, size.lines)?;
        // Ghostty's default byte limit keeps a few hundred wide lines; Lemur's is in lines.
        t.set_scrollback_max_bytes(None)?
            .set_scrollback_max_lines(scrollback_limit(scrollback))?
            .set_terminfo_name("xterm-256color")?;
        t.resize(size.cols, size.lines, u32::from(size.cell_width), u32::from(size.cell_height))?;
        // The terminal and its callbacks stay on the window's thread, as Ghostty's must.
        let replies = Rc::new(RefCell::new(Vec::new()));
        let back = replies.clone();
        t.on_pty_write(move |_, data| back.borrow_mut().extend_from_slice(data))?;
        let told = Rc::new(Cell::new(size));
        let asked = told.clone();
        t.on_size(move |_| {
            let s = asked.get();
            Some(SizeReportSize {
                rows: s.lines,
                columns: s.cols,
                cell_width: u32::from(s.cell_width),
                cell_height: u32::from(s.cell_height),
            })
        })?;
        let painter = Rc::new(RefCell::new(Painter {
            render: RenderState::new()?,
            row_it: RowIterator::new()?,
            cell_it: CellIterator::new()?,
            scratch: String::new(),
            ids: Vec::new(),
            version: 0,
            frame: Frame { rows: Vec::new(), cursor: None, fg: Color::WHITE, bg: Color::BLACK, cursor_color: Color::WHITE },
            selection: RgbColor { r: 0, g: 0, b: 0 },
            changed: false,
        }));
        // A program drawing in steps (Claude Code does, every frame) holds the screen
        // meanwhile: what shows is the frame it began with, until it lets go.
        let held = Rc::new(Cell::new(None));
        let (capture, hold) = (painter.clone(), held.clone());
        t.on_render_hold(move |term, on| {
            if on {
                // Not while the view paints; it shows the frame it has then.
                if let Ok(mut p) = capture.try_borrow_mut() {
                    let _ = p.paint(term);
                }
                hold.set(Some(Instant::now()));
            } else {
                hold.set(None);
            }
        })?;
        let mut grid = Grid {
            terminal: t,
            painter,
            held,
            keys: key::Encoder::new()?,
            key_event: key::Event::new()?,
            mouse: mouse::Encoder::new()?,
            mouse_event: mouse::Event::new()?,
            gesture: Gesture::new()?,
            press: {
                let mut p = PressEvent::new()?;
                p.set_repeat_interval(Duration::from_millis(500))?;
                p
            },
            release: ReleaseEvent::new()?,
            drag: DragEvent::new()?,
            epoch: Instant::now(),
            replies,
            told,
            stale: true,
        };
        grid.set_palette(palette)?;
        grid.refresh()?;
        Ok(grid)
    }

    pub fn set_palette(&mut self, p: &Palette) -> Res<()> {
        let mut all = [RgbColor { r: 0, g: 0, b: 0 }; 256];
        for (i, c) in all.iter_mut().enumerate() {
            *c = ghostty_rgb(p.indexed(i));
        }
        self.terminal
            .set_default_fg_color(Some(ghostty_rgb(p.foreground)))?
            .set_default_bg_color(Some(ghostty_rgb(p.background)))?
            .set_default_cursor_color(Some(ghostty_rgb(p.cursor)))?
            .set_default_color_palette(Some(style::Palette(all)))?;
        self.painter.borrow_mut().selection = ghostty_rgb(p.selection());
        self.stale = true;
        Ok(())
    }

    pub fn set_scrollback(&mut self, lines: u32) -> Res<()> {
        self.terminal.set_scrollback_max_lines(scrollback_limit(lines))?;
        Ok(())
    }

    /// Feeds a program's output in.
    pub fn write(&mut self, bytes: &[u8]) {
        self.terminal.vt_write(bytes);
        self.stale = true;
    }

    /// What the terminal answered its program with since last asked.
    pub fn take_replies(&self) -> Vec<u8> {
        std::mem::take(&mut *self.replies.borrow_mut())
    }

    pub fn size(&self) -> Size {
        self.told.get()
    }

    /// Takes `size`; true when it changed.
    pub fn resize(&mut self, size: Size) -> Res<bool> {
        if size == self.size() {
            return Ok(false);
        }
        self.terminal.resize(size.cols, size.lines, u32::from(size.cell_width), u32::from(size.cell_height))?;
        self.told.set(size);
        self.stale = true;
        Ok(true)
    }

    pub fn bracketed_paste(&self) -> bool {
        self.terminal.mode(Mode::BRACKETED_PASTE).unwrap_or(false)
    }

    pub fn mouse_tracking(&self) -> bool {
        self.terminal.is_mouse_tracking().unwrap_or(false)
    }

    /// Scrolls the view `lines` back (negative) or forward through the scrollback.
    pub fn scroll(&mut self, lines: isize) {
        self.terminal.scroll_viewport(ScrollViewport::Delta(lines));
        self.stale = true;
    }

    /// Back at the bottom, where output shows.
    pub fn scroll_to_bottom(&mut self) {
        self.terminal.scroll_viewport(ScrollViewport::Bottom);
        self.stale = true;
    }

    #[cfg(test)]
    pub fn scrollback_rows(&self) -> usize {
        self.terminal.scrollback_rows().unwrap_or(0)
    }

    /// The bytes for a key, as the terminal's modes say (cursor keys, Kitty keyboard flags),
    /// or None for keys the app keeps (⌘ chords). `base` is what the key types unmodified.
    pub fn key(&mut self, ev: &KeyEvent, base: Option<char>) -> Option<Vec<u8>> {
        // Option/Shift+Enter: Claude Code reads ESC CR as a newline in its prompt, as it did
        // in Lemur before. A program speaking Kitty's keyboard protocol gets that instead.
        let kitty = self.terminal.kitty_keyboard_flags().map(|f| !f.is_empty()).unwrap_or(false);
        if matches!(ev.key, Key::Enter) && (ev.mods.shift || ev.mods.alt) && !ev.mods.logo && !kitty {
            return Some(b"\x1b\r".to_vec());
        }
        let k = super::gkeys::translate(ev, base)?;
        self.key_event
            .set_action(key::Action::Press)
            .set_key(k.key)
            .set_mods(k.mods)
            .set_consumed_mods(k.consumed)
            .set_unshifted_codepoint(k.unshifted)
            .set_utf8(k.text.clone());
        let mut out = Vec::new();
        let encoded = self.keys.set_options_from_terminal(&self.terminal).encode_to_vec(&self.key_event, &mut out);
        if encoded.is_err() || out.is_empty() {
            out = k.text.map(String::into_bytes).unwrap_or_default();
        }
        (!out.is_empty()).then_some(out)
    }

    /// Brings the rows and the cursor up to the screen, unless the program holds it: then
    /// they stay as it began its hold, and stay stale, so the view looks again. True when
    /// anything changed.
    pub fn refresh(&mut self) -> Res<bool> {
        if let Some(at) = self.held.get() {
            if at.elapsed() < HOLD {
                return Ok(std::mem::take(&mut self.painter.borrow_mut().changed));
            }
            self.terminal.set_mode(Mode::SYNC_OUTPUT, false)?;
            self.held.set(None);
        }
        self.stale = false;
        let mut p = self.painter.borrow_mut();
        p.paint(&self.terminal)?;
        Ok(std::mem::take(&mut p.changed))
    }

    /// What shows: the rows as of the last refresh.
    pub fn frame(&self) -> Ref<'_, Frame> {
        Ref::map(self.painter.borrow(), |p| &p.frame)
    }

    /// The cell at `x`, `y` points from the grid's top-left, kept on the grid.
    fn point_at(&self, x: f32, y: f32, cell: (f32, f32)) -> Point {
        let size = self.size();
        let col = (x / cell.0).floor().clamp(0.0, f32::from(size.cols.saturating_sub(1))) as u16;
        let row = (y / cell.1).floor().clamp(0.0, f32::from(size.lines.saturating_sub(1))) as u32;
        Point::Viewport(PointCoordinate { x: col, y: row })
    }

    /// The cell at `x`, `y`, as (column, row).
    pub fn cell_at(&self, x: f32, y: f32, cell: (f32, f32)) -> (u16, u16) {
        match self.point_at(x, y, cell) {
            Point::Viewport(p) => (p.x, p.y as u16),
            _ => (0, 0),
        }
    }

    /// A press at `x`, `y` (points from the grid's top-left) starts a selection: the second
    /// click of a run selects a word, the third a line.
    pub fn select_press(&mut self, x: f32, y: f32, cell: (f32, f32)) -> Res<()> {
        let point = self.point_at(x, y, cell);
        let Grid { terminal, gesture, press, epoch, .. } = self;
        let at = terminal.grid_ref(point)?;
        let selection = press
            .set_repeat_distance(f64::from(cell.0 * UNIT))?
            .set_time(epoch.elapsed())?
            .set_position(f64::from(x * UNIT), f64::from(y * UNIT))?
            .apply(gesture, terminal, at)?;
        terminal.set_selection(selection.as_ref())?;
        self.stale = true;
        Ok(())
    }

    /// The pointer dragged to `x`, `y`; with `rectangle`, a block of columns.
    pub fn select_drag(&mut self, x: f32, y: f32, cell: (f32, f32), rectangle: bool) -> Res<()> {
        let size = self.size();
        let point = self.point_at(x, y, cell);
        let Grid { terminal, gesture, drag, .. } = self;
        let at = terminal.grid_ref(point)?;
        let geometry = Geometry {
            columns: u32::from(size.cols),
            cell_width: (cell.0 * UNIT) as u32,
            padding_left: 0,
            screen_height: (f32::from(size.lines) * cell.1 * UNIT) as u32,
        };
        let selection = drag.set_rectangle(rectangle)?.set_position(f64::from(x * UNIT), f64::from(y * UNIT))?.apply(gesture, terminal, at, geometry)?;
        terminal.set_selection(selection.as_ref())?;
        self.stale = true;
        Ok(())
    }

    /// The press came up at `x`, `y`.
    pub fn select_release(&mut self, x: f32, y: f32, cell: (f32, f32)) -> Res<()> {
        let point = self.point_at(x, y, cell);
        let Grid { terminal, gesture, release, .. } = self;
        let at = terminal.grid_ref(point).ok();
        release.apply(gesture, terminal, at)?;
        Ok(())
    }

    /// The selected text, as copying takes it; None with nothing selected.
    pub fn selected_text(&self) -> Option<String> {
        let options = FormatOptions::new().with_emit_format(Format::Plain).with_trim(true).with_unwrap(true);
        let bytes = self.terminal.format_selection_alloc(None, options).ok()??;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        (!text.is_empty()).then_some(text)
    }

    pub fn clear_selection(&mut self) {
        if self.terminal.selection().is_ok_and(|s| s.is_some()) {
            let _ = self.terminal.set_selection(None);
            self.stale = true;
        }
    }

    /// What tells the program of a press, a release or a move at `x`, `y`, as it asked
    /// (X10, SGR, ...); nothing where it asked for none of these.
    pub fn mouse_report(
        &mut self,
        action: mouse::Action,
        button: Option<mouse::Button>,
        (x, y): (f32, f32),
        mods: rune::Mods,
        cell: (f32, f32),
        any_pressed: bool,
    ) -> Vec<u8> {
        let size = self.size();
        let mut gmods = key::Mods::empty();
        for (on, flag) in [(mods.shift, key::Mods::SHIFT), (mods.ctrl, key::Mods::CTRL), (mods.alt, key::Mods::ALT)] {
            if on {
                gmods |= flag;
            }
        }
        self.mouse_event.set_action(action).set_button(button).set_mods(gmods).set_position(mouse::Position { x: x * UNIT, y: y * UNIT });
        let mut out = Vec::new();
        let _ = self
            .mouse
            .set_options_from_terminal(&self.terminal)
            .set_size(mouse::EncoderSize {
                screen_width: (f32::from(size.cols) * cell.0 * UNIT) as u32,
                screen_height: (f32::from(size.lines) * cell.1 * UNIT) as u32,
                cell_width: (cell.0 * UNIT) as u32,
                cell_height: (cell.1 * UNIT) as u32,
                padding_top: 0,
                padding_bottom: 0,
                padding_left: 0,
                padding_right: 0,
            })
            .set_any_button_pressed(any_pressed)
            .set_track_last_cell(true)
            .encode_to_vec(&self.mouse_event, &mut out);
        out
    }

    /// The wheel goes to the program as arrow keys: it's on the alternate screen (a pager,
    /// an editor) and asked for that (mode 1007).
    pub fn wheel_as_arrows(&self) -> bool {
        self.terminal.active_screen().is_ok_and(|s| s == Screen::Alternate) && self.terminal.mode(Mode::ALT_SCROLL).unwrap_or(false)
    }

    /// The arrow keys in the program's cursor key mode.
    pub fn arrow(&self, up: bool) -> &'static [u8] {
        match (self.terminal.mode(Mode::DECCKM).unwrap_or(false), up) {
            (true, true) => b"\x1bOA",
            (true, false) => b"\x1bOB",
            (false, true) => b"\x1b[A",
            (false, false) => b"\x1b[B",
        }
    }

    /// The link a program put on the cell at (`col`, `row`) (OSC 8), if it put one there.
    pub fn hyperlink(&self, col: u16, row: u16) -> Option<String> {
        let at = self.terminal.grid_ref(Point::Viewport(PointCoordinate { x: col, y: u32::from(row) })).ok()?;
        let mut buf = vec![0u8; 512];
        let len = match at.hyperlink_uri(&mut buf) {
            Ok(len) => len,
            Err(libghostty_vt::error::Error::OutOfSpace { required }) => {
                buf.resize(required.max(buf.len() * 8), 0);
                at.hyperlink_uri(&mut buf).ok()?
            }
            Err(_) => return None,
        };
        (len > 0).then(|| String::from_utf8_lossy(&buf[..len]).into_owned())
    }

    /// Row `y`'s characters and the column each is at, the spacer after a wide one left
    /// out, for finding the links in it.
    pub fn line_cells(&self, y: usize) -> Vec<(char, u16)> {
        let cols = self.size().cols;
        let mut cells: Vec<Option<char>> = vec![Some(' '); usize::from(cols)];
        if let Some(row) = self.frame().rows.get(y) {
            for r in &row.runs {
                if r.text.is_ascii() {
                    for (i, c) in r.text.chars().enumerate() {
                        if let Some(slot) = cells.get_mut(usize::from(r.col) + i) {
                            *slot = Some(c);
                        }
                    }
                } else {
                    let at = usize::from(r.col);
                    if let Some(slot) = cells.get_mut(at) {
                        *slot = r.text.chars().next();
                    }
                    for spacer in 1..usize::from(r.cells) {
                        if let Some(slot) = cells.get_mut(at + spacer) {
                            *slot = None;
                        }
                    }
                }
            }
        }
        cells.into_iter().enumerate().filter_map(|(i, c)| Some((c?, i as u16))).collect()
    }

    /// The screen's text, row by row, for tests.
    #[cfg(test)]
    pub fn lines(&self) -> Vec<String> {
        self.frame().rows.iter().map(Row::text).collect()
    }
}

impl Painter {
    /// Brings the frame up to the terminal. A row keeps its id as output scrolls it up, so
    /// one Ghostty didn't mark dirty is taken from the last paint by id, wherever it now
    /// sits; the rest are rebuilt from their cells.
    fn paint(&mut self, terminal: &Terminal<'static, '_>) -> Res<()> {
        let Painter { render, row_it, cell_it, scratch, ids, version, frame, selection, changed } = self;
        let snapshot = render.update(terminal)?;
        if snapshot.dirty()? == Dirty::Clean {
            return Ok(());
        }
        let colors = snapshot.colors()?;
        let mut last: HashMap<RowId, Row> = ids.drain(..).zip(frame.rows.drain(..)).collect();
        let mut iter = row_it.update(&snapshot)?;
        while let Some(row) = iter.next() {
            let id = row.id()?;
            let kept = if row.dirty()? { None } else { last.remove(&id) };
            let built = match kept {
                Some(r) => r,
                None => {
                    *version += 1;
                    build_row(&mut cell_it.update(row)?, &colors, *selection, scratch, *version)?
                }
            };
            frame.rows.push(built);
            ids.push(id);
            row.set_dirty(false)?;
        }
        let cursor = snapshot.cursor()?;
        frame.cursor = cursor.viewport.filter(|_| cursor.visible).map(|c| (c.x, c.y));
        frame.fg = rgb(colors.foreground);
        frame.bg = rgb(colors.background);
        frame.cursor_color = rgb(colors.cursor.unwrap_or(colors.foreground));
        snapshot.set_dirty(Dirty::Clean)?;
        *changed = true;
        Ok(())
    }
}

/// What a block element (U+2580–U+259F) covers of its cell, as (left, top, right, bottom)
/// fractions, and how opaque it is. The font's glyphs don't fill the cell, so neighbours
/// would show seams between them (Claude Code's logo is made of them).
pub fn block(c: char) -> Option<(Vec<[f32; 4]>, f32)> {
    // Asked of every cell built: most are no block, and cost nothing.
    if !('\u{2580}'..='\u{259F}').contains(&c) {
        return None;
    }
    // Upper left, upper right, lower left, lower right.
    let quadrants = |on: [bool; 4]| -> Vec<[f32; 4]> {
        [[0.0, 0.0, 0.5, 0.5], [0.5, 0.0, 1.0, 0.5], [0.0, 0.5, 0.5, 1.0], [0.5, 0.5, 1.0, 1.0]]
            .into_iter()
            .zip(on)
            .filter_map(|(rect, on)| on.then_some(rect))
            .collect()
    };
    let eighths = |n: u32| n as f32 / 8.0;
    let full = vec![[0.0, 0.0, 1.0, 1.0]];
    let rects = match c {
        '▀' => vec![[0.0, 0.0, 1.0, 0.5]],
        // Lower one eighth up to the full block.
        '▁'..='█' => vec![[0.0, 1.0 - eighths(c as u32 - 0x2580), 1.0, 1.0]],
        // Left seven eighths down to one eighth.
        '▉'..='▏' => vec![[0.0, 0.0, eighths(0x2590 - c as u32), 1.0]],
        '▐' => vec![[0.5, 0.0, 1.0, 1.0]],
        '░' => return Some((full, 0.25)),
        '▒' => return Some((full, 0.5)),
        '▓' => return Some((full, 0.75)),
        '▔' => vec![[0.0, 0.0, 1.0, 0.125]],
        '▕' => vec![[0.875, 0.0, 1.0, 1.0]],
        '▖' => quadrants([false, false, true, false]),
        '▗' => quadrants([false, false, false, true]),
        '▘' => quadrants([true, false, false, false]),
        '▙' => quadrants([true, false, true, true]),
        '▚' => quadrants([true, false, false, true]),
        '▛' => quadrants([true, true, true, false]),
        '▜' => quadrants([true, true, false, true]),
        '▝' => quadrants([false, true, false, false]),
        '▞' => quadrants([false, true, true, false]),
        '▟' => quadrants([false, true, true, true]),
        _ => return None,
    };
    Some((rects, 1.0))
}

/// What a cell looks like, for joining cells into runs.
#[derive(Clone, Copy, PartialEq)]
struct Look {
    fg: RgbColor,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}

/// One row's cells as runs and backgrounds. Blank cells between two cells of one look join
/// their run as spaces; anything that isn't one-cell ASCII stands alone at its column.
fn build_row(cells: &mut CellIteration<'_, '_>, colors: &Colors, selection: RgbColor, scratch: &mut String, version: u64) -> Res<Row> {
    let mut row = Row { version, ..Row::default() };
    let mut run: Option<(Run, Look)> = None;
    let mut blanks = 0u16;
    let mut x = 0u16;
    let flush = |run: &mut Option<(Run, Look)>, row: &mut Row| {
        if let Some((r, _)) = run.take() {
            row.runs.push(r);
        }
    };
    while let Some(cell) = cells.next() {
        let col = x;
        x += 1;
        let wide = cell.raw_cell()?.wide()?;
        if matches!(wide, CellWide::SpacerTail | CellWide::SpacerHead) {
            continue;
        }
        let width = if wide == CellWide::Wide { 2 } else { 1 };
        let mut fg = cell.fg_color()?.unwrap_or(colors.foreground);
        let mut bg = cell.bg_color()?;
        let mut look = Look { fg, bold: false, italic: false, underline: false, strike: false };
        let mut invisible = false;
        if cell.has_styling()? {
            let st = cell.style()?;
            if st.inverse {
                let back = bg.unwrap_or(colors.background);
                bg = Some(fg);
                fg = back;
            }
            if st.faint {
                let mid = |a: u8, b: u8| ((u16::from(a) + u16::from(b)) / 2) as u8;
                let b = colors.background;
                fg = RgbColor { r: mid(fg.r, b.r), g: mid(fg.g, b.g), b: mid(fg.b, b.b) };
            }
            look = Look {
                fg,
                bold: st.bold,
                italic: st.italic,
                underline: st.underline != Underline::None,
                strike: st.strikethrough,
            };
            invisible = st.invisible;
        }
        if cell.is_selected()? {
            bg = Some(selection);
        }
        if let Some(bg) = bg {
            let color = rgb(bg);
            match row.fills.last_mut() {
                Some(f) if f.col + f.cells == col && f.color == color => f.cells += width,
                _ => row.fills.push(Fill { col, cells: width, color }),
            }
        }
        if invisible || cell.graphemes_len()? == 0 {
            if run.is_some() {
                blanks += width;
            }
            continue;
        }
        cell.graphemes_utf8(scratch)?;
        let mut chars = scratch.chars();
        if let (Some(c), None) = (chars.next(), chars.next())
            && block(c).is_some()
        {
            row.blocks.push((col, c, rgb(look.fg)));
            if run.is_some() {
                blanks += width;
            }
            continue;
        }
        let plain = width == 1 && scratch.is_ascii();
        match &mut run {
            Some((r, l)) if plain && *l == look && r.text.is_ascii() => {
                for _ in 0..blanks {
                    r.text.push(' ');
                }
                r.text.push_str(scratch);
                r.cells += blanks + 1;
            }
            _ => {
                flush(&mut run, &mut row);
                let r = Run {
                    col,
                    cells: width,
                    text: scratch.clone(),
                    fg: rgb(look.fg),
                    bold: look.bold,
                    italic: look.italic,
                    underline: look.underline,
                    strike: look.strike,
                };
                run = Some((r, look));
                if !plain {
                    flush(&mut run, &mut row);
                }
            }
        }
        blanks = 0;
    }
    flush(&mut run, &mut row);
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::palette::DARK;

    fn grid(cols: u16, lines: u16) -> Grid {
        Grid::new(Size { cols, lines, cell_width: 16, cell_height: 32 }, 1000, &DARK).unwrap()
    }

    #[test]
    fn rows_stay_right_as_output_scrolls_them_up() {
        let mut g = grid(80, 24);
        let out: String = (1..=3000).map(|n| format!("{n}\r\n")).collect();
        for chunk in out.as_bytes().chunks(1000) {
            g.write(chunk);
            g.refresh().unwrap();
        }
        let want: Vec<String> = (2978..=3000).map(|n| n.to_string()).chain([String::new()]).collect();
        assert_eq!(g.lines(), want);
    }

    #[test]
    fn wide_characters_stand_at_their_columns() {
        let mut g = grid(40, 4);
        g.write("中文字|wide\r\nabcdef|ascii".as_bytes());
        g.refresh().unwrap();
        let cols: Vec<u16> = g.frame().rows[0].runs.iter().map(|r| r.col).collect();
        assert_eq!(cols, [0, 2, 4, 6]);
        assert_eq!(g.frame().rows[0].text(), "中文字|wide");
        assert_eq!(g.frame().rows[1].runs.len(), 1);
    }

    /// A program holding the screen while it draws shows the frame it began with, then the
    /// finished one; one that never lets go shows what it drew after a second.
    #[test]
    fn a_hold_on_the_screen_shows_whole_frames() {
        let mut g = grid(20, 3);
        g.write(b"one");
        g.write(b"\x1b[?2026h\x1b[2J\x1b[Htw");
        g.refresh().unwrap();
        assert_eq!(g.lines()[0], "one", "the frame it began with");
        assert!(g.stale, "to look again");
        g.write(b"o\x1b[?2026l\x1b[?2026h\x1b[2J\x1b[Hthr");
        g.refresh().unwrap();
        assert_eq!(g.lines()[0], "two", "the frame it finished, though it began the next");
        std::thread::sleep(HOLD);
        g.refresh().unwrap();
        assert_eq!(g.lines()[0], "thr", "it held on too long");
        assert!(!g.stale);
    }

    /// A link a program puts on its text spans that text; only a web link opens.
    #[test]
    fn a_programs_own_links_are_found() {
        use super::super::term_links::hyperlinked;
        let mut g = grid(40, 3);
        g.write(b"see \x1b]8;;https://example.com/a\x1b\\the docs\x1b]8;;\x1b\\ now\r\n");
        g.write(b"\x1b]8;;file:///etc/passwd\x1b\\/tmp/x\x1b]8;;\x1b\\");
        assert_eq!(g.hyperlink(3, 0), None);
        let uri = g.hyperlink(6, 0).expect("a link");
        let link = hyperlinked(uri, 0, 6, 40, |c| g.hyperlink(c, 0).as_deref() == Some("https://example.com/a")).unwrap();
        assert_eq!((link.url.as_str(), link.start, link.end), ("https://example.com/a", 4, 12));
        let other = g.hyperlink(1, 1).expect("a link");
        assert_eq!(hyperlinked(other, 1, 1, 40, |_| true), None, "not a web link");
    }

    /// It keeps at least as many lines as set, whatever the width, and drops the rest a
    /// page at a time.
    #[test]
    fn scrollback_keeps_as_many_lines_as_set() {
        for cols in [80, 200] {
            let mut g = grid(cols, 24);
            for (set, round) in [(1000, 0), (3000, 1), (500, 2)] {
                g.set_scrollback(set).unwrap();
                for _ in 0..4 {
                    let lines: String = (0..5_000).map(|i| format!("{round}-{i}\r\n")).collect();
                    g.write(lines.as_bytes());
                    let kept = g.scrollback_rows() as u32;
                    assert!((set..=set + PAGE_SLACK).contains(&kept), "{cols} columns, {set} set: {kept}");
                }
            }
        }
    }

    #[test]
    fn block_elements_are_painted_not_typed() {
        let mut g = grid(40, 3);
        g.write("a▐▛x".as_bytes());
        g.refresh().unwrap();
        assert_eq!(g.lines()[0], "a  x");
        let blocks: Vec<_> = g.frame().rows[0].blocks.iter().map(|b| (b.0, b.1)).collect();
        assert_eq!(blocks, [(1, '▐'), (2, '▛')]);

        assert_eq!(block('▁'), Some((vec![[0.0, 0.875, 1.0, 1.0]], 1.0)));
        assert_eq!(block('█'), Some((vec![[0.0, 0.0, 1.0, 1.0]], 1.0)));
        assert_eq!(block('▉'), Some((vec![[0.0, 0.0, 0.875, 1.0]], 1.0)));
        assert_eq!(block('▏'), Some((vec![[0.0, 0.0, 0.125, 1.0]], 1.0)));
        assert_eq!(block('a'), None);
    }

    #[test]
    fn queries_are_answered_for_the_program() {
        let mut g = grid(80, 24);
        g.write(b"\x1b[6n");
        assert_eq!(g.take_replies(), b"\x1b[1;1R");
        assert!(g.take_replies().is_empty());
    }

    #[test]
    fn a_shifted_enter_is_escape_return() {
        let mut g = grid(80, 24);
        let ev = KeyEvent { key: Key::Enter, mods: rune::Mods { shift: true, ..Default::default() } };
        assert_eq!(g.key(&ev, None).unwrap(), b"\x1b\r");
        let ctrl_c = KeyEvent { key: Key::Text("c".into()), mods: rune::Mods { ctrl: true, ..Default::default() } };
        assert_eq!(g.key(&ctrl_c, None).unwrap(), b"\x03");
        let cmd = KeyEvent { key: Key::Command('c'), mods: rune::Mods { cmd: true, logo: true, ..Default::default() } };
        assert!(g.key(&cmd, None).is_none());
    }
}
