use std::path::PathBuf;
use std::time::SystemTime;

use globset::GlobBuilder;
use kodact_permissions::Subject;
use serde_json::{json, Value};

use super::str_arg;
use crate::{expand_path, Tool, ToolContext, ToolOutput};

pub const MAX_RESULTS: usize = 100;

pub struct Glob;

fn search_root(input: &Value, ctx: &ToolContext) -> PathBuf {
    match input.get("path").and_then(Value::as_str).filter(|p| !p.trim().is_empty() && *p != "undefined") {
        Some(p) => expand_path(p, &ctx.shell_cwd()),
        None => ctx.shell_cwd(),
    }
}

#[async_trait::async_trait]
impl Tool for Glob {
    fn name(&self) -> &str {
        "Glob"
    }

    fn description(&self) -> String {
        format!(
            "Find files by glob pattern (e.g. \"**/*.rs\", \"src/**/*.{{ts,tsx}}\"). Returns matching paths, most \
             recently modified first, at most {MAX_RESULTS}. Respects .gitignore. Use Grep to search file contents."
        )
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "The glob pattern to match files against"},
                "path": {"type": "string", "description": "Directory to search in (defaults to the current directory)"}
            },
            "required": ["pattern"],
            "additionalProperties": false
        })
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn permission_subject(&self, input: &Value, ctx: &ToolContext) -> Subject {
        Subject::Path { path: search_root(input, ctx), write: false }
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let root = search_root(&input, ctx);
        if !root.is_dir() {
            return ToolOutput::error(format!(
                "Directory does not exist: {}.{}",
                root.display(),
                crate::missing_path_hint(&root)
            ));
        }
        let pattern = str_arg(&input, "pattern").trim().to_string();
        let (base, pat) = if pattern.starts_with('/') {
            (PathBuf::from("/"), pattern.trim_start_matches('/').to_string())
        } else {
            (root.clone(), pattern.clone())
        };
        let glob = match GlobBuilder::new(&pat).literal_separator(true).build() {
            Ok(g) => g.compile_matcher(),
            Err(e) => return ToolOutput::error(format!("Invalid glob pattern: {e}")),
        };
        let start = std::time::Instant::now();
        let cancel = ctx.cancel.clone();
        let walk_root = root.clone();
        let searched = base.display().to_string();
        let found = tokio::task::spawn_blocking(move || {
            let mut out: Vec<(PathBuf, SystemTime)> = vec![];
            for entry in ignore::WalkBuilder::new(&walk_root)
                .hidden(false)
                .filter_entry(|e| e.file_name() != ".git")
                .build()
                .flatten()
            {
                if cancel.is_cancelled() {
                    break;
                }
                if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    continue;
                }
                let path = entry.path();
                let rel = path.strip_prefix(&base).unwrap_or(path);
                if glob.is_match(rel) {
                    let m = entry.metadata().ok().and_then(|m| m.modified().ok()).unwrap_or(SystemTime::UNIX_EPOCH);
                    out.push((path.to_path_buf(), m));
                }
            }
            out
        })
        .await
        .unwrap_or_default();
        let mut found = found;
        found.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let total = found.len();
        let truncated = total > MAX_RESULTS;
        let files: Vec<String> = found.into_iter().take(MAX_RESULTS).map(|(p, _)| p.display().to_string()).collect();
        let structured = json!({
            "filenames": files, "numFiles": files.len(), "truncated": truncated,
            "durationMs": start.elapsed().as_millis() as u64,
        });
        if files.is_empty() {
            // What the search covered, and the usual reason a pattern finds nothing.
            let deeper = if pat.contains("**") || pat.contains('/') {
                String::new()
            } else {
                format!(" A pattern without `**` matches only directly in that directory: try `**/{pat}`.")
            };
            return ToolOutput::text(format!(
                "No files found for `{pattern}` under {searched} (files ignored by .gitignore are not searched).{deeper}"
            ))
            .with_structured(structured);
        }
        let mut text = files.join("\n");
        if truncated {
            text.push_str(&format!(
                "\n(Results are truncated: showing {MAX_RESULTS} of {total}. Use a more specific path or pattern.)"
            ));
        }
        ToolOutput::text(text).with_structured(structured)
    }
}
