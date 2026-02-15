use thiserror::Error;

#[derive(Error, Debug)]
pub enum NekoError {
    #[error("Config error: {0}")]
    Config(String),

    #[error("LLM error: {0}")]
    Llm(String),

    #[error("Tool error: {0}")]
    Tool(String),

    #[error("Memory error: {0}")]
    Memory(String),

    #[error("Session error: {0}")]
    Session(String),

    #[error("Agent error: {0}")]
    Agent(String),

    #[error("Channel error: {0}")]
    Channel(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),


    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),
}

pub type Result<T> = std::result::Result<T, NekoError>;
