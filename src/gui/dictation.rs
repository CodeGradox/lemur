//! Dictation (⌘D): a bar under the terminal shows what you say as you say
//! it, transcribed on this Mac by lemur-dictate. Correct it there, then ⌘⏎
//! pastes it into the thread on show, whichever that is by then, without
//! sending it; Esc throws it away. ⌘D again stops listening, and once more
//! adds to the text.

use super::Lemur;
use super::settings::{Prefs, language_label};
use crate::config::Language;
use crate::dictate::{Heard, Recording};
use futures::StreamExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Enter, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme, IconName, Sizable, h_flex, v_flex};
use gpui_kit::*;
use std::path::PathBuf;
use std::time::{Duration, Instant};

actions!(dictation, [Dismiss]);

/// Key context of the bar.
pub const CONTEXT: &str = "Dictation";

/// Esc throws the bar's text away. Its field goes first and passes on the
/// Esc it has no use for.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", Dismiss, Some(CONTEXT))]);
}

#[derive(Debug, PartialEq)]
pub enum DictationEvent {
    /// Paste this into the thread on show.
    Paste(String),
    Dismiss,
}

impl EventEmitter<DictationEvent> for DictationBar {}

impl Lemur {
    /// ⌘D: opens the bar listening, or has it start or stop listening.
    pub(super) fn dictate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let bar = match &self.dictation {
            Some((bar, _)) => bar.clone(),
            None => {
                let helper = self.helper.clone();
                let bar = cx.new(|cx| DictationBar::new(helper, window, cx));
                let events =
                    cx.subscribe_in(&bar, window, |this, _, event, window, cx| match event {
                        DictationEvent::Paste(text) => this.paste_dictated(text, window, cx),
                        DictationEvent::Dismiss => this.close_dictation(window, cx),
                    });
                self.dictation = Some((bar.clone(), events));
                bar
            }
        };
        bar.update(cx, |bar, cx| {
            bar.record(window, cx);
            bar.focus(window, cx);
        });
        cx.notify();
    }

    /// ⌘⏎ outside the bar.
    pub(super) fn paste_dictation(&mut self, cx: &mut Context<Self>) {
        if let Some((bar, _)) = &self.dictation {
            bar.update(cx, |bar, cx| bar.paste(cx));
        }
    }

    fn paste_dictated(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.active.as_ref().and_then(|k| self.terminals.get(k)) else {
            self.note("Open a thread to paste into".into(), cx);
            return;
        };
        // The text stays until it has somewhere to go.
        if view.read(cx).exited() {
            self.note(
                "This thread is disconnected; click it to reconnect".into(),
                cx,
            );
            return;
        }
        view.read(cx).paste_text(text);
        self.close_dictation(window, cx);
    }

    fn close_dictation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dictation = None;
        self.focus_terminal(window, cx);
    }
}

pub struct DictationBar {
    /// lemur-dictate; tests have a stand-in.
    helper: PathBuf,
    input: Entity<TextareaState>,
    listening: Option<Listening>,
    /// Why the last recording stopped, when it failed.
    error: Option<String>,
    /// ⌘⏎ came while listening: paste once the rest is in.
    paste_when_done: bool,
    _prefs: Subscription,
}

struct Listening {
    recording: Recording,
    /// The text before this recording's words.
    before: String,
    /// What lemur-dictate says it's doing, until it listens.
    status: Option<String>,
    /// When it started listening.
    since: Option<Instant>,
    _reader: Task<()>,
    _clock: Task<()>,
}

impl DictationBar {
    pub fn new(helper: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                // Three lines whatever it holds, so the terminal above keeps its size.
                .auto_grow(3, 3)
                .placeholder("What you say shows here.")
        });
        DictationBar {
            helper,
            input,
            listening: None,
            error: None,
            paste_when_done: false,
            // The language button follows Settings.
            _prefs: cx.observe_global::<Prefs>(|_, cx| cx.notify()),
        }
    }

    #[cfg(test)]
    pub fn text(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    #[cfg(test)]
    pub fn is_listening(&self) -> bool {
        self.listening.is_some()
    }

    /// Gives the keyboard to the text.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    /// Starts listening, or stops.
    pub fn record(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &mut self.listening {
            Some(listening) => listening.recording.stop(),
            None => self.listen(window, cx),
        }
        cx.notify();
    }

    fn listen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let language = Prefs::get(cx).dictation_language;
        let (recording, mut heard) = match Recording::start(&self.helper, language.as_str()) {
            Ok(started) => started,
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                return;
            }
        };
        self.error = None;
        // The new words go after what's there.
        let mut before = self.input.read(cx).value().to_string();
        if !before.is_empty() && !before.ends_with(char::is_whitespace) {
            before.push(' ');
        }
        self.input
            .update(cx, |input, cx| input.set_readonly(true, cx));
        let reader = cx.spawn_in(window, async move |this, cx| {
            while let Some(heard) = heard.next().await {
                if this
                    .update_in(cx, |this, window, cx| this.heard(heard, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        // Ticks the time listened.
        let clock = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        self.listening = Some(Listening {
            recording,
            before,
            status: None,
            since: None,
            _reader: reader,
            _clock: clock,
        });
    }

    fn heard(&mut self, heard: Heard, window: &mut Window, cx: &mut Context<Self>) {
        let Some(listening) = &mut self.listening else {
            return;
        };
        match heard {
            Heard::Words {
                finalized,
                volatile,
            } => {
                let text = format!("{}{finalized}{volatile}", listening.before);
                self.input.update(cx, |input, cx| {
                    input.set_value(text.clone(), window, cx);
                    input.set_selected_range(text.len()..text.len(), cx);
                });
            }
            Heard::Status { status } => listening.status = Some(status),
            Heard::Listening { .. } => {
                listening.status = None;
                listening.since = Some(Instant::now());
            }
            Heard::Error { error } => {
                self.error = Some(error);
                self.paste_when_done = false;
                self.stopped(cx);
            }
            Heard::Done { .. } => {
                self.stopped(cx);
                if std::mem::take(&mut self.paste_when_done) {
                    self.paste(cx);
                }
            }
        }
        cx.notify();
    }

    fn stopped(&mut self, cx: &mut Context<Self>) {
        self.listening = None;
        self.input
            .update(cx, |input, cx| input.set_readonly(false, cx));
    }

    /// Asks for the text to be pasted; while listening, once the rest of
    /// what's said is in.
    pub fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(listening) = &mut self.listening {
            self.paste_when_done = true;
            listening.recording.stop();
            cx.notify();
            return;
        }
        let text = self.input.read(cx).value().trim().to_string();
        if !text.is_empty() {
            cx.emit(DictationEvent::Paste(text));
        }
    }

    /// ⌘⏎ pastes; a plain Enter is a new line.
    fn enter(&mut self, enter: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        if enter.secondary {
            self.paste(cx);
            cx.stop_propagation();
        }
    }

    /// What it's doing, or why it stopped, at the left of the bar.
    fn state(&self, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let line = |text: String| {
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(text)
        };
        let Some(listening) = &self.listening else {
            return match &self.error {
                Some(error) => div()
                    .text_xs()
                    .text_color(theme.danger)
                    .child(error.clone())
                    .into_any_element(),
                None => line("⌘⏎ pastes into the thread on show".into()).into_any_element(),
            };
        };
        if listening.recording.stopped() {
            return line("Finishing…".into()).into_any_element();
        }
        match (&listening.status, listening.since) {
            (Some(status), _) => line(status.clone()).into_any_element(),
            (None, None) => line("Starting…".into()).into_any_element(),
            (None, Some(since)) => {
                let secs = since.elapsed().as_secs();
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().size_2().rounded_full().bg(theme.danger))
                    .child(line(format!("Listening {}:{:02}", secs / 60, secs % 60)))
                    .into_any_element()
            }
        }
    }
}

impl Render for DictationBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let language = Prefs::get(cx).dictation_language;
        let other = match language {
            Language::English => Language::Norwegian,
            Language::Norwegian => Language::English,
        };
        let listening = self.listening.is_some();
        let theme = cx.theme();
        let buttons = h_flex()
            .gap_1()
            .child(
                Button::new("dictation-language")
                    .label(language_label(language))
                    .icon(gpui_kit::assets::IconName::Languages)
                    .ghost()
                    .xsmall()
                    .tooltip(format!(
                        "Listen for {} from the next recording",
                        language_label(other)
                    ))
                    .on_click(move |_, _, cx| Prefs::update(cx, |p| p.dictation_language = other)),
            )
            .child(
                Button::new("dictation-record")
                    .icon(if listening {
                        gpui_kit::assets::IconName::CircleStop
                    } else {
                        gpui_kit::assets::IconName::Mic
                    })
                    .ghost()
                    .xsmall()
                    .tooltip(if listening {
                        "Stop listening (⌘D)"
                    } else {
                        "Listen for more (⌘D)"
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.record(window, cx))),
            )
            .child(
                Button::new("dictation-paste")
                    .label("Paste")
                    .primary()
                    .xsmall()
                    .tooltip("Paste into the thread on show (⌘⏎)")
                    .on_click(cx.listener(|this, _, _, cx| this.paste(cx))),
            )
            .child(
                Button::new("dictation-dismiss")
                    .icon(IconName::Close)
                    .ghost()
                    .xsmall()
                    .tooltip("Throw it away (Esc)")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(DictationEvent::Dismiss))),
            );
        v_flex()
            .key_context(CONTEXT)
            .capture_action(cx.listener(Self::enter))
            .on_action(cx.listener(|_, _: &Dismiss, _, cx| cx.emit(DictationEvent::Dismiss)))
            .w_full()
            .px_3()
            .py_2()
            .gap_2()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().min_w_0().child(self.state(cx)))
                    .child(buttons),
            )
            .child(Textarea::new(&self.input).small())
    }
}
