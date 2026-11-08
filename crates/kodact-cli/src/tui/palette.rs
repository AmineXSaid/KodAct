//! KodAct's colours in the terminal: ink, paper and lime, one value per role for a dark and for a light background.
//! A role is drawn in 24-bit colour where the terminal has it, else as the
//! nearest of xterm's 256 colours, else as one of the 16 ANSI colours.

use ratatui::style::Color;

/// How many colours the terminal can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Depth {
    /// 24-bit colour.
    #[default]
    True,
    /// xterm's 256 colours.
    Ansi256,
    /// The 16 ANSI colours, in the person's own palette.
    Ansi16,
}

impl Depth {
    /// From `KODACT_COLOR_DEPTH` (`truecolor`, `256`, `16`), else what the terminal says of itself.
    pub fn detect() -> Depth {
        let var = |k: &str| std::env::var(k).unwrap_or_default().to_ascii_lowercase();
        Depth::from_env(&var("KODACT_COLOR_DEPTH"), &var("COLORTERM"), &var("TERM"), &var("TERM_PROGRAM"), {
            std::env::var_os("WT_SESSION").is_some()
        })
    }

    fn from_env(forced: &str, colorterm: &str, term: &str, program: &str, windows_terminal: bool) -> Depth {
        match forced {
            "truecolor" | "24bit" | "true" => return Depth::True,
            "256" => return Depth::Ansi256,
            "16" | "8" => return Depth::Ansi16,
            _ => {}
        }
        if matches!(colorterm, "truecolor" | "24bit")
            || term.contains("direct")
            || windows_terminal
            || matches!(program, "iterm.app" | "wezterm" | "vscode" | "ghostty" | "hyper" | "tabby")
        {
            Depth::True
        } else if term.contains("256") {
            Depth::Ansi256
        } else if cfg!(windows) && term.is_empty() {
            // Windows 10's console has drawn 24-bit colour since 1703.
            Depth::True
        } else {
            Depth::Ansi16
        }
    }
}

/// Whether the terminal's background is light, from `COLORFGBG` ("15;0" is
/// light text on black). `None` when the terminal doesn't say.
pub fn light_background() -> Option<bool> {
    let v = std::env::var("COLORFGBG").ok()?;
    let bg: u8 = v.rsplit(';').next()?.trim().parse().ok()?;
    Some(matches!(bg, 7 | 15))
}

/// One role: its dark- and light-background colours, and the ANSI colour it
/// falls back to in 16-colour terminals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Role {
    pub dark: u32,
    pub light: u32,
    pub ansi_dark: Color,
    pub ansi_light: Color,
}

const fn role(dark: u32, light: u32, ansi_dark: Color, ansi_light: Color) -> Role {
    Role { dark, light, ansi_dark, ansi_light }
}

impl Role {
    pub fn color(&self, depth: Depth, light: bool) -> Color {
        match depth {
            Depth::True => rgb(if light { self.light } else { self.dark }),
            Depth::Ansi256 => {
                let (r, g, b) = split(if light { self.light } else { self.dark });
                Color::Indexed(nearest_256(r, g, b))
            }
            Depth::Ansi16 => {
                if light {
                    self.ansi_light
                } else {
                    self.ansi_dark
                }
            }
        }
    }
}

// The roles: KodAct's identity. Ink and paper for text and surfaces, lime for
// the brand and for what needs the person. Teal and violet, set either side of
// lime on the colour wheel, carry the structure of an answer (headings, lists,
// quotes, code), so it is not all one colour. Green, red and amber still mean
// done, failed and a warning. On a light
// background the lime deepens to an olive that reads on paper.

/// The brand: the prompt mark, the spinner, selections, the keys, headings.
pub const BRAND: Role = role(0xc6ff3d, 0x4a7200, Color::LightGreen, Color::Green);
/// The brand a step brighter, for emphasis: the wordmark, the shimmer's peak.
pub const BRAND_STRONG: Role = role(0xdcff85, 0x3d5f00, Color::LightGreen, Color::Green);
/// Secondary text.
pub const MUTED: Role = role(0xa9adb2, 0x55595e, Color::Gray, Color::DarkGray);
/// Placeholders, hints, details.
/// (At least 4.5:1 on both the page and the raised panel, in both themes.)
pub const SUBTLE: Role = role(0x868b92, 0x63676d, Color::DarkGray, Color::DarkGray);
/// Rules, gutters and box edges.
pub const HAIRLINE: Role = role(0x2d3238, 0xd2d4d6, Color::DarkGray, Color::Gray);
/// Links and paths.
pub const INFO: Role = role(0x8fbef0, 0x2f63a8, Color::LightBlue, Color::Blue);
/// Done.
pub const SUCCESS: Role = role(0x7ccf8f, 0x2f7a45, Color::LightGreen, Color::Green);
/// Needs a look.
pub const WARNING: Role = role(0xf0c25e, 0x8f6200, Color::LightYellow, Color::Yellow);
/// Failed.
pub const DANGER: Role = role(0xff7a6b, 0xb8392e, Color::LightRed, Color::Red);
/// A diff's added line.
pub const ADDED: Role = role(0x7ccf8f, 0x2f7a45, Color::LightGreen, Color::Green);
/// A diff's removed line.
pub const REMOVED: Role = role(0xff7a6b, 0xb8392e, Color::LightRed, Color::Red);
/// The first companion to lime, a cool teal: second-level headings, numbers
/// in lists, table headers, the second bullet level.
pub const TEAL: Role = role(0x5fd8c4, 0x0e7568, Color::LightCyan, Color::Cyan);
/// The teal's trail, fading toward the page (the sub-agent scanner).
pub const TEAL_TRAIL: [Role; 3] = [
    role(0x41a597, 0x3f9488, Color::Cyan, Color::Cyan),
    role(0x2b7268, 0x7ab8af, Color::DarkGray, Color::Gray),
    role(0x1d4943, 0xadd5cf, Color::DarkGray, Color::Gray),
];
/// The second companion, a soft violet: third-level headings, quote bars,
/// the third bullet level, keywords in code.
pub const VIOLET: Role = role(0xb4a2ff, 0x5a44c0, Color::LightMagenta, Color::Magenta);
/// Inline code: a warm peach, the one warm accent, apart from the blue of
/// links and the lime of the brand.
pub const CODE: Role = role(0xffb98f, 0xa4461c, Color::LightRed, Color::Red);
/// The model's words: a soft paper, a step below the person's own, so long
/// answers read without glare and the two voices stay apart.
pub const ANSWER: Role = role(0xd9d7cf, 0x2b2e33, Color::Reset, Color::Reset);
/// Bold in an answer: the brightest paper, so emphasis carries.
pub const STRONG: Role = role(0xfbf9f3, 0x08090b, Color::White, Color::Black);
/// Behind inline `code`: a shade off the page, so code reads as code without backticks.
pub const CODE_BG: Role = role(0x22262b, 0xece6db, Color::Reset, Color::Reset);
/// The person's prompt: a raised surface (the logo's tile).
pub const PROMPT_BG: Role = role(0x1b1f23, 0xebeae4, Color::DarkGray, Color::Gray);
/// The glow style's band behind the person's words: a lime tint.
pub const WARM_BG: Role = role(0x1c230f, 0xe8f1d2, Color::DarkGray, Color::Gray);
/// Text on [`PROMPT_BG`].
pub const PROMPT_FG: Role = role(0xf2f0e9, 0x101214, Color::White, Color::Black);
/// The working shimmer's resting and brightest tones: grey up to lime.
pub const SHIMMER_BASE: Role = role(0x80858b, 0x83878d, Color::Gray, Color::DarkGray);
pub const SHIMMER_PEAK: Role = role(0xdcff85, 0x4a7200, Color::White, Color::Black);

// Code blocks: one quiet text colour and a few token colours.

/// Plain code.
pub const CODE_TEXT: Role = role(0xcdd1d5, 0x33373b, Color::Gray, Color::DarkGray);
/// Keywords: `fn`, `def`, `const`, `if`.
pub const CODE_KEYWORD: Role = role(0xb4a2ff, 0x5a44c0, Color::LightMagenta, Color::Magenta);
/// String literals.
pub const CODE_STRING: Role = role(0xc9ec7a, 0x4f7000, Color::LightGreen, Color::Green);
/// Numbers and constants.
pub const CODE_NUMBER: Role = role(0xf0c25e, 0x8f6200, Color::LightYellow, Color::Yellow);
/// Comments.
pub const CODE_COMMENT: Role = role(0x868b92, 0x63676d, Color::DarkGray, Color::DarkGray);
/// Type names.
pub const CODE_TYPE: Role = role(0x5fd8c4, 0x0e7568, Color::LightCyan, Color::Cyan);

pub const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

const fn split(hex: u32) -> (u8, u8, u8) {
    ((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// A colour of the logo: 24-bit, the nearest of 256, or `ansi`.
pub fn ink(hex: u32, depth: Depth, ansi: Color) -> Color {
    match depth {
        Depth::True => rgb(hex),
        Depth::Ansi256 => {
            let (r, g, b) = split(hex);
            Color::Indexed(nearest_256(r, g, b))
        }
        Depth::Ansi16 => ansi,
    }
}

/// The xterm colour (16-255) closest to `r g b`: the 6x6x6 cube or the grey ramp.
pub fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let step = |v: u8| -> usize {
        let mut best = 0;
        for (i, s) in STEPS.iter().enumerate() {
            if (v as i32 - *s as i32).abs() < (v as i32 - STEPS[best] as i32).abs() {
                best = i;
            }
        }
        best
    };
    let dist = |a: (u8, u8, u8)| -> i32 {
        let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2);
        // Weighted for the eye: green counts most, blue least.
        3 * d(a.0, r) + 4 * d(a.1, g) + 2 * d(a.2, b)
    };
    let (ri, gi, bi) = (step(r), step(g), step(b));
    let cube = (STEPS[ri], STEPS[gi], STEPS[bi]);
    let cube_index = 16 + 36 * ri as u8 + 6 * gi as u8 + bi as u8;
    let avg = (r as u32 + g as u32 + b as u32) / 3;
    let grey_i = (((avg as i32 - 8).max(0) + 5) / 10).min(23) as u8;
    let grey = 8 + 10 * grey_i;
    if dist((grey, grey, grey)) < dist(cube) {
        232 + grey_i
    } else {
        cube_index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_comes_from_what_the_terminal_says() {
        assert_eq!(Depth::from_env("", "truecolor", "xterm-256color", "", false), Depth::True);
        assert_eq!(Depth::from_env("", "", "xterm-256color", "", false), Depth::Ansi256);
        assert_eq!(Depth::from_env("", "", "screen", "", true), Depth::True, "Windows Terminal");
        assert_eq!(Depth::from_env("", "", "xterm", "iterm.app", false), Depth::True);
        if !cfg!(windows) {
            assert_eq!(Depth::from_env("", "", "xterm", "", false), Depth::Ansi16);
        }
        assert_eq!(Depth::from_env("256", "truecolor", "", "", false), Depth::Ansi256, "the override wins");
    }

    #[test]
    fn the_nearest_256_colour_is_the_right_one() {
        assert_eq!(nearest_256(0xff, 0xff, 0xff), 231);
        assert_eq!(nearest_256(0, 0, 0), 16);
        assert_eq!(nearest_256(0x80, 0x80, 0x80), 244);
        // KodAct's brand on a dark background: xterm 141 (#af87ff) or a close neighbour.
        let brand = nearest_256(0x94, 0x75, 0xdb);
        assert!([98, 104, 140, 141].contains(&brand), "{brand}");
    }

    #[test]
    fn roles_differ_for_light_and_dark_backgrounds() {
        assert_eq!(BRAND.color(Depth::True, false), Color::Rgb(0xc6, 0xff, 0x3d));
        assert_eq!(BRAND.color(Depth::True, true), Color::Rgb(0x4a, 0x72, 0x00));
        assert_eq!(BRAND.color(Depth::Ansi16, false), Color::LightGreen);
        assert!(matches!(BRAND.color(Depth::Ansi256, false), Color::Indexed(_)));
    }
}
