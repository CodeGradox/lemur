//! Terminal colours for one appearance: One Half Dark and Light, with what Settings changes
//! over them. Programs can still set any entry (OSC 4/10/11); Ghostty keeps those.

use crate::config::{Terminal, TerminalColors, parse_hex};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Palette {
    pub ansi: [Rgb; 16],
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
}

/// One color a palette sets.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Slot {
    Foreground,
    Background,
    Cursor,
    /// 0-7 normal, 8-15 bright: black, red, green, yellow, blue, magenta, cyan, white.
    Ansi(usize),
}

/// Every slot with its name, as Settings lists them.
pub const SLOTS: [(Slot, &str); 19] = [
    (Slot::Background, "Background"),
    (Slot::Foreground, "Foreground"),
    (Slot::Cursor, "Cursor"),
    (Slot::Ansi(0), "Black"),
    (Slot::Ansi(1), "Red"),
    (Slot::Ansi(2), "Green"),
    (Slot::Ansi(3), "Yellow"),
    (Slot::Ansi(4), "Blue"),
    (Slot::Ansi(5), "Magenta"),
    (Slot::Ansi(6), "Cyan"),
    (Slot::Ansi(7), "White"),
    (Slot::Ansi(8), "Bright black"),
    (Slot::Ansi(9), "Bright red"),
    (Slot::Ansi(10), "Bright green"),
    (Slot::Ansi(11), "Bright yellow"),
    (Slot::Ansi(12), "Bright blue"),
    (Slot::Ansi(13), "Bright magenta"),
    (Slot::Ansi(14), "Bright cyan"),
    (Slot::Ansi(15), "Bright white"),
];

pub const fn rgb(hex: u32) -> Rgb {
    Rgb { r: (hex >> 16) as u8, g: (hex >> 8) as u8, b: hex as u8 }
}

/// One Half Dark, as the WezTerm scheme of that name.
pub const DARK: Palette = Palette {
    ansi: [
        rgb(0x282c34),
        rgb(0xe17079),
        rgb(0x98c379),
        rgb(0xe5c07b),
        rgb(0x61afef),
        rgb(0xc678dd),
        rgb(0x56b6c2),
        rgb(0xdcdfe4),
        rgb(0x8c939f),
        rgb(0xe17079),
        rgb(0x98c379),
        rgb(0xe5c07b),
        rgb(0x61afef),
        rgb(0xc678dd),
        rgb(0x56b6c2),
        rgb(0xffffff),
    ],
    foreground: rgb(0xdcdfe4),
    background: rgb(0x282c34),
    cursor: rgb(0x979db4),
};

/// One Half Light, as the WezTerm scheme of that name.
pub const LIGHT: Palette = Palette {
    ansi: [
        rgb(0x383a42),
        rgb(0xce4136),
        rgb(0x318332),
        rgb(0x986801),
        rgb(0x356ce5),
        rgb(0xa626a4),
        rgb(0x007ab2),
        rgb(0xa0a1a7),
        rgb(0x696c77),
        rgb(0xce4136),
        rgb(0x318332),
        rgb(0x986801),
        rgb(0x356ce5),
        rgb(0xa626a4),
        rgb(0x007ab2),
        rgb(0xfafafa),
    ],
    foreground: rgb(0x383a42),
    background: rgb(0xfafafa),
    cursor: rgb(0x4f525e),
};

/// Sets one color of the dark or light palette, or with `None` puts lemur's own back; the
/// config keeps only what differs from lemur's own.
pub fn set_color(terminal: &mut Terminal, dark: bool, slot: Slot, color: Option<Rgb>) {
    let defaults = Palette::default_for(dark);
    let mut palette = Palette::themed(dark, terminal);
    palette.set(slot, color.unwrap_or(defaults.get(slot)));
    let colors = palette.overrides(defaults);
    if dark {
        terminal.dark = colors;
    } else {
        terminal.light = colors;
    }
}

/// `#rrggbb`.
pub fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

impl Palette {
    /// lemur's own colors for dark or light.
    pub fn default_for(dark: bool) -> &'static Palette {
        if dark { &DARK } else { &LIGHT }
    }

    /// The colors for dark or light: Settings' over lemur's own.
    pub fn themed(dark: bool, terminal: &Terminal) -> Palette {
        let mut palette = *Palette::default_for(dark);
        let colors = if dark { &terminal.dark } else { &terminal.light };
        let parse = |hex: &Option<String>| hex.as_deref().and_then(parse_hex).map(rgb);
        let slots = [
            (Slot::Foreground, &colors.foreground),
            (Slot::Background, &colors.background),
            (Slot::Cursor, &colors.cursor),
        ];
        for (slot, hex) in slots {
            if let Some(c) = parse(hex) {
                palette.set(slot, c);
            }
        }
        for (offset, list) in [(0, &colors.ansi), (8, &colors.brights)] {
            for (i, hex) in list.iter().flatten().take(8).enumerate() {
                if let Some(c) = parse_hex(hex).map(rgb) {
                    palette.set(Slot::Ansi(offset + i), c);
                }
            }
        }
        palette
    }

    pub fn get(&self, slot: Slot) -> Rgb {
        match slot {
            Slot::Foreground => self.foreground,
            Slot::Background => self.background,
            Slot::Cursor => self.cursor,
            Slot::Ansi(i) => self.ansi[i],
        }
    }

    pub fn set(&mut self, slot: Slot, color: Rgb) {
        match slot {
            Slot::Foreground => self.foreground = color,
            Slot::Background => self.background = color,
            Slot::Cursor => self.cursor = color,
            Slot::Ansi(i) => self.ansi[i] = color,
        }
    }

    /// What differs from `defaults`, as the config writes it.
    fn overrides(&self, defaults: &Palette) -> TerminalColors {
        let differs = |slot| (self.get(slot) != defaults.get(slot)).then(|| hex(self.get(slot)));
        let eight = |offset: usize| {
            (offset..offset + 8)
                .any(|i| self.ansi[i] != defaults.ansi[i])
                .then(|| self.ansi[offset..offset + 8].iter().map(|c| hex(*c)).collect())
        };
        TerminalColors {
            foreground: differs(Slot::Foreground),
            background: differs(Slot::Background),
            cursor: differs(Slot::Cursor),
            ansi: eight(0),
            brights: eight(8),
        }
    }

    /// Behind selected text: a quarter of the way toward the foreground.
    pub fn selection(&self) -> Rgb {
        let mix = |a: u8, b: u8| ((a as u16 + b as u16 * 3) / 4) as u8;
        let (fg, bg) = (self.foreground, self.background);
        Rgb { r: mix(fg.r, bg.r), g: mix(fg.g, bg.g), b: mix(fg.b, bg.b) }
    }

    /// xterm's 256 colours, the first 16 this palette's.
    pub fn indexed(&self, i: usize) -> Rgb {
        const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
        match i {
            0..16 => self.ansi[i],
            16..232 => {
                let i = i - 16;
                Rgb { r: LEVELS[i / 36], g: LEVELS[(i / 6) % 6], b: LEVELS[i % 6] }
            }
            _ => {
                let v = 8 + 10 * (i.min(255) - 232) as u8;
                Rgb { r: v, g: v, b: v }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_colors_go_over_lemurs_own_and_back() {
        let mut terminal = Terminal::default();
        set_color(&mut terminal, true, Slot::Ansi(1), Some(rgb(0x112233)));
        assert_eq!(terminal.dark.ansi.as_ref().unwrap()[1], "#112233");
        assert_eq!(Palette::themed(true, &terminal).get(Slot::Ansi(1)), rgb(0x112233));
        set_color(&mut terminal, true, Slot::Ansi(1), None);
        assert_eq!(terminal.dark, TerminalColors::default());
        assert_eq!(Palette::themed(true, &terminal), DARK);
    }

    #[test]
    fn the_256_colours_follow_xterm() {
        assert_eq!(DARK.indexed(16), rgb(0x000000));
        assert_eq!(DARK.indexed(231), rgb(0xffffff));
        assert_eq!(DARK.indexed(232), rgb(0x080808));
        assert_eq!(DARK.indexed(1), DARK.ansi[1]);
    }
}
