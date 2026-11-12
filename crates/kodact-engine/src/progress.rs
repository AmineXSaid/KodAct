//! Pacing by progress (GOALS pillar 5): nudges that follow what the model has done, not
//! what the task is.
//!
//! The loop guard (`stuck.rs`) watches for repetition: the same call, the same result. This
//! watches for something else: a long stretch of varied calls in which nothing was made.
//! Progress is a file written by an edit tool, or a check that passed; when the person's
//! request names no file, any call that changed something counts too (a request to start a
//! service, to commit, to install). A stretch without progress that runs past its share of the
//! run's budget gets one suggestion to make the next concrete step, which the model is free to
//! ignore when it is still rightly looking.
//!
//! - **Pacing** ([`Nudge::Progress`]): a fifth of the budget, and at least
//!   [`MIN_CALLS`] model calls, since the last progress. Without a budget (a person at the
//!   terminal), [`NO_BUDGET_CALLS`] calls.
//! - **Early** ([`Nudge::ProgressEarly`]): before the first progress of a turn, at a tenth of
//!   the budget (and the same [`MIN_CALLS`]), so a run with a limit gets a first version down
//!   while there is time to improve it. Only with a budget.
//!
//! A write counts the first time it reaches a file. Writing the same files again with no check
//! passing in between does not start the stretch over: fifteen edits to one file and no test
//! run is a stretch too, and its nudge says to run a check, not to make a change.
//!
//! Time spent waiting on a tool (a build, a download, a test suite, a sub-agent) is left out of
//! a stretch's time: the model could not have shortened it, and a nudge for it would be advice
//! with no fact behind it.
//!
//! Every threshold is a share of the run's own budget or a count of the model's own calls:
//! nothing depends on the model, the task or where it runs. All of it is off unless turned on
//! (`nudges` in settings, or `KODACT_NUDGES`), and `reminders: false` turns it off with the
//! rest of KodAct's reminders.
//!
//! **Back-off** ([`BackOff`]): a pacing nudge the model did not need is not given again in
//! that run. The signal is the delay: progress within [`ACTED_WITHIN`] model calls of the nudge
//! means the model acted on it, and it is kept; progress only later means the model went its
//! own way, and twice that silences it. A strong model that would have gone on anyway pays for
//! no more of them; a model that needed one keeps getting it. Only the pacing nudges back off:
//! a note that carries a fact (a file missing, a check never run, a limit passed) is never
//! silenced, and a nudge is never silenced for having worked (docs/goal.md, rule 8).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// Model calls since the last progress before either nudge can come: reading a few files
/// before the first change is normal work, never a reason to nudge.
pub(crate) const MIN_CALLS: u32 = 5;
/// The share of the run's budget a stretch without progress may take before the pacing nudge.
const PACING_SHARE: f64 = 0.20;
/// The share of the budget that may pass before the first progress, for the early nudge.
const EARLY_SHARE: f64 = 0.10;
/// Without a budget: model calls without progress before the pacing nudge. A person answering
/// a question by reading a dozen files is not slow; twice that with nothing made may be.
pub(crate) const NO_BUDGET_CALLS: u32 = 15;
/// Times a pacing nudge may be followed by progress only late (not needed) before back-off
/// stops it for the run.
const BACK_OFF_AFTER: u32 = 2;
/// Model calls after a pacing nudge, the one that made progress included, within which the
/// progress is taken as the nudge acted on.
pub(crate) const ACTED_WITHIN: u32 = 2;

/// Which nudges are on (`nudges.<name>` in settings, or `KODACT_NUDGES`). All are off by
/// default: each is turned on once an eval task has measured it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NudgeConfig {
    /// Pacing by progress: a long stretch without a change or a passing check.
    pub progress: bool,
    /// The same signal before the first progress, earlier, when the run has a budget.
    pub progress_early: bool,
    /// A shell command that edited a file and left it unparseable: the parse error, and a
    /// suggestion to use Edit for changes that need care.
    pub shell_edit: bool,
    /// Before the turn ends: files the request names that don't exist, commands it names that
    /// never ran, and a size limit it states for a file that the file exceeds. The size limit
    /// is also told at once, with the result of the step that took the file over it.
    pub constraints: bool,
    /// Commands the request names that are checks join the verification loop's list, so an
    /// unrun one is told to the model (never run by KodAct).
    pub named_checks: bool,
    /// After a check passes, the files are saved with the Checkpoint tool; when the last check
    /// then fails, the model and the result are told where the passing version is.
    pub passing_snapshots: bool,
    /// A pacing nudge the model didn't need (progress came, but only long after it), twice, is
    /// not repeated in the run; one it acted on is kept. Notes that carry a fact never back off.
    pub back_off: bool,
    /// The time-limit warning states the steps left at the run's own pace, comes earlier when
    /// few are left, and names the files the request asks for that don't exist yet.
    pub pace: bool,
    /// A command's `key=value` output held against the limits the request states: each line
    /// met or not, by how much, and limits across all files that per-file counts don't show.
    pub limit_verdicts: bool,
    /// A measurement met every limit stated for its file, and the file the request asks for
    /// wasn't written since: the model is told the best result is not saved yet.
    pub stale_deliverables: bool,
    /// The reasoning level each request asks for follows what the previous steps' thinking
    /// produced: down after thinking that led nowhere, back up after progress, never above the
    /// person's level and never below `low` (`effort_loop`).
    pub adaptive_effort: bool,
    /// A reply that is still all thinking past a limit from the run's own pace is stopped, its
    /// notes handed back, and the model asked for one concrete step.
    pub thinking_time_box: bool,
}

impl NudgeConfig {
    /// The names used in settings and `KODACT_NUDGES`.
    pub const NAMES: [&'static str; 12] = [
        "progress",
        "progressEarly",
        "shellEdit",
        "constraints",
        "namedChecks",
        "passingSnapshots",
        "backOff",
        "pace",
        "limitVerdicts",
        "staleDeliverables",
        "adaptiveEffort",
        "thinkingTimeBox",
    ];

    /// From a list of names (`KODACT_NUDGES=progress,shellEdit`); `all` turns every one on,
    /// `none` (or an empty list) none. Unknown names are returned for a warning.
    pub fn from_list(list: &str) -> (NudgeConfig, Vec<String>) {
        let mut c = NudgeConfig::default();
        let mut unknown = vec![];
        for name in list.split([',', ' ']).map(str::trim).filter(|n| !n.is_empty()) {
            if name.eq_ignore_ascii_case("all") {
                for n in Self::NAMES {
                    c.set(n, true);
                }
            } else if !name.eq_ignore_ascii_case("none") && !c.set(name, true) {
                unknown.push(name.to_string());
            }
        }
        (c, unknown)
    }

    /// Turn one nudge on or off by its name; false for a name that isn't one.
    pub fn set(&mut self, name: &str, on: bool) -> bool {
        let field = match name {
            "progress" => &mut self.progress,
            "progressEarly" => &mut self.progress_early,
            "shellEdit" => &mut self.shell_edit,
            "constraints" => &mut self.constraints,
            "namedChecks" => &mut self.named_checks,
            "passingSnapshots" => &mut self.passing_snapshots,
            "backOff" => &mut self.back_off,
            "pace" => &mut self.pace,
            "limitVerdicts" => &mut self.limit_verdicts,
            "staleDeliverables" => &mut self.stale_deliverables,
            "adaptiveEffort" => &mut self.adaptive_effort,
            "thinkingTimeBox" => &mut self.thinking_time_box,
            _ => return false,
        };
        *field = on;
        true
    }

    pub fn any(&self) -> bool {
        *self != NudgeConfig::default()
    }
}

/// A kind of nudge, for the transcript (`system/nudge`) and for back-off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Nudge {
    ProgressEarly,
    Progress,
    ShellEdit,
    Constraints,
    PassingSnapshot,
    LimitVerdict,
    StaleDeliverable,
    SizeLimit,
    AdaptiveEffort,
    ThinkingTimeBox,
}

impl Nudge {
    pub fn as_str(self) -> &'static str {
        match self {
            Nudge::ProgressEarly => "progress_early",
            Nudge::Progress => "progress",
            Nudge::ShellEdit => "shell_edit",
            Nudge::Constraints => "constraints",
            Nudge::PassingSnapshot => "passing_snapshot",
            Nudge::LimitVerdict => "limit_verdict",
            Nudge::StaleDeliverable => "stale_deliverable",
            Nudge::SizeLimit => "size_limit",
            Nudge::AdaptiveEffort => "adaptive_effort",
            Nudge::ThinkingTimeBox => "thinking_time_box",
        }
    }
}

/// What the run has to spend, and what it has spent: the budgets a share is measured against.
/// Each is `None` when the run has no such limit.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Budget {
    /// `--max-turns`.
    pub max_calls: Option<u32>,
    /// `--max-time`'s whole length.
    pub time: Option<Duration>,
    /// `--max-budget-usd`.
    pub usd: Option<f64>,
}

impl Budget {
    pub fn exists(&self) -> bool {
        self.max_calls.is_some() || self.time.is_some() || self.usd.is_some()
    }
}

/// How far a stretch without progress has gone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Stretch {
    /// Model calls since the last progress (or the turn's start).
    pub calls: u32,
    /// The largest share of any budget spent since then (0 without a budget).
    pub share: f64,
    /// Calls in it that wrote again to files already written, with no check passing between.
    pub rewrites: u32,
}

/// One turn's progress: when something was last made, and which nudges this stretch has had.
#[derive(Debug, Clone)]
pub(crate) struct Progress {
    calls: u32,
    since: Instant,
    /// Time spent waiting on tools since then: not the model's to shorten.
    waited: Duration,
    cost_since: f64,
    made_any: bool,
    nudged_this_stretch: bool,
    rewrites: u32,
}

impl Default for Progress {
    fn default() -> Self {
        Progress::new(0.0)
    }
}

impl Progress {
    pub fn new(cost_now: f64) -> Progress {
        Progress {
            calls: 0,
            since: Instant::now(),
            waited: Duration::ZERO,
            cost_since: cost_now,
            made_any: false,
            nudged_this_stretch: false,
            rewrites: 0,
        }
    }

    /// A model call that made no progress.
    pub fn step(&mut self) {
        self.calls += 1;
    }

    /// This step wrote again to files already written since a check last passed.
    pub fn rewrote(&mut self) {
        self.rewrites += 1;
    }

    /// Time spent waiting on tools: left out of the stretch's time.
    pub fn waited(&mut self, d: Duration) {
        self.waited += d;
    }

    /// Something was made: a file written, a check passed (or, when the request names no file,
    /// a call that changed something). The next stretch starts now.
    pub fn made(&mut self, cost_now: f64) {
        *self = Progress { made_any: true, ..Progress::new(cost_now) };
    }

    #[cfg(test)]
    pub fn made_any(&self) -> bool {
        self.made_any
    }

    pub fn stretch(&self, budget: &Budget, cost_now: f64) -> Stretch {
        let mut share: f64 = 0.0;
        if let Some(m) = budget.max_calls.filter(|m| *m > 0) {
            share = share.max(f64::from(self.calls) / f64::from(m));
        }
        if let Some(t) = budget.time.filter(|t| !t.is_zero()) {
            share = share.max(time_share(self.since.elapsed(), self.waited, t));
        }
        if let Some(u) = budget.usd.filter(|u| *u > 0.0) {
            share = share.max((cost_now - self.cost_since).max(0.0) / u);
        }
        Stretch { calls: self.calls, share, rewrites: self.rewrites }
    }

    /// The nudge due now, if any (at most one per stretch). `early` and `pacing` say which are on.
    pub fn due(&mut self, budget: &Budget, cost_now: f64, early: bool, pacing: bool) -> Option<(Nudge, Stretch)> {
        if self.nudged_this_stretch {
            return None;
        }
        let s = self.stretch(budget, cost_now);
        let kind = decide(budget.exists(), self.made_any, s, early, pacing)?;
        self.nudged_this_stretch = true;
        Some((kind, s))
    }
}

/// The share of a time budget a stretch took: its time, less what was spent waiting on tools.
pub(crate) fn time_share(elapsed: Duration, waited: Duration, total: Duration) -> f64 {
    elapsed.saturating_sub(waited).as_secs_f64() / total.as_secs_f64()
}

/// Which nudge a stretch calls for: the decision alone, for the tests.
pub(crate) fn decide(has_budget: bool, made_any: bool, s: Stretch, early: bool, pacing: bool) -> Option<Nudge> {
    if s.calls < MIN_CALLS {
        return None;
    }
    if has_budget {
        if early && !made_any && s.share >= EARLY_SHARE {
            return Some(Nudge::ProgressEarly);
        }
        if pacing && s.share >= PACING_SHARE {
            return Some(Nudge::Progress);
        }
        return None;
    }
    (pacing && s.calls >= NO_BUDGET_CALLS).then_some(Nudge::Progress)
}

/// Steps left at the time limit: the model calls the time left holds at the run's pace so far
/// (`elapsed` over `calls`). `None` before two calls, when there is no pace yet.
pub(crate) fn steps_left(left: Duration, elapsed: Duration, calls: u32) -> Option<(u32, Duration)> {
    if calls < 2 || elapsed.is_zero() {
        return None;
    }
    let pace = elapsed / calls;
    let steps = (left.as_secs_f64() / pace.as_secs_f64()).floor();
    Some((steps.min(f64::from(u32::MAX)) as u32, pace))
}

/// Steps left at or under which the time warning comes, whatever share of the time is left:
/// on a run whose every step takes a minute, a fifth of half an hour is five steps.
pub(crate) const FEW_STEPS: u32 = 5;

/// Is the time-limit warning due: a fifth of the time left (at least a minute), or, with a pace
/// known, [`FEW_STEPS`] steps or fewer.
pub(crate) fn time_warning_due(left: Duration, total: Duration, steps: Option<u32>) -> bool {
    left <= (total / 5).max(Duration::from_secs(60)) || steps.is_some_and(|n| n <= FEW_STEPS)
}

/// What the model is told, as a suggestion it can set aside.
pub(crate) fn text(kind: Nudge, s: Stretch, has_budget: bool) -> String {
    let pct = (s.share * 100.0).round() as u32;
    // Most of the stretch went into the same files: the next step is a check, not a change.
    if kind == Nudge::Progress && s.rewrites * 2 >= s.calls && s.rewrites > 0 {
        let spent = if has_budget { format!(" (about {pct}% of this run's budget)") } else { String::new() };
        return format!(
            "<system-reminder>\n{} model calls{spent} have passed since a file was first changed or a check \
             passed, {} of them changing files you had already changed. Run a check on what is there (the \
             tests, the build, the program itself) before changing it further: it tells you whether the \
             changes are going the right way. Ignore this if a check can't run until the change is \
             complete.\n</system-reminder>",
            s.calls, s.rewrites
        );
    }
    let body = match (kind, has_budget) {
        (Nudge::ProgressEarly, _) => format!(
            "About {pct}% of this run's budget has gone in {} model calls, and nothing has been written or \
             checked yet. If you know enough for a first version of what was asked, make it now and improve it \
             afterwards: a working first version is worth more than a better plan when the limit comes. If you \
             still need to look before any change makes sense, go on.",
            s.calls
        ),
        (_, true) => format!(
            "{} model calls (about {pct}% of this run's budget) have passed since anything was written or a check \
             passed. Take stock: if you have what you need, make the change now; if you are answering a \
             question, answer with what you have. Ignore this if your next step is already a change.",
            s.calls
        ),
        (_, false) => format!(
            "{} model calls have passed since anything was written or a check passed. Take stock: if you have \
             what you need, make the change now; if you are answering a question, answer with what you have. \
             Ignore this if your next step is already a change, or if more looking is what was asked.",
            s.calls
        ),
    };
    format!("<system-reminder>\n{body}\n</system-reminder>")
}

/// The transcript record of a progress nudge.
pub(crate) fn record(kind: Nudge, s: Stretch) -> Value {
    json!({"kind": kind.as_str(), "calls": s.calls, "share": (s.share * 1000.0).round() / 1000.0})
}

/// Back-off, across a run, for the pacing nudges: one followed by progress only late,
/// [`BACK_OFF_AFTER`] times, is silenced; one acted on (progress within [`ACTED_WITHIN`] calls)
/// is kept.
#[derive(Debug, Clone, Default)]
pub(crate) struct BackOff {
    /// Pacing nudges given and not yet followed by progress, with the model calls since.
    waiting: Vec<(Nudge, u32)>,
    /// Times each was followed by progress only late: not needed.
    unneeded: HashMap<Nudge, u32>,
}

impl BackOff {
    /// Does this kind back off? Only the pacing nudges, which give advice; a note that
    /// carries a fact the model lacks is always given.
    pub fn backs_off(kind: Nudge) -> bool {
        matches!(kind, Nudge::Progress | Nudge::ProgressEarly)
    }

    pub fn allows(&self, kind: Nudge) -> bool {
        !Self::backs_off(kind) || self.unneeded.get(&kind).copied().unwrap_or(0) < BACK_OFF_AFTER
    }

    pub fn nudged(&mut self, kind: Nudge) {
        if Self::backs_off(kind) {
            self.waiting.retain(|(k, _)| *k != kind);
            self.waiting.push((kind, 0));
        }
    }

    /// A model call that made no progress: one more for each nudge waiting.
    pub fn step(&mut self) {
        for (_, calls) in &mut self.waiting {
            *calls += 1;
        }
    }

    /// Progress was made by this call. A nudge it came within [`ACTED_WITHIN`] calls of was
    /// acted on, and is kept; one it came late to wasn't needed, and counts toward silence.
    /// Returns the nudges this silenced, for the transcript.
    pub fn progress(&mut self) -> Vec<Nudge> {
        let mut silenced = vec![];
        for (kind, calls) in std::mem::take(&mut self.waiting) {
            if calls < ACTED_WITHIN {
                continue;
            }
            let n = self.unneeded.entry(kind).or_insert(0);
            *n += 1;
            if *n == BACK_OFF_AFTER {
                silenced.push(kind);
            }
        }
        silenced
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(calls: u32, share: f64) -> Stretch {
        Stretch { calls, share, rewrites: 0 }
    }

    #[test]
    fn reading_a_few_files_first_is_never_nudged() {
        // However large the share, under MIN_CALLS calls is normal exploration.
        for calls in 0..MIN_CALLS {
            assert_eq!(decide(true, false, s(calls, 0.9), true, true), None, "{calls} calls");
            assert_eq!(decide(false, false, s(calls, 0.0), true, true), None);
        }
    }

    #[test]
    fn with_a_budget_the_share_decides_and_early_comes_first() {
        // max-turns 50: early at 5 calls (10%), pacing at 10 (20%).
        assert_eq!(decide(true, false, s(5, 0.10), true, true), Some(Nudge::ProgressEarly));
        assert_eq!(decide(true, false, s(5, 0.10), false, true), None, "pacing needs a fifth");
        assert_eq!(decide(true, false, s(10, 0.20), false, true), Some(Nudge::Progress));
        // After the first progress, only pacing.
        assert_eq!(decide(true, true, s(5, 0.15), true, true), None);
        assert_eq!(decide(true, true, s(10, 0.20), true, true), Some(Nudge::Progress));
        // A large budget (max-turns 500): 20 varied calls are 4%, no nudge yet.
        assert_eq!(decide(true, false, s(20, 0.04), true, true), None);
        // Off is off.
        assert_eq!(decide(true, false, s(40, 0.9), false, false), None);
    }

    #[test]
    fn without_a_budget_only_the_count_and_never_early() {
        assert_eq!(decide(false, false, s(NO_BUDGET_CALLS - 1, 0.0), true, true), None);
        assert_eq!(decide(false, false, s(NO_BUDGET_CALLS, 0.0), true, true), Some(Nudge::Progress));
        assert_eq!(decide(false, false, s(40, 0.0), true, false), None, "early needs a budget");
    }

    #[test]
    fn one_nudge_per_stretch_and_progress_starts_a_new_one() {
        let b = Budget { max_calls: Some(20), ..Default::default() };
        let mut p = Progress::new(0.0);
        for _ in 0..5 {
            p.step();
        }
        assert_eq!(p.due(&b, 0.0, true, true).map(|(k, _)| k), Some(Nudge::ProgressEarly));
        p.step();
        assert_eq!(p.due(&b, 0.0, true, true), None, "once per stretch");
        p.made(0.0);
        assert!(p.made_any());
        for _ in 0..4 {
            p.step();
            assert_eq!(p.due(&b, 0.0, true, true), None);
        }
        p.step();
        assert_eq!(p.due(&b, 0.0, true, true).map(|(k, _)| k), Some(Nudge::Progress), "25% of 20, 5 calls");
    }

    #[test]
    fn money_and_time_budgets_count_from_the_last_progress() {
        let b = Budget { usd: Some(1.0), ..Default::default() };
        let mut p = Progress::new(0.50);
        for _ in 0..6 {
            p.step();
        }
        assert!((p.stretch(&b, 0.70).share - 0.20).abs() < 1e-9, "20 cents of a dollar since the start");
        let t = Budget { time: Some(Duration::from_secs(3600)), ..Default::default() };
        assert!(p.stretch(&t, 0.0).share < 0.01, "seconds of an hour");
    }

    #[test]
    fn waiting_on_a_tool_is_not_time_without_progress() {
        let m = Duration::from_secs(60);
        // Half an hour: ten minutes in a build and one of the model's own is a thirtieth.
        assert!((time_share(11 * m, 10 * m, 30 * m) - 1.0 / 30.0).abs() < 1e-9);
        assert!((time_share(11 * m, Duration::ZERO, 30 * m) - 11.0 / 30.0).abs() < 1e-9, "as before without waits");
        assert_eq!(time_share(m, 2 * m, 30 * m), 0.0, "never below nothing");
        let mut p = Progress::new(0.0);
        p.waited(Duration::from_secs(3600));
        let t = Budget { time: Some(30 * m), ..Default::default() };
        assert_eq!(p.stretch(&t, 0.0).share, 0.0);
        p.made(0.0);
        assert_eq!(p.waited, Duration::ZERO, "a new stretch starts with no waits");
    }

    #[test]
    fn back_off_keeps_a_nudge_acted_on_and_drops_one_not_needed() {
        let mut b = BackOff::default();
        // Acted on: progress in the next call, or the one after. Kept, however often.
        for _ in 0..5 {
            b.nudged(Nudge::Progress);
            assert!(b.progress().is_empty());
            b.nudged(Nudge::Progress);
            b.step();
            assert!(b.progress().is_empty(), "within {ACTED_WITHIN} calls: acted on");
        }
        assert!(b.allows(Nudge::Progress), "never silenced for having worked");
        assert!(b.progress().is_empty(), "progress with no nudge waiting counts nothing");
        // Not needed: progress came, but only calls later. Twice silences it.
        b.nudged(Nudge::Progress);
        b.step();
        b.step();
        assert!(b.progress().is_empty());
        assert!(b.allows(Nudge::Progress));
        b.nudged(Nudge::Progress);
        for _ in 0..6 {
            b.step();
        }
        assert_eq!(b.progress(), vec![Nudge::Progress], "the second time silences it");
        assert!(!b.allows(Nudge::Progress));
        assert!(b.allows(Nudge::ProgressEarly), "each nudge on its own count");
    }

    #[test]
    fn notes_that_carry_a_fact_never_back_off() {
        let mut b = BackOff::default();
        let facts =
            [Nudge::Constraints, Nudge::ShellEdit, Nudge::PassingSnapshot, Nudge::SizeLimit, Nudge::StaleDeliverable];
        for kind in facts {
            for _ in 0..4 {
                b.nudged(kind);
                for _ in 0..10 {
                    b.step();
                }
                assert!(b.progress().is_empty());
            }
            assert!(b.allows(kind), "{kind:?}");
        }
    }

    #[test]
    fn steps_left_follow_the_runs_own_pace() {
        let m = Duration::from_secs(60);
        // 28 calls in 29 minutes: about a minute a step; 5 minutes left are 5 steps.
        let (steps, pace) = steps_left(5 * m, 29 * m, 28).unwrap();
        assert_eq!((steps, pace.as_secs()), (4, 62));
        // A fast model: 2 seconds a step, 5 minutes are 150 steps.
        assert_eq!(steps_left(5 * m, Duration::from_secs(60), 30).unwrap().0, 150);
        assert_eq!(steps_left(5 * m, m, 1), None, "no pace from one call");
    }

    #[test]
    fn the_time_warning_comes_when_few_steps_are_left_whatever_the_share() {
        let m = Duration::from_secs(60);
        // Half an hour, ten minutes left: not a fifth yet.
        assert!(!time_warning_due(10 * m, 30 * m, None));
        assert!(!time_warning_due(10 * m, 30 * m, Some(40)), "a fast run has 40 steps left");
        assert!(time_warning_due(10 * m, 30 * m, Some(5)), "a slow run has 5: now");
        assert!(time_warning_due(5 * m, 30 * m, Some(400)), "a fifth left: as before");
    }

    #[test]
    fn names_turn_nudges_on() {
        let (c, unknown) = NudgeConfig::from_list("progress, shellEdit,bogus");
        assert!(c.progress && c.shell_edit && !c.constraints);
        assert_eq!(unknown, ["bogus"]);
        let (all, _) = NudgeConfig::from_list("all");
        assert!(NudgeConfig::NAMES.iter().all(|n| {
            let mut one = NudgeConfig::default();
            one.set(n, true);
            all.any() && one.any()
        }));
        assert_eq!(NudgeConfig::from_list("none").0, NudgeConfig::default());
        assert!(!NudgeConfig::default().any(), "off by default");
    }

    #[test]
    fn the_texts_are_suggestions() {
        let t = text(Nudge::Progress, s(12, 0.24), true);
        assert!(t.contains("12 model calls") && t.contains("24%") && t.contains("Ignore this"), "{t}");
        let t = text(Nudge::Progress, s(15, 0.0), false);
        assert!(!t.contains('%') && t.contains("answer with what you have"), "{t}");
        let t = text(Nudge::ProgressEarly, s(5, 0.1), true);
        assert!(t.contains("first version") && t.contains("go on"), "{t}");
    }
}
