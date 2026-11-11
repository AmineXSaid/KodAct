//! Permission decisions.
//!
//! A tool call is described by a [`Request`] (tool name, what it touches,
//! whether it only reads). [`Engine::decide`] combines the permission mode,
//! the allow / ask / deny rules and the working directories into a
//! [`Decision`]. Order: deny rules, then bypass, then ask rules, then allow
//! rules, then the mode's defaults.

mod mode;
mod paths;
mod rule;
mod shell;
pub mod threat;

pub use mode::PermissionMode;
pub use paths::{is_within, normalize};
pub use rule::{Rule, RuleError, RuleSpec};
pub use shell::split_compound;

use std::path::{Path, PathBuf};

/// What a tool call touches, for matching rule specifiers.
#[derive(Debug, Clone, PartialEq)]
pub enum Subject {
    /// A shell command line (Bash).
    Command(String),
    /// A filesystem path; `write` for tools that modify it.
    Path { path: PathBuf, write: bool },
    /// A URL (WebFetch).
    Url(String),
    /// A named target: the skill for Skill, the agent type for Task.
    Name(String),
    /// Nothing rule-matchable beyond the tool name.
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Request<'a> {
    pub tool: &'a str,
    pub subject: Subject,
    /// The call cannot change anything (Read, Grep, `git status`, ...).
    pub read_only: bool,
    /// The call runs inside the OS sandbox, confined to the working directories.
    pub sandboxed: bool,
}

impl<'a> Request<'a> {
    pub fn new(tool: &'a str, subject: Subject, read_only: bool) -> Self {
        Request { tool, subject, read_only, sandboxed: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Behavior {
    Allow,
    Ask,
    Deny,
}

/// Why a decision was made (surfaced to the model and the host).
#[derive(Debug, Clone, PartialEq)]
pub enum Reason {
    Rule {
        behavior: Behavior,
        rule: String,
    },
    Mode(PermissionMode),
    ReadOnlyInWorkingDir,
    Sandboxed,
    /// A dangerous-command pattern matched (never approved automatically).
    Threat {
        name: String,
        description: String,
    },
    OutsideWorkingDirs(PathBuf),
    PlanMode,
    Default,
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reason::Rule { behavior, rule } => write!(f, "matched {behavior:?} rule {rule}"),
            Reason::Mode(m) => write!(f, "permission mode {}", m.as_str()),
            Reason::ReadOnlyInWorkingDir => write!(f, "read-only access inside the working directories"),
            Reason::Sandboxed => write!(f, "runs inside the sandbox"),
            Reason::Threat { description, .. } => write!(f, "flagged as dangerous: this command {description}"),
            Reason::OutsideWorkingDirs(p) => write!(f, "{} is outside the working directories", p.display()),
            Reason::PlanMode => write!(f, "plan mode allows only read-only tools"),
            Reason::Default => write!(f, "this tool requires permission"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Allow { reason: Reason },
    Ask { reason: Reason, suggestions: Vec<Suggestion> },
    Deny { reason: Reason },
}

impl Decision {
    pub fn behavior(&self) -> Behavior {
        match self {
            Decision::Allow { .. } => Behavior::Allow,
            Decision::Ask { .. } => Behavior::Ask,
            Decision::Deny { .. } => Behavior::Deny,
        }
    }
}

/// One rule in SDK shape: `{"toolName": "Bash", "ruleContent": "git *"}`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleValue {
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_content: Option<String>,
}

impl RuleValue {
    /// Parse `Tool` / `Tool(content)`.
    pub fn parse(s: &str) -> Self {
        match s.find('(') {
            Some(i) if s.ends_with(')') => {
                RuleValue { tool_name: s[..i].to_string(), rule_content: Some(s[i + 1..s.len() - 1].to_string()) }
            }
            _ => RuleValue { tool_name: s.to_string(), rule_content: None },
        }
    }

    pub fn to_rule_string(&self) -> String {
        match &self.rule_content {
            Some(c) => format!("{}({c})", self.tool_name),
            None => self.tool_name.clone(),
        }
    }
}

/// A change the user can accept along with an approval ("always allow"),
/// in the SDK's `PermissionUpdate` shape.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Suggestion {
    AddRules { rules: Vec<RuleValue>, behavior: Behavior, destination: String },
    SetMode { mode: String, destination: String },
    AddDirectories { directories: Vec<String>, destination: String },
}

/// Rules from every source, already merged.
#[derive(Debug, Clone, Default)]
pub struct RuleSet {
    pub allow: Vec<Rule>,
    pub ask: Vec<Rule>,
    pub deny: Vec<Rule>,
}

impl RuleSet {
    /// Parse rule strings; invalid ones are returned separately so callers can warn.
    pub fn from_strings(allow: &[String], ask: &[String], deny: &[String]) -> (Self, Vec<RuleError>) {
        let mut errors = vec![];
        let mut parse = |list: &[String]| -> Vec<Rule> {
            list.iter()
                .flat_map(|s| split_rule_list(s))
                .filter_map(|s| Rule::parse(&s).map_err(|e| errors.push(e)).ok())
                .collect()
        };
        let set = RuleSet { allow: parse(allow), ask: parse(ask), deny: parse(deny) };
        (set, errors)
    }

    pub fn add(&mut self, behavior: Behavior, rule: Rule) {
        let list = match behavior {
            Behavior::Allow => &mut self.allow,
            Behavior::Ask => &mut self.ask,
            Behavior::Deny => &mut self.deny,
        };
        if !list.contains(&rule) {
            list.push(rule);
        }
    }
}

/// Split a `--allowedTools` value: commas or whitespace separate rules, but
/// not inside parentheses (`Bash(git commit *)` stays whole).
pub fn split_rule_list(raw: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut depth = 0i32;
    for c in raw.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c)
            }
            ')' => {
                depth -= 1;
                cur.push(c)
            }
            ',' | ' ' | '\t' | '\n' if depth <= 0 => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Tools that edit files: `Edit(...)` rules cover all of them.
pub const EDIT_TOOLS: &[&str] = &["Edit", "MultiEdit", "Write", "NotebookEdit"];
/// Tools that read files: `Read(...)` rules cover all of them.
pub const READ_TOOLS: &[&str] = &["Read", "Glob", "Grep", "LS", "NotebookRead"];

/// Commands `acceptEdits` mode allows inside the working directories.
/// The shell commands Edit automatically runs unasked inside the working
/// directories: they make, copy, move and edit files. Deleting is not editing,
/// so `rm` and `rmdir` still ask, as in KodAct for VS Code ("a green pass to
/// edit files, not to delete them").
const ACCEPT_EDITS_COMMANDS: &[&str] = &["mkdir", "touch", "mv", "cp", "sed"];

#[derive(Debug, Clone)]
pub struct Engine {
    pub mode: PermissionMode,
    pub rules: RuleSet,
    pub cwd: PathBuf,
    /// cwd plus `--add-dir` directories.
    pub working_dirs: Vec<PathBuf>,
    /// Extra directories that may be read without a prompt (saved tool output).
    pub read_dirs: Vec<PathBuf>,
    /// Rules added while the session runs (prompt answers, `/permissions add`),
    /// so a rebuilt session (`/clear`, `/reload-*`) keeps them.
    pub added: RuleSet,
    /// Allowed for this turn only: a skill's `allowed-tools`. Cleared when the turn ends.
    pub turn_allow: Vec<Rule>,
    /// Tools taken away for this turn only: a skill's `disallowed-tools`.
    pub turn_removed: Vec<String>,
}

impl Engine {
    pub fn new(mode: PermissionMode, rules: RuleSet, cwd: &Path, extra_dirs: &[PathBuf]) -> Self {
        let cwd = normalize(cwd, Path::new("/"));
        let mut working_dirs = vec![cwd.clone()];
        for d in extra_dirs {
            let d = normalize(d, &cwd);
            if !working_dirs.contains(&d) {
                working_dirs.push(d);
            }
        }
        Engine {
            mode,
            rules,
            cwd,
            working_dirs,
            read_dirs: vec![],
            added: RuleSet::default(),
            turn_allow: vec![],
            turn_removed: vec![],
        }
    }

    /// For the rest of this turn: allow `allow` without asking, and take the
    /// `removed` tools away. Deny rules still win.
    pub fn grant_turn(&mut self, allow: Vec<Rule>, removed: Vec<String>) {
        self.turn_allow.extend(allow);
        for t in removed {
            if !self.turn_removed.contains(&t) {
                self.turn_removed.push(t);
            }
        }
    }

    /// The turn ended: its grants and removals go.
    pub fn clear_turn(&mut self) {
        self.turn_allow.clear();
        self.turn_removed.clear();
    }

    /// Whether `tool` is taken away for this turn.
    pub fn removed_this_turn(&self, tool: &str) -> bool {
        self.turn_removed.iter().any(|t| t == tool)
    }

    fn allow_rules(&self) -> impl Iterator<Item = &Rule> {
        self.rules.allow.iter().chain(self.turn_allow.iter())
    }

    /// Add a rule while the session runs; it is remembered in [`Engine::added`].
    pub fn add_rule(&mut self, behavior: Behavior, rule: Rule) {
        self.added.add(behavior, rule.clone());
        self.rules.add(behavior, rule);
    }

    /// Allow reading (not writing) `dir` without a prompt.
    pub fn add_read_dir(&mut self, dir: &Path) {
        let d = normalize(dir, &self.cwd);
        if !self.read_dirs.contains(&d) {
            self.read_dirs.push(d);
        }
    }

    pub fn add_directory(&mut self, dir: &Path) {
        let d = normalize(dir, &self.cwd);
        if !self.working_dirs.contains(&d) {
            self.working_dirs.push(d);
        }
    }

    /// Is `path` inside a working directory, as written and where it really leads
    /// (a link in the project that points outside it is outside).
    pub fn in_working_dirs(&self, path: &Path) -> bool {
        let p = normalize(path, &self.cwd);
        if !self.working_dirs.iter().any(|d| is_within(&p, d)) {
            return false;
        }
        let real = paths::real(&p);
        real == p || self.working_dirs.iter().any(|d| is_within(&real, d) || is_within(&real, &paths::real(d)))
    }

    /// The first deny or ask rule that covers the request. A path is tried as written and
    /// as where it really leads, so a rule on a folder also covers a link into it.
    fn first_match<'r>(&self, rules: &'r [Rule], req: &Request) -> Option<&'r Rule> {
        if let Some(r) = rules.iter().find(|r| r.matches(req, &self.cwd, MatchMode::Any)) {
            return Some(r);
        }
        let Subject::Path { path, write } = &req.subject else { return None };
        let written = normalize(path, &self.cwd);
        let real = paths::real(&written);
        if real == written {
            return None;
        }
        let through = Request { subject: Subject::Path { path: real, write: *write }, ..req.clone() };
        rules.iter().find(|r| r.matches(&through, &self.cwd, MatchMode::Any))
    }

    /// The allow rule(s) covering `req`, joined for display.
    fn allow_rule_for(&self, req: &Request) -> Option<String> {
        if let Subject::Command(cmd) = &req.subject {
            let parts = split_compound(cmd);
            if parts.is_empty() {
                return None;
            }
            let mut used: Vec<String> = vec![];
            for part in parts {
                let sub = Request {
                    tool: req.tool,
                    subject: Subject::Command(part),
                    read_only: req.read_only,
                    sandboxed: req.sandboxed,
                };
                let r = self.allow_rules().find(|r| r.matches(&sub, &self.cwd, MatchMode::All))?;
                let name = r.to_string();
                if !used.contains(&name) {
                    used.push(name);
                }
            }
            return Some(used.join(", "));
        }
        let rule = self.allow_rules().find(|r| r.matches(req, &self.cwd, MatchMode::All))?;
        // An allow on a path covers where the path really leads, or it does not apply:
        // `Read(./docs/**)` is no leave to read what a link under docs/ points to elsewhere.
        if let Subject::Path { path, write } = &req.subject {
            let written = normalize(path, &self.cwd);
            let real = paths::real(&written);
            if real != written && !self.in_working_dirs(&written) {
                let through = Request { subject: Subject::Path { path: real, write: *write }, ..req.clone() };
                self.allow_rules().find(|r| r.matches(&through, &self.cwd, MatchMode::All))?;
            }
        }
        Some(rule.to_string())
    }

    pub fn decide(&self, req: &Request) -> Decision {
        // 1. Deny rules always win. For compound commands a deny matches if any part does.
        if let Some(r) = self.first_match(&self.rules.deny, req) {
            return Decision::Deny { reason: Reason::Rule { behavior: Behavior::Deny, rule: r.to_string() } };
        }
        // 1b. A tool the running skill took away for this turn.
        if self.removed_this_turn(req.tool) {
            return Decision::Deny {
                reason: Reason::Rule {
                    behavior: Behavior::Deny,
                    rule: format!("{} (the skill's disallowed-tools)", req.tool),
                },
            };
        }
        // 2. Bypass skips prompts, not deny rules.
        if self.mode == PermissionMode::BypassPermissions {
            return Decision::Allow { reason: Reason::Mode(self.mode) };
        }
        // 2b. Dangerous commands always ask (or are denied headless); no "always allow" offered.
        if let Subject::Command(cmd) = &req.subject {
            if let Some(t) = threat::flagged(cmd) {
                let reason = Reason::Threat { name: t.name.into(), description: t.description.into() };
                if self.mode == PermissionMode::DontAsk {
                    return Decision::Deny { reason };
                }
                return Decision::Ask { reason, suggestions: vec![] };
            }
        }
        // 3. Ask rules.
        if let Some(r) = self.first_match(&self.rules.ask, req) {
            return self.ask_or_deny(Reason::Rule { behavior: Behavior::Ask, rule: r.to_string() }, req);
        }
        // 4. Allow rules. For compound commands every part must be allowed,
        //    possibly each by a different rule.
        if let Some(rule) = self.allow_rule_for(req) {
            return Decision::Allow { reason: Reason::Rule { behavior: Behavior::Allow, rule } };
        }
        // 5. Mode defaults.
        let path_outside = match &req.subject {
            Subject::Path { path, .. } if !self.in_working_dirs(path) => Some(normalize(path, &self.cwd)),
            // A command that names a path outside is asked about like a tool that does:
            // `cat ~/.ssh/id_rsa` ran unasked where `Read` of the same file asked.
            Subject::Command(c) => self.command_path_outside(c),
            _ => None,
        };
        if req.read_only {
            return match path_outside {
                None => Decision::Allow { reason: Reason::ReadOnlyInWorkingDir },
                Some(p)
                    if self
                        .read_dirs
                        .iter()
                        .any(|d| is_within(&p, d) && is_within(&paths::real(&p), &paths::real(d))) =>
                {
                    Decision::Allow { reason: Reason::ReadOnlyInWorkingDir }
                }
                Some(p) => self.ask_or_deny(Reason::OutsideWorkingDirs(p), req),
            };
        }
        if self.mode == PermissionMode::Plan {
            return Decision::Deny { reason: Reason::PlanMode };
        }
        // 6. The OS sandbox confines the call to the working directories: no prompt needed.
        if req.sandboxed && path_outside.is_none() {
            return Decision::Allow { reason: Reason::Sandboxed };
        }
        if self.mode == PermissionMode::AcceptEdits && path_outside.is_none() {
            let is_edit = matches!(req.subject, Subject::Path { write: true, .. }) || EDIT_TOOLS.contains(&req.tool);
            let is_fs_command = match &req.subject {
                Subject::Command(c) => split_compound(c).iter().all(|part| {
                    let first = part.split_whitespace().next().unwrap_or("");
                    ACCEPT_EDITS_COMMANDS.contains(&first) && !part.contains("..") && !part.contains(" /")
                }),
                _ => false,
            };
            if is_edit || is_fs_command {
                return Decision::Allow { reason: Reason::Mode(self.mode) };
            }
        }
        if self.mode == PermissionMode::Auto {
            if let Subject::Command(c) = &req.subject {
                if mode::auto_mode_allows(c) {
                    return Decision::Allow { reason: Reason::Mode(self.mode) };
                }
            } else if matches!(req.subject, Subject::Path { write: true, .. }) && path_outside.is_none() {
                return Decision::Allow { reason: Reason::Mode(self.mode) };
            }
        }
        let reason = path_outside.map(Reason::OutsideWorkingDirs).unwrap_or(Reason::Default);
        self.ask_or_deny(reason, req)
    }

    /// The first path a command line names that is outside the working directories (and
    /// the folders reads are allowed from): an argument that is written as a path
    /// (`/etc/x`, `~/x`, `../x`), or one that exists here and leads out through a link.
    /// Words that are not paths (flags, patterns, names of things that don't exist) are
    /// left alone, and so are the standard streams (`2>/dev/null`).
    fn command_path_outside(&self, cmd: &str) -> Option<PathBuf> {
        const STREAMS: &[&str] = &["/dev/null", "/dev/stdin", "/dev/stdout", "/dev/stderr", "/dev/tty"];
        for part in split_compound(cmd) {
            for word in part.split_whitespace().skip(1) {
                // `2>/x`, `<x`, `--out=/x`, `"x"`: the path inside.
                let w = word.trim_start_matches(|c: char| c.is_ascii_digit()).trim_start_matches(['<', '>', '&']);
                let w = if word.starts_with('-') { w.split_once('=').map(|(_, v)| v).unwrap_or("") } else { w };
                let w = w.trim_matches(['"', '\'', ';', ')', '(']);
                if w.is_empty() || w.starts_with('-') || STREAMS.contains(&w) {
                    continue;
                }
                let written_as_path =
                    w.starts_with('/') || w.starts_with('~') || w.starts_with("..") || w.contains("/../");
                let p = normalize(Path::new(w), &self.cwd);
                if !written_as_path && p.symlink_metadata().is_err() {
                    continue;
                }
                let readable =
                    self.read_dirs.iter().any(|d| is_within(&p, d) && is_within(&paths::real(&p), &paths::real(d)));
                if !self.in_working_dirs(&p) && !readable {
                    return Some(p);
                }
            }
        }
        None
    }

    fn ask_or_deny(&self, reason: Reason, req: &Request) -> Decision {
        if self.mode == PermissionMode::DontAsk {
            return Decision::Deny { reason };
        }
        Decision::Ask { reason, suggestions: self.suggest(req) }
    }

    /// "Always allow" options offered alongside a prompt.
    pub fn suggest(&self, req: &Request) -> Vec<Suggestion> {
        let dest = "localSettings".to_string();
        match &req.subject {
            Subject::Command(c) => {
                let rules: Vec<RuleValue> = split_compound(c)
                    .iter()
                    .filter_map(|part| {
                        let words: Vec<&str> = part.split_whitespace().collect();
                        let n = if words.len() > 1 && !words[1].starts_with('-') { 2 } else { 1 };
                        (!words.is_empty()).then(|| RuleValue {
                            tool_name: req.tool.to_string(),
                            rule_content: Some(format!("{} *", words[..n.min(words.len())].join(" "))),
                        })
                    })
                    .collect();
                if rules.is_empty() {
                    vec![]
                } else {
                    vec![Suggestion::AddRules { rules, behavior: Behavior::Allow, destination: dest }]
                }
            }
            Subject::Path { path, write } => {
                let p = normalize(path, &self.cwd);
                if !self.in_working_dirs(&p) {
                    let dir =
                        if p.is_dir() { p.clone() } else { p.parent().map(Path::to_path_buf).unwrap_or(p.clone()) };
                    vec![Suggestion::AddDirectories {
                        directories: vec![dir.display().to_string()],
                        destination: "session".into(),
                    }]
                } else if *write {
                    vec![Suggestion::SetMode { mode: "acceptEdits".into(), destination: "session".into() }]
                } else {
                    vec![]
                }
            }
            Subject::Url(u) => match rule::host_of(u) {
                Some(h) => vec![Suggestion::AddRules {
                    rules: vec![RuleValue {
                        tool_name: req.tool.to_string(),
                        rule_content: Some(format!("domain:{h}")),
                    }],
                    behavior: Behavior::Allow,
                    destination: dest,
                }],
                None => vec![],
            },
            Subject::Name(n) => vec![Suggestion::AddRules {
                rules: vec![RuleValue { tool_name: req.tool.to_string(), rule_content: Some(n.clone()) }],
                behavior: Behavior::Allow,
                destination: dest,
            }],
            Subject::None => {
                vec![Suggestion::AddRules {
                    rules: vec![RuleValue { tool_name: req.tool.to_string(), rule_content: None }],
                    behavior: Behavior::Allow,
                    destination: dest,
                }]
            }
        }
    }
}

/// How a rule matches a compound shell command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMode {
    /// Any sub-command matching is enough (deny / ask).
    Any,
    /// Every sub-command must match (allow).
    All,
}

#[cfg(test)]
mod tests;
