use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::json;

use super::{schema_object, Tool, ToolContext, ToolResult};
use crate::error::Result;

pub struct WriteFileTool {
    workspace: PathBuf,
}

impl WriteFileTool {
    pub fn new(workspace: PathBuf) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates parent directories if needed. Path is relative to workspace."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        schema_object(
            json!({
                "path": {
                    "type": "string",
                    "description": "File path relative to workspace"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write"
                }
            }),
            &["path", "content"],
        )
    }

    async fn execute(&self, params: serde_json::Value, _ctx: &ToolContext) -> Result<ToolResult> {
        let path = params["path"].as_str().unwrap_or_default();
        let content = params["content"].as_str().unwrap_or_default();

        let full_path = self.workspace.join(path);

        // Security: use parent check since file may not exist yet
        if let Some(parent) = full_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return Ok(ToolResult::error(format!("Failed to create directories: {e}")));
            }

            // Verify parent is within workspace
            if let (Ok(parent_canonical), Ok(workspace_canonical)) =
                (parent.canonicalize(), self.workspace.canonicalize())
            {
                if !parent_canonical.starts_with(&workspace_canonical) {
                    return Ok(ToolResult::error("Path is outside workspace boundary"));
                }
            }
        }

        match std::fs::write(&full_path, content) {
            Ok(()) => Ok(ToolResult::success(format!("Written {} bytes to {path}", content.len()))),
            Err(e) => Ok(ToolResult::error(format!("Failed to write file: {e}"))),
        }
    }
}
