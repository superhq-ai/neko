use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::json;

use super::{schema_object, Tool, ToolContext, ToolResult};
use crate::error::Result;

pub struct ReadFileTool {
    workspace: PathBuf,
}

impl ReadFileTool {
    pub fn new(workspace: PathBuf) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Path is relative to the workspace."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        schema_object(
            json!({
                "path": {
                    "type": "string",
                    "description": "File path relative to workspace"
                }
            }),
            &["path"],
        )
    }

    async fn execute(&self, params: serde_json::Value, _ctx: &ToolContext) -> Result<ToolResult> {
        let path = params["path"]
            .as_str()
            .unwrap_or_default();

        let full_path = self.workspace.join(path);

        // Security: ensure path stays within workspace
        let canonical = match full_path.canonicalize() {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::error(format!("Cannot resolve path: {e}"))),
        };

        let workspace_canonical = match self.workspace.canonicalize() {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::error(format!("Cannot resolve workspace: {e}"))),
        };

        if !canonical.starts_with(&workspace_canonical) {
            return Ok(ToolResult::error("Path is outside workspace boundary"));
        }

        match std::fs::read_to_string(&canonical) {
            Ok(content) => Ok(ToolResult::success(content)),
            Err(e) => Ok(ToolResult::error(format!("Failed to read file: {e}"))),
        }
    }
}
