use tracing::{debug, warn};

use crate::error::{NekoError, Result};
use crate::tools::{ToolContext, ToolRegistry, ToolResult};

/// Execute a single tool call.
pub async fn execute_tool(
    registry: &ToolRegistry,
    tool_name: &str,
    arguments_json: &str,
    ctx: &ToolContext,
) -> Result<ToolResult> {
    let tool = registry
        .get(tool_name)
        .ok_or_else(|| NekoError::Tool(format!("Unknown tool: {tool_name}")))?;

    let params: serde_json::Value = serde_json::from_str(arguments_json).map_err(|e| {
        NekoError::Tool(format!(
            "Invalid arguments for tool {tool_name}: {e}"
        ))
    })?;

    debug!("Executing tool: {tool_name}");
    let result = tool.execute(params, ctx).await?;

    if result.is_error {
        warn!("Tool {tool_name} returned error: {}", &result.output[..result.output.len().min(200)]);
    }

    Ok(result)
}
