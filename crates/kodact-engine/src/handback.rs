//! What an act reminder hands back of a reply's reasoning, where the provider doesn't send
//! reasoning back (chat completions): a reply cut off at the reply cap, stopped by the thinking
//! time limit, or ended with no answer would otherwise lose it, and the next reply works it out
//! again. How much comes back follows the model's limits ([`budget_bytes`]); what doesn't fit is
//! saved whole in the session's spill directory. A paragraph repeated word for word is shown once,
//! at the last place the reminder shows it, and none that an earlier reminder already gave is
//! shown again; code inside fences always comes back as written.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use kodact_types::{Message, Role};
use serde_json::{json, Value};

/// The least reasoning a handback carries, in bytes (about 8,000 characters of English), when
/// the room before compaction holds it. A fixed 8,000 characters kept under a tenth of a 70,000
/// to 110,000-character analysis on a 524k window, the middle with the findings was lost, and the
/// next reply began it again ("Let me restate the reverse-engineered logic") and was cut off
/// again (2026-11-14).
const MIN_HANDBACK_BYTES: usize = 8_000;
/// When the reasoning is longer than the handback: one part in this many goes to how it began
/// (the reading of the task), the rest to where it stopped (the latest step).
const HEAD_SHARE: usize = 5;
/// Tokens kept free below the compaction threshold for the reminder's own words.
const MARGIN_TOKENS: u64 = 512;
/// A paragraph shorter than this is never taken for a repeat: short ones ("Wait.", a closing
/// brace) recur naturally and carry the shape of a draft.
const MIN_REPEAT_CHARS: usize = 40;
/// The longest line in a saved reasoning file: Read cuts lines past 2,000 characters.
const SAVED_LINE_CHARS: usize = 1_000;
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

/// Bytes of reasoning a handback may carry, from the reply cap and the `room` (tokens) the
/// conversation has left before the next request compacts. Bytes, because that is what the
/// context estimate counts (a character of Chinese is three). A quarter of the cap (one reply's
/// reasoning is at most one cap), or half the room when that is more, and at least
/// [`MIN_HANDBACK_BYTES`]; but never more than the room holds, so the reminder isn't summarized
/// away before the model reads it. Half the room leaves the other half to the reply that acts.
pub(crate) fn budget_bytes(cap: u64, room: u64) -> usize {
    let per_token = kodact_compact::BYTES_PER_TOKEN as u64;
    let least = MIN_HANDBACK_BYTES as u64 / per_token;
    let tokens = (cap / 4).max(room / 2).max(least).min(room.saturating_sub(MARGIN_TOKENS));
    usize::try_from(tokens.saturating_mul(per_token)).unwrap_or(usize::MAX)
}

/// The reminder: `what` happened, the reasoning as far as `room` (bytes) holds, and the ask to go
/// on from it and act. `above` holds the paragraphs earlier reminders gave ([`handed_back`]).
/// When part is left out, `save` keeps the whole reasoning, its long lines wrapped so Read can
/// take all of it, and the reminder names the file.
pub(crate) fn notes(
    what: &str,
    reasoning: &str,
    above: &HashSet<String>,
    room: usize,
    save: impl FnOnce(&str) -> Option<PathBuf>,
) -> Handback {
    let reasoning = reasoning.trim();
    let whole = without_repeats(&[(reasoning, false)], above).remove(0);
    let (shown, kept, saved) = if whole.len() <= room {
        let kept = whole.chars().count();
        (format!(" Here it is:\n\n<reasoning>\n{whole}\n</reasoning>"), kept, None)
    } else {
        let saved = save(&wrapped(reasoning, SAVED_LINE_CHARS));
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
            // Cut first, then give each repeat once within what is shown, so the copy kept is one
            // the model sees.
            let head_end = floor_boundary(reasoning, room / HEAD_SHARE);
            let tail_start =
                ceil_boundary(reasoning, reasoning.len().saturating_sub(room - room / HEAD_SHARE)).max(head_end);
            let (head, tail) = (&reasoning[..head_end], &reasoning[tail_start..]);
            let gap = reasoning[head_end..tail_start].chars().count();
            let parts = without_repeats(&[(head, false), (tail, fence_open_at(reasoning, tail_start))], above);
            let kept = parts.iter().map(|p| p.chars().count()).sum();
            (
                format!(
                    " Here is how it began and where it stopped ({gap} characters in between left out):\n\n\
                     <reasoning_start>\n{}\n</reasoning_start>\n\n<reasoning_end>\n{}\n</reasoning_end>{file}",
                    parts[0], parts[1]
                ),
                kept,
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
                    seen.extend(keyed(part, false).into_iter().filter_map(|(_, k)| k).map(str::to_string));
                }
            }
        }
    }
    seen
}

/// Why a paragraph was left out of a handback.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dropped {
    /// The same paragraph is shown again further on.
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

/// `parts` (each with whether it starts inside a code fence), shown in order, with each repeated
/// paragraph given once, at its last place among them, so a revised draft keeps all of its
/// parts; and with none that `above` already holds. What is left out leaves a short marker
/// (several in a row, one). A part with nothing left out comes back as it is.
fn without_repeats(parts: &[(&str, bool)], above: &HashSet<String>) -> Vec<String> {
    let keyed: Vec<Vec<(&str, Option<&str>)>> = parts.iter().map(|(text, fenced)| keyed(text, *fenced)).collect();
    let mut last: HashMap<&str, (usize, usize)> = HashMap::new();
    for (pi, paras) in keyed.iter().enumerate() {
        for (i, (_, k)) in paras.iter().enumerate() {
            if let Some(k) = k {
                last.insert(k, (pi, i));
            }
        }
    }
    keyed
        .iter()
        .zip(parts)
        .enumerate()
        .map(|(pi, (paras, (text, _)))| {
            let mut out: Vec<String> = Vec::with_capacity(paras.len());
            let mut run: Option<(Dropped, usize)> = None;
            let mut dropped = false;
            for (i, (p, k)) in paras.iter().enumerate() {
                let why = k.and_then(|k| {
                    if above.contains(k) {
                        Some(Dropped::Above)
                    } else if last[k] != (pi, i) {
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
                (*text).to_string()
            }
        })
        .collect()
}

/// `text`'s paragraphs, each with the key it is matched on as a repeat: trimmed, long enough,
/// and not code inside a fence (``` or ~~~), which always comes back as written. `fenced`: the
/// text starts inside a fence.
fn keyed(text: &str, mut fenced: bool) -> Vec<(&str, Option<&str>)> {
    paragraphs(text)
        .into_iter()
        .map(|p| {
            let mut code = fenced;
            for line in p.lines() {
                if is_fence(line) {
                    fenced = !fenced;
                    code = true;
                }
            }
            let key = Some(p.trim()).filter(|k| !code && k.chars().count() >= MIN_REPEAT_CHARS);
            (p, key)
        })
        .collect()
}

fn is_fence(line: &str) -> bool {
    let l = line.trim_start();
    l.starts_with("```") || l.starts_with("~~~")
}

/// Whether `text` is inside a code fence at byte `at`.
fn fence_open_at(text: &str, at: usize) -> bool {
    text[..at].lines().filter(|l| is_fence(l)).count() % 2 == 1
}

/// `text`'s paragraphs (separated by blank lines), as slices without their line breaks.
fn paragraphs(text: &str) -> Vec<&str> {
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

/// The largest char boundary of `s` at or before byte `i`.
fn floor_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// The smallest char boundary of `s` at or after byte `i`.
fn ceil_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// `text` with every line longer than `max` characters broken after its last space within the
/// limit (or at the limit when there is none). Nothing else changes.
fn wrapped(text: &str, max: usize) -> Cow<'_, str> {
    if text.lines().all(|l| l.chars().count() <= max) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + text.len() / max + 1);
    for (n, line) in text.split('\n').enumerate() {
        if n > 0 {
            out.push('\n');
        }
        let mut rest = line;
        while let Some((cut, _)) = rest.char_indices().nth(max) {
            let at = rest[..cut].rfind(' ').map_or(cut, |s| s + 1);
            out.push_str(&rest[..at]);
            out.push('\n');
            rest = &rest[at..];
        }
        out.push_str(rest);
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> HashSet<String> {
        HashSet::new()
    }

    fn one(text: &str, above: &HashSet<String>) -> String {
        without_repeats(&[(text, false)], above).remove(0)
    }

    #[test]
    fn the_budget_follows_the_room_and_never_fills_it() {
        // A large window: half the room, far more than a reply's reasoning.
        assert_eq!(budget_bytes(32_000, 800_000), 1_600_000);
        // Room for a quarter of the cap: that much (32,000 bytes on a 32k cap).
        assert_eq!(budget_bytes(32_000, 12_500), 32_000);
        // A nearly full window: what fits before the next request would compact, under the floor.
        assert_eq!(budget_bytes(32_000, 2_500), (2_500 - 512) * 4);
        // No room at all: nothing.
        assert_eq!(budget_bytes(32_000, 300), 0);
        // A small cap with room: never under the floor.
        assert_eq!(budget_bytes(1_000, 9_000), 18_000);
        assert_eq!(budget_bytes(1_000, 4_000), MIN_HANDBACK_BYTES);
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
        let out = one(&text, &none());
        assert_eq!(out.matches(includes).count(), 1, "{out}");
        assert!(out.contains(&format!("Revised:\n\n{includes}\n\nint main(void) {{\n    return 0;\n}}")), "{out}");
        assert!(out.contains("First draft:\n\n[a paragraph repeated below]\n\nint main(void)"), "{out}");
        // Short paragraphs are never taken for repeats.
        let short = "x\n\n}\n\ny\n\n}";
        assert_eq!(one(short, &none()), short);
        // Nothing repeated: the text as it is, blank lines and all.
        let plain = "a\n\n\n\nb";
        assert_eq!(one(plain, &none()), plain);
    }

    #[test]
    fn code_in_fences_comes_back_as_written() {
        let block = "fn scale(x: f32) -> f32 {\n    x * 4.0 // the grid's scale, decoded above\n}";
        let text = format!("Draft:\n\n```rust\n{block}\n\n{block}\n```\n\nDone.");
        assert_eq!(one(&text, &none()), text, "a repeat inside a fence is kept");
        let above: HashSet<String> = [block.to_string()].into();
        assert_eq!(one(&text, &above), text, "and so is code handed back before");
        // Outside the fence, the same rule as any paragraph.
        let prose = "The table at offset 0x40 holds the settings: grid size, scale and the step count.";
        let mixed = format!("```\nx\n```\n\n{prose}\n\n{prose}");
        assert_eq!(one(&mixed, &none()), format!("```\nx\n```\n\n[a paragraph repeated below]\n\n{prose}"));
    }

    #[test]
    fn what_was_handed_back_above_is_marked_not_repeated() {
        let found = "The table at offset 0x40 holds the settings: grid size, scale and the step count.";
        let given = format!(
            "<system-reminder>\nCut off. Here it is:\n\n<reasoning>\n{found}\n</reasoning>\n</system-reminder>"
        );
        let above = handed_back(&[Message::user_text("do it"), Message::user_text(given.clone())]);
        assert!(above.contains(found));
        let out = one(&format!("Restate.\n\n{found}\n\n{found}\n\nNext: write it."), &above);
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
        // The file holds all of it, its long line wrapped so Read takes all of it.
        let mut file = String::new();
        notes("Cut off.", &reasoning, &none(), 10_000, |w| {
            file = w.to_string();
            None
        });
        assert!(file.lines().all(|l| l.chars().count() <= SAVED_LINE_CHARS));
        assert_eq!(file.replace('\n', ""), reasoning);
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

    #[test]
    fn the_room_is_counted_in_bytes_and_cut_at_characters() {
        // Three bytes a character: 30,000 characters are 90,000 bytes, more than 32,000 of room.
        let reasoning = "推理继续。".repeat(6_000);
        let h = notes("Cut off.", &reasoning, &none(), 32_000, |_| None);
        let shown: usize = tagged(&h.text, "<reasoning_start>\n", "\n</reasoning_start>")[0].len()
            + tagged(&h.text, "<reasoning_end>\n", "\n</reasoning_end>")[0].len();
        assert!(shown <= 32_000 && shown > 31_000, "{shown}");
        assert!(h.kept < 11_000, "{}", h.kept);
    }

    #[test]
    fn a_repeat_whose_last_copy_is_left_out_is_shown_where_it_is_seen() {
        // The opening is restated in the middle, which doesn't fit: the reminder shows it in its
        // start, not a marker pointing at text left out.
        let opening = "Reading the task: decode the table first, then write the program that uses it.";
        let reasoning =
            format!("{opening}\n\n{}\n\n{opening}\n\n{}\n\nThe end.", "a ".repeat(20_000), "b ".repeat(20_000));
        let h = notes("Cut off.", &reasoning, &none(), 10_000, |_| None);
        let start = tagged(&h.text, "<reasoning_start>\n", "\n</reasoning_start>")[0];
        assert!(start.starts_with(opening), "{start:.200}");
        assert!(!h.text.contains("repeated below"));
    }
}
