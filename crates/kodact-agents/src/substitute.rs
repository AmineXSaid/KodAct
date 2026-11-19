//! A skill's text filled in before the model reads it, as the reference CLI
//! does, so a skill written for it runs here unchanged:
//!
//! - `$ARGUMENTS` is everything typed after the skill's name; `$ARGUMENTS[N]`
//!   and `$N` the Nth word (from 0, shell-style quoting: `"hello world"` is
//!   one); `$name` the word in the place `arguments:` gives that name.
//! - `${KODACT_SKILL_DIR}`, `${KODACT_PROJECT_DIR}`, `${KODACT_SESSION_ID}`,
//!   `${KODACT_EFFORT}`, `${KODACT_PLUGIN_ROOT}` and `${KODACT_PLUGIN_DATA}`, and
//!   the reference CLI's `${CLAUDE_*}` names for the same things (and the
//!   `${FORGE_*}` names from before KodAct was renamed).
//!
//! Values go in as they are and are never expanded again. `\$` before a
//! placeholder keeps a literal `$`. An index past the last word stays as
//! written (`$2`); a named argument with no word becomes empty. When no
//! placeholder takes the arguments, they are appended as `ARGUMENTS: …`.

use std::path::Path;

/// What a skill's text can refer to, besides its arguments.
#[derive(Debug, Clone, Default)]
pub struct SkillVars<'a> {
    pub skill_dir: Option<&'a Path>,
    pub project_dir: Option<&'a Path>,
    pub session_id: Option<&'a str>,
    pub effort: Option<&'a str>,
    /// The plugin a skill came with: its folder and its name (for its data folder).
    pub plugin: Option<(&'a Path, &'a str)>,
}

/// `args` split as a shell would: spaces separate, quotes group, `\` escapes.
pub fn split_args(args: &str) -> Vec<String> {
    let (mut out, mut cur, mut any, mut quote) = (vec![], String::new(), false, None::<char>);
    let mut chars = args.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), _) if c == q => quote = None,
            (Some('"') | None, '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                    any = true;
                }
            }
            (Some(_), _) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            (None, c) => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

fn var(name: &str, v: &SkillVars) -> Option<String> {
    // KodAct's names, ForgeCLI's from before the rename, and the reference CLI's.
    let name = name
        .strip_prefix("KODACT_")
        .or_else(|| name.strip_prefix("FORGE_"))
        .or_else(|| name.strip_prefix("CLAUDE_"))?;
    let path = |p: &Path| p.display().to_string();
    match name {
        "SKILL_DIR" => v.skill_dir.map(path),
        "PROJECT_DIR" => v.project_dir.map(path),
        "SESSION_ID" => v.session_id.map(str::to_string),
        "EFFORT" => v.effort.map(str::to_string),
        "PLUGIN_ROOT" => v.plugin.map(|(d, _)| path(d)),
        "PLUGIN_DATA" => v.plugin.map(|(_, n)| {
            let d = crate::plugins::data_dir(n);
            let _ = std::fs::create_dir_all(&d);
            path(&d)
        }),
        _ => None,
    }
}

/// `text` with only its `${…}` folder placeholders filled in: for
/// `allowed-tools` rules and hook commands, where `$1` means something else.
pub fn fill_vars(text: &str, vars: &SkillVars) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        match after.find('}').and_then(|e| var(&after[..e], vars).map(|v| (v, e))) {
            Some((v, e)) => {
                out.push_str(&v);
                rest = &after[e + 1..];
            }
            None => {
                out.push_str("${");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn ident_len(s: &str) -> usize {
    s.char_indices().find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '_')).map_or(s.len(), |(i, _)| i)
}

/// Whether `body` has any placeholder that consumes `$ARGUMENTS`, `$N`, or a named argument.
pub fn uses_arguments(body: &str, names: &[String]) -> bool {
    let mut rest = body;
    while let Some(at) = rest.find(['$', '\\']) {
        let tail = &rest[at..];
        if let Some(after) = tail.strip_prefix("\\$") {
            let n = ident_len(after);
            let fills = after.starts_with(|c: char| c.is_ascii_digit())
                || after.starts_with("ARGUMENTS")
                || after.starts_with('{')
                || (n > 0 && names.iter().any(|x| x == &after[..n]));
            if fills {
                rest = after;
                continue;
            }
        }
        if let Some(after) = tail.strip_prefix('\\') {
            rest = after;
            continue;
        }
        let after = &tail[1..];
        if let Some(inner) = after.strip_prefix('{').and_then(|s| s.find('}').map(|e| &s[..e])) {
            if inner
                .strip_prefix("KODACT_")
                .or_else(|| inner.strip_prefix("FORGE_"))
                .or_else(|| inner.strip_prefix("CLAUDE_"))
                .is_some_and(|k| {
                    matches!(k, "SKILL_DIR" | "PROJECT_DIR" | "SESSION_ID" | "EFFORT" | "PLUGIN_ROOT" | "PLUGIN_DATA")
                })
            {
                rest = &after[inner.len() + 2..];
                continue;
            }
        }
        let n = ident_len(after);
        if after.starts_with("ARGUMENTS")
            || after.starts_with(|c: char| c.is_ascii_digit())
            || (n > 0 && names.iter().any(|x| x == &after[..n]))
        {
            return true;
        }
        rest = after;
    }
    false
}

/// `body` with its placeholders filled in (see the module docs).
pub fn fill(body: &str, args: &str, names: &[String], vars: &SkillVars) -> String {
    let args = args.trim();
    let words = split_args(args);
    let (mut out, mut took) = (String::with_capacity(body.len()), false);
    let mut rest = body;
    while let Some(at) = rest.find(['$', '\\']) {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        // `\$` before something we would fill: a literal `$`.
        if let Some(after) = tail.strip_prefix("\\$") {
            let n = ident_len(after);
            let fills = after.starts_with(|c: char| c.is_ascii_digit())
                || after.starts_with("ARGUMENTS")
                || after.starts_with('{')
                || (n > 0 && names.iter().any(|x| x == &after[..n]));
            if fills {
                out.push('$');
                rest = after;
                continue;
            }
        }
        if let Some(after) = tail.strip_prefix('\\') {
            out.push('\\');
            rest = after;
            continue;
        }
        let after = &tail[1..];
        if let Some(inner) = after.strip_prefix('{').and_then(|s| s.find('}').map(|e| &s[..e])) {
            if let Some(v) = var(inner, vars) {
                out.push_str(&v);
                rest = &after[inner.len() + 2..];
                continue;
            }
        } else if let Some(more) = after.strip_prefix("ARGUMENTS") {
            took = true;
            let index = more
                .strip_prefix('[')
                .and_then(|s| s.find(']').map(|e| (&s[..e], e + 2)))
                .and_then(|(n, len)| n.parse::<usize>().ok().map(|n| (n, len)));
            match index {
                Some((n, len)) => {
                    match words.get(n) {
                        Some(w) => out.push_str(w),
                        None => out.push_str(&tail[..1 + "ARGUMENTS".len() + len]),
                    }
                    rest = &more[len..];
                }
                None => {
                    out.push_str(args);
                    rest = more;
                }
            }
            continue;
        } else {
            let digits = after.chars().take_while(char::is_ascii_digit).count();
            if digits > 0 {
                took = true;
                match after[..digits].parse::<usize>().ok().and_then(|n| words.get(n)) {
                    Some(w) => out.push_str(w),
                    None => out.push_str(&tail[..1 + digits]),
                }
                rest = &after[digits..];
                continue;
            }
            let n = ident_len(after);
            if let Some(i) = names.iter().position(|x| n > 0 && x == &after[..n]) {
                took = true;
                out.push_str(words.get(i).map_or("", String::as_str));
                rest = &after[n..];
                continue;
            }
        }
        out.push('$');
        rest = after;
    }
    out.push_str(rest);
    if !took && !args.is_empty() {
        out.push_str(&format!("\n\nARGUMENTS: {args}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_like_a_shell() {
        assert_eq!(split_args(r#"  "hello world" two 'a b' c\ d "" "#), ["hello world", "two", "a b", "c d", ""]);
        assert!(split_args("   ").is_empty());
    }

    #[test]
    fn fills_arguments_as_the_reference_cli_does() {
        let none = SkillVars::default();
        let names = vec!["issue".to_string(), "branch".to_string()];
        let body =
            "Migrate $ARGUMENTS[0] from $1 to $2. All: $ARGUMENTS. Issue $issue on $branch; $3 stays; costs \\$1.00";
        assert_eq!(
            fill(body, r#""Button group" react vue"#, &names, &none),
            "Migrate Button group from react to vue. All: \"Button group\" react vue. Issue Button group on react; $3 stays; costs $1.00"
        );
        // Unused arguments are appended; none, nothing.
        assert_eq!(fill("Review the diff.", "src/main.rs", &[], &none), "Review the diff.\n\nARGUMENTS: src/main.rs");
        assert_eq!(fill("Review the diff.", "  ", &[], &none), "Review the diff.");
        // A missing named argument is empty; values are never expanded again.
        assert_eq!(fill("[$issue][$branch]", "$0", &names, &none), "[$0][]");
        // Plain dollars and backslashes stay.
        assert_eq!(fill("cost $ 5, $HOME, a\\b, \\n", "", &[], &none), "cost $ 5, $HOME, a\\b, \\n");
    }

    #[test]
    fn uses_arguments_detects_placeholders() {
        let names = vec!["issue".to_string(), "branch".to_string()];
        assert!(uses_arguments("Ship $ARGUMENTS.", &names));
        assert!(uses_arguments("Migrate $ARGUMENTS[0]", &names));
        assert!(uses_arguments("Step $0 and $1", &names));
        assert!(uses_arguments("Fix $issue", &names));
        assert!(!uses_arguments("Costs \\$1.00 and \\$ARGUMENTS and \\$issue", &names));
        assert!(!uses_arguments("Use ${CLAUDE_SKILL_DIR} and $HOME and $other", &names));
    }

    #[test]
    fn fills_folders_under_both_names() {
        let v = SkillVars {
            skill_dir: Some(Path::new("/s/pdf")),
            project_dir: Some(Path::new("/p")),
            session_id: Some("abc"),
            effort: None,
            plugin: None,
        };
        assert_eq!(
            fill("${CLAUDE_SKILL_DIR}/x.py ${KODACT_SKILL_DIR} ${CLAUDE_PROJECT_DIR} ${CLAUDE_SESSION_ID} ${CLAUDE_EFFORT} ${OTHER}", "", &[], &v),
            "/s/pdf/x.py /s/pdf /p abc ${CLAUDE_EFFORT} ${OTHER}"
        );
        assert_eq!(fill("\\${CLAUDE_SKILL_DIR}", "", &[], &v), "${CLAUDE_SKILL_DIR}");
        assert_eq!(fill("${FORGE_SKILL_DIR}/x.py", "", &[], &v), "/s/pdf/x.py", "ForgeCLI's names still work");
        assert_eq!(fill_vars("Bash(${CLAUDE_SKILL_DIR}/x.sh $1 ${HOME})", &v), "Bash(/s/pdf/x.sh $1 ${HOME})");
    }
}
