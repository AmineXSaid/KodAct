//! What an act reminder hands back of a reply's reasoning, where the provider doesn't send
//! reasoning back (chat completions): a reply cut off at the reply cap, stopped by the thinking
//! time limit, or ended with no answer would otherwise lose it, and the next reply works it out
//! again. How much comes back follows the model's limits ([`budget_chars`]); what doesn't fit is
//! saved whole in the session's spill directory. A paragraph repeated word for word comes back
//! once, at its last place, and none that an earlier reminder already gave comes back again.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use kodact_types::{Message, Role};
use serde_json::{json, Value};

/// The least reasoning a handback carries, in characters, when the room before compaction holds
/// it. A fixed 8,000 kept under a tenth of a 70,000 to 110,000-character analysis on a 524k
/// window, the middle with the findings was lost, and the next reply began it again ("Let me
/// restate the reverse-engineered logic") and was cut off again (2026-11-14).
pub(crate) const MIN_HANDBACK_CHARS: usize = 8_000;
/// When the reasoning is longer than the handback: one part in this many goes to how it began
/// (the reading of the task), the rest to where it stopped (the latest step).
const HEAD_SHARE: usize = 5;
/// Tokens kept free below the compaction threshold for the reminder's own words.
const MARGIN_TOKENS: u64 = 512;
/// A paragraph shorter than this is never taken for a repeat: short ones ("Wait.", a closing
/// brace, a code fence) recur naturally and carry the shape of a draft.
const MIN_REPEAT_CHARS: usize = 40;
/// The tags around reasoning in an act reminder: whole, or how it began and where it stopped.
const TAGS: [&str; 3] = ["reasoning", "reasoning_start", "reasoning_end"];

/// What an act reminder hands back of a reply's reasoning.
pub(crate) struct Handback {
    /// The reminder.
    pub text: String,
    /// Characters of reasoning it carries.
    pub kept: usize,
    /// Where the whole reasoning was saved, when part of it was left out.
    pub saved: Option<PathBuf>,
}

impl Handback {
    /// A reminder that hands nothing back.
    pub fn plain(text: impl Into<String>) -> Self {
        Handback { text: text.into(), kept: 0, saved: None }
    }

    /// The reminder's transcript event: its kind, how much reasoning it carried, and where the
    /// whole of it went.
    pub fn event(&self, kind: &str) -> Value {
        let mut e = json!({"kind": kind, "reasoning_chars": self.kept});
        if let Some(p) = &self.saved {
            e["saved"] = json!(p.display().to_string());
        }
        e
    }
}

/// Characters of reasoning a handback may carry, from the reply cap and the `room` (tokens) the
/// conversation has left before the next request compacts: a quarter of the cap (one reply's
/// reasoning is at most one cap), or half the room when that is more, and at least
/// [`MIN_HANDBACK_CHARS`]; but never more than the room holds, so the handback can't be what
/// compacts it away. Half the room leaves the other half to the reply that acts on it.
pub(crate) fn budget_chars(cap: u64, room: u64) -> usize {
    let per_token = kodact_compact::CHARS_PER_TOKEN as u64;
    let least = MIN_HANDBACK_CHARS as u64 / per_token;
    let tokens = (cap / 4).max(room / 2).max(least).min(room.saturating_sub(MARGIN_TOKENS));
    usize::try_from(tokens.saturating_mul(per_token)).unwrap_or(usize::MAX)
}

/// The reminder: `what` happened, the reasoning as far as `room` (characters) holds, and the ask
/// to go on from it and act. `above` holds the paragraphs earlier reminders gave
/// ([`handed_back`]). When part is left out, `save` keeps the whole reasoning and the reminder
/// names the file.
pub(crate) fn notes(
    what: &str,
    reasoning: &str,
    above: &HashSet<String>,
    room: usize,
    save: impl FnOnce(&str) -> Option<PathBuf>,
) -> Handback {
    let reasoning = reasoning.trim();
    let text = without_repeats(reasoning, above);
    let n = text.chars().count();
    let (shown, kept, saved) = if n <= room {
        (format!(" Here it is:\n\n<reasoning>\n{text}\n</reasoning>"), n, None)
    } else {
        let saved = save(reasoning);
        let file = saved.as_ref().map_or(String::new(), |p| {
            format!(
                "\n\nThe full reasoning ({} characters) is saved at {}. Grep it, or Read it with offset and \
                 limit, instead of working it out again.",
                reasoning.chars().count(),
                p.display()
            )
        });
        if room == 0 {
            (format!(" The conversation has no room for it now.{file}"), 0, saved)
        } else {
            let head_len = room / HEAD_SHARE;
            let head: String = text.chars().take(head_len).collect();
            let tail: String = text.chars().skip(n - (room - head_len)).collect();
            let gap = n - room;
            (
                format!(
                    " Here is how it began and where it stopped ({gap} characters in between left out):\n\n\
                     <reasoning_start>\n{head}\n</reasoning_start>\n\n<reasoning_end>\n{tail}\n</reasoning_end>{file}"
                ),
                room,
                saved,
            )
        }
    };
    let text = format!(
        "<system-reminder>\n{what} Your reasoning is not kept from one reply to the next.{shown}\n\n\
         Go on from where it stopped instead of starting over, and act now: take the next concrete step \
         (run a command, make an edit, or give your answer), then continue from its result.\n</system-reminder>"
    );
    Handback { text, kept, saved }
}

/// The paragraphs of reasoning that act reminders handed back since the model last called a
/// tool. Read from the conversation itself, so a reminder that compaction or a rewind took out
/// no longer counts.
pub(crate) fn handed_back(messages: &[Message]) -> HashSet<String> {
    let mut seen = HashSet::new();
    for m in messages.iter().rev() {
        if m.role == Role::Assistant && m.tool_uses().next().is_some() {
            break;
        }
        if m.role != Role::User {
            continue;
        }
        for text in m.content.iter().filter_map(|b| b.as_text()).filter(|t| t.contains("<system-reminder>")) {
            for tag in TAGS {
                for part in tagged(text, &format!("<{tag}>\n"), &format!("\n</{tag}>")) {
                    seen.extend(paragraphs(part).into_iter().filter_map(repeatable).map(str::to_string));
                }
            }
        }
    }
    seen
}

/// Why a paragraph was left out of a handback.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dropped {
    /// The same paragraph comes again later in this reasoning.
    Below,
    /// An earlier reminder already handed it back.
    Above,
}

fn marker(why: Dropped, n: usize) -> String {
    let what = if n == 1 { "a paragraph".to_string() } else { format!("{n} paragraphs") };
    match why {
        Dropped::Below => format!("[{what} repeated below]"),
        Dropped::Above => format!("[{what} handed back above]"),
    }
}

/// `text` with each repeated paragraph given once, at its last place, so a revised draft keeps
/// all of its parts, and with none that `above` already holds. What is left out leaves a short
/// marker (several in a row, one), so a draft keeps its shape. With nothing left out, `text` as
/// it is.
pub(crate) fn without_repeats(text: &str, above: &HashSet<String>) -> String {
    let paras = paragraphs(text);
    let keys: Vec<Option<&str>> = paras.iter().map(|p| repeatable(p)).collect();
    let mut last: HashMap<&str, usize> = HashMap::new();
    for (i, k) in keys.iter().enumerate() {
        if let Some(k) = k {
            last.insert(k, i);
        }
    }
    let mut out: Vec<String> = Vec::with_capacity(paras.len());
    let mut run: Option<(Dropped, usize)> = None;
    let mut dropped = false;
    for (i, p) in paras.iter().enumerate() {
        let why = keys[i].and_then(|k| {
            if above.contains(k) {
                Some(Dropped::Above)
            } else if last[k] != i {
                Some(Dropped::Below)
            } else {
                None
            }
        });
        match (why, run) {
            (Some(w), Some((r, n))) if w == r => run = Some((r, n + 1)),
            (Some(w), _) => {
                if let Some((r, n)) = run {
                    out.push(marker(r, n));
                }
                run = Some((w, 1));
            }
            (None, _) => {
                if let Some((r, n)) = run.take() {
                    out.push(marker(r, n));
                }
                out.push((*p).to_string());
            }
        }
        dropped |= why.is_some();
    }
    if let Some((r, n)) = run {
        out.push(marker(r, n));
    }
    if dropped {
        out.join("\n\n")
    } else {
        text.to_string()
    }
}

/// A paragraph, trimmed, when it is long enough to count as a repeat.
fn repeatable(p: &str) -> Option<&str> {
    Some(p.trim()).filter(|k| k.chars().count() >= MIN_REPEAT_CHARS)
}

/// `text`'s paragraphs (separated by blank lines), as slices without their line breaks.
pub(crate) fn paragraphs(text: &str) -> Vec<&str> {
    let mut out = vec![];
    let (mut start, mut end, mut pos) = (None, 0, 0);
    for line in text.split_inclusive('\n') {
        if line.trim().is_empty() {
            if let Some(s) = start.take() {
                out.push(&text[s..end]);
            }
        } else {
            start.get_or_insert(pos);
            end = pos + line.trim_end_matches(['\n', '\r']).len();
        }
        pos += line.len();
    }
    if let Some(s) = start {
        out.push(&text[s..end]);
    }
    out
}

/// The parts of `text` between each `open` and the `close` after it.
fn tagged<'a>(text: &'a str, open: &str, close: &str) -> Vec<&'a str> {
    let mut out = vec![];
    let mut rest = text;
    while let Some(i) = rest.find(open) {
        let after = &rest[i + open.len()..];
        let Some(j) = after.find(close) else { break };
        out.push(&after[..j]);
        rest = &after[j + close.len()..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> HashSet<String> {
        HashSet::new()
    }

    #[test]
    fn the_budget_follows_the_room_and_never_fills_it() {
        // A large window: half the room, far more than a reply's reasoning.
        assert_eq!(budget_chars(32_000, 800_000), 1_600_000);
        // Room for a quarter of the cap: that much (32,000 characters on a 32k cap).
        assert_eq!(budget_chars(32_000, 12_500), 32_000);
        // A nearly full window: what fits before the next request would compact, under the floor.
        assert_eq!(budget_chars(32_000, 2_500), (2_500 - 512) * 4);
        // No room at all: nothing.
        assert_eq!(budget_chars(32_000, 300), 0);
        // A small cap with room: never under the floor.
        assert_eq!(budget_chars(1_000, 9_000), MIN_HANDBACK_CHARS.max(9_000 / 2 * 4));
        assert_eq!(budget_chars(1_000, 4_000), MIN_HANDBACK_CHARS);
    }

    #[test]
    fn paragraphs_split_at_blank_lines_of_any_kind() {
        let text = "one\r\ntwo\r\n\r\nthree\n   \n\nfour";
        assert_eq!(paragraphs(text), ["one\r\ntwo", "three", "four"]);
        assert_eq!(paragraphs("no blank line at all"), ["no blank line at all"]);
        assert!(paragraphs("\n\n  \n").is_empty());
    }

    #[test]
    fn a_repeat_is_kept_at_its_last_place_and_a_draft_stays_whole() {
        let includes = "#include <stdio.h>\n#include <stdlib.h>\n#include <math.h>";
        let text = format!(
            "First draft:\n\n{includes}\n\nint main(void) {{\n    return 1;\n}}\n\nRevised:\n\n{includes}\n\n\
             int main(void) {{\n    return 0;\n}}"
        );
        let out = without_repeats(&text, &none());
        assert_eq!(out.matches(includes).count(), 1, "{out}");
        assert!(out.contains(&format!("Revised:\n\n{includes}\n\nint main(void) {{\n    return 0;\n}}")), "{out}");
        assert!(out.contains("First draft:\n\n[a paragraph repeated below]\n\nint main(void)"), "{out}");
        // Short paragraphs ("}", a fence) are never taken for repeats.
        let short = "x\n\n}\n\ny\n\n}";
        assert_eq!(without_repeats(short, &none()), short);
        // Nothing repeated: the text as it is, blank lines and all.
        let plain = "a\n\n\n\nb";
        assert_eq!(without_repeats(plain, &none()), plain);
    }

    #[test]
    fn what_was_handed_back_above_is_marked_not_repeated() {
        let found = "The table at offset 0x40 holds the settings: grid size, scale and the step count.";
        let given = format!(
            "<system-reminder>\nCut off. Here it is:\n\n<reasoning>\n{found}\n</reasoning>\n</system-reminder>"
        );
        let above = handed_back(&[Message::user_text("do it"), Message::user_text(given.clone())]);
        assert!(above.contains(found));
        let out = without_repeats(&format!("Restate.\n\n{found}\n\n{found}\n\nNext: write it."), &above);
        assert_eq!(out, "Restate.\n\n[2 paragraphs handed back above]\n\nNext: write it.");
        // A tool call since then: that handback no longer counts.
        let acted = Message::assistant(vec![kodact_types::ContentBlock::ToolUse {
            id: "t".into(),
            name: "Read".into(),
            input: json!({}),
            cache_control: None,
            provider: None,
        }]);
        assert!(handed_back(&[Message::user_text(given), acted]).is_empty());
    }

    #[test]
    fn what_does_not_fit_keeps_its_start_and_end_and_is_saved_whole() {
        let reasoning = format!("start here. {}end here.", "middle ".repeat(5_000));
        let h = notes("Cut off.", &reasoning, &none(), 10_000, |_| Some(PathBuf::from("/s/reasoning-1.txt")));
        assert_eq!(h.kept, 10_000);
        assert!(h.text.contains("<reasoning_start>\nstart here.") && h.text.contains("end here.\n</reasoning_end>"));
        assert!(h.text.contains("characters in between left out"));
        assert!(h.text.contains("is saved at /s/reasoning-1.txt"), "{}", h.text);
        assert_eq!(h.event("reasoning_cutoff")["saved"], "/s/reasoning-1.txt");
        // Without a spill directory: the start and the end, and no file named.
        let h = notes("Cut off.", &reasoning, &none(), 10_000, |_| None);
        assert!(h.text.contains("left out") && !h.text.contains("saved at"));
        assert!(h.event("reasoning_cutoff").get("saved").is_none());
        // No room at all: the file alone.
        let h = notes("Cut off.", &reasoning, &none(), 0, |_| Some(PathBuf::from("/s/reasoning-2.txt")));
        assert_eq!(h.kept, 0);
        assert!(h.text.contains("no room for it now") && h.text.contains("/s/reasoning-2.txt"));
        assert!(!h.text.contains("<reasoning"));
        // It fits: whole, nothing saved.
        let h = notes("Cut off.", "short", &none(), 10_000, |_| panic!("nothing to save"));
        assert!(h.text.contains("<reasoning>\nshort\n</reasoning>") && h.saved.is_none());
    }
}
