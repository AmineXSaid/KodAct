//! What a person's request names: the files and the commands in it, and a size limit stated
//! for a file. Read from the request's own words, nothing else.
//!
//! - A **file** is a word with a path separator (`src/main.rs`, `./out/report.json`) or a name
//!   with an extension (`answer.txt`). URLs, e-mail addresses, version numbers (`1.4.0`) and
//!   abbreviations (`e.g.`) are not files.
//! - A **command** is a span in backticks whose first word is a program this machine has
//!   (on `PATH`) or a script in the project (`./build.sh`, `python3 run_tests.py`).
//! - A **size limit** is a size with its unit (`40 KB`, `2048 bytes`) after "at most", "under",
//!   "no more than", "less than", "smaller than", "below", "max", "maximum", "limit" or
//!   `<`/`<=`/`≤`, in the same sentence as exactly one named file.
//!
//! A sentence that says not to do something ("don't run `make deploy`", "never touch
//! secrets.env") names nothing: what it names is not asked for.

use std::path::{Path, PathBuf};

/// The files, commands and size limits a request names.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Named {
    pub files: Vec<String>,
    pub commands: Vec<String>,
    /// A file and the most bytes the request allows it, with the words that said so.
    pub limits: Vec<(String, u64, String)>,
    /// Files named in a sentence that asks to delete or remove something.
    pub removed: Vec<String>,
}

/// Words that ask for a file to be gone: what such a sentence names is not missing when it is.
const REMOVALS: [&str; 5] = ["delete", "remove", "rm ", "get rid of", "clean up"];
/// Words that turn a sentence into a prohibition.
const NEGATIONS: [&str; 6] = ["don't", "do not", "never", "must not", "should not", "shouldn't"];
/// Words before a size that make it a limit.
const LIMIT_WORDS: [&str; 11] =
    ["at most", "under", "no more than", "less than", "smaller than", "below", "maximum", "max", "limit", "<=", "≤"];

impl Named {
    pub fn from_request(text: &str, project: &Path) -> Named {
        let mut n = Named::default();
        for sentence in sentences(text) {
            let lower = sentence.to_lowercase();
            if NEGATIONS.iter().any(|w| lower.contains(w)) {
                continue;
            }
            // In the order the sentence names them: backticked spans and plain words alike.
            let mut files_here = vec![];
            for (i, part) in sentence.split('`').enumerate() {
                if i % 2 == 1 {
                    let span = part.trim().to_string();
                    if let Some(cmd) = as_command(&span, project) {
                        push(&mut n.commands, cmd);
                    } else if is_file_word(&span) {
                        push(&mut files_here, span);
                    }
                    continue;
                }
                for word in part.split_whitespace() {
                    let w = trim_word(word);
                    if is_file_word(w) {
                        push(&mut files_here, w.to_string());
                    }
                }
            }
            if let ([file], Some((bytes, said))) = (files_here.as_slice(), size_limit(&sentence)) {
                n.limits.push((file.clone(), bytes, said));
            }
            let removing = REMOVALS.iter().any(|w| lower.contains(w));
            for f in files_here {
                if removing {
                    push(&mut n.removed, f);
                } else {
                    push(&mut n.files, f);
                }
            }
        }
        n
    }

    pub fn names_files(&self) -> bool {
        !self.files.is_empty()
    }

    /// Named files that don't exist now (relative to the project).
    pub fn missing_files(&self, project: &Path) -> Vec<String> {
        self.files.iter().filter(|f| !self.removed.contains(f) && !resolve(f, project).exists()).cloned().collect()
    }

    /// Named commands no command run this turn contains.
    pub fn unrun_commands(&self, ran: &[String]) -> Vec<String> {
        let ran: Vec<String> = ran.iter().map(|r| squash(r)).collect();
        self.commands.iter().filter(|c| !ran.iter().any(|r| r.contains(&squash(c)))).cloned().collect()
    }

    /// Named files over the limit the request states for them: (file, size, limit, the words).
    pub fn over_limit(&self, project: &Path) -> Vec<(String, u64, u64, String)> {
        self.limits
            .iter()
            .filter_map(|(f, max, said)| {
                let size = std::fs::metadata(resolve(f, project)).ok()?.len();
                (size > *max).then(|| (f.clone(), size, *max, said.clone()))
            })
            .collect()
    }
}

/// What the model is told before the turn ends, when the request names something that isn't
/// there: `None` when everything it names is.
pub(crate) fn report(n: &Named, project: &Path, ran: &[String]) -> Option<(String, serde_json::Value)> {
    let missing = n.missing_files(project);
    let unrun = n.unrun_commands(ran);
    let over = n.over_limit(project);
    if missing.is_empty() && unrun.is_empty() && over.is_empty() {
        return None;
    }
    let ticks = |v: &[String]| v.iter().map(|x| format!("`{x}`")).collect::<Vec<_>>().join(", ");
    let mut lines = vec!["Before you finish, compare with the request:".to_string()];
    if !missing.is_empty() {
        lines.push(format!("- It names files that don't exist: {}.", ticks(&missing)));
    }
    if !unrun.is_empty() {
        lines.push(format!("- It names commands that didn't run in this turn: {}.", ticks(&unrun)));
    }
    for (f, size, max, said) in &over {
        lines.push(format!("- It asks for `{f}` {said}; the file is {size} bytes (the limit read as {max} bytes)."));
    }
    lines.push(
        "If the request needs them, do it now. If a name was only an example, or something is meant to be \
         missing, say so in your answer."
            .into(),
    );
    let record = serde_json::json!({
        "kind": crate::progress::Nudge::Constraints.as_str(),
        "missing_files": missing,
        "unrun_commands": unrun,
        "over_limit": over.iter().map(|(f, size, max, _)| serde_json::json!({"file": f, "size": size, "limit": max})).collect::<Vec<_>>(),
    });
    Some((format!("<system-reminder>\n{}\n</system-reminder>", lines.join("\n")), record))
}

fn push(v: &mut Vec<String>, s: String) {
    if !v.contains(&s) {
        v.push(s);
    }
}

fn resolve(f: &str, project: &Path) -> PathBuf {
    let p = Path::new(f);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        project.join(p)
    }
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Sentences, split at `.`/`!`/`?` followed by a space or the end, and at line breaks; a dot
/// inside a word (`main.py`, `1.5`) does not end one.
fn sentences(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut in_ticks = false;
    for (i, &c) in chars.iter().enumerate() {
        if c == '`' {
            in_ticks = !in_ticks;
        }
        let next = chars.get(i + 1).copied();
        let ends = !in_ticks && (c == '\n' || (matches!(c, '.' | '!' | '?') && next.is_none_or(char::is_whitespace)));
        if c != '\n' {
            cur.push(c);
        }
        if ends && !cur.trim().is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// A word without the punctuation around it in a sentence.
fn trim_word(w: &str) -> &str {
    w.trim_matches(|c: char| matches!(c, '"' | '\'' | '(' | ')' | '[' | ']' | ',' | ';' | ':' | '!' | '?' | '*'))
        .trim_end_matches('.')
}

fn is_file_word(w: &str) -> bool {
    if w.is_empty() || w.contains(char::is_whitespace) || w.contains("://") || w.contains('@') || w.starts_with('-') {
        return false;
    }
    if w.starts_with("www.") || w.chars().all(|c| c == '/' || c == '.') {
        return false;
    }
    let name = w.rsplit('/').next().unwrap_or(w);
    if w.contains('/') {
        // A path: at least one part with a letter or a digit, and no spaces.
        return w.chars().any(|c| c.is_ascii_alphanumeric()) && !w.starts_with("//");
    }
    // A name with an extension: a stem with a letter, an extension of letters and digits that
    // isn't all digits (`1.4.0`, `3.5`), and not an abbreviation (`e.g`, `i.e`).
    let Some((stem, ext)) = name.rsplit_once('.') else { return false };
    let stem_ok = stem.chars().any(|c| c.is_ascii_alphabetic()) && !stem.split('.').all(|p| p.len() <= 1);
    let ext_ok = (1..=8).contains(&ext.len())
        && ext.chars().all(|c| c.is_ascii_alphanumeric())
        && !ext.chars().all(|c| c.is_ascii_digit());
    stem_ok && ext_ok && stem.chars().all(|c| c.is_ascii_alphanumeric() || "._-+".contains(c))
}

/// A backticked span that runs something here: its first word (after `VAR=value`s) is a program
/// on `PATH` or a file in the project.
fn as_command(span: &str, project: &Path) -> Option<String> {
    let words: Vec<&str> = span.split_whitespace().collect();
    let first = words.iter().find(|w| !(w.contains('=') && !w.starts_with('-') && !w.starts_with('/')))?;
    let ok = if first.contains('/') { resolve(first, project).is_file() } else { on_path(first) };
    ok.then(|| squash(span))
}

fn on_path(program: &str) -> bool {
    if program.is_empty() || !program.chars().all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c)) {
        return false;
    }
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|d| d.join(program).is_file()))
        .unwrap_or(false)
}

/// A size limit stated in a sentence: (bytes, the words that state it).
fn size_limit(sentence: &str) -> Option<(u64, String)> {
    let lower = sentence.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        let w = w.trim_matches(|c: char| matches!(c, '(' | ')' | ',' | ';' | ':'));
        // `40kb`, `40 kb`, `2048 bytes`.
        let digits: String = w.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        if digits.is_empty() {
            continue;
        }
        let rest = &w[digits.len()..];
        let unit =
            if rest.is_empty() { words.get(i + 1).map(|u| u.trim_end_matches(['.', ',', ';'])) } else { Some(rest) };
        let mult = match unit? {
            "b" | "byte" | "bytes" => 1.0,
            "kb" | "k" => 1_000.0,
            "kib" => 1_024.0,
            "mb" | "m" => 1_000_000.0,
            "mib" => 1_048_576.0,
            _ => continue,
        };
        // The limit word must come just before the size: within its last three words.
        let from = i.saturating_sub(3);
        let before = words[from..i].join(" ");
        let Some(at) = LIMIT_WORDS.iter().filter_map(|l| before.rfind(l)).max() else { continue };
        let n: f64 = digits.parse().ok()?;
        let end = if rest.is_empty() { i + 1 } else { i };
        let said = format!("{} {}", &before[at..], words[i..=end.min(words.len() - 1)].join(" "));
        let said = said.trim().trim_end_matches(['.', ',', ';']).to_string();
        return Some(((n * mult).round() as u64, said));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(text: &str) -> Named {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("build.sh"), "").unwrap();
        Named::from_request(text, d.path())
    }

    #[test]
    fn files_are_paths_and_names_with_extensions() {
        let n = named(
            "Fix src/parser.rs so `cargo test` passes, then write the result to answer.txt. See e.g. the README.",
        );
        assert_eq!(n.files, ["src/parser.rs", "answer.txt"]);
        let n = named("Upgrade to 1.4.0 and read https://example.com/guide.html or mail me@host.org (v2.5 notes).");
        assert!(n.files.is_empty(), "{:?}", n.files);
        let n = named("Write \"out/summary.json\", then copy it to `./dist/`.");
        assert_eq!(n.files, ["out/summary.json", "./dist/"]);
    }

    #[test]
    fn commands_are_backticked_spans_that_run_something_here() {
        let n =
            named("Make `sh ./build.sh` pass, then run `./build.sh --release` and `nosuchprogram123 x`. Use `x = 5`.");
        assert_eq!(n.commands, ["sh ./build.sh", "./build.sh --release"]);
        let n = named("Run `FOO=1 sh -c true`.");
        assert_eq!(n.commands, ["FOO=1 sh -c true"]);
    }

    #[test]
    fn a_prohibition_names_nothing() {
        let n = named("Fix app.py. Don't run `sh ./build.sh` and never touch secrets.env. Do not change tests.py!");
        assert_eq!((n.files.as_slice(), n.commands.len()), (["app.py".to_string()].as_slice(), 0));
    }

    #[test]
    fn a_size_limit_belongs_to_the_one_file_in_its_sentence() {
        let n = named("Compress the log into out.bin, at most 40 KB. Keep notes.md short.");
        assert_eq!(n.limits, [("out.bin".to_string(), 40_000, "at most 40 kb".to_string())]);
        let n = named("Write image.png under 2048 bytes.");
        assert_eq!(n.limits[0].1, 2048);
        // Two files in the sentence: whose limit it is isn't clear, so none.
        assert!(named("Make a.txt and b.txt at most 1 KB.").limits.is_empty());
        // A size that isn't a limit.
        assert!(named("The file data.csv is 40 KB.").limits.is_empty());
    }

    #[test]
    fn what_is_missing_unrun_and_too_large() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("there.txt"), "x".repeat(50)).unwrap();
        let n = Named::from_request(
            "Update there.txt to at most 10 bytes. Write gone.txt, and run `sh -c true`.",
            d.path(),
        );
        assert_eq!(n.missing_files(d.path()), ["gone.txt"]);
        assert_eq!(n.unrun_commands(&[]), ["sh -c true"]);
        assert!(n.unrun_commands(&["cd x &&  sh   -c true -v".to_string()]).is_empty(), "contained, spaces aside");
        assert_eq!(n.over_limit(d.path()), [("there.txt".to_string(), 50, 10, "at most 10 bytes".to_string())]);
    }

    #[test]
    fn a_file_asked_to_be_removed_is_not_missing() {
        let d = tempfile::tempdir().unwrap();
        let n =
            Named::from_request("Move config.ini to settings.toml and delete config.ini. Write notes.md.", d.path());
        assert_eq!(n.missing_files(d.path()), ["notes.md"]);
        assert_eq!(n.removed, ["config.ini", "settings.toml"]);
    }
}
