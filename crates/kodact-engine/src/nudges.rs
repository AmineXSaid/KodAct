//! Nudges (GOALS pillar 5): notes KodAct adds when what the model did gives it a fact the
//! model may not have: a file a shell edit left unparseable, a file the request names that
//! isn't there, a check the request names that never ran, where a passing version is saved,
//! a limit a measurement missed. Each follows what the model does, never the task or which
//! model runs it.
//!
//! All are off unless turned on (`nudges` in settings, or `KODACT_NUDGES`), each once an eval
//! task has measured it, and `reminders: false` turns them off with the rest of KodAct's
//! reminders.
//!
//! Pacing nudges (a long stretch with nothing written, an early first version, the steps left
//! at the run's pace) and their back-off were removed: in the runs that measured them they
//! rarely spoke and never showed a gain, and their note was advice the model could give
//! itself, not a fact it lacked (docs/goal.md, rule 8).

/// Which nudges are on (`nudges.<name>` in settings, or `KODACT_NUDGES`). All are off by
/// default: each is turned on once an eval task has measured it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NudgeConfig {
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
    pub const NAMES: [&'static str; 8] = [
        "shellEdit",
        "constraints",
        "namedChecks",
        "passingSnapshots",
        "limitVerdicts",
        "staleDeliverables",
        "adaptiveEffort",
        "thinkingTimeBox",
    ];

    /// From a list of names (`KODACT_NUDGES=shellEdit,constraints`); `all` turns every one on,
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
            "shellEdit" => &mut self.shell_edit,
            "constraints" => &mut self.constraints,
            "namedChecks" => &mut self.named_checks,
            "passingSnapshots" => &mut self.passing_snapshots,
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

/// A kind of nudge, for the transcript (`system/nudge`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Nudge {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_turn_nudges_on() {
        let (c, unknown) = NudgeConfig::from_list("constraints, shellEdit,bogus");
        assert!(c.constraints && c.shell_edit && !c.named_checks);
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
    fn the_removed_pacing_nudges_are_unknown_names() {
        // Old scripts that still ask for them get a warning, and nothing turns on.
        let (c, unknown) = NudgeConfig::from_list("progress,progressEarly,backOff,pace");
        assert!(!c.any());
        assert_eq!(unknown, ["progress", "progressEarly", "backOff", "pace"]);
    }
}
