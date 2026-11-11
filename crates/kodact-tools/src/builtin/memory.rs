//! What the model keeps for itself across a long task (GOALS pillar 5): notes that outlast
//! the context, and checkpoints of its own edits it can go back to.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use kodact_permissions::Subject;
use serde_json::{json, Value};

use super::str_arg;
use crate::{Tool, ToolContext, ToolOutput};

/// The session's notes: kept in memory and, when the session is persisted, in a file next to
/// its transcript (`<session>.notes.md`), so a resumed session has them and a person can read
/// them.
#[derive(Default)]
pub struct NotesStore {
    /// `None` until first used (then loaded from the file, if there is one).
    text: Mutex<Option<String>>,
}

/// Where a session's notes are kept, given its transcript.
pub fn notes_path(transcript: &Path) -> PathBuf {
    transcript.with_extension("notes.md")
}

impl NotesStore {
    pub fn get(&self, transcript: Option<&Path>) -> String {
        let mut t = self.text.lock().unwrap();
        t.get_or_insert_with(|| {
            transcript.and_then(|p| std::fs::read_to_string(notes_path(p)).ok()).unwrap_or_default()
        })
        .clone()
    }

    fn set(&self, transcript: Option<&Path>, text: String) -> Result<(), String> {
        if let Some(p) = transcript.map(notes_path) {
            std::fs::write(&p, &text).map_err(|e| format!("Could not save the notes to {}: {e}", p.display()))?;
        }
        *self.text.lock().unwrap() = Some(text);
        Ok(())
    }
}

pub struct Notes;

#[async_trait::async_trait]
impl Tool for Notes {
    fn name(&self) -> &str {
        "Notes"
    }

    fn description(&self) -> String {
        "Keep notes that outlast this conversation's context. On a long task, older messages are summarized \
         or dropped to make room; your notes are given back to you in full after that, and when the session \
         is resumed. Write what you would need to carry on without rereading everything: what you found and \
         where, decisions and their reasons, what was tried and failed, what is left to do and how it will be \
         checked. `append` adds a note, `replace` rewrites them all (to tidy or drop what is settled), `read` \
         returns them. Not for the task list (TodoWrite), and not for what the code or the conversation \
         already says plainly: a few lines at each turning point, not a log."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["append", "replace", "read"]},
                "text": {"type": "string", "description": "The note to add, or the new notes (not for read)"}
            },
            "required": ["action"],
            "additionalProperties": false
        })
    }

    /// Changes only the session's own notes, never the project.
    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    fn validate(&self, input: &Value, _ctx: &ToolContext) -> Result<(), String> {
        crate::validate_required(&self.input_schema(), input)?;
        match str_arg(input, "action") {
            "read" => Ok(()),
            "append" | "replace" if input.get("text").and_then(Value::as_str).is_some() => Ok(()),
            "append" | "replace" => Err("`text` is required for append and replace.".into()),
            other => Err(format!("Unknown action {other:?}: use append, replace or read.")),
        }
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let transcript = ctx.transcript_path.as_deref();
        let old = ctx.notes.get(transcript);
        let text = str_arg(&input, "text").trim();
        let new = match str_arg(&input, "action") {
            "read" => {
                let shown = if old.is_empty() { "(no notes yet)".to_string() } else { old.clone() };
                return ToolOutput::text(shown).with_structured(json!({"notes": old}));
            }
            "append" if old.is_empty() => text.to_string(),
            "append" => format!("{old}\n{text}"),
            _ => text.to_string(),
        };
        if let Err(e) = ctx.notes.set(transcript, new.clone()) {
            return ToolOutput::error(e);
        }
        ToolOutput::text(format!(
            "Notes saved ({} lines, {} characters). They will be given back to you after a compaction and on \
             resume.",
            new.lines().count(),
            new.chars().count()
        ))
        .with_structured(json!({"notes": new}))
    }
}

/// One checkpoint: for each file a tool changed after it was saved, the file as it was then
/// (`None`: it did not exist).
pub struct Saved {
    pub label: String,
    files: HashMap<PathBuf, Option<Vec<u8>>>,
    /// In the order they were first changed.
    order: Vec<PathBuf>,
}

/// The model's checkpoints, oldest first. Kept in memory: they end with the process.
#[derive(Default)]
pub struct Checkpoints {
    saved: Mutex<Vec<Saved>>,
}

impl Checkpoints {
    /// A tool is about to write `path`: every checkpoint that has not seen it changed yet
    /// keeps it as it is now.
    pub fn before_write(&self, path: &Path) {
        let mut saved = self.saved.lock().unwrap();
        if saved.iter().all(|s| s.files.contains_key(path)) {
            return;
        }
        let now = std::fs::read(path).ok();
        for s in saved.iter_mut().filter(|s| !s.files.contains_key(path)) {
            s.files.insert(path.to_path_buf(), now.clone());
            s.order.push(path.to_path_buf());
        }
    }

    fn labels(&self) -> Vec<(String, usize)> {
        self.saved.lock().unwrap().iter().map(|s| (s.label.clone(), s.files.len())).collect()
    }
}

/// In a git repository a checkpoint is the whole working tree, kept under a ref of the
/// session's own (`refs/kodact/checkpoints/<session>/<n>`, the label as its subject): it
/// covers what shell commands changed too, and a resumed session still has it. The index,
/// HEAD and the branches are never touched.
struct GitCheckpoints {
    repo: PathBuf,
    prefix: String,
}

impl GitCheckpoints {
    fn of(ctx: &ToolContext) -> Option<Self> {
        let repo = kodact_git::repo_root(&ctx.project_dir)?;
        let session = if ctx.session_id.is_empty() { "local" } else { ctx.session_id.as_str() };
        Some(GitCheckpoints { repo, prefix: format!("refs/kodact/checkpoints/{session}/") })
    }

    /// `(ref, label)`, oldest first.
    fn list(&self) -> Vec<(String, String)> {
        kodact_git::refs_under(&self.repo, &self.prefix)
    }

    fn save(&self, label: &str) -> Result<(), String> {
        let next = self.list().last().and_then(|(r, _)| r.rsplit('/').next()?.parse::<u32>().ok()).unwrap_or(0) + 1;
        kodact_git::save_worktree(&self.repo, &format!("{}{next:04}", self.prefix), label)
    }
}

/// The checkpoints a call can name: git's in a repository, the in-memory ones elsewhere.
fn known_labels(ctx: &ToolContext) -> Vec<String> {
    match GitCheckpoints::of(ctx) {
        Some(g) => g.list().into_iter().map(|(_, l)| l).collect(),
        None => ctx.checkpoints.labels().into_iter().map(|(l, _)| l).collect(),
    }
}

fn shown(paths: &[PathBuf]) -> Vec<String> {
    paths.iter().map(|p| p.display().to_string()).collect()
}

pub struct Checkpoint;

#[async_trait::async_trait]
impl Tool for Checkpoint {
    fn name(&self) -> &str {
        "Checkpoint"
    }

    fn description(&self) -> String {
        "Save a point you can bring the project's files back to, and go back to it. `save` (with a short \
         `label`) before trying something you may want to undo: a risky refactor, one of several approaches. \
         `restore` puts the files back as they were at that checkpoint (files created since are removed) \
         and drops the checkpoints saved after it. `list` shows them. Use it instead of undoing changes one \
         by one when an approach fails. In a git repository it covers every file git does not ignore, \
         whatever changed it (your edits, shell commands), without touching the index or any branch, and a \
         resumed session still has its checkpoints. Outside one it covers the files you change with Edit, \
         MultiEdit, Write or NotebookEdit, for this run of the session. Commits are for work to keep."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["save", "restore", "list"]},
                "label": {"type": "string", "description": "The checkpoint's name (save, restore)"}
            },
            "required": ["action"],
            "additionalProperties": false
        })
    }

    /// Only `restore` changes files.
    fn is_read_only(&self, input: &Value) -> bool {
        str_arg(input, "action") != "restore"
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    fn permission_subject(&self, input: &Value, _ctx: &ToolContext) -> Subject {
        Subject::Name(str_arg(input, "action").to_string())
    }

    fn validate(&self, input: &Value, ctx: &ToolContext) -> Result<(), String> {
        crate::validate_required(&self.input_schema(), input)?;
        let label = str_arg(input, "label").trim();
        let known = known_labels(ctx);
        let names = || match known.is_empty() {
            true => "No checkpoint has been saved in this session.".to_string(),
            false => format!("Saved: {}.", known.join(", ")),
        };
        match str_arg(input, "action") {
            "list" => Ok(()),
            "save" | "restore" if label.is_empty() => Err("`label` is required for save and restore.".into()),
            "save" if known.iter().any(|l| l == label) => {
                Err(format!("A checkpoint named {label:?} exists already; pick another label. {}", names()))
            }
            "save" => Ok(()),
            "restore" if known.iter().any(|l| l == label) => Ok(()),
            "restore" => Err(format!("No checkpoint named {label:?}. {}", names())),
            other => Err(format!("Unknown action {other:?}: use save, restore or list.")),
        }
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let label = str_arg(&input, "label").trim().to_string();
        if let Some(git) = GitCheckpoints::of(ctx) {
            return git_call(&git, str_arg(&input, "action"), &label, ctx);
        }
        match str_arg(&input, "action") {
            "save" => {
                ctx.checkpoints.saved.lock().unwrap().push(Saved {
                    label: label.clone(),
                    files: HashMap::new(),
                    order: vec![],
                });
                ToolOutput::text(format!(
                    "Checkpoint {label:?} saved. `restore` brings back the files you change with the edit tools \
                     from here on (not what shell commands change)."
                ))
                .with_structured(json!({"action": "save", "label": label}))
            }
            "restore" => {
                // Taken out with the checkpoints after it; the one restored to starts over once the
                // files are back.
                let taken: Vec<Saved> = {
                    let mut saved = ctx.checkpoints.saved.lock().unwrap();
                    let Some(at) = saved.iter().position(|s| s.label == label) else {
                        return ToolOutput::error(format!("No checkpoint named {label:?}."));
                    };
                    let mut taken: Vec<Saved> = saved.drain(at..).collect();
                    taken.truncate(1);
                    taken
                };
                let s = &taken[0];
                let (mut restored, mut removed, mut failed) = (vec![], vec![], vec![]);
                for path in &s.order {
                    // The user's own checkpoints (rewind) and earlier ones of the model see this
                    // as one more write.
                    ctx.checkpoint(path);
                    let done = match &s.files[path] {
                        Some(bytes) => std::fs::write(path, bytes).map(|_| restored.push(path.display().to_string())),
                        None if path.exists() => {
                            std::fs::remove_file(path).map(|_| removed.push(path.display().to_string()))
                        }
                        None => Ok(()),
                    };
                    if let Err(e) = done {
                        failed.push(format!("{}: {e}", path.display()));
                    }
                    // What the model last read of it is no longer what is there.
                    ctx.files.forget(path);
                }
                ctx.checkpoints.saved.lock().unwrap().push(Saved {
                    label: label.clone(),
                    files: HashMap::new(),
                    order: vec![],
                });
                let mut text = if restored.is_empty() && removed.is_empty() && failed.is_empty() {
                    format!("Nothing to restore: no file was changed with the edit tools since checkpoint {label:?}.")
                } else {
                    format!("Restored to checkpoint {label:?}.")
                };
                for (what, list) in [("Put back", &restored), ("Removed (created since)", &removed)] {
                    if !list.is_empty() {
                        text.push_str(&format!("\n{what}: {}", list.join(", ")));
                    }
                }
                if !restored.is_empty() {
                    text.push_str("\nRead a restored file again before editing it.");
                }
                text.push_str(
                    "\nChanges made by shell commands since then (generated files, moves, installs) are not undone.",
                );
                let structured = json!({"action": "restore", "label": label, "restored": restored, "removed": removed, "failed": failed});
                if failed.is_empty() {
                    ToolOutput::text(text).with_structured(structured)
                } else {
                    ToolOutput::error(format!("{text}\nCould not restore: {}", failed.join("; ")))
                        .with_structured(structured)
                }
            }
            _ => {
                let known = ctx.checkpoints.labels();
                let text = if known.is_empty() {
                    "No checkpoint has been saved in this session.".to_string()
                } else {
                    known
                        .iter()
                        .map(|(l, n)| format!("- {l}: {n} file(s) changed since"))
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                ToolOutput::text(text).with_structured(json!({
                    "action": "list",
                    "checkpoints": known.iter().map(|(l, n)| json!({"label": l, "filesChanged": n})).collect::<Vec<_>>(),
                }))
            }
        }
    }
}

fn git_call(git: &GitCheckpoints, action: &str, label: &str, ctx: &ToolContext) -> ToolOutput {
    match action {
        "save" => match git.save(label) {
            Ok(()) => ToolOutput::text(format!(
                "Checkpoint {label:?} saved: every file git does not ignore, as it is now. `restore` brings \
                 them back, whatever changes them from here on."
            ))
            .with_structured(json!({"action": "save", "label": label})),
            Err(e) => ToolOutput::error(format!("Could not save the checkpoint: {e}")),
        },
        "restore" => {
            let all = git.list();
            let Some(at) = all.iter().position(|(_, l)| l == label) else {
                return ToolOutput::error(format!("No checkpoint named {label:?}."));
            };
            let saved = &all[at].0;
            let (restore, remove) = match kodact_git::worktree_changes_since(&git.repo, saved) {
                Ok(c) => c,
                Err(e) => return ToolOutput::error(format!("Could not compare with the checkpoint: {e}")),
            };
            // The person's own checkpoints (rewind) see this as one more write of each file.
            for p in restore.iter().chain(&remove) {
                ctx.checkpoint(p);
            }
            if let Err(e) = kodact_git::restore_worktree(&git.repo, saved, &restore, &remove) {
                return ToolOutput::error(format!("Could not restore checkpoint {label:?}: {e}"));
            }
            for p in restore.iter().chain(&remove) {
                // What the model last read of it is no longer what is there.
                ctx.files.forget(p);
            }
            for (later, _) in &all[at + 1..] {
                let _ = kodact_git::delete_ref(&git.repo, later);
            }
            let (restored, removed) = (shown(&restore), shown(&remove));
            let mut text = if restored.is_empty() && removed.is_empty() {
                format!("Nothing to restore: no file differs from checkpoint {label:?}.")
            } else {
                format!("Restored to checkpoint {label:?}.")
            };
            for (what, list) in [("Put back", &restored), ("Removed (created since)", &removed)] {
                if !list.is_empty() {
                    let more = if list.len() > 40 { format!(" (and {} more)", list.len() - 40) } else { String::new() };
                    text.push_str(&format!(
                        "\n{what}: {}{more}",
                        list.iter().take(40).cloned().collect::<Vec<_>>().join(", ")
                    ));
                }
            }
            if !restored.is_empty() {
                text.push_str("\nRead a restored file again before editing it.");
            }
            text.push_str("\nFiles git ignores (build output, installed packages) are as they were left.");
            ToolOutput::text(text).with_structured(
                json!({"action": "restore", "label": label, "restored": restored, "removed": removed, "failed": []}),
            )
        }
        _ => {
            let all = git.list();
            let rows: Vec<(String, usize)> = all
                .iter()
                .map(|(r, l)| {
                    let n =
                        kodact_git::worktree_changes_since(&git.repo, r).map(|(a, b)| a.len() + b.len()).unwrap_or(0);
                    (l.clone(), n)
                })
                .collect();
            let text = if rows.is_empty() {
                "No checkpoint has been saved in this session.".to_string()
            } else {
                rows.iter().map(|(l, n)| format!("- {l}: {n} file(s) changed since")).collect::<Vec<_>>().join("\n")
            };
            ToolOutput::text(text).with_structured(json!({
                "action": "list",
                "checkpoints": rows.iter().map(|(l, n)| json!({"label": l, "filesChanged": n})).collect::<Vec<_>>(),
            }))
        }
    }
}
