use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// Which files the model has read, and when: Edit and Write refuse to touch
/// an existing file the model has not read, or that changed since. A file
/// created after the session started (by a shell command, a script the model
/// ran) is not one of the person's files to protect: it may be written unread.
#[derive(Debug)]
pub struct FileState {
    reads: Mutex<HashMap<PathBuf, Option<SystemTime>>>,
    /// What each Read showed: (path, offset, limit) -> (content hash, tool_use_id).
    views: Mutex<HashMap<ViewKey, (u64, String)>>,
    /// When the session started: files created after it are the session's own.
    started: SystemTime,
    /// Files that must be read again whatever their age (put back by a checkpoint restore).
    unseen: Mutex<HashSet<PathBuf>>,
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
        }
    }
}

const CLOCK_TICK: std::time::Duration = std::time::Duration::from_millis(20);

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
    }

    /// After a full compaction nothing earlier is in view.
    pub fn forget_all_views(&self) {
        self.views.lock().unwrap().clear();
    }

    /// `Ok` when an existing file may be modified.
    pub fn check_writable(&self, path: &Path) -> Result<(), String> {
        if !path.exists() {
            return Ok(());
        }
        match self.reads.lock().unwrap().get(path) {
            // Made during this session and not since put back from a checkpoint: the session's own.
            None if created_after(path, self.started) && !self.unseen.lock().unwrap().contains(path) => Ok(()),
            None => Err("File has not been read yet. Read it first before writing to it.".into()),
            Some(seen) if *seen != mtime(path) => Err(
                "File has been modified since read, either by the user or by a linter. Read it again before attempting to write it."
                    .into(),
            ),
            Some(_) => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
