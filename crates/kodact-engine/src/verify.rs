//! The verification loop (GOALS pillar 3).
//!
//! A turn that changed files may not end until the project's checks have run
//! since the last change. Otherwise the model gets one reminder (configurable)
//! naming the changed files and the detected check commands, and the loop goes
//! on. The final answer is asked to say what was verified and what was not.
//!
//! Evidence is a Bash call, made after the last change, that runs a check: a
//! configured or detected command, a well-known test/build/lint tool, a
//! `build*`/`test*`/`eval*`/`bench*` script, one of the changed files itself, a
//! program run on a changed file (`./sim -f input.cfg`), or inline code that
//! loads a changed module. Viewers, file managers, editors and version control
//! naming a changed file are not checks, and here-document bodies are data.
//!
//! Changes are the edit tools' writes, from the file history, so sub-agent
//! edits count too. Shell commands that may write are confirmed against a
//! fingerprint of the files (not of git's state), so a `sed -i` counts and an
//! `ls` or a `git commit` does not.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Settings for the verification loop (`verification` in settings).
#[derive(Debug, Clone, PartialEq)]
pub struct VerifyConfig {
    /// The project's check commands: `verification.commands`, else detected.
    pub commands: Vec<String>,
    /// Reminders per user turn (`verification.maxReminders`, default 1).
    pub max_reminders: u32,
}

impl Default for VerifyConfig {
    fn default() -> Self {
        VerifyConfig { commands: vec![], max_reminders: 1 }
    }
}

/// Check commands for the project at `dir`, read from its manifests.
pub fn detect_checks(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let has = |f: &str| dir.join(f).exists();
    let mut push = |c: &str| {
        if !out.iter().any(|o| o == c) {
            out.push(c.to_string());
        }
    };
    if has("Cargo.toml") {
        push("cargo build");
        push("cargo test");
    }
    if let Some(scripts) = std::fs::read_to_string(dir.join("package.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("scripts").cloned())
    {
        let runner = if has("pnpm-lock.yaml") {
            "pnpm"
        } else if has("yarn.lock") {
            "yarn"
        } else if has("bun.lockb") || has("bun.lock") {
            "bun"
        } else {
            "npm"
        };
        for name in ["typecheck", "lint", "build", "test"] {
            let Some(body) = scripts.get(name).and_then(Value::as_str) else { continue };
            if name == "test" && body.contains("no test specified") {
                continue;
            }
            match (runner, name) {
                ("npm", "test") => push("npm test"),
                ("npm", n) => push(&format!("npm run {n}")),
                (r, n) => push(&format!("{r} {n}")),
            }
        }
    }
    if has("go.mod") {
        push("go build ./...");
        push("go test ./...");
    }
    let pyproject = std::fs::read_to_string(dir.join("pyproject.toml")).unwrap_or_default();
    if has("pytest.ini") || has("conftest.py") || pyproject.contains("[tool.pytest") || has("tox.ini") {
        push("python3 -m pytest");
    } else if has_python_tests(&dir.join("tests")) || has_python_tests(dir) {
        push("python3 -m unittest");
    }
    if let Ok(make) = std::fs::read_to_string(dir.join("Makefile")) {
        for target in ["check", "test"] {
            if make.lines().any(|l| l.starts_with(&format!("{target}:"))) {
                push(&format!("make {target}"));
            }
        }
    }
    for script in ["build.sh", "test.sh", "check.sh"] {
        if has(script) {
            push(&format!("./{script}"));
        }
    }
    if has("deno.json") || has("deno.jsonc") {
        push("deno test");
    }
    if has("pom.xml") {
        push("mvn -q test");
    }
    if has("build.gradle") || has("build.gradle.kts") {
        push(if has("gradlew") { "./gradlew test" } else { "gradle test" });
    }
    if has("Gemfile") && has("spec") {
        push("bundle exec rspec");
    }
    if has("mix.exs") {
        push("mix test");
    }
    if has("Package.swift") {
        push("swift test");
    }
    out
}

fn has_python_tests(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten().any(|e| {
                let n = e.file_name().to_string_lossy().to_string();
                (n.starts_with("test_") || n.ends_with("_test.py")) && n.ends_with(".py")
            })
        })
        .unwrap_or(false)
}

/// Well-known check tools, matched against a command's leading words.
const CHECK_PREFIXES: &[&str] = &[
    "cargo test",
    "cargo build",
    "cargo check",
    "cargo clippy",
    "cargo nextest",
    "cargo run",
    "npm test",
    "npm t",
    "npm run",
    "pnpm test",
    "pnpm run",
    "pnpm build",
    "pnpm lint",
    "pnpm typecheck",
    "yarn test",
    "yarn run",
    "yarn build",
    "yarn lint",
    "yarn typecheck",
    "bun test",
    "bun run",
    "npx tsc",
    "npx jest",
    "npx vitest",
    "npx eslint",
    "tsc",
    "jest",
    "vitest",
    "eslint",
    "node --test",
    "deno test",
    "deno check",
    "pytest",
    "py.test",
    "python -m pytest",
    "python3 -m pytest",
    "python -m unittest",
    "python3 -m unittest",
    "python -m mypy",
    "python3 -m mypy",
    "mypy",
    "ruff",
    "flake8",
    "pylint",
    "pyright",
    "tox",
    "nox",
    "python -m py_compile",
    "python3 -m py_compile",
    "python -m compileall",
    "python3 -m compileall",
    "go test",
    "go build",
    "go vet",
    "go run",
    "make",
    "cmake --build",
    "ctest",
    "ninja",
    "mvn",
    "gradle",
    "./gradlew",
    "dotnet test",
    "dotnet build",
    "bundle exec rspec",
    "rspec",
    "rake test",
    "bundle exec rake",
    "phpunit",
    "vendor/bin/phpunit",
    "composer test",
    "mix test",
    "swift test",
    "swift build",
    "zig build",
    "stack test",
    "cabal test",
    "dune test",
    "dune build",
    "gcc",
    "g++",
    "clang",
    "javac",
];

const INTERPRETERS: &[&str] =
    &["python", "python3", "node", "bash", "sh", "ruby", "perl", "php", "deno", "bun", "tsx", "ts-node"];

/// Words a command can start with that don't change what it runs.
fn skip_wrappers(words: &[String]) -> &[String] {
    let mut i = 0;
    while i < words.len() {
        let w = words[i].as_str();
        let assignment = w.contains('=') && !w.starts_with('-') && !w.starts_with('=');
        // Shell keywords that start a command (`do ./sim …` inside a loop, `if pytest; then`) count
        // as wrappers too.
        let keyword = matches!(w, "if" | "then" | "else" | "elif" | "do" | "while" | "until" | "!" | "{" | "sudo");
        let step = if assignment
            || keyword
            || matches!(w, "env" | "time" | "nice" | "nohup" | "command" | "exec" | "xvfb-run")
        {
            1
        } else if w == "timeout"
            || (matches!(w, "uv" | "poetry" | "pipenv" | "hatch" | "pdm" | "rye")
                && words.get(i + 1).map(String::as_str) == Some("run"))
        {
            2
        } else {
            break;
        };
        i += step;
    }
    &words[i.min(words.len())..]
}

fn words(part: &str) -> Vec<String> {
    part.split_whitespace().map(|w| w.trim_matches(|c| c == '\'' || c == '"').to_string()).collect()
}

fn starts_with_words(words: &[String], prefix: &str) -> bool {
    let p: Vec<&str> = prefix.split_whitespace().collect();
    words.len() >= p.len() && words.iter().zip(&p).all(|(w, p)| w == p)
}

fn is_check_script(word: &str) -> bool {
    let name = word.rsplit('/').next().unwrap_or(word).to_ascii_lowercase();
    ["build", "test", "check", "run_tests", "runtests", "verify", "ci", "eval", "bench"]
        .iter()
        .any(|p| name.starts_with(p))
        && (word.contains('/') || name.ends_with(".sh") || name.ends_with(".py"))
}

/// Programs that show, move, edit, install or archive a file without running it, and
/// shell syntax that isn't a program: naming a changed file doesn't make them a check.
const NOT_CHECKS: &[&str] = &[
    // viewers and file information
    "cat",
    "head",
    "tail",
    "less",
    "more",
    "bat",
    "batcat",
    "nl",
    "wc",
    "file",
    "stat",
    "ls",
    "tree",
    "du",
    "readlink",
    "realpath",
    "basename",
    "dirname",
    "xxd",
    "od",
    "hexdump",
    "strings",
    "test",
    "[",
    "[[",
    // file management and transfer
    "cp",
    "mv",
    "rm",
    "ln",
    "touch",
    "mkdir",
    "rmdir",
    "chmod",
    "chown",
    "chgrp",
    "install",
    "rsync",
    "scp",
    "dd",
    "truncate",
    "curl",
    "wget",
    // editors and openers
    "vi",
    "vim",
    "nvim",
    "nano",
    "emacs",
    "ed",
    "code",
    "open",
    "xdg-open",
    // text tools
    "echo",
    "printf",
    "sed",
    "awk",
    "gawk",
    "grep",
    "egrep",
    "fgrep",
    "rg",
    "ag",
    "find",
    "tee",
    "sort",
    "uniq",
    "cut",
    "tr",
    "diff",
    "cmp",
    "comm",
    "paste",
    "join",
    "fold",
    "fmt",
    "column",
    "iconv",
    "base64",
    "md5sum",
    "cksum",
    "b2sum",
    // version control and archives
    "git",
    "hg",
    "svn",
    "tar",
    "zip",
    "unzip",
    "gzip",
    "gunzip",
    "bzip2",
    "bunzip2",
    "xz",
    "unxz",
    "zcat",
    "7z",
    // formatters (they rewrite the file) and package managers
    "black",
    "isort",
    "autopep8",
    "yapf",
    "rustfmt",
    "gofmt",
    "goimports",
    "clang-format",
    "prettier",
    "shfmt",
    "pip",
    "pip3",
    "uv",
    "conda",
    "mamba",
    "apt",
    "apt-get",
    "brew",
    "npm",
    "pnpm",
    "yarn",
    // shell syntax and builtins
    "for",
    "case",
    "select",
    "in",
    "done",
    "esac",
    "fi",
    "export",
    "cd",
    "pushd",
    "popd",
    "unset",
    "set",
    "local",
    "declare",
    "read",
    "wait",
    "sleep",
    "kill",
    "exit",
    "return",
    "true",
    "false",
    ":",
];

/// Words in a script's name that say it changes something outside the project, or destroys:
/// such a script named in a request is work to do, not a check.
const NOT_CHECK_SCRIPTS: [&str; 10] =
    ["deploy", "publish", "release", "install", "push", "upload", "migrate", "drop", "delete", "clean"];

/// A command the person's request names that the verification loop may treat as one of the
/// project's checks: what [`is_check`] recognizes, or a script in the project run directly or
/// by an interpreter (`./verify.sh`, `python3 run_checks.py`) whose name doesn't say it
/// deploys, publishes or deletes. KodAct only names it to the model; it never runs it.
pub(crate) fn named_check(command: &str, root: &Path) -> bool {
    if is_check(command, &[], &[]) {
        return true;
    }
    let parts = kodact_permissions::split_compound(command);
    let Some(part) = parts.first() else { return false };
    let all = words(part);
    let w = skip_wrappers(&all);
    let Some(first) = w.first() else { return false };
    let prog = program(first);
    if not_a_check(prog) {
        return false;
    }
    let script = if first.contains('/') {
        Some(first.as_str())
    } else if INTERPRETERS.contains(&prog) || is_shell(prog) {
        w.get(1).map(String::as_str).filter(|a| !a.starts_with('-'))
    } else {
        None
    };
    script.is_some_and(|s| {
        let name = program(s).to_ascii_lowercase();
        root.join(s).is_file() && !NOT_CHECK_SCRIPTS.iter().any(|v| name.contains(v))
    })
}

fn not_a_check(program: &str) -> bool {
    NOT_CHECKS.contains(&program) || (program.starts_with("sha") && program.ends_with("sum"))
}

/// The name a command word runs: `./tools/sim` → `sim`.
fn program(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

fn is_changed(arg: &str, changed: &[PathBuf]) -> bool {
    let t = arg.trim_start_matches("./");
    !t.is_empty()
        && changed.iter().any(|c| {
            let c = c.to_string_lossy();
            c == t || c.ends_with(&format!("/{t}"))
        })
}

/// The files a command part names as arguments or input. Flags are skipped (a flag's
/// `=value` counts), and so are output redirection targets (`> out`, `2>err`), which are
/// written rather than run, and here-document delimiters. `< input` counts.
fn file_args(args: &[String]) -> Vec<&str> {
    let mut out = vec![];
    let mut skip_next = false;
    for a in args {
        if std::mem::take(&mut skip_next) {
            continue;
        }
        let a = a.as_str();
        if let Some(rest) =
            ["&>>", "&>", "2>>", "2>", "1>>", "1>", ">>", ">|", ">"].iter().find_map(|op| a.strip_prefix(op))
        {
            skip_next = rest.is_empty();
            continue;
        }
        if a.starts_with("<<") {
            skip_next = matches!(a, "<<" | "<<-" | "<<<");
            continue;
        }
        if let Some(rest) = a.strip_prefix('<') {
            if !rest.is_empty() {
                out.push(rest);
            }
            continue;
        }
        if a.starts_with('-') {
            if let Some((_, value)) = a.split_once('=') {
                out.push(value);
            }
            continue;
        }
        out.push(a);
    }
    out
}

/// Does the command edit its file arguments in place (`perl -pi -e …`, `--in-place`)?
/// Only for programs whose `-i` means that: elsewhere `-i` is often an input flag.
fn in_place(prog: &str, args: &[String]) -> bool {
    args.iter().any(|a| {
        a == "--in-place"
            || a.starts_with("--in-place=")
            || a == "--write"
            || (matches!(prog, "perl" | "ruby")
                && a.starts_with('-')
                && !a.starts_with("--")
                && a[1..]
                    .split('.')
                    .next()
                    .is_some_and(|f| f.contains('i') && f.chars().all(|c| c.is_ascii_alphabetic())))
    })
}

/// The command a shell runs with `-c` (`bash -c "cd x && pytest"`, `sh -ec '…'`). Whether
/// it is a check is decided on that command, not on the shell.
fn shell_command(prog: &str, args: &[String]) -> Option<String> {
    if !is_shell(prog) {
        return None;
    }
    let i = args.iter().position(|a| {
        a.len() >= 2
            && a.starts_with('-')
            && !a.starts_with("--")
            && a.ends_with('c')
            && a[1..].chars().all(|c| c.is_ascii_alphabetic())
    })?;
    Some(args[i + 1..].join(" "))
}

fn is_shell(prog: &str) -> bool {
    matches!(prog, "bash" | "sh" | "zsh" | "dash" | "ksh")
}

/// The command without its here-document bodies: the lines between `<<EOF` and `EOF` are
/// data (a file being written, code fed to an interpreter), not commands. A here-document
/// whose end line is missing is kept as is.
fn strip_heredoc_bodies(command: &str) -> String {
    split_heredocs(command).0
}

/// The command's here-documents, as (the line that opens one, its body).
fn heredoc_bodies(command: &str) -> Vec<(String, String)> {
    split_heredocs(command).1
}

fn split_heredocs(command: &str) -> (String, Vec<(String, String)>) {
    let lines: Vec<&str> = command.lines().collect();
    let (mut kept, mut bodies) = (vec![], vec![]);
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        kept.push(line);
        i += 1;
        for delim in heredoc_delimiters(line) {
            if let Some(end) = lines[i..].iter().position(|l| l.trim() == delim) {
                bodies.push((line.to_string(), lines[i..i + end].join("\n")));
                i += end + 1;
            }
        }
    }
    (kept.join("\n"), bodies)
}

/// The delimiters of the here-documents a line opens: `<<EOF`, `<< 'EOF'`, `<<-"END"`.
fn heredoc_delimiters(line: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = line;
    while let Some(i) = rest.find("<<") {
        let after = &rest[i + 2..];
        if let Some(herestring) = after.strip_prefix('<') {
            rest = herestring;
            continue;
        }
        let after = after.strip_prefix('-').unwrap_or(after).trim_start();
        let quoted = after.starts_with('\'') || after.starts_with('"');
        let word: String =
            after.trim_start_matches(['\'', '"']).chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if !word.is_empty() && (quoted || word.starts_with(|c: char| c.is_alphabetic() || c == '_')) {
            out.push(word);
        }
        rest = after;
    }
    out
}

/// Inline code (`python3 -c`, `node -e`), stdin (`python3 -`) or a heredoc, as
/// opposed to running a named script. These forms carry the check in the command
/// itself, so [`references_changed`] decides whether it exercises a changed file.
fn runs_inline_code(args: &[String]) -> bool {
    args.iter().any(|a| a == "-c" || a == "-e" || a == "-" || a.starts_with("<<"))
}

/// Does `command` run the changed code inline? True when it names a changed file
/// (`ast.parse(open('algo.py'))`) or imports a changed module by its stem
/// (`import algo`, `from algo import map`, `require('algo')`). The code body is
/// matched against the whole command because `split_compound` can break it on a
/// `;` inside the string.
fn references_changed(command: &str, changed: &[PathBuf]) -> bool {
    if changed.is_empty() {
        return false;
    }
    let ws = words(command);
    changed.iter().any(|c| {
        let name = c.file_name().and_then(|n| n.to_str()).unwrap_or("");
        // A package's `__init__.py` is imported by the package's name: `from vectorops import dot`.
        let stem = if name == "__init__.py" {
            c.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or("")
        } else {
            c.file_stem().and_then(|n| n.to_str()).unwrap_or("")
        };
        if !name.is_empty() && name != "__init__.py" && command.contains(name) {
            return true; // the file named directly, e.g. open('algo.py')
        }
        if stem.is_empty() {
            return false;
        }
        // The changed module imported by its stem: `import algo`, `from algo ...`.
        // The module name after `import`/`from` may carry trailing punctuation
        // (`import algo;`), so compare on its identifier part.
        let module = |w: &str| w.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_').to_string();
        let import = ws
            .iter()
            .enumerate()
            .any(|(i, w)| (w == "import" || w == "from") && ws.get(i + 1).is_some_and(|n| module(n) == stem));
        // The module named in a string, however the code loads it: `require('./util')`,
        // `importlib.import_module('optimizer')`, `for m in ['optimizer', 'sweep']`,
        // `__import__('algo')`. A bare stem must be 3 characters or more: `'a'` is any string.
        let relative = [format!("'./{stem}'"), format!("\"./{stem}\"")];
        let bare = [format!("'{stem}'"), format!("\"{stem}\"")];
        import
            || relative.iter().any(|p| command.contains(p))
            || (stem.chars().count() >= 3 && bare.iter().any(|p| command.contains(p)))
    })
}

/// Does this Bash command run a check? `changed` are the files written this
/// turn, relative or absolute; running one of them, or running a program on one,
/// counts as a check.
pub fn is_check(command: &str, configured: &[String], changed: &[PathBuf]) -> bool {
    let commands = strip_heredoc_bodies(command);
    let parts = kodact_permissions::split_compound(&commands);
    if parts.iter().any(|part| part_is_check(part, configured, changed)) {
        return true;
    }
    // A here-document fed to a shell (`bash <<'EOF' … EOF`) runs its lines as commands.
    for (opener, body) in heredoc_bodies(command) {
        let to_shell = kodact_permissions::split_compound(&opener).iter().any(|p| {
            let all = words(p);
            let w = skip_wrappers(&all);
            w.first().is_some_and(|f| is_shell(program(f))) && w.iter().any(|a| a.starts_with("<<"))
        });
        if to_shell && is_check(&body, configured, changed) {
            return true;
        }
    }
    // Inline interpreter checks the per-part scan can't see: `python3 -c "import
    // algo; assert ..."` or a heredoc running the changed code. Verifying a
    // scripts/data project with no test runner is the common real case.
    let inline_interp = parts.iter().any(|part| {
        let all = words(part);
        let w = skip_wrappers(&all);
        w.first()
            .map(|f| {
                let prog = program(f);
                // A shell's inline code is judged on its commands (`bash -c`, a here-document).
                INTERPRETERS.contains(&prog) && !is_shell(prog) && runs_inline_code(&w[1..]) && !in_place(prog, &w[1..])
            })
            .unwrap_or(false)
    });
    inline_interp && references_changed(command, changed)
}

fn part_is_check(part: &str, configured: &[String], changed: &[PathBuf]) -> bool {
    let all = words(part);
    let w = skip_wrappers(&all);
    let Some(first) = w.first() else { return false };
    if configured.iter().map(String::as_str).chain(CHECK_PREFIXES.iter().copied()).any(|c| starts_with_words(w, c)) {
        return true;
    }
    if is_check_script(first) {
        return true;
    }
    let prog = program(first);
    if let Some(inner) = shell_command(prog, &w[1..]) {
        return is_check(&inner, configured, changed);
    }
    let interp = INTERPRETERS.contains(&prog);
    let args: Vec<&String> = if interp { w.iter().skip(1).filter(|a| !a.starts_with('-')).collect() } else { vec![] };
    if args.first().map(|a| is_check_script(a) || a.contains("test")).unwrap_or(false) {
        return true;
    }
    // Running a changed file: `python3 convert.py x.csv`, `./tool.sh`.
    let target = if interp { args.first().map(|a| a.as_str()) } else { Some(first.as_str()) };
    if target.is_some_and(|t| is_changed(t, changed)) {
        return true;
    }
    // A program run on a changed file: `./sim -f input.cfg`, `shellcheck x.sh`,
    // `./app --config=config.json`, `./prog < input.txt`. Not a viewer, file manager, editor
    // or VCS (`cat f`, `cp f g`, `git add f`), inline code (decided by `references_changed`),
    // an in-place edit (`perl -pi -e … f`), an installer module (`python3 -m pip install -r
    // requirements.txt`) or a redirect target (`echo x > f`).
    let module = w.iter().position(|a| a == "-m").and_then(|i| w.get(i + 1)).map(String::as_str);
    if not_a_check(prog)
        || (interp && runs_inline_code(&w[1..]))
        || in_place(prog, &w[1..])
        || (interp && matches!(module, Some("pip" | "venv" | "ensurepip" | "virtualenv")))
    {
        return false;
    }
    file_args(&w[1..]).iter().any(|a| is_changed(a, changed))
}

/// Per-turn verification state.
#[derive(Debug, Default)]
pub(crate) struct Tracker {
    /// File-history writes already covered by a check.
    pub write_mark: usize,
    /// A shell command that may have written ran since the last check.
    pub shell_may_have_changed: bool,
    /// Worktree fingerprint when the turn started or the last check ran.
    pub fingerprint: Option<u64>,
    /// The last check's command and whether it failed.
    pub last_check: Option<(String, bool)>,
    pub reminders: u32,
    /// Checks still running in the background (they outlived their Bash timeout): the shell's ID,
    /// the command, and the file-history writes when it moved there.
    pub pending: Vec<(String, String, usize)>,
    /// Files read back (Read, or a viewer like `cat` or `od`), as named, with the file-history
    /// writes at the time: a plain data file read after its last change has been checked.
    pub read_back: Vec<(String, usize)>,
    /// The writes and the worktree fingerprint when the last reminder went out: another comes
    /// only after something changed since.
    pub reminded_at: Option<(usize, Option<u64>)>,
    /// The last passing check built a program (its command and the program's path), and
    /// nothing has run that program since: compiling isn't seeing it work.
    pub built: Option<(String, String)>,
}

/// Plain data a task produces (an answer, a table, a log, notes): no program in the project runs
/// it, so reading it back is how it gets checked. Code and configuration aren't here: they are
/// checked by running what uses them.
const DATA_EXTENSIONS: &[&str] = &["txt", "csv", "tsv", "md", "log", "out"];

/// Is `path` plain data ([`DATA_EXTENSIONS`])?
pub(crate) fn is_data_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| DATA_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Programs that read a file back for a look or a comparison.
const VIEWERS: &[&str] =
    &["cat", "head", "tail", "less", "more", "bat", "batcat", "nl", "od", "xxd", "hexdump", "diff", "cmp"];

/// The files a Bash command reads back: `cat answer.txt`, `od -c out.txt | tail -3`,
/// `diff out.csv expected.csv`.
pub(crate) fn files_viewed(command: &str) -> Vec<String> {
    let commands = strip_heredoc_bodies(command);
    kodact_permissions::split_compound(&commands)
        .iter()
        .flat_map(|part| {
            let all = words(part);
            let w = skip_wrappers(&all);
            match w.first() {
                Some(f) if VIEWERS.contains(&program(f)) => {
                    file_args(&w[1..]).into_iter().map(str::to_string).collect::<Vec<_>>()
                }
                _ => vec![],
            }
        })
        .collect()
}

/// Does `arg`, as a command named it, name the written file `path`?
pub(crate) fn names_file(path: &Path, arg: &str) -> bool {
    is_changed(arg, std::slice::from_ref(&path.to_path_buf()))
}

/// Compilers that produce a program to run, as the first word of a command.
const COMPILERS: &[&str] = &["gcc", "cc", "clang", "g++", "c++", "clang++", "tcc", "rustc", "go"];

/// The program a compile command produces: `gcc -O2 -o image image.c` builds `image`, a plain
/// `gcc x.c` builds `a.out`, `rustc main.rs` builds `main`, `go build -o app` builds `app`.
/// `None` when it builds nothing to run (`-c`, `-S`, `-E`, `-fsyntax-only`, a library) or isn't
/// a compile. The last part of a compound command that builds one wins.
pub(crate) fn built_artifact(command: &str) -> Option<String> {
    let commands = strip_heredoc_bodies(command);
    let mut built = None;
    for part in kodact_permissions::split_compound(&commands) {
        let all = words(&part);
        let w = skip_wrappers(&all);
        let Some(first) = w.first() else { continue };
        let prog = program(first);
        if !COMPILERS.contains(&prog) || (prog == "go" && w.get(1).map(String::as_str) != Some("build")) {
            continue;
        }
        let args = &w[1..];
        if args
            .iter()
            .any(|a| matches!(a.as_str(), "-c" | "-S" | "-E" | "-fsyntax-only" | "-shared" | "--crate-type=lib"))
        {
            continue;
        }
        let named = args.iter().enumerate().find_map(|(i, a)| match a.as_str() {
            "-o" => args.get(i + 1).cloned(),
            o if o.starts_with("-o") && o.len() > 2 && prog != "go" => Some(o[2..].to_string()),
            _ => None,
        });
        built = match (named, prog) {
            (Some(n), _) => Some(n),
            (None, "rustc") => args
                .iter()
                .find(|a| a.ends_with(".rs"))
                .and_then(|a| Path::new(a).file_stem())
                .map(|s| s.to_string_lossy().into_owned()),
            (None, "go") => None,
            (None, _) => Some("a.out".to_string()),
        };
    }
    built
}

/// Does `command` run the program at `artifact` (`./image`, `/work/image > out.ppm`,
/// `time ./a.out`)? Its path as the command names it, from the same directory or absolute.
pub(crate) fn runs_artifact(command: &str, artifact: &str) -> bool {
    let commands = strip_heredoc_bodies(command);
    let target = artifact.trim_start_matches("./");
    kodact_permissions::split_compound(&commands).iter().any(|part| {
        let all = words(part);
        let w = skip_wrappers(&all);
        w.first().is_some_and(|f| {
            let f = f.trim_start_matches("./");
            f == target || f.ends_with(&format!("/{target}")) || target.ends_with(&format!("/{f}"))
        })
    })
}

/// Where a turn's changes stand when it ends: the `verification` field of the result, so a
/// script or a CI job can tell checked work from work nobody ran, without reading the answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Verified {
    /// The turn changed no file.
    NoChanges,
    /// A check ran after the last change and passed.
    Passed { command: String },
    /// The only changes are plain data files, read back after their last change.
    ReadBack,
    /// The last check failed, and nothing changed after it.
    Failed { command: String },
    /// Changes no check has run on since: the files the edit tools wrote, or (`shell`) files
    /// changed through shell commands.
    Unchecked { files: Vec<String>, shell: bool },
}

impl Verified {
    pub fn status(&self) -> &'static str {
        match self {
            Verified::NoChanges => "no_changes",
            Verified::Passed { .. } => "passed",
            Verified::ReadBack => "read_back",
            Verified::Failed { .. } => "failed",
            Verified::Unchecked { .. } => "unchecked",
        }
    }

    pub fn record(&self) -> Value {
        let mut v = serde_json::json!({"status": self.status()});
        match self {
            Verified::Passed { command } | Verified::Failed { command } => v["command"] = command.as_str().into(),
            Verified::Unchecked { files, shell } => {
                v["files"] = serde_json::json!(files);
                if *shell {
                    v["shell"] = true.into();
                }
            }
            _ => {}
        }
        v
    }
}

/// What the model is reminded of.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Reminder {
    Unchecked {
        files: Vec<String>,
        shell: bool,
    },
    LastCheckFailed {
        command: String,
    },
    /// A program was built and never run since.
    BuiltNotRun {
        command: String,
        artifact: String,
    },
}

impl Reminder {
    /// The transcript record (`system/verification`), for later analysis.
    pub fn record(&self) -> Value {
        match self {
            Reminder::Unchecked { files, shell } => {
                serde_json::json!({"kind": "unchecked", "files": files, "shell": shell})
            }
            Reminder::LastCheckFailed { command } => {
                serde_json::json!({"kind": "last_check_failed", "command": command})
            }
            Reminder::BuiltNotRun { command, artifact } => {
                serde_json::json!({"kind": "built_not_run", "command": command, "artifact": artifact})
            }
        }
    }

    pub fn text(&self, commands: &[String]) -> String {
        let how = if commands.is_empty() {
            "No check command was detected for this project: run the relevant tests, or run the changed code \
             directly"
                .to_string()
        } else {
            format!(
                "The project's checks: {}",
                commands.iter().map(|c| format!("`{c}`")).collect::<Vec<_>>().join(", ")
            )
        };
        match self {
            Reminder::Unchecked { files, shell } => {
                let what = match (files.is_empty(), shell) {
                    (false, _) => format!("You changed {} this turn", list(files)),
                    (true, _) => "Files in the working tree changed during this turn (through shell commands)".into(),
                };
                format!(
                    "<system-reminder>\n{what}, and no check has run since the last change.\n{how}.\nBefore you \
                     finish, run what applies and fix what fails. If a check can't run here, say why.\nIn your \
                     final answer, state what you verified and what you did not.\n</system-reminder>"
                )
            }
            Reminder::BuiltNotRun { command, artifact } => format!(
                "<system-reminder>\n`{command}` built `{artifact}`, and it hasn't run since: compiling doesn't show \
                 it works.\nRun it once and check its output before you finish. If it can't run here, say so in \
                 your final answer.\n</system-reminder>"
            ),
            Reminder::LastCheckFailed { command } => format!(
                "<system-reminder>\nThe last check (`{command}`) failed, and nothing changed after it.\nFix the \
                 failure, or, if it is expected or unrelated to your change, say so plainly in your final \
                 answer.\n</system-reminder>"
            ),
        }
    }
}

fn list(files: &[String]) -> String {
    const MAX: usize = 8;
    let mut s = files.iter().take(MAX).map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ");
    if files.len() > MAX {
        s.push_str(&format!(" and {} more", files.len() - MAX));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A compile that builds a program, and the commands that run it.
    #[test]
    fn a_built_program_is_known_and_so_is_running_it() {
        assert_eq!(built_artifact("gcc -static -o image image.c -lm").as_deref(), Some("image"));
        assert_eq!(built_artifact("cd /work && gcc -O3 model.c -lm").as_deref(), Some("a.out"));
        assert_eq!(built_artifact("gcc -oprog x.c").as_deref(), Some("prog"));
        assert_eq!(built_artifact("rustc -O main.rs").as_deref(), Some("main"));
        assert_eq!(built_artifact("go build -o bin/app ./cmd").as_deref(), Some("bin/app"));
        for none in ["gcc -c x.c", "gcc -fsyntax-only x.c", "go test ./...", "cargo build", "make", "go build ./..."] {
            assert_eq!(built_artifact(none), None, "{none}");
        }
        assert!(runs_artifact("./image && ls -l reconstructed.ppm", "image"));
        assert!(runs_artifact("time /work/a.out > out.txt", "a.out"));
        assert!(runs_artifact("cd /work && ./bin/app --help", "bin/app"));
        assert!(!runs_artifact("wc -c image.c && head -3 image.c", "image"));
        assert!(!runs_artifact("ls -l image", "image"));
    }

    #[test]
    fn a_named_command_is_a_check_when_it_checks() {
        let d = tempfile::tempdir().unwrap();
        for f in ["run_checks.py", "verify.sh", "deploy.sh", "clean_db.py"] {
            std::fs::write(d.path().join(f), "").unwrap();
        }
        for yes in ["python3 run_checks.py", "./verify.sh --fast", "sh verify.sh", "cargo test", "pytest -q"] {
            assert!(named_check(yes, d.path()), "{yes}");
        }
        for no in
            ["./deploy.sh", "python3 clean_db.py", "git push", "python3 missing.py", "ls", "python3 -c 'print(1)'"]
        {
            assert!(!named_check(no, d.path()), "{no}");
        }
    }

    #[test]
    fn detects_checks_from_manifests() {
        let d = tempfile::tempdir().unwrap();
        assert!(detect_checks(d.path()).is_empty());
        std::fs::write(d.path().join("Cargo.toml"), "[package]").unwrap();
        std::fs::write(
            d.path().join("package.json"),
            r#"{"scripts":{"test":"vitest","lint":"eslint .","dev":"vite"}}"#,
        )
        .unwrap();
        std::fs::write(d.path().join("pnpm-lock.yaml"), "").unwrap();
        std::fs::write(d.path().join("test_stats.py"), "").unwrap();
        std::fs::write(d.path().join("Makefile"), "all:\n\ttrue\ntest:\n\ttrue\n").unwrap();
        std::fs::write(d.path().join("build.sh"), "").unwrap();
        assert_eq!(
            detect_checks(d.path()),
            ["cargo build", "cargo test", "pnpm lint", "pnpm test", "python3 -m unittest", "make test", "./build.sh"]
        );
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("package.json"), r#"{"scripts":{"test":"echo \"Error: no test specified\""}}"#)
            .unwrap();
        assert!(detect_checks(d.path()).is_empty(), "npm's placeholder test is not a check");
    }

    #[test]
    fn recognizes_check_commands() {
        let none: &[String] = &[];
        let changed = [PathBuf::from("/p/convert.py"), PathBuf::from("/p/src/lib.rs")];
        for yes in [
            "cargo test -q",
            "cd sub && cargo build --release",
            "python3 -m unittest -q test_stats",
            "PYTHONPATH=. pytest -x tests/",
            "uv run pytest",
            "timeout 60 npm test",
            "./build.sh",
            "bash scripts/test-all.sh",
            "python3 tests/test_api.py",
            "python3 convert.py sample.csv",
            "go vet ./... 2>&1 | head",
            "make",
        ] {
            assert!(is_check(yes, none, &changed), "{yes}");
        }
        for no in ["ls -la", "cat convert.py", "git status", "echo test", "sed -i s/a/b/ x.py", "python3 -c 'print(1)'"]
        {
            assert!(!is_check(no, none, &changed), "{no}");
        }
        assert!(is_check("just verify", &["just verify".into()], &changed), "configured commands count");
    }

    #[test]
    fn recognizes_inline_interpreter_checks() {
        // A project with no test runner, verified by running the changed code inline.
        let none: &[String] = &[];
        let changed = [PathBuf::from("/work/repo/algo.py"), PathBuf::from("/work/util.js")];
        for yes in [
            r#"python3 -c "from algo import solve; assert solve(1) == 1""#,
            r#"python3 -c "import algo; print(algo.area(2))""#,
            r#"cd /work/repo && python3 -c "import ast; ast.parse(open('algo.py').read())""#,
            "python3 - <<'EOF'\nimport algo\nprint(algo.solve(1))\nEOF",
            r#"node -e "require('./util').run()""#,
        ] {
            assert!(is_check(yes, none, &changed), "{yes}");
        }
        for no in [
            r#"python3 -c "print(1)""#,               // touches nothing changed
            r#"python3 -c "import os; os.getcwd()""#, // imports an unrelated module
            "cat algo.py",                            // not an interpreter running code
            r#"echo "import algo""#,                  // echo, not an interpreter
        ] {
            assert!(!is_check(no, none, &changed), "{no}");
        }
    }

    #[test]
    fn recognizes_a_program_run_on_a_changed_file() {
        let none: &[String] = &[];
        let changed = [
            PathBuf::from("/work/my_player.agent"),
            PathBuf::from("/work/x.sh"),
            PathBuf::from("/work/a.xml"),
            PathBuf::from("/work/config.json"),
            PathBuf::from("/work/input.txt"),
            PathBuf::from("/work/f.py"),
        ];
        for yes in [
            // A run (231fd6b): the entry was edited, then run against each opponent.
            "cd /work && for opp in stone vampire paper snake g2-clear; do\n  simulate -b -r 100 -f my_player.agent \
             players/$opp.agent | tail -1\ndone",
            "simulate -b -r 100 -f my_player.agent players/stone.agent 2>&1 | tail -4",
            "shellcheck x.sh",
            "xmllint --noout a.xml",
            "./app config.json",
            "./app --config=config.json",
            "./solve < input.txt",
            "python3 tool.py config.json",
            "python3 -m json.tool config.json",
            "if simulate -b my_player.agent w/stone.agent; then echo ok; fi",
            "bash -c \"cd /work && simulate -b my_player.agent w/stone.agent\"",
        ] {
            assert!(is_check(yes, none, &changed), "{yes}");
        }
        for no in [
            "cat my_player.agent",
            "head -5 my_player.agent && wc -l my_player.agent",
            "grep -n spl my_player.agent",
            "cp my_player.agent /tmp/best.red",
            "rm -f input.txt",
            "chmod +x x.sh",
            "sed -i 's/a/b/' f.py",
            "perl -pi -e 's/a/b/' f.py",
            "perl -i.bak -pe 's/a/b/' f.py",
            "git add f.py && git commit -m fix",
            "echo x > config.json",
            "./prog > input.txt 2>&1",
            "test -f my_player.agent",
            "black f.py",
            "vim f.py",
            "python3 -m pip install -r input.txt",
            "for f in my_player.agent x.sh; do echo $f; done",
            "bash -c \"cat my_player.agent\"",
        ] {
            assert!(!is_check(no, none, &changed), "{no}");
        }
    }

    #[test]
    fn recognizes_inline_code_that_names_a_changed_module() {
        let none: &[String] = &[];
        // A run (f4d913e): both changed modules imported by name.
        let changed = [PathBuf::from("/work/task_file/scripts/optimizer.py"), PathBuf::from("/work/scripts/sweep.py")];
        let real = "cd /work/task_file/scripts && python3 -c \"\nimport importlib, sys\n# import both changed modules \
                    to confirm they parse/import cleanly\nfor m in ['optimizer','sweep']:\n    \
                    importlib.import_module(m)\nprint('ok')\"";
        assert!(is_check(real, none, &changed));
        for yes in [
            r#"python3 -c "__import__('optimizer').main()""#,
            r#"python3 -c "import importlib.util as u; s = u.spec_from_file_location('sweep', 'x')""#,
            r#"node -e "require('./sweep')""#,
        ] {
            assert!(is_check(yes, none, &changed), "{yes}");
        }
        for no in
            [r#"python3 -c "import optimizers""#, r#"python3 -c "print('optimizers')""#, r#"python3 -c "x = 'swe'""#]
        {
            assert!(!is_check(no, none, &changed), "{no}");
        }
        let short = [PathBuf::from("/work/ab.py")];
        assert!(!is_check(r#"python3 -c "x = 'ab'""#, none, &short), "a 2-letter string is not the module");
        assert!(is_check(r#"python3 -c "import ab""#, none, &short), "an import is, whatever the length");
    }

    #[test]
    fn a_package_is_imported_by_its_name() {
        // A run (090446d): the package's __init__.py changed, and the check imported the
        // package; the file's own stem (`__init__`) is never what code names.
        let changed = vec![PathBuf::from("/work/vectorops/vectorops/__init__.py")];
        assert!(is_check(
            r#"python3 -c "from vectorops import dotproduct; assert dotproduct([1],[2]) == 2""#,
            &[],
            &changed
        ));
        assert!(!is_check(r#"python3 -c "import json; print(1)""#, &[], &changed));
    }

    #[test]
    fn files_read_back_are_named_and_plain_data_is_told_apart() {
        assert_eq!(
            files_viewed("cat /work/move.txt && od -c /work/move.txt | tail -3"),
            ["/work/move.txt", "/work/move.txt"]
        );
        assert_eq!(files_viewed("diff out.csv expected.csv"), ["out.csv", "expected.csv"]);
        assert!(files_viewed("python3 solve.py && ls").is_empty());
        for data in ["move.txt", "out.CSV", "notes.md", "run.log", "result.out", "table.tsv"] {
            assert!(is_data_file(Path::new(data)), "{data}");
        }
        for code in ["solve.py", "config.toml", "package.json", "Makefile", "app.rs", "deploy.yaml"] {
            assert!(!is_data_file(Path::new(code)), "{code}");
        }
        assert!(names_file(Path::new("/work/move.txt"), "move.txt"));
        assert!(names_file(Path::new("/work/move.txt"), "./move.txt"));
        assert!(!names_file(Path::new("/work/move.txt"), "remove.txt"));
    }

    #[test]
    fn eval_and_bench_scripts_are_checks() {
        let none: &[String] = &[];
        // A run (f4d913e): the task's own evaluation script, run twice.
        let changed = [PathBuf::from("/work/eigen.py"), PathBuf::from("/work/_fast_eigen.c")];
        for yes in [
            r#"python eval.py && echo "=== second run ===" && python eval.py"#,
            "./bench.sh",
            "python3 benchmark.py --quick",
            "bash scripts/evaluate.sh",
        ] {
            assert!(is_check(yes, none, &changed), "{yes}");
        }
        assert!(!is_check(r#"eval "$(ssh-agent -s)""#, none, &changed), "the shell builtin is not a script");
    }

    #[test]
    fn here_document_bodies_are_data_not_commands() {
        let none: &[String] = &[];
        let changed = [PathBuf::from("/work/solver.py"), PathBuf::from("/work/my_player.agent")];
        for no in [
            "cat > run.sh <<'EOF'\npython3 solver.py input.txt\nEOF",
            "cat > my_player.agent <<'EOF'\nmove 0 5\nEOF",
            // A shell fed a here-document runs its lines: here, only a viewer.
            "bash <<'EOF'\ncat my_player.agent\nEOF",
        ] {
            assert!(!is_check(no, none, &changed), "{no}");
        }
        for yes in [
            "bash <<'EOF'\ncd /work && python3 -m pytest -q\nEOF",
            "sh -s <<EOF\npmars -b my_player.agent w/stone.agent\nEOF",
            "python3 - <<'EOF'\nimport solver\nprint(solver.solve(1))\nEOF",
            "cat > run.sh <<'EOF'\necho hi\nEOF\npython3 solver.py input.txt",
            // An unterminated `<<` (a shift inside code) hides nothing.
            "python3 -c \"x = 1 << n\nimport solver\"",
        ] {
            assert!(is_check(yes, none, &changed), "{yes}");
        }
    }

    #[test]
    fn the_result_states_where_the_changes_stand() {
        assert_eq!(Verified::NoChanges.record(), serde_json::json!({"status": "no_changes"}));
        assert_eq!(
            Verified::Passed { command: "cargo test".into() }.record(),
            serde_json::json!({"status": "passed", "command": "cargo test"})
        );
        assert_eq!(
            Verified::Unchecked { files: vec!["a.rs".into()], shell: false }.record(),
            serde_json::json!({"status": "unchecked", "files": ["a.rs"]})
        );
        assert_eq!(
            Verified::Unchecked { files: vec![], shell: true }.record(),
            serde_json::json!({"status": "unchecked", "files": [], "shell": true})
        );
        assert_eq!(Verified::Failed { command: "make".into() }.record()["status"], "failed");
        assert_eq!(Verified::ReadBack.status(), "read_back");
    }

    #[test]
    fn reminder_text_names_files_and_checks() {
        let r = Reminder::Unchecked { files: vec!["a.py".into()], shell: false };
        let t = r.text(&["python3 -m unittest".into()]);
        assert!(t.contains("`a.py`") && t.contains("`python3 -m unittest`") && t.contains("what you did not"), "{t}");
        let t = Reminder::Unchecked { files: vec![], shell: true }.text(&[]);
        assert!(t.contains("shell commands") && t.contains("No check command was detected"), "{t}");
        let many: Vec<String> = (0..10).map(|i| format!("f{i}")).collect();
        assert!(Reminder::Unchecked { files: many, shell: false }.text(&[]).contains("and 2 more"));
    }
}
