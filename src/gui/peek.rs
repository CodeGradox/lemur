//! Peeking at a thread, as Herdr peeks at a sub-agent: its terminal over
//! the one on show, live, without switching to it. ↗ shows it instead;
//! × or a click outside puts the one on show back. Setting up a host shows
//! its terminal the same way.

use super::sidebar::thread_icon;
use super::{Key, Lemur, worktree_label};
use crate::actions;
use crate::config::Host;
use crate::hc;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

impl Lemur {
    /// Peeks at thread `key`: a helper's terminal, or another thread.
    pub(super) fn peek(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        if self.active.as_ref() == Some(&key) {
            return;
        }
        let Some(h) = self.cfg.host(&key.0).cloned() else {
            return;
        };
        let view = self.terminal_for(&key, h, cx);
        view.read(cx).focus_handle(cx).focus(window, cx);
        self.peek = Some(key);
        cx.notify();
    }

    pub(super) fn close_peek(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let closed = self.peek.take();
        if let Some(view) = closed.as_ref().and_then(|k| self.terminals.get(k)) {
            view.update(cx, |v, _| v.hidden());
        }
        if let Some(key) = closed {
            self.set_up_done(key, cx);
        }
        self.focus_terminal(window, cx);
        cx.notify();
    }

    /// Opens `hc machine add` for `host` in a terminal over the window, as
    /// Herdr does, where you answer what herdr-core asks to add it: to
    /// install herdr-core there or update it, a password, a new host key.
    pub(super) fn set_up_host(&mut self, host: Host, cx: &mut Context<Self>) {
        let asked = host.clone();
        self.in_background(
            cx,
            move || actions::set_up_host(&asked),
            move |this, started, window, cx| match started {
                Ok((address, here)) => {
                    let key = (here.name.clone(), address);
                    this.setups.insert(key.clone(), host);
                    let view = this.terminal_for(&key, here, cx);
                    view.read(cx).focus_handle(cx).focus(window, cx);
                    this.peek = Some(key);
                    cx.notify();
                }
                Err(e) => this.fail(e.context(format!("setting up {}", host.name)), cx),
            },
        )
        .detach();
    }

    /// Done with setting up a host, if `key` is the terminal that did:
    /// it goes, ended if it still runs, and herdr-core is asked again.
    fn set_up_done(&mut self, key: Key, cx: &mut Context<Self>) {
        let Some(host) = self.setups.remove(&key) else {
            return;
        };
        self.terminals.remove(&key);
        self.in_background(
            cx,
            move || {
                let core = hc::core(hc::machines_session(&host).as_deref())?;
                core.kill(&key.1)?;
                core.forget_machine(&host);
                anyhow::Ok(())
            },
            |this, done, _, cx| {
                if let Err(e) = done {
                    this.fail(e, cx);
                }
                this.refresh(cx).detach();
            },
        )
        .detach();
    }

    /// The thread peeked at, over the window: who it is and how it's doing
    /// over its terminal.
    pub(super) fn peek_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let key = self.peek.clone()?;
        let view = self.terminals.get(&key)?.clone();
        let theme = cx.theme().clone();
        let session = self.session_of(&key).cloned();
        let setting_up = self
            .setups
            .get(&key)
            .map(|h| format!("Setting up {}", h.name));
        let label = setting_up.clone().unwrap_or_else(|| {
            session
                .as_ref()
                .map_or_else(|| self.label_of(&key.0, &key.1), |s| s.label.clone())
        });
        let place = match self.project_of(&key.0, &key.1) {
            Some((base, w)) => format!("{} · {}", self.cfg.bases[base].name(), worktree_label(w)),
            None => key.0.clone(),
        };
        self.dots
            .borrow_mut()
            .retain(|(place, k), _| *place != "peek" || *k == key);
        let dot = session.as_ref().map(|s| {
            let (color, tip) = self.activity(&key, s, cx);
            self.dot_view("peek", &key, color, self.breathes(s.activity), tip, cx)
        });
        let header = h_flex()
            .h_10()
            .flex_shrink_0()
            .px_3()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .children(session.as_ref().map(|s| thread_icon(s).small()))
            .child(div().text_sm().child(label))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(place),
            )
            .children(dot)
            // A host's set-up isn't a thread to show instead.
            .when(setting_up.is_none(), |header| {
                header.child(
                    div().debug_selector(|| "peek-open".into()).child(
                        Button::new("peek-open")
                            .icon(IconName::ExternalLink)
                            .ghost()
                            .xsmall()
                            .tooltip("Show it instead")
                            .on_click(cx.listener({
                                let key = key.clone();
                                move |this, _, window, cx| {
                                    let (host, session) = key.clone();
                                    this.open(host, session, window, cx);
                                }
                            })),
                    ),
                )
            })
            .child(
                Button::new("peek-close")
                    .icon(IconName::Close)
                    .ghost()
                    .xsmall()
                    .tooltip("Close")
                    .on_click(cx.listener(|this, _, window, cx| this.close_peek(window, cx))),
            );
        let card = v_flex()
            .debug_selector(|| "peek".into())
            .w(relative(0.8))
            .h(relative(0.8))
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .shadow_lg()
            .overflow_hidden()
            .child(header)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(view.cached(StyleRefinement::default().size_full())),
            );
        let this = cx.entity().downgrade();
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui_kit::black().opacity(0.25))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    this.update(cx, |this, cx| this.close_peek(window, cx)).ok();
                })
                .child(card.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()))
                .into_any_element(),
        )
    }
}
