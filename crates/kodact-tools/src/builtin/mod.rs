//! Built-in tools.

pub(crate) mod bash;
mod edit;
mod exit_hints;
mod glob;
mod grep;
mod interact;
mod ls;
mod memory;
mod near;
mod notebook;
mod omission;
mod read;
mod todo;
mod web;
mod write;

use std::sync::Arc;

pub use bash::{exit_notice, still_running_notice, Bash, BashOutput, KillShell};
pub use edit::{apply_edit, apply_edit_near, apply_line_edit, Edit, MultiEdit};
pub use glob::Glob;
pub use grep::Grep;
pub use interact::{AskUserQuestion, EnterPlanMode, ExitPlanMode};
pub use ls::Ls;
pub use memory::{drop_checkpoint, notes_path, Checkpoint, Checkpoints, Notes, NotesStore};
pub use near::{NearKind, NearMatch};
pub use notebook::NotebookEdit;
pub use read::Read;
pub use todo::TodoWrite;
pub use web::{normalize_url, WebBackend, WebFetch, WebSearch};
pub use write::Write;

use crate::ToolRegistry;

/// The core file and shell tools.
pub fn register_core(reg: &mut ToolRegistry) {
    reg.register(Arc::new(Bash::default()));
    reg.register(Arc::new(BashOutput));
    reg.register(Arc::new(KillShell));
    reg.register(Arc::new(Glob));
    reg.register(Arc::new(Grep));
    reg.register(Arc::new(Ls));
    reg.register(Arc::new(Read));
    reg.register(Arc::new(Edit::default()));
    reg.register(Arc::new(MultiEdit));
    reg.register(Arc::new(Write));
    reg.register(Arc::new(NotebookEdit));
    reg.register(Arc::new(TodoWrite));
    reg.register(Arc::new(AskUserQuestion));
    reg.register(Arc::new(EnterPlanMode));
    reg.register(Arc::new(ExitPlanMode));
}

/// Give the registry's Bash tool the session's shell (description and read-only rules).
pub fn set_shell(reg: &mut ToolRegistry, shell: &crate::shells::ShellChoice) {
    reg.wrap("Bash", |_| Arc::new(Bash::new(shell)));
}

/// The Notes and Checkpoint tools (`tools.notes`, `tools.checkpoint`). Off by default: in the
/// first 79 measured runs models called Notes never and Checkpoint twice, and together they
/// add about 1,900 characters to every request (docs/goal.md, point 9).
pub fn set_memory_tools(reg: &mut ToolRegistry, notes: bool, checkpoint: bool) {
    if notes {
        reg.register(Arc::new(Notes));
    }
    if checkpoint {
        reg.register(Arc::new(Checkpoint));
    }
}

/// Edit's opt-in forms: naming the lines to replace (`edit.lineRanges`), and changing a file
/// never read when the call holds its exact text (`edit.withoutRead`).
pub fn set_edit_options(reg: &mut ToolRegistry, line_ranges: bool, unread: bool) {
    if line_ranges || unread {
        reg.wrap("Edit", move |_| Arc::new(Edit { line_ranges, unread }));
    }
}

fn str_arg<'a>(input: &'a serde_json::Value, key: &str) -> &'a str {
    input.get(key).and_then(serde_json::Value::as_str).unwrap_or("")
}

#[cfg(test)]
mod tests;
