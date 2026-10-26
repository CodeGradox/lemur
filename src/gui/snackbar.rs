//! Snackbars: messages in a dark pill at the bottom of the terminal, each
//! with a × to close it.

use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
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

/// Stacks `snacks` at the bottom center of the area it's put in; the
/// area around them still takes clicks.
pub fn stack(snacks: Vec<AnyElement>) -> Div {
    v_flex()
        .absolute()
        .left_0()
        .right_0()
        .bottom_4()
        .px_4()
        .gap_2()
        .items_center()
        .children(snacks)
}

/// One message; a click on its × calls `close`. Long ones wrap.
pub fn snackbar(
    id: &'static str,
    text: impl Into<SharedString>,
    kind: Kind,
    close: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    // Against the theme, to stand out: LAFT's dark pill in the light
    // theme, its white one in the dark.
    let dark = cx.theme().is_dark();
    let (bg, fg, hover) = if dark {
        (rgb(0xffffff), rgb(0x1f2937), black().opacity(0.08))
    } else {
        (rgb(0x3d4451), rgb(0xe5e6e6), white().opacity(0.12))
    };
    let icon = match kind {
        Kind::Warning => Some((
            IconName::TriangleAlert,
            if dark { 0xf29349 } else { 0xffbd70 },
        )),
        Kind::Error => Some((IconName::CircleX, if dark { 0xcc6152 } else { 0xe88f82 })),
        Kind::Progress | Kind::Note => None,
    };
    h_flex()
        .id(id)
        .occlude()
        .max_w(px(560.0))
        .items_start()
        .gap_2()
        .py(px(6.0))
        .pr(px(6.0))
        .pl_4()
        .rounded(px(20.0))
        .bg(bg)
        .text_color(fg)
        .text_sm()
        .shadow_lg()
        .when_some(icon, |this, (icon, color)| {
            this.child(
                Icon::new(icon)
                    .small()
                    .text_color(rgb(color))
                    .flex_shrink_0()
                    .mt(px(6.0)),
            )
        })
        // As tall as the × on one line, so a short message sits centered.
        .child(div().min_w_0().py(px(4.0)).child(text.into()))
        .child(
            div()
                .id("close")
                .debug_selector(move || format!("close {id}"))
                .flex_shrink_0()
                .size(px(28.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .hover(move |this| this.bg(hover))
                .on_click(move |_, window, cx| close(window, cx))
                .child(Icon::new(IconName::Close).small()),
        )
        .into_any_element()
}
