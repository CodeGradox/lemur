//! A popup at the top-right of the screen when a session finishes or needs
//! input while you can't see it, modeled on Zed's agent notifications.
//!
//! The popups are one of the app's edge overlays (`App::overlay`): a panel at the top-right
//! of the main screen, above every app and on every Space, that takes clicks but never the
//! keyboard and never makes Lemur the active app, as the popups' small windows of their own
//! did in the GPUI app. An overlay inside the window would show only while the window does,
//! when they matter least. The window keeps which popups show; the overlay draws a copy, and
//! its buttons go back to the window through its `AppHandle`.

use super::{Key, Lemur, icons};
use crate::snapshot::Activity;
use rune::{Align, AppHandle, Cx, El, OverlayHandle, OverlaySpec, ScreenEdge, Tx, col, icon, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit};
use std::sync::OnceLock;
use std::time::Duration;

/// A popup's size, and the room between two.
const WIDTH: f32 = 420.0;
const HEIGHT: f32 = 72.0;
const GAP: f32 = 8.0;

/// The overlay the popups show in; none in tests, which drive the window without an app.
static OVERLAY: OnceLock<OverlayHandle<Popups>> = OnceLock::new();

/// One thread's popup.
#[derive(Clone, Debug)]
pub struct Popup {
    key: Key,
    title: String,
    caption: String,
    /// It needs input, rather than having finished.
    asks: bool,
    /// When it first showed, on the window's clock, once the poll has seen it.
    since: Option<f64>,
}

pub struct Notify {
    /// The popups that show, the oldest at the top.
    popups: Vec<Popup>,
    /// The window is the active one.
    looking: bool,
}

impl Default for Notify {
    fn default() -> Self {
        // Lemur opens in front.
        Notify { popups: vec![], looking: true }
    }
}

impl Notify {
    /// The threads whose popup shows.
    pub fn shown(&self) -> Vec<Key> {
        self.popups.iter().map(|p| p.key.clone()).collect()
    }

    /// Hands the overlay the popups as they are now, in the app's light or `dark`.
    fn sync(&self, dark: bool) {
        let Some(overlay) = OVERLAY.get() else { return };
        let shown = self.popups.clone();
        let count = shown.len();
        overlay.update(move |p, _| {
            p.shown = shown;
            p.dark = dark;
        });
        if count == 0 {
            overlay.hide();
        } else {
            overlay.resize((WIDTH, count as f32 * (HEIGHT + GAP) - GAP));
            overlay.show();
        }
    }
}

impl Lemur {
    /// Pops up a notification for each session that finished or needs input, except the
    /// one you're looking at in the active window.
    pub(super) fn notify(&mut self, news: Vec<(Key, Activity, Option<String>)>) {
        // Decided once: a popup showing mustn't change the answer.
        let looking = self.panels.notify.looking;
        let mut popped = false;
        for (key, activity, waiting_for) in news {
            if looking && self.active.as_ref() == Some(&key) {
                continue;
            }
            let host = &key.0;
            let (place, session) = self.titles(host, &key.1);
            let (title, caption, asks) = match activity {
                Activity::NeedsInput => (format!("{session} needs input"), waiting_for.unwrap_or_else(|| host.clone()), true),
                _ => (format!("{session} finished"), host.clone(), false),
            };
            let caption = match place {
                Some(place) => format!("{place}  ·  {caption}"),
                None => caption,
            };
            // A newer event for the same session replaces its popup.
            let popups = &mut self.panels.notify.popups;
            popups.retain(|p| p.key != key);
            popups.push(Popup { key, title, caption, asks, since: None });
            popped = true;
        }
        if popped {
            self.panels.notify.sync(self.dark);
            ask_attention();
        }
    }

    /// Takes down the popups that have shown as long as Settings say, `now` on the window's
    /// clock, and wakes the window for the next to go. With no time set they stay till
    /// they're answered. True if one went.
    pub(super) fn expire_popups(&mut self, now: f64, tx: &mut Tx) -> bool {
        let popups = &mut self.panels.notify.popups;
        for p in popups.iter_mut() {
            p.since.get_or_insert(now);
        }
        let ms = self.prefs.notification_dismiss_ms;
        if ms == 0 {
            return false;
        }
        let ends = |p: &Popup| p.since.unwrap_or(now) + f64::from(ms) / 1000.0;
        let before = popups.len();
        popups.retain(|p| ends(p) > now);
        if let Some(next) = popups.iter().map(ends).reduce(f64::min) {
            tx.wake_after(Duration::from_secs_f64(next - now));
        }
        if popups.len() == before {
            return false;
        }
        self.panels.notify.sync(self.dark);
        true
    }

    pub(super) fn close_popup(&mut self, key: &Key) {
        let popups = &mut self.panels.notify.popups;
        let before = popups.len();
        popups.retain(|p| p.key != *key);
        if popups.len() != before {
            self.panels.notify.sync(self.dark);
        }
    }

    /// A popup's Show: its thread comes on show, which closes it. The overlay brings Lemur
    /// forward.
    pub(super) fn show_popup(&mut self, key: Key) {
        self.close_popup(&key);
        self.open(key.0, key.1);
    }

    /// The window became the active one, or stopped being it. Back in it, the shown
    /// session's popup has had its say, as in Zed; the others wait.
    pub(super) fn window_focused(&mut self, focused: bool) {
        self.panels.notify.looking = focused;
        if focused && let Some(key) = self.active.clone() {
            self.close_popup(&key);
        }
    }
}

/// Bounces Lemur's Dock icon once, as a window asking for attention does, unless Lemur is
/// the active app.
fn ask_attention() {
    if OVERLAY.get().is_some()
        && rune::tooling::headless_run().is_none()
        && let Some(main) = objc2::MainThreadMarker::new()
    {
        let app = objc2_app_kit::NSApplication::sharedApplication(main);
        app.requestUserAttention(objc2_app_kit::NSRequestUserAttentionType::InformationalRequest);
    }
}

/// What the overlay draws: a copy of the window's popups.
pub struct Popups {
    shown: Vec<Popup>,
    dark: bool,
    /// The window, which Show and Dismiss go back to.
    app: Option<AppHandle<Lemur>>,
}

impl Popups {
    /// Show (or with `show` false, Dismiss) on `key`'s popup: it goes at once, and the window
    /// is told.
    fn answer(&mut self, key: Key, show: bool) {
        self.shown.retain(|p| p.key != key);
        let Some(app) = &self.app else { return };
        if show {
            app.update(move |s, _| s.show_popup(key));
            app.open();
        } else {
            app.update(move |s, _| s.close_popup(&key));
        }
    }
}

/// Adds the popups' overlay to the app, hidden until a thread has news. The app's own handle
/// is how the popups' buttons reach the window.
pub fn add_overlay(app: &mut rune::App<Lemur>) {
    let popups = Popups { shown: vec![], dark: true, app: Some(app.handle()) };
    let spec = OverlaySpec {
        name: "Lemur notifications",
        size: (WIDTH, HEIGHT),
        edge: ScreenEdge::Top,
        // Below the menu bar, 16 points in from the screen's right edge.
        margin: 48.0,
        align: Align::End,
        inset: 16.0,
        interactive: true,
        view: popups_view,
    };
    let _ = OVERLAY.set(app.overlay(spec, popups));
}

fn popups_view(p: &Popups, _: &mut Cx<Popups>) -> El {
    let k = Kit::neutral(p.dark);
    col().key("popups").w_full().h_full().gap(GAP).children(p.shown.iter().map(|popup| card(popup, &k)))
}

/// A popup: its icon, title and caption, with Show over Dismiss.
fn card(p: &Popup, k: &Kit) -> El {
    let c = &k.c;
    let id = format!("{}-{}", p.key.0, p.key.1);
    let (glyph, ink) = if p.asks { (&icons::CIRCLE_ALERT, c.warning.icon) } else { (&icons::CIRCLE_CHECK, c.success.icon) };
    let (show, dismiss) = (p.key.clone(), p.key.clone());
    let words = col()
        .grow(1.0)
        .min_w(0.0)
        .gap(2.0)
        .child(text(p.title.clone()).key(format!("popup-title-{id}")).font(k.ty.ui_medium).color(c.text_base).truncate())
        .child(text(p.caption.clone()).font(k.ty.small).color(c.text_muted).truncate());
    let buttons = col()
        .gap(4.0)
        .child(
            Button::new("Show")
                .variant(ButtonVariant::Contrast)
                .control_size(ControlSize::Mini)
                .on_press(move |s: &mut Popups| s.answer(show.clone(), true))
                .build(k)
                .key(format!("popup-show-{id}")),
        )
        .child(
            Button::new("Dismiss")
                .variant(ButtonVariant::Outline)
                .control_size(ControlSize::Mini)
                .on_press(move |s: &mut Popups| s.answer(dismiss.clone(), false))
                .build(k)
                .key(format!("popup-dismiss-{id}")),
        );
    row()
        .key(format!("popup-{id}"))
        .w_full()
        .h(HEIGHT)
        .padding(12.0)
        .gap(16.0)
        .center_y()
        .radius(k.radius.xl)
        .fill(c.surface_raised)
        .border(1.0, c.border_base)
        .child(row().grow(1.0).min_w(0.0).gap(8.0).items_start().child(icon(glyph, 14.0).color(ink).mt(2.0)).child(words))
        .child(buttons)
}

#[cfg(test)]
#[path = "notify_tests.rs"]
mod tests;
