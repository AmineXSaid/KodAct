//! A reply that repeats itself while it streams (GOALS pillar 2).
//!
//! A model stuck in its reasoning or its prose writes the same passage over and over until the
//! output cap, which costs a whole reply in time and tokens before anything can react. The
//! watcher reads the text as it streams and says when its end is one passage repeated, so the
//! request can be stopped and the model told.
//!
//! It asks for a lot before it speaks: the last [`TAIL`] characters must be exact repeats of
//! one passage, at least [`COPIES`] times over. A long answer, a table, a list with numbered
//! items or code with similar lines is not that, and is never cut. Tool-call arguments are not
//! watched: a file's contents may well repeat.

use std::collections::VecDeque;

/// The stretch at the end of the reply that must be all repeats.
const TAIL: usize = 6_000;
/// How many times over the passage must fill that stretch.
const COPIES: usize = 6;
/// Characters between two looks at the tail.
const STEP: usize = 500;
/// Characters of the passage quoted to the model.
const QUOTE: usize = 240;

#[derive(Debug, Default)]
pub(crate) struct Watcher {
    tail: VecDeque<char>,
    since_check: usize,
}

/// What was found: the repeated passage (its start) and how many times it came.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Repeating {
    pub passage: String,
    pub copies: usize,
    /// It was in the model's reasoning, not in its answer.
    pub reasoning: bool,
}

impl Repeating {
    /// The reminder sent in place of the reply.
    pub fn text(&self) -> String {
        let what = if self.reasoning { "reasoning" } else { "reply" };
        format!(
            "<system-reminder>\nYour {what} was stopped because it was repeating itself: the same passage came \
             {} times in a row. It began:\n{}\nDo not write it again and do not restart the analysis. Act on what \
             you have now: take the next concrete step (run a command, make an edit, or give your answer), then \
             continue from its result.\n</system-reminder>",
            self.copies, self.passage
        )
    }
}

/// The smallest period of `s`: the least `p` with `s[i] == s[i + p]` throughout.
fn period(s: &[char]) -> usize {
    let mut pi = vec![0usize; s.len()];
    for i in 1..s.len() {
        let mut k = pi[i - 1];
        while k > 0 && s[i] != s[k] {
            k = pi[k - 1];
        }
        if s[i] == s[k] {
            k += 1;
        }
        pi[i] = k;
    }
    s.len() - pi.last().copied().unwrap_or(0)
}

impl Watcher {
    /// Add streamed text; `Some` once the end of the reply is one passage repeated.
    pub fn push(&mut self, text: &str, reasoning: bool) -> Option<Repeating> {
        for c in text.chars() {
            if self.tail.len() == TAIL {
                self.tail.pop_front();
            }
            self.tail.push_back(c);
        }
        self.since_check += text.chars().count();
        if self.tail.len() < TAIL || self.since_check < STEP {
            return None;
        }
        self.since_check = 0;
        let tail: Vec<char> = self.tail.iter().copied().collect();
        let p = period(&tail);
        (p * COPIES <= TAIL).then(|| Repeating {
            passage: tail[..p.min(QUOTE)].iter().collect::<String>().trim().to_string(),
            copies: TAIL / p,
            reasoning,
        })
    }

    /// A new block began (another kind of text, a tool call): what came before is not part of
    /// a run with what follows.
    pub fn reset(&mut self) {
        self.tail.clear();
        self.since_check = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(w: &mut Watcher, text: &str) -> Option<Repeating> {
        // In small pieces, as a stream delivers it.
        let chars: Vec<char> = text.chars().collect();
        chars.chunks(37).find_map(|c| w.push(&c.iter().collect::<String>(), true))
    }

    #[test]
    fn a_passage_repeated_to_fill_the_tail_is_found() {
        let passage = "Let me reconsider the approach: the parser must handle the nested case first. ";
        let mut w = Watcher::default();
        let found = feed(&mut w, &format!("Some thinking first. {}", passage.repeat(120))).unwrap();
        assert!(found.copies >= COPIES && found.reasoning);
        assert!(found.text().contains("was repeating itself"), "{}", found.text());
        // Whatever the rotation the tail starts at, the quote is from the passage.
        assert!(passage.repeat(2).contains(&found.passage), "{}", found.passage);
    }

    #[test]
    fn it_is_found_soon_after_the_tail_fills() {
        let passage = "the same thought again and again. ";
        let mut w = Watcher::default();
        let mut sent = 0;
        let found = loop {
            sent += passage.len();
            if let Some(f) = w.push(passage, false) {
                break f;
            }
            assert!(sent < TAIL + 2 * STEP, "not found after {sent} characters");
        };
        assert!(!found.reasoning && sent >= TAIL);
    }

    #[test]
    fn long_text_that_does_not_repeat_exactly_is_never_cut() {
        // Numbered lines: similar, never the same.
        let list: String = (0..2_000).map(|i| format!("{i}. check the handler for case {i}\n")).collect();
        assert_eq!(feed(&mut Watcher::default(), &list), None);
        // A passage a few times over, well short of the tail.
        let passage = "A paragraph that the answer quotes more than once, as answers sometimes do. ";
        let some = format!("{}{}", "x".repeat(10), passage.repeat(5));
        assert_eq!(feed(&mut Watcher::default(), &some), None);
        // A long passage repeated only three times fills the tail but is under the count.
        let long: String = (0..60).map(|i| format!("sentence number {i} of a long passage. ")).collect();
        assert!(long.len() > TAIL / COPIES);
        assert_eq!(feed(&mut Watcher::default(), &long.repeat(4)), None);
        // Repeats, then new text: the tail is no longer all repeats.
        let mut w = Watcher::default();
        let mixed = format!("{}{}", "loop. ".repeat(900), list);
        let chars: Vec<char> = mixed.chars().collect();
        let after: usize = "loop. ".len() * 900;
        let late = chars[after..].chunks(STEP).filter_map(|c| w.push(&c.iter().collect::<String>(), false)).count();
        assert_eq!(late, 0);
    }

    #[test]
    fn a_new_block_starts_the_watch_over() {
        let mut w = Watcher::default();
        for _ in 0..(TAIL / 10 - 20) {
            assert_eq!(w.push("thinking. ", true), None);
        }
        w.reset();
        for _ in 0..100 {
            assert_eq!(w.push("thinking. ", false), None, "the two blocks are not one run");
        }
    }

    #[test]
    fn the_period_of_a_string() {
        let chars = |s: &str| s.chars().collect::<Vec<_>>();
        assert_eq!(period(&chars("abcabcabc")), 3);
        assert_eq!(period(&chars("aaaa")), 1);
        assert_eq!(period(&chars("abcd")), 4);
        assert_eq!(period(&chars("abcab")), 3);
    }
}
