pub mod read_file;
pub mod write_file;
pub mod list_files;
pub mod exec;
pub mod http_request;
pub mod memory_flush;
pub mod memory_search;
pub mod cd;
pub mod memory_replace;
pub mod run_python;
pub mod process_manager;
pub mod process;
pub mod send_file;
pub mod cron_manage;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;

use self::process_manager::ProcessManager;

use crate::channels::Attachment;
use crate::config::ToolsConfig;
use crate::error::Result;
use crate::llm::types::ToolDefinition;

/// The channel + chat ID the current message arrived from.
#[derive(Debug, Clone)]
pub struct ChannelContext {
    pub channel: String,
    pub recipient_id: String,
}

/// Context passed to tool execution.
pub struct ToolContext {
    /// Root workspace directory — security boundary (immutable).
    pub workspace: PathBuf,
    /// Current working directory — mutable, shared across tool calls.
    /// Relative paths in file/exec tools resolve against this.
    pub cwd: Arc<Mutex<PathBuf>>,
    /// Files queued for sending as media attachments.
    pub pending_attachments: Arc<Mutex<Vec<Attachment>>>,
    /// The channel this message arrived from (if any).
    pub channel: Option<ChannelContext>,
}

/// Result of a tool execution
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub output: String,
    pub is_error: bool,
}

impl ToolResult {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
        }
    }
    pub fn error(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: true,
        }
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;
    async fn execute(&self, params: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult>;
}

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|t| ToolDefinition {
                tool_type: "function".to_string(),
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters_schema(),
            })
            .collect()
    }

    pub fn names(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }
}

/// Register core tools, respecting the config's enabled tools list.
pub fn register_core_tools(
    registry: &mut ToolRegistry,
    config: &ToolsConfig,
) {
    let pm = Arc::new(ProcessManager::new(config.exec_yield_ms));

    registry.register(Box::new(read_file::ReadFileTool));
    registry.register(Box::new(write_file::WriteFileTool));
    registry.register(Box::new(list_files::ListFilesTool));
    registry.register(Box::new(exec::ExecTool::new(
        config.exec_allowlist.clone(),
        config.exec_timeout_secs,
        Arc::clone(&pm),
    )));
    registry.register(Box::new(process::ProcessTool::new(Arc::clone(&pm))));
    registry.register(Box::new(http_request::HttpRequestTool::new(
        config.http_allowed_domains.clone(),
    )));
    registry.register(Box::new(cd::CdTool));
    registry.register(Box::new(memory_flush::MemoryFlushTool));
    registry.register(Box::new(memory_search::MemorySearchTool));
    registry.register(Box::new(memory_replace::MemoryReplaceTool));

    registry.register(Box::new(send_file::SendFileTool));
    registry.register(Box::new(cron_manage::CronManageTool));

    if config.python.enabled {
        registry.register(Box::new(run_python::RunPythonTool::new(
            config.python.clone(),
            config.http_allowed_domains.clone(),
        )));
    }
}

/// Helper to build a JSON Schema object with given properties.
pub fn schema_object(properties: serde_json::Value, required: &[&str]) -> serde_json::Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}
