use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use tokio::process::Command;

use super::{schema_object, Tool, ToolContext, ToolResult};
use crate::error::Result;

pub struct ExecTool {
    allowlist: Vec<String>,
    timeout_secs: u64,
}

impl ExecTool {
    pub fn new(allowlist: Vec<String>, timeout_secs: u64) -> Self {
        Self {
            allowlist,
            timeout_secs,
        }
    }
}

#[async_trait]
impl Tool for ExecTool {
    fn name(&self) -> &str {
        "exec"
    }

    fn description(&self) -> &str {
        "Execute a shell command in the current directory. Returns stdout and stderr."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        schema_object(
            json!({
                "command": {
                    "type": "string",
                    "description": "Shell command to execute"
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

        let cwd = ctx.cwd.lock().unwrap().clone();

        let result = tokio::time::timeout(
            Duration::from_secs(self.timeout_secs),
            Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(&cwd)
                .output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let mut result = String::new();
                if !stdout.is_empty() {
                    result.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    result.push_str("[stderr] ");
                    result.push_str(&stderr);
                }
                if result.is_empty() {
                    result = format!("Command exited with code {}", output.status.code().unwrap_or(-1));
                }
                if output.status.success() {
                    Ok(ToolResult::success(result))
                } else {
                    Ok(ToolResult::error(result))
                }
            }
            Ok(Err(e)) => Ok(ToolResult::error(format!("Failed to execute: {e}"))),
            Err(_) => Ok(ToolResult::error(format!(
                "Command timed out after {}s",
                self.timeout_secs
            ))),
        }
    }
}
