use async_trait::async_trait;
use serde_json::json;

use super::{schema_object, Tool, ToolContext, ToolResult};
use crate::channels::Attachment;
use crate::error::Result;

pub struct SendFileTool;

fn guess_mime(ext: &str) -> Option<&'static str> {
    match ext {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "svg" => Some("image/svg+xml"),
        "bmp" => Some("image/bmp"),
        "mp3" => Some("audio/mpeg"),
        "ogg" | "oga" => Some("audio/ogg"),
        "wav" => Some("audio/wav"),
        "flac" => Some("audio/flac"),
        "mp4" => Some("video/mp4"),
        "webm" => Some("video/webm"),
        "avi" => Some("video/x-msvideo"),
        "mkv" => Some("video/x-matroska"),
        "pdf" => Some("application/pdf"),
        "zip" => Some("application/zip"),
        "tar" => Some("application/x-tar"),
        "gz" => Some("application/gzip"),
        "json" => Some("application/json"),
        "csv" => Some("text/csv"),
        "txt" => Some("text/plain"),
        "html" | "htm" => Some("text/html"),
        "xml" => Some("application/xml"),
        "doc" => Some("application/msword"),
        "docx" => Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        _ => None,
    }
}

#[async_trait]
impl Tool for SendFileTool {
    fn name(&self) -> &str {
        "send_file"
    }

    fn description(&self) -> &str {
        "Queue a file to be sent as media (image, audio, video, or document) in the response. \
         Path is relative to the current directory. MIME type is auto-detected from extension \
         but can be overridden."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        schema_object(
            json!({
                "path": {
                    "type": "string",
                    "description": "File path relative to current directory"
                },
                "mime_type": {
                    "type": "string",
                    "description": "Optional MIME type override (e.g. 'image/png'). Auto-detected from extension if omitted."
                }
            }),
            &["path"],
        )
    }

    async fn execute(&self, params: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult> {
        let path = params["path"].as_str().unwrap_or_default();
        if path.is_empty() {
            return Ok(ToolResult::error("path is required"));
        }

        let cwd = ctx.cwd.lock().unwrap().clone();
        let full_path = cwd.join(path);

        // Resolve and validate within workspace
        let canonical = match full_path.canonicalize() {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::error(format!("Cannot resolve path: {e}"))),
        };

        let workspace_canonical = match ctx.workspace.canonicalize() {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::error(format!("Cannot resolve workspace: {e}"))),
        };

        if !canonical.starts_with(&workspace_canonical) {
            return Ok(ToolResult::error("Path is outside workspace boundary"));
        }

        // Must be a regular file
        let metadata = match std::fs::metadata(&canonical) {
            Ok(m) => m,
            Err(e) => return Ok(ToolResult::error(format!("Cannot stat file: {e}"))),
        };

        if !metadata.is_file() {
            return Ok(ToolResult::error("Path is not a regular file"));
        }

        // Determine MIME type
        let mime_type = if let Some(explicit) = params["mime_type"].as_str() {
            explicit.to_string()
        } else {
            let ext = canonical
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            guess_mime(&ext)
                .unwrap_or("application/octet-stream")
                .to_string()
        };

        let attachment = Attachment {
            path: canonical.clone(),
            mime_type: mime_type.clone(),
        };

        ctx.pending_attachments.lock().unwrap().push(attachment);

        let display_path = canonical
            .strip_prefix(&workspace_canonical)
            .unwrap_or(&canonical)
            .display();

        Ok(ToolResult::success(format!(
            "Queued {display_path} ({mime_type}) for sending"
        )))
    }
}
