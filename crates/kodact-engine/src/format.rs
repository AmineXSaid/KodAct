//! Formatting after edits (GOALS pillar 3; `verification.format`, off by default).
//!
//! A project that keeps its files formatted fails its own checks on an edit that isn't
//! (`cargo fmt --check`, `ruff format --check`, a lint step), and the model then spends a turn
//! finding out and another fixing spacing. With `verification.format` on, the file just edited
//! is run through its language's formatter and the result written back, and the edit's result
//! says what moved, so the model's next edit matches the file.
//!
//! A formatter is only ever applied to a file that was *already formatted by it* before the
//! edit: the text from before the edit is formatted first, and unless that changes nothing the
//! file is left alone. So a project that doesn't use the formatter never has a file rewritten
//! by it, whatever is installed on the machine. A new file has nothing before it: it is
//! formatted only by a formatter the settings name (`verification.formatters`).
//!
//! Nothing is done or said when no formatter is installed, when it fails (a file that doesn't
//! parse is the parse check's business), or when the edit was already formatted.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::io::AsyncWriteExt;

/// How long a formatter may take on one file.
const TIMEOUT: Duration = Duration::from_secs(15);
/// Lines of the difference shown: enough to see what moved, the size of a message.
const MAX_DIFF_LINES: usize = 24;

/// `verification.format` and `verification.formatters`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FormatConfig {
    /// Extension (lower case, no dot) to a command that reads the file's text on stdin and
    /// writes the formatted text to stdout; `{file}` in an argument is the file's path. Tried
    /// instead of the built-in ones; an empty command turns the extension off.
    pub commands: HashMap<String, Vec<String>>,
}

impl FormatConfig {
    pub fn from_settings(enabled: Option<bool>, formatters: Option<&Value>) -> Option<Self> {
        if enabled != Some(true) {
            return None;
        }
        let commands = formatters
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .map(|(ext, cmd)| {
                        let cmd: Vec<String> = cmd
                            .as_array()
                            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                            .unwrap_or_default();
                        (ext.trim_start_matches('.').to_ascii_lowercase(), cmd)
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(FormatConfig { commands })
    }
}

/// The edition of the crate `path` belongs to (rustfmt takes it from the command line, not
/// from Cargo.toml): the nearest manifest that states one, 2021 otherwise.
fn rust_edition(path: &Path) -> String {
    for dir in path.ancestors().skip(1) {
        let Ok(manifest) = std::fs::read_to_string(dir.join("Cargo.toml")) else { continue };
        let stated = manifest.lines().map(str::trim).filter(|l| l.starts_with("edition")).find_map(|l| {
            let year = l.split('"').nth(1)?;
            (year.len() == 4 && year.chars().all(|c| c.is_ascii_digit())).then(|| year.to_string())
        });
        if let Some(year) = stated {
            return year;
        }
    }
    "2021".into()
}

/// The formatters tried for a file when the settings name none, each reading stdin and
/// writing stdout. The first one installed is the one used.
fn builtin(ext: &str, path: &Path, root: &Path) -> Vec<Vec<String>> {
    let file = path.display().to_string();
    let v = |parts: &[&str]| parts.iter().map(|s| s.to_string()).collect::<Vec<String>>();
    match ext {
        "rs" => vec![v(&["rustfmt", "--edition", &rust_edition(path), "--emit", "stdout"])],
        "py" | "pyi" => vec![
            v(&["ruff", "format", "--stdin-filename", &file, "-"]),
            v(&["black", "-q", "--stdin-filename", &file, "-"]),
        ],
        "go" => vec![v(&["gofmt"])],
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "json" | "css" | "scss" | "md" | "yaml" | "yml" | "html" => {
            let local = root.join("node_modules").join(".bin").join("prettier").display().to_string();
            vec![v(&[&local, "--stdin-filepath", &file]), v(&["prettier", "--stdin-filepath", &file])]
        }
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" => vec![v(&["clang-format", &format!("--assume-filename={file}")])],
        _ => vec![],
    }
}

enum Ran {
    NotInstalled,
    /// It ran and refused the text (a syntax error), or took too long.
    Failed,
    Text(String),
}

async fn pipe(command: &[String], path: &Path, text: &str) -> Ran {
    let Some((program, args)) = command.split_first() else { return Ran::NotInstalled };
    let file = path.display().to_string();
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args.iter().map(|a| a.replace("{file}", &file)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(dir) = path.parent() {
        cmd.current_dir(dir);
    }
    let Ok(mut child) = cmd.spawn() else { return Ran::NotInstalled };
    let Some(mut stdin) = child.stdin.take() else { return Ran::Failed };
    let input = text.to_string();
    let feed = tokio::spawn(async move {
        let _ = stdin.write_all(input.as_bytes()).await;
    });
    let out = tokio::time::timeout(TIMEOUT, child.wait_with_output()).await;
    feed.abort();
    match out {
        Ok(Ok(o)) if o.status.success() => match String::from_utf8(o.stdout) {
            // A formatter that prints nothing for a file that had text did not format it.
            Ok(s) if !s.is_empty() || text.is_empty() => Ran::Text(s),
            _ => Ran::Failed,
        },
        _ => Ran::Failed,
    }
}

/// What a formatter did to an edited file.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Formatted {
    pub formatter: String,
    /// The file's new text.
    pub text: String,
    /// Lines of the edited text the formatter changed or removed.
    pub lines: usize,
    diff: String,
}

impl Formatted {
    /// The text added to the edit's result.
    pub fn note(&self, file: &str) -> String {
        format!(
            "\n\n<system-reminder>\n{} reformatted {file} after this change, as the file was formatted before it \
             ({} line{} differ). The file on disk is the formatted one; what moved:\n{}\nCopy text for later \
             edits from this, not from what you wrote.\n</system-reminder>",
            self.formatter,
            self.lines,
            if self.lines == 1 { "" } else { "s" },
            self.diff
        )
    }
}

/// The lines that differ between `before` and `after`, as `-`/`+` lines with the line number in
/// `after`, and how many of `before`'s lines changed.
fn difference(before: &str, after: &str) -> (usize, String) {
    let (a, b): (Vec<&str>, Vec<&str>) = (before.lines().collect(), after.lines().collect());
    let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let tail = a[head..].iter().rev().zip(b[head..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (gone, new) = (&a[head..a.len() - tail], &b[head..b.len() - tail]);
    let mut lines: Vec<String> = gone.iter().map(|l| format!("-{l}")).collect();
    lines.extend(new.iter().enumerate().map(|(i, l)| format!("+{:>5}\t{l}", head + i + 1)));
    let total = lines.len();
    if total > MAX_DIFF_LINES {
        lines.truncate(MAX_DIFF_LINES);
        lines.push(format!(
            "… ({} more lines; Read lines {}-{} for the rest)",
            total - MAX_DIFF_LINES,
            head + 1,
            head + new.len()
        ));
    }
    (gone.len().max(new.len()), lines.join("\n"))
}

/// Format `path` as it is on disk now, if its formatter leaves `original` (the text before the
/// edit) as it is. `created` is a file the edit made. Returns the formatted text when it
/// differs from what is on disk; the caller writes it.
pub(crate) async fn format(
    cfg: &FormatConfig,
    root: &Path,
    path: &Path,
    original: Option<&str>,
    created: bool,
) -> Option<Formatted> {
    let ext = path.extension().and_then(|e| e.to_str())?.to_ascii_lowercase();
    let named = cfg.commands.get(&ext);
    let commands = match named {
        Some(cmd) if cmd.is_empty() => return None,
        Some(cmd) => vec![cmd.clone()],
        None => builtin(&ext, path, root),
    };
    let current = tokio::fs::read_to_string(path).await.ok()?;
    for command in commands {
        match original.filter(|o| !created && !o.is_empty()) {
            // The project's file was not in this formatter's style: it stays the project's.
            Some(before) => match pipe(&command, path, before).await {
                Ran::NotInstalled => continue,
                Ran::Failed => return None,
                Ran::Text(t) if t != before => return None,
                Ran::Text(_) => {}
            },
            // Nothing to compare with: only a formatter the settings name.
            None if named.is_none() => return None,
            None => {}
        }
        return match pipe(&command, path, &current).await {
            Ran::NotInstalled => continue,
            Ran::Failed => None,
            Ran::Text(t) if t == current => None,
            Ran::Text(text) => {
                let (lines, diff) = difference(&current, &text);
                let formatter = Path::new(&command[0]).file_name()?.to_string_lossy().into_owned();
                Some(Formatted { formatter, text, lines, diff })
            }
        };
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_setting_is_off_unless_asked_for() {
        assert_eq!(FormatConfig::from_settings(None, None), None);
        assert_eq!(FormatConfig::from_settings(Some(false), Some(&json!({"py": ["black"]}))), None);
        let on =
            FormatConfig::from_settings(Some(true), Some(&json!({".PY": ["black", "-q", "-"], "rs": []}))).unwrap();
        assert_eq!(on.commands["py"], ["black", "-q", "-"]);
        assert!(on.commands["rs"].is_empty());
    }

    #[test]
    fn the_edition_comes_from_the_nearest_manifest_that_states_one() {
        let d = tempfile::tempdir().unwrap();
        let member = d.path().join("crates/a/src");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(d.path().join("Cargo.toml"), "[workspace.package]\nedition = \"2024\"\n").unwrap();
        std::fs::write(d.path().join("crates/a/Cargo.toml"), "[package]\nedition.workspace = true\n").unwrap();
        assert_eq!(rust_edition(&member.join("lib.rs")), "2024");
        std::fs::write(d.path().join("crates/a/Cargo.toml"), "[package]\nedition = \"2018\"\n").unwrap();
        assert_eq!(rust_edition(&member.join("lib.rs")), "2018");
    }

    #[test]
    fn the_difference_shows_what_moved_and_is_clipped() {
        let (n, d) = difference("a\nb  =  1\nc\n", "a\nb = 1\nc\n");
        assert_eq!((n, d.as_str()), (1, "-b  =  1\n+    2\tb = 1"));
        let before: String = (0..100).map(|i| format!("x{i}  =  1\n")).collect();
        let after = before.replace("  =  ", " = ");
        let (n, d) = difference(&before, &after);
        assert_eq!(n, 100);
        assert!(d.lines().count() == MAX_DIFF_LINES + 1 && d.contains("more lines; Read lines 1-100"), "{d}");
    }

    /// A stand-in formatter (`tr`, where there is one): upper-cases what it is given.
    fn upper() -> Option<FormatConfig> {
        std::process::Command::new("tr").arg("a").arg("a").stdin(Stdio::null()).output().ok()?;
        FormatConfig::from_settings(Some(true), Some(&json!({"zz": ["tr", "a-z", "A-Z"]})))
    }

    #[tokio::test]
    async fn a_file_is_formatted_only_if_it_was_formatted_before_the_edit() {
        let Some(cfg) = upper() else { return };
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.zz");
        // Formatted before (all upper case), the edit added a lower-case line: formatted.
        std::fs::write(&f, "ONE\ntwo\nTHREE\n").unwrap();
        let done = format(&cfg, d.path(), &f, Some("ONE\nTHREE\n"), false).await.unwrap();
        assert_eq!((done.text.as_str(), done.lines, done.formatter.as_str()), ("ONE\nTWO\nTHREE\n", 1, "tr"));
        assert!(done.note("a.zz").contains("tr reformatted a.zz") && done.note("a.zz").contains("+    2\tTWO"));
        // Not in the formatter's style before the edit: left as the project has it.
        assert_eq!(format(&cfg, d.path(), &f, Some("One\nTHREE\n"), false).await, None);
        // Already formatted after the edit: nothing to do.
        std::fs::write(&f, "ONE\nTWO\n").unwrap();
        assert_eq!(format(&cfg, d.path(), &f, Some("ONE\n"), false).await, None);
        // A new file: formatted, since the settings name the formatter.
        std::fs::write(&f, "new\n").unwrap();
        assert_eq!(format(&cfg, d.path(), &f, None, true).await.unwrap().text, "NEW\n");
    }

    #[tokio::test]
    async fn a_missing_or_failing_formatter_and_an_unknown_language_do_nothing() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.zz");
        std::fs::write(&f, "text\n").unwrap();
        let missing = FormatConfig::from_settings(Some(true), Some(&json!({"zz": ["kodact-no-such-formatter"]})));
        assert_eq!(format(&missing.unwrap(), d.path(), &f, Some("TEXT\n"), false).await, None);
        let failing = FormatConfig::from_settings(Some(true), Some(&json!({"zz": ["false"]})));
        assert_eq!(format(&failing.unwrap(), d.path(), &f, None, true).await, None);
        let off = FormatConfig::from_settings(Some(true), Some(&json!({"zz": []})));
        assert_eq!(format(&off.unwrap(), d.path(), &f, None, true).await, None);
        // Built-in formatters never touch a new file, and there is none for this extension.
        let builtin_only = FormatConfig::default();
        assert_eq!(format(&builtin_only, d.path(), &f, None, true).await, None);
        let rs = d.path().join("new.rs");
        std::fs::write(&rs, "fn  main ( ) { }\n").unwrap();
        assert_eq!(format(&builtin_only, d.path(), &rs, None, true).await, None);
    }

    /// With the real formatter, when this machine has it.
    #[tokio::test]
    async fn rustfmt_formats_an_edit_to_a_formatted_file_and_leaves_an_unformatted_one() {
        if std::process::Command::new("rustfmt").arg("--version").output().is_err() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("lib.rs");
        let before = "fn one() -> u32 {\n    1\n}\n";
        std::fs::write(&f, "fn one() -> u32 {\n    1\n}\nfn two()->u32{2}\n").unwrap();
        let done = format(&FormatConfig::default(), d.path(), &f, Some(before), false).await.unwrap();
        assert!(done.text.ends_with("fn two() -> u32 {\n    2\n}\n"), "{}", done.text);
        assert_eq!(format(&FormatConfig::default(), d.path(), &f, Some("fn one()->u32{1}\n"), false).await, None);
    }
}
