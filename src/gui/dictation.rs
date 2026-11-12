//! Dictation (⌘D): a bar under the terminal shows what you say as you say
//! it, transcribed on this Mac by lemur-dictate. Correct it there, then ⌘⏎
//! pastes it into the thread on show, whichever that is by then, without
//! sending it; Esc throws it away. ⌘D again stops listening, and once more
//! adds to the text.

use super::{Lemur, icons};
use crate::config::{Config, Language};
use crate::dictate::{Heard, Recording};
use futures::StreamExt;
use rune::{El, Key as K, TextEdit, col, el, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit, TextArea};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Counts recordings, so what one thrown away says late is told from what the next says.
static RECORDINGS: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
thread_local! {
    /// A stand-in for lemur-dictate, for the test on this thread.
    pub(super) static STAND_IN: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// lemur-dictate; tests have a stand-in.
fn helper() -> PathBuf {
    #[cfg(test)]
    if let Some(stand_in) = STAND_IN.with(|h| h.borrow().clone()) {
        return stand_in;
    }
    crate::dictate::helper()
}

pub fn language_label(language: Language) -> &'static str {
    match language {
        Language::English => "English",
        Language::Norwegian => "Norwegian Bokmål",
    }
}

pub struct Dictation {
    pub input: TextEdit,
    listening: Option<Listening>,
    /// Why the last recording stopped, when it failed.
    error: Option<String>,
    /// ⌘⏎ came while listening: paste once the rest is in.
    paste_when_done: bool,
}

struct Listening {
    recording: Recording,
    /// Which recording it is, of `RECORDINGS`.
    serial: u64,
    /// The text before this recording's words.
    before: String,
    /// What lemur-dictate says it's doing, until it listens.
    status: Option<String>,
    /// When it started listening.
    since: Option<Instant>,
}

impl Dictation {
    fn new() -> Dictation {
        Dictation { input: TextEdit::new(""), listening: None, error: None, paste_when_done: false }
    }

    #[cfg(test)]
    pub fn is_listening(&self) -> bool {
        self.listening.is_some()
    }
}

impl Lemur {
    /// ⌘D: opens the bar listening, or has it start or stop listening. Its text gets the
    /// keyboard.
    pub(super) fn dictate(&mut self) {
        if self.overlaid() {
            return;
        }
        let bar = self.panels.dictation.get_or_insert_with(Dictation::new);
        bar.input.focused = true;
        self.record();
    }

    /// Starts listening, or stops.
    fn record(&mut self) {
        let Some(bar) = &mut self.panels.dictation else { return };
        match &mut bar.listening {
            Some(listening) => listening.recording.stop(),
            None => self.listen(),
        }
    }

    fn listen(&mut self) {
        let language = self.prefs.dictation_language;
        let Some(bar) = &mut self.panels.dictation else { return };
        let (recording, mut heard) = match Recording::start(&helper(), language.as_str()) {
            Ok(started) => started,
            Err(e) => {
                bar.error = Some(format!("{e:#}"));
                return;
            }
        };
        bar.error = None;
        // The new words go after what's there.
        let mut before = bar.input.text.clone();
        if !before.is_empty() && !before.ends_with(char::is_whitespace) {
            before.push(' ');
        }
        let serial = RECORDINGS.fetch_add(1, Ordering::Relaxed);
        bar.listening = Some(Listening { recording, serial, before, status: None, since: None });
        let back = self.bg.back();
        std::thread::spawn(move || {
            while let Some(said) = futures::executor::block_on(heard.next()) {
                if !back.send(move |s| s.heard(serial, said)) {
                    break;
                }
            }
        });
        self.bg.after(Duration::from_secs(1), move |s| s.tick_listening(serial));
    }

    /// What recording `serial` heard.
    fn heard(&mut self, serial: u64, heard: Heard) {
        let Some(bar) = &mut self.panels.dictation else { return };
        let Some(listening) = bar.listening.as_mut().filter(|l| l.serial == serial) else { return };
        match heard {
            Heard::Words { finalized, volatile } => {
                bar.input.replace_text(format!("{}{finalized}{volatile}", listening.before));
            }
            Heard::Status { status } => listening.status = Some(status),
            Heard::Listening { .. } => {
                listening.status = None;
                listening.since = Some(Instant::now());
            }
            Heard::Error { error } => {
                bar.error = Some(error);
                bar.paste_when_done = false;
                bar.listening = None;
            }
            Heard::Done { .. } => {
                bar.listening = None;
                if std::mem::take(&mut bar.paste_when_done) {
                    self.paste_dictation();
                }
            }
        }
    }

    /// Once a second while recording `serial` listens, so the time listened shows.
    fn tick_listening(&mut self, serial: u64) {
        let listening = self.panels.dictation.as_ref().and_then(|b| b.listening.as_ref());
        if listening.is_some_and(|l| l.serial == serial) {
            self.bg.after(Duration::from_secs(1), move |s| s.tick_listening(serial));
        }
    }

    /// ⌘⏎ or Paste: pastes what was dictated into the thread on show, and the bar closes;
    /// while listening, once the rest of what's said is in.
    pub(super) fn paste_dictation(&mut self) {
        let Some(bar) = &mut self.panels.dictation else { return };
        if let Some(listening) = &mut bar.listening {
            bar.paste_when_done = true;
            listening.recording.stop();
            return;
        }
        let text = bar.input.text.trim().to_string();
        if text.is_empty() {
            return;
        }
        let Some(view) = self.active.as_ref().and_then(|k| self.terminals.get_mut(k)) else {
            self.note("Open a thread to paste into".into());
            return;
        };
        // The text stays until it has somewhere to go.
        if view.exited() {
            self.note("This thread is disconnected; it reconnects on its own, or click it".into());
            return;
        }
        view.paste_text(&text);
        self.close_dictation();
    }

    /// Esc or ×: the text goes, and lemur-dictate stops if it still listens.
    fn close_dictation(&mut self) {
        self.panels.dictation = None;
    }

    /// The bar's language button: listens for `language` from the next recording on, as
    /// Settings does, and the config file keeps it.
    fn set_dictation_language(&mut self, language: Language) {
        self.prefs.dictation_language = language;
        let p = &self.prefs;
        let saved = p.config_file().map(|file| {
            Config::save_settings(file, p.theme, p.dictation_language, p.thread_order, &p.terminal, p.claude_usage, p.codex_usage)
        });
        if let Some(Err(e)) = saved {
            self.fail(e);
        }
    }
}

/// What it's doing, or why it stopped, at the left of the bar.
fn state(bar: &Dictation, k: &Kit) -> El {
    let c = &k.c;
    let line = |words: &str| text(words.to_owned()).font(k.ty.small).color(c.text_muted).truncate();
    let Some(listening) = &bar.listening else {
        return match &bar.error {
            Some(error) => text(error.clone()).key("dictation-error").font(k.ty.small).color(c.danger.fg).truncate(),
            None => line("⌘⏎ pastes into the thread on show"),
        };
    };
    if listening.recording.stopped() {
        return line("Finishing…");
    }
    match (&listening.status, listening.since) {
        (Some(status), _) => line(status),
        (None, None) => line("Starting…"),
        (None, Some(since)) => {
            let secs = since.elapsed().as_secs();
            row()
                .gap(8.0)
                .center_y()
                .child(el().size(8.0, 8.0).radius(4.0).fill(c.danger.icon))
                .child(line(&format!("Listening {}:{:02}", secs / 60, secs % 60)))
        }
    }
}

/// Under the terminal while dictating: what it's doing and its buttons over the text.
pub fn bar(s: &Lemur, k: &Kit) -> Option<El> {
    let bar = s.panels.dictation.as_ref()?;
    let c = &k.c;
    let language = s.prefs.dictation_language;
    let other = match language {
        Language::English => Language::Norwegian,
        Language::Norwegian => Language::English,
    };
    let listening = bar.listening.is_some();
    let buttons = row()
        .gap(4.0)
        .center_y()
        .child(
            Button::new(language_label(language))
                .variant(ButtonVariant::Ghost)
                .control_size(ControlSize::Small)
                .icon(&icons::LANGUAGES)
                .help(format!("Listen for {} from the next recording", language_label(other)))
                .on_press(move |s: &mut Lemur| s.set_dictation_language(other))
                .build(k)
                .key("dictation-language"),
        )
        .child(
            icons::button(if listening { &icons::CIRCLE_STOP } else { &icons::MIC }, if listening { "Stop listening (⌘D)" } else { "Listen for more (⌘D)" })
                .on_press(|s: &mut Lemur| s.record())
                .build(k)
                .key("dictation-record"),
        )
        .child(
            Button::new("Paste")
                .variant(ButtonVariant::Contrast)
                .control_size(ControlSize::Small)
                .help("Paste into the thread on show (⌘⏎)")
                .on_press(|s: &mut Lemur| s.paste_dictation())
                .build(k)
                .key("dictation-paste"),
        )
        .child(
            icons::button(&icons::X, "Throw it away (Esc)")
                .on_press(|s: &mut Lemur| s.close_dictation())
                .build(k)
                .key("dictation-dismiss"),
        );
    let field = TextArea::new(&bar.input)
        .placeholder("What you say shows here.")
        .label("Dictation")
        .width(None)
        // Three lines whatever it holds, so the terminal above keeps its size.
        .auto_grow(3, 3)
        .on_edit(|s: &mut Lemur| &mut s.panels.dictation.as_mut().expect("dictating").input)
        // A plain Enter is a new line; ⌘⏎ pastes.
        .on_submit(|s: &mut Lemur, _| {
            if let Some(bar) = &mut s.panels.dictation {
                bar.input.insert("\n");
            }
        })
        .on_key(|s: &mut Lemur, ev, _| {
            let listening = s.panels.dictation.as_ref().is_some_and(|b| b.listening.is_some());
            match ev.key {
                K::Escape => {
                    s.close_dictation();
                    true
                }
                // The field would take it for Return.
                K::Enter if ev.mods.cmd => {
                    s.paste_dictation();
                    true
                }
                // Words typed while it listens would be overwritten, so they aren't taken; ⌘D
                // goes on to the window.
                K::Command('v' | 'x' | 'z') if listening => true,
                _ if listening => !ev.mods.cmd,
                _ => false,
            }
        })
        .build(k)
        .key("dictation-text");
    Some(
        col()
            .key("dictation")
            .w_full()
            .px(12.0)
            .py(8.0)
            .gap(8.0)
            .border_sides([1.0, 0.0, 0.0, 0.0], c.border_weak)
            .fill(c.bg_base)
            .child(row().w_full().gap(8.0).center_y().child(row().grow(1.0).min_w(0.0).child(state(bar, k))).child(buttons))
            .child(field),
    )
}

#[cfg(test)]
#[path = "dictation_tests.rs"]
mod tests;
