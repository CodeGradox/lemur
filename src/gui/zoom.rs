//! ⌘+, ⌘− and ⌘0: the whole window larger or smaller, as a browser's page zoom, the
//! terminals' text with it. The config keeps it.

use super::Lemur;
use crate::config::Config;

/// The zooms ⌘+ and ⌘− step through, as Chrome's do.
pub const ZOOMS: [f32; 11] = [0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

impl Lemur {
    /// ⌘+, or with `out` ⌘−: the next zoom up or down from the one there is.
    pub(super) fn step_zoom(&mut self, out: bool) {
        let now = self.prefs.zoom;
        let next = if out { ZOOMS.iter().rev().find(|z| **z < now - 0.001) } else { ZOOMS.iter().find(|z| **z > now + 0.001) };
        self.set_zoom(next.copied().unwrap_or(now));
    }

    /// Zooms the window to `zoom`, 1 as it is, says so for a moment, and keeps it.
    pub(super) fn set_zoom(&mut self, zoom: f32) {
        self.note(format!("Zoom {:.0}%", zoom * 100.0));
        if zoom == self.prefs.zoom {
            return;
        }
        self.prefs.zoom = zoom;
        if let Some(Err(e)) = self.prefs.config_file().map(|file| Config::save_zoom(file, zoom)) {
            self.fail(e);
        }
    }
}

#[cfg(test)]
#[path = "zoom_tests.rs"]
mod tests;
