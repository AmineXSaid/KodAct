use crate::shell::split_compound;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PermissionMode {
    /// Ask for anything that is not read-only (`manual` is an alias).
    #[default]
    Default,
    AcceptEdits,
    Plan,
    /// Never prompt; anything that would ask is denied.
    DontAsk,
    BypassPermissions,
    /// Allow routine work, ask for risky commands (rule-based in KodAct).
    Auto,
}

impl PermissionMode {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "default" | "manual" => PermissionMode::Default,
            "acceptEdits" => PermissionMode::AcceptEdits,
            "plan" => PermissionMode::Plan,
            "dontAsk" => PermissionMode::DontAsk,
            "bypassPermissions" => PermissionMode::BypassPermissions,
            "auto" => PermissionMode::Auto,
            _ => return None,
        })
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            PermissionMode::Default => "default",
            PermissionMode::AcceptEdits => "acceptEdits",
            PermissionMode::Plan => "plan",
            PermissionMode::DontAsk => "dontAsk",
            PermissionMode::BypassPermissions => "bypassPermissions",
            PermissionMode::Auto => "auto",
        }
    }

    /// The Shift+Tab cycle in the TUI.
    pub fn next(&self, bypass_available: bool) -> Self {
        match self {
            PermissionMode::Default => PermissionMode::AcceptEdits,
            PermissionMode::AcceptEdits => PermissionMode::Auto,
            PermissionMode::Auto => PermissionMode::Plan,
            PermissionMode::Plan if bypass_available => PermissionMode::BypassPermissions,
            _ => PermissionMode::Default,
        }
    }
}

const RISKY: &[&str] = &[
    "sudo ",
    "rm -rf",
    "rm -fr",
    "mkfs",
    "dd ",
    "chmod -R",
    "chown -R",
    "git push",
    "git reset --hard",
    "git clean",
    "curl ",
    "wget ",
    "ssh ",
    "scp ",
    "npm publish",
    "cargo publish",
    "docker ",
    "kubectl ",
    "shutdown",
    "reboot",
    "> /dev/",
    ":(){",
];

const ROUTINE_PREFIXES: &[&str] = &[
    "ls",
    "cat",
    "head",
    "tail",
    "wc",
    "pwd",
    "echo",
    "grep",
    "rg",
    "find",
    "which",
    "file",
    "stat",
    "tree",
    "diff",
    "git status",
    "git diff",
    "git log",
    "git show",
    "git branch",
    "git add",
    "git commit",
    "git checkout -b",
    "git switch",
    "git stash",
    "cargo build",
    "cargo test",
    "cargo check",
    "cargo clippy",
    "cargo fmt",
    "cargo run",
    "npm test",
    "npm run",
    "npm install",
    "pnpm ",
    "yarn ",
    "pytest",
    "python -m pytest",
    "go test",
    "go build",
    "make",
    "mkdir",
    "touch",
    "cp ",
    "mv ",
    "jq",
    "sort",
    "uniq",
    "sed -n",
];

/// The rule-based stand-in for auto mode's classifier: routine development
/// commands pass, anything risky or unrecognised asks.
pub(crate) fn auto_mode_allows(command: &str) -> bool {
    let parts = split_compound(command);
    !parts.is_empty()
        && parts.iter().all(|p| {
            let p = p.trim();
            !RISKY.iter().any(|r| p.contains(r))
                && ROUTINE_PREFIXES
                    .iter()
                    .any(|pre| p == pre.trim() || p.starts_with(&format!("{} ", pre.trim())) || p.starts_with(pre))
        })
}
