//! Silencing a thread: it makes no more notifications, while its dot and the
//! agents panel go on saying what it does. herdr-core keeps it as a label on the
//! terminal (`lemur.silent`), so it stays silent across restarts, and from any Mac.

use super::{Key, Lemur};
use crate::actions;

/// What offers to silence a thread, or to let it notify again.
pub(super) fn silence_label(silent: bool) -> &'static str {
    if silent {
        "Allow notifications"
    } else {
        "Silence notifications"
    }
}

impl Lemur {
    /// Silences thread `k`, taking down a popup it already made, or lets it
    /// notify again. Shown at once, until the next refresh says what herdr-core
    /// keeps.
    pub(super) fn set_silent(&mut self, k: Key, silent: bool) {
        if let Some(s) = self.session_mut(&k) {
            s.silent = silent;
        }
        if silent {
            self.close_popup(&k);
        }
        self.write_core(move |cfg| actions::set_silent(cfg, &k.0, &k.1, silent));
    }
}
