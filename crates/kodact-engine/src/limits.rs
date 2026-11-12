//! Numeric limits a request states, and how a command's output measures up to them (GOALS
//! pillar 3).
//!
//! A request that states limits ("cost at most 1.5e7", a table of thresholds per output file,
//! "at most 6 distinct slot sizes across both plans") gives the model numbers to hold against
//! every measurement it prints. Several rows of a sweep look alike, and four thresholds are a
//! lot to compare by eye after many steps. When a command prints `key=value` numbers whose keys
//! name a stated limit, the result says, line by line, which limits are met and which are not,
//! by how much. A limit stated across all files (a union, a total) is said apart: per-file
//! counts don't show it.
//!
//! Everything comes from the request's own words and the command's own output: no metric name,
//! threshold or file is known in advance. A request without limits, or output without
//! matching keys, adds nothing.

/// A limit on one quantity: at most (`upper`) or at least `bound`, for one row of a table (a
/// file, a bucket) or for every measurement (`scope` is `None`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Limit {
    pub name: String,
    pub bound: f64,
    pub upper: bool,
    pub scope: Option<String>,
}

/// A limit across everything the request asks for together (a union, a total).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Global {
    pub name: String,
    pub bound: f64,
    pub said: String,
}

/// The limits a request states.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Limits {
    pub per: Vec<Limit>,
    pub global: Vec<Global>,
}

/// Words before a number that make it an upper or a lower bound.
const UPPER: [&str; 11] =
    ["at most", "no more than", "not more than", "less than", "under", "below", "max", "maximum", "up to", "<=", "≤"];
const LOWER: [&str; 6] = ["at least", "no less than", "more than", "above", ">=", "≥"];
/// Words that make a limit one over everything together.
const ACROSS: [&str; 8] = ["across", "combined", "in total", "total", "overall", "altogether", "in all", "union"];
/// Words in a quantity's name that don't name it.
const FILLER: [&str; 9] = ["unique", "distinct", "different", "number", "of", "the", "a", "count", "total"];

impl Limits {
    pub fn from_request(text: &str) -> Limits {
        let mut l = Limits::default();
        let lines: Vec<&str> = text.lines().collect();
        let mut i = 0;
        let mut intro = "";
        while i < lines.len() {
            let line = lines[i].trim();
            if line.starts_with('|') {
                let start = i;
                while i < lines.len() && lines[i].trim().starts_with('|') {
                    i += 1;
                }
                l.table(&lines[start..i], intro);
                continue;
            }
            if !line.is_empty() {
                intro = lines[i];
                l.inline(line);
            }
            i += 1;
        }
        l
    }

    pub fn is_empty(&self) -> bool {
        self.per.is_empty() && self.global.is_empty()
    }

    /// A table of limits: a header of quantity names (its first column names the rows: files,
    /// buckets), and cells that are bounds. A cell is a bound when it says so ("≤ 3e11", "at
    /// most 0.5"), or when the line that introduces the table says its numbers are bounds.
    fn table(&mut self, rows: &[&str], intro: &str) {
        let cells = |r: &str| -> Vec<String> {
            r.trim().trim_matches('|').split('|').map(|c| c.trim().replace('`', "")).collect()
        };
        let Some(header) = rows.first().map(|r| cells(r)) else { return };
        let intro = intro.to_lowercase();
        let intro_bound = match (bound_word(&intro, true), bound_word(&intro, false)) {
            (Some(_), _) => Some(true),
            (None, Some(_)) => Some(false),
            (None, None) => None,
        };
        for row in rows.iter().skip(1) {
            let c = cells(row);
            if c.iter().all(|x| x.chars().all(|ch| matches!(ch, '-' | ':' | ' '))) {
                continue;
            }
            let scope = c.first().filter(|s| !s.is_empty()).cloned();
            for (name, cell) in header.iter().zip(&c).skip(1) {
                let lower_cell = cell.to_lowercase();
                let upper = match (bound_word(&lower_cell, true), bound_word(&lower_cell, false)) {
                    (Some(_), _) => true,
                    (None, Some(_)) => false,
                    (None, None) => match intro_bound {
                        Some(up) => up,
                        None => continue,
                    },
                };
                if let Some(bound) = first_number(cell) {
                    self.per.push(Limit { name: clean_name(name), bound, upper, scope: scope.clone() });
                }
            }
        }
    }

    /// A limit in a sentence: "pad_ratio ≤ 0.055", "cost must be at most 1.5e7", "at most 6
    /// distinct slot sizes across both plans".
    fn inline(&mut self, line: &str) {
        for sentence in sentences(line).into_iter().filter(|s| s.chars().any(|c| c.is_ascii_digit())) {
            let lower = sentence.to_lowercase();
            let across = ACROSS.iter().any(|w| contains_word(&lower, w));
            let words: Vec<&str> = lower.split_whitespace().collect();
            for (i, w) in words.iter().enumerate() {
                let Some(n) = number_in(w) else { continue };
                // The bound: the words just before the number, or a sign stuck to it (`≤0.05`).
                let before = format!("{} {w}", words[i.saturating_sub(3)..i].join(" "));
                let before =
                    before.trim_end_matches(|c: char| c.is_ascii_digit() || matches!(c, '.' | 'e' | '+' | '-'));
                let upper = match (bound_word(before, true), bound_word(before, false)) {
                    (Some(_), _) => true,
                    (None, Some(_)) => false,
                    (None, None) => continue,
                };
                // Where the bound starts: back over its words and over verbs ("must be").
                let mut bound_at = i;
                while bound_at > 0 && is_bound_token(words[bound_at - 1]) {
                    bound_at -= 1;
                }
                let mut start = bound_at;
                while start > 0 && is_verb(words[start - 1]) {
                    start -= 1;
                }
                let sign_alone = w.starts_with(['<', '>', '≤', '≥']);
                let sign =
                    sign_alone || (start < i && words[start..i].iter().any(|t| t.starts_with(['<', '>', '≤', '≥'])));
                // The quantity: the word before a sign ("latency_ms ≤ 250"), before "must be"
                // ("cost must be at most 6") or before a clause that ends at the number ("cost at
                // most 100, wait…"); else the words after the number ("6 distinct slot sizes").
                let after: Vec<&str> = words[i + 1..]
                    .iter()
                    .take(4)
                    .copied()
                    .take_while(|x| x.chars().next().is_some_and(char::is_alphabetic) && !is_stop(x))
                    .collect();
                let lead = if start > 0 { words[start - 1] } else { "" };
                let clause_ends = w.ends_with([',', ';', ':']);
                let name = if sign || start < bound_at || clause_ends || after.is_empty() || is_unit(after[0]) {
                    lead.to_string()
                } else {
                    after.join(" ")
                };
                let name = clean_name(&name);
                if name.is_empty() || name.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                if across && upper {
                    self.global.push(Global { name, bound: n, said: sentence.trim().to_string() });
                } else {
                    self.per.push(Limit { name, bound: n, upper, scope: None });
                }
            }
        }
    }

    /// Does a printed key name this limit's quantity? 2: the whole name, 1: a word of it or a
    /// prefix of its first word (`seq` for "sequential timecost", `pad` for "pad ratio").
    fn matches(key: &str, name: &str) -> u8 {
        let key = key.to_lowercase();
        let joined = name.replace(' ', "_");
        if key == joined || key == name.replace(' ', "") {
            return 2;
        }
        let words: Vec<&str> = name.split(' ').filter(|w| !FILLER.contains(w)).collect();
        if key.len() >= 2
            && words
                .iter()
                .enumerate()
                .any(|(i, w)| *w == key || (i == 0 && w.starts_with(&key)) || (w.len() >= 3 && key.starts_with(*w)))
        {
            return 1;
        }
        0
    }
}

/// A line of output that measures something: two or more `key=value` (or `key: value`) numbers.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Measured {
    pub line: String,
    /// The heading above it in the output (`=== bucket 1`), if any.
    pub section: String,
    pub values: Vec<(String, f64)>,
}

pub(crate) fn measurements(output: &str) -> Vec<Measured> {
    let mut out = vec![];
    let mut section = String::new();
    for line in output.lines() {
        let values = pairs(line);
        if values.len() >= 2 {
            out.push(Measured { line: line.trim().to_string(), section: section.clone(), values });
        } else if values.is_empty() && line.chars().any(char::is_alphanumeric) {
            section = line.trim().to_string();
        }
    }
    out
}

fn pairs(line: &str) -> Vec<(String, f64)> {
    let mut out = vec![];
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_alphabetic() && (i == 0 || !chars[i - 1].is_ascii_alphanumeric()) {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let key: String = chars[start..i].iter().collect();
            let mut j = i;
            if j < chars.len() && (chars[j] == '=' || chars[j] == ':') {
                j += 1;
                while j < chars.len() && chars[j] == ' ' {
                    j += 1;
                }
                let rest: String = chars[j..].iter().collect();
                let num: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E'))
                    .collect();
                if let Ok(v) = num.trim_end_matches(['.', 'e', 'E', '-', '+']).parse::<f64>() {
                    if num.chars().any(|c| c.is_ascii_digit()) {
                        out.push((key, v));
                        i = j + num.len();
                        continue;
                    }
                }
            }
        } else {
            i += 1;
        }
    }
    out
}

/// One measured line held against the limits that apply to it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Verdict {
    pub line: String,
    pub scope: Option<String>,
    /// (key, value, bound, upper, met)
    pub checks: Vec<(String, f64, f64, bool, bool)>,
    /// Every limit stated for this scope was measured on the line.
    pub complete: bool,
}

impl Verdict {
    pub fn all_met(&self) -> bool {
        self.complete && self.checks.iter().all(|c| c.4)
    }
}

impl Limits {
    /// The limits a line answers to: its scope (the row whose distinguishing words are in the
    /// command, the section heading or the line) and the unscoped ones.
    pub fn judge(&self, m: &Measured, command: &str) -> Option<Verdict> {
        let context = tokens(&format!("{command} {} {}", m.section, m.line));
        let scopes: Vec<&String> = {
            let mut v: Vec<&String> = vec![];
            for l in &self.per {
                if let Some(s) = &l.scope {
                    if !v.contains(&s) {
                        v.push(s);
                    }
                }
            }
            v
        };
        let scope = match scopes.len() {
            0 => None,
            _ => {
                let all: Vec<Vec<String>> = scopes.iter().map(|s| tokens(s)).collect();
                let fits: Vec<&String> = scopes
                    .iter()
                    .zip(&all)
                    .filter(|(_, mine)| {
                        let own: Vec<&String> =
                            mine.iter().filter(|t| !all.iter().all(|other| other.contains(t))).collect();
                        !own.is_empty() && own.iter().all(|t| context.contains(t))
                    })
                    .map(|(s, _)| *s)
                    .collect();
                match fits.as_slice() {
                    [one] => Some((*one).clone()),
                    _ => None,
                }
            }
        };
        let applies: Vec<&Limit> = self
            .per
            .iter()
            .filter(|l| l.scope.is_none() || (scope.is_some() && l.scope.as_ref() == scope.as_ref()))
            .collect();
        // Complete: every limit of the line's row was measured (every unscoped one, without a row).
        let needed = applies.iter().filter(|l| l.scope == scope).count();
        let mut checks = vec![];
        let mut covered = 0;
        for l in &applies {
            let best = m
                .values
                .iter()
                .map(|(k, v)| (Limits::matches(k, &l.name), k, v))
                .filter(|(s, ..)| *s > 0)
                .max_by_key(|(s, ..)| *s);
            if let Some((_, k, v)) = best {
                covered += usize::from(l.scope == scope);
                let met = if l.upper { *v <= l.bound } else { *v >= l.bound };
                checks.push((k.clone(), *v, l.bound, l.upper, met));
            }
        }
        if checks.is_empty() {
            return None;
        }
        Some(Verdict { line: m.line.clone(), scope, checks, complete: needed > 0 && covered == needed })
    }

    /// Per-file counts of a quantity limited across all files together: they don't show the
    /// combined number. `(limit, counts)` for each such limit measured on more than one line.
    pub fn global_counts(&self, measured: &[(Option<String>, &Measured)]) -> Vec<(&Global, Vec<f64>)> {
        let mut out = vec![];
        for g in &self.global {
            let mut by_scope: Vec<(Option<String>, f64)> = vec![];
            for (scope, m) in measured {
                if let Some((_, v)) = m.values.iter().find(|(k, _)| Limits::matches(k, &g.name) > 0) {
                    if !by_scope.iter().any(|(s, _)| s == scope) {
                        by_scope.push((scope.clone(), *v));
                    }
                }
            }
            let counts: Vec<f64> = by_scope.into_iter().map(|(_, v)| v).collect();
            if counts.len() >= 2 && counts.iter().sum::<f64>() > g.bound {
                out.push((g, counts));
            }
        }
        out
    }
}

/// What a result is told: each judged line, with its scope, and what across-file limits its
/// per-file counts can't show. `MAX_LINES` lines at most.
pub(crate) fn text(verdicts: &[Verdict], globals: &[(&Global, Vec<f64>)]) -> String {
    const MAX_LINES: usize = 12;
    let mut lines = vec!["Against the limits the request states:".to_string()];
    for v in verdicts.iter().take(MAX_LINES) {
        let label: String = if v.line.chars().count() > 60 {
            v.line.chars().take(57).collect::<String>() + "..."
        } else {
            v.line.clone()
        };
        let scope = v.scope.as_deref().map(|s| format!(" ({s})")).unwrap_or_default();
        let broken: Vec<String> = v
            .checks
            .iter()
            .filter(|c| !c.4)
            .map(|(k, val, bound, up, _)| format!("{k} {} {} {}", num(*val), if *up { ">" } else { "<" }, num(*bound)))
            .collect();
        let state = if v.all_met() {
            format!("all {} limits met", v.checks.len())
        } else if broken.is_empty() {
            format!("{} measured limits met; not every limit stated for it is printed", v.checks.len())
        } else {
            format!("{} of {} not met: {}", broken.len(), v.checks.len(), broken.join(", "))
        };
        lines.push(format!("- `{label}`{scope}: {state}"));
    }
    if verdicts.len() > MAX_LINES {
        let met = verdicts[MAX_LINES..].iter().filter(|v| v.all_met()).count();
        lines.push(format!("- … {} more lines, {met} of them meeting every limit", verdicts.len() - MAX_LINES));
    }
    for (g, counts) in globals {
        lines.push(format!(
            "- {}: at most {} across all files together (\"{}\"). These counts are per file ({}); the combined \
             number can be up to {}: count the distinct values across all the files.",
            g.name,
            num(g.bound),
            g.said,
            counts.iter().map(|c| num(*c)).collect::<Vec<_>>().join(", "),
            num(counts.iter().sum())
        ));
    }
    format!("\n\n<system-reminder>\n{}\n</system-reminder>", lines.join("\n"))
}

fn num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e6 {
        format!("{v:.0}")
    } else if v.abs() >= 1e4 || (v != 0.0 && v.abs() < 1e-2) {
        format!("{v:.4e}")
    } else {
        format!("{v:.4}")
    }
}

fn bound_word(text: &str, upper: bool) -> Option<&'static str> {
    let words: &[&'static str] = if upper { &UPPER } else { &LOWER };
    words
        .iter()
        .copied()
        .find(|w| {
            if w.chars().all(char::is_alphabetic) || w.contains(' ') {
                contains_word(text, w)
            } else {
                text.contains(w)
            }
        })
        // "listed below", "shown below": a place in the text, not a bound.
        .filter(|w| !(*w == "below" && is_place_below(text)))
        .or_else(|| (upper && text.contains('<') && !text.contains("<<")).then_some("<"))
}

fn is_place_below(text: &str) -> bool {
    ["listed below", "shown below", "given below", "see below", "below:", "as below", "described below"]
        .iter()
        .any(|p| text.contains(p))
        && !["below the", "below each", "below these", "below their"].iter().any(|p| text.contains(p))
}

fn contains_word(text: &str, w: &str) -> bool {
    text.match_indices(w).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + w.len()..].chars().next();
        before.is_none_or(|c| !c.is_alphanumeric()) && after.is_none_or(|c| !c.is_alphanumeric())
    })
}

/// The first number in a cell: `≤ 3.0e11`, `at most 1.55e7`, `2.1e+06`, `0.055`.
fn first_number(s: &str) -> Option<f64> {
    s.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '=' | '≤' | '≥' | '(' | ')' | ','))
        .find_map(number_in)
}

fn number_in(w: &str) -> Option<f64> {
    let w = w.trim_start_matches(['<', '>', '=', '≤', '≥', '~']).trim_end_matches([',', ';', ':', ')', '.']);
    let w = w.replace(',', "");
    if !w.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    let num: String =
        w.chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-')).collect();
    let v: f64 = num.parse().ok()?;
    // `40kb`, `5000 bytes` belong to the size check; `20%` is a share.
    let rest = &w[num.len()..];
    if !rest.is_empty() && !rest.chars().all(|c| c == '%') {
        return None;
    }
    Some(if rest == "%" { v / 100.0 } else { v })
}

/// Sentences of a line: split at `.`, `;`, `!` or `?` followed by a space or the end, so that
/// `0.01` stays whole.
fn sentences(line: &str) -> Vec<&str> {
    let mut out = vec![];
    let mut from = 0;
    let b = line.as_bytes();
    for (i, c) in line.char_indices() {
        if matches!(c, '.' | ';' | '!' | '?') && b.get(i + 1).is_none_or(|n| n.is_ascii_whitespace()) {
            out.push(&line[from..i]);
            from = i + 1;
        }
    }
    out.push(&line[from..]);
    out
}

fn is_bound_token(w: &str) -> bool {
    matches!(
        w,
        "at" | "most"
            | "least"
            | "no"
            | "not"
            | "more"
            | "less"
            | "than"
            | "under"
            | "below"
            | "above"
            | "max"
            | "maximum"
            | "up"
            | "<"
            | "<="
            | "≤"
            | ">"
            | ">="
            | "≥"
            | "~"
    )
}

/// Words that end a quantity's name after its number: "6 slot sizes across both", "5 per file".
fn is_stop(w: &str) -> bool {
    ACROSS.contains(&w)
        || matches!(w, "and" | "or" | "per" | "in" | "for" | "each" | "on" | "at" | "with" | "from" | "both" | "all")
}

fn is_unit(w: &str) -> bool {
    matches!(w, "ms" | "s" | "seconds" | "minutes" | "bytes" | "kb" | "mb" | "gb" | "tokens" | "%" | "times" | "x")
}

fn is_verb(w: &str) -> bool {
    matches!(w, "must" | "should" | "be" | "is" | "are" | "stay" | "remain" | "keep" | "with" | "of" | "to" | "=" | ":")
}

/// A quantity's name as the request writes it, lower case, without units in brackets or
/// trailing punctuation: "P95 Latency (ms)" → "p95 latency".
fn clean_name(name: &str) -> String {
    let no_units = match name.find('(') {
        Some(i) => &name[..i],
        None => name,
    };
    no_units
        .to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .skip_while(|w| FILLER.contains(w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Words of a text, lower case, split at anything that isn't a letter or a digit, with file
/// extensions kept as words: `requests_bucket_1.jsonl` → requests, bucket, 1, jsonl.
fn tokens(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUEST: &str = "Pack the orders into two route files.\n\
        Today's plans score like this, far from good enough:\n\n\
        | File | Cost | Late Ratio | P99 Wait (s) |\n| --- | --- | --- | --- |\n\
        | `routes_north.csv` | 9.1e9 | 0.71 | 4.2e5 |\n| `routes_south.csv` | 8.4e9 | 0.65 | 3.9e5 |\n\n\
        Bring each file below these thresholds:\n\n\
        | File | Cost | Late Ratio | P99 Wait (s) |\n| --- | --- | --- | --- |\n\
        | `routes_north.csv` | 1.2e9 | 0.05 | 6.0e4 |\n| `routes_south.csv` | 9.0e8 | 0.08 | 4.5e4 |\n\n\
        Use at most 5 distinct vehicle types across both files combined. Each route must have a margin of at least 0.2.";

    #[test]
    fn limits_come_from_the_requests_own_words() {
        let l = Limits::from_request(REQUEST);
        // The table introduced as "far from good enough" holds scores, not bounds.
        assert_eq!(l.per.iter().filter(|x| x.scope.is_some()).count(), 6, "{l:#?}");
        let north: Vec<(&str, f64)> = l
            .per
            .iter()
            .filter(|x| x.scope.as_deref() == Some("routes_north.csv"))
            .map(|x| (x.name.as_str(), x.bound))
            .collect();
        assert_eq!(north, [("cost", 1.2e9), ("late ratio", 0.05), ("p99 wait", 6.0e4)]);
        assert!(l.per.iter().all(|x| x.scope.is_none() || x.upper));
        assert_eq!(l.global.len(), 1);
        assert_eq!((l.global[0].name.as_str(), l.global[0].bound), ("vehicle types", 5.0));
        let margin = l.per.iter().find(|x| x.scope.is_none()).unwrap();
        assert_eq!((margin.name.as_str(), margin.bound, margin.upper), ("margin", 0.2, false));
    }

    #[test]
    fn inline_limits_in_their_usual_forms() {
        let l = Limits::from_request("Keep latency_ms ≤ 250 and error_rate <= 0.01. The score must be at least 0.9.");
        let got: Vec<(&str, f64, bool)> = l.per.iter().map(|x| (x.name.as_str(), x.bound, x.upper)).collect();
        assert_eq!(got, [("latency_ms", 250.0, true), ("error_rate", 0.01, true), ("score", 0.9, false)]);
        assert!(Limits::from_request("Fix the bug in parser.py; see the steps listed below: 1. run it.").is_empty());
        assert!(Limits::from_request("Write 3 tests.").is_empty(), "a count that isn't a bound");
        let l = Limits::from_request("Keep cost at most 100, wait at most 5 and use at most 4 spare vans.");
        let got: Vec<(&str, f64)> = l.per.iter().map(|x| (x.name.as_str(), x.bound)).collect();
        assert_eq!(got, [("cost", 100.0), ("wait", 5.0), ("spare vans", 4.0)]);
    }

    #[test]
    fn a_sweep_is_judged_line_by_line_and_the_row_by_its_own_words() {
        let l = Limits::from_request(REQUEST);
        let out =
            "=== north\n k=1 cost=1.19e9 late=0.049 p99=5.9e4 types=5\n k=2 cost=1.18e9 late=0.048 p99=6.3e4 types=5\n\
                   === south\n k=1 cost=8.7e8 late=0.07 p99=4.4e4 types=5\n";
        let ms = measurements(out);
        assert_eq!(ms.len(), 3);
        let vs: Vec<Verdict> = ms.iter().filter_map(|m| l.judge(m, "python3 sweep.py")).collect();
        assert_eq!(vs[0].scope.as_deref(), Some("routes_north.csv"));
        assert!(vs[0].all_met(), "{:?}", vs[0]);
        assert!(!vs[1].all_met());
        assert_eq!(vs[1].checks.iter().filter(|c| !c.4).map(|c| c.0.as_str()).collect::<Vec<_>>(), ["p99"]);
        assert_eq!(vs[2].scope.as_deref(), Some("routes_south.csv"));
        assert!(vs[2].all_met());
        // Per-file counts of the across-files limit: 5 and 5 may be 10 together.
        let scoped: Vec<(Option<String>, &Measured)> = vs.iter().map(|v| v.scope.clone()).zip(&ms).collect();
        let g = l.global_counts(&scoped);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].1, [5.0, 5.0]);
        let t = text(&vs, &g);
        assert!(
            t.contains("`k=1 cost=1.19e9 late=0.049 p99=5.9e4 types=5` (routes_north.csv): all 3 limits met"),
            "{t}"
        );
        assert!(t.contains("1 of 3 not met: p99 63000 > 60000"), "{t}");
        assert!(t.contains("combined number can be up to 10"), "{t}");
    }

    #[test]
    fn the_command_names_the_row_when_the_output_does_not() {
        let l = Limits::from_request(REQUEST);
        let m = &measurements("cost=8.0e8 late=0.05 p99=4.0e4")[0];
        assert_eq!(l.judge(m, "python3 score.py routes_south.csv").unwrap().scope.as_deref(), Some("routes_south.csv"));
        // Neither the command nor the output says which row: only unscoped limits apply.
        let v = l.judge(m, "python3 score.py");
        assert!(v.is_none(), "{v:?}");
    }

    #[test]
    fn output_without_matching_keys_or_a_request_without_limits_says_nothing() {
        let l = Limits::from_request(REQUEST);
        assert!(measurements("compiling 3 crates\nfinished in 2.1s").is_empty());
        let m = &measurements("loss=0.31 acc=0.88")[0];
        assert!(l.judge(m, "python3 train.py").is_none());
        assert!(Limits::default().judge(m, "x").is_none());
    }
}
