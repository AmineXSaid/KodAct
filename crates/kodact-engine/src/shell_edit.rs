//! Shell edits that break a file (GOALS pillar 3).
//!
//! A shell command that rewrites a file (`sed -i`, `perl -pi`, a redirect) is the right tool
//! for a mechanical change across many files, and nothing is said when it works. When the file
//! it changed no longer parses, the model is told so with the command's result, as an edit
//! would be told, and, once a turn, that the Edit tool shows the exact lines it replaces and
//! says at once when a file stops parsing.
//!
//! Which files a command changed is seen, not guessed: before it runs, the files its words name
//! are noted (size and modification time, or that they don't exist); after it, those that
//! differ are the ones it changed.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What a file was before a command ran.
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    path: PathBuf,
    state: Option<(u64, Option<SystemTime>)>,
}

/// The files a command names, as they were before it ran.
#[derive(Debug, Clone, Default)]
pub(crate) struct Before(Vec<Seen>);

/// Most files one command's words are checked for: enough for a command that names its files,
/// and a cap on the work for one with a long argument list.
const MAX_FILES: usize = 64;

fn state(p: &Path) -> Option<(u64, Option<SystemTime>)> {
    let m = std::fs::metadata(p).ok().filter(|m| m.is_file())?;
    Some((m.len(), m.modified().ok()))
}

impl Before {
    /// Note the files `command`'s words name under `root` (or absolute), existing or not.
    pub fn note(command: &str, root: &Path) -> Before {
        let mut seen: Vec<Seen> = vec![];
        for word in command.split(|c: char| c.is_whitespace() || matches!(c, ';' | '|' | '&' | '(' | ')')) {
            // `>out.py`, `2>>log`, `--file=x.py`, quotes.
            let w = word.trim_start_matches(|c: char| c.is_ascii_digit() || c == '>' || c == '<');
            let w = w.rsplit_once('=').map(|(_, v)| v).unwrap_or(w);
            let w = w.trim_matches(|c: char| matches!(c, '"' | '\'' | '`'));
            if w.is_empty() || w.starts_with('-') || w.contains('*') || w.contains('$') {
                continue;
            }
            let name = w.rsplit('/').next().unwrap_or(w);
            if !name.contains('.') || name.starts_with('.') && name.len() < 2 {
                continue;
            }
            let p = if Path::new(w).is_absolute() { PathBuf::from(w) } else { root.join(w) };
            if p.is_dir() || seen.iter().any(|s| s.path == p) {
                continue;
            }
            seen.push(Seen { state: state(&p), path: p });
            if seen.len() >= MAX_FILES {
                break;
            }
        }
        Before(seen)
    }

    /// The noted files that exist now and are not as they were.
    pub fn changed(&self) -> Vec<PathBuf> {
        self.0
            .iter()
            .filter_map(|s| state(&s.path).filter(|now| Some(*now) != s.state).map(|_| s.path.clone()))
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Said once a turn, after a shell command left a file that doesn't parse.
pub(crate) fn suggestion(file: &str) -> String {
    format!(
        "\n\n<system-reminder>\nThat shell command changed {file}, and it no longer parses. Shell edits suit \
         mechanical changes; for a change that needs care, the Edit tool shows the exact lines it replaces and \
         says at once when a file stops parsing. Ignore this if the command did what you meant and a later step \
         completes the file.\n</system-reminder>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_files_a_command_changed_are_seen_not_guessed() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::write(root.join("a.py"), "x = 1\n").unwrap();
        std::fs::write(root.join("b.py"), "y = 2\n").unwrap();
        std::fs::create_dir(root.join("pkg")).unwrap();
        let before =
            Before::note("sed -i 's/1/2/' a.py && cat b.py > out.json; echo hi 2>>log.txt --x=pkg/new.py pkg", root);
        let names: Vec<String> =
            before.0.iter().map(|s| s.path.strip_prefix(root).unwrap().display().to_string()).collect();
        assert_eq!(
            names,
            ["a.py", "b.py", "out.json", "log.txt", "pkg/new.py"],
            "named files, existing or not; not the directory"
        );
        assert!(before.changed().is_empty(), "nothing ran yet");
        std::fs::write(root.join("a.py"), "x = 22\n").unwrap();
        std::fs::write(root.join("out.json"), "{}").unwrap();
        let changed: Vec<String> =
            before.changed().iter().map(|p| p.file_name().unwrap().to_string_lossy().into()).collect();
        assert_eq!(changed, ["a.py", "out.json"], "changed and created; b.py was only read");
        assert!(Before::note("ls -la && git status", root).is_empty());
    }

    #[test]
    fn the_suggestion_leaves_shell_edits_their_place() {
        let t = suggestion("app.py");
        assert!(
            t.contains("app.py")
                && t.contains("Edit tool")
                && t.contains("mechanical changes")
                && t.contains("Ignore this"),
            "{t}"
        );
    }
}
