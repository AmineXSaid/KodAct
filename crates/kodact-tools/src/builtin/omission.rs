//! Lines that stand for text left out ("// ... rest of the code ..."). A model that writes one
//! into a file in place of the code it stands for deletes that code, and nothing fails until
//! something runs. Write and Edit refuse such a call once, naming the line; the same call sent
//! again goes through, for the file that really holds such a line (a tutorial, a template).
//!
//! The words are English: a placeholder in another language is not recognised, and nothing is
//! said.

/// What a placeholder starts with, once the comment marks and the dots are gone.
const STARTS: &[&str] = &[
    "rest of",
    "the rest",
    "remaining",
    "remainder",
    "existing",
    "previous",
    "unchanged",
    "same as",
    "as before",
    "keep ",
    "omitted",
    "other methods",
    "other functions",
    "more code",
    "code here",
    "your code",
    "implementation here",
];
/// Or holds anywhere.
const HOLDS: &[&str] = &["unchanged", "omitted", "as before", "remains the same", "stays the same", "for brevity"];

/// The placeholder a line is, if it is one: only a comment or bare text, with an ellipsis, a
/// few words long, saying that something is left out.
fn placeholder(line: &str) -> Option<&str> {
    let shown = line.trim();
    if !(shown.contains("...") || shown.contains('…')) {
        return None;
    }
    let mut t = shown;
    for lead in ["//", "/*", "<!--", "{/*", "#", "--", ";", "%", "*", "\"\"\"", "'''"] {
        if let Some(rest) = t.strip_prefix(lead) {
            t = rest.trim_start();
            break;
        }
    }
    for tail in ["*/}", "*/", "-->", "\"\"\"", "'''"] {
        if let Some(rest) = t.strip_suffix(tail) {
            t = rest.trim_end();
            break;
        }
    }
    let words: String = t
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '\'' { c.to_ascii_lowercase() } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    // Code with an ellipsis in it (`def f(*args): ...`, `[1, ..., n]`, a spread) is not one:
    // what is left of a placeholder is a few plain words and the dots.
    let plain = t.chars().all(|c| c.is_alphanumeric() || c.is_whitespace() || ".…,'()[]-:".contains(c));
    if !plain || words.is_empty() || words.split(' ').count() > 9 {
        return None;
    }
    let padded = format!("{words} ");
    (STARTS.iter().any(|s| padded.starts_with(s)) || HOLDS.iter().any(|h| words.contains(h))).then_some(shown)
}

/// The first placeholder line `new` would add: one that `old` (the text being replaced, or
/// the file's content before a Write) does not already hold.
pub fn added_placeholder<'a>(old: &str, new: &'a str) -> Option<(usize, &'a str)> {
    new.lines()
        .enumerate()
        .find_map(|(i, l)| placeholder(l).filter(|p| !old.lines().any(|o| o.trim() == *p)).map(|p| (i + 1, p)))
}

/// The refusal for a call whose `field` would add a placeholder line.
pub fn refusal(field: &str, line: usize, text: &str) -> String {
    format!(
        "Nothing was written: line {line} of `{field}` is `{text}`, which would go into the file as it is, in \
         place of the text it stands for. Send the complete text. If the file is meant to hold this line, send \
         the same call again and it will be written."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_told_from_code_and_prose() {
        for yes in [
            "    // ... rest of the code ...",
            "# ... existing code ...",
            "/* ... unchanged ... */",
            "<!-- … remaining items … -->",
            "{/* ... rest of component ... */}",
            "    ... (rest of file unchanged)",
            "// Rest of methods...",
            "-- ... other functions omitted for brevity ...",
        ] {
            assert!(placeholder(yes).is_some(), "{yes}");
        }
        for no in [
            "def handler(*args): ...",
            "    ...",
            "const all = [...existing, item];",
            "// Loading...",
            "# Wait for the remaining workers to exit before closing the pool.",
            "print(\"... rest of the output ...\")",
            "// The rest of this file is generated; see tools/gen.py for how to change it...",
            "x = values[..., 0]",
            "// ... et le reste du code ...",
        ] {
            assert!(placeholder(no).is_none(), "{no}");
        }
        // A line the replaced text already had is not added by the call.
        assert_eq!(added_placeholder("a\n# ... existing code ...\n", "b\n  # ... existing code ...\n"), None);
        assert_eq!(added_placeholder("a\n", "b\n# ... existing code ...\n"), Some((2, "# ... existing code ...")));
    }
}
