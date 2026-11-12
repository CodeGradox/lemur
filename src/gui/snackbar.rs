//! Snackbars: messages in a dark pill at the bottom of the terminal, each with a × to close
//! it.

use super::icons;
use rune::{El, col, icon, row, text};
use rune_kit::{IconButton, IconButtonVariant, Kit, Spinner};
use std::time::Duration;

/// What a message is, which decides how long it stays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// Work under way; it goes when the work is done.
    Progress,
    /// Closes itself after `NOTE_TIME`.
    Note,
    /// Stays until closed.
    Warning,
    /// Stays until closed.
    Error,
}

/// How long a note shows.
pub const NOTE_TIME: Duration = Duration::from_secs(5);

/// Stacks `snacks` at the bottom centre of the area it's put in.
pub fn stack(snacks: Vec<El>) -> El {
    col().key("snackbars").pin(Some(0.0), None, Some(0.0), Some(16.0)).px(16.0).gap(8.0).items_center().children(snacks)
}

/// One message; a click on its × calls `close`. Long ones wrap.
pub fn snackbar<S: 'static>(id: &str, message: &str, kind: Kind, close: impl Fn(&mut S) + 'static, k: &Kit) -> El {
    let c = &k.c;
    let lead = match kind {
        Kind::Progress => Spinner::new().build(k),
        Kind::Note => icon(&icons::CIRCLE_CHECK, 14.0).color(c.success.icon),
        Kind::Warning => icon(&icons::TRIANGLE_ALERT, 14.0).color(c.warning.icon),
        Kind::Error => icon(&icons::CIRCLE_ALERT, 14.0).color(c.danger.icon),
    };
    let close = IconButton::new(&icons::X).variant(IconButtonVariant::GhostMuted).label("Close").on_press(close).build(k);
    row()
        .key(format!("snack-{id}"))
        .max_w(640.0)
        .min_h(36.0)
        .pl(12.0)
        .pr(4.0)
        .py(4.0)
        .gap(8.0)
        .center_y()
        .radius(k.radius.lg)
        .fill(c.layer_02)
        .border(1.0, c.border_base)
        .shadow(4.0, 16.0, c.scrim)
        .child(lead)
        .child(text(message.to_owned()).key(format!("snack-text-{id}")).font(k.ty.ui).color(c.text_base).wrap().max_w(560.0))
        .child(close)
}
