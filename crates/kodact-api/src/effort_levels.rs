//! The reasoning levels an OpenAI-compatible server takes, learned from its refusals.
//!
//! `--effort` goes out as `reasoning_effort` (`openai::to_chat_request`). Servers take different
//! levels, and say which when they refuse one: DeepSeek V4.1 behind vLLM, "reasoning_effort must
//! be low, high, xhigh, max, or an integer within [1, 100] in chat_template_kwargs"; vLLM's own
//! check, "Input should be 'none', 'minimal', 'low', 'medium', 'high' or 'xhigh'"; OpenAI,
//! "Supported values are: 'low', 'medium', and 'high'". The refusal is read, the level closest to
//! the one asked for is sent instead, and the levels are remembered for the model. Before, a
//! refused level was dropped and the model reasoned at its server's default, whatever was asked.
//!
//! Nothing is assumed about any server: only the levels its own refusal lists are used. A level
//! that turns thinking off (`none`) is never picked for one that doesn't ask for it.

use serde_json::{json, Value};

/// Every named level, from least reasoning to most.
pub const LADDER: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

/// What a server takes for `reasoning_effort`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Accepted {
    /// Named levels, in [`LADDER`] order.
    pub named: Vec<&'static str>,
    /// A whole-number scale it takes too.
    pub scale: Option<Scale>,
}

/// A number scale for the level (`[1, 100]`), and whether it goes in `chat_template_kwargs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scale {
    pub lo: i64,
    pub hi: i64,
    pub in_template: bool,
}

/// What to send for a level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Named(&'static str),
    /// A number on the server's scale, and whether it goes in `chat_template_kwargs`.
    Number(i64, bool),
}

/// The words that introduce the list of what a server takes.
const LIST_WORDS: [&str; 11] = [
    "must be",
    "should be",
    "one of",
    "supported values",
    "valid values",
    "allowed values",
    "expected",
    "valid levels",
    "supported levels",
    "allowed levels",
    "accepted values",
];

/// What a refusal says the server takes for `reasoning_effort`, when it says. `None` for a
/// refusal about anything else, or one that names no level.
pub fn from_refusal(message: &str) -> Option<Accepted> {
    // Gateways nest the server's JSON in their own: quotes come escaped.
    let m = message.to_ascii_lowercase().replace('\\', "");
    let mention =
        ["reasoning_effort", "reasoning effort", "reasoning.effort"].iter().filter_map(|w| m.find(w)).min()?;
    // The list: from the word that introduces it, after the mention when there is one there.
    let start = LIST_WORDS
        .iter()
        .filter_map(|w| m[mention..].find(w).map(|i| i + mention))
        .min()
        .or_else(|| LIST_WORDS.iter().filter_map(|w| m.find(w)).min())?;
    let rest = &m[start..];
    let end = [". ", "\n", "\"}", "\","].iter().filter_map(|e| rest.find(e)).min().unwrap_or(rest.len()).min(400);
    let clause = &rest[..end];
    let named: Vec<&'static str> = LADDER.iter().copied().filter(|l| has_word(clause, l)).collect();
    let scale = clause.find("integer").and_then(|i| {
        let tail = &clause[i..];
        let nums: Vec<i64> =
            tail.split(|c: char| !(c.is_ascii_digit() || c == '-')).filter_map(|n| n.parse().ok()).take(2).collect();
        match nums.as_slice() {
            [lo, hi] if lo < hi => Some(Scale { lo: *lo, hi: *hi, in_template: tail.contains("chat_template_kwargs") }),
            _ => None,
        }
    });
    (!named.is_empty() || scale.is_some()).then_some(Accepted { named, scale })
}

fn has_word(text: &str, w: &str) -> bool {
    text.match_indices(w).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + w.len()..].chars().next();
        before.is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
            && after.is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
    })
}

fn rank(level: &str) -> Option<usize> {
    LADDER.iter().position(|l| *l == level)
}

/// Where a level sits on a number scale: its share of the way from the bottom to the top.
fn share(level: &str) -> f64 {
    match level {
        "minimal" => 0.1,
        "low" => 0.25,
        "medium" => 0.5,
        "high" => 0.75,
        "xhigh" => 0.9,
        _ => 1.0,
    }
}

/// What to send for `wanted` (KodAct's level: low, medium, high, xhigh or max) to a server that
/// takes `accepted`: the level itself when it takes it; else, on a number scale, the matching
/// point (medium is the middle); else the closest level at or below it, or the lowest above it.
/// `none` only when `none` is what's wanted: thinking is never switched off on anyone's behalf.
pub fn pick(wanted: &str, accepted: &Accepted) -> Option<Setting> {
    if let Some(n) = accepted.named.iter().find(|n| **n == wanted) {
        return Some(Setting::Named(n));
    }
    let want = rank(wanted)?;
    if let Some(s) = accepted.scale.filter(|_| wanted != "none") {
        let at = s.lo + ((s.hi - s.lo) as f64 * share(wanted)).round() as i64;
        return Some(Setting::Number(at.clamp(s.lo, s.hi), s.in_template));
    }
    let usable: Vec<(usize, &'static str)> = accepted
        .named
        .iter()
        .filter(|n| **n != "none" || wanted == "none")
        .filter_map(|n| rank(n).map(|r| (r, *n)))
        .collect();
    usable
        .iter()
        .filter(|(r, _)| *r <= want)
        .max()
        .or_else(|| usable.iter().filter(|(r, _)| *r > want).min())
        .map(|(_, n)| Setting::Named(n))
}

/// Write `setting` into a chat-completions body, in the one place the server reads it.
pub fn apply(body: &mut Value, setting: Setting) {
    let Some(o) = body.as_object_mut() else { return };
    let template = |o: &mut serde_json::Map<String, Value>| {
        if let Some(Value::Object(k)) = o.get_mut("chat_template_kwargs") {
            k.remove("reasoning_effort");
            if k.is_empty() {
                o.remove("chat_template_kwargs");
            }
        }
    };
    match setting {
        Setting::Named(n) => {
            template(o);
            o.insert("reasoning_effort".into(), json!(n));
        }
        Setting::Number(v, true) => {
            o.remove("reasoning_effort");
            let k = o.entry("chat_template_kwargs").or_insert_with(|| json!({}));
            if let Some(k) = k.as_object_mut() {
                k.insert("reasoning_effort".into(), json!(v));
            }
        }
        Setting::Number(v, false) => {
            template(o);
            o.insert("reasoning_effort".into(), json!(v));
        }
    }
}

/// The level a body sends, wherever it is: for telling whether a change changed anything.
pub fn sent(body: &Value) -> Option<Value> {
    body.get("reasoning_effort").or_else(|| body.pointer("/chat_template_kwargs/reasoning_effort")).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEEPSEEK: &str = r#"{"detail":"litellm.BadRequestError: Hosted_vllmException - {\"error\":{\"message\":\"DeepSeek V4.1 reasoning_effort must be low, high, xhigh, max, or an integer within [1, 100] in chat_template_kwargs\",\"type\":\"BadRequestError\"}}"}"#;
    const VLLM: &str = r#"1 validation error:\n  {'type': 'literal_error', 'loc': 'body.reasoning_effort', 'msg': "Input should be 'none', 'minimal', 'low', 'medium', 'high' or 'xhigh'", 'input': 'bogus'}"#;
    const OPENAI: &str = "Unsupported value: 'reasoning_effort' does not support 'minimal' with this model. Supported values are: 'low', 'medium', and 'high'.";

    #[test]
    fn a_refusal_says_what_the_server_takes() {
        assert_eq!(
            from_refusal(DEEPSEEK),
            Some(Accepted {
                named: vec!["low", "high", "xhigh", "max"],
                scale: Some(Scale { lo: 1, hi: 100, in_template: true })
            })
        );
        assert_eq!(
            from_refusal(VLLM).unwrap().named,
            ["none", "minimal", "low", "medium", "high", "xhigh"],
            "xhigh is not also read as high"
        );
        assert_eq!(from_refusal(OPENAI).unwrap().named, ["low", "medium", "high"], "not the refused 'minimal'");
        assert_eq!(from_refusal("max_tokens is too large: 900000"), None, "a refusal about something else");
        assert_eq!(from_refusal("reasoning_effort is not supported for this model"), None, "names no level");
    }

    #[test]
    fn the_closest_level_the_server_takes() {
        let deepseek = from_refusal(DEEPSEEK).unwrap();
        assert_eq!(pick("high", &deepseek), Some(Setting::Named("high")));
        assert_eq!(pick("xhigh", &deepseek), Some(Setting::Named("xhigh")));
        assert_eq!(pick("medium", &deepseek), Some(Setting::Number(51, true)), "the middle of 1..100");
        let named = Accepted { named: vec!["low", "high"], scale: None };
        assert_eq!(pick("medium", &named), Some(Setting::Named("low")), "at or below what was asked");
        assert_eq!(pick("max", &named), Some(Setting::Named("high")));
        let above = Accepted { named: vec!["none", "medium", "high"], scale: None };
        assert_eq!(pick("low", &above), Some(Setting::Named("medium")), "never none: the lowest above instead");
        assert_eq!(pick("bogus", &named), None);
    }

    #[test]
    fn a_setting_goes_where_the_server_reads_it() {
        let mut b = json!({"model": "m", "reasoning_effort": "medium"});
        apply(&mut b, Setting::Number(51, true));
        assert_eq!(b, json!({"model": "m", "chat_template_kwargs": {"reasoning_effort": 51}}));
        assert_eq!(sent(&b), Some(json!(51)));
        apply(&mut b, Setting::Named("high"));
        assert_eq!(b, json!({"model": "m", "reasoning_effort": "high"}), "nothing left in the template kwargs");
    }
}
