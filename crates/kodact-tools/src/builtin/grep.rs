use std::path::{Path, PathBuf};
use std::time::SystemTime;

use globset::{Glob as GsGlob, GlobSet, GlobSetBuilder};
use kodact_permissions::Subject;
use regex::RegexBuilder;
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
        let re = match RegexBuilder::new(str_arg(&input, "pattern"))
            .case_insensitive(arg_bool(&input, "-i").unwrap_or(false))
            .multi_line(true)
            .dot_matches_new_line(multiline)
            .build()
        {
            Ok(r) => r,
            Err(e) => return ToolOutput::error(format!("Invalid regex: {e}")),
        };
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
        let result = tokio::task::spawn_blocking(move || -> Result<(Vec<String>, usize, usize), String> {
            let mut wb = ignore::WalkBuilder::new(&root);
            if !ty.is_empty() {
                let mut tb = ignore::types::TypesBuilder::new();
                tb.add_defaults();
                tb.select(&ty);
                wb.types(tb.build().map_err(|e| format!("Unknown file type {ty}: {e}"))?);
            }
            let mut files: Vec<(PathBuf, SystemTime, Vec<String>, usize)> = vec![];
            for entry in wb.build().flatten() {
                if cancel.is_cancelled() {
                    break;
                }
                if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    continue;
                }
                let path = entry.path();
                if let Some((set, slash)) = &globs {
                    let target: &Path = if *slash {
                        path.strip_prefix(&root).unwrap_or(path)
                    } else {
                        Path::new(path.file_name().unwrap_or_default())
                    };
                    if !set.is_match(target) {
                        continue;
                    }
                }
                let Ok(bytes) = std::fs::read(path) else { continue };
                if bytes.iter().take(8000).any(|&b| b == 0) {
                    continue;
                }
                let text = String::from_utf8_lossy(&bytes);
                let mut matched_lines: Vec<usize> = vec![];
                let mut count = 0usize;
                let line_starts: Vec<usize> =
                    std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
                let line_of = |byte: usize| line_starts.partition_point(|&s| s <= byte) - 1;
                for m in re.find_iter(&text) {
                    count += 1;
                    if mode == Mode::Content {
                        let (a, b) = (line_of(m.start()), line_of(m.end().saturating_sub(1).max(m.start())));
                        for l in a..=b {
                            if matched_lines.last() != Some(&l) {
                                matched_lines.push(l);
                            }
                        }
                    }
                }
                if count == 0 {
                    continue;
                }
                let mut lines_out = vec![];
                if mode == Mode::Content {
                    let all: Vec<&str> = text.lines().collect();
                    let mut shown: Vec<usize> = vec![];
                    for &l in &matched_lines {
                        for c in l.saturating_sub(before)..=(l + after).min(all.len().saturating_sub(1)) {
                            if !shown.contains(&c) {
                                shown.push(c);
                            }
                        }
                    }
                    shown.sort_unstable();
                    let mut prev: Option<usize> = None;
                    for l in shown {
                        if (before > 0 || after > 0) && prev.map(|p| l > p + 1).unwrap_or(false) {
                            lines_out.push("--".to_string());
                        }
                        let sep = if matched_lines.binary_search(&l).is_ok() { ':' } else { '-' };
                        let body = all.get(l).copied().unwrap_or("");
                        lines_out.push(if line_numbers {
                            format!("{}{sep}{}{sep}{}", path.display(), l + 1, body)
                        } else {
                            format!("{}{sep}{}", path.display(), body)
                        });
                        prev = Some(l);
                    }
                }
                let mtime = entry.metadata().ok().and_then(|m| m.modified().ok()).unwrap_or(SystemTime::UNIX_EPOCH);
                files.push((path.to_path_buf(), mtime, lines_out, count));
            }
            let total_matches: usize = files.iter().map(|f| f.3).sum();
            let n_files = files.len();
            let lines: Vec<String> = match mode {
                Mode::Files => {
                    files.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                    files.into_iter().map(|f| f.0.display().to_string()).collect()
                }
                Mode::Count => {
                    files.sort_by(|a, b| a.0.cmp(&b.0));
                    files.into_iter().map(|f| format!("{}:{}", f.0.display(), f.3)).collect()
                }
                Mode::Content => {
                    files.sort_by(|a, b| a.0.cmp(&b.0));
                    files.into_iter().flat_map(|f| f.2).collect()
                }
            };
            Ok((lines, n_files, total_matches))
        })
        .await;
        let (lines, n_files, total_matches) = match result {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return ToolOutput::error(e),
            Err(e) => return ToolOutput::error(format!("search failed: {e}")),
        };
        let total = lines.len();
        let page: Vec<String> =
            lines.into_iter().skip(offset).take(if head_limit == 0 { usize::MAX } else { head_limit }).collect();
        let structured = json!({"mode": format!("{mode:?}").to_lowercase(), "numFiles": n_files, "numMatches": total_matches, "numLines": page.len()});
        if page.is_empty() {
            let what = if mode == Mode::Content { "No matches found" } else { "No files found" };
            // Past the end of the results, not an empty search.
            if total > 0 {
                return ToolOutput::text(format!("{what} from offset {offset}: there are {total} in all."))
                    .with_structured(structured);
            }
            return ToolOutput::text(format!("{what}{}", no_match_hint(&input, &searched))).with_structured(structured);
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
        ToolOutput::text(text).with_structured(structured)
    }
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
