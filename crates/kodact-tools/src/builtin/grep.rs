use std::path::{Path, PathBuf};
use std::time::SystemTime;

use globset::{Glob as GsGlob, GlobSet, GlobSetBuilder};
use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use kodact_permissions::Subject;
use serde_json::{json, Value};

use super::str_arg;
use crate::{expand_path, Tool, ToolContext, ToolOutput};

pub const DEFAULT_HEAD_LIMIT: usize = 250;

pub struct Grep;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Files,
    Content,
    Count,
}

fn search_root(input: &Value, ctx: &ToolContext) -> PathBuf {
    match input.get("path").and_then(Value::as_str).filter(|p| !p.trim().is_empty()) {
        Some(p) => expand_path(p, &ctx.shell_cwd()),
        None => ctx.shell_cwd(),
    }
}

/// Split `*.{ts,tsx}` style globs on top-level commas or spaces is not needed:
/// globset understands braces. Several globs may be separated by spaces.
fn build_globs(raw: &str) -> Result<Option<(GlobSet, bool)>, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let mut b = GlobSetBuilder::new();
    let mut any_slash = false;
    for g in raw.split_whitespace() {
        any_slash |= g.contains('/');
        b.add(GsGlob::new(g).map_err(|e| format!("Invalid glob {g}: {e}"))?);
    }
    Ok(Some((b.build().map_err(|e| e.to_string())?, any_slash)))
}

fn arg_usize(input: &Value, key: &str) -> Option<usize> {
    input.get(key).and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).map(|n| n as usize)
}

fn arg_bool(input: &Value, key: &str) -> Option<bool> {
    input.get(key).and_then(Value::as_bool)
}

#[async_trait::async_trait]
impl Tool for Grep {
    fn name(&self) -> &str {
        "Grep"
    }

    fn description(&self) -> String {
        "Search file contents with a regular expression (Rust regex syntax, like ripgrep).\n\n\
         - `output_mode`: \"files_with_matches\" (default, paths sorted by modification time), \"content\" \
           (matching lines; supports -n, -A, -B, -C), or \"count\".\n\
         - Filter files with `glob` (e.g. \"*.rs\", \"**/*.{ts,tsx}\") or `type` (e.g. \"rust\", \"py\", \"js\").\n\
         - `multiline: true` lets `.` match newlines and patterns span lines.\n\
         - `head_limit` caps output lines/entries (default 250, 0 = unlimited); `offset` skips the first N.\n\
         - Respects .gitignore and skips hidden and binary files. Escape literal braces (`interface\\{\\}`)."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "The regular expression to search for"},
                "path": {"type": "string", "description": "File or directory to search (defaults to the current directory)"},
                "glob": {"type": "string", "description": "Glob filter for file names, e.g. \"*.js\""},
                "type": {"type": "string", "description": "File type filter, e.g. \"rust\", \"py\""},
                "output_mode": {"type": "string", "enum": ["content", "files_with_matches", "count"]},
                "-i": {"type": "boolean", "description": "Case insensitive"},
                "-n": {"type": "boolean", "description": "Show line numbers (content mode, default true)"},
                "-A": {"type": "number", "description": "Lines after each match"},
                "-B": {"type": "number", "description": "Lines before each match"},
                "-C": {"type": "number", "description": "Lines before and after each match"},
                "context": {"type": "number", "description": "Alias for -C"},
                "multiline": {"type": "boolean", "description": "Allow patterns to span lines"},
                "head_limit": {"type": "number", "description": "Limit output to the first N lines/entries (0 = unlimited)"},
                "offset": {"type": "number", "description": "Skip the first N lines/entries"}
            },
            "required": ["pattern"],
            "additionalProperties": false
        })
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn permission_subject(&self, input: &Value, ctx: &ToolContext) -> Subject {
        Subject::Path { path: search_root(input, ctx), write: false }
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let root = search_root(&input, ctx);
        if !root.exists() {
            return ToolOutput::error(format!(
                "Path does not exist: {}.{}",
                root.display(),
                crate::missing_path_hint(&root)
            ));
        }
        let mode = match str_arg(&input, "output_mode") {
            "content" => Mode::Content,
            "count" => Mode::Count,
            _ => Mode::Files,
        };
        let multiline = arg_bool(&input, "multiline").unwrap_or(false);
        let globs = match build_globs(str_arg(&input, "glob")) {
            Ok(g) => g,
            Err(e) => return ToolOutput::error(e),
        };
        let ty = str_arg(&input, "type").to_string();
        let ctx_c = arg_usize(&input, "-C").or_else(|| arg_usize(&input, "context")).unwrap_or(0);
        let after = arg_usize(&input, "-A").unwrap_or(ctx_c);
        let before = arg_usize(&input, "-B").unwrap_or(ctx_c);
        let line_numbers = arg_bool(&input, "-n").unwrap_or(true);
        let head_limit = arg_usize(&input, "head_limit").unwrap_or(DEFAULT_HEAD_LIMIT);
        let offset = arg_usize(&input, "offset").unwrap_or(0);
        let cancel = ctx.cancel.clone();

        let searched = root.clone();
        let params = SearchParams {
            root,
            globs,
            ty,
            mode,
            pattern: str_arg(&input, "pattern").to_string(),
            case_insensitive: arg_bool(&input, "-i").unwrap_or(false),
            multiline,
            before,
            after,
            line_numbers,
            offset,
            limit: if head_limit == 0 { usize::MAX } else { head_limit },
            cancel,
            skip_dirs: kernel_dirs(),
            heap_limit: SEARCH_HEAP_LIMIT,
        };
        let result = tokio::task::spawn_blocking(move || search(&params)).await;
        let SearchOutcome { lines: page, total_lines: total, n_files, total_matches, notes } = match result {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return ToolOutput::error(e),
            Err(e) => return ToolOutput::error(format!("search failed: {e}")),
        };
        let notes_text = if notes.is_empty() { String::new() } else { format!("\n\n{}", notes.join("\n")) };
        let structured = json!({"mode": format!("{mode:?}").to_lowercase(), "numFiles": n_files, "numMatches": total_matches, "numLines": page.len()});
        if page.is_empty() {
            let what = if mode == Mode::Content { "No matches found" } else { "No files found" };
            // Past the end of the results, not an empty search.
            if total > 0 {
                return ToolOutput::text(format!("{what} from offset {offset}: there are {total} in all.{notes_text}"))
                    .with_structured(structured);
            }
            return ToolOutput::text(format!("{what}{}{notes_text}", no_match_hint(&input, &searched)))
                .with_structured(structured);
        }
        let mut text = match mode {
            Mode::Files => format!("Found {n_files} file{}\n{}", if n_files == 1 { "" } else { "s" }, page.join("\n")),
            Mode::Count => {
                format!("{}\n\nFound {total_matches} total occurrences across {n_files} files.", page.join("\n"))
            }
            Mode::Content => page.join("\n"),
        };
        if offset + page.len() < total {
            text.push_str(&format!(
                "\n\n(Showing {} of {total}; pass offset={} for more.)",
                page.len(),
                offset + page.len()
            ));
        }
        text.push_str(&notes_text);
        ToolOutput::text(text).with_structured(structured)
    }
}

/// Memory bound of one search: the most a single buffer may grow to, which is
/// the longest line in line mode and the whole file in multiline mode. A bound
/// on this process, whatever the model.
const SEARCH_HEAP_LIMIT: usize = 256 * 1024 * 1024;

/// A file with a NUL in its first bytes is binary, as before.
const BINARY_PROBE: u64 = 8000;

/// Most "not searched" notes spelled out; the rest are counted.
const MAX_NOTES: usize = 5;

/// The kernel's virtual filesystems, by canonical path: some of their files
/// never end and /proc mirrors every process.
fn kernel_dirs() -> Vec<PathBuf> {
    ["/proc", "/sys"].iter().filter_map(|p| Path::new(p).canonicalize().ok()).collect()
}

struct SearchParams {
    root: PathBuf,
    globs: Option<(GlobSet, bool)>,
    ty: String,
    mode: Mode,
    pattern: String,
    case_insensitive: bool,
    multiline: bool,
    before: usize,
    after: usize,
    line_numbers: bool,
    offset: usize,
    limit: usize,
    cancel: tokio_util::sync::CancellationToken,
    skip_dirs: Vec<PathBuf>,
    heap_limit: usize,
}

struct SearchOutcome {
    /// The page: lines `offset..offset + limit` of the full result.
    lines: Vec<String>,
    total_lines: usize,
    n_files: usize,
    total_matches: usize,
    notes: Vec<String>,
}

/// Keeps only the lines inside the requested page; counts all of them.
struct Window {
    offset: usize,
    limit: usize,
    seen: usize,
    out: Vec<String>,
}

impl Window {
    fn push(&mut self, line: impl FnOnce() -> String) {
        if self.seen >= self.offset && self.seen - self.offset < self.limit {
            self.out.push(line());
        }
        self.seen += 1;
    }
}

/// Reads a file, failing once the search is cancelled so an interrupt also
/// stops a long search of one huge file that has no match to report.
struct CancellableRead<R> {
    inner: R,
    cancel: tokio_util::sync::CancellationToken,
}

impl<R: std::io::Read> std::io::Read for CancellableRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "search cancelled"));
        }
        self.inner.read(buf)
    }
}

struct LineSink<'a> {
    path: String,
    window: &'a mut Window,
    matcher: &'a RegexMatcher,
    emit_lines: bool,
    line_numbers: bool,
    breaks: bool,
    cancel: &'a tokio_util::sync::CancellationToken,
    count: usize,
    prev: Option<u64>,
    binary_at: Option<u64>,
}

impl LineSink<'_> {
    fn emit(&mut self, line_no: u64, bytes: &[u8], sep: char) {
        if self.breaks && self.prev.is_some_and(|p| line_no > p + 1) {
            self.window.push(|| "--".to_string());
        }
        self.prev = Some(line_no);
        let path = &self.path;
        let numbers = self.line_numbers;
        self.window.push(|| {
            let body = bytes.strip_suffix(b"\n").unwrap_or(bytes);
            let body = body.strip_suffix(b"\r").unwrap_or(body);
            let body = String::from_utf8_lossy(body);
            if numbers {
                format!("{path}{sep}{line_no}{sep}{body}")
            } else {
                format!("{path}{sep}{body}")
            }
        });
    }
}

impl Sink for LineSink<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        let mut n = 0usize;
        let _ = self.matcher.find_iter(mat.bytes(), |_| {
            n += 1;
            true
        });
        self.count += n.max(1);
        if self.emit_lines {
            let first = mat.line_number().unwrap_or(0);
            for (i, line) in mat.lines().enumerate() {
                self.emit(first + i as u64, line, ':');
            }
        }
        Ok(!self.cancel.is_cancelled())
    }

    fn context(&mut self, _: &Searcher, ctx: &SinkContext<'_>) -> Result<bool, Self::Error> {
        if self.emit_lines {
            self.emit(ctx.line_number().unwrap_or(0), ctx.bytes(), '-');
        }
        Ok(!self.cancel.is_cancelled())
    }

    fn binary_data(&mut self, _: &Searcher, offset: u64) -> Result<bool, Self::Error> {
        self.binary_at = Some(offset);
        Ok(false)
    }
}

fn human_size(n: u64) -> String {
    const UNITS: [&str; 4] = ["bytes", "KiB", "MiB", "GiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} bytes")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// Searches `p.root` the way ripgrep does: each file streamed through a bounded
/// buffer, binary files recognised from their first bytes, and only the page of
/// results asked for kept in memory. Files are visited in path order, so the
/// lines of Content and Count modes are final as they are produced.
fn search(p: &SearchParams) -> Result<SearchOutcome, String> {
    let build = |line_terminator: bool| {
        let mut b = RegexMatcherBuilder::new();
        b.case_insensitive(p.case_insensitive).multi_line(true).dot_matches_new_line(p.multiline);
        if line_terminator {
            b.line_terminator(Some(b'\n'));
        }
        b.build(&p.pattern)
    };
    // A pattern that can match a newline needs the whole file in view.
    let (matcher, multi_line) = if p.multiline {
        (build(false).map_err(|e| format!("Invalid regex: {e}"))?, true)
    } else {
        match build(true) {
            Ok(m) => (m, false),
            Err(_) => (build(false).map_err(|e| format!("Invalid regex: {e}"))?, true),
        }
    };
    let content = p.mode == Mode::Content;
    let (before, after) = if content { (p.before, p.after) } else { (0, 0) };
    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        .multi_line(multi_line)
        .before_context(before)
        .after_context(after)
        .binary_detection(BinaryDetection::quit(0))
        .heap_limit(Some(p.heap_limit))
        .build();

    let mut wb = ignore::WalkBuilder::new(&p.root);
    wb.sort_by_file_path(|a, b| a.cmp(b));
    if !p.ty.is_empty() {
        let mut tb = ignore::types::TypesBuilder::new();
        tb.add_defaults();
        tb.select(&p.ty);
        wb.types(tb.build().map_err(|e| format!("Unknown file type {}: {e}", p.ty))?);
    }
    // Kernel directories below the root are not entered; one that holds the root is.
    let canonical_root = p.root.canonicalize().unwrap_or_else(|_| p.root.clone());
    let skipped_at: Vec<(PathBuf, PathBuf)> = p
        .skip_dirs
        .iter()
        .filter(|d| d.starts_with(&canonical_root) && **d != canonical_root)
        .filter_map(|d| Some((p.root.join(d.strip_prefix(&canonical_root).ok()?), d.clone())))
        .collect();
    let skip_paths: Vec<PathBuf> = skipped_at.iter().map(|(walk, _)| walk.clone()).collect();
    let skipped: Vec<PathBuf> =
        skipped_at.iter().filter(|(walk, _)| walk.is_dir()).map(|(_, canon)| canon.clone()).collect();
    wb.filter_entry(move |e| e.depth() == 0 || !skip_paths.iter().any(|s| s == e.path()));

    let mut window = Window { offset: p.offset, limit: p.limit, seen: 0, out: vec![] };
    let mut listed: Vec<(PathBuf, SystemTime)> = vec![];
    let mut n_files = 0usize;
    let mut total_matches = 0usize;
    let mut notes: Vec<String> = vec![];
    let mut extra_notes = 0usize;
    let mut note = |notes: &mut Vec<String>, text: String| {
        if notes.len() < MAX_NOTES {
            notes.push(text);
        } else {
            extra_notes += 1;
        }
    };

    for entry in wb.build() {
        if p.cancel.is_cancelled() {
            break;
        }
        let Ok(entry) = entry else { continue };
        let ft = entry.file_type();
        if !ft.is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        if let Some((set, slash)) = &p.globs {
            let target: &Path = if *slash {
                path.strip_prefix(&p.root).unwrap_or(path)
            } else {
                Path::new(path.file_name().unwrap_or_default())
            };
            if !set.is_match(target) {
                continue;
            }
        }
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        if multi_line && size > p.heap_limit as u64 {
            note(
                &mut notes,
                format!(
                    "Not searched: {} ({}, over the multiline limit); search it without multiline or narrow the path.",
                    path.display(),
                    human_size(size)
                ),
            );
            continue;
        }
        let Ok(file) = std::fs::File::open(path) else { continue };
        let reader = CancellableRead { inner: file, cancel: p.cancel.clone() };
        let (seen0, out0) = (window.seen, window.out.len());
        let mut sink = LineSink {
            path: path.display().to_string(),
            window: &mut window,
            matcher: &matcher,
            emit_lines: content,
            line_numbers: p.line_numbers,
            breaks: before > 0 || after > 0,
            cancel: &p.cancel,
            count: 0,
            prev: None,
            binary_at: None,
        };
        let res = searcher.search_reader(&matcher, reader, &mut sink);
        let (count, binary_at) = (sink.count, sink.binary_at);
        let rollback = |w: &mut Window| {
            w.seen = seen0;
            w.out.truncate(out0);
        };
        if let Err(e) = res {
            rollback(&mut window);
            if p.cancel.is_cancelled() {
                break;
            }
            if e.to_string().contains("allocation limit") {
                note(
                    &mut notes,
                    format!(
                        "Not searched: {} ({}, a line or file over the {} limit); narrow the path or the pattern.",
                        path.display(),
                        human_size(size),
                        human_size(p.heap_limit as u64)
                    ),
                );
            }
            continue;
        }
        if let Some(at) = binary_at {
            if at < BINARY_PROBE {
                rollback(&mut window);
                continue;
            }
            note(&mut notes, format!("Search of {} stopped at binary data after {}.", path.display(), human_size(at)));
        }
        if count == 0 {
            continue;
        }
        n_files += 1;
        total_matches += count;
        match p.mode {
            Mode::Files => {
                let mtime = entry.metadata().ok().and_then(|m| m.modified().ok()).unwrap_or(SystemTime::UNIX_EPOCH);
                listed.push((path.to_path_buf(), mtime));
            }
            Mode::Count => window.push(|| format!("{}:{count}", path.display())),
            Mode::Content => {}
        }
    }
    let lines = if p.mode == Mode::Files {
        listed.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        window.seen = listed.len();
        listed.into_iter().skip(p.offset).take(p.limit).map(|f| f.0.display().to_string()).collect()
    } else {
        std::mem::take(&mut window.out)
    };
    if !skipped.is_empty() {
        let names: Vec<String> = skipped.iter().map(|d| d.display().to_string()).collect();
        notes.insert(
            0,
            format!("Not searched: {} (kernel files); give one as the path to search it.", names.join(", ")),
        );
    }
    if extra_notes > 0 {
        notes.push(format!("(and {extra_notes} more files not searched or stopped early)"));
    }
    Ok(SearchOutcome { lines, total_lines: window.seen, n_files, total_matches, notes })
}

/// What an empty search covered, and what usually explains it (GOALS pillar 4).
fn no_match_hint(input: &Value, root: &std::path::Path) -> String {
    let pattern = str_arg(input, "pattern");
    let mut out = format!(" for `{pattern}` in {}", root.display());
    let filters: Vec<String> = [("glob", "glob"), ("type", "type")]
        .into_iter()
        .filter_map(|(k, label)| input.get(k).and_then(Value::as_str).map(|v| format!("{label} {v}")))
        .collect();
    if !filters.is_empty() {
        out.push_str(&format!(" (only files matching {})", filters.join(", ")));
    }
    out.push('.');
    let insensitive = input.get("-i").and_then(Value::as_bool).unwrap_or(false);
    if !insensitive && pattern.chars().any(char::is_alphabetic) {
        out.push_str(" The search is case-sensitive: `-i` ignores case.");
    }
    // An unescaped metacharacter in what reads as plain text (a call, a path, a version).
    let literal_looking = pattern.contains(['(', ')', '[', '{', '+', '?', '|']) && !pattern.contains('\\');
    if literal_looking {
        out.push_str(" The pattern is a regular expression: to find ( ) [ ] { } + ? | . * as written, put a backslash before each.");
    }
    if !filters.is_empty() {
        out.push_str(" Try without the file filter.");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(root: &Path, pattern: &str, mode: Mode) -> SearchParams {
        SearchParams {
            root: root.to_path_buf(),
            globs: None,
            ty: String::new(),
            mode,
            pattern: pattern.to_string(),
            case_insensitive: false,
            multiline: false,
            before: 0,
            after: 0,
            line_numbers: true,
            offset: 0,
            limit: usize::MAX,
            cancel: tokio_util::sync::CancellationToken::new(),
            skip_dirs: vec![],
            heap_limit: SEARCH_HEAP_LIMIT,
        }
    }

    #[cfg(target_os = "linux")]
    fn peak_kib() -> u64 {
        let s = std::fs::read_to_string("/proc/self/status").unwrap();
        let l = s.lines().find(|l| l.starts_with("VmHWM:")).unwrap();
        l.split_whitespace().nth(1).unwrap().parse().unwrap()
    }

    #[test]
    fn a_sparse_binary_file_is_skipped_without_reading_it() {
        let d = tempfile::tempdir().unwrap();
        let f = std::fs::File::create(d.path().join("disk.img")).unwrap();
        f.set_len(8 * 1024 * 1024 * 1024).unwrap(); // 8 GiB of NULs
        std::fs::write(d.path().join("a.txt"), "needle\n").unwrap();
        let t = std::time::Instant::now();
        let out = search(&params(d.path(), "needle", Mode::Files)).unwrap();
        assert_eq!(out.n_files, 1, "{:?}", out.lines);
        assert!(out.lines[0].ends_with("a.txt"));
        assert!(t.elapsed().as_secs() < 5, "took {:?}", t.elapsed());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_large_text_file_is_streamed_not_loaded() {
        let d = tempfile::tempdir().unwrap();
        let line = "2024-01-01 INFO request served in 12ms path=/index\n";
        let mut body = String::new();
        while body.len() < 1 << 20 {
            body.push_str(line);
        }
        let mut f = std::fs::File::create(d.path().join("big.log")).unwrap();
        for _ in 0..200 {
            std::io::Write::write_all(&mut f, body.as_bytes()).unwrap(); // about 200 MB
        }
        std::io::Write::write_all(&mut f, b"the needle\n").unwrap();
        drop(f);
        let before = peak_kib();
        let out = search(&params(d.path(), "needle", Mode::Content)).unwrap();
        let grew = peak_kib().saturating_sub(before);
        assert_eq!(out.total_lines, 1);
        assert!(out.lines[0].ends_with(":the needle"), "{:?}", out.lines);
        assert!(grew < 64 * 1024, "peak resident grew by {grew} KiB searching 200 MB");
    }

    #[test]
    fn matches_context_and_pages_keep_their_format() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "one\nhit a\ntwo\nthree\nfour\nhit b\nhit c\nsix\r\n").unwrap();
        let mut p = params(d.path(), "hit", Mode::Content);
        p.before = 1;
        p.after = 1;
        let out = search(&p).unwrap();
        let f = d.path().join("a.txt").display().to_string();
        let want = [
            format!("{f}-1-one"),
            format!("{f}:2:hit a"),
            format!("{f}-3-two"),
            "--".to_string(),
            format!("{f}-5-four"),
            format!("{f}:6:hit b"),
            format!("{f}:7:hit c"),
            format!("{f}-8-six"),
        ];
        assert_eq!(out.lines, want);
        assert_eq!((out.n_files, out.total_matches), (1, 3));
        // A page keeps nothing else, but the totals stay exact.
        let mut p = params(d.path(), "hit", Mode::Content);
        p.offset = 1;
        p.limit = 1;
        let out = search(&p).unwrap();
        assert_eq!(out.lines, [format!("{f}:6:hit b")]);
        assert_eq!(out.total_lines, 3);
        let out = search(&params(d.path(), "hit", Mode::Count)).unwrap();
        assert_eq!(out.lines, [format!("{f}:3")]);
    }

    #[test]
    fn a_file_over_the_multiline_cap_is_named_not_dropped() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("big.txt"), format!("{}start\nend\n", "x\n".repeat(100))).unwrap();
        std::fs::write(d.path().join("small.txt"), "start\nend\n").unwrap();
        let mut p = params(d.path(), "start\\nend", Mode::Files);
        p.multiline = true;
        p.heap_limit = 64;
        let out = search(&p).unwrap();
        assert_eq!(out.n_files, 1);
        assert!(out.lines[0].ends_with("small.txt"));
        assert_eq!(out.notes.len(), 1, "{:?}", out.notes);
        assert!(out.notes[0].contains("Not searched:") && out.notes[0].contains("big.txt"), "{:?}", out.notes);
        assert!(out.notes[0].contains("over the multiline limit"));
    }

    #[test]
    fn a_multiline_match_reports_every_line_of_it() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "a\nstart\nend\nz\n").unwrap();
        let mut p = params(d.path(), "start.*end", Mode::Content);
        p.multiline = true;
        let out = search(&p).unwrap();
        let f = d.path().join("a.txt").display().to_string();
        assert_eq!(out.lines, [format!("{f}:2:start"), format!("{f}:3:end")]);
    }

    #[test]
    fn kernel_like_directories_below_the_root_are_not_entered() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("proc")).unwrap();
        std::fs::write(root.join("proc/self.txt"), "needle\n").unwrap();
        std::fs::write(root.join("etc.txt"), "needle\n").unwrap();
        let mut p = params(&root, "needle", Mode::Files);
        p.skip_dirs = vec![root.join("proc")];
        let out = search(&p).unwrap();
        assert_eq!(out.n_files, 1, "{:?}", out.lines);
        assert_eq!(
            out.notes,
            [format!(
                "Not searched: {} (kernel files); give one as the path to search it.",
                root.join("proc").display()
            )]
        );
        // A search that starts inside one works as usual, with no note.
        let mut p = params(&root.join("proc"), "needle", Mode::Files);
        p.skip_dirs = vec![root.join("proc")];
        let out = search(&p).unwrap();
        assert_eq!(out.n_files, 1);
        assert!(out.notes.is_empty());
    }

    #[test]
    fn a_cancelled_search_stops_inside_a_file() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "needle\n".repeat(100_000)).unwrap();
        let p = params(d.path(), "needle", Mode::Content);
        p.cancel.cancel();
        let out = search(&p).unwrap();
        assert_eq!(out.total_lines, 0);
    }
}
