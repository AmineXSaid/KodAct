//! What a person's request names: the files and the commands in it, and a size limit stated
//! for a file. Read from the request's own words, nothing else.
//!
//! - A **file** is a word with a path separator (`src/main.rs`, `./out/report.json`) or a name
//!   with an extension (`answer.txt`). URLs, e-mail addresses, version numbers (`1.4.0`) and
//!   abbreviations (`e.g.`) are not files.
//! - A **command** is a span in backticks whose first word is a program this machine has
//!   (on `PATH`) or a script in the project (`./build.sh`, `python3 run_tests.py`).
//! - A **size limit** is a size with its unit (`40 KB`, `2048 bytes`, `<5000 bytes`) after "at
//!   most", "under", "no more than", "less than", "smaller than", "below", "max", "maximum",
//!   "limit" or `<`/`<=`/`≤`. "Under", "less than", "smaller than", "below" and `<` are strict
//!   (`<5000 bytes`: 4,999 at most), as a test's `size < 5000` is. The limit is the file's that
//!   its sentence names, when it names exactly one. A sentence that names no file ("your
//!   program must be <5000 bytes") sets the limit of the one file the request asks for that
//!   doesn't exist yet, or, of several, of the one whose type the sentence names ("c program":
//!   `gpt2.c`; "the Python script": `run.py`).
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
const LIMIT_WORDS: [&str; 12] = [
    "at most",
    "under",
    "no more than",
    "less than",
    "smaller than",
    "below",
    "maximum",
    "max",
    "limit",
    "<=",
    "≤",
    "<",
];

impl Named {
    pub fn from_request(text: &str, project: &Path) -> Named {
        let mut n = Named::default();
        // Limits stated without a file: (the file types their sentence names, bytes, words).
        let mut unattached: Vec<(Vec<&'static str>, u64, String)> = vec![];
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
            match (files_here.as_slice(), size_limit(&sentence)) {
                ([file], Some((bytes, said))) => n.limits.push((file.clone(), bytes, said)),
                ([], Some((bytes, said))) => unattached.push((type_words(&lower), bytes, said)),
                _ => {}
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
        // A limit without a file: the one new file the request asks for, or the one of its
        // type. Files there already are what the request reads, not what it asks to be made.
        let new: Vec<&String> =
            n.files.iter().filter(|f| !n.removed.contains(f) && !resolve(f, project).exists()).collect();
        for (types, bytes, said) in unattached {
            let of_type: Vec<&&String> = new.iter().filter(|f| types.iter().any(|t| *t == extension(f))).collect();
            let file = match (new.as_slice(), of_type.as_slice()) {
                ([one], _) => Some((*one).clone()),
                (_, [one]) => Some((**one).clone()),
                _ => None,
            };
            if let Some(f) = file.filter(|f| !n.limits.iter().any(|(g, ..)| g == f)) {
                n.limits.push((f, bytes, said));
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

/// A file as it is on disk: its size and modification time.
type FileState = (u64, Option<std::time::SystemTime>);

/// A file as it is on disk, or `None` when there is none.
fn file_state(p: &Path) -> Option<FileState> {
    let m = std::fs::metadata(p).ok().filter(|m| m.is_file())?;
    Some((m.len(), m.modified().ok()))
}

/// The files a request names, watched on disk through a turn: what counts as progress.
///
/// The files that don't exist when the turn starts are what the request asks to be made, its
/// deliverables. When there are some, only a change to a named file is progress: helper
/// scripts and scratch files are work towards them, not them. A change is seen on disk, so a
/// script that writes a deliverable counts as much as an edit tool does. When every named file
/// already exists (a request to fix `parser.py`), a change can rightly land in a file the
/// request didn't name, and any file written counts.
#[derive(Debug, Clone, Default)]
pub(crate) struct Watched {
    files: Vec<(String, PathBuf, Option<FileState>)>,
    asks_for_new: bool,
}

impl Watched {
    pub fn at_start(n: &Named, project: &Path) -> Watched {
        let files: Vec<_> = n
            .files
            .iter()
            .map(|f| {
                let p = resolve(f, project);
                (f.clone(), file_state(&p), p)
            })
            .filter(|(_, _, p)| !p.is_dir())
            .map(|(f, st, p)| (f, p, st))
            .collect();
        let asks_for_new = files.iter().any(|(_, _, st)| st.is_none());
        Watched { files, asks_for_new }
    }

    /// The request names files that didn't exist when the turn started.
    pub fn asks_for_new(&self) -> bool {
        self.asks_for_new
    }

    /// Named files that changed since the last look (created, rewritten, removed).
    pub fn changed(&mut self) -> Vec<String> {
        let mut out = vec![];
        for (name, path, st) in &mut self.files {
            let now = file_state(path);
            if now != *st {
                *st = now;
                out.push(name.clone());
            }
        }
        out
    }

    /// Does a command name one of the files (by its path, or its file name)?
    pub fn named_in(&self, command: &str) -> bool {
        self.files.iter().any(|(n, p, _)| {
            command.contains(n.as_str())
                || p.file_name().is_some_and(|f| command.contains(f.to_string_lossy().as_ref()))
        })
    }

    /// The files a row of limits is about: those whose words the row's label carries
    /// (`north` for `routes_north.csv`), else all of them.
    pub fn names_for(&self, row: Option<&str>) -> Vec<String> {
        let words = |s: &str| -> Vec<String> {
            s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 1).map(String::from).collect()
        };
        let all: Vec<String> = self.files.iter().map(|(n, ..)| n.clone()).collect();
        let Some(row) = row else { return all };
        let row = words(row);
        let some: Vec<String> = all
            .iter()
            .filter(|n| {
                let mine = words(n);
                let own: Vec<&String> = mine.iter().filter(|w| !all.iter().all(|o| words(o).contains(w))).collect();
                !own.is_empty() && own.iter().all(|w| row.contains(w))
            })
            .cloned()
            .collect();
        if some.is_empty() {
            all
        } else {
            some
        }
    }

    /// Of the files the request asks to be made, those that still don't exist.
    pub fn still_missing(&self) -> Vec<String> {
        self.files.iter().filter(|(_, p, _)| !p.exists()).map(|(n, ..)| n.clone()).collect()
    }
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

/// A size limit stated in a sentence: (the most bytes it allows, the words that state it).
fn size_limit(sentence: &str) -> Option<(u64, String)> {
    let lower = sentence.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        let w = w.trim_matches(|c: char| matches!(c, '(' | ')' | ',' | ';' | ':'));
        // A sign stuck to the size: `<5000`, `<=4kb`, `≤2048`.
        let (sign, w) = match ["<=", "≤", "<"].iter().find_map(|s| w.strip_prefix(s).map(|r| (*s, r))) {
            Some((s, rest)) => (Some(s), rest),
            None => (None, w),
        };
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
        let end = if rest.is_empty() { i + 1 } else { i };
        let size_words = words[i..=end.min(words.len() - 1)].join(" ");
        // The limit word: stuck to the size, or within the three words before it (the last
        // one there, and of two that end at the same place, the longer: `<=` over `<`).
        let (word, said) = match sign {
            Some(s) => (s, size_words),
            None => {
                let from = i.saturating_sub(3);
                let before = words[from..i].join(" ");
                let Some((at, word)) = LIMIT_WORDS
                    .iter()
                    .filter_map(|l| before.rfind(l).map(|at| (at, *l)))
                    .max_by_key(|(at, l)| (at + l.len(), l.len()))
                else {
                    continue;
                };
                (word, format!("{} {size_words}", &before[at..]))
            }
        };
        let n: f64 = digits.parse().ok()?;
        let bytes = (n * mult).round() as u64;
        let strict = matches!(word, "<" | "under" | "less than" | "smaller than" | "below");
        let said = said.trim().trim_end_matches(['.', ',', ';']).to_string();
        return Some((if strict { bytes.saturating_sub(1) } else { bytes }, said));
    }
    None
}

/// The file types a sentence names, as extensions: a language or format by name ("c program",
/// "the Python script", "a JSON file"), or the extension itself ("the .csv", "a png").
fn type_words(lower: &str) -> Vec<&'static str> {
    const TYPES: [(&str, &[&str]); 24] = [
        ("c", &["c", "h"]),
        ("c++", &["cpp", "cc", "cxx", "hpp"]),
        ("cpp", &["cpp", "cc", "cxx", "hpp"]),
        ("python", &["py"]),
        ("py", &["py"]),
        ("javascript", &["js", "mjs", "cjs"]),
        ("js", &["js", "mjs", "cjs"]),
        ("typescript", &["ts"]),
        ("rust", &["rs"]),
        ("go", &["go"]),
        ("java", &["java"]),
        ("ruby", &["rb"]),
        ("shell", &["sh"]),
        ("bash", &["sh"]),
        ("json", &["json"]),
        ("yaml", &["yaml", "yml"]),
        ("toml", &["toml"]),
        ("csv", &["csv"]),
        ("html", &["html", "htm"]),
        ("css", &["css"]),
        ("markdown", &["md"]),
        ("sql", &["sql"]),
        ("xml", &["xml"]),
        ("text", &["txt"]),
    ];
    let mut out = vec![];
    for w in lower.split_whitespace().map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '+')) {
        let w = w.trim_start_matches('.');
        for (name, exts) in TYPES {
            if w == name {
                out.extend_from_slice(exts);
            }
        }
    }
    out
}

/// A file's extension, lower case (`gpt2.c` → `c`), or "".
fn extension(f: &str) -> String {
    f.rsplit('/').next().unwrap_or(f).rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
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
        // "Under" is strict, as a test's `size < 2048` is: 2047 at most.
        let n = named("Write image.png under 2048 bytes.");
        assert_eq!(n.limits[0].1, 2047);
        // Two files in the sentence: whose limit it is isn't clear, so none.
        assert!(named("Make a.txt and b.txt at most 1 KB.").limits.is_empty());
        // A size that isn't a limit.
        assert!(named("The file data.csv is 40 KB.").limits.is_empty());
    }

    #[test]
    fn a_limit_without_a_file_belongs_to_the_new_file_it_is_about() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("weights.bin"), "").unwrap();
        let read = |t: &str| Named::from_request(t, d.path()).limits;
        // `<` stuck to the size, strict; the sentence names no file, the request one new one.
        assert_eq!(
            read("Write solve.c that reads weights.bin. Your program must be <5000 bytes."),
            [("solve.c".to_string(), 4999, "<5000 bytes".to_string())]
        );
        // Two new files: the one of the type the sentence names.
        let two =
            "Call your program /app/gpt.c; I will run /app/a.out weights.bin. Your c program must be <5000 bytes.";
        assert_eq!(read(two), [("/app/gpt.c".to_string(), 4999, "<5000 bytes".to_string())]);
        assert_eq!(
            read("Write run.py and notes.md. Keep the Python script under 2 KB.")[0],
            ("run.py".to_string(), 1999, "under 2 kb".to_string())
        );
        // Neither one new file nor one of the type: whose limit it is isn't clear, so none.
        assert!(read("Write a.py and b.py. Each script must be at most 1 KB.").is_empty());
        // A file there already is read, not made: no limit for it from a sentence that names none.
        assert!(read("Use weights.bin. The output must be at most 10 bytes.").is_empty());
        // Inclusive words stay inclusive; `<=` wins over `<` at the same place.
        assert_eq!(read("Write out.txt. It must be <= 100 bytes.")[0].1, 100);
        assert_eq!(read("Write out.txt. It must be < 100 bytes.")[0].1, 99);
        assert_eq!(read("Write out.txt, at most 100 bytes.")[0].1, 100);
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

    #[test]
    fn deliverables_are_the_named_files_missing_at_the_start() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::write(root.join("cost_model.py"), "x").unwrap();
        std::fs::create_dir(root.join("out")).unwrap();
        let n = Named::from_request("Use cost_model.py to write out/plan_a.jsonl and out/plan_b.jsonl.", root);
        let mut w = Watched::at_start(&n, root);
        assert!(w.asks_for_new());
        assert_eq!(w.still_missing(), ["out/plan_a.jsonl", "out/plan_b.jsonl"]);
        std::fs::write(root.join("helper.py"), "scratch").unwrap();
        assert!(w.changed().is_empty(), "a helper script is not a named file");
        // Written by a script, not an edit tool: seen on disk all the same.
        std::fs::write(root.join("out/plan_a.jsonl"), "{}").unwrap();
        assert_eq!(w.changed(), ["out/plan_a.jsonl"]);
        assert!(w.changed().is_empty(), "seen once");
        assert_eq!(w.still_missing(), ["out/plan_b.jsonl"]);
        // Every named file exists: the request asks for nothing new.
        let n = Named::from_request("Fix cost_model.py.", root);
        assert!(!Watched::at_start(&n, root).asks_for_new());
    }
}
