//! A terminal's scroll bar, over its right edge while it has scrollback: a thumb to drag through
//! it, and a track a click pages along or jumps on, as macOS's "Click in the scroll bar to"
//! says (Option-click does the other). It follows "Show scroll bars" as Rune's own bars do:
//! always there, or overlaid while scrolled back, dragged, or with the pointer at the edge.
//! Its sizes are rune-kit's scroll bar's. A program on the alternate screen (Claude Code
//! fullscreen, an editor, a pager) keeps no scrollback, so it has none.

use super::terminal::TermView;
use super::{Key, Lemur};
use rune::{Color, Drag, El, Phase, Tx, col, el, row};

/// The strip along the right edge that takes presses.
const TRACK: f32 = 14.0;
/// The thumb's gap from the right edge.
const EDGE: f32 = 2.0;
/// The track's gap from the top and bottom.
const INSET: f32 = 2.0;
const THUMB: f32 = 6.0;
/// The thumb under the pointer, or dragged.
const THUMB_HOVER: f32 = 10.0;
/// The shortest the thumb gets, however long the scrollback.
const MIN_THUMB: f32 = 24.0;

/// Where the thumb starts on a track `track` points long, and how long it is, for `total` rows
/// of which `len` show from row `offset`.
pub fn thumb(total: usize, offset: usize, len: usize, track: f32) -> (f32, f32) {
    if total <= len || track <= 0.0 {
        return (0.0, track.max(0.0));
    }
    let long = (track * len as f32 / total as f32).max(MIN_THUMB).min(track);
    let back = (total - len) as f32;
    (((track - long) * offset.min(total - len) as f32 / back).max(0.0), long)
}

/// The row a thumb dragged `dy` points from where row `from` put it brings to the top.
fn dragged(total: usize, len: usize, from: usize, dy: f32, track: f32) -> usize {
    let (_, long) = thumb(total, from, len, track);
    let room = track - long;
    if room <= 0.0 {
        return from;
    }
    let back = (total - len) as f32;
    (from as f32 + dy * back / room).round().clamp(0.0, back) as usize
}

/// `view`'s scroll bar, drawn in `fg` (its text's colour, so it shows on any palette), or none
/// while it has no scrollback.
pub fn bar(view: &TermView, fg: Color) -> Option<El> {
    let (total, offset, len) = view.grid.as_ref()?.scrollbar()?;
    if total <= len {
        return None;
    }
    let held = view.bar_drag.is_some();
    let lit = view.bar_hover || held;
    let overlay = rune::system_scroll_bars().is_overlay(rune::system_scroller_overlay());
    let shown = !overlay || offset + len < total || lit;
    let width = if lit { THUMB_HOVER } else { THUMB };
    // Laid out by its share: the rows above, those shown, those below. The thumb keeps its
    // least length and the rest stays in proportion, as `thumb` works it out.
    let pill = el().w(width).h_full().radius(width / 2.0).fill(fg.alpha(if lit { 0.5 } else { 0.28 }));
    // Only the thumb fades: the strip takes the pointer while it's unseen too, to show it.
    let thumb = row()
        .key("term-bar-thumb")
        .w_full()
        .grow(len as f32)
        .min_h(MIN_THUMB)
        .opacity(if shown { 1.0 } else { 0.0 })
        .child(el().grow(1.0))
        .child(pill);
    let key: Key = (view.host.name.clone(), view.session.clone());
    let (hovered, dragged) = (key.clone(), key);
    Some(
        col()
            .key("term-bar")
            .pin(None, Some(INSET), Some(0.0), Some(INSET))
            .w(TRACK)
            .pr(EDGE)
            .cursor(rune::Cursor::Default)
            .child(el().grow(offset as f32))
            .child(thumb)
            .child(el().grow((total - len - offset) as f32))
            .on_hover_change(move |s: &mut Lemur, inside, _: &mut Tx| {
                if let Some(v) = s.terminals.get_mut(&hovered) {
                    v.bar_hover = inside;
                }
            })
            .on_drag(move |s: &mut Lemur, d: &mut Drag, tx: &mut Tx| s.bar_drag(&dragged, d, tx)),
    )
}

impl Lemur {
    /// The scroll bar of `key`'s terminal pressed or dragged: the thumb goes where it's
    /// dragged; the track above or below it pages that way, or jumps there.
    fn bar_drag(&mut self, key: &Key, d: &Drag, tx: &mut Tx) {
        let Some(view) = self.terminals.get_mut(key) else { return };
        let Some((total, offset, len)) = view.grid.as_ref().and_then(|g| g.scrollbar()) else { return };
        let row = match d.phase {
            Phase::Down => {
                let (top, long) = thumb(total, offset, len, d.h);
                let y = d.start_y;
                if (top..=top + long).contains(&y) {
                    view.bar_drag = Some(offset);
                    return;
                }
                if rune::system_scroller_jumps() != tx.mods().alt {
                    // The thumb's middle to the press, and dragged on from there.
                    let to = dragged(total, len, 0, y - long / 2.0, d.h);
                    view.bar_drag = Some(to);
                    to
                } else if y < top {
                    offset.saturating_sub(len)
                } else {
                    (offset + len).min(total - len)
                }
            }
            Phase::Move => {
                // Moves are measured from the press, and the row it left the thumb at.
                let Some(from) = view.bar_drag else { return };
                let at = dragged(total, len, from, d.dy, d.h);
                if at == offset {
                    return;
                }
                at
            }
            _ => {
                view.bar_drag = None;
                return;
            }
        };
        if let Some(g) = view.grid.as_mut() {
            g.scroll_to_row(row);
            view.drawn_at = None;
        }
    }
}
