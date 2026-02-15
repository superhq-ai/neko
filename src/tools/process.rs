use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use super::process_manager::ProcessManager;
use super::{schema_object, Tool, ToolContext, ToolResult};
use crate::error::Result;

pub struct ProcessTool {
    process_manager: Arc<ProcessManager>,
}

impl ProcessTool {
    pub fn new(process_manager: Arc<ProcessManager>) -> Self {
        Self { process_manager }
    }
}

#[async_trait]
impl Tool for ProcessTool {
    fn name(&self) -> &str {
        "process"
    }

    fn description(&self) -> &str {
        "Manage background processes spawned by exec. \
         Actions: \"list\" (show all sessions), \
         \"poll\" (get new output from a session), \
         \"input\" (write to stdin, optional eof to close stdin), \
         \"kill\" (terminate a session)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        schema_object(
            json!({
                "action": {
                    "type": "string",
                    "enum": ["list", "poll", "input", "kill"],
                    "description": "Action to perform"
                },
                "session_id": {
                    "type": "string",
                    "description": "Session ID (e.g. bg_1). Required for poll, input, kill."
                },
                "data": {
                    "type": "string",
                    "description": "Data to write to stdin (for input action)"
                },
                "eof": {
                    "type": "boolean",
                    "description": "Close stdin after writing (for input action). Signals end-of-input."
                }
            }),
            &["action"],
        )
    }

    async fn execute(&self, params: serde_json::Value, _ctx: &ToolContext) -> Result<ToolResult> {
        let action = params["action"].as_str().unwrap_or_default();

        match action {
            "list" => self.action_list().await,
            "poll" => {
                let session_id = params["session_id"]
                    .as_str()
                    .ok_or_else(|| crate::error::NekoError::Tool("session_id is required for poll".into()))?;
                self.action_poll(session_id).await
            }
            "input" => {
                let session_id = params["session_id"]
                    .as_str()
                    .ok_or_else(|| crate::error::NekoError::Tool("session_id is required for input".into()))?;
                let data = params["data"].as_str().unwrap_or("");
                let eof = params["eof"].as_bool().unwrap_or(false);
                self.action_input(session_id, data, eof).await
            }
            "kill" => {
                let session_id = params["session_id"]
                    .as_str()
                    .ok_or_else(|| crate::error::NekoError::Tool("session_id is required for kill".into()))?;
                self.action_kill(session_id).await
            }
            _ => Ok(ToolResult::error(format!("Unknown action: {action}"))),
        }
    }
}

impl ProcessTool {
    async fn action_list(&self) -> Result<ToolResult> {
        let infos = self.process_manager.list_sessions().await;
        if infos.is_empty() {
            return Ok(ToolResult::success("No background sessions."));
        }

        let mut out = String::new();
        for info in &infos {
            let status = match info.exit_status {
                Some(code) => format!("exited (code {code})"),
                None => "running".to_string(),
            };
            out.push_str(&format!(
                "{}: `{}` — {} ({}s)\n",
                info.id, info.command, status, info.elapsed_secs,
            ));
        }
        Ok(ToolResult::success(out))
    }

    async fn action_poll(&self, session_id: &str) -> Result<ToolResult> {
        let session = self
            .process_manager
            .get_session(session_id)
            .await
            .ok_or_else(|| crate::error::NekoError::Tool(format!("Session '{session_id}' not found")))?;

        let (new_output, exit_status) = session.poll_output().await;

        let mut msg = String::new();

        if let Some(code) = exit_status {
            msg.push_str(&format!("[exited with code {code}]\n"));
            // Auto-remove completed session after poll
            self.process_manager.remove_session(session_id).await;
        } else {
            msg.push_str("[still running]\n");
        }

        if new_output.is_empty() {
            msg.push_str("(no new output)");
        } else {
            msg.push_str(&new_output);
        }

        Ok(ToolResult::success(msg))
    }

    async fn action_input(&self, session_id: &str, data: &str, eof: bool) -> Result<ToolResult> {
        let session = self
            .process_manager
            .get_session(session_id)
            .await
            .ok_or_else(|| crate::error::NekoError::Tool(format!("Session '{session_id}' not found")))?;

        match session.write_stdin(data, eof).await {
            Ok(()) => {
                let mut msg = "Input sent.".to_string();
                if eof {
                    msg.push_str(" stdin closed (EOF).");
                }
                Ok(ToolResult::success(msg))
            }
            Err(e) => Ok(ToolResult::error(e)),
        }
    }

    async fn action_kill(&self, session_id: &str) -> Result<ToolResult> {
        let session = self
            .process_manager
            .get_session(session_id)
            .await
            .ok_or_else(|| crate::error::NekoError::Tool(format!("Session '{session_id}' not found")))?;

        if let Err(e) = session.kill().await {
            return Ok(ToolResult::error(e));
        }

        let output = session.drain_output().await;
        self.process_manager.remove_session(session_id).await;

        let mut msg = format!("Session {session_id} killed.");
        if !output.is_empty() {
            msg.push_str("\n\nFinal output:\n");
            msg.push_str(&output);
        }
        Ok(ToolResult::success(msg))
    }
}
