# neko
<img width="1536" height="1024" alt="ChatGPT Image Feb 16, 2026, 01_17_27 PM (1)" src="https://github.com/user-attachments/assets/340d14d7-1674-47ca-805b-fe33e87eb04f" />

Lightweight AI agent runtime targeting Raspberry Pi Zero 2W and low-end VPS. File-based memory, MCP tool support, Telegram integration — all in a single binary.

## Install

**Shell (Linux / macOS):**

```sh
curl -fsSL https://raw.githubusercontent.com/superhq-ai/neko/main/install.sh | sh
```

Installs to `~/.local/bin` by default. Override with `NEKO_INSTALL_DIR`:

```sh
NEKO_INSTALL_DIR=/usr/local/bin curl -fsSL https://raw.githubusercontent.com/superhq-ai/neko/main/install.sh | sh
```

**From source:**

```sh
git clone https://github.com/superhq-ai/neko.git
cd neko
cargo build --release
# binary at target/release/neko
```

## Quick start

```sh
# Create default config and workspace
neko init

# Or run interactive setup
neko init --interactive

# Edit config (defaults to ~/.neko/config.toml)
neko config edit

# Start the agent gateway
neko start

# Send a test message
neko message "Hello, what can you do?"
```

## Configuration

Config lives at `~/.neko/config.toml`. Key sections:

```toml
[gateway]
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

[tools]
sandbox = false
exec_timeout_secs = 1800

# MCP servers
[mcp.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
```

Environment variables are substituted via `${VAR_NAME}` syntax.

## CLI

```
neko init              Initialize config and workspace
neko init -i           Interactive setup
neko start             Start the gateway server
neko stop              Stop the running gateway
neko status            Show gateway status
neko logs [-l N]       Show recent logs
neko message <text>    Send a message to the agent
neko config show       Print current config
neko config edit       Open config in $EDITOR
neko sessions list     List active sessions
neko sessions clear    Clear all sessions
neko memory list       List memory files
neko memory search Q   Search memory files
neko skills list       List installed skills
neko skills install P  Install a skill from path
neko skills remove N   Remove a skill by name
neko cron list         List all cron jobs
neko cron add <prompt> Add a scheduled job
neko cron edit <id>    Edit a cron job
neko cron remove <id>  Remove a cron job
neko cron history      Show execution history
```

## Features

### OpenResponses-compatible LLM

Works with any provider exposing `POST /v1/responses` — OpenAI, Anthropic (via proxy), Ollama, or self-hosted models.

### File-based memory

No database. No embeddings. No vector store. Memory is just markdown files — readable, editable, and version-controllable by both humans and the agent.

The system uses a **two-tier architecture**:

- **`memory/MEMORY.md`** (core memory) — long-term facts and user preferences, always injected into the system prompt. Capped at 2000 chars with automatic warnings when the agent needs to compact it, forcing the agent to self-curate rather than accumulate stale context.
- **`memory/YYYY-MM-DD.md`** (daily logs) — ephemeral session notes. Today's and yesterday's logs are loaded automatically, giving the agent a rolling two-day window of recent context without unbounded growth.
- **`memory/recall/*.md`** (recall) — past conversations, auto-logged. Searchable via `memory_search` with regex support for when the agent needs to reach further back.

The agent manages its own memory through three tools:
- `memory_write` — write or append to any memory file
- `memory_replace` — surgical find-and-replace for updating facts (empty replacement = delete)
- `memory_search` — case-insensitive search across all memory files with regex support

This means the agent actively maintains its own knowledge base — correcting outdated facts, promoting ephemeral notes to long-term memory, and compacting when context gets bloated. All of it happens in plain text files you can `cat`, `grep`, or commit to git.

### MCP tool support

Connect external tools via [Model Context Protocol](https://modelcontextprotocol.io) stdio transport. Tools are discovered automatically and registered in the agent's tool registry.

### Skills

Install [AgentSkills.io](https://agentskills.io)-compatible skills as `SKILL.md` files with YAML frontmatter. Skills use progressive disclosure — metadata is always in context, full body is loaded on activation.

### Telegram

Enable the Telegram channel to interact with the agent via a Telegram bot:

```toml
[channels.telegram]
enabled = true
bot_token = "${TELEGRAM_BOT_TOKEN}"
allowed_users = [123456789]
```

### Cron jobs

Schedule recurring or one-shot tasks that the agent executes autonomously. Results are delivered back to the originating channel (Telegram, HTTP, etc.).

```sh
# Add a recurring job — every day at 9am
neko cron add "summarize yesterday's news" --schedule "0 0 9 * * *" --name morning-digest

# One-shot job
neko cron add "remind me to call the dentist" --at "2026-02-17 09:00"

# List jobs
neko cron list

# Announce results to a Telegram chat
neko cron edit morning-digest --announce "telegram:123456"

# View execution history
neko cron history --lines 10
```

The agent can also create cron jobs itself via the `cron_manage` tool — when a user on Telegram says "remind me every morning at 9am", the agent creates the job and automatically routes results back to that chat. No manual wiring needed.

Jobs are stored at `workspace/cron/jobs.json` and history at `workspace/cron/history.jsonl`. The scheduler ticks every 15 seconds with exponential backoff on failures (30s → 1m → 5m → 15m → 60m cap).

### Sandboxed Python

Built-in Python interpreter via [monty](https://github.com/pydantic/monty) for safe code execution with configurable memory and recursion limits.

## Supported platforms

| Target | Notes |
|--------|-------|
| `x86_64-unknown-linux-gnu` | Standard Linux |
| `x86_64-unknown-linux-musl` | Static binary, Alpine-friendly |
| `aarch64-unknown-linux-gnu` | Raspberry Pi, ARM servers |
| `x86_64-apple-darwin` | Intel Mac |
| `aarch64-apple-darwin` | Apple Silicon |

## License

MIT
