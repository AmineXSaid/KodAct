use std::path::{Component, Path, PathBuf};

/// Absolute, lexically normalised path (no `.`/`..`), without touching the
/// filesystem, so it works for files that do not exist yet. `~` expands to
/// the home directory.
pub fn normalize(path: &Path, base: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    // Windows: Git Bash paths (`/c/x`) and verbatim ones (`\\?\C:\x`) name the
    // same files as `C:\x`; rules and working directories use that form.
    if cfg!(windows) {
        if let Some(w) = windows_form(&s) {
            return normalize(Path::new(&w), base);
        }
    }
    let joined = if let Some(rest) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\").filter(|_| cfg!(windows))) {
        dirs::home_dir().unwrap_or_default().join(rest)
    } else if s == "~" {
        dirs::home_dir().unwrap_or_default()
    } else if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// A Windows path written another way, in native form: `/c/x` and
/// `/cygdrive/c/x` (Git Bash), `C:/x`, and the verbatim `\\?\C:\x`. `None` when
/// it is already native (or not a Windows path).
pub fn windows_form(s: &str) -> Option<String> {
    if s.starts_with(r"\\?\") {
        return kodact_platform::path::simplify_verbatim(s);
    }
    if s.contains('/') {
        return kodact_platform::path::msys_to_windows(s);
    }
    None
}

/// `path` equals `dir` or lies below it (both already normalised). Windows
/// paths compare without regard to case or separator, as the filesystem does.
/// Where a normalised path really leads: symbolic links followed in the part of it
/// that exists, the rest (a file about to be created) kept as written.
///
/// [`normalize`] reads a path as text. A link inside the project that points outside it
/// (`project/link -> ~/.ssh`) is, as text, inside the project: a read through it was
/// allowed without the question every other path outside gets, and a deny rule on the
/// folder it points to did not see it.
pub fn real(path: &Path) -> PathBuf {
    let mut rest = vec![];
    let mut at = path;
    loop {
        if let Ok(found) = std::fs::canonicalize(at) {
            // Windows: canonical paths come back in the verbatim form.
            let found = if cfg!(windows) { normalize(&found, Path::new("")) } else { found };
            return rest.iter().rev().fold(found, |p, part| p.join(part));
        }
        match (at.parent(), at.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                at = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

pub fn is_within(path: &Path, dir: &Path) -> bool {
    is_within_on(cfg!(windows), path, dir)
}

fn is_within_on(windows: bool, path: &Path, dir: &Path) -> bool {
    if !windows {
        return path.starts_with(dir);
    }
    let (p, d) = (fold(path), fold(dir));
    let d = d.trim_end_matches('/');
    p == d || p.starts_with(&format!("{d}/"))
}

/// A Windows path for comparison: forward slashes, lower case.
pub(crate) fn fold(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/").to_lowercase()
}

#[cfg(test)]
mod windows_tests {
    use super::*;

    #[test]
    fn windows_paths_in_other_forms() {
        assert_eq!(windows_form("/c/Users/me/proj").as_deref(), Some(r"C:\Users\me\proj"));
        assert_eq!(windows_form("C:/Users/me").as_deref(), Some(r"C:\Users\me"));
        assert_eq!(windows_form(r"\\?\C:\Users\me").as_deref(), Some(r"C:\Users\me"));
        assert_eq!(windows_form(r"C:\Users\me"), None);
        assert_eq!(windows_form("src/main.rs"), None, "relative paths stay relative");
    }

    #[test]
    fn windows_containment_ignores_case_and_separators() {
        let dir = Path::new(r"C:\Users\Me\Proj");
        assert!(is_within_on(true, Path::new(r"c:\users\me\proj\src\a.rs"), dir));
        assert!(is_within_on(true, Path::new("C:/Users/Me/Proj"), dir));
        assert!(!is_within_on(true, Path::new(r"C:\Users\Me\Project2\a.rs"), dir));
        assert!(!is_within_on(false, Path::new("/home/ME/proj/a"), Path::new("/home/me/proj")));
    }
}
