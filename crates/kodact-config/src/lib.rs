//! Configuration: layered settings, memory files (KODACT.md) and where resources live.

mod memory;
mod settings;

pub use memory::{load_memory, MemoryFile, MemoryKind};
pub use redact::{is_secret_key, redact, redact_deep, redact_text};

mod redact;
pub use settings::{
    deep_merge, load_settings, parse_sources, remove_setting, write_setting, LoadedSettings, SettingSource,
    SettingsLayer, SettingsOptions,
};

use std::path::{Path, PathBuf};

/// `base/new`, unless only `base/old` exists: a folder made before KodAct was
/// renamed (from ForgeCLI) keeps working, settings, history and all.
pub fn renamed(base: &Path, new: &str, old: &str) -> PathBuf {
    let n = base.join(new);
    let o = base.join(old);
    if !n.exists() && o.exists() {
        o
    } else {
        n
    }
}

/// A project's own folder: `.kodact`, or ForgeCLI's `.forge` when only that one exists.
pub fn project_dir(project: &Path) -> PathBuf {
    renamed(project, ".kodact", ".forge")
}

fn env_dir(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from).filter(|p| p.is_absolute())
}

/// User configuration: settings.json, KODACT.md, agents, commands, skills.
///
/// `$KODACT_HOME` if set; else `$XDG_CONFIG_HOME/kodact`; else the platform's
/// config directory (`~/.config/kodact` on Linux,
/// `~/Library/Application Support/kodact` on macOS, `%APPDATA%\kodact` on Windows).
pub fn config_dir() -> PathBuf {
    env_dir("KODACT_HOME")
        .or_else(|| env_dir("XDG_CONFIG_HOME").map(|d| renamed(&d, "kodact", "forge")))
        .or_else(|| dirs::config_dir().map(|d| renamed(&d, "kodact", "forge")))
        .unwrap_or_else(|| renamed(&home(), ".kodact", ".forge"))
}

/// Sessions, file history and other state: `$KODACT_HOME/state`, else
/// `$XDG_STATE_HOME/kodact`, else the platform's local data directory.
pub fn state_dir() -> PathBuf {
    env_dir("KODACT_HOME")
        .map(|d| d.join("state"))
        .or_else(|| env_dir("XDG_STATE_HOME").map(|d| renamed(&d, "kodact", "forge")))
        .or_else(|| dirs::state_dir().or_else(dirs::data_local_dir).map(|d| renamed(&d, "kodact", "forge")))
        .unwrap_or_else(|| renamed(&home(), ".kodact", ".forge").join("state"))
}

/// The state directory, made if missing and kept to its owner (mode 0700 on Unix).
///
/// What is under it is what was said and done in a project: session transcripts, file
/// checkpoints, debug logs, feedback bundles, the prompt history. Only the history was
/// written private; on a shared machine every other user could read the rest.
pub fn private_state_dir() -> std::io::Result<PathBuf> {
    let dir = state_dir();
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&dir)?.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(dir)
}

/// Disposable caches: `$KODACT_HOME/cache`, else `$XDG_CACHE_HOME/kodact`, else the platform cache directory.
pub fn cache_dir() -> PathBuf {
    env_dir("KODACT_HOME")
        .map(|d| d.join("cache"))
        .or_else(|| env_dir("XDG_CACHE_HOME").map(|d| renamed(&d, "kodact", "forge")))
        .or_else(|| dirs::cache_dir().map(|d| renamed(&d, "kodact", "forge")))
        .unwrap_or_else(|| renamed(&home(), ".kodact", ".forge").join("cache"))
}

/// The user configuration directory (kept for callers that predate the XDG split).
pub fn kodact_home() -> PathBuf {
    config_dir()
}

pub fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// A path as people write it: `~/proj` under the home directory, else unchanged.
pub fn short_path(p: &Path) -> String {
    let full = p.display().to_string();
    let home = home().display().to_string();
    match full.strip_prefix(&home) {
        Some(rest) if home.len() > 1 && (rest.is_empty() || rest.starts_with(['/', '\\'])) => format!("~{rest}"),
        _ => full,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    User,
    Project,
}

/// The reference CLI's configuration folder, read for compatibility so the
/// skills, commands and agents installed for it load here unchanged:
/// `$CLAUDE_CONFIG_DIR` if set, else `~/.claude`.
pub fn compat_home() -> PathBuf {
    env_dir("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home().join(".claude"))
}

/// The folders from the project's repository root down to `project`, root
/// first: where its `.claude/<kind>` folders are looked for. Outside a
/// repository, only `project`. A linked worktree (`.git` is a file) is its own root.
pub fn project_chain(project: &Path) -> Vec<PathBuf> {
    let mut chain = vec![];
    for dir in project.ancestors() {
        chain.push(dir.to_path_buf());
        if dir.join(".git").exists() {
            chain.reverse();
            return chain;
        }
    }
    vec![project.to_path_buf()]
}

/// Directories holding `kind` resources (`agents`, `commands`, `skills`,
/// `output-styles`), lowest precedence first: the user's (the reference CLI's
/// `~/.claude/<kind>`, then KodAct's), then the project's: `.claude/<kind>` from
/// the repository root down, the vendor-neutral `.agents/<kind>` (shared with
/// other agent CLIs), then `.kodact/<kind>`.
pub fn resource_dirs(kind: &str, project: &Path) -> Vec<(Scope, PathBuf)> {
    let mut out = vec![(Scope::User, compat_home().join(kind)), (Scope::User, kodact_home().join(kind))];
    out.extend(project_chain(project).into_iter().map(|d| (Scope::Project, d.join(".claude").join(kind))));
    out.push((Scope::Project, project.join(".agents").join(kind)));
    // ForgeCLI's `.forge/<kind>`, read below KodAct's own.
    out.push((Scope::Project, project.join(".forge").join(kind)));
    out.push((Scope::Project, project.join(".kodact").join(kind)));
    // A project at the home folder: its `.claude` is the user's, read once.
    let mut seen = std::collections::HashSet::new();
    out.retain(|(_, d)| seen.insert(d.clone()));
    out
}

#[cfg(test)]
mod rename_tests {
    use super::*;

    #[test]
    fn forgecli_folders_are_used_until_kodact_has_its_own() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(project_dir(d.path()), d.path().join(".kodact"), "a new project gets KodAct's folder");
        std::fs::create_dir_all(d.path().join(".forge")).unwrap();
        assert_eq!(project_dir(d.path()), d.path().join(".forge"), "an old project keeps ForgeCLI's");
        std::fs::create_dir_all(d.path().join(".kodact")).unwrap();
        assert_eq!(project_dir(d.path()), d.path().join(".kodact"), "KodAct's wins once it exists");
        assert_eq!(renamed(d.path(), "kodact", "forge"), d.path().join("kodact"));
    }
}
