use async_trait::async_trait;
use serde_json::json;

use super::{schema_object, Tool, ToolContext, ToolResult};
use crate::error::Result;

pub struct MemoryReplaceTool;

#[async_trait]
impl Tool for MemoryReplaceTool {
    fn name(&self) -> &str {
        "memory_replace"
    }

    fn description(&self) -> &str {
        "Find and replace text in a memory file. Use empty new_text to delete text."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        schema_object(
            json!({
                "file": {
                    "type": "string",
                    "description": "Filename within the memory directory (e.g. 'MEMORY.md')"
                },
                "old_text": {
                    "type": "string",
                    "description": "Text to find (exact match)"
                },
                "new_text": {
                    "type": "string",
                    "description": "Replacement text (empty string to delete)"
                }
            }),
            &["file", "old_text", "new_text"],
        )
    }

    async fn execute(&self, params: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult> {
        let file = params["file"].as_str().unwrap_or_default();
        let old_text = params["old_text"].as_str().unwrap_or_default();
        let new_text = params["new_text"].as_str().unwrap_or_default();

        if file.is_empty() {
            return Ok(ToolResult::error("file is required"));
        }
        if old_text.is_empty() {
            return Ok(ToolResult::error("old_text is required"));
        }

        // Validate filename — no path traversal
        if file.contains("..") || file.contains('/') || file.contains('\\') {
            return Ok(ToolResult::error(
                "Invalid filename: must not contain path separators or '..'",
            ));
        }

        let file_path = ctx.workspace.join("memory").join(file);

        if !file_path.exists() {
            return Ok(ToolResult::error(format!(
                "File not found: memory/{file}"
            )));
        }

        let content = match std::fs::read_to_string(&file_path) {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::error(format!("Failed to read file: {e}"))),
        };

        if !content.contains(old_text) {
            return Ok(ToolResult::error(format!(
                "old_text not found in memory/{file}"
            )));
        }

        // Replace first occurrence only
        let new_content = content.replacen(old_text, new_text, 1);

        if let Err(e) = std::fs::write(&file_path, &new_content) {
            return Ok(ToolResult::error(format!("Failed to write file: {e}")));
        }

        if new_text.is_empty() {
            Ok(ToolResult::success(format!(
                "Deleted text from memory/{file} ({} chars removed)",
                old_text.len()
            )))
        } else {
            Ok(ToolResult::success(format!(
                "Replaced text in memory/{file}"
            )))
        }
    }
}
