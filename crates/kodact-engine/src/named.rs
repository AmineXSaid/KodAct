//! What a person's request names: the files and the commands in it, and a size limit stated
//! for a file. Read from the request's own words, nothing else.
//!
//! - A **file** is a path (`src/main.rs`, `./out/report.json`, `dist/`) or a name with an
//!   extension (`answer.txt`). A word with a slash is a path when it says so: it is there, starts
//!   at a root, ends with `/`, or ends with a file's name. Words joined by a slash (`and/or`,
//!   `input/output`), URLs, e-mail addresses, version numbers (`1.4.0`) and abbreviations
//!   (`e.g.`) are not files. A name alone (`parser.py`) that isn't at the project's root is the
//!   file of that name elsewhere in the project (`src/parser.py`), when there is one.
//! - A **command** is a span in backticks whose first word is a program this machine has
//!   (on `PATH`) or a script in the project (`./build.sh`, `python3 run_tests.py`), and that
//!   the request asks to run or to pass: "run `make test`", "make `make test` pass", "verify
//!   with `pytest -q`". A command only mentioned ("see `make deploy` for releases") is not asked
//!   for: reading a request is a guess, and it acts on what the request says, never on a word
//!   it lacks (docs/goal.md, rule 10).
//! - A **size limit** is a size with its unit (`40 KB`, `2048 bytes`, `<5000 bytes`) after "at
//!   most", "under", "no more than", "less than", "smaller than", "below", "max", "maximum",
//!   "limit" or `<`/`<=`/`≤`. "Under", "less than", "smaller than", "below" and `<` are strict
//!   (`<5000 bytes`: 4,999 at most), as a test's `size < 5000` is. The limit is the file's that
//!   its sentence names, when it names exactly one. A sentence that names no file ("your
//!   program must be <5000 bytes") sets the limit of the one file the request asks for that
//!   doesn't exist yet, or, of several, of the one whose type the sentence names ("c program":
//!   `model.c`; "the Python script": `run.py`).
//!
//! A sentence that says not to do something, or sets something aside ("don't run `make
//! deploy`", "never touch secrets.env", "avoid `git push`", "use X instead of Y", "without
//! `sudo`"), names nothing: what it names is not asked for. A request that isn't in English
//! names nothing at all: these words are English, and in another language a prohibition would
//! be missed, so the reading stays silent rather than guess.

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

/// Words that turn a sentence into a prohibition, or set aside what it names: whole words,
/// in any case.
const PROHIBITIONS: [&str; 17] = [
    "don't",
    "dont",
    "do not",
    "never",
    "not",
    "must not",
    "mustn't",
    "shouldn't",
    "cannot",
    "can't",
    "won't",
    "avoid",
    "avoiding",
    "without",
    "instead of",
    "except",
    "skip",
];
/// Words just before a command that ask to run it ("run `make test`", "then execute `./x.sh`").
const RUN_WORDS: [&str; 8] = ["run", "rerun", "re-run", "execute", "invoke", "launch", "start", "call"];
/// Words just after a command that ask for it to pass ("make `make test` pass", "`npm test` must
/// succeed").
const PASS_WORDS: [&str; 7] = ["pass", "passes", "succeed", "succeeds", "work", "works", "green"];
/// Words just after a command that say what it does: a request to run it when the request
/// wants it so ("so that `x 1994` prints `MCMXCIV`", "`x` must print ..."), a description
/// otherwise ("`route.py <path>` prints the backend").
const EXPECT_WORDS: [&str; 10] =
    ["prints", "print", "outputs", "output", "returns", "return", "exits", "exit", "shows", "writes"];
/// Words just after a command that say it fails: of a check, a request to make it pass.
const FAIL_WORDS: [&str; 7] = ["fails", "fail", "failing", "failed", "errors", "breaks", "crashes"];
/// Words that make "with `x`" a request to run it, when one comes before it in the sentence
/// ("verify with `pytest -q`", "run the tests with `npm test`").
const CHECK_WORDS: [&str; 12] =
    ["run", "verify", "check", "test", "tests", "confirm", "validate", "build", "compile", "lint", "format", "execute"];
/// The words that introduce the command a check verb asks for.
const WITH_WORDS: [&str; 3] = ["with", "using", "via"];
/// Common English words of requests, to tell an English request from one in another language.
const ENGLISH: [&str; 96] = [
    "the", "a", "an", "to", "of", "in", "on", "for", "with", "and", "or", "so", "it", "its", "this", "that", "these",
    "those", "is", "are", "be", "was", "must", "should", "can", "will", "not", "from", "by", "at", "as", "if", "then",
    "than", "all", "each", "every", "only", "into", "out", "up", "my", "your", "me", "you", "we", "i", "what", "which",
    "how", "why", "where", "when", "does", "do", "there", "here", "please", "but", "after", "before", "until", "now",
    "new", "same", "fix", "write", "run", "make", "add", "create", "change", "update", "remove", "delete", "rename",
    "use", "using", "keep", "read", "find", "check", "test", "tests", "build", "move", "copy", "save", "set", "show",
    "print", "pass", "file", "most", "under", "least",
];
/// Common words of other languages that requests are written in: more of them than of
/// [`ENGLISH`] means the request isn't English.
const FOREIGN: [&str; 62] = [
    "le", "la", "les", "des", "du", "et", "est", "pas", "ne", "une", "pour", "dans", "avec", "sur", "que", "qui", "ce",
    "el", "los", "las", "del", "y", "es", "para", "con", "por", "una", "se", "lo", "al", "der", "die", "das", "und",
    "ist", "nicht", "mit", "für", "auf", "ein", "eine", "den", "dem", "zu", "von", "im", "o", "os", "não", "com", "um",
    "uma", "da", "il", "gli", "di", "e", "è", "che", "non", "si", "en",
];
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
        if !looks_english(text) {
            return n;
        }
        // Limits stated without a file: (the file types their sentence names, bytes, words).
        let mut unattached: Vec<(Vec<&'static str>, u64, String)> = vec![];
        for sentence in sentences(text) {
            let lower = sentence.to_lowercase().replace('\u{2019}', "'");
            if PROHIBITIONS.iter().any(|w| has_word(&lower, w)) {
                continue;
            }
            // In the order the sentence names them: backticked spans and plain words alike.
            let mut files_here = vec![];
            // The sentence's words so far, lower case: what comes just before a name.
            let mut before: Vec<String> = vec![];
            let tokens = tokens(&sentence);
            for (i, part) in sentence.split('`').enumerate() {
                if i % 2 == 1 {
                    let span = part.trim().to_string();
                    if let Some(cmd) = as_command(&span, project) {
                        // A check said to fail ("`make test` fails") is one to make pass.
                        let failing_check = said_to_fail(&tokens, i / 2) && crate::verify::named_check(&cmd, project);
                        if asked_to_run(&tokens, i / 2) || failing_check {
                            push(&mut n.commands, cmd);
                        }
                    } else if is_file_word(&span) && file_evidence(&span, project, &before, !files_here.is_empty()) {
                        push(&mut files_here, span.clone());
                    }
                    before.push(span.to_lowercase());
                    continue;
                }
                for word in part.split_whitespace() {
                    let w = trim_word(word);
                    if is_file_word(w) && file_evidence(w, project, &before, !files_here.is_empty()) {
                        push(&mut files_here, w.to_string());
                    }
                    before.push(w.to_lowercase());
                }
            }
            match (files_here.as_slice(), size_limit(&sentence)) {
                ([file], Some((bytes, said))) => n.limits.push((file.clone(), bytes, said)),
                ([], Some((bytes, said))) => unattached.push((type_words(&lower), bytes, said)),
                _ => {}
            }
            let removing = asks_to_remove(&lower);
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
        if unattached.is_empty() {
            return n;
        }
        let located = locate(&n.files, project);
        let new: Vec<&String> = located
            .iter()
            .filter(|(f, at)| !n.removed.contains(f) && !at.iter().any(|p| p.exists()))
            .map(|(f, _)| f)
            .collect();
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
/// there: `None` when everything it names is. `missing` are the files it asks to be made that
/// still don't exist ([`Watched::still_missing`]): a file it names that existed when the turn
/// started and is gone now was renamed or removed, as asked or not, and isn't said missing.
pub(crate) fn report(
    n: &Named,
    missing: Vec<String>,
    project: &Path,
    ran: &[String],
) -> Option<(String, serde_json::Value)> {
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
        "kind": crate::nudges::Nudge::Constraints.as_str(),
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
/// deliverables, except those named in a sentence that asks to remove something. When there
/// are some, only a change to a named file is progress: helper scripts and scratch files are
/// work towards them, not them. A change is seen on disk, so a script that writes a deliverable
/// counts as much as an edit tool does. When every named file already exists (a request to fix
/// `parser.py`), a change can rightly land in a file the request didn't name, and any file
/// written counts.
#[derive(Debug, Clone, Default)]
pub(crate) struct Watched {
    files: Vec<(String, PathBuf, Option<FileState>)>,
    /// The named files that didn't exist when the turn started, as the request names them.
    deliverables: Vec<String>,
}

impl Watched {
    /// A name alone found elsewhere in the project (`parser.py` for `src/parser.py`) is watched
    /// there, as a file the request reads: it is not one to make.
    pub fn at_start(n: &Named, project: &Path) -> Watched {
        let mut files = vec![];
        for (f, at) in locate(&n.files, project) {
            for p in at.into_iter().filter(|p| !p.is_dir()) {
                let st = file_state(&p);
                files.push((f.clone(), p, st));
            }
        }
        let mut deliverables = vec![];
        for (f, _, st) in &files {
            if st.is_none() && !n.removed.contains(f) {
                push(&mut deliverables, f.clone());
            }
        }
        Watched { files, deliverables }
    }

    /// The request names files that didn't exist when the turn started.
    pub fn asks_for_new(&self) -> bool {
        !self.deliverables.is_empty()
    }

    /// Named files that changed since the last look (created, rewritten, removed).
    pub fn changed(&mut self) -> Vec<String> {
        let mut out = vec![];
        for (name, path, st) in &mut self.files {
            let now = file_state(path);
            if now != *st {
                *st = now;
                push(&mut out, name.clone());
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
        let mut all: Vec<String> = vec![];
        for (n, ..) in &self.files {
            push(&mut all, n.clone());
        }
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

    /// Of the files the request asks to be made, those that still don't exist. A named file that
    /// was there when the turn started is never among them: gone now, it was renamed or removed
    /// (`Rename helper.py to utils.py` leaves no `helper.py`), not left unmade.
    pub fn still_missing(&self) -> Vec<String> {
        self.files
            .iter()
            .filter(|(n, p, _)| self.deliverables.contains(n) && !p.exists())
            .map(|(n, ..)| n.clone())
            .collect()
    }
}

/// A sentence's words in order, lower case and without the punctuation around them; a span in
/// backticks is one token, `None`, numbered by the order of the spans.
fn tokens(sentence: &str) -> Vec<Option<String>> {
    let mut out = vec![];
    for (i, part) in sentence.split('`').enumerate() {
        if i % 2 == 1 {
            out.push(None);
            continue;
        }
        for w in part.split_whitespace() {
            let w = w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '-').to_lowercase();
            if !w.is_empty() {
                out.push(Some(w));
            }
        }
    }
    out
}

/// Does the sentence ask to run (or pass) its `nth` backticked span: a run word among the
/// three words before it, a pass word among the three after it, "with" (`using`, `via`) just
/// before it after a word that runs or checks something, or what it must do after it ("prints",
/// "returns") where the sentence wants it so ("so that", "make sure", "must").
fn asked_to_run(tokens: &[Option<String>], nth: usize) -> bool {
    let Some(at) = span_at(tokens, nth) else { return false };
    let word = |t: &Option<String>, list: &[&str]| t.as_deref().is_some_and(|w| list.contains(&w));
    let before = &tokens[at.saturating_sub(3)..at];
    let after = &tokens[at + 1..(at + 4).min(tokens.len())];
    if before.iter().any(|t| word(t, &RUN_WORDS)) || after.iter().any(|t| word(t, &PASS_WORDS)) {
        return true;
    }
    if at > 0 && word(&tokens[at - 1], &WITH_WORDS) && tokens[..at - 1].iter().any(|t| word(t, &CHECK_WORDS)) {
        return true;
    }
    let pair = |a: &str, b: &str| tokens[..at].windows(2).any(|w| word(&w[0], &[a]) && word(&w[1], &[b]));
    let wanted = after.iter().any(|t| word(t, &["must", "should"]))
        || pair("so", "that")
        || pair("such", "that")
        || pair("make", "sure")
        || tokens[..at].iter().any(|t| word(t, &["ensure", "ensures"]));
    wanted && after.iter().any(|t| word(t, &EXPECT_WORDS))
}

/// Does the sentence say its `nth` backticked span fails: a fail word among the three after it.
fn said_to_fail(tokens: &[Option<String>], nth: usize) -> bool {
    let Some(at) = span_at(tokens, nth) else { return false };
    tokens[at + 1..(at + 4).min(tokens.len())].iter().any(|t| t.as_deref().is_some_and(|w| FAIL_WORDS.contains(&w)))
}

/// Where the `nth` backticked span is among a sentence's tokens.
fn span_at(tokens: &[Option<String>], nth: usize) -> Option<usize> {
    tokens.iter().enumerate().filter(|(_, t)| t.is_none()).map(|(i, _)| i).nth(nth)
}

/// Is `w` in `text` as a whole word (or phrase): no letter, digit or `_` right before or after.
/// Does a sentence ask for something to be gone? What it names is then not missing when it is.
/// By stem, so every form counts (`delete`, `deleted`, `deleting`, `deletion`; `remove`,
/// `removing`, `removal`; `clean up`, `cleaned up`, `cleanup`; `get rid of`, `got rid of`), and the
/// command `rm` only as a word of its own: `form of`, `platform` and `long-term` have its letters,
/// not its sense.
fn asks_to_remove(lower: &str) -> bool {
    let words: Vec<&str> = lower
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .collect();
    has_word(lower, "rid of")
        || words.iter().enumerate().any(|(i, w)| {
            w.starts_with("delet")
                || w.starts_with("remov")
                || *w == "rm"
                || *w == "cleanup"
                || *w == "clean-up"
                || (w.starts_with("clean") && words.get(i + 1) == Some(&"up"))
        })
}

fn has_word(text: &str, w: &str) -> bool {
    text.match_indices(w).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + w.len()..].chars().next();
        before.is_none_or(|c| !c.is_alphanumeric() && c != '_')
            && after.is_none_or(|c| !c.is_alphanumeric() && c != '_')
    })
}

/// Is the request in English: of its prose words (outside backticks, letters only), more are
/// common English words than common words of other languages, and at least a fifth are. A
/// request with no prose (paths and code alone) has nothing to misread.
fn looks_english(text: &str) -> bool {
    let mut prose = vec![];
    for (i, part) in text.split('`').enumerate() {
        if i % 2 == 1 {
            continue;
        }
        for w in part.split_whitespace() {
            let w = w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
            if !w.is_empty() && w.chars().all(char::is_alphabetic) {
                prose.push(w);
            }
        }
    }
    let english = prose.iter().filter(|w| ENGLISH.contains(&w.as_str())).count();
    let foreign = prose.iter().filter(|w| FOREIGN.contains(&w.as_str())).count();
    prose.is_empty() || (english > foreign && english * 5 >= prose.len())
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

/// Where each named file is: its path when that is there; for a name alone that isn't at the
/// project's root, the files of that name elsewhere in it ([`found_elsewhere`]); else its path,
/// still to be made.
fn locate(files: &[String], project: &Path) -> Vec<(String, Vec<PathBuf>)> {
    let alone: Vec<&str> =
        files.iter().filter(|f| !f.contains('/') && !resolve(f, project).exists()).map(String::as_str).collect();
    let elsewhere = found_elsewhere(&alone, project);
    files
        .iter()
        .map(|f| {
            let there: Vec<PathBuf> = elsewhere.iter().filter(|(n, _)| n == f).map(|(_, p)| p.clone()).collect();
            (f.clone(), if there.is_empty() { vec![resolve(f, project)] } else { there })
        })
        .collect()
}

/// The most entries a search for names alone visits: a larger tree is cut there, and a name not
/// found by then is taken to be nowhere yet, as before the search.
const MAX_SEARCHED: usize = 20_000;

/// Folders no request means when it names a file alone: what a build or an installer made.
const NOT_OWN: [&str; 8] =
    ["node_modules", "site-packages", "dist-packages", "__pycache__", "target", "dist", "build", "vendor"];

/// The files with these names in the project, nearest first: "fix parser.py" means
/// `src/parser.py`. Read as a file to make, it was reported missing, the model was told to
/// write it, and edits to the real one stopped counting as progress. Hidden folders, virtual
/// environments and the folders in [`NOT_OWN`] are skipped: pip's own `main.py` isn't the one a
/// request for `main.py` asks for. Walked only when a name alone isn't at the project's root.
fn found_elsewhere(names: &[&str], project: &Path) -> Vec<(String, PathBuf)> {
    let mut found = vec![];
    let (mut level, mut visited) = (if names.is_empty() { vec![] } else { vec![project.to_path_buf()] }, 0usize);
    while !level.is_empty() && visited < MAX_SEARCHED {
        let mut next = vec![];
        for dir in level {
            if visited >= MAX_SEARCHED {
                break;
            }
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            // A folder of a million files is read only up to what is left of the budget.
            let mut entries: Vec<_> = rd.flatten().take(MAX_SEARCHED - visited).collect();
            entries.sort_by_key(|e| e.file_name());
            for e in entries {
                visited += 1;
                let name = e.file_name().to_string_lossy().into_owned();
                let path = e.path();
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    if !name.starts_with('.') && !NOT_OWN.contains(&name.as_str()) && !path.join("pyvenv.cfg").exists()
                    {
                        next.push(path);
                    }
                } else if names.contains(&name.as_str()) && path.is_file() {
                    found.push((name, path));
                }
            }
        }
        level = next;
    }
    found
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

/// File types common enough that a bare name with one is read as a file even before it exists
/// (`answer.txt`, `weights.json`, `report.md`). Left out on purpose: extensions that are also
/// everyday attribute or function names in code (`self.data`, `math.log`, `self.db`, `self.cfg`,
/// `mock.patch`, `np.diff`, `process.env`, `np.mat`), and one- or two-letter ones that read as
/// variables (`p.r`, `v.o`). A file with any of these still counts when it exists or the request
/// asks to write it.
const FILE_EXTENSIONS: &str = concat!(
    "c h cc cpp cxx hpp hh cs java kt kts scala go rs py pyi pyx ipynb js mjs cjs jsx ts tsx rb ",
    "php pl pm lua jl swift dart exs erl hs clj vue svelte zig nim sh bash zsh fish ps1 bat sql ",
    "graphql proto cu cuh f90 sol tf hcl nix cmake mk gradle groovy el scm rkt asm wasm wat vhd ",
    "vhdl sv ",
    "txt md markdown rst adoc tex bib json jsonl ndjson yaml yml toml ini xml xsd csv ",
    "tsv plist html htm css scss sass less svg j2 jinja tpl erb ejs hbs ",
    "bak ",
    "bin dat sqlite sqlite3 parquet feather arrow avro orc h5 hdf5 npy npz pkl pickle pt pth ",
    "ckpt safetensors onnx gguf tflite pb joblib pdf doc docx xls xlsx ppt pptx odt ods odp rtf ",
    "epub png jpg jpeg gif bmp tif tiff webp ico ppm pgm pbm pnm heic psd mp3 wav flac ogg m4a ",
    "aac mp4 mkv mov avi webm srt vtt ttf otf woff woff2 ",
    "zip tar gz tgz bz2 xz zst 7z rar jar war whl deb rpm dmg iso so dll dylib exe pyc pem crt ",
    "cer csr pub p12 pfx ",
);
/// Words that ask for the file named just after them to be made (`Write me data.comp`, `generate
/// data.comp`, `save model.v2`): within [`WRITE_REACH`] words before the name. Farther, the name
/// may be anything the sentence goes on to mention (`Write code that uses np.float64`). Asking
/// forms only: `written as module.function` describes, it doesn't ask.
const WRITE_WORDS: &str = concat!(
    "write writes create creates save saves generate generates produce produces output outputs ",
    "export exports dump dumps store stores emit emits ",
);
const WRITE_REACH: usize = 2;

/// Is a word with the shape of a file name read as one? A path (with a folder) is when it says
/// so ([`path_evidence`]). A bare `name.ext` needs evidence: the file exists, its extension is a
/// common file type ([`FILE_EXTENSIONS`]), one of the few words before it asks to write it
/// ([`WRITE_WORDS`]), or it is where a sentence that writes files puts something (`Write the port
/// to port.txt and the process ID to server.pid`: after "to", with a write word and a file earlier
/// in the sentence, and not a numeric type: `converts the columns to np.float64`). `np.float64`,
/// `self.data` and `math.log` have a file name's shape, not its sense: read as files, they were
/// reported missing and the model was told to write them. `files_before`: the sentence already
/// named one.
fn file_evidence(w: &str, project: &Path, before: &[String], files_before: bool) -> bool {
    if w.contains('/') {
        return path_evidence(w, project);
    }
    if resolve(w, project).exists() || FILE_EXTENSIONS.split_whitespace().any(|e| e == extension(w)) {
        return true;
    }
    let asks_to_write = |b: &String| WRITE_WORDS.split_whitespace().any(|v| v == b);
    if before.iter().rev().take(WRITE_REACH).any(asks_to_write) {
        return true;
    }
    before.last().is_some_and(|b| b == "to" || b == "into")
        && files_before
        && before.iter().any(asks_to_write)
        && !numeric_type(&extension(w))
}

/// Does a word with a folder in it say it is a path? It is there, it starts at a root (`/app/x`,
/// `./x`, `../x`, `~/x`), it ends with `/` (a folder: `dist/`, not `w/`), or its last part is
/// named like a file (`out/plan_a.jsonl`). Words joined by a slash are not paths (`and/or`,
/// `input/output`, `missing/duplicate`): read as files, they were reported missing at the end of
/// the turn, and the model was told to make them.
fn path_evidence(w: &str, project: &Path) -> bool {
    let folder = w.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
    resolve(w, project).exists()
        || ["/", "./", "../", "~/"].iter().any(|r| w.starts_with(r))
        || (w.ends_with('/') && folder.chars().count() > 1)
        || named_like_file(w.rsplit('/').next().unwrap_or(w))
}

/// `float64`, `int8`, `uint16`, `bfloat16`, `complex128`: a numeric type's name, not a file type.
fn numeric_type(ext: &str) -> bool {
    let bits = ext.trim_start_matches(|c: char| c.is_ascii_alphabetic());
    bits.len() < ext.len() && matches!(bits, "8" | "16" | "32" | "64" | "128")
}

fn is_file_word(w: &str) -> bool {
    // `<` and `>` are markup or redirections (`</system-reminder>`, `2>/dev/null`), never a path.
    if w.is_empty()
        || w.contains(char::is_whitespace)
        || w.contains("://")
        || w.contains(['@', '<', '>'])
        || w.starts_with('-')
    {
        return false;
    }
    if w.starts_with("www.") || w.chars().all(|c| c == '/' || c == '.') {
        return false;
    }
    if w.contains('/') {
        // A path: at least one part with a letter or a digit, and only the characters paths are
        // made of (`ceil(len/64)*64`, `/^a$/` and `$HOME/x` are not), no spaces.
        return w.chars().any(|c| c.is_ascii_alphanumeric())
            && !w.starts_with("//")
            && w.chars().all(|c| c.is_alphanumeric() || "._-+~/:".contains(c));
    }
    named_like_file(w)
}

/// A name with an extension: a stem with a letter, an extension of letters and digits that isn't
/// all digits (`1.4.0`, `3.5`), and not an abbreviation (`e.g`, `i.e`).
fn named_like_file(name: &str) -> bool {
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

/// A file's extension, lower case (`model.c` → `c`), or "".
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
        // Ruled out or set aside, in other words than "don't": nothing named, file or command.
        for text in [
            "Avoid running `sh ./build.sh`.",
            "Build it without `sh ./build.sh`.",
            "Use `sh -c true` instead of `sh ./build.sh`.",
            "Run `sh -c true`, not `sh ./build.sh`.",
            "Avoid touching secrets.env.",
            "Everything except notes.md can change.",
        ] {
            let n = named(text);
            assert!(n.commands.is_empty() && n.files.is_empty(), "{text}: {n:?}");
        }
    }

    #[test]
    fn a_command_is_asked_for_only_when_the_request_says_to_run_or_pass_it() {
        // Mentioned, not asked for: nothing to remind the model of.
        assert!(named("See `sh ./build.sh` for how releases are made.").commands.is_empty());
        assert!(named("The script `sh ./build.sh` is slow.").commands.is_empty());
        // Asked for: run it, make it pass, check with it.
        assert_eq!(named("Verify with `sh ./build.sh`.").commands, ["sh ./build.sh"]);
        assert_eq!(named("Run the tests with `sh ./build.sh`.").commands, ["sh ./build.sh"]);
        assert_eq!(named("`sh ./build.sh` must succeed.").commands, ["sh ./build.sh"]);
        assert_eq!(named("Then execute `sh -c true` and `sh ./build.sh`.").commands, ["sh -c true", "sh ./build.sh"]);
        // Said to fail: a check is one to make pass; anything else is only described.
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("run_checks.py"), "").unwrap();
        let read = |t: &str| Named::from_request(t, d.path()).commands;
        assert_eq!(read("`python3 run_checks.py` fails. Fix parser.py."), ["python3 run_checks.py"]);
        assert!(read("`git push` fails because of the proxy.").is_empty(), "not a check: not asked for");
        // What it must print, where the request wants it so; a description of a tool isn't.
        assert_eq!(
            named("Write x.sh so that `sh x.sh 1` prints `one` and `sh x.sh 2` prints `two`.").commands,
            ["sh x.sh 1", "sh x.sh 2"]
        );
        assert_eq!(named("`sh ./build.sh` must print ok.").commands, ["sh ./build.sh"]);
        assert!(named("`sh ./build.sh <dir>` prints where a file goes.").commands.is_empty());
    }

    #[test]
    fn a_request_in_another_language_names_nothing() {
        // Its prohibitions aren't in the English words above: silent rather than wrong.
        for text in [
            "Ne crée pas notes.md et n'exécute jamais `sh ./build.sh`.",
            "Lance `sh ./build.sh` puis écris le rapport dans rapport.txt.",
            "No ejecutes `sh ./build.sh` y no crees notas.md.",
            "Schreibe die Datei bericht.txt und führe `sh ./build.sh` nicht aus.",
        ] {
            assert_eq!(named(text), Named::default(), "{text}");
        }
        // English with technical words, and a request that is all paths, still read.
        assert_eq!(named("Optimize the kernel using tiling and write timings.csv.").files, ["timings.csv"]);
        assert_eq!(named("out/report.json").files, ["out/report.json"]);
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
            "Call your program /work/model.c; I will run /work/a.out weights.bin. Your c program must be <5000 bytes.";
        assert_eq!(read(two), [("/work/model.c".to_string(), 4999, "<5000 bytes".to_string())]);
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
        assert_eq!(Watched::at_start(&n, d.path()).still_missing(), ["gone.txt"]);
        assert_eq!(n.unrun_commands(&[]), ["sh -c true"]);
        assert!(n.unrun_commands(&["cd x &&  sh   -c true -v".to_string()]).is_empty(), "contained, spaces aside");
        assert_eq!(n.over_limit(d.path()), [("there.txt".to_string(), 50, 10, "at most 10 bytes".to_string())]);
    }

    #[test]
    fn a_file_asked_to_be_removed_is_not_missing() {
        let d = tempfile::tempdir().unwrap();
        let n =
            Named::from_request("Move config.ini to settings.toml and delete config.ini. Write notes.md.", d.path());
        assert_eq!(Watched::at_start(&n, d.path()).still_missing(), ["notes.md"]);
        assert_eq!(n.removed, ["config.ini", "settings.toml"]);
    }

    #[test]
    fn a_dotted_code_name_is_not_a_file() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("vocab.bpe"), "x").unwrap();
        // The shape of a file name, not the sense: no such file, no common file type, and nothing
        // just before it asks to write it.
        let ask = "The input is an array with real np.float64 entries. Use `math.log`, not self.data or notes.xyz.";
        let n = Named::from_request(ask, d.path());
        assert!(n.files.is_empty(), "{:?}", n.files);
        // Evidence: it exists, its type is common, or the words just before it ask to write it.
        let ask = "Read vocab.bpe and fix parser.py. Write me data.comp, then save model.v2.";
        assert_eq!(Named::from_request(ask, d.path()).files, ["vocab.bpe", "parser.py", "data.comp", "model.v2"]);
        assert_eq!(Named::from_request("You can generate data.comp any way you want.", d.path()).files, ["data.comp"]);
        // A path is a file whatever its extension.
        assert_eq!(Named::from_request("Run /app/a.out on it.", d.path()).files, ["/app/a.out"]);
        // A write word farther before the name doesn't reach it, and a description isn't a request.
        for ask in [
            "Write code that uses np.float64 for the values.",
            "Write a note that mentions model.v2.",
            "Write a function that converts the array to np.float64.",
            "List each handler, written as `module.function` the way the registry stores it.",
        ] {
            assert!(Named::from_request(ask, d.path()).files.is_empty(), "{ask}");
        }
        // Where a sentence that writes files puts something, unless it is a numeric type.
        for (ask, files) in [
            ("Write the compressed form of data.txt to data.comp.", vec!["data.txt", "data.comp"]),
            ("Write the port number to port.txt and the process ID to server.pid.", vec!["port.txt", "server.pid"]),
            ("Write a script that loads data.csv and converts the columns to np.float64.", vec!["data.csv"]),
        ] {
            assert_eq!(Named::from_request(ask, d.path()).files, files, "{ask}");
        }
    }

    #[test]
    fn removal_words_count_in_every_form_and_rm_only_alone() {
        let d = tempfile::tempdir().unwrap();
        // The letters of `rm` inside another word, or a removal word's letters across words: no removal.
        for ask in [
            "Write the compressed form of data.txt to data.json.",
            "Transform data.txt into data.json for the platform.",
            "Confirm data.txt converts, then write data.json.",
            "Plan the long-term layout of data.txt and write data.json.",
            "Draw a grid of data.txt values into data.json.",
        ] {
            let n = Named::from_request(ask, d.path());
            assert_eq!(n.files, ["data.txt", "data.json"], "{ask}");
            assert!(n.removed.is_empty(), "{ask}");
        }
        // Any form of a removal word, and `rm` as a word of its own.
        for ask in [
            "rm data.txt, then write data.json.",
            "Run rm -f data.txt first.",
            "Delete data.txt and data.json.",
            "Make sure data.txt is deleted.",
            "Deleting data.txt is part of the job.",
            "Removing data.txt is fine.",
            "The removal of data.txt is wanted.",
            "Clean up data.txt when done.",
            "We cleaned up data.txt already.",
            "Get rid of data.txt.",
        ] {
            assert!(Named::from_request(ask, d.path()).removed.contains(&"data.txt".to_string()), "{ask}");
        }
    }

    #[test]
    fn markup_is_not_a_file() {
        let d = tempfile::tempdir().unwrap();
        let ask = "<system-reminder>\nKeep working toward the goal: report.txt has the totals\n</system-reminder>";
        assert_eq!(Named::from_request(ask, d.path()).files, ["report.txt"]);
    }

    #[test]
    fn a_renamed_file_is_not_missing() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::write(root.join("helper.py"), "def f(): pass\n").unwrap();
        let n = Named::from_request("Rename helper.py to utils.py and update the imports.", root);
        let w = Watched::at_start(&n, root);
        assert_eq!(w.still_missing(), ["utils.py"], "the new name is asked for");
        std::fs::rename(root.join("helper.py"), root.join("utils.py")).unwrap();
        assert!(w.still_missing().is_empty(), "the old name is gone as asked, not missing");
        let report = report(&n, w.still_missing(), root, &[]);
        assert!(report.is_none(), "{report:?}");
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

    #[test]
    fn words_joined_by_a_slash_are_not_paths() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("src/bin")).unwrap();
        let read = |t: &str| Named::from_request(t, d.path()).files;
        // Read as files, these were reported missing and the model was told to make them.
        for ask in [
            "Keep it fast and/or small, with no missing/duplicate rows.",
            "Compare the cost/latency of each input/output format.",
            "Pad each batch to ceil(prompt_len/64)*64 tokens, or `ceil(n/64)*64`.",
            "It works w/ Python 3, w/o a GPU, 24/7, at 30 km/h, over TCP/IP.",
            "Use $HOME/cache for scratch, and match /^v[0-9]+$/ names.",
        ] {
            assert!(read(ask).is_empty(), "{ask}: {:?}", read(ask));
        }
        // A path says so: it is there, starts at a root, is a folder, or ends with a file's name.
        assert_eq!(
            read("Build src/bin, /opt/tool, ./out, ../lib and dist/, then write reports/2026/summary.json."),
            ["src/bin", "/opt/tool", "./out", "../lib", "dist/", "reports/2026/summary.json"]
        );
    }

    #[test]
    fn a_file_named_alone_is_the_one_elsewhere_in_the_project() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        for (dir, file) in [
            ("src", "parser.py"),
            ("data/input", "requests_1.jsonl"),
            // Not the project's own: installed packages, a virtual environment, a hidden folder.
            ("node_modules/x", "index.js"),
            ("venv/lib/python3.12/site-packages/pip", "main.py"),
            (".cache", "notes.md"),
        ] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::write(root.join(dir).join(file), "x").unwrap();
        }
        std::fs::write(root.join("venv/pyvenv.cfg"), "").unwrap();
        let ask = "Fix parser.py so it reads requests_1.jsonl, then write main.py, index.js and notes.md.";
        let mut w = Watched::at_start(&Named::from_request(ask, root), root);
        assert_eq!(w.still_missing(), ["main.py", "index.js", "notes.md"]);
        // Watched where it is: an edit there is a change to a named file.
        std::fs::write(root.join("src/parser.py"), "fixed").unwrap();
        assert_eq!(w.changed(), ["parser.py"]);
        // Every named file is there: the request asks for nothing new.
        assert!(!Watched::at_start(&Named::from_request("Fix parser.py.", root), root).asks_for_new());
        // Read, not made: a limit from a sentence that names no file isn't its.
        let ask = "Use requests_1.jsonl. The output must be at most 10 bytes.";
        assert!(Named::from_request(ask, root).limits.is_empty());
    }
}
