//! A macOS notification when a session finishes or needs input while you can't see it: one
//! a thread, so newer news replaces its last. A click opens the thread. They follow Focus and
//! Do Not Disturb, and System Settings › Notifications › Lemur says whether they show as
//! banners or alerts.
//!
//! The window keeps which notifications are out; Notification Center shows them. Only the
//! app bundle can post (`rune_notify::available`), and the first post asks for permission.

use super::{Key, Lemur};
use crate::snapshot::Activity;
use rune::Tx;
use rune_mac::notify::{self as center, Notification};
use std::sync::OnceLock;
use std::time::Duration;

/// Set once the app has started notifying: none in tests or headless runs, which have no
/// Notification Center to post to.
static LIVE: OnceLock<()> = OnceLock::new();

/// One thread's notification.
#[derive(Clone, Debug)]
pub struct Popup {
    key: Key,
    title: String,
    caption: String,
    /// When it first showed, on the window's clock, once the poll has seen it.
    since: Option<f64>,
}

pub struct Notify {
    /// The notifications out, the oldest first.
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
    /// The threads whose notification is out.
    pub fn shown(&self) -> Vec<Key> {
        self.popups.iter().map(|p| p.key.clone()).collect()
    }
}

/// A thread's notification's id in Notification Center.
fn id(key: &Key) -> String {
    format!("{}/{}", key.0, key.1)
}

/// Posts `p`, in place of the thread's last.
fn post(p: &Popup) {
    if LIVE.get().is_some() {
        let n = Notification::new(id(&p.key), p.title.clone()).body(p.caption.clone()).info("host", p.key.0.clone()).info("session", p.key.1.clone());
        center::post(&n);
    }
}

/// Takes `keys`' notifications out of Notification Center.
fn take_down(keys: &[Key]) {
    if LIVE.get().is_some() && !keys.is_empty() {
        let ids: Vec<String> = keys.iter().map(id).collect();
        center::remove(&ids.iter().map(String::as_str).collect::<Vec<_>>());
    }
}

/// Lets the app notify: a click on a notification, which brings Lemur forward, opens its
/// thread. Headless runs don't.
pub fn start(app: &rune::App<Lemur>) {
    if rune::tooling::is_headless() {
        return;
    }
    let _ = LIVE.set(());
    let handle = app.handle();
    center::on_click(move |click| {
        let (Some(host), Some(session)) = (click.get("host"), click.get("session")) else { return };
        let key = (host.to_string(), session.to_string());
        handle.update(move |s, _| s.show_popup(key));
        handle.open();
    });
}

impl Lemur {
    /// Notifies of each session that finished or needs input, with what it waits for or the
    /// shell's command that finished, except the one you're looking at in the active window.
    pub(super) fn notify(&mut self, news: Vec<(Key, Activity, Option<String>)>) {
        // Decided once: a notification going out mustn't change the answer.
        let looking = self.panels.notify.looking;
        let mut posted = false;
        for (key, activity, said) in news {
            if looking && self.active.as_ref() == Some(&key) {
                continue;
            }
            let host = &key.0;
            let (place, session) = self.titles(host, &key.1);
            let (title, caption) = match (activity, said) {
                (Activity::NeedsInput, waiting_for) => (format!("{session} needs input"), waiting_for.unwrap_or_else(|| host.clone())),
                (_, Some(command)) => (format!("{command} finished in {session}"), host.clone()),
                (_, None) => (format!("{session} finished"), host.clone()),
            };
            let caption = match place {
                Some(place) => format!("{place}  ·  {caption}"),
                None => caption,
            };
            // A newer event for the same session replaces its notification.
            let popups = &mut self.panels.notify.popups;
            popups.retain(|p| p.key != key);
            let popup = Popup { key, title, caption, since: None };
            post(&popup);
            popups.push(popup);
            posted = true;
        }
        if posted {
            ask_attention();
        }
    }

    /// Takes down the notifications that have been out as long as Settings say, `now` on the
    /// window's clock, and wakes the window for the next to go. With no time set they stay
    /// till they're answered. True if one went.
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
        let (kept, gone): (Vec<Popup>, Vec<Popup>) = popups.drain(..).partition(|p| ends(p) > now);
        *popups = kept;
        if let Some(next) = popups.iter().map(ends).reduce(f64::min) {
            tx.wake_after(Duration::from_secs_f64(next - now));
        }
        take_down(&gone.iter().map(|p| p.key.clone()).collect::<Vec<_>>());
        !gone.is_empty()
    }

    pub(super) fn close_popup(&mut self, key: &Key) {
        let popups = &mut self.panels.notify.popups;
        let before = popups.len();
        popups.retain(|p| p.key != *key);
        if popups.len() != before {
            take_down(std::slice::from_ref(key));
        }
    }

    /// A click on a notification: its thread comes on show, which takes it down.
    pub(super) fn show_popup(&mut self, key: Key) {
        self.close_popup(&key);
        self.open(key.0, key.1);
    }

    /// The window became the active one, or stopped being it. Back in it, the shown
    /// session's notification has had its say, as in Zed; the others wait.
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
    if LIVE.get().is_some()
        && let Some(main) = objc2::MainThreadMarker::new()
    {
        let app = objc2_app_kit::NSApplication::sharedApplication(main);
        app.requestUserAttention(objc2_app_kit::NSRequestUserAttentionType::InformationalRequest);
    }
}

#[cfg(test)]
#[path = "notify_tests.rs"]
mod tests;
