//! Rune's key events as Ghostty's: which key, which modifiers, and the text it types. Ghostty's
//! encoder turns them into the bytes the program expects.

use libghostty_vt::key::{Key as GKey, Mods as GMods};
use rune::{Key, KeyEvent};

pub struct Translated {
    pub key: GKey,
    pub mods: GMods,
    /// Modifiers the text already accounts for (Shift in "A").
    pub consumed: GMods,
    /// The character the key types with no modifiers, which the Kitty protocol reports.
    pub unshifted: char,
    pub text: Option<String>,
}

/// The key that types `c` unshifted on a US layout; `Unidentified` for the rest, whose text
/// the encoder sends as it is.
fn key_for(c: char) -> GKey {
    use GKey::*;
    match c.to_ascii_lowercase() {
        'a' => A, 'b' => B, 'c' => C, 'd' => D, 'e' => E, 'f' => F, 'g' => G, 'h' => H, 'i' => I,
        'j' => J, 'k' => K, 'l' => L, 'm' => M, 'n' => N, 'o' => O, 'p' => P, 'q' => Q, 'r' => R,
        's' => S, 't' => T, 'u' => U, 'v' => V, 'w' => W, 'x' => X, 'y' => Y, 'z' => Z,
        '0' => Digit0, '1' => Digit1, '2' => Digit2, '3' => Digit3, '4' => Digit4,
        '5' => Digit5, '6' => Digit6, '7' => Digit7, '8' => Digit8, '9' => Digit9,
        ' ' => Space, '-' => Minus, '=' => Equal, '[' => BracketLeft, ']' => BracketRight,
        '\\' => Backslash, ';' => Semicolon, '\'' => Quote, ',' => Comma, '.' => Period,
        '/' => Slash, '`' => Backquote,
        _ => Unidentified,
    }
}

fn function(n: u8) -> Option<GKey> {
    use GKey::*;
    [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12, F13, F14, F15, F16, F17, F18, F19, F20, F21, F22, F23, F24]
        .get(usize::from(n).checked_sub(1)?)
        .copied()
}

/// `ev` as Ghostty's key event, or None for keys the app keeps (⌘ chords). `base` is the
/// character the key types with no modifiers under the current layout, when the window knows.
pub fn translate(ev: &KeyEvent, base: Option<char>) -> Option<Translated> {
    let m = ev.mods;
    // ⌘ is the app's on macOS: menus and shortcuts, never the program's.
    if m.logo {
        return None;
    }
    let mut mods = GMods::empty();
    for (on, flag) in [(m.shift, GMods::SHIFT), (m.ctrl, GMods::CTRL), (m.alt, GMods::ALT)] {
        if on {
            mods |= flag;
        }
    }
    let named = |key| Some(Translated { key, mods, consumed: GMods::empty(), unshifted: '\0', text: None });
    match &ev.key {
        Key::Text(t) => {
            let c = t.chars().next()?;
            let unshifted = base.unwrap_or(c.to_ascii_lowercase());
            let key = key_for(unshifted);
            // A Control chord types no text of its own: the encoder makes ^C from the key.
            let text = (!m.ctrl).then(|| t.clone());
            let consumed = if m.shift && text.is_some() { GMods::SHIFT } else { GMods::empty() };
            Some(Translated { key, mods, consumed, unshifted, text })
        }
        Key::Enter => named(GKey::Enter),
        Key::Backspace => named(GKey::Backspace),
        Key::Delete => named(GKey::Delete),
        Key::Left => named(GKey::ArrowLeft),
        Key::Right => named(GKey::ArrowRight),
        Key::Up => named(GKey::ArrowUp),
        Key::Down => named(GKey::ArrowDown),
        Key::Home => named(GKey::Home),
        Key::End => named(GKey::End),
        Key::PageUp => named(GKey::PageUp),
        Key::PageDown => named(GKey::PageDown),
        Key::Insert => named(GKey::Insert),
        Key::Escape => named(GKey::Escape),
        Key::Tab => named(GKey::Tab),
        Key::Function(n) => named(function(*n)?),
        _ => None,
    }
}
