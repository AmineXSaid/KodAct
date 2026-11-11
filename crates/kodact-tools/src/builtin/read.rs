use base64::Engine as _;
use kodact_permissions::Subject;
use kodact_types::{ContentBlock, MediaSource};
use serde_json::{json, Value};

use super::str_arg;
use crate::util::number_lines;
use crate::{expand_path, Tool, ToolContext, ToolOutput};

pub const DEFAULT_LINE_LIMIT: usize = 2000;
pub const MAX_LINE_CHARS: usize = 2000;
pub const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;
pub const MAX_PDF_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_PDF_PAGES_WITHOUT_RANGE: usize = 10;
/// Text files larger than this must be read with offset / limit.
pub const MAX_TEXT_BYTES: u64 = 256 * 1024;

pub struct Read;

fn image_type(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    })
}

/// Rough page count: number of `/Type /Page` objects (not `/Pages`).
pub fn pdf_page_count(bytes: &[u8]) -> usize {
    let re = regex::bytes::Regex::new(r"/Type\s*/Page[^s]").unwrap();
    re.find_iter(bytes).count()
}

#[async_trait::async_trait]
impl Tool for Read {
    fn name(&self) -> &str {
        "Read"
    }

    fn description(&self) -> String {
        format!(
            "Read a file from the local filesystem. `file_path` must be absolute.\n\n\
             - Reads up to {DEFAULT_LINE_LIMIT} lines from the start by default; use `offset` (1-based line) and \
               `limit` for large files.\n\
             - Lines longer than {MAX_LINE_CHARS} characters are truncated. Output is numbered like `cat -n`.\n\
             - Images (png, jpg, gif, webp) are returned as images; PDFs as documents (more than \
               {MAX_PDF_PAGES_WITHOUT_RANGE} pages need `pages`, e.g. \"1-5\"); Jupyter notebooks as cells with \
               outputs.\n\
             - Use LS or Bash `ls` for directories. Read several files in parallel when useful."
        )
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {"type": "string", "description": "The absolute path to the file to read"},
                "offset": {"type": "integer", "description": "The 1-based line number to start reading from"},
                "limit": {"type": "integer", "description": "The number of lines to read"},
                "pages": {"type": "string", "description": "Page range for PDF files, e.g. \"1-5\" (max 20 pages)"}
            },
            "required": ["file_path"],
            "additionalProperties": false
        })
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn permission_subject(&self, input: &Value, ctx: &ToolContext) -> Subject {
        Subject::Path { path: expand_path(str_arg(input, "file_path"), &ctx.project_dir), write: false }
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let path = expand_path(str_arg(&input, "file_path"), &ctx.project_dir);
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => {
                return ToolOutput::error(format!(
                    "File does not exist. Current working directory: {}.{}",
                    ctx.shell_cwd().display(),
                    crate::missing_path_hint(&path)
                ))
            }
        };
        if meta.is_dir() {
            return ToolOutput::error(format!(
                "EISDIR: {} is a directory. Use LS (or Bash `ls`) to list it.",
                path.display()
            ));
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();

        if let Some(mime) = image_type(&ext) {
            if meta.len() > MAX_IMAGE_BYTES {
                return ToolOutput::error(format!(
                    "Image is {} bytes; the limit is {MAX_IMAGE_BYTES} bytes. Resize it first.",
                    meta.len()
                ));
            }
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => return ToolOutput::error(e.to_string()),
            };
            ctx.files.record_read(&path);
            let data = base64::engine::general_purpose::STANDARD.encode(bytes);
            return ToolOutput::blocks(vec![ContentBlock::Image {
                source: MediaSource::Base64 { media_type: mime.into(), data },
                cache_control: None,
            }])
            .with_structured(
                json!({"type": "image", "file": {"filePath": path, "type": mime, "originalSize": meta.len()}}),
            );
        }

        if ext == "pdf" {
            if meta.len() > MAX_PDF_BYTES {
                return ToolOutput::error(format!("PDF is {} bytes; the limit is {MAX_PDF_BYTES} bytes.", meta.len()));
            }
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => return ToolOutput::error(e.to_string()),
            };
            let pages = pdf_page_count(&bytes);
            if pages > MAX_PDF_PAGES_WITHOUT_RANGE && input.get("pages").is_none() {
                return ToolOutput::error(format!(
                    "This PDF has {pages} pages. Provide `pages` (e.g. \"1-10\", at most 20 pages per request)."
                ));
            }
            ctx.files.record_read(&path);
            let data = base64::engine::general_purpose::STANDARD.encode(bytes);
            let mut blocks = vec![ContentBlock::Document {
                source: MediaSource::Base64 { media_type: "application/pdf".into(), data },
                title: path.file_name().map(|n| n.to_string_lossy().into_owned()),
                cache_control: None,
            }];
            if let Some(range) = input.get("pages").and_then(Value::as_str) {
                blocks.push(ContentBlock::text(format!("(Requested pages {range} of {pages}. Focus on that range.)")));
            }
            return ToolOutput::blocks(blocks)
                .with_structured(json!({"type": "pdf", "file": {"filePath": path, "pages": pages}}));
        }

        if ext == "ipynb" {
            let raw = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(e) => return ToolOutput::error(e.to_string()),
            };
            ctx.files.record_read(&path);
            return match super::notebook::render_notebook(&raw) {
                Ok(text) => ToolOutput::text(text),
                Err(e) => ToolOutput::error(format!("Could not parse notebook: {e}")),
            };
        }

        let offset = input.get("offset").and_then(Value::as_u64).map(|o| o.max(1) as usize);
        let limit = input.get("limit").and_then(Value::as_u64).map(|l| l as usize);
        if meta.len() > MAX_TEXT_BYTES && offset.is_none() && limit.is_none() {
            return ToolOutput::error(format!(
                "File content ({} KB) exceeds the maximum allowed size ({} KB). Use offset and limit to read \
                 specific portions, or Grep to search for specific content.",
                meta.len() / 1024,
                MAX_TEXT_BYTES / 1024
            ));
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => return ToolOutput::error(e.to_string()),
        };
        if bytes.iter().take(8000).any(|&b| b == 0) {
            return ToolOutput::error("This file appears to be binary and cannot be read as text.");
        }
        ctx.files.record_read(&path);
        // No repeated reads (GOALS pillar 1): the same range of unchanged content is still in context.
        let hash = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            bytes.hash(&mut h);
            h.finish()
        };
        let key_offset = offset.unwrap_or(1);
        if !ctx.tool_use_id.is_empty() {
            if ctx.files.same_view(&path, key_offset, limit, hash).is_some() {
                return ToolOutput::text(format!(
                    "<system-reminder>{} is unchanged since you last read these lines in this conversation, so \
                     they are not repeated here: that earlier result is still current. Read a different range if \
                     you need more.</system-reminder>",
                    path.display()
                ))
                .with_structured(json!({"type": "file_unchanged", "file": {"filePath": path}}));
            }
            ctx.files.record_view(&path, key_offset, limit, hash, &ctx.tool_use_id);
        }
        let text = String::from_utf8_lossy(&bytes);
        if text.is_empty() {
            return ToolOutput::text(
                "<system-reminder>Warning: the file exists but its contents are empty.</system-reminder>",
            );
        }
        let all: Vec<&str> = text.lines().collect();
        let start = offset.unwrap_or(1);
        if start > all.len() {
            return ToolOutput::text(format!(
                "<system-reminder>Warning: the file has only {} lines; offset {start} is past the end.</system-reminder>",
                all.len()
            ));
        }
        let count = limit.unwrap_or(DEFAULT_LINE_LIMIT);
        let slice: Vec<String> = all[start - 1..(start - 1 + count).min(all.len())]
            .iter()
            .map(|l| {
                if l.chars().count() > MAX_LINE_CHARS {
                    let cut: String = l.chars().take(MAX_LINE_CHARS).collect();
                    format!("{cut}... [line truncated]")
                } else {
                    l.to_string()
                }
            })
            .collect();
        let refs: Vec<&str> = slice.iter().map(String::as_str).collect();
        let mut out = number_lines(&refs, start);
        let shown_end = start - 1 + slice.len();
        if shown_end < all.len() && limit.is_none() {
            out.push_str(&format!(
                "\n<system-reminder>Showing lines {start}-{shown_end} of {}. Use offset/limit to read more.</system-reminder>",
                all.len()
            ));
        }
        ToolOutput::text(out).with_structured(json!({
            "type": "text",
            "file": {"filePath": path, "numLines": slice.len(), "startLine": start, "totalLines": all.len()}
        }))
    }
}
