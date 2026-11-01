//! Draws an attached terminal's grid with GPUI and feeds it keys, mouse,
//! pastes and drops. Dropped or pasted images go to the host first; the
//! session gets the path.

use super::keys::{self, Key};
use super::links::{Link, link_at};
use super::settings::Prefs;
use super::snackbar::Kind;
use super::terminal::{Palette, Screen, Size, hex};
use crate::actions;
use crate::config::Host;
use crate::hc::{self, Core, Stream};
use alacritty_terminal::event::Event as TermEvent;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{CursorShape, NamedColor, Rgb};
use futures::StreamExt;
use gpui_kit::component::ActiveTheme;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// Key context for terminal-only key bindings.
pub const CONTEXT: &str = "Terminal";

/// The least time between attaching again on its own after a drop.
const RETRY: std::time::Duration = std::time::Duration::from_secs(2);

/// Output is drawn at most this often, about 30 times a second, as
/// Herdr does: a program printing fast redraws the window far less, and
/// looks the same. Right after typing (`TYPING`), at once.
const OUTPUT_EVERY: std::time::Duration = std::time::Duration::from_millis(33);
const TYPING: std::time::Duration = std::time::Duration::from_millis(200);

/// Where this view is with its terminal's byte stream.
enum Attachment {
    Attaching,
    /// The core, and the channel the terminal's bytes come on.
    Attached(Arc<Core>, u32),
    /// The stream ended: the program exited, or the line dropped.
    Ended,
}

pub struct TerminalView {
    pub host: Host,
    pub session: String,
    screen: Option<Screen>,
    attachment: Attachment,
    focus: FocusHandle,
    /// Where the grid was last painted, and the size of its cells, for
    /// mapping mouse positions to cells. `cell` rounds that size for the
    /// program.
    bounds: Bounds<Pixels>,
    cell_size: gpui_kit::Size<Pixels>,
    cell: Size,
    /// Measured on the next render after Settings change the font.
    metrics: Option<Metrics>,
    /// The button the program was told is down, and the cell it was last at.
    pressed: Option<(keys::MouseButton, (usize, usize))>,
    /// A selection is being dragged out.
    selecting: bool,
    /// Text an input method is still composing (a dead key's accent, or
    /// Japanese before it's converted), shown at the cursor until it's typed.
    marked: Option<String>,
    /// Where the pointer is over the terminal, and the link under it while
    /// ⌘ is held: underlined, and opened by a ⌘-click.
    pointer: Option<Point<Pixels>>,
    hovered: Option<Link>,
    scroll_lines: f32,
    /// What the core was last told: the colors and cell size lemur shows
    /// the terminal in, and whether it has the keyboard.
    told_display: Option<Facts>,
    told_focus: Option<bool>,
    /// Shown in the window's snackbars while this terminal is.
    status: Option<(String, Kind)>,
    /// The stream dropped while its program may still run: the line to its
    /// host or to herdr-core went, or herdr-core is restarting a kept
    /// program. Lemur attaches again once the terminal runs (`reconnect`).
    lost: bool,
    /// When it last tried to attach.
    tried: Option<Instant>,
    _reader: Option<Task<()>>,
    /// Shows a synchronized update that never said it was done.
    _sync: Option<Task<()>>,
    /// When you last typed into it, so its echo shows at once.
    typed_at: std::cell::Cell<Option<Instant>>,
    /// Draws output that came since the last frame, at `OUTPUT_EVERY`.
    _redraw: Option<Task<()>>,
    #[cfg(test)]
    renders: usize,
    _prefs: Subscription,
}

/// The terminal's message came or went.
pub struct StatusChanged;

impl EventEmitter<StatusChanged> for TerminalView {}

/// Text pasted with ⌘V, as it was on the clipboard.
pub struct Pasted(pub String);

impl EventEmitter<Pasted> for TerminalView {}

/// The colors and cell size the terminal shows in, as the core answers
/// programs that ask (a theme picks light or dark by the background).
/// Compared each frame, so it's only sent when it changes.
#[derive(Clone, Copy, PartialEq)]
struct Facts {
    fg: Rgb,
    bg: Rgb,
    cursor: Rgb,
    cell: [u16; 2],
}

impl Facts {
    fn new(palette: &Palette, cell: Size) -> Facts {
        Facts {
            fg: palette.foreground,
            bg: palette.background,
            cursor: palette.indexed(NamedColor::Cursor as usize, &Default::default()),
            cell: [cell.cell_width, cell.cell_height],
        }
    }

    fn json(self) -> serde_json::Value {
        serde_json::json!({
            "fg": hex(self.fg),
            "bg": hex(self.bg),
            "cursor": hex(self.cursor),
            "cell": self.cell,
        })
    }
}

/// The terminal font and the size of one cell in it.
#[derive(Clone)]
struct Metrics {
    font: Font,
    size: Pixels,
    cell_width: Pixels,
    line_height: Pixels,
}

impl Drop for TerminalView {
    fn drop(&mut self) {
        self.detach();
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TerminalView {
    pub fn new(host: Host, session: String, cx: &mut Context<Self>) -> Self {
        let mut view = TerminalView {
            host,
            session,
            screen: None,
            attachment: Attachment::Ended,
            focus: cx.focus_handle(),
            bounds: Bounds::default(),
            cell_size: size(px(8.0), px(16.0)),
            cell: Size {
                cols: 80,
                lines: 24,
                cell_width: 8,
                cell_height: 16,
            },
            metrics: None,
            pressed: None,
            selecting: false,
            marked: None,
            pointer: None,
            hovered: None,
            scroll_lines: 0.0,
            told_display: None,
            told_focus: None,
            status: None,
            lost: false,
            tried: None,
            _reader: None,
            _sync: None,
            typed_at: std::cell::Cell::new(None),
            _redraw: None,
            #[cfg(test)]
            renders: 0,
            _prefs: cx.observe_global::<Prefs>(|this, cx| {
                this.metrics = None;
                let scrollback = Prefs::get(cx).terminal.scrollback;
                if let Some(screen) = &mut this.screen {
                    screen.set_scrollback(scrollback);
                }
                cx.notify();
            }),
        };
        view.connect(cx);
        view
    }

    #[cfg(test)]
    pub fn screen_size(&self) -> Option<Size> {
        self.screen.as_ref().map(|s| s.size())
    }

    #[cfg(test)]
    pub fn is_attached(&self) -> bool {
        matches!(self.attachment, Attachment::Attached(..))
    }

    /// Its stream ended, so it shows what it last had.
    pub fn exited(&self) -> bool {
        matches!(self.attachment, Attachment::Ended)
    }

    pub fn status(&self) -> Option<&(String, Kind)> {
        self.status.as_ref()
    }

    /// Closes the message, as its × does.
    pub fn dismiss_status(&mut self, cx: &mut Context<Self>) {
        self.set_status(None, cx);
        cx.notify();
    }

    fn set_status(&mut self, status: Option<(String, Kind)>, cx: &mut Context<Self>) {
        if self.status != status {
            self.status = status;
            cx.emit(StatusChanged);
        }
    }

    /// Whether its stream dropped while its program may still run.
    pub fn disconnected(&self) -> bool {
        self.lost && matches!(self.attachment, Attachment::Ended)
    }

    /// Attaches again after the stream dropped, as Herdr does, once its
    /// terminal runs again; at most every `RETRY`, while attaching fails.
    pub fn reconnect(&mut self, cx: &mut Context<Self>) {
        if self.disconnected() && self.tried.is_none_or(|t| t.elapsed() >= RETRY) {
            self.connect(cx);
        }
    }

    /// (Re)attaches to this session's terminal, e.g. after the line to its
    /// host dropped. The core paints its screen and scrollback first.
    pub fn connect(&mut self, cx: &mut Context<Self>) {
        self.detach();
        let scrollback = Prefs::get(cx).terminal.scrollback;
        self.screen = Some(Screen::new(self.cell, scrollback));
        self.attachment = Attachment::Attaching;
        self.tried = Some(Instant::now());
        self.told_display = None;
        self.told_focus = None;
        // Reconnecting, it still says so until it's back.
        if !self.lost {
            self.set_status(None, cx);
        }
        let (host, session) = (self.host.clone(), self.session.clone());
        let (cols, lines) = (self.cell.cols, self.cell.lines);
        let palette = Palette::themed(cx.theme().is_dark(), &Prefs::get(cx).terminal);
        let facts = Facts::new(&palette, self.cell);
        self.told_display = Some(facts);
        let display = facts.json();
        self._reader = Some(cx.spawn(async move |this, cx| {
            // A terminal on a host still being reached can take seconds.
            let attached = cx
                .background_spawn(async move {
                    let (core, _) = hc::for_host(&host)?;
                    let attached = core.attach(&session, cols, lines, scrollback, display)?;
                    anyhow::Ok((core, attached))
                })
                .await;
            let mut stream = match attached {
                Ok((core, attached)) => {
                    let channel = attached.channel;
                    let shown = this.update(cx, |this, cx| this.attached(core, channel, cx));
                    if shown.is_err() {
                        return;
                    }
                    attached.stream
                }
                Err(e) => {
                    this.update(cx, |this, cx| {
                        // Tried again once the terminal runs.
                        this.attachment = Attachment::Ended;
                        this.lost = true;
                        let error = format!("{e:#} · trying again");
                        this.set_status(Some((error, Kind::Error)), cx);
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            while let Some(first) = stream.next().await {
                let mut batch = vec![first];
                while let Ok(more) = stream.try_recv() {
                    batch.push(more);
                }
                if !matches!(this.update(cx, |this, cx| this.ingest(batch, cx)), Ok(true)) {
                    break;
                }
            }
        }));
        cx.notify();
    }

    /// Lets go of the terminal's stream, so the core stops sending it.
    fn detach(&mut self) {
        if let Attachment::Attached(core, channel) = &self.attachment {
            core.detach(*channel);
        }
        self.attachment = Attachment::Ended;
    }

    fn attached(&mut self, core: Arc<Core>, channel: u32, cx: &mut Context<Self>) {
        // The terminal takes the size it's shown at. The attach asked for
        // the size known then, often before the first layout, and the core
        // goes by a viewer's size only once the viewer says something.
        core.resize(channel, self.cell.cols, self.cell.lines);
        self.attachment = Attachment::Attached(core, channel);
        if std::mem::take(&mut self.lost) {
            self.set_status(None, cx);
        }
        cx.notify();
    }

    /// Feeds relayed bytes in; false once the stream is over.
    fn ingest(&mut self, batch: Vec<Stream>, cx: &mut Context<Self>) -> bool {
        let Some(screen) = &mut self.screen else {
            return false;
        };
        let mut open = true;
        // Its program ended: the thread goes from the sidebar, and this with it.
        let mut over = false;
        for item in batch {
            match item {
                Stream::Data(bytes) => {
                    screen.advance(&bytes);
                    if let Attachment::Attached(core, channel) = &self.attachment {
                        core.credit(*channel, bytes.len());
                    }
                }
                Stream::Closed(why) => {
                    open = false;
                    over = matches!(why["reason"].as_str(), Some("exited" | "killed"));
                }
                Stream::Lost => open = false,
            }
        }
        let events = screen.events();
        let deadline = screen.sync_deadline();
        for event in events {
            self.on_term_event(event, cx);
        }
        if let Some(deadline) = deadline
            && self._sync.is_none()
        {
            self._sync = Some(cx.spawn(async move |this, cx| {
                let wait = deadline.saturating_duration_since(Instant::now());
                cx.background_executor().timer(wait).await;
                this.update(cx, |this, cx| {
                    this._sync = None;
                    if let Some(screen) = &mut this.screen
                        && screen.sync_deadline().is_some_and(|d| d <= Instant::now())
                    {
                        screen.stop_sync();
                        cx.notify();
                    }
                })
                .ok();
            }));
        }
        // New output may have moved the link under the pointer.
        if self.hovered.is_some() {
            self.hovered = self.pointer.and_then(|p| self.link_at(p));
        }
        if !open {
            self.attachment = Attachment::Ended;
            self.lost = !over;
            let note = (!over).then(|| ("disconnected · reconnecting…".into(), Kind::Warning));
            self.set_status(note, cx);
            cx.notify();
        } else {
            self.redraw_soon(cx);
        }
        open
    }

    /// Draws new output at the next `OUTPUT_EVERY`, or at once while you
    /// type.
    fn redraw_soon(&mut self, cx: &mut Context<Self>) {
        if self.typed_at.get().is_some_and(|t| t.elapsed() < TYPING) {
            cx.notify();
            return;
        }
        if self._redraw.is_some() {
            return;
        }
        self._redraw = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(OUTPUT_EVERY).await;
            this.update(cx, |this, cx| {
                this._redraw = None;
                cx.notify();
            })
            .ok();
        }));
    }

    fn on_term_event(&mut self, event: TermEvent, cx: &mut Context<Self>) {
        match event {
            TermEvent::PtyWrite(text) => self.write(text.into_bytes()),
            // Programs copy with OSC 52.
            TermEvent::ClipboardStore(_, text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text))
            }
            TermEvent::ColorRequest(index, format) => {
                let Some(screen) = &self.screen else { return };
                let palette = Palette::themed(cx.theme().is_dark(), &Prefs::get(cx).terminal);
                let color = palette.indexed(index, screen.term.lock().colors());
                self.write(format(color).into_bytes());
            }
            TermEvent::TextAreaSizeRequest(format) => {
                self.write(format(self.cell.window()).into_bytes())
            }
            _ => {}
        }
    }

    fn mode(&self) -> TermMode {
        self.screen
            .as_ref()
            .map(|s| *s.term.lock().mode())
            .unwrap_or(TermMode::NONE)
    }

    fn write(&self, bytes: Vec<u8>) {
        self.typed_at.set(Some(Instant::now()));
        if let Attachment::Attached(core, channel) = &self.attachment {
            core.input(*channel, &bytes);
        }
    }

    pub fn paste_text(&self, text: &str) {
        self.write(keys::paste(
            text,
            self.mode().contains(TermMode::BRACKETED_PASTE),
        ));
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let m = &ks.modifiers;
        if m.platform {
            if ks.key == "v" && !m.shift && !m.alt && !m.control {
                self.paste_clipboard(cx);
                cx.stop_propagation();
            }
            if ks.key == "c" && !m.shift && !m.alt && !m.control {
                self.copy(cx);
                cx.stop_propagation();
            }
            return;
        }
        let key = Key {
            key: &ks.key,
            key_char: ks.key_char.as_deref(),
            ctrl: m.control,
            alt: m.alt,
            shift: m.shift,
            caps_lock: window.capslock().on,
        };
        let app_cursor = self.mode().contains(TermMode::APP_CURSOR);
        // Text is left to the input method (`TerminalInput`), which composes
        // accents and other scripts; a held key repeats here instead.
        let bytes = keys::encode(&key, app_cursor)
            .or_else(|| event.is_held.then(|| keys::typed(&key)).flatten());
        if let Some(bytes) = bytes {
            self.send(bytes);
            cx.stop_propagation();
        }
    }

    /// Sends what the user typed, and shows the bottom of the scrollback.
    fn send(&self, bytes: Vec<u8>) {
        if let Some(screen) = &self.screen {
            let mut term = screen.term.lock();
            term.scroll_display(Scroll::Bottom);
            term.selection = None;
        }
        self.write(bytes);
    }

    /// ⌘C: the selected text goes to the clipboard.
    fn copy(&mut self, cx: &mut Context<Self>) {
        let text = self
            .screen
            .as_ref()
            .and_then(|s| s.term.lock().selection_to_string())
            .filter(|t| !t.is_empty());
        if let Some(text) = text {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Text from the input method, which ends any composing.
    fn type_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.marked = None;
        self.send(text.as_bytes().to_vec());
        cx.notify();
    }

    /// Cmd+V: files and images are uploaded and pasted as remote paths;
    /// text is pasted as is.
    fn paste_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let mut paths = vec![];
        let mut image = None;
        let mut text = None;
        for entry in item.into_entries() {
            match entry {
                ClipboardEntry::ExternalPaths(p) => paths.extend(p.0),
                ClipboardEntry::Image(i) => image = Some(i),
                ClipboardEntry::String(s) => text = Some(s.text),
            }
        }
        // Finder puts a file's icon next to its path; the path wins.
        if !paths.is_empty() {
            self.upload(paths, None, cx);
        } else if image.is_some() {
            self.upload(vec![], image, cx);
        } else if let Some(text) = text {
            self.paste_text(&text);
            cx.emit(Pasted(text));
        }
    }

    /// Copies files (and a pasted image) to the host, then pastes their
    /// remote paths.
    fn upload(&mut self, mut files: Vec<PathBuf>, image: Option<Image>, cx: &mut Context<Self>) {
        let host = self.host.clone();
        let count = files.len() + usize::from(image.is_some());
        self.set_status(
            Some((format!("uploading {count} file(s)…"), Kind::Progress)),
            cx,
        );
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    // An image goes up as a temporary file.
                    let temporary = image.map(|i| save_image(&i)).transpose()?;
                    files.extend(temporary.clone());
                    let uploaded = actions::upload(&host, &files);
                    if let Some(t) = temporary {
                        let _ = std::fs::remove_file(t);
                    }
                    uploaded
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(paths) => {
                        this.set_status(None, cx);
                        this.paste_text(&actions::paste_line(&paths));
                    }
                    Err(e) => {
                        this.set_status(Some((format!("upload failed: {e:#}"), Kind::Error)), cx)
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Copies a file the session printed the path of from the host, then
    /// shows it with Quick Look.
    fn preview(&mut self, path: String, cx: &mut Context<Self>) {
        let (host, session) = (self.host.clone(), self.session.clone());
        self.set_status(Some((format!("downloading {path}…"), Kind::Progress)), cx);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let file = actions::download(&host, &session, &path)?;
                    actions::quick_look(&file)
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => this.set_status(None, cx),
                    Err(e) => {
                        this.set_status(Some((format!("download failed: {e:#}"), Kind::Error)), cx)
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn cell_at(&self, position: Point<Pixels>) -> (usize, usize) {
        let b = self.bounds;
        let col = ((position.x - b.origin.x) / self.cell_size.width).max(0.0) as usize;
        let line = ((position.y - b.origin.y) / self.cell_size.height).max(0.0) as usize;
        (
            col.min(self.cell.cols.saturating_sub(1) as usize),
            line.min(self.cell.lines.saturating_sub(1) as usize),
        )
    }

    /// The grid point at `position`, scrollback counted, and which half of
    /// its cell it's in: where a selection starts or ends.
    fn grid_point(&self, position: Point<Pixels>) -> (GridPoint, Side) {
        let (col, line) = self.cell_at(position);
        let offset = self
            .screen
            .as_ref()
            .map_or(0, |s| s.term.lock().grid().display_offset());
        let x = (position.x - self.bounds.origin.x) / self.cell_size.width;
        let side = if x - col as f32 > 0.5 && x >= 0.0 {
            Side::Right
        } else {
            Side::Left
        };
        let point = GridPoint::new(Line(line as i32 - offset as i32), Column(col));
        (point, side)
    }

    /// The link at `position`, if it's over the grid.
    fn link_at(&self, position: Point<Pixels>) -> Option<Link> {
        if !self.bounds.contains(&position) {
            return None;
        }
        let (col, line) = self.cell_at(position);
        link_at(&self.screen.as_ref()?.term.lock(), col, line)
    }

    /// Follows the pointer and ⌘ (`platform`): the link under the pointer
    /// shows while ⌘ is held.
    fn hover(&mut self, platform: bool, cx: &mut Context<Self>) {
        let link = self
            .pointer
            .filter(|_| platform)
            .and_then(|p| self.link_at(p));
        if link != self.hovered {
            self.hovered = link;
            cx.notify();
        }
    }

    /// A program in mouse mode wants SGR reports; otherwise lemur selects.
    fn reports_mouse(mode: TermMode) -> bool {
        mode.intersects(TermMode::MOUSE_MODE) && mode.contains(TermMode::SGR_MOUSE)
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if self.exited() {
            self.connect(cx);
            return;
        }
        // ⌘-click opens a link; the program never sees that click.
        if event.button == MouseButton::Left
            && event.modifiers.platform
            && let Some(link) = self.link_at(event.position)
        {
            if link.file {
                self.preview(link.url, cx);
            } else {
                cx.open_url(&link.url);
            }
            return;
        }
        // ⇧ selects even in a program that takes the mouse.
        if let Some(button) = button(event.button)
            && Self::reports_mouse(self.mode())
            && !event.modifiers.shift
        {
            let (col, line) = self.cell_at(event.position);
            self.pressed = Some((button, (col, line)));
            self.write(keys::sgr_mouse(
                button,
                false,
                mods(event.modifiers),
                col,
                line,
                true,
            ));
            return;
        }
        if event.button == MouseButton::Left
            && let Some(screen) = &self.screen
        {
            let (point, side) = self.grid_point(event.position);
            let kind = match event.click_count {
                2 => SelectionType::Semantic,
                3.. => SelectionType::Lines,
                _ => SelectionType::Simple,
            };
            screen.term.lock().selection = Some(Selection::new(kind, point, side));
            self.selecting = true;
            cx.notify();
        }
    }

    /// The press the program was told about comes up, wherever the pointer
    /// is; a selection being dragged out ends.
    fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        if self.selecting && event.button == MouseButton::Left {
            self.selecting = false;
            if let Some(screen) = &self.screen {
                let mut term = screen.term.lock();
                // A click that selected nothing clears what was selected.
                if term.selection.as_ref().is_some_and(|s| s.is_empty()) {
                    term.selection = None;
                }
            }
            cx.notify();
        }
        let Some((pressed, _)) = self.pressed else {
            return;
        };
        if button(event.button) != Some(pressed) {
            return;
        }
        self.pressed = None;
        if Self::reports_mouse(self.mode()) {
            let (col, line) = self.cell_at(event.position);
            self.write(keys::sgr_mouse(
                pressed,
                false,
                mods(event.modifiers),
                col,
                line,
                false,
            ));
        }
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.pointer = Some(event.position);
        self.hover(event.modifiers.platform, cx);
    }

    /// A press follows the pointer wherever it goes, off the terminal too;
    /// positions there land on the nearest cell. A selection grows with it,
    /// and a press the program was told about reports its moves.
    fn drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.selecting {
            if event.pressed_button != Some(MouseButton::Left) {
                return;
            }
            let (point, side) = self.grid_point(event.position);
            if let Some(screen) = &self.screen
                && let Some(selection) = &mut screen.term.lock().selection
            {
                selection.update(point, side);
            }
            cx.notify();
            return;
        }
        let Some((pressed, at)) = self.pressed else {
            return;
        };
        if event.pressed_button.and_then(button) != Some(pressed)
            || !Self::reports_mouse(self.mode())
        {
            return;
        }
        let cell = self.cell_at(event.position);
        if cell != at {
            self.pressed = Some((pressed, cell));
            self.write(keys::sgr_mouse(
                pressed,
                true,
                mods(event.modifiers),
                cell.0,
                cell.1,
                true,
            ));
        }
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.scroll_lines += match event.delta {
            ScrollDelta::Lines(p) => p.y,
            ScrollDelta::Pixels(p) => p.y / px(self.cell.cell_height as f32),
        };
        let lines = self.scroll_lines.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_lines -= lines as f32;
        let mode = self.mode();
        let notches = lines.unsigned_abs() as usize;
        if Self::reports_mouse(mode) {
            let (col, line) = self.cell_at(event.position);
            let button = if lines > 0 {
                keys::MouseButton::WheelUp
            } else {
                keys::MouseButton::WheelDown
            };
            let report = keys::sgr_mouse(button, false, mods(event.modifiers), col, line, true);
            self.write(report.repeat(notches));
        } else if mode.contains(TermMode::ALT_SCREEN) {
            let arrow: &[u8] = if lines > 0 { b"\x1b[A" } else { b"\x1b[B" };
            self.write(arrow.repeat(notches));
        } else if let Some(screen) = &self.screen {
            screen.term.lock().scroll_display(Scroll::Delta(lines));
            cx.notify();
        }
    }

    /// The terminal font from Settings, and its cell size.
    fn measure(window: &mut Window, cx: &App) -> Metrics {
        let terminal = &Prefs::get(cx).terminal;
        let font = font(terminal.font_family.clone());
        let size = terminal.font_size as f32;
        let text = window.text_system();
        let font_id = text.resolve_font(&font);
        let cell_width = text
            .advance(font_id, px(size), 'm')
            .map(|s| s.width)
            .unwrap_or(px(8.0));
        Metrics {
            font,
            size: px(size),
            cell_width,
            line_height: px((size * 1.3).round()),
        }
    }
}

/// The buttons a mouse report can name.
fn button(button: MouseButton) -> Option<keys::MouseButton> {
    match button {
        MouseButton::Left => Some(keys::MouseButton::Left),
        MouseButton::Middle => Some(keys::MouseButton::Middle),
        MouseButton::Right => Some(keys::MouseButton::Right),
        MouseButton::Navigate(_) => None,
    }
}

fn mods(m: Modifiers) -> keys::Mods {
    keys::Mods {
        shift: m.shift,
        alt: m.alt,
        ctrl: m.control,
    }
}

fn save_image(image: &Image) -> anyhow::Result<PathBuf> {
    let ext = match image.format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Svg => "svg",
        ImageFormat::Bmp => "bmp",
        _ => "tiff",
    };
    let path = std::env::temp_dir().join(format!(
        "lemur-paste-{}-{}.{ext}",
        std::process::id(),
        image.id
    ));
    std::fs::write(&path, &image.bytes)?;
    Ok(path)
}

fn hsla(c: Rgb) -> Hsla {
    Rgba {
        r: c.r as f32 / 255.0,
        g: c.g as f32 / 255.0,
        b: c.b as f32 / 255.0,
        a: 1.0,
    }
    .into()
}

/// What a run of text looks like; runs merge while it stays the same.
#[derive(Clone, Copy, PartialEq)]
struct Style {
    fg: Rgb,
    /// Only the flags that change how text is drawn.
    flags: Flags,
}

impl Style {
    const DRAWN: Flags = Flags::BOLD
        .union(Flags::ITALIC)
        .union(Flags::ALL_UNDERLINES)
        .union(Flags::STRIKEOUT);

    fn text_run(self, len: usize, font: &Font) -> TextRun {
        let mut font = font.clone();
        if self.flags.contains(Flags::BOLD) {
            font.weight = FontWeight::BOLD;
        }
        if self.flags.contains(Flags::ITALIC) {
            font.style = FontStyle::Italic;
        }
        let color = hsla(self.fg);
        TextRun {
            len,
            font,
            color,
            background_color: None,
            underline: self
                .flags
                .intersects(Flags::ALL_UNDERLINES)
                .then(|| UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(color),
                    wavy: self.flags.contains(Flags::UNDERCURL),
                }),
            strikethrough: self
                .flags
                .contains(Flags::STRIKEOUT)
                .then(|| StrikethroughStyle {
                    thickness: px(1.0),
                    color: Some(color),
                }),
        }
    }
}

/// One screen row: its text, styled runs (style, byte length), background
/// spans (first column, end column, color), and block elements (column,
/// character, color), which are painted rather than left in the text.
struct Row {
    text: String,
    runs: Vec<(Style, usize)>,
    backgrounds: Vec<(usize, usize, Rgb)>,
    blocks: Vec<(usize, char, Rgb)>,
}

/// What a block element (U+2580–U+259F) covers of its cell, as (left, top,
/// right, bottom) fractions, and how opaque it is. The font's glyphs don't
/// fill the cell, so neighbors would show seams between them.
fn block(c: char) -> Option<(Vec<[f32; 4]>, f32)> {
    // Asked of every cell each frame: most are no block, and cost nothing.
    if !('\u{2580}'..='\u{259F}').contains(&c) {
        return None;
    }
    // Upper left, upper right, lower left, lower right.
    let quadrants = |on: [bool; 4]| -> Vec<[f32; 4]> {
        [
            [0.0, 0.0, 0.5, 0.5],
            [0.5, 0.0, 1.0, 0.5],
            [0.0, 0.5, 0.5, 1.0],
            [0.5, 0.5, 1.0, 1.0],
        ]
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

struct Frame {
    rows: Vec<Row>,
    background: Hsla,
    cursor: Option<(usize, usize, CursorShape, Hsla)>,
    /// The cursor's cell even while it's hidden, where text an input
    /// method composes shows.
    caret: Option<(usize, usize)>,
}

/// Snapshots the grid under the lock so painting doesn't block the reader.
fn snapshot(screen: &Screen, palette: &Palette) -> Frame {
    let term = screen.term.lock();
    let content = term.renderable_content();
    let colors = content.colors;
    let default_bg = palette.indexed(NamedColor::Background as usize, colors);
    let selection = content.selection;
    let selected_bg = palette.selection();
    let offset = content.display_offset as i32;
    let cols = term.columns();
    let mut rows: Vec<Row> = (0..term.screen_lines())
        .map(|_| Row {
            text: String::with_capacity(cols),
            runs: vec![],
            backgrounds: vec![],
            blocks: vec![],
        })
        .collect();
    for cell in content.display_iter {
        let Some(row) = usize::try_from(cell.point.line.0 + offset)
            .ok()
            .and_then(|i| rows.get_mut(i))
        else {
            continue;
        };
        let flags = cell.flags;
        let mut fg = palette.resolve(cell.fg, colors);
        let mut bg = palette.resolve(cell.bg, colors);
        if flags.contains(Flags::DIM) {
            fg = palette.dim(fg);
        }
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if selection.is_some_and(|s| s.contains(cell.point)) {
            bg = selected_bg;
        }
        let col = cell.point.column.0;
        if bg != default_bg {
            match row.backgrounds.last_mut() {
                Some((_, end, color)) if *end == col && *color == bg => *end = col + 1,
                _ => row.backgrounds.push((col, col + 1, bg)),
            }
        }
        // A wide character's spacer stays a space so later cells keep their column.
        let c = if flags.intersects(Flags::HIDDEN | Flags::WIDE_CHAR_SPACER) || cell.c == '\0' {
            ' '
        } else {
            cell.c
        };
        let c = if block(c).is_some() {
            row.blocks.push((col, c, fg));
            ' '
        } else {
            c
        };
        let style = Style {
            fg,
            flags: flags.intersection(Style::DRAWN),
        };
        let len = c.len_utf8();
        row.text.push(c);
        match row.runs.last_mut() {
            Some((last, run_len)) if *last == style => *run_len += len,
            _ => row.runs.push((style, len)),
        }
    }
    // Blanks at the end of a row draw nothing (backgrounds are painted on
    // their own), yet would cost a glyph each to shape and paint.
    for row in &mut rows {
        while row.text.ends_with(' ') {
            let Some((style, len)) = row.runs.last_mut() else {
                break;
            };
            if style
                .flags
                .intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT)
            {
                break;
            }
            row.text.pop();
            *len -= 1;
            if *len == 0 {
                row.runs.pop();
            }
        }
    }
    let cursor = (content.mode.contains(TermMode::SHOW_CURSOR)
        && content.cursor.shape != CursorShape::Hidden)
        .then(|| {
            let line = usize::try_from(content.cursor.point.line.0 + offset).ok()?;
            let color = hsla(palette.indexed(NamedColor::Cursor as usize, colors));
            Some((
                content.cursor.point.column.0,
                line,
                content.cursor.shape,
                color,
            ))
        })
        .flatten();
    let caret = usize::try_from(content.cursor.point.line.0 + offset)
        .ok()
        .filter(|&line| line < rows.len())
        .map(|line| (content.cursor.point.column.0, line));
    Frame {
        rows,
        background: hsla(default_bg),
        cursor,
        caret,
    }
}

fn paint(
    frame: Frame,
    bounds: Bounds<Pixels>,
    metrics: &Metrics,
    focused: bool,
    link: Option<(Link, Hsla)>,
    window: &mut Window,
    cx: &mut App,
) {
    let (w, h) = (metrics.cell_width, metrics.line_height);
    window.paint_quad(fill(bounds, frame.background));
    // Block elements' edges go to whole device pixels: neighbors then share
    // them instead of each half covering the pixel between them.
    let scale = window.scale_factor();
    let snap = |v: Pixels| px((f32::from(v) * scale).round() / scale);
    for (i, row) in frame.rows.iter().enumerate() {
        let y = bounds.origin.y + h * i as f32;
        for &(start, end, color) in &row.backgrounds {
            let origin = point(bounds.origin.x + w * start as f32, y);
            window.paint_quad(fill(
                Bounds::new(origin, size(w * (end - start) as f32, h)),
                hsla(color),
            ));
        }
        for &(col, c, color) in &row.blocks {
            let Some((rects, alpha)) = block(c) else {
                continue;
            };
            let x = bounds.origin.x + w * col as f32;
            let corner = |fx: f32, fy: f32| point(snap(x + w * fx), snap(y + h * fy));
            for [left, top, right, bottom] in rects {
                window.paint_quad(fill(
                    Bounds::from_corners(corner(left, top), corner(right, bottom)),
                    hsla(color).opacity(alpha),
                ));
            }
        }
    }
    if let Some((col, line, shape, color)) = frame.cursor {
        let origin = point(
            bounds.origin.x + w * col as f32,
            bounds.origin.y + h * line as f32,
        );
        let cursor = match shape {
            CursorShape::Beam => Bounds::new(origin, size(px(2.0), h)),
            CursorShape::Underline => {
                Bounds::new(point(origin.x, origin.y + h - px(2.0)), size(w, px(2.0)))
            }
            _ => Bounds::new(origin, size(w, h)),
        };
        if focused || !matches!(shape, CursorShape::Block | CursorShape::HollowBlock) {
            window.paint_quad(fill(cursor, color.opacity(0.6)));
        } else {
            window.paint_quad(outline(cursor, color, BorderStyle::Solid));
        }
    }
    for (i, row) in frame.rows.into_iter().enumerate() {
        if row.text.trim_end().is_empty() {
            continue;
        }
        let y = bounds.origin.y + h * i as f32;
        let runs: Vec<TextRun> = row
            .runs
            .iter()
            .map(|&(style, len)| style.text_run(len, &metrics.font))
            .collect();
        let line = window
            .text_system()
            .shape_line(row.text.into(), metrics.size, &runs, Some(w));
        let _ = line.paint(
            point(bounds.origin.x, y),
            h,
            TextAlign::Left,
            None,
            window,
            cx,
        );
    }
    // The link under the pointer while ⌘ is held.
    if let Some((link, color)) = link {
        for (line, start, end) in link.spans {
            let origin = point(
                bounds.origin.x + w * start as f32,
                bounds.origin.y + h * (line + 1) as f32 - px(2.0),
            );
            window.paint_quad(fill(
                Bounds::new(origin, size(w * (end - start) as f32, px(1.0))),
                color,
            ));
        }
    }
}

/// Text an input method is composing, underlined over the cursor's cell
/// and those after it, as macOS shows it in text fields.
fn paint_marked(
    text: &str,
    at: Bounds<Pixels>,
    metrics: &Metrics,
    fg: Rgb,
    bg: Hsla,
    window: &mut Window,
    cx: &mut App,
) {
    let style = Style {
        fg,
        flags: Flags::UNDERLINE,
    };
    let runs = [style.text_run(text.len(), &metrics.font)];
    let line = window
        .text_system()
        .shape_line(text.to_string().into(), metrics.size, &runs, None);
    let width = line.width().max(at.size.width);
    window.paint_quad(fill(
        Bounds::new(at.origin, size(width, at.size.height)),
        bg,
    ));
    let _ = line.paint(at.origin, at.size.height, TextAlign::Left, None, window, cx);
}

/// Text from macOS's input methods: dead keys' accents, the emoji picker,
/// and scripts typed through an IME. The terminal has no text of its own to
/// offer them, only what's being composed.
struct TerminalInput {
    view: Entity<TerminalView>,
    /// The cursor's cell, where composing shows and an IME's candidates open.
    caret: Option<Bounds<Pixels>>,
}

impl InputHandler for TerminalInput {
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<UTF16Selection> {
        let end = self.marked(cx).map_or(0, |r| r.end);
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }

    fn marked_text_range(&mut self, _: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        self.marked(cx)
    }

    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<String> {
        None
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut App,
    ) {
        self.view.update(cx, |view, cx| view.type_text(text, cx));
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut App,
    ) {
        self.view.update(cx, |view, cx| {
            view.marked = (!text.is_empty()).then(|| text.to_string());
            cx.notify();
        });
    }

    /// macOS asks to keep what was composed as it stands.
    fn unmark_text(&mut self, _: &mut Window, cx: &mut App) {
        self.view.update(cx, |view, cx| {
            if let Some(text) = view.marked.take() {
                view.type_text(&text, cx);
            }
        });
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let mut at = self.caret?;
        at.origin.x += at.size.width * range.start as f32;
        Some(at)
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<usize> {
        None
    }
}

impl TerminalInput {
    fn marked(&self, cx: &App) -> Option<Range<usize>> {
        let marked = self.view.read(cx).marked.as_ref()?;
        Some(0..marked.encode_utf16().count())
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.renders += 1;
        }
        let metrics = self
            .metrics
            .get_or_insert_with(|| Self::measure(window, cx))
            .clone();
        let (w, h) = (metrics.cell_width, metrics.line_height);
        let focused = self.focus.is_focused(window);
        let view = cx.entity().downgrade();
        let this = cx.entity();
        let focus = self.focus.clone();
        let palette = Palette::themed(cx.theme().is_dark(), &Prefs::get(cx).terminal);
        // The core answers programs with the colors and cell size shown,
        // and hands the terminal to whoever has the keyboard.
        if let Attachment::Attached(core, channel) = &self.attachment {
            let facts = Facts::new(&palette, self.cell);
            if self.told_display != Some(facts) {
                core.facts(*channel, &facts.json());
                self.told_display = Some(facts);
            }
            if self.told_focus != Some(focused) {
                core.focus(*channel, focused);
                self.told_focus = Some(focused);
            }
        }
        let frame = self
            .screen
            .as_ref()
            .map(|screen| snapshot(screen, &palette));
        let link = self
            .hovered
            .clone()
            .map(|link| (link, hsla(palette.foreground)));
        let marked = self.marked.clone();
        let (fg, bg) = (palette.foreground, hsla(palette.background));
        let grid = canvas(
            move |bounds, _, cx| {
                let size = Size {
                    cols: (bounds.size.width / w).floor().max(1.0) as u16,
                    lines: (bounds.size.height / h).floor().max(1.0) as u16,
                    cell_width: f32::from(w).round() as u16,
                    cell_height: f32::from(h) as u16,
                };
                view.update(cx, |view, _| {
                    view.bounds = bounds;
                    view.cell_size = gpui_kit::size(w, h);
                    view.cell = size;
                    if let Some(screen) = &mut view.screen
                        && screen.resize(size)
                        && let Attachment::Attached(core, channel) = &view.attachment
                    {
                        core.resize(*channel, size.cols, size.lines);
                    }
                })
                .ok();
            },
            move |bounds, (), window, cx| {
                let caret = frame.as_ref().and_then(|f| f.caret).map(|(col, line)| {
                    let origin = point(
                        bounds.origin.x + w * col as f32,
                        bounds.origin.y + h * line as f32,
                    );
                    Bounds::new(origin, size(w, h))
                });
                if let Some(frame) = frame {
                    paint(frame, bounds, &metrics, focused, link, window, cx);
                }
                if let (Some(text), Some(at)) = (marked, caret) {
                    paint_marked(&text, at, &metrics, fg, bg, window, cx);
                }
                let input = TerminalInput {
                    view: this.clone(),
                    caret,
                };
                window.handle_input(&focus, input, cx);
                // A press is followed off the terminal too, where the
                // element's own listeners don't hear it.
                let dragged = this.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        dragged.update(cx, |view, cx| view.drag(event, cx));
                    }
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        this.update(cx, |view, cx| view.mouse_up(event, cx));
                    }
                });
            },
        )
        .size_full();

        div()
            .id("terminal")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .relative()
            .size_full()
            // Room between the text and the frame around it.
            .px(px(12.0))
            .py(px(8.0))
            .bg(hsla(palette.background))
            .on_key_down(cx.listener(Self::key_down))
            .on_any_mouse_down(cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_modifiers_changed(cx.listener(|this, e: &ModifiersChangedEvent, _, cx| {
                this.hover(e.modifiers.platform, cx)
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered {
                    this.pointer = None;
                    this.hover(false, cx);
                }
            }))
            .when(self.hovered.is_some(), |this| this.cursor_pointer())
            .on_scroll_wheel(cx.listener(Self::scroll))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.upload(paths.paths().to_vec(), None, cx)
            }))
            .child(grid)
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's `test` macro would shadow the built-in one.
    use super::{OUTPUT_EVERY, TerminalInput, TerminalView, block, snapshot};
    use crate::gui::settings::Prefs;
    use crate::gui::terminal::{DARK, Screen, Size};
    use crate::hc::Stream;
    use crate::test_support::{HcServer, wait_until};
    use alacritty_terminal::grid::Dimensions;
    use gpui_kit::{
        Capslock, InputHandler, KeyDownEvent, Keystroke, Modifiers, ModifiersChangedEvent,
        MouseButton, point, px, size,
    };
    use std::time::Duration;

    /// A terminal view of a new terminal running `argv`, in a 600×400
    /// window, once it's attached; it has the keyboard.
    fn attach<'a>(
        server: &HcServer,
        argv: &[&str],
        cx: &'a mut gpui::TestAppContext,
    ) -> (
        gpui::Entity<TerminalView>,
        String,
        &'a mut gpui::VisualTestContext,
    ) {
        let t = server.run("/", &[], argv);
        let host = server.config().hosts.remove(0);
        // The core's connection is real threads, so these tests can't be
        // deterministic.
        cx.executor().allow_parking();
        cx.update(gpui_kit::init);
        let session = t.clone();
        let (view, cx) = cx.add_window_view(|_, cx| TerminalView::new(host, session, cx));
        cx.simulate_resize(size(px(600.0), px(400.0)));
        let focus = view.read_with(cx, |v, _| v.focus.clone());
        cx.update(|window, cx| window.focus(&focus, cx));
        let attached = wait_until(Duration::from_secs(10), || {
            cx.run_until_parked();
            view.read_with(cx, |v, _| v.is_attached())
        });
        assert!(attached, "never attached");
        (view, t, cx)
    }

    /// What `view` shows, row by row, without trailing blanks.
    fn shown(view: &gpui::Entity<TerminalView>, cx: &mut gpui::VisualTestContext) -> String {
        cx.run_until_parked();
        view.read_with(cx, |v, _| {
            let frame = snapshot(v.screen.as_ref().unwrap(), &DARK);
            let rows: Vec<String> = frame.rows.into_iter().map(|r| r.text).collect();
            rows.join("\n").trim_end().to_string()
        })
    }

    /// Output is drawn at most every `OUTPUT_EVERY`, however much comes
    /// meanwhile; right after typing, at once, so the echo doesn't lag.
    #[gpui::test]
    fn output_is_drawn_about_30_times_a_second(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("redraws");
        let (view, _, cx) = attach(&server, &["sleep", "600"], cx);
        cx.executor().advance_clock(OUTPUT_EVERY);
        cx.run_until_parked();
        let renders = |cx: &mut gpui::VisualTestContext| view.read_with(cx, |v, _| v.renders);
        let before = renders(cx);
        view.update(cx, |v, cx| {
            for i in 0..50 {
                v.ingest(vec![Stream::Data(format!("line {i}\r\n").into_bytes())], cx);
            }
        });
        cx.run_until_parked();
        assert_eq!(renders(cx), before, "not yet");
        cx.executor().advance_clock(OUTPUT_EVERY);
        cx.run_until_parked();
        assert_eq!(renders(cx), before + 1, "once for all fifty");
        assert!(shown(&view, cx).contains("line 49"));

        view.update(cx, |v, cx| {
            v.typed_at.set(Some(std::time::Instant::now()));
            v.ingest(vec![Stream::Data(b"x".to_vec())], cx);
        });
        cx.run_until_parked();
        assert_eq!(renders(cx), before + 2, "typing shows at once");
    }

    /// Holding ⌘ over a URL shows it, and ⌘-clicking opens it; a plain
    /// click doesn't.
    #[gpui::test]
    fn cmd_click_opens_a_link(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("links");
        let script = "printf 'see https://example.com/x ok'; sleep 600";
        let (view, _, cx) = attach(&server, &["sh", "-c", script], cx);
        // The middle of the URL's third character, on the first row.
        let at = view.read_with(cx, |v, _| {
            let (w, h) = (v.cell.cell_width as f32, v.cell.cell_height as f32);
            point(
                v.bounds.origin.x + px(w * 6.5),
                v.bounds.origin.y + px(h * 0.5),
            )
        });
        let showed = wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            view.read_with(cx, |v, _| v.link_at(at)).is_some()
        });
        assert!(showed, "the URL never showed");

        cx.simulate_click(at, Modifiers::none());
        assert_eq!(cx.opened_url(), None);
        cx.simulate_mouse_move(at, None, Modifiers::command());
        let hovered = view.read_with(cx, |v, _| v.hovered.as_ref().map(|l| l.url.clone()));
        assert_eq!(hovered.as_deref(), Some("https://example.com/x"));
        cx.simulate_click(at, Modifiers::command());
        assert_eq!(cx.opened_url().as_deref(), Some("https://example.com/x"));
    }

    /// The mouse lands on the cell drawn under it all the way across, though
    /// cells are rarely a whole number of pixels wide.
    #[gpui::test]
    fn the_mouse_lands_on_the_cell_under_it(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("mouse");
        let (view, _, cx) = attach(&server, &["sleep", "600"], cx);
        cx.simulate_resize(size(px(800.0), px(600.0)));
        cx.run_until_parked();
        view.read_with(cx, |v, _| {
            let m = v.metrics.as_ref().expect("measured");
            let (cols, lines) = (v.cell.cols as usize, v.cell.lines as usize);
            // The middle of the last cell, as painted.
            let at = point(
                v.bounds.origin.x + m.cell_width * (cols as f32 - 0.5),
                v.bounds.origin.y + m.line_height * (lines as f32 - 0.5),
            );
            assert_eq!(v.cell_at(at), (cols - 1, lines - 1), "{:?}", m.cell_width);
        });
    }

    /// A selection dragged off the terminal follows the pointer to the
    /// edge, and ⌘C copies it.
    #[gpui::test]
    fn a_drag_off_the_terminal_still_selects(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("drag");
        let script = "printf 'hello world'; sleep 600";
        let (view, _, cx) = attach(&server, &["sh", "-c", script], cx);
        assert!(
            wait_until(Duration::from_secs(5), || shown(&view, cx) == "hello world"),
            "{:?}",
            shown(&view, cx)
        );

        // Press on the first character, drag over two more, then leave the
        // window to the right of the row and let go there.
        let (origin, cell) = view.read_with(cx, |v, _| (v.bounds.origin, v.cell_size));
        let at = |col: f32| point(origin.x + cell.width * col, origin.y + cell.height * 0.5);
        let none = Modifiers::none();
        cx.simulate_mouse_down(at(0.2), MouseButton::Left, none);
        cx.simulate_mouse_move(at(2.5), MouseButton::Left, none);
        let off = point(px(700.0), at(0.0).y);
        cx.simulate_mouse_move(off, MouseButton::Left, none);
        cx.simulate_mouse_up(off, MouseButton::Left, none);
        cx.simulate_keystrokes("cmd-c");
        let copied = cx.read_from_clipboard().and_then(|c| c.text());
        assert_eq!(copied.as_deref(), Some("hello world"));
        // Typing lets go of it.
        cx.simulate_keystrokes("x");
        let selected = view.read_with(cx, |v, _| {
            v.screen.as_ref().unwrap().term.lock().selection.is_some()
        });
        assert!(!selected);
    }

    /// Text reaches the program through macOS's input method, which is what
    /// composes accents: keys typed, and what it composed once it's done.
    #[gpui::test]
    fn text_comes_through_the_input_method(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("input");
        let (view, t, cx) = attach(&server, &["cat"], cx);
        let input = || TerminalInput {
            view: view.clone(),
            caret: None,
        };

        cx.simulate_keystrokes("h i");
        // A dead key's accent shows, unsent, until the next key completes it.
        cx.update(|window, cx| input().replace_and_mark_text_in_range(None, "´", None, window, cx));
        let marked = view.read_with(cx, |v, _| v.marked.clone());
        assert_eq!(marked.as_deref(), Some("´"));
        cx.update(|window, cx| input().replace_text_in_range(None, "é", window, cx));
        // What macOS asks to keep as it stands is sent as it stands.
        cx.update(|window, cx| {
            input().replace_and_mark_text_in_range(None, "~", None, window, cx);
            input().unmark_text(window, cx);
        });
        assert_eq!(view.read_with(cx, |v, _| v.marked.clone()), None);
        let read = || server.hc(&["read", &t]);
        assert!(
            wait_until(Duration::from_secs(5), || read() == "hié~"),
            "it shows {:?}",
            read()
        );
    }

    /// A held key repeats from here, not the input method, which only sees
    /// the first press; Caps Lock still capitalizes it.
    #[gpui::test]
    fn a_held_key_repeats_with_caps_lock(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("held");
        let (_view, t, cx) = attach(&server, &["cat"], cx);

        cx.simulate_event(ModifiersChangedEvent {
            modifiers: Modifiers::none(),
            capslock: Capslock { on: true },
        });
        for _ in 0..2 {
            cx.simulate_event(KeyDownEvent {
                keystroke: Keystroke::parse("a").unwrap().with_simulated_ime(),
                is_held: true,
                prefer_character_input: false,
            });
        }
        let read = || server.hc(&["read", &t]);
        assert!(
            wait_until(Duration::from_secs(5), || read() == "AA"),
            "it shows {:?}",
            read()
        );
    }

    /// A program that asks for the mouse gets every button, as SGR reports.
    #[gpui::test]
    fn clicks_reach_a_program_that_asks(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("buttons");
        // Shows, in hex, the 4 reports it reads.
        let script = r"printf '\033[?1000h\033[?1006h'; stty raw -echo; head -c 36 | xxd -p -c 64";
        let (view, t, cx) = attach(&server, &["sh", "-c", script], cx);
        let reporting = wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            view.read_with(cx, |v, _| TerminalView::reports_mouse(v.mode()))
        });
        assert!(reporting, "the program never asked for mouse reports");

        let at = view.read_with(cx, |v, _| {
            v.bounds.origin + point(v.cell_size.width * 2.5, v.cell_size.height * 2.5)
        });
        let none = Modifiers::none();
        cx.simulate_mouse_down(at, MouseButton::Middle, none);
        cx.simulate_mouse_up(at, MouseButton::Middle, none);
        cx.simulate_mouse_down(at, MouseButton::Right, none);
        cx.simulate_mouse_up(at, MouseButton::Right, none);
        let read = || server.hc(&["read", &t]);
        // ESC [ < button ; column ; row M (pressed) or m (released).
        let reports = "1b5b3c313b333b334d1b5b3c313b333b336d1b5b3c323b333b334d1b5b3c323b333b336d";
        assert!(
            wait_until(Duration::from_secs(5), || read().contains(reports)),
            "it read {:?}",
            read()
        );
    }

    /// Rows reach the painter without the blanks after their text.
    #[test]
    fn trailing_blanks_are_not_drawn() {
        let mut screen = Screen::new(
            Size {
                cols: 40,
                lines: 3,
                cell_width: 8,
                cell_height: 16,
            },
            100,
        );
        screen.advance(b"hi");
        let frame = snapshot(&screen, &DARK);
        assert_eq!(frame.rows[0].text, "hi");
        assert_eq!(
            frame.rows[0].runs.iter().map(|(_, len)| len).sum::<usize>(),
            2
        );
        assert_eq!(frame.rows[1].text, "");
    }

    /// Block elements reach the painter as rectangles of their cell, not as
    /// glyphs, so Claude's mascot has no seams.
    #[test]
    fn block_elements_are_painted_not_typed() {
        let mut screen = Screen::new(
            Size {
                cols: 40,
                lines: 3,
                cell_width: 8,
                cell_height: 16,
            },
            100,
        );
        screen.advance("a▐▛x".as_bytes());
        let frame = snapshot(&screen, &DARK);
        assert_eq!(frame.rows[0].text, "a  x");
        let blocks: Vec<_> = frame.rows[0].blocks.iter().map(|b| (b.0, b.1)).collect();
        assert_eq!(blocks, [(1, '▐'), (2, '▛')]);

        assert_eq!(block('▁'), Some((vec![[0.0, 0.875, 1.0, 1.0]], 1.0)));
        assert_eq!(block('█'), Some((vec![[0.0, 0.0, 1.0, 1.0]], 1.0)));
        assert_eq!(block('▉'), Some((vec![[0.0, 0.0, 0.875, 1.0]], 1.0)));
        assert_eq!(block('▏'), Some((vec![[0.0, 0.0, 0.125, 1.0]], 1.0)));
        assert_eq!(block('a'), None);
    }

    /// Resizing the window resizes the grid, and the program sees the new
    /// size.
    #[gpui::test]
    fn the_terminal_follows_the_window_size(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("resize");
        let (view, t, cx) = attach(&server, &["sleep", "600"], cx);
        let small = view
            .read_with(cx, |v, _| v.screen_size())
            .expect("attached");
        let core = server.core();
        let seen = || {
            core.terminals()
                .into_iter()
                .find(|x| x.address == t)
                .map(|x| (x.cols, x.rows))
        };
        // Laid out before it attached: the program gets that size at once.
        assert!(
            wait_until(Duration::from_secs(5), || seen()
                == Some((small.cols, small.lines))),
            "the core sees {:?} for {small:?}",
            seen()
        );
        cx.simulate_resize(size(px(1000.0), px(700.0)));
        cx.run_until_parked();
        let large = view
            .read_with(cx, |v, _| v.screen_size())
            .expect("attached");
        assert!(
            large.cols > small.cols && large.lines > small.lines,
            "{small:?} -> {large:?}"
        );
        assert!(
            wait_until(Duration::from_secs(5), || seen()
                == Some((large.cols, large.lines))),
            "the core sees {:?}",
            seen()
        );
    }

    /// A terminal keeps as many lines to scroll back through as Settings
    /// say, and fewer at once when they say fewer.
    #[gpui::test]
    fn the_terminal_keeps_the_scrollback_set(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("scrollback");
        cx.update(|cx| Prefs::update(cx, |p| p.terminal.scrollback = 300));
        let (view, _, cx) = attach(&server, &["sh", "-c", "seq 1 1000; exec sleep 600"], cx);
        let history = |cx: &mut gpui::VisualTestContext| {
            view.read_with(cx, |v, _| {
                let screen = v.screen.as_ref().expect("a screen");
                Dimensions::history_size(screen.term.lock().grid())
            })
        };
        let filled = wait_until(Duration::from_secs(10), || {
            cx.run_until_parked();
            history(cx) == 300
        });
        assert!(filled, "kept {}", history(cx));

        // Changed in Settings, it keeps fewer at once.
        cx.update(|_, cx| Prefs::update(cx, |p| p.terminal.scrollback = 50));
        cx.run_until_parked();
        assert_eq!(history(cx), 50);
    }

    /// A font size from Settings reaches open terminals: a bigger font fits
    /// fewer columns and lines in the same window.
    #[gpui::test]
    fn the_terminal_follows_the_font_size(cx: &mut gpui::TestAppContext) {
        let server = HcServer::new("font");
        let (view, _, cx) = attach(&server, &["sleep", "600"], cx);
        cx.simulate_resize(size(px(800.0), px(600.0)));
        cx.run_until_parked();
        let small = view
            .read_with(cx, |v, _| v.screen_size())
            .expect("attached");

        cx.update(|_, cx| Prefs::update(cx, |p| p.terminal.font_size = 26.0));
        cx.run_until_parked();
        let large = view
            .read_with(cx, |v, _| v.screen_size())
            .expect("attached");
        assert!(
            large.cols < small.cols && large.lines < small.lines,
            "{small:?} -> {large:?}"
        );
    }
}
