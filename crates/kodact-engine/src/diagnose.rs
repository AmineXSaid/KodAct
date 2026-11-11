//! Diagnostics after edits (GOALS pillar 3).
//!
//! A file the model just wrote that no longer parses is told with the edit's own result, instead
//! of several turns later when a check finally runs on it. This is the part that needs no
//! language server and no project setup: the language's own parser, run on the one file, when
//! it is installed. It says nothing about types or other files; the project's checks and
//! `PostToolUse` hooks do that.
//!
//! Nothing is added to a result when the file parses, when its language has no parser here, or
//! when the parser is missing or slow: a strong model on a healthy file never sees this.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

/// How long a parser may take on one file before its answer is dropped.
const TIMEOUT: Duration = Duration::from_secs(10);
/// A parser's first errors, not its whole output: the size of a message, whatever the model.
const MAX_LINES: usize = 12;
const MAX_CHARS: usize = 1_500;

/// Parses the file named as its argument and prints `file:line:col: SyntaxError: msg`.
const PYTHON: &str = "import ast,sys\np=sys.argv[1]\ntry:\n ast.parse(open(p,'rb').read(),p)\nexcept SyntaxError as e:\n print('%s:%s:%s: SyntaxError: %s'%(p,e.lineno,e.offset,e.msg));sys.exit(1)\nexcept (ValueError,OSError):\n pass\n";

/// What a file failed to parse with, and what the parser said.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Diagnostic {
    pub parser: &'static str,
    pub message: String,
}

impl Diagnostic {
    /// The text added to the edit's result.
    pub fn text(&self, file: &str) -> String {
        format!(
            "\n\n<system-reminder>\nAfter this change, {file} does not parse ({}):\n{}\nFix it before going \
             on, unless a later edit you already planned completes it.\n</system-reminder>",
            self.parser, self.message
        )
    }
}

/// The parsers tried for a file, by extension: each a program and the arguments before the path.
fn parsers(path: &Path) -> &'static [(&'static str, &'static [&'static str])] {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    match ext.as_str() {
        "py" => &[("python3", &["-c", PYTHON]), ("python", &["-c", PYTHON])],
        "js" | "mjs" | "cjs" => &[("node", &["--check"])],
        "sh" => &[("sh", &["-n"])],
        "bash" => &[("bash", &["-n"])],
        "rb" => &[("ruby", &["-c"])],
        "php" => &[("php", &["-l"])],
        _ => &[],
    }
}

/// JSON is parsed here. Files written with comments (JSONC: editor and compiler settings) are
/// not strict JSON on purpose and are left alone.
fn check_json(text: &str) -> Option<String> {
    if text.contains("//") || text.contains("/*") {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(text).err().map(|e| e.to_string())
}

fn clip(output: &str) -> String {
    let mut out = String::new();
    for line in output.lines().map(str::trim_end).filter(|l| !l.is_empty()).take(MAX_LINES) {
        if out.len() + line.len() > MAX_CHARS {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// Does `path`, as it is on disk now, parse? `None` when it does, or when nothing here can tell.
pub(crate) async fn check(path: &Path) -> Option<Diagnostic> {
    let is_json = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("json"));
    if is_json {
        let text = tokio::fs::read_to_string(path).await.ok()?;
        return check_json(&text).map(|message| Diagnostic { parser: "JSON", message });
    }
    for (program, args) in parsers(path) {
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(*args).arg(path).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.kill_on_drop(true);
        if let Some(dir) = path.parent() {
            cmd.current_dir(dir);
        }
        // Not installed: try the next name, or say nothing.
        let Ok(child) = cmd.spawn() else { continue };
        let Ok(Ok(out)) = tokio::time::timeout(TIMEOUT, child.wait_with_output()).await else { return None };
        if out.status.success() {
            return None;
        }
        // Killed, or the program itself failed to start properly: not a verdict on the file.
        out.status.code()?;
        let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        let message = clip(&said);
        return (!message.is_empty()).then_some(Diagnostic { parser: program, message });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_is_checked_and_jsonc_left_alone() {
        assert!(check_json(r#"{"a": 1}"#).is_none());
        assert!(check_json(r#"{"a": 1,}"#).unwrap().contains("line 1"));
        assert!(check_json("{\n  // editor settings\n  \"a\": 1,\n}").is_none());
    }

    #[test]
    fn only_known_languages_have_a_parser() {
        assert!(!parsers(Path::new("a.py")).is_empty());
        assert!(!parsers(Path::new("a.MJS")).is_empty());
        assert!(parsers(Path::new("a.rs")).is_empty(), "a build, not a parser: left to the checks");
        assert!(parsers(Path::new("a.ts")).is_empty());
        assert!(parsers(Path::new("Makefile")).is_empty());
    }

    #[test]
    fn long_output_is_clipped_to_its_first_errors() {
        let many = (0..100).map(|i| format!("a.py:{i}: error")).collect::<Vec<_>>().join("\n\n");
        assert_eq!(clip(&many).lines().count(), MAX_LINES);
        assert!(clip(&"x".repeat(5_000)).is_empty(), "one endless line says nothing useful");
    }

    #[tokio::test]
    async fn a_broken_file_is_reported_and_a_good_one_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "{\"a\": }").unwrap();
        let d = check(&bad).await.unwrap();
        assert_eq!(d.parser, "JSON");
        assert!(d.text("bad.json").contains("bad.json does not parse (JSON)"));
        let good = dir.path().join("good.json");
        std::fs::write(&good, "{\"a\": 1}").unwrap();
        assert_eq!(check(&good).await, None);
        // No parser for the language, or a file that isn't there: nothing is said.
        let rust = dir.path().join("a.rs");
        std::fs::write(&rust, "fn main( {").unwrap();
        assert_eq!(check(&rust).await, None);
        assert_eq!(check(&dir.path().join("missing.json")).await, None);
    }

    /// With the language's own parser, when this machine has it.
    #[tokio::test]
    async fn python_and_shell_syntax_errors_name_the_line() {
        let dir = tempfile::tempdir().unwrap();
        let has = |p: &str| std::process::Command::new(p).arg("--version").output().is_ok();
        if has("python3") || has("python") {
            let f = dir.path().join("m.py");
            std::fs::write(&f, "def f(:\n    pass\n").unwrap();
            let d = check(&f).await.unwrap();
            assert!(d.message.contains(":1:") && d.message.contains("SyntaxError"), "{}", d.message);
            std::fs::write(&f, "def f():\n    pass\n").unwrap();
            assert_eq!(check(&f).await, None);
            assert!(!dir.path().join("__pycache__").exists(), "the check leaves no file behind");
        }
        if has("sh") {
            let f = dir.path().join("s.sh");
            std::fs::write(&f, "if true; then\necho x\n").unwrap();
            assert!(check(&f).await.is_some());
            std::fs::write(&f, "if true; then\necho x\nfi\n").unwrap();
            assert_eq!(check(&f).await, None);
        }
    }
}
