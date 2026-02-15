use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use rmcp::model::{CallToolRequestParams, Tool as McpToolDef};
use rmcp::service::{RunningService, ServiceExt};
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use tokio::process::Command;
use tracing::{debug, error};

use crate::config::McpServerConfig;
use crate::error::{NekoError, Result};
use crate::tools::{Tool, ToolContext, ToolResult};

type ClientService = RunningService<rmcp::RoleClient, ()>;

/// An MCP client connected to a server via the official rmcp SDK.
pub struct McpClient {
    name: String,
    service: Arc<ClientService>,
}

impl McpClient {
    /// Spawn an MCP server subprocess and perform the initialize handshake.
    pub async fn connect(name: &str, config: &McpServerConfig) -> Result<Self> {
        debug!(
            "Spawning MCP server '{}': {} {:?}",
            name, config.command, config.args
        );

        let args = config.args.clone();
        let envs = config.env.clone();
        let command_str = config.command.clone();

        let transport = TokioChildProcess::new(
            Command::new(&config.command).configure(move |cmd| {
                cmd.args(&args);
                for (k, v) in &envs {
                    cmd.env(k, v);
                }
            }),
        )
        .map_err(|e| {
            NekoError::Tool(format!("Failed to spawn MCP server '{name}': {e}"))
        })?;

        let service = ().serve(transport).await.map_err(|e| {
            NekoError::Tool(format!(
                "Failed to initialize MCP server '{name}' ({command_str}): {e}"
            ))
        })?;

        if let Some(info) = service.peer_info() {
            debug!("MCP server '{name}' initialized: {info:?}");
        }

        Ok(McpClient {
            name: name.to_string(),
            service: Arc::new(service),
        })
    }

    /// List available tools from the MCP server.
    pub async fn list_tools(&self) -> Result<Vec<McpToolDef>> {
        let tools = self.service.list_all_tools().await.map_err(|e| {
            NekoError::Tool(format!(
                "Failed to list tools from MCP server '{}': {e}",
                self.name
            ))
        })?;

        debug!("MCP server '{}' has {} tools", self.name, tools.len());
        Ok(tools)
    }

    /// Call a tool on the MCP server.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<ToolResult> {
        let result = self
            .service
            .call_tool(CallToolRequestParams {
                name: name.to_string().into(),
                arguments: arguments.as_object().cloned(),
                meta: None,
                task: None,
            })
            .await
            .map_err(|e| {
                NekoError::Tool(format!(
                    "MCP server '{}' tool call '{}' failed: {e}",
                    self.name, name
                ))
            })?;

        // Extract text content from the response
        let text = result
            .content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
            .collect::<Vec<_>>()
            .join("\n");

        let is_error = result.is_error.unwrap_or(false);

        if is_error {
            Ok(ToolResult::error(text))
        } else {
            Ok(ToolResult::success(text))
        }
    }
}

/// An MCP tool exposed as a native Tool for the registry.
pub struct McpTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
    client: Arc<McpClient>,
}

impl McpTool {
    pub fn new(def: &McpToolDef, client: Arc<McpClient>) -> Self {
        let input_schema = serde_json::to_value(&*def.input_schema)
            .unwrap_or_else(|_| serde_json::json!({"type": "object", "properties": {}}));

        Self {
            name: def.name.to_string(),
            description: def
                .description
                .as_deref()
                .unwrap_or("MCP tool")
                .to_string(),
            input_schema,
            client,
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.input_schema.clone()
    }

    async fn execute(&self, params: serde_json::Value, _ctx: &ToolContext) -> Result<ToolResult> {
        self.client.call_tool(&self.name, params).await
    }
}

/// Connect to all configured MCP servers and register their tools.
pub async fn connect_all(
    configs: &HashMap<String, McpServerConfig>,
) -> Result<Vec<Arc<McpClient>>> {
    let mut clients = Vec::new();

    for (name, config) in configs {
        match McpClient::connect(name, config).await {
            Ok(client) => {
                debug!("Connected to MCP server '{name}'");
                clients.push(Arc::new(client));
            }
            Err(e) => {
                error!("Failed to connect to MCP server '{name}': {e}");
            }
        }
    }

    Ok(clients)
}
