use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// Which files the model has read, and when: Edit and Write refuse to touch
/// an existing file the model has not read, or that changed since. A file
/// created after the session started (by a shell command, a script the model
/// ran) is not one of the person's files to protect: it may be written unread.
/// A file whose whole current content a shell command printed (`cat config.py`),
/// in output still in the conversation, has been seen: it counts as read.
#[derive(Debug)]
pub struct FileState {
    reads: Mutex<HashMap<PathBuf, Option<SystemTime>>>,
    /// What each Read showed: (path, offset, limit) -> (content hash, tool_use_id).
    views: Mutex<HashMap<ViewKey, (u64, String)>>,
    /// When the session started: files created after it are the session's own.
    started: SystemTime,
    /// Files that must be read again whatever their age (put back by a checkpoint restore).
    unseen: Mutex<HashSet<PathBuf>>,
    /// Placeholder lines ("... rest of the code ...") already refused once, per file: the
    /// same line sent again is meant, and is written.
    refused: Mutex<HashSet<(PathBuf, String)>>,
    /// What shell commands printed, as the model was shown it (after any cut), newest last:
    /// (tool_use_id, text). A file whose whole content is in one of them has been seen.
    shown: Mutex<Vec<(String, String)>>,
}

impl Default for FileState {
    fn default() -> Self {
        FileState {
            reads: Default::default(),
            views: Default::default(),
            // Creation times come from the kernel's coarse clock, a tick (up to 10 ms) behind
            // the one read here: a file made just after the start would look older without this.
            started: SystemTime::now() - CLOCK_TICK,
            unseen: Default::default(),
            refused: Default::default(),
            shown: Default::default(),
        }
    }
}

const CLOCK_TICK: std::time::Duration = std::time::Duration::from_millis(20);

/// Shell outputs kept to tell whether a file was shown in full: the latest few, within a total size.
const SHOWN_KEEP: usize = 32;
const SHOWN_BYTES: usize = 4 * 1024 * 1024;
/// A file larger than this is never matched against shell output.
const SHOWN_FILE_MAX: u64 = 1024 * 1024;

/// Created after `since`, where the file system records creation times (most on Linux,
/// macOS and Windows do); `false` where it doesn't, so the rule stays as strict as before.
fn created_after(p: &Path, since: SystemTime) -> bool {
    std::fs::metadata(p).and_then(|m| m.created()).is_ok_and(|c| c > since)
}

type ViewKey = (PathBuf, usize, Option<usize>);

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

impl FileState {
    pub fn record_read(&self, path: &Path) {
        self.reads.lock().unwrap().insert(path.to_path_buf(), mtime(path));
        self.unseen.lock().unwrap().remove(path);
    }

    /// Call after KodAct itself writes the file, so the next edit is allowed.
    pub fn record_write(&self, path: &Path) {
        self.record_read(path);
    }

    /// Every file read or written in this session.
    pub fn touched(&self) -> Vec<PathBuf> {
        self.reads.lock().unwrap().keys().cloned().collect()
    }

    pub fn forget(&self, path: &Path) {
        self.reads.lock().unwrap().remove(path);
        self.unseen.lock().unwrap().insert(path.to_path_buf());
    }

    /// The earlier Read (its tool_use_id) that showed this exact range of
    /// this exact content, if its result is still in the conversation.
    pub fn same_view(&self, path: &Path, offset: usize, limit: Option<usize>, hash: u64) -> Option<String> {
        let views = self.views.lock().unwrap();
        views.get(&(path.to_path_buf(), offset, limit)).filter(|(h, _)| *h == hash).map(|(_, id)| id.clone())
    }

    pub fn record_view(&self, path: &Path, offset: usize, limit: Option<usize>, hash: u64, tool_use_id: &str) {
        self.views.lock().unwrap().insert((path.to_path_buf(), offset, limit), (hash, tool_use_id.to_string()));
    }

    /// Results cleared from the conversation (micro-compaction) no longer count as seen.
    pub fn forget_views(&self, tool_use_ids: &[String]) {
        self.views.lock().unwrap().retain(|_, (_, id)| !tool_use_ids.contains(id));
        self.shown.lock().unwrap().retain(|(id, _)| !tool_use_ids.contains(id));
    }

    /// After a full compaction nothing earlier is in view.
    pub fn forget_all_views(&self) {
        self.views.lock().unwrap().clear();
        self.shown.lock().unwrap().clear();
    }

    /// A shell command's output as the model was shown it (its stdout, after any cut). A file
    /// whose whole current content is in it counts as read, as if Read had shown it: a model
    /// that prints files with `cat` isn't sent back to read what it has just seen.
    pub fn record_shown(&self, tool_use_id: &str, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        let mut shown = self.shown.lock().unwrap();
        shown.push((tool_use_id.to_string(), text.to_string()));
        let mut total: usize = shown.iter().map(|(_, t)| t.len()).sum();
        while shown.len() > SHOWN_KEEP || (total > SHOWN_BYTES && shown.len() > 1) {
            total -= shown.remove(0).1.len();
        }
    }

    /// The file's whole current content is in a shell output the model still has.
    fn shown_in_full(&self, path: &Path) -> bool {
        let Ok(meta) = std::fs::metadata(path) else { return false };
        if !meta.is_file() || meta.len() > SHOWN_FILE_MAX {
            return false;
        }
        let Ok(content) = std::fs::read_to_string(path) else { return false };
        let body = content.trim_end();
        !body.is_empty() && self.shown.lock().unwrap().iter().any(|(_, t)| t.contains(body))
    }

    /// `true` the first time this line is refused for this file, `false` once it has been.
    pub fn refuse_once(&self, path: &Path, line: &str) -> bool {
        self.refused.lock().unwrap().insert((path.to_path_buf(), line.to_string()))
    }

    /// Never read in this session, and not put back by a checkpoint restore: the case
    /// `edit.withoutRead` lets an exact-text Edit through. A file that changed since it was
    /// read is not this case.
    pub fn never_read(&self, path: &Path) -> bool {
        !self.reads.lock().unwrap().contains_key(path) && !self.unseen.lock().unwrap().contains(path)
    }

    /// `Ok` when an existing file may be modified.
    pub fn check_writable(&self, path: &Path) -> Result<(), String> {
        if !path.exists() {
            return Ok(());
        }
        let seen = self.reads.lock().unwrap().get(path).copied();
        let refusal = match seen {
            // Made during this session and not since put back from a checkpoint: the session's own.
            None if created_after(path, self.started) && !self.unseen.lock().unwrap().contains(path) => return Ok(()),
            None => "File has not been read yet. Read it first before writing to it.",
            Some(seen) if seen != mtime(path) => {
                "File has been modified since read, either by the user or by a linter. Read it again before attempting to write it."
            }
            Some(_) => return Ok(()),
        };
        // Printed in full by a shell command the model ran, as it is now: seen, as a Read shows it.
        if self.shown_in_full(path) {
            self.record_read(path);
            return Ok(());
        }
        Err(refusal.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// optimize-under-time and stop-at-target (2026-11-16): the model printed the file with `cat`,
    /// then wrote it, and was told to read it first: a model call spent reading what it had just
    /// seen. A file whose whole current content a shell command showed counts as read; one shown
    /// in part, changed since, or whose output was cleared from the conversation doesn't.
    #[test]
    fn a_file_a_shell_command_showed_in_full_counts_as_read() {
        let d = tempfile::tempdir().unwrap();
        let pack = d.path().join("pack.py");
        let config = d.path().join("config.py");
        std::fs::write(&pack, "def pack(w):\n    return [w]\n").unwrap();
        std::fs::write(&config, "TABLE = 1\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let files = FileState::default();
        assert!(files.check_writable(&pack).unwrap_err().contains("not been read"));
        // Shown in part: still unread.
        files.record_shown("t1", "def pack(w):\n... 1 line omitted ...");
        assert!(files.check_writable(&pack).unwrap_err().contains("not been read"));
        // Shown in full, among other output.
        files.record_shown("t2", "total 8\n---\ndef pack(w):\n    return [w]\n---\nitems.txt");
        files.check_writable(&pack).unwrap();
        // Changed since by a command: modified since read, until it is shown again as it is now.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&pack, "def pack(w):\n    return []\n").unwrap();
        assert!(files.check_writable(&pack).unwrap_err().contains("modified since read"));
        files.record_shown("t3", "def pack(w):\n    return []");
        files.check_writable(&pack).unwrap();
        // Output cleared from the conversation is no longer seen.
        files.record_shown("t4", "TABLE = 1");
        files.forget_views(&["t4".to_string()]);
        assert!(files.check_writable(&config).unwrap_err().contains("not been read"));
        files.record_shown("t5", "TABLE = 1");
        files.forget_all_views();
        assert!(files.check_writable(&config).unwrap_err().contains("not been read"));
    }

    #[test]
    fn a_file_made_during_the_session_may_be_written_unread() {
        let d = tempfile::tempdir().unwrap();
        let old = d.path().join("old.py");
        std::fs::write(&old, "x = 1\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let files = FileState::default();
        assert!(files.check_writable(&old).unwrap_err().contains("not been read"), "the person's file");
        let made = d.path().join("made.py");
        std::fs::write(&made, "y = 2\n").unwrap(); // as `cat > made.py` would
        if std::fs::metadata(&made).and_then(|m| m.created()).is_err() {
            return; // no creation times here: the strict rule holds, nothing more to check
        }
        assert!(files.check_writable(&made).is_ok(), "made by this session");
        // Read, then changed by something else: read it again, as for any file.
        files.record_read(&made);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&made, "y = 3\n").unwrap();
        assert!(files.check_writable(&made).unwrap_err().contains("modified since read"));
        // Put back by a checkpoint restore: read it first, however new.
        files.forget(&made);
        assert!(files.check_writable(&made).unwrap_err().contains("not been read"));
        files.record_read(&made);
        assert!(files.check_writable(&made).is_ok());
    }
}
