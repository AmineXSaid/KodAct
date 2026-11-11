use kodact_permissions::Subject;
use serde_json::{json, Value};

use super::str_arg;
use crate::{expand_path, Tool, ToolContext, ToolOutput};

fn source_text(v: &Value) -> String {
    match v {
        Value::Array(a) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(""),
        Value::String(s) => s.clone(),
        _ => String::new(),
    }
}

/// Render a notebook for Read: each cell with its id, type, source and text outputs.
pub fn render_notebook(raw: &str) -> Result<String, String> {
    let nb: Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    let lang = nb.pointer("/metadata/language_info/name").and_then(Value::as_str).unwrap_or("python");
    let mut out = String::new();
    for (i, cell) in nb.get("cells").and_then(Value::as_array).into_iter().flatten().enumerate() {
        let id = cell.get("id").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("cell-{i}"));
        let ty = cell.get("cell_type").and_then(Value::as_str).unwrap_or("code");
        out.push_str(&format!(
            "<cell id=\"{id}\" type=\"{ty}\"{}>\n",
            if ty == "code" { format!(" language=\"{lang}\"") } else { String::new() }
        ));
        out.push_str(&source_text(cell.get("source").unwrap_or(&Value::Null)));
        out.push_str("\n</cell>\n");
        for o in cell.get("outputs").and_then(Value::as_array).into_iter().flatten() {
            let text = o
                .get("text")
                .map(source_text)
                .or_else(|| o.pointer("/data/text~1plain").map(source_text))
                .or_else(|| o.get("traceback").map(source_text))
                .unwrap_or_else(|| {
                    if o.pointer("/data/image~1png").is_some() {
                        "[image output]".into()
                    } else {
                        String::new()
                    }
                });
            if !text.is_empty() {
                out.push_str(&format!("<output cell=\"{id}\">\n{text}\n</output>\n"));
            }
        }
    }
    Ok(out)
}

pub struct NotebookEdit;

#[async_trait::async_trait]
impl Tool for NotebookEdit {
    fn name(&self) -> &str {
        "NotebookEdit"
    }

    fn description(&self) -> String {
        "Edit a Jupyter notebook (.ipynb) cell. `edit_mode`: \"replace\" (default) replaces the source of \
         `cell_id`; \"insert\" adds a new cell after `cell_id` (or at the start) and needs `cell_type`; \
         \"delete\" removes `cell_id`. Read the notebook first to see cell ids."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "notebook_path": {"type": "string", "description": "The absolute path to the notebook"},
                "cell_id": {"type": "string", "description": "The id of the cell to edit (or to insert after)"},
                "new_source": {"type": "string", "description": "The new source for the cell"},
                "cell_type": {"type": "string", "enum": ["code", "markdown"]},
                "edit_mode": {"type": "string", "enum": ["replace", "insert", "delete"]}
            },
            "required": ["notebook_path", "new_source"],
            "additionalProperties": false
        })
    }

    fn permission_subject(&self, input: &Value, ctx: &ToolContext) -> Subject {
        Subject::Path { path: expand_path(str_arg(input, "notebook_path"), &ctx.project_dir), write: true }
    }

    fn validate(&self, input: &Value, ctx: &ToolContext) -> Result<(), String> {
        crate::validate_required(&self.input_schema(), input)?;
        let path = expand_path(str_arg(input, "notebook_path"), &ctx.project_dir);
        if path.extension().and_then(|e| e.to_str()) != Some("ipynb") {
            return Err("File must be a Jupyter notebook (.ipynb)".into());
        }
        ctx.files.check_writable(&path)
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let path = expand_path(str_arg(&input, "notebook_path"), &ctx.project_dir);
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => return ToolOutput::error(format!("Could not read notebook: {e}")),
        };
        let mut nb: Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => return ToolOutput::error(format!("Notebook is not valid JSON: {e}")),
        };
        let mode = match str_arg(&input, "edit_mode") {
            "" => "replace",
            m => m,
        };
        let cell_id = input.get("cell_id").and_then(Value::as_str);
        let source = str_arg(&input, "new_source");
        let Some(cells) = nb.get_mut("cells").and_then(Value::as_array_mut) else {
            return ToolOutput::error("Notebook has no cells array");
        };
        let find = |cells: &Vec<Value>, id: &str| {
            cells
                .iter()
                .position(|c| c.get("id").and_then(Value::as_str) == Some(id))
                .or_else(|| id.strip_prefix("cell-").and_then(|n| n.parse::<usize>().ok()).filter(|&n| n < cells.len()))
        };
        let lines: Vec<Value> = source.split_inclusive('\n').map(|l| json!(l)).collect();
        // The ids a call can name, for an id that isn't one of them.
        let known: Vec<String> = cells
            .iter()
            .enumerate()
            .map(|(i, c)| {
                c.get("id").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("cell-{i}"))
            })
            .collect();
        let not_found = || {
            let shown: Vec<&str> = known.iter().take(30).map(String::as_str).collect();
            let more = if known.len() > shown.len() {
                format!(" (and {} more)", known.len() - shown.len())
            } else {
                String::new()
            };
            ToolOutput::error(format!(
                "cell_id {:?} not found. The notebook has {} cell(s): {}{more}. Read the notebook to see them; \
                 `cell-N` names the Nth cell from 0.",
                cell_id.unwrap_or(""),
                known.len(),
                shown.join(", ")
            ))
        };
        match mode {
            "replace" => {
                let Some(i) = cell_id.and_then(|id| find(cells, id)) else {
                    return not_found();
                };
                cells[i]["source"] = Value::Array(lines);
                if let Some(t) = input.get("cell_type").and_then(Value::as_str) {
                    cells[i]["cell_type"] = json!(t);
                }
                if cells[i]["cell_type"] == "code" {
                    cells[i]["outputs"] = json!([]);
                    cells[i]["execution_count"] = Value::Null;
                }
            }
            "insert" => {
                let ty = input.get("cell_type").and_then(Value::as_str).unwrap_or("code");
                let at = match cell_id {
                    Some(id) => match find(cells, id) {
                        Some(i) => i + 1,
                        None => return not_found(),
                    },
                    None => 0,
                };
                let mut cell = json!({"cell_type": ty, "id": uuid::Uuid::new_v4().simple().to_string()[..8], "metadata": {}, "source": lines});
                if ty == "code" {
                    cell["outputs"] = json!([]);
                    cell["execution_count"] = Value::Null;
                }
                cells.insert(at, cell);
            }
            "delete" => {
                let Some(i) = cell_id.and_then(|id| find(cells, id)) else {
                    return not_found();
                };
                cells.remove(i);
            }
            other => return ToolOutput::error(format!("Unknown edit_mode {other}")),
        }
        ctx.checkpoint(&path);
        let text = serde_json::to_string_pretty(&nb).unwrap_or_default() + "\n";
        if let Err(e) = std::fs::write(&path, text) {
            return ToolOutput::error(format!("Could not write notebook: {e}"));
        }
        ctx.files.record_write(&path);
        let count = nb.get("cells").and_then(Value::as_array).map_or(0, Vec::len);
        ToolOutput::text(format!("Notebook {} updated ({mode}); it now has {count} cell(s).", path.display()))
            .with_structured(json!({
                "notebookPath": path, "editMode": mode, "cellId": cell_id, "cellType": input.get("cell_type"),
                "numCells": count,
            }))
    }
}
