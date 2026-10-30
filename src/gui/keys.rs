//! Keyboard and mouse input as the bytes a terminal program expects (xterm).

pub struct Key<'a> {
    /// GPUI's key name: "a", "enter", "up", "f5", ...
    pub key: &'a str,
    /// The character the keystroke types, if any (layout and Option aware).
    pub key_char: Option<&'a str>,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub caps_lock: bool,
}

/// Bytes for a keystroke, or None if it types nothing the terminal knows
/// or only text, which is the input method's to type (see `typed`).
/// Cmd shortcuts never get here: they belong to the app.
pub fn encode(k: &Key, app_cursor: bool) -> Option<Vec<u8>> {
    // xterm modifier parameter: 1 + shift + 2*alt + 4*ctrl.
    let m = 1 + k.shift as u8 + 2 * k.alt as u8 + 4 * k.ctrl as u8;
    let cursor = |end: char| {
        if m > 1 {
            format!("\x1b[1;{m}{end}")
        } else if app_cursor {
            format!("\x1bO{end}")
        } else {
            format!("\x1b[{end}")
        }
        .into_bytes()
    };
    let function = |end: char| {
        if m > 1 {
            format!("\x1b[1;{m}{end}")
        } else {
            format!("\x1bO{end}")
        }
        .into_bytes()
    };
    let tilde = |n: u8| {
        if m > 1 {
            format!("\x1b[{n};{m}~")
        } else {
            format!("\x1b[{n}~")
        }
        .into_bytes()
    };
    let meta = |bytes: &[u8]| {
        if k.alt {
            [b"\x1b", bytes].concat()
        } else {
            bytes.to_vec()
        }
    };
    let bytes = match k.key {
        // Option/Shift+Enter: Claude Code reads ESC CR as a newline in the prompt.
        "enter" if k.shift || k.alt => b"\x1b\r".to_vec(),
        "enter" => b"\r".to_vec(),
        "tab" if k.shift => b"\x1b[Z".to_vec(),
        "tab" => b"\t".to_vec(),
        "backspace" if k.ctrl => meta(b"\x08"),
        "backspace" => meta(b"\x7f"),
        "escape" => b"\x1b".to_vec(),
        "up" => cursor('A'),
        "down" => cursor('B'),
        "right" => cursor('C'),
        "left" => cursor('D'),
        "home" => cursor('H'),
        "end" => cursor('F'),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "f1" => function('P'),
        "f2" => function('Q'),
        "f3" => function('R'),
        "f4" => function('S'),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        "space" if k.ctrl => vec![0],
        key if k.ctrl => meta(&[control_byte(key)?]),
        _ => return None,
    };
    Some(bytes)
}

/// The text a key types by itself, for a held key's repeats: macOS's input
/// method, which types everything else, only sees the first press.
pub fn typed(k: &Key) -> Option<Vec<u8>> {
    if k.ctrl {
        return None;
    }
    let text = k.key_char?;
    // GPUI's key_char ignores Caps Lock, which on a Mac capitalizes
    // letters only, Shift or not. Option on macOS composes characters (on
    // many layouts @, |, [, ]), so that's sent rather than treated as Meta.
    Some(if k.caps_lock && !k.alt {
        text.to_uppercase().into_bytes()
    } else {
        text.as_bytes().to_vec()
    })
}

fn control_byte(key: &str) -> Option<u8> {
    let mut chars = key.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(match c {
        'a'..='z' => c as u8 - b'a' + 1,
        '@' | '2' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '-' | '7' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

#[derive(Clone, Copy)]
pub enum MouseButton {
    Left = 0,
    WheelUp = 64,
    WheelDown = 65,
}

/// An SGR (1006) mouse report; `col` and `line` are 0-based cells.
pub fn sgr_mouse(
    button: MouseButton,
    drag: bool,
    col: usize,
    line: usize,
    pressed: bool,
) -> Vec<u8> {
    let code = button as u8 + if drag { 32 } else { 0 };
    format!(
        "\x1b[<{code};{};{}{}",
        col + 1,
        line + 1,
        if pressed { 'M' } else { 'm' }
    )
    .into_bytes()
}

/// Wraps pasted text in bracketed-paste markers when the program asked for them.
pub fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    // Pasted escape sequences (including a smuggled end marker) would run as
    // keystrokes, so keep only printable text, tabs and newlines.
    let text: String = text
        .chars()
        .filter(|&c| !c.is_control() || matches!(c, '\t' | '\n' | '\r'))
        .collect();
    if bracketed {
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: &'static str, key_char: Option<&'static str>) -> Key<'static> {
        Key {
            key,
            key_char,
            ctrl: false,
            alt: false,
            shift: false,
            caps_lock: false,
        }
    }

    #[test]
    fn text_is_left_to_the_input_method() {
        assert_eq!(encode(&key("a", Some("a")), false), None);
        assert_eq!(encode(&key("space", Some(" ")), false), None);
        assert_eq!(
            encode(
                &Key {
                    alt: true,
                    ..key("7", Some("|"))
                },
                false
            ),
            None
        );
    }

    #[test]
    fn held_keys_type_what_they_type() {
        assert_eq!(typed(&key("a", Some("a"))).unwrap(), b"a");
        assert_eq!(typed(&key("å", Some("å"))).unwrap(), "å".as_bytes());
        // Option+7 on a Norwegian layout types |.
        assert_eq!(
            typed(&Key {
                alt: true,
                ..key("7", Some("|"))
            })
            .unwrap(),
            b"|"
        );
        assert_eq!(
            typed(&Key {
                ctrl: true,
                ..key("c", Some("c"))
            }),
            None
        );
    }

    #[test]
    fn caps_lock_capitalizes_letters() {
        let caps = |key: Key<'static>| {
            let key = Key {
                caps_lock: true,
                ..key
            };
            encode(&key, false).or_else(|| typed(&key)).unwrap()
        };
        assert_eq!(caps(key("a", Some("a"))), b"A");
        assert_eq!(caps(key("å", Some("å"))), "Å".as_bytes());
        assert_eq!(
            caps(Key {
                shift: true,
                ..key("a", Some("A"))
            }),
            b"A"
        );
        assert_eq!(caps(key("1", Some("1"))), b"1");
        assert_eq!(
            caps(Key {
                ctrl: true,
                ..key("c", Some("c"))
            }),
            [3]
        );
    }

    #[test]
    fn control_and_named_keys() {
        assert_eq!(
            encode(
                &Key {
                    ctrl: true,
                    ..key("c", Some("c"))
                },
                false
            )
            .unwrap(),
            [3]
        );
        assert_eq!(
            encode(
                &Key {
                    ctrl: true,
                    ..key("space", Some(" "))
                },
                false
            )
            .unwrap(),
            [0]
        );
        assert_eq!(encode(&key("enter", Some("\n")), false).unwrap(), b"\r");
        assert_eq!(
            encode(
                &Key {
                    shift: true,
                    ..key("enter", Some("\n"))
                },
                false
            )
            .unwrap(),
            b"\x1b\r"
        );
        assert_eq!(encode(&key("backspace", None), false).unwrap(), b"\x7f");
        assert_eq!(
            encode(
                &Key {
                    shift: true,
                    ..key("tab", None)
                },
                false
            )
            .unwrap(),
            b"\x1b[Z"
        );
        assert_eq!(encode(&key("up", None), false).unwrap(), b"\x1b[A");
        assert_eq!(encode(&key("up", None), true).unwrap(), b"\x1bOA");
        assert_eq!(
            encode(
                &Key {
                    alt: true,
                    ..key("left", None)
                },
                false
            )
            .unwrap(),
            b"\x1b[1;3D"
        );
        assert_eq!(encode(&key("pagedown", None), false).unwrap(), b"\x1b[6~");
        assert_eq!(encode(&key("f5", None), false).unwrap(), b"\x1b[15~");
    }

    #[test]
    fn mouse_and_paste() {
        assert_eq!(
            sgr_mouse(MouseButton::WheelUp, false, 0, 4, true),
            b"\x1b[<64;1;5M"
        );
        assert_eq!(
            sgr_mouse(MouseButton::Left, true, 9, 0, true),
            b"\x1b[<32;10;1M"
        );
        assert_eq!(
            sgr_mouse(MouseButton::Left, false, 9, 0, false),
            b"\x1b[<0;10;1m"
        );
        assert_eq!(paste("/a.png ", true), b"\x1b[200~/a.png \x1b[201~");
        assert_eq!(paste("a\nb", false), b"a\rb");
        // An end marker split around another one must not survive the filter.
        assert_eq!(
            paste("x\x1b[20\x1b[201~1~y", true),
            b"\x1b[200~x[20[201~1~y\x1b[201~"
        );
        assert_eq!(paste("a\x1b[2Jb\x07", false), b"a[2Jb");
    }
}
