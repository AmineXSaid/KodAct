use kodact_permissions::Subject;
use serde_json::{json, Value};

use super::str_arg;
use crate::{expand_path, Tool, ToolContext, ToolOutput};

pub struct Write;

#[async_trait::async_trait]
impl Tool for Write {
    fn name(&self) -> &str {
        "Write"
    }

    fn description(&self) -> String {
        "Write a file to the local filesystem, replacing it if it exists. `file_path` must be absolute.\n\n\
         - An existing file must have been read with Read first in this conversation.\n\
         - Prefer Edit for changes to existing files; Write sends the whole file.\n\
         - Do not create documentation or README files unless asked."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {"type": "string", "description": "The absolute path to the file to write"},
                "content": {"type": "string", "description": "The content to write to the file"}
            },
            "required": ["file_path", "content"],
            "additionalProperties": false
        })
    }

    fn permission_subject(&self, input: &Value, ctx: &ToolContext) -> Subject {
        Subject::Path { path: expand_path(str_arg(input, "file_path"), &ctx.project_dir), write: true }
    }

    fn validate(&self, input: &Value, ctx: &ToolContext) -> Result<(), String> {
        crate::validate_required(&self.input_schema(), input)?;
        let path = expand_path(str_arg(input, "file_path"), &ctx.project_dir);
        if path.is_dir() {
            return Err(format!("{} is a directory", path.display()));
        }
        ctx.files.check_writable(&path)
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let path = expand_path(str_arg(&input, "file_path"), &ctx.project_dir);
        let content = str_arg(&input, "content");
        let existed = path.exists();
        let old = if existed { std::fs::read_to_string(&path).ok() } else { None };
        ctx.checkpoint(&path);
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return ToolOutput::error(format!("Could not create {}: {e}", parent.display()));
            }
        }
        if let Err(e) = std::fs::write(&path, content) {
            return ToolOutput::error(format!("Could not write {}: {e}", path.display()));
        }
        ctx.files.record_write(&path);
        let structured = json!({
            "type": if existed { "update" } else { "create" },
            "filePath": path,
            "content": content,
            "originalFile": old,
            "structuredPatch": old.as_deref().map(|o| super::edit::patch_hunks(o, content)).unwrap_or_default(),
        });
        let msg = if existed {
            format!("The file {} has been updated.", path.display())
        } else {
            format!("File created successfully at: {}", path.display())
        };
        ToolOutput::text(msg).with_structured(structured)
    }
}
