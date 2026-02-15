use std::collections::HashMap;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::{NekoError, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub gateway: GatewayConfig,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub channels: ChannelsConfig,
    #[serde(default)]
    pub session: SessionConfig,
    #[serde(default)]
    pub tools: ToolsConfig,
    #[serde(default)]
    pub heartbeat: HeartbeatConfig,
    #[serde(default)]
    pub mcp: HashMap<String, McpServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default)]
    pub api_token: Option<String>,
    #[serde(default = "default_workspace")]
    pub workspace: String,
}

fn default_bind() -> String {
    "127.0.0.1:3000".to_string()
}

fn default_workspace() -> String {
    "~/.neko/workspace".to_string()
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            api_token: None,
            workspace: default_workspace(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default = "default_compaction_threshold")]
    pub compaction_threshold: u32,
    #[serde(default = "default_max_history")]
    pub max_history: u32,
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    #[serde(default)]
    pub instructions: Option<String>,
}

fn default_model() -> String {
    "gpt-5-mini".to_string()
}
fn default_provider() -> String {
    "openai".to_string()
}
fn default_max_tokens() -> u32 {
    4096
}
fn default_compaction_threshold() -> u32 {
    50
}
fn default_max_history() -> u32 {
    100
}
fn default_max_iterations() -> u32 {
    10
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: default_model(),
            provider: default_provider(),
            max_tokens: default_max_tokens(),
            tools: vec![
                "read_file".into(),
                "write_file".into(),
                "list_files".into(),
                "exec".into(),
                "http_request".into(),
                "memory_write".into(),
            ],
            compaction_threshold: default_compaction_threshold(),
            max_history: default_max_history(),
            max_iterations: default_max_iterations(),
            instructions: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub api_key: Option<String>,
    pub base_url: String,
    #[serde(default)]
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChannelsConfig {
    #[serde(default)]
    pub telegram: Option<TelegramConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    #[serde(default)]
    pub enabled: bool,
    pub bot_token: Option<String>,
    #[serde(default)]
    pub allowed_users: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsConfig {
    #[serde(default)]
    pub sandbox: bool,
    #[serde(default)]
    pub exec_allowlist: Vec<String>,
    #[serde(default)]
    pub http_allowed_domains: Vec<String>,
    #[serde(default = "default_exec_timeout")]
    pub exec_timeout_secs: u64,
    #[serde(default = "default_exec_yield_ms")]
    pub exec_yield_ms: u64,
    #[serde(default)]
    pub python: PythonConfig,
}

fn default_exec_timeout() -> u64 {
    1800
}

fn default_exec_yield_ms() -> u64 {
    10_000
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            sandbox: false,
            exec_allowlist: vec![],
            http_allowed_domains: vec![],
            exec_timeout_secs: default_exec_timeout(),
            exec_yield_ms: default_exec_yield_ms(),
            python: PythonConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PythonConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_python_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_python_max_memory")]
    pub max_memory: usize,
    #[serde(default = "default_python_max_allocations")]
    pub max_allocations: usize,
    #[serde(default = "default_python_max_recursion")]
    pub max_recursion: usize,
    #[serde(default)]
    pub external_functions: Vec<String>,
}

fn default_python_timeout() -> u64 {
    10
}
fn default_python_max_memory() -> usize {
    16 * 1024 * 1024 // 16 MB
}
fn default_python_max_allocations() -> usize {
    100_000
}
fn default_python_max_recursion() -> usize {
    100
}

impl Default for PythonConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            timeout_secs: default_python_timeout(),
            max_memory: default_python_max_memory(),
            max_allocations: default_python_max_allocations(),
            max_recursion: default_python_max_recursion(),
            external_functions: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
    #[serde(default)]
    pub checklist_file: Option<String>,
    #[serde(default)]
    pub notify_channels: Vec<String>,
}

fn default_interval() -> u64 {
    3600
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: default_interval(),
            checklist_file: None,
            notify_channels: vec![],
        }
    }
}

// ---------------------------------------------------------------------------
// Session config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    #[serde(default)]
    pub dm_scope: DmScope,
    #[serde(default)]
    pub reset_mode: ResetMode,
    #[serde(default = "default_reset_at_hour")]
    pub reset_at_hour: u32,
    #[serde(default)]
    pub idle_minutes: Option<u32>,
    #[serde(default = "default_max_history")]
    pub max_history: u32,
    #[serde(default = "default_max_cached")]
    pub max_cached: usize,
}

fn default_reset_at_hour() -> u32 {
    4
}

fn default_max_cached() -> usize {
    8
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            dm_scope: DmScope::default(),
            reset_mode: ResetMode::default(),
            reset_at_hour: default_reset_at_hour(),
            idle_minutes: None,
            max_history: default_max_history(),
            max_cached: default_max_cached(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DmScope {
    #[default]
    Main,
    PerChannelPeer,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ResetMode {
    #[default]
    Daily,
    Idle,
    Both,
}

/// MCP server configuration (stdio transport).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| NekoError::Config(format!("Failed to read config: {e}")))?;
        let content = substitute_env_vars(&content);
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }

    pub fn default_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".neko")
            .join("config.toml")
    }

    pub fn workspace_path(&self) -> PathBuf {
        let path = self.gateway.workspace.replace('~', &dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .to_string_lossy());
        PathBuf::from(path)
    }

    pub fn default_toml() -> &'static str {
        r#"[gateway]
bind = "127.0.0.1:3000"
workspace = "~/.neko/workspace"

[agent]
model = "gpt-5-mini"
provider = "openai"
max_tokens = 4096
tools = ["read_file", "write_file", "list_files", "exec", "http_request", "memory_write"]

[providers.openai]
api_key = "${OPENAI_API_KEY}"
base_url = "https://api.openai.com"
models = ["gpt-5-mini", "gpt-5"]

[tools]
sandbox = false
exec_timeout_secs = 1800
exec_yield_ms = 10000

[heartbeat]
enabled = false
interval_secs = 3600

# MCP servers — uncomment to enable
# [mcp.filesystem]
# command = "npx"
# args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
#
# [mcp.brave-search]
# command = "npx"
# args = ["-y", "@anthropic/mcp-server-brave-search"]
# env = { BRAVE_API_KEY = "${BRAVE_API_KEY}" }
"#
    }
}

/// Substitute `${VAR_NAME}` patterns with environment variable values.
pub fn substitute_env_vars(input: &str) -> String {
    let re = Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}").unwrap();
    re.replace_all(input, |caps: &regex::Captures| {
        let var_name = &caps[1];
        std::env::var(var_name).unwrap_or_default()
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_parses() {
        let config: Config = toml::from_str(Config::default_toml()).unwrap();
        assert_eq!(config.gateway.bind, "127.0.0.1:3000");
        assert_eq!(config.agent.model, "gpt-5-mini");
    }

    #[test]
    fn test_env_var_substitution() {
        std::env::set_var("NEKO_TEST_VAR", "hello123");
        let result = substitute_env_vars("key = \"${NEKO_TEST_VAR}\"");
        assert_eq!(result, "key = \"hello123\"");
        std::env::remove_var("NEKO_TEST_VAR");
    }

    #[test]
    fn test_missing_env_var_becomes_empty() {
        let result = substitute_env_vars("key = \"${NONEXISTENT_VAR_XYZ}\"");
        assert_eq!(result, "key = \"\"");
    }

    #[test]
    fn test_empty_config() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.gateway.bind, "127.0.0.1:3000");
        assert_eq!(config.agent.max_tokens, 4096);
    }

    #[test]
    fn test_mcp_config_parses() {
        let toml_str = r#"
[mcp.test-server]
command = "node"
args = ["server.js"]

[mcp.test-server.env]
API_KEY = "test123"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let mcp = config.mcp.get("test-server").unwrap();
        assert_eq!(mcp.command, "node");
        assert_eq!(mcp.args, vec!["server.js"]);
        assert_eq!(mcp.env.get("API_KEY").unwrap(), "test123");
    }
}
