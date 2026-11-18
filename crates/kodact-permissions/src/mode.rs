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

    /// Human-readable mode name for the TUI and LLM notifications.
    pub fn label(&self) -> &'static str {
        match self {
            PermissionMode::Default => "Manual",
            PermissionMode::AcceptEdits => "Accept edits",
            PermissionMode::Plan => "Plan",
            PermissionMode::DontAsk => "Don't ask",
            PermissionMode::BypassPermissions => "Dangerous",
            PermissionMode::Auto => "Auto",
        }
    }

    /// Short behavioral definition shown to the user and injected into LLM reminders.
    pub fn definition(&self) -> &'static str {
        match self {
            PermissionMode::Default => "Reads and searches freely; asks before editing files or running shell commands.",
            PermissionMode::AcceptEdits => {
                "Reads and edits files in the project automatically; asks before running shell commands."
            }
            PermissionMode::Plan => {
                "Read-only exploration and planning; cannot edit files or run mutating commands until the plan is approved."
            }
            PermissionMode::DontAsk => {
                "Never prompts; reads and pre-allowed tools run, and anything that would ask is denied."
            }
            PermissionMode::BypassPermissions => {
                "Runs all tools, file edits, and shell commands without asking for confirmation."
            }
            PermissionMode::Auto => {
                "Runs file edits and routine project commands automatically; asks before risky or external actions."
            }
        }
    }

    /// Ultra-concise capability tag shown in compact UI chrome.
    pub fn short_rule(&self) -> &'static str {
        match self {
            PermissionMode::Default => "asks edits & shell",
            PermissionMode::AcceptEdits => "auto-edits · asks shell",
            PermissionMode::Auto => "auto-edits & routine shell",
            PermissionMode::Plan => "read-only",
            PermissionMode::DontAsk => "denies unallowed",
            PermissionMode::BypassPermissions => "no prompts",
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

#[cfg(test)]
mod tests {
    use super::PermissionMode;

    #[test]
    fn short_rules_distinguish_all_modes() {
        assert_eq!(PermissionMode::Default.short_rule(), "asks edits & shell");
        assert_eq!(PermissionMode::AcceptEdits.short_rule(), "auto-edits · asks shell");
        assert_eq!(PermissionMode::Auto.short_rule(), "auto-edits & routine shell");
        assert_eq!(PermissionMode::Plan.short_rule(), "read-only");
        assert_eq!(PermissionMode::DontAsk.short_rule(), "denies unallowed");
        assert_eq!(PermissionMode::BypassPermissions.short_rule(), "no prompts");
    }
}
