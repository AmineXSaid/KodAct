use std::path::Path;

use globset::{Glob, GlobSetBuilder};
use kodact_permissions::Subject;
use serde_json::{json, Value};

use super::str_arg;
use crate::{expand_path, Tool, ToolContext, ToolOutput};

pub const MAX_ENTRIES: usize = 1000;

pub struct Ls;

#[async_trait::async_trait]
impl Tool for Ls {
    fn name(&self) -> &str {
        "LS"
    }

    fn description(&self) -> String {
        "List a directory as a tree. `path` must be absolute. `ignore` takes glob patterns to skip. Hidden \
         entries are skipped. Prefer Glob or Grep when you know what you are looking for."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "The absolute path of the directory to list"},
                "ignore": {"type": "array", "items": {"type": "string"}, "description": "Glob patterns to ignore"}
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn permission_subject(&self, input: &Value, ctx: &ToolContext) -> Subject {
        Subject::Path { path: expand_path(str_arg(input, "path"), &ctx.project_dir), write: false }
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let root = expand_path(str_arg(&input, "path"), &ctx.project_dir);
        if !root.is_dir() {
            return ToolOutput::error(if root.is_file() {
                format!("{} is a file, not a directory: Read it, or list its directory.", root.display())
            } else {
                format!("Directory does not exist: {}.{}", root.display(), crate::missing_path_hint(&root))
            });
        }
        let mut gb = GlobSetBuilder::new();
        for g in input.get("ignore").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
            if let Ok(g) = Glob::new(g) {
                gb.add(g);
            }
        }
        let ignore = gb.build().unwrap_or_else(|_| GlobSetBuilder::new().build().unwrap());
        let mut lines = vec![format!("- {}/", root.display())];
        let mut count = 0usize;
        walk(&root, &root, 1, &ignore, &mut lines, &mut count);
        if count >= MAX_ENTRIES {
            lines.insert(0, format!("There are more than {MAX_ENTRIES} entries; showing the first {MAX_ENTRIES}. Use Glob or a more specific path.\n"));
        }
        let truncated = count >= MAX_ENTRIES;
        if count == 0 {
            lines.push("  (empty, or everything in it is hidden or ignored)".into());
        } else if !truncated {
            lines.push(format!("\n{count} entries."));
        }
        ToolOutput::text(lines.join("\n")).with_structured(json!({
            "path": root, "numEntries": count, "truncated": truncated,
        }))
    }
}

fn walk(root: &Path, dir: &Path, depth: usize, ignore: &globset::GlobSet, out: &mut Vec<String>, count: &mut usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if *count >= MAX_ENTRIES {
            return;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.')
            || name == "node_modules"
            || name == "target" && depth == 1 && root.join("Cargo.toml").exists()
        {
            continue;
        }
        let path = e.path();
        let rel = path.strip_prefix(root).unwrap_or(&path);
        if ignore.is_match(rel) || ignore.is_match(&name) {
            continue;
        }
        *count += 1;
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        out.push(format!("{}- {}{}", "  ".repeat(depth), name, if is_dir { "/" } else { "" }));
        if is_dir {
            walk(root, &path, depth + 1, ignore, out, count);
        }
    }
}
