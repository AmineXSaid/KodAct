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

/// In `Auto` mode, allow all commands except destructive/risky ones (removals,
/// privilege escalation, force-pushes, raw device writes).
pub(crate) fn auto_mode_allows(command: &str) -> bool {
    let parts = split_compound(command);
    !parts.is_empty() && parts.iter().all(|p| part_allowed(p.trim(), 0))
}

/// Programs that remove files or what is in them, on Unix and on Windows (cmd and
/// PowerShell, whose names are case-insensitive): Auto asks before each.
const REMOVERS: &[&str] = &[
    "rm",
    "rmdir",
    "unlink",
    "shred",
    "sudo",
    "su",
    "mkfs",
    "dd",
    "del",
    "erase",
    "rd",
    "remove-item",
    "ri",
    "clear-content",
    "clc",
    "format",
    "takeown",
    "icacls",
];

/// Shells that run a command given as an argument: what they run is checked too.
const WRAPPERS: &[&str] = &["powershell", "pwsh", "cmd", "bash", "sh"];

fn program(word: &str) -> String {
    let w = word.trim_matches(['"', '\'', '&', '(']).to_ascii_lowercase();
    let base = w.rsplit(['/', '\\']).next().unwrap_or(&w).to_string();
    base.strip_suffix(".exe").map(str::to_string).unwrap_or(base)
}

/// `pattern` appears in `part` as its own word, not inside one: `dd ` in
/// `pptx_tool.py add deck.pptx` is no disk copy, `curl ` in `--curly` no fetch.
fn has_word(part: &str, pattern: &str) -> bool {
    part.match_indices(pattern).any(|(i, _)| {
        part[..i]
            .chars()
            .next_back()
            .is_none_or(|c| c.is_whitespace() || matches!(c, ';' | '|' | '&' | '(' | '`' | '"' | '\''))
    })
}

fn part_allowed(p: &str, depth: u8) -> bool {
    if RISKY.iter().any(|r| has_word(p, r)) {
        return false;
    }
    let words: Vec<&str> = p.split_whitespace().collect();
    let Some(first) = words.first() else { return true };
    let first = program(first);
    if REMOVERS.contains(&first.as_str()) {
        return false;
    }
    // `powershell -Command "Remove-Item x"`, `cmd /c del x`, `bash -c "rm x"`.
    if WRAPPERS.contains(&first.as_str()) && depth < 3 {
        let at = words.iter().position(|w| {
            let w = w.to_ascii_lowercase();
            matches!(w.as_str(), "-c" | "/c" | "/k" | "-command" | "-encodedcommand" | "-ec")
        });
        if let Some(i) = at {
            if matches!(words[i].to_ascii_lowercase().as_str(), "-encodedcommand" | "-ec") {
                return false; // what it runs can't be read: ask.
            }
            let inner = words[i + 1..].join(" ");
            let inner = inner.trim().trim_matches(['"', '\'']);
            return split_compound(inner).iter().all(|q| part_allowed(q.trim(), depth + 1));
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{auto_mode_allows, PermissionMode};

    #[test]
    fn auto_asks_before_windows_removals_too() {
        for c in [
            "del /q build\\out.txt",
            "rd /s /q pages",
            "rmdir /s pages",
            "Remove-Item -Recurse pages",
            "remove-item pages",
            "ri pages",
            "Clear-Content notes.txt",
            "C:\\Windows\\System32\\cmd.exe /c del x",
            "cmd /c \"del x\"",
            "powershell -NoProfile -Command \"Remove-Item -Recurse pages\"",
            "pwsh.exe -c 'ri pages'",
            "powershell -EncodedCommand SQBFAFgA",
            "bash -c \"rm -r pages\"",
            "cd pages && rm -rf x",
            "dd if=/dev/zero of=x",
            "x=1; curl https://example.com",
            "git push origin main",
        ] {
            assert!(!auto_mode_allows(c), "{c} should ask in Auto");
        }
        for c in [
            "python build_deck.py",
            "powershell -NoProfile -ExecutionPolicy Bypass -File render.ps1",
            "powershell -Command \"Get-ChildItem pages\"",
            "cmd /c dir",
            "cargo build",
            "echo rd",
            // Risky words inside other words are not those commands.
            r#"python pptx_tool.py add deck.pptx --json '{"title": "Each has a job"}'"#,
            "grep --curly-braces x",
        ] {
            assert!(auto_mode_allows(c), "{c} should run in Auto");
        }
    }

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
