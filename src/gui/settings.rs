//! Settings (lemur → Settings…, ⌘,): the theme, the terminal's font,
//! scrollback and colors, the language dictation listens for, whether to
//! show Claude's plan usage, and Lemur's Linear sign-in. Changes apply at
//! once in every window and are saved to the config file.

use super::linear_panel::{self, LinearAccount};
use super::meter::ClaudeFound;
use super::sidebar::claude_icon;
use super::terminal::{Palette, SLOTS, Slot, set_color};
use crate::config::{Config, FONT_SIZES, Language, SCROLLBACKS, Terminal, ThemeChoice};
use crate::core_update::{self, Versions};
use alacritty_terminal::vte::ansi::Rgb;
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::setting::{
    NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage, Settings,
};
use gpui_kit::component::{ActiveTheme, IconName, Sizable, Theme, ThemeMode, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::path::PathBuf;
use std::sync::LazyLock;

actions!(settings, [CloseWindow]);

/// Key context of the Settings window.
pub const CONTEXT: &str = "Settings";

/// The settings every window follows. Change them with [`Prefs::update`].
#[derive(Clone, Default)]
pub struct Prefs {
    pub theme: ThemeChoice,
    pub dictation_language: Language,
    pub terminal: Terminal,
    /// Whether the sidebar shows how much of Claude's plan is used.
    pub claude_usage: bool,
    /// The herdr-cores Lemur uses on this Mac, whose versions Settings
    /// shows.
    pub cores: core_update::Cores,
    /// The config file that keeps changes; tests have none.
    file: Option<PathBuf>,
    /// Why the last change couldn't be saved.
    error: Option<String>,
}

impl Global for Prefs {}

impl Prefs {
    pub fn new(cfg: &Config, file: Option<PathBuf>) -> Self {
        Prefs {
            theme: cfg.theme,
            dictation_language: cfg.dictation_language,
            terminal: cfg.terminal.clone(),
            claude_usage: cfg.claude_usage,
            cores: core_update::Cores::of(cfg),
            file,
            error: None,
        }
    }

    /// The config file changes are saved to; tests have none.
    pub fn config_file(&self) -> Option<&std::path::Path> {
        self.file.as_deref()
    }

    /// The current settings; the defaults until the app sets them.
    pub fn get(cx: &App) -> &Prefs {
        static DEFAULT: LazyLock<Prefs> = LazyLock::new(Prefs::default);
        cx.try_global().unwrap_or(&DEFAULT)
    }

    /// Changes the settings: every window follows, and the config file keeps them.
    pub fn update(cx: &mut App, change: impl FnOnce(&mut Prefs)) {
        let mut prefs = Prefs::get(cx).clone();
        let theme = prefs.theme;
        change(&mut prefs);
        prefs.error = prefs.file.as_deref().and_then(|path| {
            Config::save_settings(
                path,
                prefs.theme,
                prefs.dictation_language,
                &prefs.terminal,
                prefs.claude_usage,
            )
            .err()
            .map(|e| format!("{e:#}"))
        });
        cx.set_global(prefs);
        if Prefs::get(cx).theme != theme {
            apply_theme(None, cx);
        }
    }
}

/// Goes light or dark as the theme setting says; `window` when one is at hand.
pub fn apply_theme(window: Option<&mut Window>, cx: &mut App) {
    match Prefs::get(cx).theme {
        ThemeChoice::System => Theme::sync_system_appearance(window, cx),
        ThemeChoice::Light => Theme::change(ThemeMode::Light, window, cx),
        ThemeChoice::Dark => Theme::change(ThemeMode::Dark, window, cx),
    }
}

/// The Settings window while it's open, so ⌘, brings it forward instead of
/// opening another.
struct Open(AnyWindowHandle);

impl Global for Open {}

pub fn open(cx: &mut App) {
    if let Some(&Open(window)) = cx.try_global::<Open>()
        && window
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("Settings".into()),
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(760.0), px(480.0)),
            cx,
        ))),
        ..Default::default()
    };
    if let Ok((window, _)) = gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| SettingsView::new(window, cx))
    }) {
        cx.set_global(Open(window));
    }
}

struct SettingsView {
    /// Installed monospaced fonts, as dropdown options.
    fonts: Vec<(SharedString, SharedString)>,
    /// A picker for each terminal color, dark then light.
    pickers: Vec<Picker>,
    focus: FocusHandle,
    _prefs: Subscription,
    _found: Subscription,
    _linear: Subscription,
    /// herdr-core's versions here and on each machine, once asked.
    core: Option<Result<Versions, String>>,
    /// Updating it, and then what that did, or why it didn't.
    updating: bool,
    core_note: Option<(String, bool)>,
    _pickers: Vec<Subscription>,
}

struct Picker {
    dark: bool,
    slot: Slot,
    label: &'static str,
    state: Entity<ColorPickerState>,
}

impl SettingsView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        // Focused, so ⌘W closes it before anything is clicked.
        focus.focus(window, cx);
        let fonts = monospace_fonts(cx)
            .into_iter()
            .map(|f| (f.clone(), f))
            .collect();
        let terminal = Prefs::get(cx).terminal.clone();
        let (mut pickers, mut subscriptions) = (vec![], vec![]);
        for dark in [true, false] {
            let palette = Palette::themed(dark, &terminal);
            for (slot, label) in SLOTS {
                let color = to_hsla(palette.get(slot));
                let state = cx.new(|cx| ColorPickerState::new(window, cx).default_value(color));
                subscriptions.push(cx.subscribe_in(
                    &state,
                    window,
                    move |_, _, event: &ColorPickerEvent, _, cx| {
                        let ColorPickerEvent::Change(color) = event;
                        let color = color.map(to_rgb);
                        Prefs::update(cx, |p| set_color(&mut p.terminal, dark, slot, color));
                    },
                ));
                pickers.push(Picker {
                    dark,
                    slot,
                    label,
                    state,
                });
            }
        }
        // Tests don't ask the release site.
        if !cfg!(test) {
            cx.defer_in(window, |this: &mut Self, _, cx| this.check_core(cx));
        }
        SettingsView {
            fonts,
            pickers,
            focus,
            _prefs: cx.observe_global_in::<Prefs>(window, |this, window, cx| {
                this.sync_pickers(window, cx);
                cx.notify();
            }),
            _found: cx.observe_global::<ClaudeFound>(|_, cx| cx.notify()),
            _linear: cx.observe_global::<LinearAccount>(|_, cx| cx.notify()),
            core: None,
            updating: false,
            core_note: None,
            _pickers: subscriptions,
        }
    }

    /// Shows each terminal color as Settings have it, after a reset or a
    /// change elsewhere.
    fn sync_pickers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let terminal = Prefs::get(cx).terminal.clone();
        for p in &self.pickers {
            let color = to_hsla(Palette::themed(p.dark, &terminal).get(p.slot));
            if p.state.read(cx).value() != Some(color) {
                p.state
                    .update(cx, |state, cx| state.set_value(color, window, cx));
            }
        }
    }

    /// The colors for a dark or a light app, each with a reset to
    /// lemur's own.
    fn color_group(&self, dark: bool) -> SettingGroup {
        let items = self.pickers.iter().filter(|p| p.dark == dark).map(|p| {
            let (state, slot) = (p.state.clone(), p.slot);
            let field = SettingField::render(move |_, _, _| ColorPicker::new(&state).small())
                .on_reset(
                    move |cx| {
                        Palette::themed(dark, &Prefs::get(cx).terminal).get(slot)
                            != Palette::default_for(dark).get(slot)
                    },
                    move |_, cx| {
                        Prefs::update(cx, |p| set_color(&mut p.terminal, dark, slot, None))
                    },
                );
            SettingItem::new(p.label, field)
        });
        let (title, description) = if dark {
            ("Dark", "While Lemur is dark. Starts as One Half Dark.")
        } else {
            ("Light", "While Lemur is light. Starts as One Half Light.")
        };
        SettingGroup::new()
            .title(title)
            .description(description)
            .items(items)
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let themes = ThemeChoice::ALL
            .map(|t| (t.as_str().into(), theme_label(t).into()))
            .to_vec();
        let theme = SettingField::dropdown(
            themes,
            |cx| Prefs::get(cx).theme.as_str().into(),
            |value: SharedString, cx| {
                let theme = ThemeChoice::ALL
                    .into_iter()
                    .find(|t| t.as_str() == value.as_ref())
                    .unwrap_or_default();
                Prefs::update(cx, |p| p.theme = theme);
            },
        )
        .default_value(ThemeChoice::default().as_str());
        let defaults = Terminal::default();
        let font = SettingField::scrollable_dropdown(
            self.fonts.clone(),
            |cx| Prefs::get(cx).terminal.font_family.clone().into(),
            |family: SharedString, cx| {
                Prefs::update(cx, |p| p.terminal.font_family = family.to_string())
            },
        )
        .default_value(defaults.font_family);
        let font_size = SettingField::number_input(
            NumberFieldOptions {
                min: *FONT_SIZES.start(),
                max: *FONT_SIZES.end(),
                step: 1.0,
            },
            |cx| Prefs::get(cx).terminal.font_size,
            |size, cx| Prefs::update(cx, |p| p.terminal.font_size = size),
        )
        .default_value(defaults.font_size);
        let scrollback = SettingField::number_input(
            NumberFieldOptions {
                min: f64::from(*SCROLLBACKS.start()),
                max: f64::from(*SCROLLBACKS.end()),
                step: 1000.0,
            },
            |cx| f64::from(Prefs::get(cx).terminal.scrollback),
            |lines, cx| {
                let lines = (lines.round() as u32).min(*SCROLLBACKS.end());
                Prefs::update(cx, |p| p.terminal.scrollback = lines)
            },
        )
        .default_value(f64::from(defaults.scrollback));
        let languages = Language::ALL
            .map(|l| (l.as_str().into(), language_label(l).into()))
            .to_vec();
        let language = SettingField::dropdown(
            languages,
            |cx| Prefs::get(cx).dictation_language.as_str().into(),
            |value: SharedString, cx| {
                let language = Language::ALL
                    .into_iter()
                    .find(|l| l.as_str() == value.as_ref())
                    .unwrap_or_default();
                Prefs::update(cx, |p| p.dictation_language = language);
            },
        )
        .default_value(Language::default().as_str());
        // Offered once Lemur has seen Claude Code; turned on, it can be
        // turned off whatever happens.
        let found = cx.try_global::<ClaudeFound>().is_some_and(|f| f.0);
        let usage_on = Prefs::get(cx).claude_usage;
        let usage = SettingItem::new(
            "Show plan usage",
            SettingField::switch(
                |cx| Prefs::get(cx).claude_usage,
                |on, cx| Prefs::update(cx, |p| p.claude_usage = on),
            ),
        )
        .disabled(!found && !usage_on)
        .description(if found || usage_on {
            "How much of your Claude plan is used, above the hosts' load in the sidebar. \
             Lemur runs Claude Code's /usage every 10 minutes (30 while Lemur isn't in front) \
             on the host where Claude ran last; Claude Code asks, with its own sign-in."
        } else {
            "Lemur hasn't seen Claude Code on your hosts yet. Start a Claude thread, \
             then turn this on."
        });
        let (status, error) = linear_panel::settings_status(cx);
        let (action, act) = linear_panel::settings_action(cx);
        let linear = SettingItem::new(
            "Account",
            SettingField::render(move |_, _, _| {
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(div().text_sm().child(status.clone()))
                    .child(
                        Button::new("linear-account")
                            .small()
                            .label(action)
                            .on_click(move |_, _, cx| act(cx)),
                    )
            }),
        )
        .description(error.unwrap_or_else(|| {
            "Lemur signs in with its own Linear app, and only reads: it searches issues and \
             shows the one pinned to a thread. The sign-in is kept in your Keychain, where \
             only Lemur can read it."
                .into()
        }));
        let settings = Settings::new("settings").sidebar_width(px(200.0)).pages([
            SettingPage::new("Appearance").icon(IconName::Sun).group(
                SettingGroup::new().item(
                    SettingItem::new("Theme", theme)
                        .description("System follows macOS's light or dark mode."),
                ),
            ),
            SettingPage::new("Terminal")
                .icon(IconName::SquareTerminal)
                .group(
                    SettingGroup::new()
                        .item(
                            SettingItem::new("Font", font)
                                .description("Monospaced fonts installed on this Mac."),
                        )
                        .item(SettingItem::new("Font size", font_size).description("In points."))
                        .item(SettingItem::new("Scrollback", scrollback).description(
                            "Lines each terminal keeps to scroll back through. A thousand \
                             lines of a terminal 130 columns wide take about 3 MB. herdr-core \
                             keeps about 2,600 of its own, which come back when Lemur \
                             attaches again. More apply at once; fewer when a terminal \
                             attaches again, as when Lemur starts.",
                        )),
                ),
            SettingPage::new("Terminal colors")
                .icon(IconName::Palette)
                .groups([self.color_group(true), self.color_group(false)]),
            SettingPage::new("Dictation")
                .icon(gpui_kit::assets::IconName::Mic)
                .group(
                    SettingGroup::new()
                        .item(SettingItem::new("Language", language).description(
                            "What ⌘D listens for. The dictation bar switches it too.",
                        )),
                ),
            SettingPage::new("Claude")
                .icon(claude_icon())
                .group(SettingGroup::new().item(usage)),
            SettingPage::new("herdr-core")
                .icon(super::icons::server())
                .group(self.core_group(cx)),
            SettingPage::new("Shortcuts")
                // Not among gpui-component's own icons; the full set has it.
                .icon(gpui_kit::assets::IconName::Keyboard)
                .groups(SHORTCUTS.iter().map(|(title, shortcuts)| {
                    SettingGroup::new()
                        .title(*title)
                        .items(shortcuts.iter().map(|s| {
                            let item = SettingItem::new(
                                s.what,
                                SettingField::render(|_, _, _| s.how.render()),
                            );
                            match s.more {
                                Some(more) => item.description(more),
                                None => item,
                            }
                        }))
                })),
            SettingPage::new("Linear")
                .icon(super::icons::ticket())
                .group(SettingGroup::new().item(linear)),
        ]);
        let error = Prefs::get(cx).error.clone();
        let danger = cx.theme().danger;
        v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(|_: &CloseWindow, window, _| window.remove_window())
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(div().flex_1().min_h_0().child(settings))
            .when_some(error, |this, error| {
                this.child(
                    div()
                        .px_4()
                        .py_2()
                        .text_sm()
                        .text_color(danger)
                        .child(format!("Couldn't save: {error}")),
                )
            })
    }
}

impl SettingsView {
    /// Asks which herdr-core runs here and on each machine, and the latest.
    fn check_core(&mut self, cx: &mut Context<Self>) {
        self.core = None;
        cx.notify();
        let cores = Prefs::get(cx).cores.clone();
        cx.spawn(async move |this, cx| {
            let versions = cx
                .background_spawn(async move { core_update::check(&cores) })
                .await;
            this.update(cx, |this, cx| {
                this.core = Some(versions.map_err(|e| format!("{e:#}")));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Brings what's behind up to the latest: here first, then the machines.
    fn update_core(&mut self, cx: &mut Context<Self>) {
        let Some(Ok(versions)) = &self.core else {
            return;
        };
        let outdated = versions.outdated();
        if outdated.is_empty() || self.updating {
            return;
        }
        self.updating = true;
        self.core_note = None;
        cx.notify();
        let cores = Prefs::get(cx).cores.clone();
        cx.spawn(async move |this, cx| {
            let done = cx
                .background_spawn(async move { core_update::update(&outdated, &cores) })
                .await;
            this.update(cx, |this, cx| {
                this.updating = false;
                this.core_note = Some(match done {
                    Ok(note) => (note, false),
                    Err(e) => (format!("{e:#}"), true),
                });
                this.check_core(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Each herdr-core's version, the latest, and Update.
    fn core_group(&self, cx: &mut Context<Self>) -> SettingGroup {
        let text =
            |s: String| SettingField::render(move |_, _, _| div().text_sm().child(s.clone()));
        let group = SettingGroup::new().description(
            "herdr-core runs Lemur's terminals, here and on each host. An update keeps them running.",
        );
        let versions = match &self.core {
            None => return group.item(SettingItem::new("Versions", text("Asking…".into()))),
            Some(Err(e)) => {
                return group.item(SettingItem::new(
                    "Versions",
                    text(format!("Couldn't ask: {e}")),
                ));
            }
            Some(Ok(v)) => v.clone(),
        };
        let latest = versions.latest.clone();
        let shown = |v: &Option<String>| match (v, &latest) {
            (None, _) => "unknown".to_string(),
            (Some(v), Some(l)) if core_update::newer(l, v) => format!("{v} · {l} is out"),
            (Some(v), Some(_)) => format!("{v} · up to date"),
            (Some(v), None) => v.clone(),
        };
        let mut here = shown(&versions.here);
        if let Some(by) = &versions.managed {
            here += &format!(" · {by} updates it");
        }
        let mut group = group.item(SettingItem::new("This Mac", text(here)));
        for (name, v) in &versions.machines {
            group = group.item(SettingItem::new(name.clone(), text(shown(v))));
        }
        let outdated = versions.outdated();
        let what = update_summary(&versions);
        let this = cx.entity().downgrade();
        let updating = self.updating;
        let can = !outdated.is_empty() && !updating;
        let buttons = SettingField::render(move |_, _, _| {
            let (update, check) = (this.clone(), this.clone());
            h_flex()
                .gap_2()
                .child(
                    Button::new("update-core")
                        .small()
                        .primary()
                        .label(if updating { "Updating…" } else { "Update" })
                        .disabled(!can)
                        .on_click(move |_, _, cx| {
                            update.update(cx, |v, cx| v.update_core(cx)).ok();
                        }),
                )
                .child(
                    Button::new("check-core")
                        .small()
                        .label("Check again")
                        .disabled(updating)
                        .on_click(move |_, _, cx| {
                            check.update(cx, |v, cx| v.check_core(cx)).ok();
                        }),
                )
        });
        let mut group = group.item(SettingItem::new("Update", buttons).description(what));
        if let Some((note, failed)) = self.core_note.clone() {
            let color = if failed {
                cx.theme().danger
            } else {
                cx.theme().muted_foreground
            };
            group = group.item(SettingItem::new(
                "",
                SettingField::render(move |_, _, _| {
                    div().text_sm().text_color(color).child(note.clone())
                }),
            ));
        }
        group
    }
}

/// What Update does, or why there's nothing for it to do.
fn update_summary(versions: &Versions) -> String {
    let outdated = versions.outdated();
    let Some(latest) = &versions.latest else {
        return "Couldn't ask for the latest release.".to_string();
    };
    if !outdated.is_empty() {
        return format!("Updates {}.", versions.describe(&outdated));
    }
    // Behind, but this Mac's isn't Lemur's to update, and the hosts get
    // what this Mac has.
    match &versions.managed {
        Some(by)
            if versions
                .here
                .as_deref()
                .is_some_and(|v| core_update::newer(latest, v)) =>
        {
            format!("{by} updates this Mac's herdr-core; then Lemur can update the hosts.")
        }
        _ => "Everything is up to date.".to_string(),
    }
}

/// A shortcut on the Shortcuts page.
struct Shortcut {
    what: &'static str,
    how: Press,
    more: Option<&'static str>,
}

/// How a shortcut goes: keys, one after another as alternatives, or a
/// click with keys held.
enum Press {
    Keys(&'static [&'static str]),
    Click(&'static str),
}

impl Press {
    fn render(&self) -> AnyElement {
        match self {
            Press::Keys(keys) => h_flex()
                .gap_1()
                .children(
                    keys.iter()
                        .filter_map(|key| Keystroke::parse(key).ok())
                        .map(Kbd::new),
                )
                .into_any_element(),
            Press::Click(click) => div().text_sm().child(*click).into_any_element(),
        }
    }
}

const fn keys(what: &'static str, keys: &'static [&'static str]) -> Shortcut {
    Shortcut {
        what,
        how: Press::Keys(keys),
        more: None,
    }
}

/// lemur's shortcuts, group by group: the keys `bind_keys` binds, and the
/// ones the terminal, the notes and the spotlights take themselves.
const SHORTCUTS: &[(&str, &[Shortcut])] = &[
    (
        "Window",
        &[
            Shortcut {
                what: "Command palette",
                how: Press::Keys(&["cmd-shift-p"]),
                more: Some("Every command by name, with its key."),
            },
            keys("Toggle the sidebar", &["cmd-b"]),
            Shortcut {
                what: "Toggle the right panel",
                how: Press::Keys(&["cmd-r"]),
                more: Some("The panel that showed last: the notes at first."),
            },
            keys("Toggle the agents", &["cmd-shift-a"]),
            Shortcut {
                what: "Next agent, or the one before",
                how: Press::Keys(&["cmd-]", "cmd-["]),
                more: Some("In the agents panel's order: the ones needing you first."),
            },
            keys("Add a project", &["cmd-o"]),
            keys("Settings", &["cmd-,"]),
            keys("Close Settings", &["cmd-w"]),
            keys("Hide Lemur", &["cmd-h"]),
            keys("Hide the other apps", &["alt-cmd-h"]),
            keys("Quit Lemur", &["cmd-q"]),
        ],
    ),
    (
        "Threads",
        &[
            Shortcut {
                what: "New terminal",
                how: Press::Keys(&["cmd-t"]),
                more: Some("A new tab where the thread on show runs."),
            },
            Shortcut {
                what: "New Claude thread or shell, or go to one",
                how: Press::Keys(&["cmd-j"]),
                more: Some(
                    "Named, in the worktree of the thread on show; type to find another thread.",
                ),
            },
            keys("Go to a thread", &["cmd-p"]),
            Shortcut {
                what: "Close the thread on show",
                how: Press::Keys(&["cmd-w"]),
                more: Some("Asking first while something runs in it."),
            },
            Shortcut {
                what: "Next thread in the worktree",
                how: Press::Keys(&["ctrl-tab"]),
                more: Some(
                    "The one opened before it; hold ⌃ and press Tab again to go further back.",
                ),
            },
            keys("Previous thread in the worktree", &["ctrl-shift-tab"]),
        ],
    ),
    (
        "Dictation",
        &[
            Shortcut {
                what: "Dictate",
                how: Press::Keys(&["cmd-d"]),
                more: Some(
                    "Opens a bar under the terminal that writes down what you say; again stops listening, and once more adds to it.",
                ),
            },
            Shortcut {
                what: "Paste it into the thread on show",
                how: Press::Keys(&["cmd-enter"]),
                more: Some("Without sending it. From the terminal too, after switching threads."),
            },
            keys("Throw it away", &["escape"]),
        ],
    ),
    (
        "Terminal",
        &[
            Shortcut {
                what: "Paste",
                how: Press::Keys(&["cmd-v"]),
                more: Some("An image or copied files go to the host, and their paths are pasted."),
            },
            Shortcut {
                what: "Open a link",
                how: Press::Click("⌘-click"),
                more: Some("Holding ⌘ underlines it first."),
            },
        ],
    ),
    (
        "Notes",
        &[
            Shortcut {
                what: "Open a link",
                how: Press::Click("⌘-click"),
                more: Some("Holding ⌘ underlines it first."),
            },
            Shortcut {
                what: "Make a link",
                how: Press::Keys(&["cmd-k"]),
                more: Some(
                    "Of the selected text, to the URL on the clipboard; in a link, it selects the URL.",
                ),
            },
            Shortcut {
                what: "Link the selected text",
                how: Press::Keys(&["cmd-v"]),
                more: Some("Pasting a URL over text on one line."),
            },
            Shortcut {
                what: "Tick a box",
                how: Press::Click("⌘-click"),
                more: Some("The [ ] of a - [ ] item; again unticks it."),
            },
            keys(
                "Tick the box on the cursor's line",
                &["cmd-enter", "shift-cmd-u"],
            ),
            keys("Make the lines a checklist", &["shift-cmd-l"]),
            Shortcut {
                what: "Next list item",
                how: Press::Keys(&["enter"]),
                more: Some("At the end of a list item; on an empty one, it ends the list."),
            },
            keys("Indent a list item", &["tab", "cmd-]"]),
            keys("Outdent a list item", &["shift-tab", "cmd-["]),
            Shortcut {
                what: "Move the lines up or down",
                how: Press::Keys(&["ctrl-cmd-up", "ctrl-cmd-down"]),
                more: Some("A list item takes its nested items along."),
            },
        ],
    ),
    (
        "Naming in the spotlight",
        &[
            keys("Pick Claude or Shell, for a new thread", &["up", "down"]),
            keys("Start or rename", &["enter"]),
            keys("Next field", &["tab"]),
            keys("Cancel", &["escape"]),
        ],
    ),
    (
        "Adding a project",
        &[
            keys("Choose a folder", &["up", "down"]),
            keys("Go into the folder", &["tab"]),
            keys("Add it", &["enter"]),
            keys("Close", &["escape"]),
        ],
    ),
];

fn to_hsla(c: Rgb) -> Hsla {
    rgb(u32::from(c.r) << 16 | u32::from(c.g) << 8 | u32::from(c.b)).into()
}

fn to_rgb(c: Hsla) -> Rgb {
    let c: Rgba = c.into();
    let channel = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgb {
        r: channel(c.r),
        g: channel(c.g),
        b: channel(c.b),
    }
}

pub fn language_label(language: Language) -> &'static str {
    match language {
        Language::English => "English",
        Language::Norwegian => "Norwegian Bokmål",
    }
}

fn theme_label(theme: ThemeChoice) -> &'static str {
    match theme {
        ThemeChoice::System => "System",
        ThemeChoice::Light => "Light",
        ThemeChoice::Dark => "Dark",
    }
}

/// Installed font families whose letters all have the same width.
fn monospace_fonts(cx: &App) -> Vec<SharedString> {
    let text = cx.text_system();
    text.all_font_names()
        .into_iter()
        // A leading dot marks macOS's private system fonts.
        .filter(|name| !name.starts_with('.'))
        .filter(|name| {
            let id = text.resolve_font(&font(name.clone()));
            let width = |c| text.advance(id, px(16.0), c).map(|s| s.width).ok();
            let i = width('i');
            i.is_some() && i == width('m') && i == width('W')
        })
        .map(SharedString::from)
        .collect()
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's `test` macro would shadow the built-in one.
    use super::{Prefs, Press, SHORTCUTS, SettingsView, open, to_hsla, update_summary};
    use crate::config::ThemeChoice;
    use crate::core_update::Versions;

    /// With nothing Lemur can update, Update says why, though this Mac's
    /// herdr-core is behind: a package manager updates it.
    #[test]
    fn update_says_why_theres_nothing_to_do() {
        let behind = Versions {
            here: Some("0.1.15".into()),
            managed: Some("homebrew".into()),
            machines: vec![("agent".into(), Some("0.1.15".into()))],
            latest: Some("0.1.16".into()),
        };
        assert_eq!(
            update_summary(&behind),
            "homebrew updates this Mac's herdr-core; then Lemur can update the hosts."
        );
        let current = Versions {
            here: Some("0.1.16".into()),
            machines: vec![("agent".into(), Some("0.1.16".into()))],
            ..behind.clone()
        };
        assert_eq!(update_summary(&current), "Everything is up to date.");
        let ours = Versions {
            managed: None,
            ..behind
        };
        assert_eq!(
            update_summary(&ours),
            "Updates this Mac 0.1.15, agent 0.1.15."
        );
    }
    use crate::gui::terminal::{DARK, Palette, Slot, set_color};
    use gpui_kit::component::ActiveTheme;
    use gpui_kit::{Keystroke, hsla};

    /// A color picked in Settings becomes that slot's color and is kept;
    /// a reset puts lemur's own back, in the picker too.
    #[gpui::test]
    fn picked_terminal_colors_are_kept(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(SettingsView::new);
        let background = view.read_with(cx, |v, _| {
            let p = &v.pickers[0];
            assert!(p.dark && p.slot == Slot::Background);
            p.state.clone()
        });
        let red = hsla(0.0, 1.0, 0.5, 1.0);
        cx.update(|window, cx| background.update(cx, |s, cx| s.select_color(red, window, cx)));
        cx.run_until_parked();
        let terminal = cx.update(|_, cx| Prefs::get(cx).terminal.clone());
        assert_eq!(terminal.dark.background.as_deref(), Some("#ff0000"));
        assert_eq!(
            Palette::themed(false, &terminal).background,
            Palette::default_for(false).background
        );

        cx.update(|_, cx| {
            Prefs::update(cx, |p| {
                set_color(&mut p.terminal, true, Slot::Background, None)
            })
        });
        cx.run_until_parked();
        assert_eq!(
            background.read_with(cx, |s, _| s.value()),
            Some(to_hsla(DARK.background))
        );
        assert!(cx.update(|_, cx| Prefs::get(cx).terminal.dark.is_empty()));
    }

    #[gpui::test]
    fn the_theme_setting_switches_light_and_dark(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        cx.update(|cx| {
            Prefs::update(cx, |p| p.theme = ThemeChoice::Dark);
            assert!(cx.theme().is_dark());
            Prefs::update(cx, |p| p.theme = ThemeChoice::Light);
            assert!(!cx.theme().is_dark());
        });
    }

    /// The Shortcuts page's keys parse, and the window's, the threads' and
    /// dictation's are the ones bound.
    #[gpui::test]
    fn the_shortcuts_page_shows_bound_keys(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            crate::gui::bind_keys(cx);
            let keymap = cx.key_bindings();
            let keymap = keymap.borrow();
            for (group, shortcuts) in SHORTCUTS {
                for shortcut in *shortcuts {
                    let Press::Keys(keys) = shortcut.how else {
                        continue;
                    };
                    for key in keys {
                        let stroke = Keystroke::parse(key).unwrap();
                        if ["Window", "Threads", "Dictation"].contains(group) {
                            let bound = keymap.bindings().any(|b| {
                                b.keystrokes().len() == 1
                                    && b.keystrokes()[0].to_string() == stroke.to_string()
                            });
                            assert!(bound, "{key} isn't bound");
                        }
                    }
                }
            }
        });
    }

    /// ⌘, brings an open Settings window forward rather than opening another;
    /// once it's closed, ⌘, opens a new one.
    #[gpui::test]
    fn settings_open_in_one_window(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let windows = |cx: &mut gpui::TestAppContext| cx.update(|cx| cx.windows());
        cx.update(open);
        cx.update(open);
        let open_windows = windows(cx);
        assert_eq!(open_windows.len(), 1);

        open_windows[0]
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        cx.run_until_parked();
        assert!(windows(cx).is_empty());
        cx.update(open);
        assert_eq!(windows(cx).len(), 1);
    }
}
