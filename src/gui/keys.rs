//! Pasted text as the bytes a terminal program expects. Keys go through Ghostty's encoder
//! (`grid`); `lemur paste` and the window both paste through here.

/// Wraps pasted text in bracketed-paste markers when the program asked for them.
pub fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    // Pasted escape sequences (including a smuggled end marker) would run as keystrokes,
    // so keep only printable text, tabs and newlines.
    let text: String = text.chars().filter(|&c| !c.is_control() || matches!(c, '\t' | '\n' | '\r')).collect();
    if bracketed {
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::paste;

    #[test]
    fn pastes_are_bracketed_and_cleaned() {
        assert_eq!(paste("/a.png ", true), b"\x1b[200~/a.png \x1b[201~");
        assert_eq!(paste("a\nb", false), b"a\rb");
        assert_eq!(paste("x\x1b[20\x1b[201~1~y", true), b"\x1b[200~x[20[201~1~y\x1b[201~");
        assert_eq!(paste("a\x1b[2Jb\x07", false), b"a[2Jb");
    }
}
