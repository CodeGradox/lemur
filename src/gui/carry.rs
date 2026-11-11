//! Files dragged out of Lemur to other apps (Finder, Mail, Slack): a path a terminal printed,
//! held with ⌘, or a changed file in the Changes panel. A drag needs its file on this Mac as
//! it starts, so a file on another host is copied here while the pointer is over it, and
//! drags once it's here.

use super::Lemur;
use crate::actions;
use crate::config::Host;
use rune::{Color, DragItem};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// A file to drag: the host it's on, the session that printed its path (which a relative
/// path is from), if it was printed, and the path.
pub type Carried = (String, Option<String>, String);

/// How long a copy from another host stands for the file there: past that, the pointer
/// coming back copies it again.
const FRESH: Duration = Duration::from_secs(5);

/// A file to drag, as far as it's got here.
#[derive(Default)]
pub struct Carry {
    /// It on this Mac, and when it was copied.
    file: Option<(PathBuf, Instant)>,
    /// It's being copied.
    fetching: bool,
}

impl Lemur {
    /// Gets `path` on `host` ready to drag, as the pointer comes over it: a file on this Mac
    /// once it's found, one on another host once it's copied, again when the last copy is
    /// older than `FRESH`.
    pub(super) fn ready_to_drag(&mut self, host: &Host, session: Option<String>, path: String) {
        let key: Carried = (host.name.clone(), session.clone(), path.clone());
        let carry = self.carried.entry(key.clone()).or_default();
        if carry.fetching || carry.file.as_ref().is_some_and(|(_, at)| at.elapsed() < FRESH) {
            return;
        }
        carry.fetching = true;
        let host = host.clone();
        self.in_background(
            move || actions::file_here(&host, session.as_deref(), &path),
            move |s, got| {
                let carry = s.carried.entry(key).or_default();
                carry.fetching = false;
                // One that couldn't be had is tried again when the pointer comes back.
                if let Ok(file) = got {
                    carry.file = Some((file, Instant::now()));
                }
            },
        );
    }

    /// What dragging `key`'s file carries, on `backing` (the colour under what's dragged),
    /// once the file is here; nothing yet, and the drag doesn't start, before.
    pub(super) fn drag_item(&self, key: &Carried, backing: Color) -> Option<DragItem> {
        let (file, _) = self.carried.get(key)?.file.as_ref()?;
        Some(DragItem::new().file(file).opaque(backing))
    }
}
