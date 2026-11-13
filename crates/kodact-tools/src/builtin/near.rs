//! Near matches for Edit: `old_string` is not in the file as written, but one transformation maps
//! it exactly onto one place in the file (trailing whitespace, a uniform indentation offset, tabs
//! for spaces, or typographic characters for their ASCII forms). The edit is then applied there,
//! with `new_string` written through the same transformation, and the result says what differed.
//!
//! The idea is common to other harnesses (Aider's edit blocks, Gemini CLI's flexible match,
//! Codex's patch context search, opencode's normalized match); this is KodAct's own form of it.
//! What it adds: a near match is applied only when the transformation reproduces the file's
//! text from `old_string` exactly, and the text `new_string` copied from `old_string` gets the
//! file's own characters, so the file's style survives an edit typed in another style.

/// What differed between `old_string` and the file.
#[derive(Debug, Clone, PartialEq)]
pub enum NearKind {
    TrailingWhitespace,
    /// `how` names the file's indentation (for example "tabs" or "4 more spaces").
    Indentation {
        how: String,
    },
    Typographic {
        chars: String,
    },
}

/// A near match applied: the new content, the kind, and the 1-based lines it replaced.
#[derive(Debug, Clone, PartialEq)]
pub struct NearMatch {
    pub content: String,
    pub kind: NearKind,
    pub first_line: usize,
    pub last_line: usize,
}

impl NearMatch {
    /// The sentence the tool result carries, so the model knows the file differed from what it
    /// sent.
    pub fn note(&self) -> String {
        let at = if self.first_line == self.last_line {
            format!("line {}", self.first_line)
        } else {
            format!("lines {}-{}", self.first_line, self.last_line)
        };
        match &self.kind {
            NearKind::TrailingWhitespace => format!(
                "old_string matched {at} only once trailing whitespace was ignored; the replaced lines have none now."
            ),
            NearKind::Indentation { how } => format!(
                "old_string matched {at} only once indentation was ignored: the file indents them with {how}, and \
                 new_string was re-indented to match."
            ),
            NearKind::Typographic { chars } => format!(
                "old_string matched {at} only once typographic characters were read as their ASCII forms (the file \
                 has {chars}); the text new_string kept from old_string uses the file's characters, and the text it \
                 added is written as given."
            ),
        }
    }
}

/// Find and apply a near match of `old` in `content` (both with LF line endings). `None` when
/// there is none, or more than one, or when no single transformation explains the difference.
pub fn find(content: &str, old: &str, new: &str) -> Option<NearMatch> {
    by_lines(content, old, new, Mode::Trailing)
        .or_else(|| by_lines(content, old, new, Mode::Indent))
        .or_else(|| typographic(content, old, new))
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Trailing,
    Indent,
}

/// `old` split into lines; a final newline gives no extra empty line.
fn lines_of(s: &str) -> Vec<&str> {
    let s = s.strip_suffix('\n').unwrap_or(s);
    s.split('\n').collect()
}

fn indent_of(l: &str) -> &str {
    &l[..l.len() - l.trim_start().len()]
}

fn by_lines(content: &str, old: &str, new: &str, mode: Mode) -> Option<NearMatch> {
    let file: Vec<&str> = content.split_inclusive('\n').collect();
    let bare: Vec<&str> = file.iter().map(|l| l.strip_suffix('\n').unwrap_or(l)).collect();
    let want = lines_of(old);
    let m = want.len();
    if m == 0 || m > bare.len() || want.iter().all(|l| l.trim().is_empty()) {
        return None;
    }
    let same = |a: &str, b: &str| match mode {
        Mode::Trailing => a.trim_end() == b.trim_end(),
        Mode::Indent => a.trim() == b.trim(),
    };
    let hits: Vec<usize> = (0..=bare.len() - m).filter(|&i| (0..m).all(|j| same(bare[i + j], want[j]))).collect();
    let [i] = hits.as_slice() else { return None };
    let region = &bare[*i..*i + m];
    let (replacement, kind) = match mode {
        Mode::Trailing => (lines_of(new).join("\n"), NearKind::TrailingWhitespace),
        Mode::Indent => {
            let map = IndentMap::infer(&want, region)?;
            // The map must give back the file's own lines from old_string.
            if want.iter().zip(region).any(|(o, f)| map.apply(o).as_deref().map(str::trim_end) != Some(f.trim_end())) {
                return None;
            }
            let lines: Option<Vec<String>> = lines_of(new).iter().map(|l| map.apply(l)).collect();
            (lines?.join("\n"), NearKind::Indentation { how: map.describe() })
        }
    };
    let ends_with_newline = file[*i + m - 1].ends_with('\n');
    let mut out = file[..*i].concat();
    out.push_str(&replacement);
    if ends_with_newline && !replacement.is_empty() {
        out.push('\n');
    }
    out.push_str(&file[*i + m..].concat());
    if out == content {
        return None;
    }
    Some(NearMatch { content: out, kind, first_line: i + 1, last_line: i + m })
}

/// How the indentation `old_string` used maps onto the file's.
enum IndentMap {
    /// The same characters, with a common prefix swapped (`from` for `to`).
    Prefix { from: String, to: String },
    /// Spaces in old_string, tabs in the file: `width` spaces per tab, plus `extra` tabs.
    SpacesToTabs { width: usize, extra: usize },
    /// Tabs in old_string, spaces in the file: `width` spaces per tab, plus `extra` spaces.
    TabsToSpaces { width: usize, extra: usize },
}

impl IndentMap {
    fn infer(old: &[&str], file: &[&str]) -> Option<Self> {
        let pairs: Vec<(&str, &str)> = old
            .iter()
            .zip(file)
            .filter(|(o, _)| !o.trim().is_empty())
            .map(|(o, f)| (indent_of(o), indent_of(f)))
            .collect();
        let (o0, f0) = *pairs.first()?;
        let only = |s: &str, c: char| s.chars().all(|x| x == c);
        let old_spaces = pairs.iter().all(|(o, _)| only(o, ' '));
        let old_tabs = pairs.iter().all(|(o, _)| only(o, '\t'));
        let file_spaces = pairs.iter().all(|(_, f)| only(f, ' '));
        let file_tabs = pairs.iter().all(|(_, f)| only(f, '\t'));
        if old_spaces && file_tabs && pairs.iter().any(|(o, _)| !o.is_empty()) {
            let width = Self::width(&pairs, |o| o.len(), |f| f.len())?;
            let extra = f0.len().checked_sub(o0.len() / width)?;
            return Some(IndentMap::SpacesToTabs { width, extra });
        }
        if old_tabs
            && file_spaces
            && pairs.iter().any(|(_, f)| !f.is_empty())
            && pairs.iter().any(|(o, _)| !o.is_empty())
        {
            let width = Self::width(&pairs, |f| f.len(), |o| o.len())?;
            let extra = f0.len().checked_sub(o0.len() * width)?;
            return Some(IndentMap::TabsToSpaces { width, extra });
        }
        // The same style: every line's indentation is the first line's plus the same rest.
        let consistent = pairs.iter().all(|(o, f)| {
            o.strip_prefix(o0).is_some_and(|rest| f.strip_prefix(f0) == Some(rest))
                || f.strip_prefix(f0).is_some_and(|rest| o.strip_prefix(o0) == Some(rest))
        });
        (consistent && o0 != f0).then(|| IndentMap::Prefix { from: o0.to_string(), to: f0.to_string() })
    }

    /// Spaces per tab, from the lines' levels: one width for every line, or `None`.
    fn width(pairs: &[(&str, &str)], spaces: impl Fn(&str) -> usize, tabs: impl Fn(&str) -> usize) -> Option<usize> {
        let levels: Vec<(usize, usize)> = pairs
            .iter()
            .map(|(o, f)| if spaces(o) >= spaces(f) { (spaces(o), tabs(f)) } else { (spaces(f), tabs(o)) })
            .collect();
        // Spaces per tab: from two lines at different levels, or from one line alone.
        let width = levels
            .iter()
            .flat_map(|a| levels.iter().map(move |b| (a, b)))
            .find(|(a, b)| a.1 != b.1)
            .map(|(a, b)| a.0.abs_diff(b.0) / a.1.abs_diff(b.1))
            .or_else(|| levels.iter().find(|l| l.1 > 0 && l.0 % l.1 == 0).map(|l| l.0 / l.1))?;
        (width > 0 && levels.iter().all(|(s, _)| s % width == 0)).then_some(width)
    }

    /// `line` with its indentation mapped, or `None` when this map can't say what it becomes.
    fn apply(&self, line: &str) -> Option<String> {
        if line.trim().is_empty() {
            return Some(String::new());
        }
        let (ind, rest) = line.split_at(line.len() - line.trim_start().len());
        let ind = match self {
            IndentMap::Prefix { from, to } => {
                let tail = ind.strip_prefix(from.as_str())?;
                format!("{to}{tail}")
            }
            IndentMap::SpacesToTabs { width, extra } => {
                if !ind.chars().all(|c| c == ' ') || ind.len() % width != 0 {
                    return None;
                }
                "\t".repeat(ind.len() / width + extra)
            }
            IndentMap::TabsToSpaces { width, extra } => {
                if !ind.chars().all(|c| c == '\t') {
                    return None;
                }
                " ".repeat(ind.len() * width + extra)
            }
        };
        Some(format!("{ind}{rest}"))
    }

    fn describe(&self) -> String {
        match self {
            IndentMap::Prefix { from, to } => {
                let tabs = to.contains('\t') || from.contains('\t');
                let unit = if tabs { "tab" } else { "space" };
                let (a, b) = (from.chars().count(), to.chars().count());
                if b >= a {
                    format!("{} more {unit}{}", b - a, if b - a == 1 { "" } else { "s" })
                } else {
                    format!("{} fewer {unit}{}", a - b, if a - b == 1 { "" } else { "s" })
                }
            }
            IndentMap::SpacesToTabs { width, .. } => format!("tabs (where old_string had {width} spaces per level)"),
            IndentMap::TabsToSpaces { width, .. } => format!("{width} spaces per level (where old_string had tabs)"),
        }
    }
}

/// The ASCII form of a typographic character, if it has one. One character for one, so
/// positions in the text and its normalized form line up.
fn ascii(c: char) -> Option<char> {
    Some(match c {
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' | '\u{2032}' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' | '\u{2033}' => '"',
        '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}' | '\u{2212}' => '-',
        '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}' | '\u{2007}' | '\u{2008}'
        | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
        _ => return None,
    })
}

fn normalized(chars: &[char]) -> Vec<char> {
    chars.iter().map(|&c| ascii(c).unwrap_or(c)).collect()
}

fn typographic(content: &str, old: &str, new: &str) -> Option<NearMatch> {
    let file: Vec<char> = content.chars().collect();
    let want: Vec<char> = old.chars().collect();
    if want.is_empty() || want.len() > file.len() {
        return None;
    }
    let (nf, nw) = (normalized(&file), normalized(&want));
    let hits: Vec<usize> = (0..=nf.len() - nw.len()).filter(|&i| nf[i..i + nw.len()] == nw[..]).take(2).collect();
    let [at] = hits.as_slice() else { return None };
    let region = &file[*at..*at + want.len()];
    if !region.iter().any(|c| ascii(*c).is_some()) {
        return None;
    }
    // Text new_string kept from old_string takes the file's characters; added text is as given.
    let new_chars: Vec<char> = new.chars().collect();
    let ops = similar::capture_diff_slices_deadline(
        similar::Algorithm::Myers,
        &want,
        &new_chars,
        Some(std::time::Instant::now() + std::time::Duration::from_millis(50)),
    );
    let mut replacement = String::new();
    for op in &ops {
        match op.tag() {
            similar::DiffTag::Equal => replacement.extend(&region[op.old_range()]),
            similar::DiffTag::Insert | similar::DiffTag::Replace => replacement.extend(&new_chars[op.new_range()]),
            similar::DiffTag::Delete => {}
        }
    }
    let mut kinds: Vec<char> = region.iter().copied().filter(|c| ascii(*c).is_some()).collect();
    kinds.sort();
    kinds.dedup();
    let chars = kinds.iter().map(|c| describe_char(*c)).collect::<Vec<_>>().join(", ");
    let before: String = file[..*at].iter().collect();
    let after: String = file[*at + want.len()..].iter().collect();
    let first_line = before.matches('\n').count() + 1;
    let last_line = first_line + region.iter().filter(|c| **c == '\n').count();
    let out = format!("{before}{replacement}{after}");
    (out != content).then_some(NearMatch { content: out, kind: NearKind::Typographic { chars }, first_line, last_line })
}

fn describe_char(c: char) -> String {
    match ascii(c) {
        Some(' ') => format!("U+{:04X} (a non-ASCII space)", c as u32),
        Some(a) => format!("{c} for {a}"),
        None => c.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_offsets_are_applied_in_both_directions() {
        let file = "a:\n    b\n        c\n";
        let m = find(file, "  b\n      c", "  b\n      c\n      d").unwrap();
        assert_eq!(m.content, "a:\n    b\n        c\n        d\n");
        assert_eq!(m.kind, NearKind::Indentation { how: "2 more spaces".into() });
        // Too much indentation in old_string is mapped down too.
        let m = find(file, "      b\n          c", "      B\n          c").unwrap();
        assert_eq!(m.content, "a:\n    B\n        c\n");
    }

    #[test]
    fn indentation_that_is_off_by_different_amounts_is_not_guessed() {
        let file = "a:\n    b\n        c\n";
        assert!(find(file, "  b\n        c", "  b\n        C").is_none());
    }

    #[test]
    fn tabs_onto_a_space_indented_file() {
        let file = "def f():\n    if x:\n        return 1\n";
        let m = find(file, "\tif x:\n\t\treturn 1", "\tif x:\n\t\treturn 2").unwrap();
        assert_eq!(m.content, "def f():\n    if x:\n        return 2\n");
    }

    #[test]
    fn two_places_or_none_mean_no_near_match() {
        assert!(find("x = 1  \nx = 1\t\n", "x = 1", "x = 2").is_none());
        assert!(find("nothing like it\n", "x = 1", "x = 2").is_none());
    }

    #[test]
    fn typographic_text_kept_from_old_string_keeps_the_files_characters() {
        let file = "say(\u{201C}it\u{2019}s done\u{201D})\n";
        let m = find(file, "say(\"it's done\")", "say(\"it's all done\")").unwrap();
        assert_eq!(m.content, "say(\u{201C}it\u{2019}s all done\u{201D})\n");
        assert!(m.note().contains("typographic") && m.note().contains("line 1"), "{}", m.note());
    }
}
