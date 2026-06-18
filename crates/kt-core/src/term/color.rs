//! Color resolution: turn alacritty/vte `Color` values into concrete RGB.
//!
//! The terminal grid stores colors as `Named(NamedColor)`, `Indexed(u8)`, or
//! `Spec(Rgb)`. The renderer (and the headless printer) want concrete 24-bit
//! RGB, so we resolve everything here against a built-in palette, falling back
//! to any palette overrides the terminal itself has set (OSC 4 etc.).

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb as VteRgb};

/// A concrete 24-bit color. Plain data, trivially `Send`/`Copy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

impl From<VteRgb> for Rgb {
    fn from(c: VteRgb) -> Self {
        Self {
            r: c.r,
            g: c.g,
            b: c.b,
        }
    }
}

/// Default foreground / background for the built-in dark theme.
pub const DEFAULT_FG: Rgb = Rgb::new(0xd0, 0xd0, 0xd0);
pub const DEFAULT_BG: Rgb = Rgb::new(0x1a, 0x1b, 0x26);
pub const DEFAULT_CURSOR: Rgb = Rgb::new(0xc0, 0xca, 0xf5);

/// The 16 ANSI base colors (a Tokyo-Night-ish palette).
const ANSI_16: [Rgb; 16] = [
    Rgb::new(0x15, 0x16, 0x1e), // 0 black
    Rgb::new(0xf7, 0x76, 0x8e), // 1 red
    Rgb::new(0x9e, 0xce, 0x6a), // 2 green
    Rgb::new(0xe0, 0xaf, 0x68), // 3 yellow
    Rgb::new(0x7a, 0xa2, 0xf7), // 4 blue
    Rgb::new(0xbb, 0x9a, 0xf7), // 5 magenta
    Rgb::new(0x7d, 0xcf, 0xff), // 6 cyan
    Rgb::new(0xa9, 0xb1, 0xd6), // 7 white
    Rgb::new(0x41, 0x48, 0x68), // 8 bright black
    Rgb::new(0xf7, 0x76, 0x8e), // 9 bright red
    Rgb::new(0x9e, 0xce, 0x6a), // 10 bright green
    Rgb::new(0xe0, 0xaf, 0x68), // 11 bright yellow
    Rgb::new(0x7a, 0xa2, 0xf7), // 12 bright blue
    Rgb::new(0xbb, 0x9a, 0xf7), // 13 bright magenta
    Rgb::new(0x7d, 0xcf, 0xff), // 14 bright cyan
    Rgb::new(0xc0, 0xca, 0xf5), // 15 bright white
];

/// Resolve an indexed (0..=255) color using the xterm 256-color cube.
fn indexed_to_rgb(i: u8) -> Rgb {
    match i {
        0..=15 => ANSI_16[i as usize],
        16..=231 => {
            // 6×6×6 color cube.
            let i = i - 16;
            let r = i / 36;
            let g = (i % 36) / 6;
            let b = i % 6;
            let level = |v: u8| -> u8 {
                if v == 0 {
                    0
                } else {
                    55 + v * 40
                }
            };
            Rgb::new(level(r), level(g), level(b))
        }
        232..=255 => {
            // 24-step grayscale ramp.
            let v = 8 + (i - 232) * 10;
            Rgb::new(v, v, v)
        }
    }
}

/// Resolve a named color to RGB, honoring terminal palette overrides.
fn named_to_rgb(named: NamedColor, palette: &Colors) -> Rgb {
    // OSC overrides take precedence when present.
    if let Some(over) = palette[named as usize] {
        return over.into();
    }
    match named {
        NamedColor::Foreground | NamedColor::BrightForeground => DEFAULT_FG,
        NamedColor::DimForeground => Rgb::new(0x9a, 0x9a, 0x9a),
        NamedColor::Background => DEFAULT_BG,
        NamedColor::Cursor => DEFAULT_CURSOR,
        // The 16 base colors and their dim variants map onto ANSI_16.
        other => {
            let idx = other as usize;
            if idx < 16 {
                ANSI_16[idx]
            } else {
                // Dim variants (DimBlack..DimWhite) → approximate with base color.
                match other {
                    NamedColor::DimBlack => ANSI_16[0],
                    NamedColor::DimRed => ANSI_16[1],
                    NamedColor::DimGreen => ANSI_16[2],
                    NamedColor::DimYellow => ANSI_16[3],
                    NamedColor::DimBlue => ANSI_16[4],
                    NamedColor::DimMagenta => ANSI_16[5],
                    NamedColor::DimCyan => ANSI_16[6],
                    NamedColor::DimWhite => ANSI_16[7],
                    _ => DEFAULT_FG,
                }
            }
        }
    }
}

/// Resolve any terminal `Color` into concrete RGB.
pub fn resolve(color: Color, palette: &Colors) -> Rgb {
    match color {
        Color::Spec(rgb) => rgb.into(),
        Color::Indexed(i) => {
            if let Some(over) = palette[i as usize] {
                over.into()
            } else {
                indexed_to_rgb(i)
            }
        }
        Color::Named(named) => named_to_rgb(named, palette),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_corners() {
        // 16 = black corner of the cube, 231 = white corner.
        assert_eq!(indexed_to_rgb(16), Rgb::new(0, 0, 0));
        assert_eq!(indexed_to_rgb(231), Rgb::new(255, 255, 255));
    }

    #[test]
    fn grayscale_ramp_monotonic() {
        let mut last = 0u8;
        for i in 232u8..=255 {
            let g = indexed_to_rgb(i);
            assert_eq!(g.r, g.g);
            assert_eq!(g.g, g.b);
            assert!(g.r >= last);
            last = g.r;
        }
    }

    #[test]
    fn base_16_passthrough() {
        assert_eq!(indexed_to_rgb(1), ANSI_16[1]);
        assert_eq!(indexed_to_rgb(15), ANSI_16[15]);
    }

    #[test]
    fn spec_is_verbatim() {
        let palette = Colors::default();
        let c = resolve(Color::Spec(VteRgb { r: 1, g: 2, b: 3 }), &palette);
        assert_eq!(c, Rgb::new(1, 2, 3));
    }
}
