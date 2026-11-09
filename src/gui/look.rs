//! Lemur's colours: the ones it had on GPUI, gpui-component's default theme (shadcn's
//! neutral greys), laid onto rune-kit's neutral kit in place of its blue-grey ones.

use rune::{Color, Shadow};
use rune_kit::Kit;

/// The greys one appearance uses, by what they're for.
struct Greys {
    /// The window around the panes: the tabs.
    window: &'static str,
    sidebar: &'static str,
    /// Panes and cards.
    card: &'static str,
    text: &'static str,
    muted: &'static str,
    border: &'static str,
    /// Under the pointer.
    hover: &'static str,
    /// What's picked or pressed: the thread on show, the tab on show.
    picked: &'static str,
    /// Tracks and wells: a bar's, a field's.
    track: &'static str,
    /// A switch that's off.
    off: &'static str,
    indicator: &'static str,
    /// The status bar.
    status: &'static str,
}

const LIGHT: Greys = Greys {
    window: "#ffffff",
    sidebar: "#fafafa",
    card: "#ffffff",
    text: "#0a0a0a",
    muted: "#737373",
    border: "#e5e5e5",
    hover: "#f5f5f5",
    picked: "#e5e5e5",
    track: "#f5f5f5",
    off: "#d4d4d4",
    indicator: "#a3a3a3",
    status: "#f8f8f8",
};

const DARK: Greys = Greys {
    window: "#0a0a0a",
    sidebar: "#0a0a0a",
    card: "#0a0a0a",
    text: "#fafafa",
    muted: "#a3a3a3",
    border: "#262626",
    hover: "#262626",
    picked: "#262626",
    track: "#262626",
    off: "#404040",
    indicator: "#525252",
    status: "#171717",
};

fn color(hex: &str) -> Color {
    rune::color::hex(hex).expect("a colour")
}

/// The kit in `dark` or light, in Lemur's colours.
pub fn kit(dark: bool) -> Kit {
    let g = if dark { &DARK } else { &LIGHT };
    let (window, card, text, muted, border) = (color(g.window), color(g.card), color(g.text), color(g.muted), color(g.border));
    let (hover, picked, track) = (color(g.hover), color(g.picked), color(g.track));
    let mut k = Kit::neutral(dark);
    let c = &mut k.c;
    // As rune-kit's neutral kit lays its own greys out.
    c.page = window;
    c.bg_deep = window;
    c.layer_01 = window;
    c.bg_base = card;
    c.layer_02 = track;
    c.layer_03 = color(g.off);
    c.layer_04 = color(g.indicator);
    c.bg_inverse = text;
    c.bg_contrast = text;
    c.bg_button_neutral = card;
    c.bg_icon_button_contrast = text;
    c.submit_fg = card;
    c.submit_icon = card;
    c.text_base = text;
    c.text_strong = text;
    c.std_text = text;
    c.text_muted = muted;
    c.text_weak = muted;
    c.text_faint = muted.alpha(0.8);
    c.text_inverse = card;
    c.text_contrast = card;
    c.text_invert_strong = card;
    c.icon_base = text;
    c.std_icon_strong = text;
    c.icon_muted = muted;
    c.icon_weaker = muted;
    c.std_icon = muted;
    c.icon_faint = muted.alpha(0.7);
    c.icon_inverse = card;
    c.icon_contrast = card;
    c.std_icon_invert = card;
    c.border_muted = border;
    c.border_base = border;
    c.border_strong = border;
    c.border_weak = border;
    c.border_hover = border;
    c.border_disabled = border;
    c.std_border = border;
    c.hover = hover;
    c.surface_hover = hover;
    c.surface_raised_hover = hover;
    c.pressed = picked;
    c.surface_base = track;
    c.surface_weak = track;
    c.surface_disabled = track;
    c.surface_raised = card;
    c.surface_raised_base = card;
    c.surface_float = text;
    // The hairline around raised things: a ring, a shadow spread and not offset.
    let e = &mut k.elevation;
    for shadows in [&mut e.raised, &mut e.floating, &mut e.overlay, &mut e.button_neutral] {
        for s in shadows.iter_mut().filter(|s: &&mut Shadow| s.spread > 0.0 && s.y == 0.0) {
            s.color = border;
        }
    }
    k
}

/// The sidebar's colour, in `dark` or light.
pub fn sidebar(dark: bool) -> Color {
    color(if dark { DARK.sidebar } else { LIGHT.sidebar })
}

/// The status bar's colour, in `dark` or light.
pub fn status_bar(dark: bool) -> Color {
    color(if dark { DARK.status } else { LIGHT.status })
}
