//! ⌘+, ⌘− and ⌘0 zoom the window; the config keeps the zoom.

use crate::config::Config;
use crate::gui::commands::tests::{cmd, press, window};
use crate::gui::{Lemur, Prefs};
use crate::test_support::TempDir;
use rune::{Key as K, Runtime};

/// The zoom Lemur keeps, and the one the window lays out at.
fn zoom(rt: &mut Runtime<Lemur>) -> (f32, f32) {
    rt.frame(1.0 / 60.0);
    (rt.state.prefs.zoom, rt.zoom())
}

/// ⌘= and ⌘+ (a Norwegian keyboard's own key) step in, ⌘− out, ⌘0 back, as far as the
/// steps go; the config keeps all but 100%.
#[test]
fn the_window_zooms_in_and_out_and_back() {
    let dir = TempDir::new("zoom");
    let file = dir.join("config.toml");
    std::fs::write(&file, "# mine\n").unwrap();
    let cfg: Config = toml::from_str("").unwrap();
    let mut rt = window(cfg.clone());
    rt.state.prefs = Prefs::new(&cfg, Some(file.clone()));
    assert_eq!(zoom(&mut rt), (1.0, 1.0));
    press(&mut rt, K::Command('='), cmd());
    assert_eq!(zoom(&mut rt), (1.1, 1.1));
    press(&mut rt, K::Command('+'), cmd());
    assert_eq!(zoom(&mut rt), (1.25, 1.25));
    assert!(std::fs::read_to_string(&file).unwrap().contains("zoom = 1.25\n"));
    assert_eq!(rt.state.status.as_ref().map(|(t, _)| t.as_str()), Some("Zoom 125%"));
    for _ in 0..3 {
        press(&mut rt, K::Command('-'), cmd());
    }
    assert_eq!(zoom(&mut rt), (0.9, 0.9));
    press(&mut rt, K::Command('0'), cmd());
    assert_eq!(zoom(&mut rt), (1.0, 1.0));
    let kept = std::fs::read_to_string(&file).unwrap();
    assert!(kept.contains("# mine") && !kept.contains("zoom"), "{kept}");

    // No further than the steps go; one between them steps to the next.
    rt.update(|s, _| s.set_zoom(2.0));
    press(&mut rt, K::Command('='), cmd());
    assert_eq!(zoom(&mut rt).0, 2.0);
    rt.update(|s, _| s.set_zoom(0.6));
    press(&mut rt, K::Command('-'), cmd());
    assert_eq!(zoom(&mut rt).0, 0.5);
    press(&mut rt, K::Command('-'), cmd());
    assert_eq!(zoom(&mut rt).0, 0.5);
}

/// Zoomed in, the terminal has fewer, larger cells in the same window.
#[test]
fn zoomed_the_terminal_fits_fewer_cells() {
    let server = crate::test_support::HcServer::new("rzoom");
    let t = server.run("/", &[], &["sh"]);
    let mut rt = crate::gui::tests::window(server.config());
    let key = ("local".to_string(), t);
    rt.update(|s, _| s.open(key.0.clone(), key.1.clone()));
    let painted = crate::gui::core::wait_for(&mut rt, 10.0, |s| s.terminals.get(&key).is_some_and(|v| v.painted));
    assert!(painted, "attached");
    let size = |rt: &mut Runtime<Lemur>| {
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        rt.state.terminals.get(&key).and_then(|v| v.grid.as_ref()).map(|g| (g.size().cols, g.size().lines)).unwrap()
    };
    let (cols, rows) = size(&mut rt);
    press(&mut rt, K::Command('='), cmd());
    press(&mut rt, K::Command('='), cmd());
    let (zoomed_cols, zoomed_rows) = size(&mut rt);
    assert!(zoomed_cols < cols && zoomed_rows < rows, "{cols}x{rows} -> {zoomed_cols}x{zoomed_rows}");
}
