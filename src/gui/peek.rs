//! Peeking at a thread, as Herdr peeks at a sub-agent: its terminal over
//! the one on show, live, without switching to it. ↗ shows it instead;
//! × or a click outside puts the one on show back.

use super::sidebar::thread_icon;
use super::{Key, Lemur, worktree_label};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, IconName, Sizable, h_flex, v_flex};
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
        if let Some(view) = self.peek.take().and_then(|k| self.terminals.get(&k)) {
            view.update(cx, |v, _| v.hidden());
        }
        self.focus_terminal(window, cx);
        cx.notify();
    }

    /// The thread peeked at, over the window: who it is and how it's doing
    /// over its terminal.
    pub(super) fn peek_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let key = self.peek.clone()?;
        let view = self.terminals.get(&key)?.clone();
        let theme = cx.theme().clone();
        let session = self.session_of(&key).cloned();
        let label = session
            .as_ref()
            .map_or_else(|| self.label_of(&key.0, &key.1), |s| s.label.clone());
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
            .child(
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
