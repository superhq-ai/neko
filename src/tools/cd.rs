use async_trait::async_trait;
use serde_json::json;

use super::{schema_object, Tool, ToolContext, ToolResult};
use crate::error::Result;

pub struct CdTool;

#[async_trait]
impl Tool for CdTool {
    fn name(&self) -> &str {
        "cd"
    }

    fn description(&self) -> &str {
        "Change the current working directory. Path can be absolute or relative to the current directory. Must stay within the workspace boundary."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        schema_object(
            json!({
                "path": {
                    "type": "string",
                    "description": "Directory path (absolute or relative to current directory)"
                }
            }),
            &["path"],
        )
    }

    async fn execute(&self, params: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult> {
        let path = params["path"].as_str().unwrap_or_default();

        let current = ctx.cwd.lock().unwrap().clone();
        let target = current.join(path);

        let canonical = match target.canonicalize() {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::error(format!("Cannot resolve path: {e}"))),
        };

        let workspace_canonical = match ctx.workspace.canonicalize() {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolResult::error(format!(
                    "Cannot resolve workspace: {e}"
                )))
            }
        };

        if !canonical.starts_with(&workspace_canonical) {
            return Ok(ToolResult::error("Path is outside workspace boundary"));
        }

        if !canonical.is_dir() {
            return Ok(ToolResult::error(format!(
                "Not a directory: {}",
                canonical.display()
            )));
        }

        // Update the shared cwd
        *ctx.cwd.lock().unwrap() = canonical.clone();

        // Show path relative to workspace for readability
        let display = canonical
            .strip_prefix(&workspace_canonical)
            .map(|p| {
                if p.as_os_str().is_empty() {
                    ".".to_string()
                } else {
                    p.display().to_string()
                }
            })
            .unwrap_or_else(|_| canonical.display().to_string());

        Ok(ToolResult::success(format!("Changed directory to {display}")))
    }
}
