//! Settings (lemur → Settings…, ⌘,): the theme, how long notifications show, the terminal's font,
//! scrollback and colors, the language dictation listens for, whether to
//! show Claude's and Codex's plan usage, Lemur's Linear sign-in, herdr-core's
//! versions, and every shortcut. Changes apply at once and are saved to the
//! config file.
//!
//! It's a page over the whole window, not a window of its own as it was: Rune gives each
//! window a state of its own, and what Settings changes is this one's, which the terminals
//! follow at once.

use super::palette::{Palette, Rgb, SLOTS, Slot, hex, rgb, set_color};
use super::terminal::Metrics;
use super::core::COMMAND_NEWS;
use super::{Lemur, icons};
use crate::config::{Agent, Config, FONT_SIZES, Language, SCROLLBACKS, ThemeChoice, ThreadOrder, parse_hex};
use crate::core_update::{self, Versions};
use rune::{El, Icon, Key as K, PopupMenu, ScrollSpec, TextEdit, Tx, col, el, icon, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, Select, SelectAppearance, Switch, TextInput};

/// Settings' pages, as its list has them.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Page {
    #[default]
    Appearance,
    Notifications,
    Terminal,
    Colors,
    Dictation,
    Claude,
    Codex,
    Core,
    Shortcuts,
    Linear,
}

/// Each page with its key, its name and its icon.
static PAGES: [(Page, &str, &str, &Icon); 10] = [
    (Page::Appearance, "appearance", "Appearance", &icons::SUN),
    (Page::Notifications, "notifications", "Notifications", &icons::BELL),
    (Page::Terminal, "terminal", "Terminal", &icons::SQUARE_TERMINAL),
    (Page::Colors, "colors", "Terminal colors", &icons::PALETTE),
    (Page::Dictation, "dictation", "Dictation", &icons::MIC),
    (Page::Claude, "claude", "Claude", &icons::CLAUDE),
    (Page::Codex, "codex", "Codex", &icons::CODE_XML),
    (Page::Core, "core", "herdr-core", &icons::SERVER),
    (Page::Shortcuts, "shortcuts", "Shortcuts", &icons::KEYBOARD),
    (Page::Linear, "linear", "Linear", &icons::TICKET),
];

#[derive(Default)]
pub struct Settings {
    /// Over the window, on `page`.
    pub open: bool,
    page: Page,
    /// Monospaced fonts installed on this Mac, once listed.
    fonts: Option<Vec<String>>,
    /// The font size, the scrollback and how long notifications show, as typed.
    font_size: TextEdit,
    scrollback: TextEdit,
    dismiss: TextEdit,
    /// A terminal color being typed: for a dark Lemur or a light one, which, and its field.
    color: Option<(bool, Slot, TextEdit)>,
    /// herdr-core's versions here and on each machine, once asked.
    core: Option<Result<Versions, String>>,
    /// Updating it, and then what that did, or why it didn't.
    updating: bool,
    core_note: Option<(String, bool)>,
    /// Why the last change couldn't be saved.
    error: Option<String>,
    /// The most lines of scrollback the terminals were given.
    given: Option<u32>,
}

/// A size as typed: whole ones without the ".0".
fn number(size: f64) -> String {
    if size.fract() == 0.0 { format!("{size:.0}") } else { size.to_string() }
}

impl Lemur {
    /// ⌘,: opens Settings over the window. Like the spotlight, it waits under what's over
    /// the window already.
    pub(super) fn open_settings(&mut self) {
        if self.panels.settings.open || self.overlaid() {
            return;
        }
        let terminal = &self.prefs.terminal;
        let (size, lines) = (number(terminal.font_size), terminal.scrollback);
        let dismiss = self.prefs.notification_dismiss_ms;
        let st = &mut self.panels.settings;
        st.open = true;
        st.font_size = TextEdit::new(size);
        st.scrollback = TextEdit::new(lines.to_string());
        st.dismiss = TextEdit::new(dismiss.to_string());
        st.color = None;
        st.given.get_or_insert(lines);
        // Tests ask neither this Mac's fonts nor the release site.
        if cfg!(test) {
            return;
        }
        if st.fonts.is_none() {
            self.in_background(monospace_fonts, |s, fonts| s.panels.settings.fonts = Some(fonts));
        }
        self.check_core();
    }

    /// Esc, ⌘W or Done: back to the window.
    pub(super) fn close_settings(&mut self) {
        let st = &mut self.panels.settings;
        st.open = false;
        st.color = None;
    }

    /// Keeps the settings in the config file, or says why it couldn't.
    fn save_settings(&mut self) {
        let p = &self.prefs;
        let saved = p.config_file().map(|file| {
            Config::save_settings(file, p.theme, p.dictation_language, p.thread_order, &p.terminal, p.claude_usage, p.codex_usage)
        });
        self.panels.settings.error = match saved {
            Some(Err(e)) => Some(format!("{e:#}")),
            _ => None,
        };
    }

    /// The view picks light or dark by it; the terminals follow once it has.
    fn set_theme(&mut self, theme: ThemeChoice) {
        self.prefs.theme = theme;
        self.save_settings();
    }

    fn set_language(&mut self, language: Language) {
        self.prefs.dictation_language = language;
        self.save_settings();
    }

    fn set_thread_order(&mut self, order: ThreadOrder) {
        self.prefs.thread_order = order;
        self.save_settings();
    }

    /// Notifications go by themselves once they've shown `ms`, or with 0 stay till they're
    /// answered: the ones showing too.
    fn set_dismiss(&mut self, ms: u32) {
        self.prefs.notification_dismiss_ms = ms;
        let saved = self.prefs.config_file().map(|file| Config::save_notification_dismiss(file, ms));
        self.panels.settings.error = match saved {
            Some(Err(e)) => Some(format!("{e:#}")),
            _ => None,
        };
    }

    /// Whether threads running `agent`, or with none a shell, notify: a notification out
    /// stays till it's answered.
    fn set_notifies(&mut self, agent: Option<Agent>, on: bool) {
        let p = &mut self.prefs;
        match agent {
            Some(Agent::Claude) => p.claude_notifications = on,
            Some(Agent::Codex) => p.codex_notifications = on,
            None => p.shell_notifications = on,
        }
        let saved = p.config_file().map(|file| Config::save_notifications(file, p.claude_notifications, p.codex_notifications, p.shell_notifications));
        self.panels.settings.error = match saved {
            Some(Err(e)) => Some(format!("{e:#}")),
            _ => None,
        };
    }

    fn dismiss_typed(&mut self) {
        let typed = self.panels.settings.dismiss.text.trim().parse::<u32>().ok();
        if let Some(ms) = typed.filter(|v| *v != self.prefs.notification_dismiss_ms) {
            self.set_dismiss(ms);
        }
    }

    /// − and +: a second shorter or longer.
    fn step_dismiss(&mut self, steps: i32) {
        let ms = i64::from(self.prefs.notification_dismiss_ms) + i64::from(steps) * 1000;
        let ms = ms.clamp(0, i64::from(u32::MAX)) as u32;
        self.panels.settings.dismiss.replace_text(ms.to_string());
        self.set_dismiss(ms);
    }

    fn set_font_family(&mut self, family: String) {
        self.prefs.terminal.font_family = family;
        self.refont();
    }

    fn set_font_size(&mut self, size: f64) {
        self.prefs.terminal.font_size = size;
        self.refont();
    }

    /// The terminal font changed: the terminals take its cells, and fit the pane in them.
    fn refont(&mut self) {
        let t = &self.prefs.terminal;
        self.metrics = Metrics::new(&t.font_family, t.font_size);
        self.fit_terminals();
        self.save_settings();
    }

    /// More lines apply to the terminals at once; fewer once a terminal attaches again.
    /// Typing a number passes through smaller ones, which would drop history for good
    /// (beyond what herdr-core keeps).
    fn set_scrollback(&mut self, lines: u32) {
        self.prefs.terminal.scrollback = lines;
        self.save_settings();
        let given = self.panels.settings.given.get_or_insert(lines);
        if lines > *given {
            *given = lines;
            for g in self.terminals.values_mut().filter_map(|v| v.grid.as_mut()) {
                let _ = g.set_scrollback(lines);
            }
        }
    }

    /// Sets a terminal color for a dark or a light Lemur, or with `None` puts lemur's own
    /// back; the terminals take it at once.
    fn set_terminal_color(&mut self, dark: bool, slot: Slot, color: Option<Rgb>) {
        set_color(&mut self.prefs.terminal, dark, slot, color);
        self.save_settings();
        self.recolor_terminals();
    }

    fn set_usage(&mut self, agent: Agent, on: bool) {
        match agent {
            Agent::Claude => self.prefs.claude_usage = on,
            Agent::Codex => self.prefs.codex_usage = on,
        }
        self.save_settings();
        self.follow_meter_setting();
    }

    fn font_size_typed(&mut self) {
        let typed = self.panels.settings.font_size.text.trim().parse::<f64>().ok();
        if let Some(size) = typed.filter(|v| FONT_SIZES.contains(v) && *v != self.prefs.terminal.font_size) {
            self.set_font_size(size);
        }
    }

    /// − and +, or ↓ and ↑ in the field: a point smaller or larger.
    fn step_font_size(&mut self, steps: i32) {
        let size = (self.prefs.terminal.font_size.round() + f64::from(steps)).clamp(*FONT_SIZES.start(), *FONT_SIZES.end());
        self.panels.settings.font_size.replace_text(number(size));
        self.set_font_size(size);
    }

    fn scrollback_typed(&mut self) {
        let typed = self.panels.settings.scrollback.text.trim().parse::<u32>().ok();
        if let Some(lines) = typed.filter(|v| SCROLLBACKS.contains(v) && *v != self.prefs.terminal.scrollback) {
            self.set_scrollback(lines);
        }
    }

    /// − and +: a thousand lines fewer or more.
    fn step_scrollback(&mut self, steps: i32) {
        let lines = i64::from(self.prefs.terminal.scrollback) + i64::from(steps) * 1000;
        let lines = lines.clamp(i64::from(*SCROLLBACKS.start()), i64::from(*SCROLLBACKS.end())) as u32;
        self.panels.settings.scrollback.replace_text(lines.to_string());
        self.set_scrollback(lines);
    }

    /// Types the color of `slot`, for a dark Lemur or a light one.
    fn edit_color(&mut self, dark: bool, slot: Slot) {
        let now = Palette::themed(dark, &self.prefs.terminal).get(slot);
        self.panels.settings.color = Some((dark, slot, super::prompt::field(&hex(now))));
    }

    fn color_typed(&mut self) {
        let Some((dark, slot, edit)) = &self.panels.settings.color else { return };
        let (dark, slot) = (*dark, *slot);
        if let Some(color) = parse_hex(edit.text.trim()).map(rgb)
            && Palette::themed(dark, &self.prefs.terminal).get(slot) != color
        {
            self.set_terminal_color(dark, slot, Some(color));
        }
    }

    /// Asks which herdr-core runs here and on each machine, and the latest.
    fn check_core(&mut self) {
        self.panels.settings.core = None;
        let cores = core_update::Cores::of(&self.cfg);
        self.in_background(
            move || core_update::check(&cores),
            |s, versions| s.panels.settings.core = Some(versions.map_err(|e| format!("{e:#}"))),
        );
    }

    /// Brings what's behind up to the latest: here first, then the machines. Settings says
    /// how it went, and asks again.
    fn update_cores(&mut self) {
        let st = &mut self.panels.settings;
        let Some(Ok(versions)) = &st.core else { return };
        let outdated = versions.outdated();
        if outdated.is_empty() || st.updating {
            return;
        }
        st.updating = true;
        st.core_note = None;
        let cores = core_update::Cores::of(&self.cfg);
        self.in_background(
            move || core_update::update(&outdated, &cores),
            |s, done| {
                let st = &mut s.panels.settings;
                st.updating = false;
                st.core_note = Some(match done {
                    Ok(note) => (note, false),
                    Err(e) => (format!("{e:#}"), true),
                });
                s.check_core();
            },
        );
    }
}

/// Settings over the window, while it's open: its pages on the left, the one picked beside
/// them, and why a change couldn't be saved under both.
pub fn overlay(s: &Lemur, k: &Kit) -> Option<El> {
    let st = &s.panels.settings;
    if !st.open {
        return None;
    }
    let c = &k.c;
    let mut nav = col().key("settings-nav").w(200.0).h_full().padding(8.0).gap(2.0);
    for &(page, id, name, glyph) in &PAGES {
        let on = st.page == page;
        let r = row()
            .key(format!("settings-page-{id}"))
            .h(28.0)
            .px(8.0)
            .gap(8.0)
            .center_y()
            .radius(k.radius.sm)
            .cursor(rune::Cursor::Pointer)
            .child(icon(glyph, 14.0).color(if on { c.icon_base } else { c.icon_muted }))
            .child(text(name).font(if on { k.ty.ui_medium } else { k.ty.ui }).color(c.text_base));
        let r = if on { r.fill(c.layer_02) } else { k.interactive(r) };
        nav = nav.child(r.on_press(move |s: &mut Lemur| {
            s.panels.settings.page = page;
            s.panels.settings.color = None;
        }));
    }
    let name = PAGES.iter().find(|(p, ..)| *p == st.page).map_or("", |(_, _, name, _)| name);
    let groups = match st.page {
        Page::Appearance => appearance(s, k),
        Page::Notifications => notifications(s, k),
        Page::Terminal => terminal(s, k),
        Page::Colors => vec![color_group(s, true, k), color_group(s, false, k)],
        Page::Dictation => dictation(s, k),
        Page::Claude => usage(s, Agent::Claude, k),
        Page::Codex => usage(s, Agent::Codex, k),
        Page::Core => core(s, k),
        Page::Shortcuts => shortcuts::page(s, k),
        Page::Linear => linear(s, k),
    };
    let content = col()
        .key("settings-content")
        .w_full()
        .padding(24.0)
        .gap(20.0)
        .child(text(name).font(k.ty.body_medium).color(c.text_base))
        .children(groups);
    let pane = col()
        .key(format!("settings-{name}"))
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .min_h(0.0)
        .border_sides([0.0, 0.0, 0.0, 1.0], c.border_weak)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(content);
    let mut body = col().w_full().h_full().min_h(0.0).child(row().w_full().grow(1.0).min_h(0.0).child(nav).child(pane));
    if let Some(error) = &st.error {
        body = body.child(
            row()
                .key("settings-error")
                .w_full()
                .px(16.0)
                .py(8.0)
                .border_sides([1.0, 0.0, 0.0, 0.0], c.border_weak)
                .child(text(format!("Couldn't save: {error}")).font(k.ty.ui).color(c.danger.fg).wrap()),
        );
    }
    let done = Button::new("Done")
        .variant(ButtonVariant::Outline)
        .control_size(ControlSize::Small)
        .on_press(|s: &mut Lemur| s.close_settings())
        .build(k)
        .key("settings-done");
    let page = rune_kit::Page::new(body).title("Settings").trailing(done).frame_inset(6.0).build(k);
    Some(el().key("settings").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).child(page))
}

/// A setting: its name beside what sets it, and its help under both, across the page, so it
/// leaves what sets it the room it needs however narrow the window.
fn item(id: &str, title: &str, control: El, help: Option<String>, k: &Kit) -> El {
    let c = &k.c;
    let line = row()
        .w_full()
        .min_h(28.0)
        .gap(16.0)
        .center_y()
        .child(text(title.to_owned()).font(k.ty.ui).color(c.text_base).grow(1.0).shrink().truncate())
        .child(control);
    let mut e = col().key(format!("setting-{id}")).w_full().py(if help.is_some() { 10.0 } else { 6.0 }).gap(4.0).child(line);
    if let Some(help) = help {
        e = e.child(text(help).key(format!("help-{id}")).w_full().font(k.ty.caption()).color(c.text_muted).wrap());
    }
    e
}

/// Settings that go together, under a title and what they're about, if they have them.
fn group(title: Option<&str>, about: Option<&str>, items: Vec<El>, k: &Kit) -> El {
    let c = &k.c;
    let mut g = col().w_full().gap(6.0);
    if let Some(title) = title {
        g = g.child(text(title.to_owned()).font(k.ty.ui_medium).color(c.text_base));
    }
    if let Some(about) = about {
        g = g.child(text(about.to_owned()).w_full().font(k.ty.caption()).color(c.text_muted).wrap());
    }
    let mut card = col().w_full().px(14.0).radius(k.radius.md).border(1.0, c.border_weak).fill(c.layer_01);
    for (i, item) in items.into_iter().enumerate() {
        if i > 0 {
            card = card.child(el().w_full().h(1.0).fill(c.border_weak));
        }
        card = card.child(item);
    }
    g.child(card)
}

/// A pop-up button showing `current`'s name among `options` (`(id, name)`), which offers
/// them in a native menu and runs `pick` with the one picked.
fn picker(id: &str, label: &str, options: Vec<(String, String)>, current: &str, pick: fn(&mut Lemur, &str), k: &Kit) -> El {
    let shown = options.iter().find(|(o, _)| o == current).map_or_else(|| current.to_owned(), |(_, n)| n.clone());
    let (anchor, current) = (format!("settings-{id}-trigger"), current.to_owned());
    Select::new(format!("settings-{id}"), &[shown.as_str()])
        .current(Some(&shown))
        .label(label)
        .appearance(SelectAppearance::Field)
        .width(220.0)
        .trigger(k)
        .on_press_tx(move |_: &mut Lemur, tx: &mut Tx| {
            let menu = PopupMenu::picker(options.iter().map(|(o, n)| (o.as_str(), n.as_str())), &current);
            tx.popup_menu(menu, anchor.as_str(), move |s: &mut Lemur, picked, _| {
                if let Some(picked) = picked {
                    pick(s, picked);
                }
            });
        })
}

/// A number field with − and + beside it: ↓ and ↑ in it step it too.
fn stepper(id: &str, edit: &TextEdit, access: fn(&mut Lemur) -> &mut TextEdit, typed: fn(&mut Lemur), by: fn(&mut Lemur, i32), invalid: bool, k: &Kit) -> El {
    let input = TextInput::new(edit)
        .width(Some(90.0))
        .invalid(invalid)
        .on_edit(access)
        .on_change(move |s: &mut Lemur, _| typed(s))
        .on_submit(|_: &mut Lemur, tx| tx.blur())
        .on_key(move |s: &mut Lemur, ev, _| match ev.key {
            K::Up => {
                by(s, 1);
                true
            }
            K::Down => {
                by(s, -1);
                true
            }
            _ => false,
        })
        .build(k)
        .key(format!("settings-{id}"));
    let step = |glyph: &'static Icon, label: &str, steps: i32| {
        IconButton::new(glyph)
            .variant(IconButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .label(label)
            .on_press(move |s: &mut Lemur| by(s, steps))
            .build(k)
            .key(format!("settings-{id}-{}", if steps < 0 { "less" } else { "more" }))
    };
    row().gap(4.0).center_y().child(step(&icons::MINUS, "Fewer", -1)).child(input).child(step(&icons::PLUS, "More", 1))
}

fn appearance(s: &Lemur, k: &Kit) -> Vec<El> {
    let themes = ThemeChoice::ALL.iter().map(|t| (t.as_str().to_owned(), theme_label(*t).to_owned())).collect();
    let theme = picker(
        "theme",
        "Theme",
        themes,
        s.prefs.theme.as_str(),
        |s, id| s.set_theme(ThemeChoice::ALL.into_iter().find(|t| t.as_str() == id).unwrap_or_default()),
        k,
    );
    vec![group(None, None, vec![item("theme", "Theme", theme, Some("System follows macOS's light or dark mode.".into()), k)], k)]
}

fn notifications(s: &Lemur, k: &Kit) -> Vec<El> {
    let st = &s.panels.settings;
    let ok = st.dismiss.text.trim().parse::<u32>().is_ok();
    let after = stepper("dismiss", &st.dismiss, |s| &mut s.panels.settings.dismiss, Lemur::dismiss_typed, Lemur::step_dismiss, !ok, k);
    let help = "Milliseconds before Lemur takes a notification away, from Notification Center too. 0 leaves it until you clear it or open its thread. System Settings › Notifications › Lemur says whether it shows as a banner or an alert. Silence a thread to have none from it.";
    let which = [Some(Agent::Claude), Some(Agent::Codex), None].map(|agent| notifies(s, agent, k)).into();
    vec![
        group(Some("Notify of"), None, which, k),
        group(None, None, vec![item("dismiss", "Dismiss after (ms)", after, Some(help.into()), k)], k),
    ]
}

/// Whether threads running `agent`, or with none a shell, notify.
fn notifies(s: &Lemur, agent: Option<Agent>, k: &Kit) -> El {
    let on = s.prefs.notifies(agent);
    let (id, title, help) = match agent {
        Some(Agent::Claude) => ("notify-claude", "Claude", "A Claude thread finishing or needing input.".to_string()),
        Some(Agent::Codex) => ("notify-codex", "Codex", "A Codex thread finishing or needing input.".to_string()),
        None => (
            "notify-shell",
            "Shell commands",
            format!(
                "A command finishing in a shell thread, once it has run {} seconds. Editors, pagers, ssh and the like wait on you, so they don't count.",
                COMMAND_NEWS.as_secs()
            ),
        ),
    };
    let switch = Switch::new(on)
        .label(title)
        .hide_label()
        .on_toggle(move |s: &mut Lemur| s.set_notifies(agent, !on))
        .build(k)
        .key(format!("settings-{id}"));
    item(id, title, switch, Some(help), k)
}

fn terminal(s: &Lemur, k: &Kit) -> Vec<El> {
    let st = &s.panels.settings;
    let t = &s.prefs.terminal;
    let mut fonts = st.fonts.clone().unwrap_or_default();
    if !fonts.contains(&t.font_family) {
        fonts.insert(0, t.font_family.clone());
    }
    let fonts = fonts.into_iter().map(|f| (f.clone(), f)).collect();
    let font = picker("font", "Font", fonts, &t.font_family, |s, family| s.set_font_family(family.to_owned()), k);
    let size_ok = st.font_size.text.trim().parse::<f64>().is_ok_and(|v| FONT_SIZES.contains(&v));
    let size = stepper(
        "font-size",
        &st.font_size,
        |s| &mut s.panels.settings.font_size,
        Lemur::font_size_typed,
        Lemur::step_font_size,
        !size_ok,
        k,
    );
    let lines_ok = st.scrollback.text.trim().parse::<u32>().is_ok_and(|v| SCROLLBACKS.contains(&v));
    let lines = stepper(
        "scrollback",
        &st.scrollback,
        |s| &mut s.panels.settings.scrollback,
        Lemur::scrollback_typed,
        Lemur::step_scrollback,
        !lines_ok,
        k,
    );
    let listed = if st.fonts.is_some() { "Monospaced fonts installed on this Mac." } else { "Monospaced fonts installed on this Mac, once they're listed." };
    vec![group(
        None,
        None,
        vec![
            item("font", "Font", font, Some(listed.into()), k),
            item("font-size", "Font size", size, Some(format!("In points, {} to {}.", FONT_SIZES.start(), FONT_SIZES.end())), k),
            item(
                "scrollback",
                "Scrollback",
                lines,
                Some(
                    "Lines each terminal keeps to scroll back through. A thousand lines of a terminal 130 columns wide \
                     take about 3 MB. herdr-core keeps about 2,600 of its own, which come back when Lemur attaches \
                     again. More apply at once; fewer when a terminal attaches again, as when Lemur starts."
                        .into(),
                ),
                k,
            ),
        ],
        k,
    )]
}

/// A slot's key: its name, lowercased and joined with dashes.
fn slot_id(label: &str) -> String {
    label.to_lowercase().replace(' ', "-")
}

/// The colors for a dark or a light Lemur, each with a reset to lemur's own.
fn color_group(s: &Lemur, dark: bool, k: &Kit) -> El {
    let c = &k.c;
    let palette = Palette::themed(dark, &s.prefs.terminal);
    let defaults = Palette::default_for(dark);
    let side = if dark { "dark" } else { "light" };
    let items = SLOTS
        .iter()
        .map(|&(slot, label)| {
            let id = format!("{side}-{}", slot_id(label));
            let color = palette.get(slot);
            let typing = s.panels.settings.color.as_ref().filter(|(d, sl, _)| *d == dark && *sl == slot);
            let value = match typing {
                Some((_, _, edit)) => TextInput::new(edit)
                    .width(Some(96.0))
                    .invalid(parse_hex(edit.text.trim()).is_none())
                    .on_edit(|s: &mut Lemur| &mut s.panels.settings.color.as_mut().expect("a color being typed").2)
                    .on_change(|s: &mut Lemur, _| s.color_typed())
                    // The field goes, and the keyboard with it.
                    .on_submit(|s: &mut Lemur, _| s.panels.settings.color = None)
                    .on_key(|s: &mut Lemur, ev, _| {
                        let done = ev.key == K::Escape;
                        if done {
                            s.panels.settings.color = None;
                        }
                        done
                    })
                    .build(k)
                    .key(format!("hex-{id}")),
                None => row()
                    .key(format!("hex-{id}"))
                    .w(96.0)
                    .h(28.0)
                    .px(8.0)
                    .center_y()
                    .radius(k.radius.md)
                    .cursor(rune::Cursor::Text)
                    .child(text(hex(color)).font(k.ty.ui).color(c.text_muted))
                    .help("Type a color")
                    .on_press(move |s: &mut Lemur| s.edit_color(dark, slot)),
            };
            let swatch = el()
                .key(format!("swatch-{id}"))
                .size(18.0, 18.0)
                .radius(4.0)
                .fill(rune::color::hex(&hex(color)).unwrap_or(c.black))
                .border(1.0, c.border_base);
            let mut control = row().gap(8.0).center_y();
            if color != defaults.get(slot) {
                control = control.child(
                    icons::button(&icons::ROTATE_CCW, format!("Put lemur's own back: {}", hex(defaults.get(slot))))
                        .on_press(move |s: &mut Lemur| {
                            s.panels.settings.color = None;
                            s.set_terminal_color(dark, slot, None);
                        })
                        .build(k)
                        .key(format!("reset-{id}")),
                );
            }
            item(&id, label, control.child(swatch).child(value), None, k)
        })
        .collect();
    let (title, about) = if dark {
        ("Dark", "While Lemur is dark. Starts as One Half Dark.")
    } else {
        ("Light", "While Lemur is light. Starts as One Half Light.")
    };
    group(Some(title), Some(about), items, k)
}

fn dictation(s: &Lemur, k: &Kit) -> Vec<El> {
    let languages = Language::ALL.iter().map(|l| (l.as_str().to_owned(), language_label(*l).to_owned())).collect();
    let language = picker(
        "language",
        "Language",
        languages,
        s.prefs.dictation_language.as_str(),
        |s, id| s.set_language(Language::ALL.into_iter().find(|l| l.as_str() == id).unwrap_or_default()),
        k,
    );
    let help = "What ⌘D listens for. The dictation bar switches it too.";
    vec![group(None, None, vec![item("language", "Language", language, Some(help.into()), k)], k)]
}

/// Whether the sidebar shows how much of `agent`'s plan is used. Offered once Lemur has seen
/// it; turned on, it can be turned off whatever happens.
fn usage(s: &Lemur, agent: Agent, k: &Kit) -> Vec<El> {
    let (on, found) = match agent {
        Agent::Claude => (s.prefs.claude_usage, s.claude_found()),
        Agent::Codex => (s.prefs.codex_usage, s.codex_found()),
    };
    let id = match agent {
        Agent::Claude => "claude-usage",
        Agent::Codex => "codex-usage",
    };
    let switch = Switch::new(on)
        .label("Show plan usage")
        .hide_label()
        .disabled(!found && !on)
        .on_toggle(move |s: &mut Lemur| s.set_usage(agent, !on))
        .build(k)
        .key(format!("settings-{id}"));
    let help = match (agent, found || on) {
        (Agent::Claude, true) => {
            "How much of your Claude plan is used, above the hosts' load in the sidebar. Lemur runs Claude Code's \
             /usage every 10 minutes (30 while Lemur isn't in front) on the host where Claude ran last; Claude Code \
             asks, with its own sign-in."
        }
        (Agent::Claude, false) => "Lemur hasn't seen Claude Code on your hosts yet. Start a Claude thread, then turn this on.",
        (Agent::Codex, true) => {
            "How much of your ChatGPT plan's Codex use is used, above the hosts' load in the sidebar. Lemur asks \
             Codex's app server every 10 minutes (30 while Lemur isn't in front) on the host where Codex ran last; \
             Codex asks, with its own sign-in. Signed in with an API key there's no plan: it shows the tokens Codex \
             used there, from Codex's session logs."
        }
        (Agent::Codex, false) => "Lemur hasn't seen Codex on your hosts yet. Start a Codex thread, then turn this on.",
    };
    vec![group(None, None, vec![item(id, "Show plan usage", switch, Some(help.into()), k)], k)]
}

fn core(s: &Lemur, k: &Kit) -> Vec<El> {
    let st = &s.panels.settings;
    let c = &k.c;
    let value = |t: String| text(t).font(k.ty.ui).color(c.text_muted);
    let about = Some("herdr-core runs Lemur's terminals, here and on each host. An update keeps them running.");
    let versions = match &st.core {
        None => return vec![group(None, about, vec![item("core-versions", "Versions", value("Asking…".into()), None, k)], k)],
        Some(Err(e)) => {
            return vec![group(None, about, vec![item("core-versions", "Versions", value(format!("Couldn't ask: {e}")), None, k)], k)];
        }
        Some(Ok(v)) => v,
    };
    let shown = |v: &Option<String>| match (v, &versions.latest) {
        (None, _) => "unknown".to_string(),
        (Some(v), Some(l)) if core_update::newer(l, v) => format!("{v} · {l} is out"),
        (Some(v), Some(_)) => format!("{v} · up to date"),
        (Some(v), None) => v.clone(),
    };
    let mut here = shown(&versions.here);
    if let Some(by) = &versions.managed {
        here += &format!(" · {by} updates it");
    }
    let mut items = vec![item("core-here", "This Mac", value(here), None, k)];
    for (name, v) in &versions.machines {
        items.push(item(&format!("core-{name}"), name, value(shown(v)), None, k));
    }
    let can = !versions.outdated().is_empty() && !st.updating;
    let buttons = row()
        .gap(8.0)
        .child(
            Button::new(if st.updating { "Updating…" } else { "Update" })
                .variant(ButtonVariant::Contrast)
                .control_size(ControlSize::Small)
                .disabled(!can)
                .on_press(|s: &mut Lemur| s.update_cores())
                .build(k)
                .key("settings-core-update"),
        )
        .child(
            Button::new("Check again")
                .variant(ButtonVariant::Outline)
                .control_size(ControlSize::Small)
                .disabled(st.updating)
                .on_press(|s: &mut Lemur| s.check_core())
                .build(k)
                .key("settings-core-check"),
        );
    items.push(item("core-update", "Update", buttons, Some(update_summary(versions)), k));
    if let Some((note, failed)) = &st.core_note {
        let ink = if *failed { c.danger.fg } else { c.text_muted };
        items.push(row().key("settings-core-note").w_full().py(10.0).child(text(note.clone()).w_full().font(k.ty.caption()).color(ink).wrap()));
    }
    vec![group(None, about, items, k)]
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
    // Behind, but this Mac's isn't Lemur's to update, and the hosts get what this Mac has.
    match &versions.managed {
        Some(by) if versions.here.as_deref().is_some_and(|v| core_update::newer(latest, v)) => {
            format!("{by} updates this Mac's herdr-core; then Lemur can update the hosts.")
        }
        _ => "Everything is up to date.".to_string(),
    }
}

fn linear(s: &Lemur, k: &Kit) -> Vec<El> {
    let (status, error) = s.linear_account_status();
    let (action, act) = s.linear_account_action();
    let control = row().gap(12.0).center_y().child(text(status).font(k.ty.ui).color(k.c.text_muted)).child(
        Button::new(action)
            .variant(ButtonVariant::Outline)
            .control_size(ControlSize::Small)
            .on_press(move |s: &mut Lemur| act(s))
            .build(k)
            .key("settings-linear-account"),
    );
    let help = error.unwrap_or_else(|| {
        "Lemur signs in with its own Linear app, and only reads: it searches issues and shows the one pinned to a \
         thread. The sign-in is kept in your Keychain, where only Lemur can read it."
            .into()
    });
    vec![group(None, None, vec![item("linear-account", "Account", control, Some(help), k)], k)]
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

/// Installed font families whose letters all have the same width, by name. A leading dot
/// marks macOS's private system fonts.
fn monospace_fonts() -> Vec<String> {
    use skrifa::MetadataProvider;
    use skrifa::instance::{LocationRef, Size};
    let mut fonts = fontique::Collection::new(fontique::CollectionOptions { shared: false, system_fonts: true });
    let names: Vec<String> = fonts.family_names().filter(|n| !n.starts_with('.')).map(str::to_owned).collect();
    let mut mono: Vec<String> = names
        .into_iter()
        .filter(|name| {
            let Some(family) = fonts.family_by_name(name) else { return false };
            let Some(font) = family.default_font() else { return false };
            let Some(data) = font.load(None) else { return false };
            let Ok(face) = skrifa::FontRef::from_index(data.as_ref(), font.index()) else { return false };
            let (map, metrics) = (face.charmap(), face.glyph_metrics(Size::unscaled(), LocationRef::default()));
            let width = |c: char| map.map(c).and_then(|g| metrics.advance_width(g));
            let i = width('i');
            i.is_some() && i == width('m') && i == width('W')
        })
        .collect();
    mono.sort_by_key(|n| n.to_lowercase());
    mono.dedup();
    mono
}

#[path = "settings_shortcuts.rs"]
mod shortcuts;

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;

/// The Shortcuts page's keys (not the actions'), group by group, for the window's tests.
#[cfg(test)]
pub(super) fn shortcuts_for_tests() -> Vec<(&'static str, Vec<&'static str>)> {
    let keys = |s: &shortcuts::Shortcut| match s.how {
        shortcuts::Press::Keys(keys) => keys.to_vec(),
        _ => vec![],
    };
    shortcuts::SHORTCUTS.iter().map(|(group, list)| (*group, list.iter().flat_map(keys).collect())).collect()
}
