//! Settings, driven through the window as a person would.

use super::shortcuts::{Press, SHORTCUTS};
use super::update_summary;
use crate::config::Config;
use crate::core_update::Versions;
use crate::gui::actions::ACTIONS;
use crate::gui::commands::tests::{click, cmd, press, type_text, window};
use crate::gui::palette::{DARK, LIGHT, Palette, rgb};
use crate::gui::projects::HostChoice;
use crate::gui::{Lemur, Prefs};
use crate::test_support::{HcServer, TempDir};
use rune::{Chord, Key as K, Mods, Runtime};

fn no_config() -> Config {
    toml::from_str("").unwrap()
}

/// The window with Settings open on page `page` (its key, as `PAGES` has it).
fn settings_on(cfg: Config, page: &str) -> Runtime<Lemur> {
    let mut rt = window(cfg);
    rt.update(|s, _| s.open_settings());
    click(&mut rt, &format!("settings-page-{page}"));
    rt
}

fn frames(rt: &mut Runtime<Lemur>, n: usize) {
    for _ in 0..n {
        rt.frame(1.0 / 60.0);
    }
}

/// With nothing Lemur can update, Update says why, though this Mac's herdr-core is behind:
/// a package manager updates it.
#[test]
fn update_says_why_theres_nothing_to_do() {
    let behind = Versions {
        here: Some("0.1.15".into()),
        managed: Some("homebrew".into()),
        machines: vec![("agent".into(), Some("0.1.15".into()))],
        latest: Some("0.1.16".into()),
    };
    assert_eq!(update_summary(&behind), "homebrew updates this Mac's herdr-core; then Lemur can update the hosts.");
    let current = Versions { here: Some("0.1.16".into()), machines: vec![("agent".into(), Some("0.1.16".into()))], ..behind.clone() };
    assert_eq!(update_summary(&current), "Everything is up to date.");
    let ours = Versions { managed: None, ..behind };
    assert_eq!(update_summary(&ours), "Updates this Mac 0.1.15, agent 0.1.15.");
}

/// ⌘, opens Settings over the window, once: again leaves it as it is. ⌘W closes it, not the
/// thread on show, and so does Esc.
#[test]
fn settings_open_once_over_the_window() {
    let mut rt = window(no_config());
    press(&mut rt, K::Command(','), cmd());
    assert!(rt.state.panels.settings.open);
    assert!(!rt.state.keyboard_on_terminal(), "the terminal under it doesn't take keys");
    frames(&mut rt, 20);
    assert!(rt.find("settings").is_some() && rt.find("settings-nav").is_some());
    press(&mut rt, K::Command(','), cmd());
    assert!(rt.state.panels.settings.open);

    press(&mut rt, K::Command('w'), cmd());
    assert!(!rt.state.panels.settings.open);
    assert!(rt.state.dialog.is_none(), "nothing asked to close");
    press(&mut rt, K::Command(','), cmd());
    assert!(rt.state.panels.settings.open);
    press(&mut rt, K::Escape, Mods::default());
    assert!(!rt.state.panels.settings.open);
}

/// Help text goes under its item, across the page, so it leaves what the item sets the room
/// it needs, however narrow the window.
#[test]
fn help_text_goes_under_its_item() {
    let mut rt = window(no_config());
    rt.set_viewport((760.0, 514.0), 2.0);
    rt.update(|s, _| s.open_settings());
    frames(&mut rt, 20);
    let (help, _) = rt.presented("help-theme").expect("Theme's help text");
    let (content, _) = rt.presented("settings-content").expect("the page");
    assert!(help[2] > content[2] * 0.8, "{} of {}", help[2], content[2]);
}

/// The theme setting switches Lemur light and dark, whatever macOS is.
#[test]
fn the_theme_setting_switches_light_and_dark() {
    let mut rt = settings_on(no_config(), "appearance");
    assert!(rt.state.dark, "macOS is dark");
    for (theme, dark) in [("light", false), ("dark", true)] {
        click(&mut rt, "settings-theme-trigger");
        rt.resolve_popup(Some(theme)).expect("the theme menu");
        frames(&mut rt, 4);
        assert_eq!(rt.state.dark, dark, "{theme}");
    }
}

/// A color typed in Settings becomes that slot's color, in the terminals too, and is kept; a
/// reset puts lemur's own back.
#[test]
fn picked_terminal_colors_are_kept() {
    let mut rt = settings_on(no_config(), "colors");
    click(&mut rt, "hex-dark-background");
    assert!(rt.state.panels.settings.color.is_some(), "typing it");
    type_text(&mut rt, "#ff0000");
    let terminal = rt.state.prefs.terminal.clone();
    assert_eq!(terminal.dark.background.as_deref(), Some("#ff0000"));
    assert_eq!(Palette::themed(false, &terminal).background, LIGHT.background, "the light one stays");
    assert_eq!(rt.state.palette().background, rgb(0xff0000), "the terminals' colors");

    press(&mut rt, K::Enter, Mods::default());
    assert!(rt.state.panels.settings.color.is_none(), "done typing");
    click(&mut rt, "hex-dark-foreground");
    assert!(rt.state.panels.settings.color.is_some(), "typing another");
    press(&mut rt, K::Escape, Mods::default());
    assert!(rt.state.panels.settings.color.is_none(), "Esc stops typing");
    assert!(rt.state.panels.settings.open, "but leaves Settings open");
    click(&mut rt, "reset-dark-background");
    assert!(rt.state.prefs.terminal.dark.is_empty());
    assert_eq!(rt.state.palette().background, DARK.background);
    assert!(rt.find("reset-dark-background").is_none(), "nothing to reset");
}

/// What Settings changes is saved to the config file, the rest of it kept, and the terminals
/// take the font at once.
#[test]
fn changes_are_kept_in_the_config() {
    let dir = TempDir::new("settings-save");
    let file = dir.join("config.toml");
    std::fs::write(&file, "# mine\n").unwrap();
    let cfg = no_config();
    let mut rt = window(cfg.clone());
    rt.state.prefs = Prefs::new(&cfg, Some(file.clone()));
    rt.update(|s, _| s.open_settings());
    click(&mut rt, "settings-page-terminal");
    click(&mut rt, "settings-font-size-more");
    assert_eq!(rt.state.prefs.terminal.font_size, 14.0);
    assert_eq!(rt.state.metrics.size, 14.0, "the terminals' font");
    assert_eq!(rt.state.panels.settings.font_size.text, "14");
    click(&mut rt, "settings-scrollback-more");
    assert_eq!(rt.state.prefs.terminal.scrollback, 3000);
    let kept = std::fs::read_to_string(&file).unwrap();
    assert!(kept.contains("# mine") && kept.contains("font_size = 14") && kept.contains("scrollback = 3000"), "{kept}");
    assert!(rt.state.panels.settings.error.is_none());
}

/// How long notifications show is typed in milliseconds, or stepped a second at a time,
/// and kept in the config; 0, as at first, keeps them till they're answered.
#[test]
fn the_notification_dismiss_is_set_and_kept() {
    let dir = TempDir::new("settings-dismiss");
    let file = dir.join("config.toml");
    std::fs::write(&file, "# mine\n").unwrap();
    let cfg = no_config();
    let mut rt = window(cfg.clone());
    rt.state.prefs = Prefs::new(&cfg, Some(file.clone()));
    rt.update(|s, _| s.open_settings());
    click(&mut rt, "settings-page-notifications");
    assert_eq!(rt.state.panels.settings.dismiss.text, "0");
    click(&mut rt, "settings-dismiss-less");
    assert_eq!(rt.state.prefs.notification_dismiss_ms, 0, "not below 0");
    click(&mut rt, "settings-dismiss-more");
    click(&mut rt, "settings-dismiss-more");
    assert_eq!(rt.state.prefs.notification_dismiss_ms, 2000);
    assert!(std::fs::read_to_string(&file).unwrap().contains("notification_dismiss_ms = 2000"));
    rt.update(|s, _| s.panels.settings.dismiss = super::super::prompt::field(""));
    click(&mut rt, "settings-dismiss");
    type_text(&mut rt, "750");
    assert_eq!(rt.state.prefs.notification_dismiss_ms, 750);
    rt.update(|s, _| s.panels.settings.dismiss = super::super::prompt::field(""));
    type_text(&mut rt, "0");
    assert_eq!(rt.state.prefs.notification_dismiss_ms, 0);
    let kept = std::fs::read_to_string(&file).unwrap();
    assert!(kept.contains("# mine") && !kept.contains("notification_dismiss"), "{kept}");
    assert!(rt.state.panels.settings.error.is_none());
}

/// Claude's, Codex's and shells' notifications go on and off one by one, all on at first,
/// and the config keeps only those off.
#[test]
fn which_threads_notify_is_set_and_kept() {
    let dir = TempDir::new("settings-notifies");
    let file = dir.join("config.toml");
    std::fs::write(&file, "# mine\n").unwrap();
    let cfg = no_config();
    let mut rt = window(cfg.clone());
    rt.state.prefs = Prefs::new(&cfg, Some(file.clone()));
    rt.update(|s, _| s.open_settings());
    click(&mut rt, "settings-page-notifications");
    let all = |s: &Lemur| (s.prefs.claude_notifications, s.prefs.codex_notifications, s.prefs.shell_notifications);
    assert_eq!(all(&rt.state), (true, true, true));
    click(&mut rt, "settings-notify-shell");
    assert_eq!(all(&rt.state), (true, true, false));
    click(&mut rt, "settings-notify-codex");
    assert_eq!(all(&rt.state), (true, false, false));
    let kept = std::fs::read_to_string(&file).unwrap();
    assert!(kept.contains("codex_notifications = false") && kept.contains("shell_notifications = false"), "{kept}");
    assert!(!kept.contains("claude_notifications"), "{kept}");
    click(&mut rt, "settings-notify-shell");
    click(&mut rt, "settings-notify-codex");
    assert_eq!(all(&rt.state), (true, true, true));
    let kept = std::fs::read_to_string(&file).unwrap();
    assert!(kept.contains("# mine") && !kept.contains("notifications"), "{kept}");
    assert!(rt.state.panels.settings.error.is_none());
}

/// A size typed applies once it's one Lemur takes; until then the field says it isn't.
#[test]
fn a_font_size_typed_applies_once_its_valid() {
    let mut rt = settings_on(no_config(), "terminal");
    rt.update(|s, _| s.panels.settings.font_size = super::super::prompt::field("13"));
    frames(&mut rt, 2);
    type_text(&mut rt, "1");
    assert_eq!(rt.state.prefs.terminal.font_size, 13.0, "1 point is too small");
    type_text(&mut rt, "6");
    assert_eq!(rt.state.prefs.terminal.font_size, 16.0);
    assert_eq!(rt.state.metrics.size, 16.0);
}

/// Turned on, plan usage can be turned off, even before Lemur has seen the agent.
#[test]
fn usage_turned_on_can_be_turned_off() {
    let mut rt = window(no_config());
    rt.state.prefs.claude_usage = true;
    rt.update(|s, _| s.open_settings());
    click(&mut rt, "settings-page-claude");
    click(&mut rt, "settings-claude-usage");
    assert!(!rt.state.prefs.claude_usage);
}

/// The Shortcuts page lists every key an action has, and its other keys read as keys.
#[test]
fn the_shortcuts_page_lists_every_key() {
    let listed: Vec<_> = SHORTCUTS
        .iter()
        .flat_map(|(_, list)| list.iter())
        .filter_map(|s| match s.how {
            Press::Actions(actions) => Some(actions),
            _ => None,
        })
        .flatten()
        .collect();
    for (action, name, _, key) in ACTIONS {
        if key.is_some() {
            assert!(listed.contains(&action), "{name} isn't listed");
        }
    }
    for (_, list) in SHORTCUTS {
        for s in *list {
            if let Press::Keys(keys) = s.how {
                for key in keys {
                    assert!(Chord::parse(key).is_ok(), "{key} for {}", s.what);
                }
                assert_eq!(s.how.keycaps().len(), keys.len());
            }
        }
    }
    let mut rt = settings_on(no_config(), "shortcuts");
    frames(&mut rt, 4);
    assert!(rt.find("settings-thread-order-trigger").is_some());
    assert!(rt.find("help-shortcut-0-0").is_some(), "the palette's, told");
}

/// The thread order picked in Settings is the one ⌃Tab goes by.
#[test]
fn the_thread_order_is_picked_on_the_shortcuts_page() {
    let mut rt = settings_on(no_config(), "shortcuts");
    click(&mut rt, "settings-thread-order-trigger");
    rt.resolve_popup(Some("recent")).expect("the order menu");
    assert_eq!(rt.state.prefs.thread_order, crate::config::ThreadOrder::Recent);
}

/// A first host added through the picker counts for Settings' herdr-core page at once, not
/// after a restart: it would say all's up to date.
#[test]
fn settings_ask_a_host_once_its_added() {
    let server = HcServer::new("rprefs");
    let far = server.far_config("prefs-box").hosts.remove(0);
    let mut rt = window(server.config());
    let machines = |s: &Lemur| crate::core_update::Cores::of(&s.cfg).machines;
    assert!(!machines(&rt.state));
    rt.update(|s, _| s.add_folder(HostChoice { host: far, configured: false }, "/".into()));
    assert!(machines(&rt.state));
}

#[cfg(target_os = "macos")]
#[test]
fn the_fonts_offered_are_monospaced() {
    let fonts = super::monospace_fonts();
    assert!(fonts.iter().any(|f| f == "Menlo"), "{fonts:?}");
    assert!(!fonts.iter().any(|f| f == "Helvetica"), "{fonts:?}");
}
