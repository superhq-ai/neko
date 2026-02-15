use async_trait::async_trait;
use serde_json::json;

use super::{schema_object, Tool, ToolContext, ToolResult};
use crate::error::Result;

pub struct ListFilesTool;

#[async_trait]
impl Tool for ListFilesTool {
    fn name(&self) -> &str {
        "list_files"
    }

    fn description(&self) -> &str {
        "List files and directories at the given path. Path is relative to current directory."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        schema_object(
            json!({
                "path": {
                    "type": "string",
                    "description": "Directory path relative to current directory (default: current directory)"
                }
            }),
            &[],
        )
    }

    async fn execute(&self, params: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult> {
        let path = params["path"].as_str().unwrap_or(".");
        let cwd = ctx.cwd.lock().unwrap().clone();
        let full_path = cwd.join(path);

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

        let mut entries = Vec::new();
        match std::fs::read_dir(&canonical) {
            Ok(dir) => {
                for entry in dir {
                    if let Ok(entry) = entry {
                        let name = entry.file_name().to_string_lossy().to_string();
                        let is_dir = entry.file_type().map_or(false, |t| t.is_dir());
                        if is_dir {
                            entries.push(format!("{name}/"));
                        } else {
                            entries.push(name);
                        }
                    }
                }
            }
            Err(e) => return Ok(ToolResult::error(format!("Failed to list directory: {e}"))),
        }

        entries.sort();
        Ok(ToolResult::success(entries.join("\n")))
    }
}
