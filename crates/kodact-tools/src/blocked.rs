//! URLs no tool fetches: `blockedUrls` in settings, or `KODACT_BLOCKED_URLS`.
//!
//! A block, not a permission: it holds in every permission mode (`--dangerously-skip-permissions`
//! included) and for every way KodAct reaches the network for the model: WebFetch and the
//! redirects it follows, the URLs written in a shell command, and the links WebFetch and WebSearch
//! hand back, which are masked. Someone keeps the agent off a site or a path with it; an eval run
//! keeps the agent off the benchmark's own tests and solution (the Harbor kit sets it for
//! Terminal-Bench, after a run fetched a task's hidden tests from a public copy of the benchmark).
//!
//! It sees the URLs a call names, not those a program builds while it runs: a script that pieces a
//! URL together still reaches it. The Harbor report flags what gets through.

use std::sync::OnceLock;

use globset::{GlobBuilder, GlobMatcher};
use kodact_permissions::Subject;
use regex::Regex;

/// What stands in for a blocked link in a page or in search results.
pub const MASK: &str = "[blocked URL]";

#[derive(Clone, Debug, Default)]
pub struct BlockedUrls {
    patterns: Vec<(String, GlobMatcher)>,
}

impl BlockedUrls {
    /// Each entry is a glob over the whole URL, without regard to case, where `*` matches
    /// anything, `/` included. An entry without `*` matches every URL that contains it
    /// (`terminal-bench` is `*terminal-bench*`). An entry that isn't a valid glob is returned
    /// with why, and left out.
    pub fn new<S: AsRef<str>>(entries: &[S]) -> (Self, Vec<String>) {
        let mut patterns = vec![];
        let mut invalid = vec![];
        for e in entries {
            let e = e.as_ref().trim();
            if e.is_empty() {
                continue;
            }
            let glob = if e.contains('*') { e.to_string() } else { format!("*{e}*") };
            match GlobBuilder::new(&glob).case_insensitive(true).literal_separator(false).build() {
                Ok(g) => patterns.push((e.to_string(), g.compile_matcher())),
                Err(err) => invalid.push(format!("{e:?}: {err}")),
            }
        }
        (BlockedUrls { patterns }, invalid)
    }

    /// From a list written as text: entries separated by commas or new lines.
    pub fn parse_list(text: &str) -> Vec<String> {
        text.split([',', '\n']).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    pub fn entries(&self) -> Vec<&str> {
        self.patterns.iter().map(|(e, _)| e.as_str()).collect()
    }

    /// The entry `url` matches, if any.
    pub fn matching(&self, url: &str) -> Option<&str> {
        self.patterns.iter().find(|(_, g)| g.is_match(url)).map(|(e, _)| e.as_str())
    }

    /// The first URL in `text` that an entry matches, with that entry.
    pub fn first_in(&self, text: &str) -> Option<(String, &str)> {
        if self.is_empty() {
            return None;
        }
        urls_in(text).into_iter().find_map(|u| self.matching(u).map(|e| (u.to_string(), e)))
    }

    /// What a call would reach that is blocked: the URL WebFetch would fetch, or a URL written
    /// in a command (Bash, PowerShell). `None` for anything else, and when nothing is blocked.
    pub fn blocks(&self, subject: &Subject) -> Option<(String, &str)> {
        if self.is_empty() {
            return None;
        }
        match subject {
            Subject::Url(u) => self.matching(u).map(|e| (u.clone(), e)),
            Subject::Command(c) => self.first_in(c),
            _ => None,
        }
    }

    /// The tool result for a call that was blocked.
    pub fn refusal(url: &str, entry: &str) -> String {
        format!(
            "Blocked: {url} matches \"{entry}\" in this session's blocked URLs (`blockedUrls`), so KodAct does \
             not fetch it, with this tool or any other. Go on without it."
        )
    }

    /// `text` with every blocked URL in it replaced by [`MASK`].
    pub fn mask(&self, text: &str) -> String {
        if self.is_empty() {
            return text.to_string();
        }
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for (start, end) in url_spans(text) {
            if self.matching(&text[start..end]).is_some() {
                out.push_str(&text[last..start]);
                out.push_str(MASK);
                last = end;
            }
        }
        out.push_str(&text[last..]);
        out
    }
}

/// The URLs written in `text`: `scheme://…`; `host.tld/path` without a scheme, as curl, wget and
/// pip take them; and `user@host:path`, as git and scp take them.
pub fn urls_in(text: &str) -> Vec<&str> {
    url_spans(text).into_iter().map(|(s, e)| &text[s..e]).collect()
}

fn url_spans(text: &str) -> Vec<(usize, usize)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    // A token ends at white space, a quote, a shell operator or a closing bracket; a scheme-less
    // host starts a word (not inside a scheme URL already found, nor a path such as `a/b.c/d`).
    let re = RE.get_or_init(|| {
        Regex::new(concat!(
            r#"(?i)[a-z][a-z0-9+.\-]*://[^\s'"<>|;&()`\\\]\[{}]+"#,
            r#"|(?:^|[\s'"=(`])((?:[a-z0-9][\w\-]*@)?(?:[a-z0-9](?:[a-z0-9\-]*[a-z0-9])?\.)+[a-z]{2,}(?::\d+)?[/:][^\s'"<>|;&()`\\\]\[{}]*)"#,
        ))
        .expect("URL pattern")
    });
    re.captures_iter(text)
        .filter_map(|c| {
            let m = c.get(1).or_else(|| c.get(0))?;
            let s = m.as_str().trim_end_matches(['.', ',', ':']);
            // `host:` alone (a ratio, a time, `key:value`) isn't an address.
            (s.contains('/') || s.contains("://")).then(|| (m.start(), m.start() + s.len()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bench() -> BlockedUrls {
        BlockedUrls::new(&["terminal-bench", "tbench.ai", "*/torch-pipeline-parallelism*/tests/*"]).0
    }

    #[test]
    fn urls_in_commands_and_scripts_are_found() {
        let cmd = "curl -s 'https://grep.app/api/search?q=x' | head; python3 - <<'EOF'\n\
                   u=\"https://raw.githubusercontent.com/AI-Harness-Evals/terminal-bench-2-1-native/main/t/tests/test.sh\"\n\
                   EOF\ngit clone git@github.com:laude-institute/terminal-bench.git; wget tbench.ai/leaderboard";
        let urls = urls_in(cmd);
        assert!(urls.contains(&"https://grep.app/api/search?q=x"), "{urls:?}");
        assert!(urls.iter().any(|u| u.ends_with("/tests/test.sh")), "{urls:?}");
        assert!(urls.contains(&"git@github.com:laude-institute/terminal-bench.git"), "{urls:?}");
        assert!(urls.contains(&"tbench.ai/leaderboard"), "{urls:?}");
        // A path or a ratio isn't an address.
        assert!(urls_in("ls step7_pipeline/pipeline_parallel.py src/a.rs 3:4 key:value").is_empty());
    }

    #[test]
    fn a_blocked_url_is_refused_by_any_tool_but_the_word_alone_is_not() {
        let b = bench();
        let fetch =
            Subject::Url("https://api.github.com/repos/AI-Harness-Evals/terminal-bench-2-1-native/contents".into());
        assert_eq!(b.blocks(&fetch).map(|(_, e)| e), Some("terminal-bench"));
        let py = Subject::Command("python3 -c \"import urllib.request as u; u.urlopen('https://TBench.ai/x')\"".into());
        assert_eq!(b.blocks(&py).map(|(_, e)| e), Some("tbench.ai"), "case doesn't matter");
        let mirror = Subject::Command(
            "curl https://gitlab.com/x/tasks/-/raw/main/torch-pipeline-parallelism/tests/test_outputs.py".into(),
        );
        assert!(b.blocks(&mirror).is_some(), "a glob over the path");
        // The name in a query, a file or an argument is not a URL: mteb-retrieve searches for "terminal-bench".
        for ok in [
            "python retrieve.py --query terminal-bench",
            "echo 'PRODID:-//Terminal-Bench//EN' > a.ics",
            "pip install --index-url https://pypi.org/simple torch",
            "curl https://github.com/huggingface/picotron_tutorial/raw/master/step7/pipeline_parallel.py",
        ] {
            assert!(b.blocks(&Subject::Command(ok.into())).is_none(), "{ok}");
        }
        assert!(BlockedUrls::default().blocks(&fetch).is_none(), "nothing blocked by default");
    }

    #[test]
    fn blocked_links_are_masked_in_pages() {
        let page = "1. [copy](https://github.com/AI-Harness-Evals/terminal-bench-2-1-native) and [tutorial](https://github.com/huggingface/picotron_tutorial)";
        let m = bench().mask(page);
        assert!(!m.contains("AI-Harness-Evals") && m.contains(MASK) && m.contains("picotron_tutorial"), "{m}");
    }

    #[test]
    fn lists_parse_from_text_and_bad_globs_are_reported() {
        assert_eq!(BlockedUrls::parse_list(" a.com, b.org/x\n\n*c*"), vec!["a.com", "b.org/x", "*c*"]);
        let (b, invalid) = BlockedUrls::new(&["good.com", "bad[.com*"]);
        assert_eq!(b.entries(), vec!["good.com"]);
        assert_eq!(invalid.len(), 1, "{invalid:?}");
    }
}
