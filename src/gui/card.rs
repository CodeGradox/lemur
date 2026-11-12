//! The cards over the window, which share one look: the spotlight (⌘P, ⌘J, renaming), the
//! command palette, the folder picker and the questions Lemur asks. Each is Rune's modal:
//! Escape or a click outside it closes it, and the window under it waits.

use rune::{El, Icon, col, el, icon, row, text};
use rune_kit::Kit;

/// How wide a card is.
pub const WIDTH: f32 = 560.0;
/// How far below the window's top a card sits: anchored there, as Zed's are, it doesn't move
/// as its list grows or shrinks.
const TOP: f32 = 80.0;

/// A card's surface, `width` wide.
pub fn surface(key: &str, width: f32, k: &Kit) -> El {
    let c = &k.c;
    col()
        .key(key)
        .w(width)
        .radius(k.radius.lg)
        .fill(c.surface_raised)
        .border(1.0, c.border_base)
        .shadow(12.0, 40.0, c.scrim)
        .clip_rounded(k.radius.lg)
}

/// Its first line: an icon and what the card is for.
pub fn title(key: &str, glyph: &'static Icon, what: String, k: &Kit) -> El {
    row()
        .key(key)
        .px(12.0)
        .h(34.0)
        .gap(8.0)
        .center_y()
        .child(icon(glyph, 14.0).color(k.c.icon_muted))
        .child(text(what).font(k.ty.ui_medium).color(k.c.text_base).truncate())
}

/// The line holding its field.
pub fn field(input: El) -> El {
    row().px(8.0).py(6.0).child(input)
}

/// A line between its parts.
pub fn divider(k: &Kit) -> El {
    el().w_full().h(1.0).fill(k.c.border_weak)
}

/// A row to pick: `lead`, `label`, then `detail` and `trail`, each if any. The one picked
/// shows in the accent.
pub fn pick_row(key: String, lead: Option<El>, label: String, detail: Option<String>, trail: Option<El>, picked: bool, k: &Kit) -> El {
    let c = &k.c;
    let (ink, faint) = if picked { (c.text_contrast, c.text_contrast) } else { (c.text_base, c.text_faint) };
    let mut r = row()
        .key(key)
        .px(8.0)
        .h(28.0)
        .gap(8.0)
        .center_y()
        .radius(k.radius.sm)
        // The pick moves at once: faded, the highlight lingers on two rows at a time. (The
        // arrow keys' steps land at once whole, their text's colours too: `Tx::instant`.)
        .motion(rune::Spring::NONE)
        .cursor(rune::Cursor::Pointer)
        .children(lead)
        .child(text(label).font(k.ty.ui).color(ink).truncate().shrink());
    if let Some(d) = detail {
        r = r.child(text(d).font(k.ty.small).color(faint).truncate().shrink());
    }
    r = r.child(el().grow(1.0)).children(trail);
    if picked { r.fill(c.bg_accent) } else { k.interactive(r) }
}

/// Its last line: what keys do, or what went wrong.
pub fn footer(key: &str, line: String, error: bool, k: &Kit) -> El {
    let c = &k.c;
    row()
        .key(key)
        .px(12.0)
        .py(6.0)
        .min_h(28.0)
        .center_y()
        .fill(c.layer_01)
        .child(text(line).font(k.ty.small).color(if error { c.danger.fg } else { c.text_faint }).wrap())
}

/// Puts `card` in `layer`, over the dimmed window, `TOP` below its top: Escape or a click
/// outside it closes it with `close`.
pub fn over_window<S: 'static>(key: &str, card: El, close: impl Fn(&mut S) + 'static, k: &Kit, layer: &mut Vec<El>) {
    // The dimming is a layer of its own: the modal leaves what's under it out of the tree.
    layer.push(el().key(format!("over-{key}-scrim")).absolute(0.0, 0.0).w_full().h_full().fill(k.c.scrim).a11y_hidden());
    layer.push(col().key(format!("over-{key}")).absolute(0.0, 0.0).w_full().h_full().items_center().pt(TOP).child(card.modal(true, close)));
}

/// The row after `at` of `count`, or with `back` the one before it, round the ends: for the
/// cards' lists, and the threads ⌃Tab and Next Agent go through.
pub fn step(at: usize, count: usize, back: bool) -> usize {
    match count {
        0 => 0,
        _ if back => (at + count - 1) % count,
        _ => (at + 1) % count,
    }
}
