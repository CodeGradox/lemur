//! Peeking at a thread, as Herdr peeks at a sub-agent: its terminal over
//! the one on show, live, without switching to it. ↗ shows it instead;
//! × or a click outside puts the one on show back. Setting up a host shows
//! its terminal the same way, and the Changes panel a file's diff, nearly
//! as wide and tall as the window.

use super::changes_panel::diff_list;
use super::sidebar::thread_icon;
use super::{Key, Lemur, worktree_label};
use crate::actions;
use crate::config::Host;
use crate::hc;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

/// The diff shown over the window: the Changes panel's, which it follows.
/// It has the keyboard, so that nothing typed goes to the terminal under it.
pub(super) struct DiffPreview {
    focus: FocusHandle,
    pub(super) scroll: UniformListScrollHandle,
}

/// `card` over the window, which a click outside it closes.
fn over_window(card: Div, close: impl Fn(&mut Window, &mut App) + 'static) -> Div {
    div()
        .absolute()
        .inset_0()
        .occlude()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui_kit::black().opacity(0.25))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| close(window, cx))
        .child(card.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()))
}

impl Lemur {
    /// Shows the Changes panel's diff over the window.
    pub(super) fn open_diff_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        self.diff_preview = Some(DiffPreview {
            focus,
            scroll: UniformListScrollHandle::new(),
        });
        cx.notify();
    }

    pub(super) fn close_diff_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.diff_preview = None;
        self.focus_terminal(window, cx);
    }

    /// The diff over the window: which file, and its lines.
    pub(super) fn diff_preview_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let preview = self.diff_preview.as_ref()?;
        let (file, lines) = self.changes_panel.read(cx).shown_diff()?;
        let (path, added, removed) = (file.path.clone(), file.added, file.removed);
        let list = diff_list("changes-diff-preview", lines.to_vec(), &preview.scroll, cx);
        let theme = cx.theme().clone();
        let header = h_flex()
            .h_10()
            .flex_shrink_0()
            .px_3()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .child(div().flex_1().min_w_0().truncate().text_sm().child(path))
            .children(added.map(|n| {
                div()
                    .text_xs()
                    .text_color(theme.success)
                    .child(format!("+{n}"))
            }))
            .children(removed.map(|n| {
                div()
                    .text_xs()
                    .text_color(theme.danger)
                    .child(format!("-{n}"))
            }))
            .child(
                div().debug_selector(|| "diff-preview-close".into()).child(
                    Button::new("diff-preview-close")
                        .icon(IconName::Close)
                        .ghost()
                        .xsmall()
                        .tooltip("Close")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.close_diff_preview(window, cx)),
                        ),
                ),
            );
        let card = v_flex()
            .debug_selector(|| "diff-preview".into())
            .track_focus(&preview.focus)
            .size_full()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .shadow_lg()
            .overflow_hidden()
            .child(header)
            .child(list.flex_1().min_h_0());
        let this = cx.entity().downgrade();
        Some(
            over_window(card, move |window, cx| {
                this.update(cx, |this, cx| this.close_diff_preview(window, cx))
                    .ok();
            })
            .p_4()
            .into_any_element(),
        )
    }

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
        // A double click, or one while it starts.
        if !self.setting_up.insert(host.name.clone()) {
            return;
        }
        let asked = host.clone();
        self.in_background(
            cx,
            move || actions::set_up_host(&asked),
            move |this, started, window, cx| {
                this.setting_up.remove(&host.name);
                match started {
                    Ok((address, here)) => {
                        // It takes the card; another set-up on it ends.
                        if this.peek.is_some() {
                            this.close_peek(window, cx);
                        }
                        let key = (here.name.clone(), address);
                        this.setups.insert(key.clone(), host);
                        let view = this.terminal_for(&key, here, cx);
                        if !this.overlaid() {
                            view.read(cx).focus_handle(cx).focus(window, cx);
                        }
                        this.peek = Some(key);
                        cx.notify();
                    }
                    Err(e) => this.fail(e.context(format!("setting up {}", host.name)), cx),
                }
            },
        )
        .detach();
    }

    /// Lemur is quitting: nothing would show a set-up still asking, and being
    /// kept it would stay, so it ends.
    pub(super) fn end_setups(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let setups: Vec<(Key, Host)> = self.setups.drain().collect();
        cx.background_spawn(async move {
            for ((_, address), host) in setups {
                if let Ok(core) = hc::core(hc::machines_session(&host).as_deref()) {
                    let _ = core.kill(&address);
                }
            }
        })
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
            over_window(card, move |window, cx| {
                this.update(cx, |this, cx| this.close_peek(window, cx)).ok();
            })
            .into_any_element(),
        )
    }
}
