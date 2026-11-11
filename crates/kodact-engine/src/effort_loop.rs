//! Adaptive reasoning effort (`nudges.adaptiveEffort`, GOALS pillar 5).
//!
//! A reasoning model can spend a step's whole budget thinking and then do nothing, or think at
//! length about something a quick test would settle; on a time-limited task that is what runs
//! the clock out. Telling it so changes little: it agrees and goes on thinking. Here the level
//! each request asks for follows what the previous steps' thinking produced:
//!
//! - a reply that was all thinking (no answer, no action) lowers the level one step at once;
//! - two steps of long thinking whose actions made no progress (no file the request asks for
//!   changed, no check passed) lower it one step;
//! - two steps in a row that made progress raise it one step again.
//!
//! The level never goes above the one the person set (`--effort`, or `high` without one) and
//! never below `low`: thinking is never switched off. The server enforces the level, so a model
//! can't think past it whatever it decides; where a server takes only some levels, the one sent
//! is the closest it takes (`kodact_api::effort_levels`).

/// The levels the loop moves between, least reasoning first.
pub(crate) const LADDER: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
/// Steps of long thinking with no progress, in a row, before the level steps down.
const DOWN_AFTER: u32 = 2;
/// Steps that made progress, in a row, before the level steps back up.
const UP_AFTER: u32 = 2;

/// How a step went, for the loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// The reply was all thinking: no answer and no tool call.
    AllThinking,
    /// It thought at length, and its actions made no progress.
    LongNoProgress,
    /// Its actions made progress: a file the request asks for changed, or a check passed.
    Progress,
    /// Anything else: a short step that read or looked around. It counts for nothing.
    Other,
}

/// The level now, the highest it may go, and the steps since it last changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EffortLoop {
    top: usize,
    now: usize,
    stuck: u32,
    moving: u32,
}

/// A change of level: from, to, and why.
pub(crate) type Change = (&'static str, &'static str, &'static str);

impl EffortLoop {
    /// Starting at `start` (the person's level), which is also the highest it goes. `None` for a
    /// level the loop doesn't know.
    pub fn new(start: &str) -> Option<EffortLoop> {
        let top = LADDER.iter().position(|l| *l == start)?;
        Some(EffortLoop { top, now: top, stuck: 0, moving: 0 })
    }

    /// The level the next request asks for.
    pub fn level(&self) -> &'static str {
        LADDER[self.now]
    }

    /// Count a step; the change it makes to the level, if any.
    pub fn record(&mut self, step: Step) -> Option<Change> {
        match step {
            Step::AllThinking => {
                self.stuck = DOWN_AFTER;
                self.moving = 0;
            }
            Step::LongNoProgress => {
                self.stuck += 1;
                self.moving = 0;
            }
            Step::Progress => {
                self.moving += 1;
                self.stuck = 0;
            }
            Step::Other => {}
        }
        let from = self.level();
        if self.stuck >= DOWN_AFTER {
            self.stuck = 0;
            if self.now > 0 {
                self.now -= 1;
                let why = if step == Step::AllThinking {
                    "a reply was all thinking"
                } else {
                    "two steps of long thinking made no progress"
                };
                return Some((from, self.level(), why));
            }
        }
        if self.moving >= UP_AFTER && self.now < self.top {
            self.moving = 0;
            self.now += 1;
            return Some((from, self.level(), "two steps in a row made progress"));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_that_leads_nowhere_lowers_the_level_and_progress_raises_it() {
        let mut l = EffortLoop::new("high").unwrap();
        assert_eq!(l.level(), "high");
        assert_eq!(l.record(Step::AllThinking), Some(("high", "medium", "a reply was all thinking")));
        assert_eq!(l.record(Step::LongNoProgress), None, "one long step is not yet a pattern");
        assert_eq!(l.record(Step::Other), None, "a short look around counts for nothing");
        assert_eq!(
            l.record(Step::LongNoProgress),
            Some(("medium", "low", "two steps of long thinking made no progress"))
        );
        assert_eq!(l.record(Step::AllThinking), None, "never below low: thinking stays on");
        assert_eq!(l.level(), "low");
        assert_eq!(l.record(Step::Progress), None);
        assert_eq!(l.record(Step::Progress), Some(("low", "medium", "two steps in a row made progress")));
        assert_eq!(l.record(Step::Progress), None);
        assert_eq!(l.record(Step::Progress), Some(("medium", "high", "two steps in a row made progress")));
        for _ in 0..4 {
            assert_eq!(l.record(Step::Progress), None, "never above the person's level");
        }
        assert_eq!(l.level(), "high");
    }

    #[test]
    fn progress_in_between_resets_the_count() {
        let mut l = EffortLoop::new("medium").unwrap();
        assert_eq!(l.record(Step::LongNoProgress), None);
        assert_eq!(l.record(Step::Progress), None);
        assert_eq!(l.record(Step::LongNoProgress), None, "not two in a row");
        assert_eq!(l.level(), "medium");
        assert!(EffortLoop::new("bogus").is_none());
    }
}
