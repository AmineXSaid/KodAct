//! Thin wrappers over the `git` binary.

use std::path::{Path, PathBuf};
use std::process::Command;

/// `git` with no console window (Windows) and no optional locks.
fn git_cmd() -> Command {
    let mut c = Command::new("git");
    kodact_platform::process::no_window(&mut c);
    c
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = git_cmd().args(args).current_dir(dir).env("GIT_OPTIONAL_LOCKS", "0").output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// Top of the working tree containing `dir`.
pub fn repo_root(dir: &Path) -> Option<PathBuf> {
    // git prints `C:/x` on Windows; KodAct compares it with native paths.
    git(dir, &["rev-parse", "--show-toplevel"]).map(|s| kodact_platform::path::native(&s))
}

pub fn is_repo(dir: &Path) -> bool {
    repo_root(dir).is_some()
}

pub fn current_branch(dir: &Path) -> Option<String> {
    git(dir, &["rev-parse", "--abbrev-ref", "HEAD"]).filter(|b| b != "HEAD")
}

/// `origin/HEAD`'s branch, else `main` or `master` when present.
pub fn default_branch(dir: &Path) -> Option<String> {
    if let Some(r) = git(dir, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]) {
        return Some(r.trim_start_matches("origin/").to_string());
    }
    ["main", "master"]
        .into_iter()
        .find(|b| git(dir, &["rev-parse", "--verify", "--quiet", b]).is_some())
        .map(str::to_string)
}

/// The repository that owns a linked worktree (its common dir's parent).
pub fn main_repo(dir: &Path) -> Option<PathBuf> {
    let common = git(dir, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let p = PathBuf::from(common);
    let main = if p.file_name().map(|n| n == ".git").unwrap_or(false) { p.parent()?.to_path_buf() } else { p };
    let root = repo_root(dir)?;
    (main != root).then_some(main)
}

/// A snapshot for the system prompt: branch, default branch, short status, recent commits.
pub fn status_snapshot(dir: &Path) -> Option<String> {
    let root = repo_root(dir)?;
    let branch = current_branch(&root).unwrap_or_else(|| "(detached)".into());
    let main = default_branch(&root).unwrap_or_default();
    let status = git(&root, &["status", "--short"]).unwrap_or_default();
    let status_lines: Vec<&str> = status.lines().collect();
    let status_text = if status_lines.is_empty() {
        "(clean)".to_string()
    } else if status_lines.len() > 40 {
        format!("{}\n... and {} more", status_lines[..40].join("\n"), status_lines.len() - 40)
    } else {
        status
    };
    let log = git(&root, &["log", "--oneline", "-n", "5"]).unwrap_or_default();
    Some(format!(
        "Current branch: {branch}\nMain branch (usually the base for PRs): {main}\n\nStatus:\n{status_text}\n\nRecent commits:\n{log}"
    ))
}

/// Uncommitted changes to tracked files (`git diff HEAD`), and untracked files.
/// `None` outside a repository.
pub fn uncommitted(dir: &Path) -> Option<(String, Vec<String>)> {
    let root = repo_root(dir)?;
    // A repository without commits has no HEAD: diff the index instead.
    let diff = git(&root, &["diff", "HEAD", "--no-color", "--no-ext-diff"])
        .or_else(|| git(&root, &["diff", "--cached", "--no-color", "--no-ext-diff"]))
        .unwrap_or_default();
    let untracked = git(&root, &["ls-files", "--others", "--exclude-standard"])
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default();
    Some((diff, untracked))
}

/// Create a worktree at `path` on a new branch from HEAD.
pub fn add_worktree(repo: &Path, path: &Path, branch: &str) -> Result<(), String> {
    let out = git_cmd()
        .args(["worktree", "add", "-b", branch])
        .arg(path)
        .arg("HEAD")
        .current_dir(repo)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Directories of caches that tools write as a side effect of running (Python, pytest,
/// type checkers, linters): they appear and disappear without anyone changing the project.
const CACHE_DIRS: &[&str] =
    &["__pycache__", ".pytest_cache", ".mypy_cache", ".ruff_cache", ".hypothesis", ".tox", ".nox"];

/// At most this many files are looked at, so a huge repository can't stall a turn.
const MAX_FINGERPRINT_FILES: usize = 200_000;

fn is_cache(path: &str) -> bool {
    path.split('/').any(|c| CACHE_DIRS.contains(&c))
        || path.ends_with(".pyc")
        || path.ends_with("/.coverage")
        || path == ".coverage"
}

/// A fingerprint of the working tree's files: the path, size and mtime of every tracked or
/// untracked, not ignored file (caches skipped), plus the content of the `extra` files (the
/// ones written this turn). Equal fingerprints mean no file was created, changed or deleted in
/// between. It looks at the files, not at git's state, so staging, committing or creating a
/// branch doesn't move it. Outside a repository only `extra` counts (`None` when it's empty).
pub fn worktree_fingerprint(dir: &Path, extra: &[PathBuf]) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut any = false;
    if let Some(root) = repo_root(dir) {
        let files = git(&root, &["ls-files", "-z", "--cached", "--others", "--exclude-standard"])?;
        for f in files.split('\0').filter(|f| !f.is_empty() && !is_cache(f)).take(MAX_FINGERPRINT_FILES) {
            let m = std::fs::symlink_metadata(root.join(f)).ok();
            (f, m.as_ref().map(|m| m.len()), m.and_then(|m| m.modified().ok())).hash(&mut h);
        }
        any = true;
    }
    for p in extra {
        let m = std::fs::metadata(p).ok();
        let content = m.as_ref().filter(|m| m.len() <= 1 << 20).and_then(|_| std::fs::read(p).ok());
        (p, m.as_ref().map(|m| m.len()), m.and_then(|m| m.modified().ok()), content).hash(&mut h);
        any = true;
    }
    any.then(|| h.finish())
}

/// Add `pattern` to the repository's `.git/info/exclude` (untracked, local ignore) if missing.
pub fn exclude(repo_root: &Path, pattern: &str) -> std::io::Result<()> {
    let git_dir = repo_root.join(".git");
    if !git_dir.is_dir() {
        return Ok(());
    }
    let path = git_dir.join("info").join("exclude");
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    if current.lines().any(|l| l.trim() == pattern) {
        return Ok(());
    }
    std::fs::create_dir_all(path.parent().expect("info dir"))?;
    let sep = if current.is_empty() || current.ends_with('\n') { "" } else { "\n" };
    std::fs::write(&path, format!("{current}{sep}{pattern}\n"))
}

/// Uncommitted changes in a working tree?
pub fn is_dirty(dir: &Path) -> bool {
    git(dir, &["status", "--porcelain"]).map(|s| !s.is_empty()).unwrap_or(false)
}

/// The commit HEAD points at.
pub fn head(dir: &Path) -> Option<String> {
    git(dir, &["rev-parse", "HEAD"]).map(|s| s.trim().to_string())
}

/// Delete a branch that carries nothing beyond what is merged (`git branch -d`).
pub fn delete_branch(repo: &Path, branch: &str) -> Result<(), String> {
    let out = git_cmd().args(["branch", "-d", branch]).current_dir(repo).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

pub fn remove_worktree(repo: &Path, path: &Path) -> Result<(), String> {
    let out = git_cmd().args(["worktree", "remove"]).arg(path).current_dir(repo).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init(dir: &Path) {
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
            vec!["commit", "-q", "--allow-empty", "-m", "first"],
        ] {
            assert!(Command::new("git").args(&args).current_dir(dir).status().unwrap().success());
        }
    }

    fn run(dir: &Path, args: &[&str]) {
        assert!(Command::new("git").args(args).current_dir(dir).status().unwrap().success(), "git {args:?}");
    }

    #[test]
    fn snapshot_and_worktree() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        init(&root);
        std::fs::write(root.join("a.txt"), "x").unwrap();
        let snap = status_snapshot(&root).unwrap();
        assert!(snap.contains("Current branch: main") && snap.contains("?? a.txt") && snap.contains("first"), "{snap}");
        let wt = root.join(".kodact/worktrees/w1");
        add_worktree(&root, &wt, "kodact/w1").unwrap();
        assert_eq!(current_branch(&wt).as_deref(), Some("kodact/w1"));
        assert_eq!(main_repo(&wt), Some(root.clone()));
        assert_eq!(main_repo(&root), None);
        assert!(!is_dirty(&wt));
        remove_worktree(&root, &wt).unwrap();
        assert!(status_snapshot(Path::new("/")).is_none());
    }

    #[test]
    fn fingerprint_moves_with_the_worktree() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(worktree_fingerprint(d.path(), &[]), None);
        let root = d.path().canonicalize().unwrap();
        init(&root);
        let fp = || worktree_fingerprint(&root, &[]).unwrap();
        let clean = fp();
        assert_eq!(fp(), clean, "stable when nothing changes");
        std::fs::write(root.join("new.txt"), "a").unwrap();
        let untracked = fp();
        assert_ne!(untracked, clean, "a created file counts");
        std::fs::write(root.join("new.txt"), "ab").unwrap();
        let edited = fp();
        assert_ne!(edited, untracked, "an edited untracked file counts");

        // Fix, run the tests, commit: staging and committing change no file.
        run(&root, &["add", "new.txt"]);
        assert_eq!(fp(), edited, "git add is not a change");
        run(&root, &["commit", "-q", "-m", "add new.txt"]);
        assert_eq!(fp(), edited, "git commit is not a change");
        run(&root, &["branch", "topic"]);
        assert_eq!(fp(), edited, "creating a branch is not a change");
        std::fs::create_dir_all(root.join("pkg/__pycache__")).unwrap();
        std::fs::write(root.join("pkg/__pycache__/m.cpython-312.pyc"), "x").unwrap();
        std::fs::create_dir_all(root.join(".pytest_cache")).unwrap();
        std::fs::write(root.join(".pytest_cache/README.md"), "x").unwrap();
        assert_eq!(fp(), edited, "caches written by running Python and pytest are not changes");
        std::fs::remove_dir_all(root.join("pkg/__pycache__")).unwrap();
        assert_eq!(fp(), edited, "nor is removing them");

        std::fs::write(root.join("new.txt"), "abc").unwrap();
        let tracked_edit = fp();
        assert_ne!(tracked_edit, edited, "editing a tracked file counts");
        std::fs::remove_file(root.join("new.txt")).unwrap();
        assert_ne!(fp(), tracked_edit, "deleting a tracked file counts");
    }

    #[test]
    fn fingerprint_outside_git_follows_the_files_written() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("my_warrior.red");
        std::fs::write(&f, ";redcode-94\nimp mov 0, 1\n").unwrap();
        let extra = [f.clone()];
        let before = worktree_fingerprint(d.path(), &extra).unwrap();
        assert_eq!(worktree_fingerprint(d.path(), &extra), Some(before));
        std::fs::write(d.path().join("notes.txt"), "x").unwrap();
        assert_eq!(worktree_fingerprint(d.path(), &extra), Some(before), "other files aren't followed outside git");
        // `cat > my_warrior.red <<EOF` after the last check: same size, different content.
        std::fs::write(&f, ";redcode-94\nimp mov 0, 2\n").unwrap();
        assert_ne!(worktree_fingerprint(d.path(), &extra), Some(before), "a shell rewrite of a written file counts");
    }
}
