//! Stuck-loop detection (GOALS pillar 2).
//!
//! Models in trouble repeat themselves: the same failing call with the same
//! error (also after changes that didn't affect it), the same read or command
//! with the same result and nothing changed in between, edits that undo each
//! other, or a long run of failures. The guard watches one user turn's tool
//! calls and, once per pattern, adds a reminder after the tool results telling
//! the model to step back and change approach.
//!
//! Only evidence counts, never time: a command that fails differently after
//! each fix is making progress, one that fails the same way after several
//! changes is not, and a check that gives the same clean output after an edit
//! is a check, not a loop.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use serde_json::{json, Value};

/// Identical failing calls, with the same error, before a reminder.
const REPEATED_FAILURES: u32 = 3;
/// Identical calls with identical results, nothing changed in between, before a reminder.
const NO_PROGRESS: u32 = 3;
/// Consecutive failing calls (any tool) before a reminder.
const ERROR_STREAK: u32 = 5;
/// A pattern that goes on is told again at these counts, then no more: a model that didn't act
/// on the first reminder gets two more chances to, and a long run of the same call doesn't
/// fill the conversation with reminders. Counts of identical calls, never time.
const REMIND_AT: [u32; 3] = [REPEATED_FAILURES, 5, 8];
/// Characters of the last error a refusal quotes: enough to recognize it, not a second copy.
const REFUSAL_QUOTE: usize = 600;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Stuck {
    /// `after_changes`: files changed between some of the failures, and the error stayed.
    RepeatedFailure {
        tool: String,
        count: u32,
        after_changes: bool,
    },
    NoProgress {
        tool: String,
        count: u32,
    },
    Oscillating {
        file: String,
    },
    ErrorStreak {
        count: u32,
    },
}

impl Stuck {
    pub fn record(&self) -> Value {
        match self {
            Stuck::RepeatedFailure { tool, count, after_changes } => {
                json!({"kind": "repeated_failure", "tool": tool, "count": count, "after_changes": after_changes})
            }
            Stuck::NoProgress { tool, count } => json!({"kind": "no_progress", "tool": tool, "count": count}),
            Stuck::Oscillating { file } => json!({"kind": "oscillating_edits", "file": file}),
            Stuck::ErrorStreak { count } => json!({"kind": "error_streak", "count": count}),
        }
    }

    pub fn text(&self) -> String {
        let body = match self {
            Stuck::RepeatedFailure { tool, count, after_changes: false } => format!(
                "This exact {tool} call has now failed {count} times with the same error. Repeating it will not \
                 change the result. Read the error, check your assumptions (the file's current contents, the path, \
                 the command's syntax), and try a different approach."
            ),
            Stuck::RepeatedFailure { tool, count, after_changes: true } => format!(
                "This exact {tool} call has now failed {count} times with the same error, and the changes you made \
                 between those runs did not affect it. Before another variation of the same fix, check your \
                 assumption about the cause (is the code you are changing the code that runs? does the error come \
                 from where you think?), then try a substantially different approach."
            ),
            Stuck::NoProgress { tool, count } => format!(
                "This exact {tool} call has returned the same result {count} times. Nothing has changed between \
                 calls, so use the result you already have, or change something before running it again."
            ),
            Stuck::Oscillating { file } => format!(
                "Your edits to {file} are undoing each other. Stop editing it: re-read the file and the failing \
                 output, decide on one approach, then make that change."
            ),
            Stuck::ErrorStreak { count } => format!(
                "The last {count} tool calls all failed. Step back: re-read the task and the errors, then change \
                 approach. If something outside your control is blocking you, say what it is."
            ),
        };
        format!("<system-reminder>\n{body}\n</system-reminder>")
    }
}

/// A call answered without being run: see [`LoopGuard::would_repeat`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Refused {
    pub tool: String,
    pub count: u32,
    pub error: String,
}

impl Refused {
    pub fn record(&self) -> Value {
        json!({"kind": "refused", "tool": self.tool, "count": self.count})
    }

    /// The call's result.
    pub fn text(&self) -> String {
        format!(
            "Not run: this exact {} call has failed {} times with the same error, and nothing has changed \
             since. Running it again would give the same error:\n{}\nSend a different call: re-read the \
             file or the error and correct the arguments, or take another approach.",
            self.tool, self.count, self.error
        )
    }
}

fn hash<T: Hash>(t: &T) -> u64 {
    let mut h = DefaultHasher::new();
    t.hash(&mut h);
    h.finish()
}

/// What sets a later reminder of one pattern apart from its first: nothing at the first count
/// and between the counts of [`REMIND_AT`] (so it fires once), the count itself at a later one.
fn later(count: u32) -> String {
    match REMIND_AT.iter().rev().find(|at| count >= **at) {
        Some(at) if *at > REPEATED_FAILURES => format!(":{at}"),
        _ => String::new(),
    }
}

/// How often one call gave one output.
#[derive(Debug, Default)]
struct Seen {
    count: u32,
    /// The guard's change count right after the last of them.
    changes_after: u64,
    /// A change came between two of them.
    changed_between: bool,
    /// The start of the error, for a refusal to quote.
    error: String,
}

/// One user turn's view of the tool calls.
#[derive(Debug, Default)]
pub(crate) struct LoopGuard {
    /// Failures per call and output.
    failures: HashMap<(u64, u64), Seen>,
    /// Successful results per call and output.
    results: HashMap<(u64, u64), Seen>,
    /// (file, old, new) of every successful edit.
    edits: Vec<(String, String, String)>,
    streak: u32,
    fired: HashSet<String>,
    /// Calls so far that may have changed files (edits and writes, shell commands that aren't
    /// read-only, sub-agents).
    changes: u64,
}

impl LoopGuard {
    /// Note one finished call; returns a pattern the first time it shows. `may_change`: the call
    /// may have changed files, so a command run after it can give a new result.
    pub fn observe(
        &mut self,
        tool: &str,
        input: &Value,
        output: &str,
        is_error: bool,
        may_change: bool,
    ) -> Option<Stuck> {
        let key = hash(&(tool, input.to_string()));
        let out = hash(&comparable(output));
        let changes_after = self.changes + u64::from(may_change);
        let mut found = None;
        if is_error {
            self.streak += 1;
            let seen = self.failures.entry((key, out)).or_default();
            seen.changed_between |= seen.count > 0 && self.changes > seen.changes_after;
            seen.count += 1;
            seen.changes_after = changes_after;
            seen.error = output.chars().take(REFUSAL_QUOTE).collect();
            if seen.count >= REPEATED_FAILURES {
                let stuck = Stuck::RepeatedFailure {
                    tool: tool.into(),
                    count: seen.count,
                    after_changes: seen.changed_between,
                };
                found = Some((format!("fail:{key}:{out}{}", later(seen.count)), stuck));
            } else if self.streak >= ERROR_STREAK {
                found = Some(("streak".into(), Stuck::ErrorStreak { count: self.streak }));
            }
        } else {
            self.streak = 0;
            if tool != "TodoWrite" {
                let seen = self.results.entry((key, out)).or_default();
                // A check or build run again after a change is new evidence even when its output is
                // the same (a clean lint after an edit). A read-only call saw the same thing again.
                if may_change && seen.count > 0 && self.changes > seen.changes_after {
                    seen.count = 0;
                }
                seen.count += 1;
                seen.changes_after = changes_after;
                if seen.count >= NO_PROGRESS {
                    let id = format!("same:{key}{}", later(seen.count));
                    found = Some((id, Stuck::NoProgress { tool: tool.into(), count: seen.count }));
                }
            }
            for (file, old, new) in edits_of(tool, input) {
                if self.edits.iter().any(|(f, o, n)| *f == file && *o == new && *n == old) {
                    found = Some((format!("osc:{file}"), Stuck::Oscillating { file: file.clone() }));
                }
                self.edits.push((file, old, new));
            }
        }
        self.changes = changes_after;
        let (id, stuck) = found?;
        self.fired.insert(id).then_some(stuck)
    }

    /// Before a call runs: would it only repeat a failure the model was already warned about?
    /// Yes when this exact call failed [`REPEATED_FAILURES`] times with the same error, the
    /// reminder for it went out, and no call that may change files ran since. Then running it
    /// again costs a step and returns what the model already has, so the engine answers it
    /// without running it ([`Refused`]).
    ///
    /// Only for calls whose result depends on nothing but the call and the files
    /// (`depends_on_files_only`): a shell command, a fetch or an MCP call may fail the same way
    /// three times and work the fourth (a server still starting, a network blip), so those are
    /// never refused, only reminded.
    pub fn would_repeat(&mut self, tool: &str, input: &Value, depends_on_files_only: bool) -> Option<Refused> {
        if !depends_on_files_only {
            return None;
        }
        let key = hash(&(tool, input.to_string()));
        let changes = self.changes;
        let fired = &self.fired;
        let seen = self.failures.iter().find_map(|((k, out), seen)| {
            (*k == key
                && seen.count >= REPEATED_FAILURES
                && seen.changes_after == changes
                && fired.contains(&format!("fail:{k}:{out}")))
            .then_some(seen)
        })?;
        let refused = Refused { tool: tool.into(), count: seen.count, error: seen.error.clone() };
        // Refusals count toward the run of failures, so a model that keeps sending the call
        // still gets the "step back" reminder.
        self.streak += 1;
        Some(refused)
    }

    /// Forget the results counted so far, after a microcompaction cleared old tool results from
    /// the model's view: running one of those calls again is not a loop. Failures, edits and the
    /// error streak are kept.
    pub fn forget_results(&mut self) {
        self.results.clear();
        self.fired.retain(|id| !id.starts_with("same:"));
    }
}

/// The output as compared between runs. The elapsed times that test runners and build tools
/// print in their own summary lines (pytest, unittest, cargo, go, jest, vitest, mocha) and
/// Python's object addresses (`<function f at 0x7f…>`) differ on every run, so they are masked.
/// Everything else is kept, a program's own timing included ("FAIL: solved in 5.2s"), so a change
/// that improves a measured result never reads as the same failure.
fn comparable(output: &str) -> String {
    let jest = output.lines().any(|l| l.trim_start().starts_with("Test Suites:"));
    let vitest = output.lines().any(|l| l.trim_start().starts_with("Test Files "));
    let mut out = String::with_capacity(output.len());
    for line in output.lines() {
        let masked = mask_runner_timing(line, jest, vitest);
        out.push_str(&mask_addresses(masked.as_deref().unwrap_or(line)));
        out.push('\n');
    }
    out
}

/// A duration as runners print it: `0.12s`, `5ms`, `(0.00s)`, `(0:01:01)`.
fn is_duration(word: &str) -> bool {
    let w = word.trim_start_matches('(').trim_end_matches([')', ',']);
    let clock = w.contains(':') && w.chars().all(|c| c.is_ascii_digit() || c == ':');
    let number = ["ms", "µs", "us", "ns", "s"].iter().find_map(|u| w.strip_suffix(u)).unwrap_or("");
    clock || (!number.is_empty() && number.chars().all(|c| c.is_ascii_digit() || c == '.'))
}

/// The line with its trailing durations masked when it has the shape of a test runner's or
/// build tool's summary line; `None` for any other line.
fn mask_runner_timing(line: &str, jest: bool, vitest: bool) -> Option<String> {
    let t = line.trim();
    let ws: Vec<&str> = t.split_whitespace().collect();
    let mask_tail = |s: &str| -> String {
        let mut words: Vec<&str> = s.split_whitespace().collect();
        while words.last().is_some_and(|w| is_duration(w)) {
            words.pop();
        }
        format!("{} <time>", words.join(" "))
    };
    // pytest: `==== 1 failed, 2 passed in 0.12s ====`, also `in 61.20s (0:01:01)`.
    if t.starts_with('=') && t.ends_with('=') && t.contains(" in ") {
        let inner = t.trim_matches('=').trim();
        let words =
            ["passed", "failed", "error", "skipped", "deselected", "xfailed", "xpassed", "warning", "no tests ran"];
        if words.iter().any(|w| inner.contains(w)) {
            return Some(format!("== {} ==", mask_tail(inner)));
        }
    }
    // unittest: `Ran 3 tests in 0.001s`.
    if ws.len() == 5 && ws[0] == "Ran" && ws[1].chars().all(|c| c.is_ascii_digit()) && ws[2].starts_with("test") {
        return Some(mask_tail(t));
    }
    // cargo: `test result: FAILED. 1 passed; 1 failed; ...; finished in 0.00s`, and
    // `Finished `test` profile [...] target(s) in 0.51s`.
    if (t.starts_with("test result:") && t.contains("finished in "))
        || (t.starts_with("Finished ") && t.contains("target(s) in "))
    {
        return Some(mask_tail(t));
    }
    // go: `ok  \tpkg\t0.012s`, `FAIL\tpkg\t0.012s`, `--- FAIL: TestX (0.00s)`.
    let fields: Vec<&str> = line.split('\t').map(str::trim).collect();
    if fields.len() == 3 && matches!(fields[0], "ok" | "FAIL") && is_duration(fields[2]) {
        return Some(format!("{}\t{}\t<time>", fields[0], fields[1]));
    }
    if ["--- FAIL: ", "--- PASS: ", "--- SKIP: "].iter().any(|p| t.starts_with(p))
        && ws.last().is_some_and(|w| is_duration(w))
    {
        return Some(mask_tail(t));
    }
    // mocha: `2 passing (5ms)`.
    if ws.len() == 3 && ws[0].chars().all(|c| c.is_ascii_digit()) && matches!(ws[1], "passing" | "failing" | "pending")
    {
        return Some(mask_tail(t));
    }
    // jest `Time:        0.512 s, estimated 1 s`, vitest `Duration  1.23s (transform 20ms, ...)`, only in
    // their summaries.
    if (jest && t.starts_with("Time:")) || (vitest && t.starts_with("Duration")) {
        return Some(format!("{} <time>", ws[0]));
    }
    None
}

/// Python's object addresses (`<function f at 0x7f3a2c1b2e50>`) masked.
fn mask_addresses(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(i) = rest.find(" at 0x") {
        let (head, tail) = rest.split_at(i + " at 0x".len());
        let hex = tail.chars().take_while(|c| c.is_ascii_hexdigit()).count();
        out.push_str(head);
        if hex >= 6 {
            out.push('…');
            rest = &tail[hex..];
        } else {
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

/// The (file, old, new) replacements an edit call made.
fn edits_of(tool: &str, input: &Value) -> Vec<(String, String, String)> {
    let file = input.get("file_path").and_then(Value::as_str).unwrap_or_default().to_string();
    let pair = |e: &Value| {
        (
            file.clone(),
            e.get("old_string").and_then(Value::as_str).unwrap_or_default().to_string(),
            e.get("new_string").and_then(Value::as_str).unwrap_or_default().to_string(),
        )
    };
    match tool {
        "Edit" => vec![pair(input)],
        "MultiEdit" => {
            input.get("edits").and_then(Value::as_array).map(|a| a.iter().map(pair).collect()).unwrap_or_default()
        }
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_failures_fire_once() {
        let mut g = LoopGuard::default();
        let call = json!({"command": "make"});
        assert_eq!(g.observe("Bash", &call, "err", true, true), None);
        assert_eq!(g.observe("Bash", &call, "err", true, true), None);
        let s = g.observe("Bash", &call, "err", true, true).unwrap();
        assert_eq!(s, Stuck::RepeatedFailure { tool: "Bash".into(), count: 3, after_changes: false });
        assert!(s.text().contains("failed 3 times with the same error") && s.text().contains("Repeating it"));
        assert_eq!(g.observe("Bash", &call, "err", true, true), None, "once per call and error");
    }

    /// After the reminder, the same failing call is not run again while nothing has changed; a
    /// change, a different call, or a tool that depends on more than the files lifts it.
    #[test]
    fn a_warned_repeat_is_refused_until_something_changes() {
        let mut g = LoopGuard::default();
        let call = json!({"file_path": "a.py", "old_string": "x", "new_string": "y"});
        let err = "String to replace not found in file.";
        assert_eq!(g.would_repeat("Edit", &call, true), None, "never seen");
        for _ in 0..2 {
            assert!(g.observe("Edit", &call, err, true, false).is_none());
            assert_eq!(g.would_repeat("Edit", &call, true), None, "not warned yet");
        }
        assert!(g.observe("Edit", &call, err, true, false).is_some(), "the reminder");
        let r = g.would_repeat("Edit", &call, true).unwrap();
        assert_eq!((r.count, r.tool.as_str()), (3, "Edit"));
        assert!(r.text().contains("Not run") && r.text().contains(err), "{}", r.text());
        assert_eq!(r.record(), json!({"kind": "refused", "tool": "Edit", "count": 3}));
        // Still refused the next time; another call is not.
        assert!(g.would_repeat("Edit", &call, true).is_some());
        assert_eq!(
            g.would_repeat("Edit", &json!({"file_path": "a.py", "old_string": "z", "new_string": "y"}), true),
            None
        );
        // A shell command may work the fourth time (a server still starting): never refused.
        let curl = json!({"command": "curl -sf localhost:8000"});
        for _ in 0..3 {
            g.observe("Bash", &curl, "exit code 7", true, true);
        }
        assert_eq!(g.would_repeat("Bash", &curl, false), None);
        // Something changed since the last failure: the call may now succeed, so it runs.
        g.observe("Write", &json!({"file_path": "a.py", "content": "x = 1"}), "ok", false, true);
        assert_eq!(g.would_repeat("Edit", &call, true), None);
    }

    /// A model that keeps sending a refused call still gets the "step back" reminder.
    #[test]
    fn refusals_count_toward_the_failure_streak() {
        let mut g = LoopGuard::default();
        let call = json!({"file_path": "/nope"});
        for _ in 0..3 {
            g.observe("Read", &call, "File does not exist.", true, false);
        }
        assert!(g.would_repeat("Read", &call, true).is_some());
        assert!(g.would_repeat("Read", &call, true).is_some());
        let other = json!({"file_path": "/nope2"});
        assert_eq!(
            g.observe("Read", &other, "File does not exist.", true, false),
            Some(Stuck::ErrorStreak { count: 6 })
        );
    }

    /// A call that is never refused (a shell command) and goes on failing the same way is told
    /// again at 5 and at 8, with the count, then no more.
    #[test]
    fn a_loop_that_goes_on_is_told_again_at_later_counts() {
        let mut g = LoopGuard::default();
        let call = json!({"command": "make"});
        let told: Vec<u32> = (1..=12)
            .filter_map(|_| match g.observe("Bash", &call, "make: *** No rule", true, false) {
                Some(Stuck::RepeatedFailure { count, .. }) => Some(count),
                Some(Stuck::ErrorStreak { .. }) | None => None,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(told, vec![3, 5, 8]);
        // The same result again and again, likewise.
        let mut g = LoopGuard::default();
        let read = json!({"file_path": "a.txt"});
        let told: Vec<u32> = (1..=12)
            .filter_map(|_| match g.observe("Read", &read, "same", false, false) {
                Some(Stuck::NoProgress { count, .. }) => Some(count),
                None => None,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(told, vec![3, 5, 8]);
    }

    #[test]
    fn a_fix_test_cycle_with_new_errors_is_not_a_loop() {
        // Fixing failures one at a time: the same command fails three times, differently each time.
        let mut g = LoopGuard::default();
        let test = json!({"command": "cargo test -q"});
        let edit = json!({"file_path": "/p/src/lib.rs", "old_string": "a", "new_string": "b"});
        for error in [
            "error[E0425]: cannot find value `total` in this scope\n --> src/lib.rs:12:5",
            "thread 'tests::sums' panicked at src/lib.rs:30:9:\nassertion `left == right` failed\n  left: 5\n right: 6",
            "thread 'tests::empty' panicked at src/lib.rs:41:9:\nassertion failed: avg(&[]).is_none()",
        ] {
            assert_eq!(g.observe("Bash", &test, &format!("Exit code 101\n{error}"), true, true), None, "{error}");
            assert_eq!(g.observe("Edit", &edit, "ok", false, true), None);
        }
    }

    #[test]
    fn the_same_failure_after_changes_is_reported() {
        // Edits to a file that isn't the one the test imports: the traceback never changes, only
        // pytest's own timing does.
        let mut g = LoopGuard::default();
        let test = json!({"command": "python3 -m pytest -q tests/test_util.py"});
        let failure = |secs: &str| {
            format!(
                "Exit code 1\nF\n=================================== FAILURES ===================================\n\
                 E   AssertionError: assert slugify('A B') == 'a-b'\n\
                 E    +  where 'A B' = <function slugify at 0x7f3a2c1b2e50>('A B')\n\
                 tests/test_util.py:4: AssertionError\n=========================== 1 failed in {secs} ===========================",
            )
        };
        for (i, secs) in ["0.03s", "0.05s"].iter().enumerate() {
            assert_eq!(g.observe("Bash", &test, &failure(secs), true, true), None, "run {i}");
            let edit = json!({"file_path": "/p/src/util.py", "old_string": format!("v{i}"), "new_string": format!("v{}", i + 1)});
            g.observe("Edit", &edit, "ok", false, true);
        }
        let s = g.observe("Bash", &test, &failure("0.04s"), true, true).unwrap();
        assert_eq!(s, Stuck::RepeatedFailure { tool: "Bash".into(), count: 3, after_changes: true });
        assert!(s.text().contains("did not affect it") && s.text().contains("the code that runs"), "{}", s.text());
        assert_eq!(s.record()["after_changes"], json!(true));
    }

    #[test]
    fn a_clean_check_after_each_edit_is_not_a_loop() {
        let mut g = LoopGuard::default();
        let lint = json!({"command": "ruff check ."});
        for i in 0..4 {
            let edit =
                json!({"file_path": "/p/app.py", "old_string": format!("x{i}"), "new_string": format!("x{}", i + 1)});
            assert_eq!(g.observe("Edit", &edit, "ok", false, true), None);
            assert_eq!(g.observe("Bash", &lint, "All checks passed!", false, true), None, "a check after an edit");
        }
        // The same check three times with nothing changed in between is a loop.
        let mut g = LoopGuard::default();
        g.observe("Bash", &lint, "All checks passed!", false, true);
        g.observe("Bash", &lint, "All checks passed!", false, true);
        assert!(matches!(g.observe("Bash", &lint, "All checks passed!", false, true), Some(Stuck::NoProgress { .. })));
    }

    #[test]
    fn polling_the_same_background_output_still_fires() {
        // Seen in two runs: a background job polled with BashOutput while other commands ran;
        // its output hadn't changed.
        let mut g = LoopGuard::default();
        let poll = json!({"bash_id": "bash_2"});
        for i in 0..2 {
            assert_eq!(g.observe("BashOutput", &poll, "<status>running</status>", false, false), None);
            g.observe("Bash", &json!({"command": format!("sleep 30; ls out{i}")}), "", false, true);
        }
        assert!(matches!(
            g.observe("BashOutput", &poll, "<status>running</status>", false, false),
            Some(Stuck::NoProgress { count: 3, .. })
        ));
    }

    #[test]
    fn runner_timing_is_ignored_but_program_timing_is_not() {
        for (a, b) in [
            ("==== 1 failed, 2 passed in 0.12s ====", "==== 1 failed, 2 passed in 3.40s ===="),
            ("== 1 failed in 61.20s (0:01:01) ==", "== 1 failed in 59.00s (0:00:59) =="),
            ("Ran 3 tests in 0.001s\n\nFAILED (failures=1)", "Ran 3 tests in 0.004s\n\nFAILED (failures=1)"),
            (
                "test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s",
                "test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.31s",
            ),
            (
                "    Finished `test` profile [unoptimized] target(s) in 0.51s",
                "    Finished `test` profile [unoptimized] target(s) in 12.0s",
            ),
            (
                "--- FAIL: TestSum (0.00s)\nFAIL\texample.com/m\t0.002s",
                "--- FAIL: TestSum (0.01s)\nFAIL\texample.com/m\t0.013s",
            ),
            ("  2 passing (5ms)\n  1 failing", "  2 passing (9ms)\n  1 failing"),
            ("Test Suites: 1 failed\nTime:        0.512 s", "Test Suites: 1 failed\nTime:        1.2 s, estimated 1 s"),
            ("<function f at 0x7f3a2c1b2e50>", "<function f at 0x7f00aa11bb22>"),
        ] {
            assert_eq!(comparable(a), comparable(b), "{a:?} vs {b:?}");
        }
        for (a, b) in [
            ("FAIL: solved in 5.2s (limit 3s)", "FAIL: solved in 4.1s (limit 3s)"),
            ("=== solved in 5.2s ===", "=== solved in 4.1s ==="),
            ("Time: 5.2s", "Time: 4.1s"),
            ("Ran simulation in 5.2s", "Ran simulation in 4.1s"),
            ("checksum 0x1a2b3c4d != expected", "checksum 0x5e6f7a8b != expected"),
            ("==== 1 failed, 2 passed in 0.12s ====", "==== 2 failed, 1 passed in 0.12s ===="),
        ] {
            assert_ne!(comparable(a), comparable(b), "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn same_result_three_times_is_no_progress() {
        let mut g = LoopGuard::default();
        let read = json!({"file_path": "/a"});
        g.observe("Read", &read, "x", false, false);
        g.observe("Read", &read, "y", false, false);
        assert_eq!(g.observe("Read", &read, "y", false, false), None, "a changed result is progress");
        assert!(matches!(g.observe("Read", &read, "y", false, false), Some(Stuck::NoProgress { count: 3, .. })));
        let todo = json!({"todos": []});
        for _ in 0..4 {
            assert_eq!(g.observe("TodoWrite", &todo, "ok", false, false), None);
        }
    }

    #[test]
    fn results_are_forgotten_after_a_microcompaction() {
        let mut g = LoopGuard::default();
        let read = json!({"file_path": "/work/opponents/a.cfg"});
        assert_eq!(g.observe("Read", &read, "x", false, false), None);
        assert_eq!(g.observe("Read", &read, "x", false, false), None);
        g.forget_results();
        assert_eq!(g.observe("Read", &read, "x", false, false), None, "the model can't see the earlier reads");
        assert_eq!(g.observe("Read", &read, "x", false, false), None);
        assert!(matches!(g.observe("Read", &read, "x", false, false), Some(Stuck::NoProgress { count: 3, .. })));
        let make = json!({"command": "make"});
        g.observe("Bash", &make, "err", true, true);
        g.observe("Bash", &make, "err", true, true);
        g.forget_results();
        assert!(
            matches!(g.observe("Bash", &make, "err", true, true), Some(Stuck::RepeatedFailure { .. })),
            "failures stay"
        );
    }

    #[test]
    fn edits_that_undo_each_other() {
        let mut g = LoopGuard::default();
        let fwd = json!({"file_path": "/f.py", "old_string": "a + b", "new_string": "a - b"});
        let back = json!({"file_path": "/f.py", "old_string": "a - b", "new_string": "a + b"});
        assert_eq!(g.observe("Edit", &fwd, "ok", false, true), None);
        assert_eq!(g.observe("Edit", &back, "ok", false, true), Some(Stuck::Oscillating { file: "/f.py".into() }));
        let multi = json!({"file_path": "/g.py", "edits": [{"old_string": "x", "new_string": "y"}]});
        let undo = json!({"file_path": "/g.py", "edits": [{"old_string": "y", "new_string": "x"}]});
        g.observe("MultiEdit", &multi, "ok", false, true);
        assert!(matches!(g.observe("MultiEdit", &undo, "ok", false, true), Some(Stuck::Oscillating { .. })));
    }

    #[test]
    fn a_streak_of_different_failures() {
        let mut g = LoopGuard::default();
        for i in 0..4 {
            assert_eq!(g.observe("Bash", &json!({"command": format!("c{i}")}), "e", true, true), None);
        }
        assert_eq!(
            g.observe("Bash", &json!({"command": "c4"}), "e", true, true),
            Some(Stuck::ErrorStreak { count: 5 })
        );
        g.observe("Bash", &json!({"command": "ok"}), "fine", false, true);
        for i in 0..5 {
            assert_eq!(g.observe("Bash", &json!({"command": format!("d{i}")}), "e", true, true), None, "once per turn");
        }
    }
}
