//! KodAct's colours in the terminal: ink, paper and a burgundy-led painter's palette, one value per role for a dark and for a light background.
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

// The roles: KodAct's identity, a painter's palette around one wine. Burgundy,
// lit to a rose so it reads on a dark page, is the brand and what needs the
// person. Its companions are chosen as on a canvas, each with one job: sea-glass
// (burgundy's complement) and plum carry the structure of an answer (headings,
// lists, quotes, code); a Prussian sky marks links; sage, ochre and vermilion
// still mean done, a warning and failed (vermilion sits apart from the rose in
// hue and is always drawn with a mark). Surfaces and greys are warmed toward
// the wine. On a light background every colour deepens so it reads on paper.

/// The brand: the prompt mark, the spinner, selections, the keys, headings.
pub const BRAND: Role = role(0xe8678a, 0x8a1538, Color::LightMagenta, Color::Magenta);
/// The brand a step brighter, for emphasis: headings in brand colour, the shimmer's peak.
pub const BRAND_STRONG: Role = role(0xf2a3b6, 0x6e0f2b, Color::LightMagenta, Color::Magenta);
/// Burgundy itself, behind white text: filled pills and buttons.
pub const BRAND_FILL: Role = role(0xa3173f, 0x8a1538, Color::Magenta, Color::Magenta);
/// Secondary text.
pub const MUTED: Role = role(0xb0a9ab, 0x5a5254, Color::Gray, Color::DarkGray);
/// Placeholders, hints, details.
/// (At least 4.5:1 on both the page and the raised panel, in both themes.)
pub const SUBTLE: Role = role(0x8f878a, 0x6a6265, Color::DarkGray, Color::DarkGray);
/// Rules, gutters and box edges.
pub const HAIRLINE: Role = role(0x3a2a30, 0xddd3d5, Color::DarkGray, Color::Gray);
/// Links and paths: a Prussian sky.
pub const INFO: Role = role(0x7fa7e8, 0x2a5596, Color::LightBlue, Color::Blue);
/// Done: sage.
pub const SUCCESS: Role = role(0x8ccb8a, 0x2f7a45, Color::LightGreen, Color::Green);
/// Needs a look: ochre.
pub const WARNING: Role = role(0xe8b860, 0x8a5e00, Color::LightYellow, Color::Yellow);
/// Failed: vermilion.
pub const DANGER: Role = role(0xff7a45, 0xb23a16, Color::LightRed, Color::Red);
/// A diff's added line.
pub const ADDED: Role = role(0x8ccb8a, 0x2f7a45, Color::LightGreen, Color::Green);
/// A diff's removed line.
pub const REMOVED: Role = role(0xff7a45, 0xb23a16, Color::LightRed, Color::Red);
/// The first companion, sea-glass (burgundy's complement): second-level headings,
/// numbers in lists, table headers, the second bullet level.
pub const TEAL: Role = role(0x6cc4b4, 0x0e6e62, Color::LightCyan, Color::Cyan);
/// The sea-glass's trail, fading toward the page (the sub-agent scanner).
pub const TEAL_TRAIL: [Role; 3] = [
    role(0x4f9a8d, 0x3f9488, Color::Cyan, Color::Cyan),
    role(0x356b62, 0x7ab8af, Color::DarkGray, Color::Gray),
    role(0x22453f, 0xadd5cf, Color::DarkGray, Color::Gray),
];
/// The second companion, plum: third-level headings, quote bars, the third
/// bullet level, keywords in code, `auto` mode.
pub const VIOLET: Role = role(0xb9a1e0, 0x5b3fa8, Color::LightBlue, Color::Blue);
/// Inline code: warm sand, apart from the rose of the brand and the blue of links.
pub const CODE: Role = role(0xe6c99a, 0x8a5a1c, Color::LightYellow, Color::Yellow);
/// The model's words: a soft paper, a step below the person's own, so long
/// answers read without glare and the two voices stay apart.
pub const ANSWER: Role = role(0xddd6d3, 0x2e2729, Color::Reset, Color::Reset);
/// Bold in an answer: the brightest paper, so emphasis carries.
pub const STRONG: Role = role(0xfbf9f3, 0x08090b, Color::White, Color::Black);
/// Behind inline `code`: a shade off the page, warmed toward the wine.
pub const CODE_BG: Role = role(0x24171c, 0xf1e7e2, Color::Reset, Color::Reset);
/// The person's prompt: a raised surface (the logo's tile).
pub const PROMPT_BG: Role = role(0x1f1418, 0xf3ecea, Color::DarkGray, Color::Gray);
/// Behind a command's `exit 0` badge.
pub const SUCCESS_BG: Role = role(0x16301f, 0xdff1e4, Color::Reset, Color::Reset);
/// Behind a failed command's `exit 1` badge.
pub const DANGER_BG: Role = role(0x3a1a10, 0xfbe2d8, Color::Reset, Color::Reset);
/// Diffs, as GitHub draws them: a soft tint behind an added or a removed line, a stronger one
/// behind the characters that changed in it, a band for a hunk's header, and the empty side of
/// a split row. Text keeps its own colour on them. (16 colours: no tints; the signs and the
/// red and green text carry it.)
pub const DIFF_ADD_LINE: Role = role(0x12261e, 0xe6ffec, Color::Reset, Color::Reset);
pub const DIFF_ADD_WORD: Role = role(0x1f5c36, 0xabf2bc, Color::Reset, Color::Reset);
pub const DIFF_DEL_LINE: Role = role(0x2d161a, 0xffebe9, Color::Reset, Color::Reset);
pub const DIFF_DEL_WORD: Role = role(0x6e2a2f, 0xffc1c0, Color::Reset, Color::Reset);
pub const DIFF_HUNK_BG: Role = role(0x131d2c, 0xddf4ff, Color::Reset, Color::Reset);
pub const DIFF_HUNK_FG: Role = role(0x8fa3bf, 0x4a6078, Color::Blue, Color::Blue);
pub const DIFF_FILLER: Role = role(0x15171c, 0xf3f4f6, Color::Reset, Color::Reset);
/// The line numbers' gutter on a changed line: the line's tint, a little stronger.
pub const DIFF_ADD_GUTTER: Role = role(0x183323, 0xccffd8, Color::Reset, Color::Reset);
pub const DIFF_DEL_GUTTER: Role = role(0x3a1c21, 0xffd7d5, Color::Reset, Color::Reset);
/// A wine tint: the selected row, the glow style's band behind the person's words.
pub const WARM_BG: Role = role(0x2a1219, 0xf6e2e7, Color::DarkGray, Color::Gray);
/// Text on [`PROMPT_BG`].
pub const PROMPT_FG: Role = role(0xf2f0e9, 0x101214, Color::White, Color::Black);
/// The working shimmer's resting and brightest tones: warm grey up to rose.
pub const SHIMMER_BASE: Role = role(0x857d80, 0x857d80, Color::Gray, Color::DarkGray);
pub const SHIMMER_PEAK: Role = role(0xf2a3b6, 0x8a1538, Color::White, Color::Black);

// Code blocks: one quiet text colour and a few token colours from the palette.

/// Plain code.
pub const CODE_TEXT: Role = role(0xd2cbcd, 0x3a3335, Color::Gray, Color::DarkGray);
/// Keywords: `fn`, `def`, `const`, `if` (plum).
pub const CODE_KEYWORD: Role = role(0xb9a1e0, 0x5b3fa8, Color::LightBlue, Color::Blue);
/// String literals (sage).
pub const CODE_STRING: Role = role(0xa8cf8e, 0x4a6e1c, Color::LightGreen, Color::Green);
/// Numbers and constants (ochre).
pub const CODE_NUMBER: Role = role(0xe8b860, 0x8a5e00, Color::LightYellow, Color::Yellow);
/// Comments.
pub const CODE_COMMENT: Role = role(0x8f878a, 0x6a6265, Color::DarkGray, Color::DarkGray);
/// Type names (sea-glass).
pub const CODE_TYPE: Role = role(0x6cc4b4, 0x0e6e62, Color::LightCyan, Color::Cyan);

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
        // KodAct's rose on a dark background: xterm 168 (#d75f87) or a close neighbour.
        let brand = nearest_256(0xe8, 0x67, 0x8a);
        assert!([168, 169, 204, 205].contains(&brand), "{brand}");
    }

    /// WCAG contrast of two colours.
    fn contrast(a: u32, b: u32) -> f64 {
        let lum = |hex: u32| {
            let (r, g, b) = split(hex);
            let lin = |c: u8| {
                let c = c as f64 / 255.0;
                if c <= 0.03928 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn text_reads_on_the_page_and_the_panel_in_both_themes() {
        // The page: ink and white; the panel: the raised prompt.
        let page = (0x0b0c0e, 0xffffff);
        let panel = (PROMPT_BG.dark, PROMPT_BG.light);
        let check = |r: Role, (bg_dark, bg_light): (u32, u32)| {
            assert!(contrast(r.dark, bg_dark) >= 4.5, "{:06x} on {bg_dark:06x}", r.dark);
            assert!(contrast(r.light, bg_light) >= 4.5, "{:06x} on {bg_light:06x}", r.light);
        };
        for r in [BRAND, BRAND_STRONG, MUTED, SUBTLE, INFO, DANGER, TEAL, VIOLET, CODE, ANSWER] {
            check(r, page);
            check(r, panel);
        }
        for r in [SUCCESS, WARNING] {
            check(r, page);
            check(r, panel);
        }
        // White on the filled brand (pills, buttons).
        assert!(contrast(0xffffff, BRAND_FILL.dark) >= 4.5 && contrast(0xffffff, BRAND_FILL.light) >= 4.5);
    }

    /// A colour's hue in degrees and its lightness (0-1).
    fn hue_light(hex: u32) -> (f64, f64) {
        let (r, g, b) = split(hex);
        let (r, g, b) = (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let l = (max + min) / 2.0;
        if max == min {
            return (0.0, l);
        }
        let d = max - min;
        let h = if max == r {
            ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        (h * 60.0, l)
    }

    #[test]
    fn a_failure_never_reads_as_the_brand() {
        for (b, d) in [(BRAND.dark, DANGER.dark), (BRAND.light, DANGER.light)] {
            let ((hb, _), (hd, _)) = (hue_light(b), hue_light(d));
            let apart = (hb - hd).abs().min(360.0 - (hb - hd).abs());
            assert!(apart >= 25.0, "{b:06x} and {d:06x} are {apart:.0}° apart");
        }
        assert_ne!(BRAND.ansi_dark, DANGER.ansi_dark);
        assert_ne!(BRAND.ansi_light, DANGER.ansi_light);
    }

    #[test]
    fn roles_differ_for_light_and_dark_backgrounds() {
        assert_eq!(BRAND.color(Depth::True, false), Color::Rgb(0xe8, 0x67, 0x8a));
        assert_eq!(BRAND.color(Depth::True, true), Color::Rgb(0x8a, 0x15, 0x38));
        assert_eq!(BRAND.color(Depth::Ansi16, false), Color::LightMagenta);
        assert!(matches!(BRAND.color(Depth::Ansi256, false), Color::Indexed(_)));
    }
}
