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

pub struct Checkpoint;

#[async_trait::async_trait]
impl Tool for Checkpoint {
    fn name(&self) -> &str {
        "Checkpoint"
    }

    fn description(&self) -> String {
        "Save a point you can bring your file changes back to, and go back to it. `save` (with a short \
         `label`) before trying something you may want to undo: a risky refactor, one of several approaches. \
         `restore` puts every file you changed with Edit, MultiEdit, Write or NotebookEdit since that \
         checkpoint back as it was then (files you created since are removed), and drops the checkpoints \
         saved after it. `list` shows them. Use it instead of undoing edits one by one when an approach \
         fails. It does not undo what shell commands changed, and checkpoints last only for this run of \
         the session; commits are for work to keep."
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
        let known = ctx.checkpoints.labels();
        let names = || match known.is_empty() {
            true => "No checkpoint has been saved in this run of the session.".to_string(),
            false => format!("Saved: {}.", known.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>().join(", ")),
        };
        match str_arg(input, "action") {
            "list" => Ok(()),
            "save" | "restore" if label.is_empty() => Err("`label` is required for save and restore.".into()),
            "save" if known.iter().any(|(l, _)| l == label) => {
                Err(format!("A checkpoint named {label:?} exists already; pick another label. {}", names()))
            }
            "save" => Ok(()),
            "restore" if known.iter().any(|(l, _)| l == label) => Ok(()),
            "restore" => Err(format!("No checkpoint named {label:?}. {}", names())),
            other => Err(format!("Unknown action {other:?}: use save, restore or list.")),
        }
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let label = str_arg(&input, "label").trim().to_string();
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
                    "No checkpoint has been saved in this run of the session.".to_string()
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
