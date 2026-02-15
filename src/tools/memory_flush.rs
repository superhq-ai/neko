use async_trait::async_trait;
use serde_json::json;

use super::{schema_object, Tool, ToolContext, ToolResult};
use crate::error::Result;

pub struct MemoryFlushTool;

#[async_trait]
impl Tool for MemoryFlushTool {
    fn name(&self) -> &str {
        "memory_write"
    }

    fn description(&self) -> &str {
        "Write or append content to a memory file. Creates the file if it doesn't exist."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        schema_object(
            json!({
                "file": {
                    "type": "string",
                    "description": "Filename within the memory directory (e.g. 'notes.md')"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write or append"
                },
                "append": {
                    "type": "boolean",
                    "description": "If true, append to existing file. If false, overwrite. Default: true"
                }
            }),
            &["file", "content"],
        )
    }

    async fn execute(&self, params: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult> {
        let file = params["file"].as_str().unwrap_or_default();
        let content = params["content"].as_str().unwrap_or_default();
        let append = params["append"].as_bool().unwrap_or(true);

        // Validate filename - no path traversal
        if file.contains("..") || file.contains('/') || file.contains('\\') {
            return Ok(ToolResult::error("Invalid filename: must not contain path separators or '..'"));
        }

        let memory_dir = ctx.workspace.join("memory");
        if let Err(e) = std::fs::create_dir_all(&memory_dir) {
            return Ok(ToolResult::error(format!("Failed to create memory dir: {e}")));
        }

        let file_path = memory_dir.join(file);

        if append {
            use std::io::Write;
            let mut f = match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&file_path)
            {
                Ok(f) => f,
                Err(e) => return Ok(ToolResult::error(format!("Failed to open file: {e}"))),
            };
            if let Err(e) = writeln!(f, "{content}") {
                return Ok(ToolResult::error(format!("Failed to write: {e}")));
            }
        } else {
            if let Err(e) = std::fs::write(&file_path, content) {
                return Ok(ToolResult::error(format!("Failed to write file: {e}")));
            }
        }


        Ok(ToolResult::success(format!(
            "{} {} to memory/{}",
            if append { "Appended" } else { "Written" },
            content.len(),
            file
        )))
    }
}
