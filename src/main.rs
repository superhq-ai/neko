use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use tokio::sync::mpsc;
use tracing::info;
use tracing_subscriber::EnvFilter;

use neko::channels::Channel;
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
    /// Cron job management
    Cron {
        #[command(subcommand)]
        action: CronAction,
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

#[derive(Subcommand)]
enum CronAction {
    /// List all cron jobs
    List,
    /// Add a new cron job
    Add {
        /// The prompt to send to the agent
        prompt: String,
        /// Cron expression (e.g. "0 0 9 * * *" for daily at 9am)
        #[arg(short, long)]
        schedule: Option<String>,
        /// One-shot datetime (e.g. "2026-02-17 09:00")
        #[arg(long)]
        at: Option<String>,
        /// Human-readable name for the job
        #[arg(short, long)]
        name: Option<String>,
        /// Announce results to a channel (e.g. "telegram:123456")
        #[arg(long)]
        announce: Option<String>,
        /// Keep one-shot jobs after execution
        #[arg(long)]
        keep_after_run: bool,
    },
    /// Edit an existing cron job
    Edit {
        /// Job ID or name
        id: String,
        /// Update the prompt
        #[arg(short, long)]
        prompt: Option<String>,
        /// Update the cron schedule
        #[arg(short, long)]
        schedule: Option<String>,
        /// Update the name
        #[arg(short, long)]
        name: Option<String>,
        /// Enable or disable the job
        #[arg(short, long)]
        enabled: Option<bool>,
        /// Set announce target (e.g. "telegram:123456"), or "none" to clear
        #[arg(long)]
        announce: Option<String>,
    },
    /// Remove a cron job
    Remove {
        /// Job ID or name
        id: String,
    },
    /// Show execution history
    History {
        /// Number of entries to show
        #[arg(short, long, default_value = "20")]
        lines: usize,
    },
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
            SessionAction::List => cmd_sessions_list(&cli.config).await?,
            SessionAction::Clear => cmd_sessions_clear(&cli.config).await?,
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
        Commands::Cron { action } => match action {
            CronAction::List => cmd_cron_list(&cli.config)?,
            CronAction::Add {
                prompt,
                schedule,
                at,
                name,
                announce,
                keep_after_run,
            } => cmd_cron_add(&cli.config, &prompt, schedule, at, name, announce, keep_after_run)?,
            CronAction::Edit {
                id,
                prompt,
                schedule,
                name,
                enabled,
                announce,
            } => cmd_cron_edit(&cli.config, &id, prompt, schedule, name, enabled, announce)?,
            CronAction::Remove { id } => cmd_cron_remove(&cli.config, &id)?,
            CronAction::History { lines } => cmd_cron_history(&cli.config, lines)?,
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
    neko::tools::register_core_tools(&mut registry, &config.tools);

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

    let enable_telegram = Confirm::new("Enable Telegram bot?")
        .with_default(false)
        .prompt()
        .map_err(|e| NekoError::Config(format!("Prompt cancelled: {e}")))?;

    let (telegram_token, telegram_users) = if enable_telegram {
        let token = Text::new("Telegram bot token:")
            .with_default("${TELEGRAM_BOT_TOKEN}")
            .with_help_message("Use ${VAR_NAME} to reference an env variable")
            .prompt()
            .map_err(|e| NekoError::Config(format!("Prompt cancelled: {e}")))?;

        let users = Text::new("Allowed Telegram user IDs (comma-separated):")
            .with_default("")
            .with_help_message("Leave empty to allow all users")
            .prompt()
            .map_err(|e| NekoError::Config(format!("Prompt cancelled: {e}")))?;

        (Some(token), users)
    } else {
        (None, String::new())
    };

    // Build config TOML
    let api_key_line = match &api_key_str {
        Some(k) => format!("api_key = \"{k}\""),
        None => "# api_key = \"\"".to_string(),
    };

    let telegram_section = match &telegram_token {
        Some(token) => {
            let users_array = if telegram_users.trim().is_empty() {
                "[]".to_string()
            } else {
                let ids: Vec<&str> = telegram_users.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
                format!("[{}]", ids.join(", "))
            };
            format!(
                "[channels.telegram]\nenabled = true\nbot_token = \"{token}\"\nallowed_users = {users_array}\n"
            )
        }
        None => "# [channels.telegram]\n# enabled = true\n# bot_token = \"${TELEGRAM_BOT_TOKEN}\"\n# allowed_users = []\n".to_string(),
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
exec_timeout_secs = 1800
exec_yield_ms = 10000

[heartbeat]
enabled = {enable_heartbeat}
interval_secs = 3600

{telegram_section}
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
    let cron_dir = ws.join("cron");

    std::fs::create_dir_all(&memory_dir)?;
    std::fs::create_dir_all(&sessions_dir)?;
    std::fs::create_dir_all(&skills_dir)?;
    std::fs::create_dir_all(&cron_dir)?;

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
    let cron_dir = workspace.join("cron");

    std::fs::create_dir_all(&memory_dir)?;
    std::fs::create_dir_all(&sessions_dir)?;
    std::fs::create_dir_all(&skills_dir)?;
    std::fs::create_dir_all(&cron_dir)?;

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

    // Ensure sessions directory exists
    let sessions_dir = workspace.join("sessions");
    let _ = std::fs::create_dir_all(&sessions_dir);

    // Build agent
    let agent = Arc::new(build_agent_from_config(&config).await?);

    // Build session store
    let session_store = Arc::new(neko::session::SessionStore::new(
        sessions_dir,
        config.session.clone(),
    ));
    session_store.load_from_disk().await?;

    // Build gateway
    let config_arc = Arc::new(config.clone());
    let gateway = Arc::new(neko::gateway::Gateway::new(
        agent,
        session_store.clone(),
        config_arc.clone(),
    ));

    // Outbound channel — shared between Telegram and cron scheduler.
    // Created unconditionally so the cron scheduler can always announce.
    let (outbound_tx, outbound_rx) = mpsc::channel::<neko::channels::OutboundMessage>(64);
    let mut cron_outbound_tx: Option<mpsc::Sender<neko::channels::OutboundMessage>> = None;

    // Start Telegram channel if configured
    if let Some(ref tg_config) = config.channels.telegram {
        if tg_config.enabled {
            let tg_channel = neko::channels::telegram::TelegramChannel::new(tg_config.clone())?;
            let (inbound_tx, mut inbound_rx) = mpsc::channel::<neko::channels::InboundMessage>(64);

            // Clone outbound_tx for the message handler before moving outbound_rx
            let outbound_tx_handler = outbound_tx.clone();
            cron_outbound_tx = Some(outbound_tx.clone());

            // Spawn Telegram polling loop
            tokio::spawn(async move {
                if let Err(e) = tg_channel.start(inbound_tx, outbound_rx).await {
                    tracing::error!("Telegram channel error: {e}");
                }
            });

            // Spawn message handler: inbound → gateway → outbound
            let gw = gateway.clone();
            tokio::spawn(async move {
                while let Some(inbound) = inbound_rx.recv().await {
                    let gw = gw.clone();
                    let tx = outbound_tx_handler.clone();
                    tokio::spawn(async move {
                        match gw.handle_message(inbound).await {
                            Ok(outbound) => {
                                if let Err(e) = tx.send(outbound).await {
                                    tracing::error!("Failed to send outbound: {e}");
                                }
                            }
                            Err(e) => {
                                tracing::error!("Gateway error: {e}");
                            }
                        }
                    });
                }
            });

            info!("Telegram channel started");
        }
    }

    // Start cron scheduler
    let cron_jobs = neko::cron::load_jobs(&workspace).unwrap_or_default();
    neko::cron::spawn_scheduler(
        gateway.agent.clone(),
        workspace.clone(),
        cron_outbound_tx,
    );

    // Build HTTP server
    let state = Arc::new(neko::api::AppState {
        gateway,
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
    if config.channels.telegram.as_ref().map_or(false, |t| t.enabled) {
        println!("  Telegram:  enabled");
    }
    if !cron_jobs.is_empty() {
        let enabled = cron_jobs.iter().filter(|j| j.enabled).count();
        println!("  Cron:      {} jobs ({} enabled)", cron_jobs.len(), enabled);
    }
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
    let agent = build_agent_from_config(&config).await?;
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

async fn cmd_sessions_list(config_path: &Option<PathBuf>) -> Result<()> {
    let config = load_config(config_path)?;
    let sessions_dir = config.workspace_path().join("sessions");

    if !sessions_dir.exists() {
        println!("No sessions directory found.");
        return Ok(());
    }

    let store = neko::session::SessionStore::new(sessions_dir, config.session.clone());
    store.load_from_disk().await?;

    let metas = store.list().await;
    if metas.is_empty() {
        println!("No active sessions.");
        return Ok(());
    }

    for meta in metas {
        let channel = meta.channel.as_deref().unwrap_or("-");
        let name = meta.display_name.as_deref().unwrap_or("-");
        println!(
            "{}\t{}\tturns={}\ttokens={}/{}\tchannel={}\tname={}\tupdated={}",
            meta.key,
            &meta.session_id[..8],
            meta.turn_count,
            meta.input_tokens,
            meta.output_tokens,
            channel,
            name,
            meta.updated_at.format("%Y-%m-%d %H:%M"),
        );
    }

    Ok(())
}

async fn cmd_sessions_clear(config_path: &Option<PathBuf>) -> Result<()> {
    let config = load_config(config_path)?;
    let sessions_dir = config.workspace_path().join("sessions");

    if !sessions_dir.exists() {
        println!("No sessions directory found.");
        return Ok(());
    }

    let store = neko::session::SessionStore::new(sessions_dir, config.session.clone());
    store.load_from_disk().await?;
    store.clear_all().await?;

    println!("All sessions cleared.");
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

// ---------------------------------------------------------------------------
// Cron commands
// ---------------------------------------------------------------------------

fn cmd_cron_list(config_path: &Option<PathBuf>) -> Result<()> {
    let config = load_config(config_path)?;
    let jobs = neko::cron::load_jobs(&config.workspace_path())?;

    if jobs.is_empty() {
        println!("No cron jobs configured.");
        return Ok(());
    }

    for job in &jobs {
        let name = job.name.as_deref().unwrap_or("-");
        let status = if job.enabled { "enabled" } else { "disabled" };
        let schedule = match &job.schedule {
            neko::cron::Schedule::Cron { expr } => format!("cron: {expr}"),
            neko::cron::Schedule::At { datetime } => {
                format!("at: {}", datetime.format("%Y-%m-%d %H:%M"))
            }
        };
        let announce = job
            .announce
            .as_ref()
            .map(|a| format!("{}:{}", a.channel, a.recipient_id))
            .unwrap_or_else(|| "-".into());
        let last = job
            .last_run_at
            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "never".into());
        let failures = job.retry.consecutive_failures;

        println!(
            "{}\t{}\t{}\t{}\tannounce={}\tlast={}\tfailures={}",
            job.id, name, status, schedule, announce, last, failures
        );
    }

    Ok(())
}

fn cmd_cron_add(
    config_path: &Option<PathBuf>,
    prompt: &str,
    schedule: Option<String>,
    at: Option<String>,
    name: Option<String>,
    announce: Option<String>,
    keep_after_run: bool,
) -> Result<()> {
    let config = load_config(config_path)?;
    let workspace = config.workspace_path();

    let sched = match (schedule, at) {
        (Some(expr), None) => {
            neko::cron::validate_cron_expr(&expr)?;
            neko::cron::Schedule::Cron { expr }
        }
        (None, Some(dt_str)) => {
            let datetime = parse_datetime(&dt_str)?;
            neko::cron::Schedule::At { datetime }
        }
        (Some(_), Some(_)) => {
            return Err(NekoError::Cron(
                "specify either --schedule or --at, not both".into(),
            ));
        }
        (None, None) => {
            return Err(NekoError::Cron(
                "must specify --schedule or --at".into(),
            ));
        }
    };

    let announce_target = announce.map(|s| neko::cron::parse_announce(&s)).transpose()?;

    let job = neko::cron::CronJob {
        id: neko::cron::new_job_id(),
        name,
        prompt: prompt.to_string(),
        schedule: sched,
        announce: announce_target,
        enabled: true,
        keep_after_run,
        created_at: Utc::now(),
        last_run_at: None,
        retry: neko::cron::RetryState::default(),
    };

    let mut jobs = neko::cron::load_jobs(&workspace)?;
    let label = job.name.as_deref().unwrap_or(&job.id).to_string();
    jobs.push(job);
    neko::cron::save_jobs(&workspace, &jobs)?;

    println!("Created cron job: {label}");
    Ok(())
}

fn cmd_cron_edit(
    config_path: &Option<PathBuf>,
    id_or_name: &str,
    prompt: Option<String>,
    schedule: Option<String>,
    name: Option<String>,
    enabled: Option<bool>,
    announce: Option<String>,
) -> Result<()> {
    let config = load_config(config_path)?;
    let workspace = config.workspace_path();
    let mut jobs = neko::cron::load_jobs(&workspace)?;

    let idx = neko::cron::find_job(&jobs, id_or_name)
        .ok_or_else(|| NekoError::Cron(format!("job '{id_or_name}' not found")))?;

    if let Some(p) = prompt {
        jobs[idx].prompt = p;
    }
    if let Some(expr) = schedule {
        neko::cron::validate_cron_expr(&expr)?;
        jobs[idx].schedule = neko::cron::Schedule::Cron { expr };
    }
    if let Some(n) = name {
        jobs[idx].name = Some(n);
    }
    if let Some(e) = enabled {
        jobs[idx].enabled = e;
        // Reset retry state when re-enabling
        if e {
            jobs[idx].retry = neko::cron::RetryState::default();
        }
    }
    if let Some(a) = announce {
        if a == "none" {
            jobs[idx].announce = None;
        } else {
            jobs[idx].announce = Some(neko::cron::parse_announce(&a)?);
        }
    }

    neko::cron::save_jobs(&workspace, &jobs)?;
    println!("Updated job: {}", jobs[idx].name.as_deref().unwrap_or(&jobs[idx].id));
    Ok(())
}

fn cmd_cron_remove(config_path: &Option<PathBuf>, id_or_name: &str) -> Result<()> {
    let config = load_config(config_path)?;
    let workspace = config.workspace_path();
    let mut jobs = neko::cron::load_jobs(&workspace)?;

    let idx = neko::cron::find_job(&jobs, id_or_name)
        .ok_or_else(|| NekoError::Cron(format!("job '{id_or_name}' not found")))?;

    let removed = jobs.remove(idx);
    neko::cron::save_jobs(&workspace, &jobs)?;

    let label = removed.name.as_deref().unwrap_or(&removed.id);
    println!("Removed job: {label}");
    Ok(())
}

fn cmd_cron_history(config_path: &Option<PathBuf>, lines: usize) -> Result<()> {
    let config = load_config(config_path)?;
    let entries = neko::cron::read_history(&config.workspace_path(), lines)?;

    if entries.is_empty() {
        println!("No execution history.");
        return Ok(());
    }

    for entry in &entries {
        let name = entry.job_name.as_deref().unwrap_or(&entry.job_id);
        let status = if entry.success { "OK" } else { "FAIL" };
        let duration = (entry.finished_at - entry.started_at).num_milliseconds() as f64 / 1000.0;
        let detail = if entry.success {
            entry
                .response
                .as_deref()
                .map(|r| {
                    let first_line = r.lines().next().unwrap_or(r);
                    if first_line.len() > 80 {
                        format!("{}...", &first_line[..80])
                    } else {
                        first_line.to_string()
                    }
                })
                .unwrap_or_default()
        } else {
            entry.error.as_deref().unwrap_or("unknown error").to_string()
        };

        println!(
            "{}\t{}\t{}\t{:.1}s\t{}",
            entry.started_at.format("%Y-%m-%d %H:%M:%S"),
            name,
            status,
            duration,
            detail,
        );
    }

    Ok(())
}

fn parse_datetime(s: &str) -> Result<DateTime<Utc>> {
    // Try "YYYY-MM-DD HH:MM" (local time assumed)
    let formats = ["%Y-%m-%d %H:%M", "%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"];
    for fmt in &formats {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            let local = chrono::Local::now().timezone();
            let local_dt = naive
                .and_local_timezone(local)
                .single()
                .ok_or_else(|| NekoError::Cron(format!("ambiguous datetime: {s}")))?;
            return Ok(local_dt.with_timezone(&Utc));
        }
    }
    // Try RFC 3339
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    Err(NekoError::Cron(format!(
        "could not parse datetime: '{s}' (expected YYYY-MM-DD HH:MM)"
    )))
}
