use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use super::process_manager::{ProcessManager, SpawnResult};
use super::{schema_object, Tool, ToolContext, ToolResult};
use crate::error::Result;

pub struct ExecTool {
    allowlist: Vec<String>,
    timeout_secs: u64,
    process_manager: Arc<ProcessManager>,
}

impl ExecTool {
    pub fn new(
        allowlist: Vec<String>,
        timeout_secs: u64,
        process_manager: Arc<ProcessManager>,
    ) -> Self {
        Self {
            allowlist,
            timeout_secs,
            process_manager,
        }
    }
}

#[async_trait]
impl Tool for ExecTool {
    fn name(&self) -> &str {
        "exec"
    }

    fn description(&self) -> &str {
        "Execute a shell command. Short commands return immediately. \
         Long-running commands are automatically backgrounded and return a \
         session_id — use the `process` tool to poll output, send input, or kill them."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        schema_object(
            json!({
                "command": {
                    "type": "string",
                    "description": "Shell command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Optional per-command timeout in seconds (overrides default)"
                }
            }),
            &["command"],
        )
    }

    async fn execute(&self, params: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult> {
        let command = params["command"].as_str().unwrap_or_default();

        // Check allowlist if configured
        if !self.allowlist.is_empty() {
            let cmd_name = command.split_whitespace().next().unwrap_or("");
            if !self.allowlist.iter().any(|a| a == cmd_name) {
                return Ok(ToolResult::error(format!(
                    "Command '{cmd_name}' is not in the exec allowlist"
                )));
            }
        }

        let timeout = params["timeout"]
            .as_u64()
            .unwrap_or(self.timeout_secs);

        let cwd = ctx.cwd.lock().unwrap().clone();

        match self.process_manager.spawn_or_yield(command, &cwd, timeout).await {
            Ok(SpawnResult::Completed { output, success }) => {
                if success {
                    Ok(ToolResult::success(output))
                } else {
                    Ok(ToolResult::error(output))
                }
            }
            Ok(SpawnResult::Backgrounded { session_id, output_so_far }) => {
                let mut msg = format!(
                    "Command backgrounded as {session_id} (still running).\n\
                     Use `process` tool with action \"poll\" to check output."
                );
                if !output_so_far.is_empty() {
                    msg.push_str("\n\nOutput so far:\n");
                    msg.push_str(&output_so_far);
                }
                Ok(ToolResult::success(msg))
            }
            Err(e) => Ok(ToolResult::error(e)),
        }
    }
}
