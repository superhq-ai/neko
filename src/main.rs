use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Parser, Subcommand};
use tokio::sync::Mutex;
use tracing::info;
use tracing_subscriber::EnvFilter;

use neko::config::Config;
use neko::error::{NekoError, Result};

#[derive(Parser)]
#[command(name = "neko", version, about = "Lightweight AI agent runtime")]
struct Cli {
    /// Path to config file
    #[arg(short, long)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize default config and workspace directories
    Init {
        /// Interactive setup with prompts
        #[arg(short, long)]
        interactive: bool,
    },
    /// Start the gateway server
    Start,
    /// Stop the running gateway
    Stop,
    /// Show gateway status
    Status,
    /// Show recent logs
    Logs {
        /// Number of lines to show
        #[arg(short, long, default_value = "50")]
        lines: usize,
    },
    /// Send a test message
    Message {
        /// The message text to send
        text: String,
    },
    /// Config management
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Session management
    Sessions {
        #[command(subcommand)]
        action: SessionAction,
    },
    /// Memory management
    Memory {
        #[command(subcommand)]
        action: MemoryAction,
    },
    /// Skills management
    Skills {
        #[command(subcommand)]
        action: SkillAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Show current configuration
    Show,
    /// Open config in editor
    Edit,
}

#[derive(Subcommand)]
enum SessionAction {
    /// List active sessions
    List,
    /// Clear all sessions
    Clear,
}

#[derive(Subcommand)]
enum MemoryAction {
    /// List memory files
    List,
    /// Search memory files for a query
    Search {
        /// Text to search for (case-insensitive)
        query: String,
    },
}

#[derive(Subcommand)]
enum SkillAction {
    /// List installed skills
    List,
    /// Install a skill from a path
    Install {
        /// Path to a SKILL.md file or a directory containing one
        path: String,
    },
    /// Remove an installed skill
    Remove {
        /// Skill name to remove
        name: String,
    },
    /// Reload and list all skills
    Reload,
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Initialize tracing — file + stderr for `start`, stderr only otherwise
    init_tracing(matches!(&cli.command, Commands::Start))?;

    match cli.command {
        Commands::Init { interactive } => {
            if interactive {
                cmd_init_interactive()?;
            } else {
                cmd_init()?;
            }
        }
        Commands::Start => cmd_start(&cli.config).await?,
        Commands::Stop => cmd_stop()?,
        Commands::Status => cmd_status().await?,
        Commands::Logs { lines } => cmd_logs(lines)?,
        Commands::Message { text } => cmd_message(&cli.config, &text).await?,
        Commands::Config { action } => match action {
            ConfigAction::Show => {
                let path = cli.config.unwrap_or_else(Config::default_path);
                let content = std::fs::read_to_string(&path)?;
                println!("{content}");
            }
            ConfigAction::Edit => {
                let path = cli.config.unwrap_or_else(Config::default_path);
                let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
                std::process::Command::new(editor).arg(&path).status()?;
            }
        },
        Commands::Sessions { action } => match action {
            SessionAction::List => cmd_sessions_list(&cli.config)?,
            SessionAction::Clear => cmd_sessions_clear(&cli.config)?,
        },
        Commands::Memory { action } => match action {
            MemoryAction::List => cmd_memory_list(&cli.config)?,
            MemoryAction::Search { query } => cmd_memory_search(&cli.config, &query)?,
        },
        Commands::Skills { action } => match action {
            SkillAction::List => cmd_skills_list(&cli.config)?,
            SkillAction::Install { path } => cmd_skills_install(&cli.config, &path)?,
            SkillAction::Remove { name } => cmd_skills_remove(&cli.config, &name)?,
            SkillAction::Reload => cmd_skills_list(&cli.config)?,
        },
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn neko_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".neko")
}

fn pid_file_path() -> PathBuf {
    neko_dir().join("neko.pid")
}

fn log_file_path() -> PathBuf {
    neko_dir().join("neko.log")
}

fn load_config(path: &Option<PathBuf>) -> Result<Config> {
    let config_path = path.clone().unwrap_or_else(Config::default_path);
    if !config_path.exists() {
        return Err(NekoError::Config(format!(
            "Config not found at {}. Run `neko init` first.",
            config_path.display()
        )));
    }
    Config::load(&config_path)
}

fn is_process_running(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Read PID and bind address from the PID file.
/// Format: line 1 = PID, line 2 = bind address.
fn read_pid_file() -> Option<(u32, String)> {
    let content = std::fs::read_to_string(pid_file_path()).ok()?;
    let mut lines = content.lines();
    let pid: u32 = lines.next()?.trim().parse().ok()?;
    let bind = lines.next().unwrap_or("127.0.0.1:3000").trim().to_string();
    Some((pid, bind))
}

fn init_tracing(with_file: bool) -> std::result::Result<(), Box<dyn std::error::Error>> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        if with_file {
            "info".into()
        } else {
            "warn".into()
        }
    });

    let stderr_layer = tracing_subscriber::fmt::layer();

    if with_file {
        let dir = neko_dir();
        let _ = std::fs::create_dir_all(&dir);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_file_path())?;

        let file_layer = tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file));

        tracing_subscriber::registry()
            .with(env_filter)
            .with(stderr_layer)
            .with(file_layer)
            .init();
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(stderr_layer)
            .init();
    }

    Ok(())
}

async fn build_agent_from_config(config: &Config) -> Result<neko::agent::Agent> {
    let provider = config
        .providers
        .get(&config.agent.provider)
        .ok_or_else(|| {
            NekoError::Config(format!(
                "Provider '{}' not found in config",
                config.agent.provider
            ))
        })?;

    let workspace = config.workspace_path();
    let skills = neko::skills::load_skills(&workspace)?;

    let mut registry = neko::tools::ToolRegistry::new();
    neko::tools::register_core_tools(&mut registry, &config.tools, &workspace);

    let mcp_clients = neko::mcp::connect_all(&config.mcp).await?;
    for client in &mcp_clients {
        let mcp_tools = client.list_tools().await?;
        for tool_def in &mcp_tools {
            registry.register(Box::new(neko::mcp::McpTool::new(tool_def, client.clone())));
        }
    }

    let llm_client = neko::llm::Client::new(&provider.base_url, provider.api_key.as_deref());

    let tool_count = registry.names().len();
    info!(
        "Agent ready: provider={}, model={}, tools={}, skills={}",
        config.agent.provider,
        config.agent.model,
        tool_count,
        skills.len(),
    );

    Ok(
        neko::agent::Agent::new(llm_client, registry, config.agent.clone())
            .with_workspace(workspace)
            .with_skills(skills),
    )
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn cmd_init_interactive() -> Result<()> {
    use inquire::{Text, Select, Confirm};

    println!("Welcome to Neko setup!\n");

    // Provider
    let providers = vec!["openai", "anthropic", "ollama", "custom"];
    let provider = Select::new("Provider:", providers)
        .with_help_message("Which LLM provider to use")
        .prompt()
        .map_err(|e| NekoError::Config(format!("Prompt cancelled: {e}")))?
        .to_string();

    let (default_url, default_model, default_key_env) = match provider.as_str() {
        "openai" => (
            "https://api.openai.com",
            "gpt-5-mini",
            "OPENAI_API_KEY",
        ),
        "anthropic" => (
            "https://api.anthropic.com",
            "claude-sonnet-4-5-20250929",
            "ANTHROPIC_API_KEY",
        ),
        "ollama" => (
            "http://localhost:11434",
            "llama3",
            "",
        ),
        _ => (
            "http://localhost:8080",
            "default",
            "",
        ),
    };

    let base_url = Text::new("Base URL:")
        .with_default(default_url)
        .prompt()
        .map_err(|e| NekoError::Config(format!("Prompt cancelled: {e}")))?;

    let model = Text::new("Model:")
        .with_default(default_model)
        .prompt()
        .map_err(|e| NekoError::Config(format!("Prompt cancelled: {e}")))?;

    // API key — offer env var reference or direct value
    let api_key_str = if default_key_env.is_empty() {
        let key = Text::new("API key (leave empty if none):")
            .with_default("")
            .prompt()
            .map_err(|e| NekoError::Config(format!("Prompt cancelled: {e}")))?;
        if key.is_empty() { None } else { Some(key) }
    } else {
        let key = Text::new("API key:")
            .with_default(&format!("${{{default_key_env}}}"))
            .with_help_message("Use ${VAR_NAME} to reference an env variable")
            .prompt()
            .map_err(|e| NekoError::Config(format!("Prompt cancelled: {e}")))?;
        if key.is_empty() { None } else { Some(key) }
    };

    let workspace = Text::new("Workspace path:")
        .with_default("~/.neko/workspace")
        .prompt()
        .map_err(|e| NekoError::Config(format!("Prompt cancelled: {e}")))?;

    let bind = Text::new("Bind address:")
        .with_default("127.0.0.1:3000")
        .prompt()
        .map_err(|e| NekoError::Config(format!("Prompt cancelled: {e}")))?;

    let enable_heartbeat = Confirm::new("Enable heartbeat?")
        .with_default(false)
        .prompt()
        .map_err(|e| NekoError::Config(format!("Prompt cancelled: {e}")))?;

    // Build config TOML
    let api_key_line = match &api_key_str {
        Some(k) => format!("api_key = \"{k}\""),
        None => "# api_key = \"\"".to_string(),
    };

    let config_content = format!(
        r#"[gateway]
bind = "{bind}"
workspace = "{workspace}"

[agent]
model = "{model}"
provider = "{provider}"
max_tokens = 4096
tools = ["read_file", "write_file", "list_files", "exec", "http_request", "memory_write"]

[providers.{provider}]
{api_key_line}
base_url = "{base_url}"
models = ["{model}"]

[tools]
sandbox = false
exec_timeout_secs = 30

[heartbeat]
enabled = {enable_heartbeat}
interval_secs = 3600

# MCP servers — uncomment to enable
# [mcp.filesystem]
# command = "npx"
# args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
"#
    );

    // Create directories + write config
    let neko = neko_dir();
    let config_path = neko.join("config.toml");
    let ws = PathBuf::from(workspace.replace('~', &dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .to_string_lossy()));
    let memory_dir = ws.join("memory");
    let sessions_dir = ws.join("sessions");
    let skills_dir = ws.join("skills");

    std::fs::create_dir_all(&memory_dir)?;
    std::fs::create_dir_all(&sessions_dir)?;
    std::fs::create_dir_all(&skills_dir)?;

    if config_path.exists() {
        let overwrite = Confirm::new("Config already exists. Overwrite?")
            .with_default(false)
            .prompt()
            .map_err(|e| NekoError::Config(format!("Prompt cancelled: {e}")))?;
        if !overwrite {
            println!("Kept existing config.");
            return Ok(());
        }
    }

    std::fs::write(&config_path, &config_content)?;
    println!("Created config at {}", config_path.display());

    let memory_md = memory_dir.join("MEMORY.md");
    if !memory_md.exists() {
        std::fs::write(
            &memory_md,
            "# Memory\n\nThis file is always loaded into the agent's context.\n",
        )?;
        println!("Created {}", memory_md.display());
    }

    println!("\nWorkspace initialized at {}", ws.display());
    println!("Run `neko start` to launch the agent.");
    Ok(())
}

fn cmd_init() -> Result<()> {
    let neko = neko_dir();
    let config_path = neko.join("config.toml");
    let workspace = neko.join("workspace");
    let memory_dir = workspace.join("memory");
    let sessions_dir = workspace.join("sessions");
    let skills_dir = workspace.join("skills");

    std::fs::create_dir_all(&memory_dir)?;
    std::fs::create_dir_all(&sessions_dir)?;
    std::fs::create_dir_all(&skills_dir)?;

    if !config_path.exists() {
        std::fs::write(&config_path, Config::default_toml())?;
        println!("Created config at {}", config_path.display());
    } else {
        println!("Config already exists at {}", config_path.display());
    }

    let memory_md = memory_dir.join("MEMORY.md");
    if !memory_md.exists() {
        std::fs::write(
            &memory_md,
            "# Memory\n\nThis file is always loaded into the agent's context.\n",
        )?;
        println!("Created {}", memory_md.display());
    }

    println!("Workspace initialized at {}", workspace.display());
    Ok(())
}

async fn cmd_start(config_path: &Option<PathBuf>) -> Result<()> {
    let config = load_config(config_path)?;

    // Check if already running
    if let Some((pid, _)) = read_pid_file() {
        if is_process_running(pid) {
            return Err(NekoError::Config(format!(
                "Neko is already running (PID {pid}). Use `neko stop` first."
            )));
        }
        // Stale PID file
        let _ = std::fs::remove_file(pid_file_path());
    }

    let bind_addr = config.gateway.bind.clone();
    let api_token = config.gateway.api_token.clone();
    let workspace = config.workspace_path();

    let agent = build_agent_from_config(&config).await?;

    let state = Arc::new(neko::api::AppState {
        agent: Mutex::new(agent),
        api_token,
    });

    let app = neko::api::router(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await.map_err(|e| {
        NekoError::Config(format!("Failed to bind to {bind_addr}: {e}"))
    })?;

    let local_addr = listener.local_addr().map_err(|e| {
        NekoError::Config(format!("Failed to get local address: {e}"))
    })?;

    // Write PID file (PID + bind address)
    let pid = std::process::id();
    std::fs::write(
        pid_file_path(),
        format!("{pid}\n{local_addr}\n"),
    )?;

    println!("Neko v{} started", env!("CARGO_PKG_VERSION"));
    println!("  Bind:      {local_addr}");
    println!("  Workspace: {}", workspace.display());
    println!(
        "  Provider:  {} ({})",
        config.agent.provider, config.agent.model
    );
    println!("  PID:       {pid}");
    println!("  Log:       {}", log_file_path().display());
    println!();
    println!("Press Ctrl+C to stop.");

    let shutdown = async {
        tokio::signal::ctrl_c().await.ok();
        println!("\nShutting down...");
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(|e| NekoError::Config(format!("Server error: {e}")))?;

    let _ = std::fs::remove_file(pid_file_path());
    println!("Neko stopped.");
    Ok(())
}

fn cmd_stop() -> Result<()> {
    let Some((pid, _)) = read_pid_file() else {
        println!("Neko is not running (no PID file found).");
        return Ok(());
    };

    if !is_process_running(pid) {
        let _ = std::fs::remove_file(pid_file_path());
        println!("Neko is not running (stale PID file for {pid}, cleaned up).");
        return Ok(());
    }

    // Send SIGTERM
    let status = std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("Sent stop signal to Neko (PID {pid}).");
            // Wait briefly for process to exit, then clean up PID file
            for _ in 0..10 {
                std::thread::sleep(std::time::Duration::from_millis(200));
                if !is_process_running(pid) {
                    let _ = std::fs::remove_file(pid_file_path());
                    println!("Neko stopped.");
                    return Ok(());
                }
            }
            println!("Process {pid} still running. It may take a moment to shut down.");
        }
        _ => {
            println!("Failed to send stop signal to PID {pid}.");
        }
    }

    Ok(())
}

async fn cmd_status() -> Result<()> {
    let Some((pid, bind)) = read_pid_file() else {
        println!("Neko is not running.");
        return Ok(());
    };

    if !is_process_running(pid) {
        let _ = std::fs::remove_file(pid_file_path());
        println!("Neko is not running (stale PID file, cleaned up).");
        return Ok(());
    }

    // Try health endpoint
    let url = format!("http://{bind}/health");
    match reqwest::get(&url).await {
        Ok(resp) if resp.status().is_success() => {
            println!("Neko is running (PID {pid}) on {bind}");
            if let Ok(body) = resp.text().await {
                println!("  Health: {body}");
            }
        }
        _ => {
            println!("Neko process is running (PID {pid}) but health check failed on {bind}");
        }
    }

    Ok(())
}

fn cmd_logs(num_lines: usize) -> Result<()> {
    let path = log_file_path();

    if !path.exists() {
        println!("No log file found at {}", path.display());
        println!("Start the server first: neko start");
        return Ok(());
    }

    let content = std::fs::read_to_string(&path)?;
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(num_lines);

    for line in &lines[start..] {
        println!("{line}");
    }

    if start > 0 {
        println!(
            "\n(Showing last {} of {} lines)",
            lines.len() - start,
            lines.len()
        );
    }

    Ok(())
}

async fn cmd_message(config_path: &Option<PathBuf>, text: &str) -> Result<()> {
    let config = load_config(config_path)?;
    let mut agent = build_agent_from_config(&config).await?;
    let response = agent.run_turn(text).await?;
    println!("{response}");
    Ok(())
}

fn cmd_memory_list(config_path: &Option<PathBuf>) -> Result<()> {
    let config = load_config(config_path)?;
    let mem_dir = config.workspace_path().join("memory");

    if !mem_dir.exists() {
        eprintln!("Memory directory not found. Run `neko init` first.");
        return Ok(());
    }

    let mut entries: Vec<_> = std::fs::read_dir(&mem_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map_or(false, |ext| ext == "md")
        })
        .collect();

    entries.sort_by_key(|e| e.file_name());

    if entries.is_empty() {
        println!("No memory files found.");
        return Ok(());
    }

    for entry in entries {
        let path = entry.path();
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        println!("{name}\t{size} bytes");
    }

    Ok(())
}

fn cmd_memory_search(config_path: &Option<PathBuf>, query: &str) -> Result<()> {
    let config = load_config(config_path)?;
    let mem_dir = config.workspace_path().join("memory");

    if !mem_dir.exists() {
        eprintln!("Memory directory not found. Run `neko init` first.");
        return Ok(());
    }

    let query_lower = query.to_lowercase();
    let mut found = false;

    let mut entries: Vec<_> = std::fs::read_dir(&mem_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map_or(false, |ext| ext == "md")
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let filename = path.file_name().unwrap_or_default().to_string_lossy();
        for (i, line) in content.lines().enumerate() {
            if line.to_lowercase().contains(&query_lower) {
                found = true;
                println!("{}:{}: {}", filename, i + 1, line);
            }
        }
    }

    if !found {
        println!("No matches found for '{query}'.");
    }

    Ok(())
}

fn cmd_sessions_list(config_path: &Option<PathBuf>) -> Result<()> {
    let config = load_config(config_path)?;
    let sessions_dir = config.workspace_path().join("sessions");

    if !sessions_dir.exists() {
        println!("No sessions directory found.");
        return Ok(());
    }

    let mut entries: Vec<_> = std::fs::read_dir(&sessions_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map_or(false, |ext| ext == "jsonl")
        })
        .collect();

    if entries.is_empty() {
        println!("No active sessions.");
        return Ok(());
    }

    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let name = path.file_stem().unwrap_or_default().to_string_lossy();
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        println!("{name}\t{size} bytes");
    }

    Ok(())
}

fn cmd_sessions_clear(config_path: &Option<PathBuf>) -> Result<()> {
    let config = load_config(config_path)?;
    let sessions_dir = config.workspace_path().join("sessions");

    if !sessions_dir.exists() {
        println!("No sessions directory found.");
        return Ok(());
    }

    let mut count = 0;
    for entry in std::fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        if entry.path().extension().map_or(false, |e| e == "jsonl") {
            std::fs::remove_file(entry.path())?;
            count += 1;
        }
    }

    println!("Cleared {count} session(s).");
    Ok(())
}

fn cmd_skills_list(config_path: &Option<PathBuf>) -> Result<()> {
    let config = load_config(config_path)?;
    let skills = neko::skills::load_skills(&config.workspace_path())?;

    if skills.is_empty() {
        println!("No skills installed.");
        return Ok(());
    }

    for skill in &skills {
        let license = skill
            .license
            .as_deref()
            .map(|l| format!(" ({l})"))
            .unwrap_or_default();
        println!("{}{license}", skill.name);
        println!("  {}", skill.description);
        if !skill.allowed_tools.is_empty() {
            println!("  tools: {}", skill.allowed_tools.join(", "));
        }
        println!("  path:  {}", skill.path.display());
    }

    Ok(())
}

fn cmd_skills_install(config_path: &Option<PathBuf>, path: &str) -> Result<()> {
    let config = load_config(config_path)?;
    let skills_dir = config.workspace_path().join("skills");
    let source = PathBuf::from(path);

    // Determine source directory and SKILL.md path
    let (source_dir, skill_md) = if source.is_file()
        && source
            .file_name()
            .map_or(false, |n| n == "SKILL.md")
    {
        (
            source.parent().unwrap_or(&source).to_path_buf(),
            source.clone(),
        )
    } else if source.is_dir() {
        let md = source.join("SKILL.md");
        if !md.exists() {
            return Err(NekoError::Config(format!(
                "No SKILL.md found in {}",
                source.display()
            )));
        }
        (source.clone(), md)
    } else {
        return Err(NekoError::Config(format!(
            "Invalid skill path: {} (expected a directory or SKILL.md file)",
            source.display()
        )));
    };

    // Validate the skill parses correctly
    let skill = neko::skills::Skill::load(&skill_md)?;

    let target_dir = skills_dir.join(&skill.name);
    if target_dir.exists() {
        return Err(NekoError::Config(format!(
            "Skill '{}' is already installed at {}",
            skill.name,
            target_dir.display()
        )));
    }

    copy_dir_recursive(&source_dir, &target_dir)?;

    println!("Installed skill '{}' to {}", skill.name, target_dir.display());
    Ok(())
}

fn cmd_skills_remove(config_path: &Option<PathBuf>, name: &str) -> Result<()> {
    let config = load_config(config_path)?;
    let skills = neko::skills::load_skills(&config.workspace_path())?;

    let skill = skills
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| NekoError::Config(format!("Skill '{name}' not found")))?;

    std::fs::remove_dir_all(&skill.path)?;
    println!("Removed skill '{name}'.");
    Ok(())
}
