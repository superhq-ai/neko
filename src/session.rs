use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Local, Timelike, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

use crate::config::{DmScope, ResetMode, SessionConfig};
use crate::error::{NekoError, Result};
use crate::llm;

// ---------------------------------------------------------------------------
// SessionKey
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionKey(pub String);

impl SessionKey {
    /// Default DM session: `neko:main`
    pub fn main_dm() -> Self {
        Self("neko:main".to_string())
    }

    /// Per-channel-peer DM: `neko:<channel>:dm:<peer_id>`
    pub fn channel_peer(channel: &str, peer_id: &str) -> Self {
        Self(format!("neko:{channel}:dm:{peer_id}"))
    }

    /// Group session: `neko:<channel>:group:<group_id>`
    pub fn group(channel: &str, group_id: &str) -> Self {
        Self(format!("neko:{channel}:group:{group_id}"))
    }

    /// Convert colons to underscores for safe filenames.
    pub fn to_filename(&self) -> String {
        self.0.replace(':', "_")
    }
}

impl std::fmt::Display for SessionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ---------------------------------------------------------------------------
// SessionMeta
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub session_id: String,
    pub key: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub turn_count: u32,
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Last API response ID — enables `previous_response_id` chaining so the
    /// API can automatically handle reasoning-item pairing across turns.
    /// Cleared on session reset; becomes `None` after a server restart (the
    /// API may have forgotten it), causing a graceful fallback to full-history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_response_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Session (in-memory)
// ---------------------------------------------------------------------------

pub struct Session {
    pub meta: SessionMeta,
    pub history: Vec<llm::Item>,
}

// ---------------------------------------------------------------------------
// SessionStore
// ---------------------------------------------------------------------------

pub struct SessionStore {
    sessions_dir: PathBuf,
    /// Session ID → Session (guarded by per-session mutex)
    sessions: RwLock<HashMap<String, Arc<Mutex<Session>>>>,
    /// Session key string → session ID
    key_index: RwLock<HashMap<String, String>>,
    config: SessionConfig,
}

impl SessionStore {
    pub fn new(sessions_dir: PathBuf, config: SessionConfig) -> Self {
        Self {
            sessions_dir,
            sessions: RwLock::new(HashMap::new()),
            key_index: RwLock::new(HashMap::new()),
            config,
        }
    }

    /// Load existing sessions from `sessions.json` on startup.
    pub async fn load_from_disk(&self) -> Result<()> {
        let meta_path = self.sessions_dir.join("sessions.json");
        if !meta_path.exists() {
            return Ok(());
        }

        let content = std::fs::read_to_string(&meta_path)?;
        let meta_map: HashMap<String, SessionMeta> = serde_json::from_str(&content)
            .map_err(|e| NekoError::Session(format!("Failed to parse sessions.json: {e}")))?;

        let mut sessions = self.sessions.write().await;
        let mut key_index = self.key_index.write().await;

        for (key, meta) in meta_map {
            let session_id = meta.session_id.clone();
            let history = self.load_transcript(&session_id)?;

            key_index.insert(key, session_id.clone());
            sessions.insert(
                session_id,
                Arc::new(Mutex::new(Session { meta, history })),
            );
        }

        info!("Loaded {} session(s) from disk", sessions.len());
        Ok(())
    }

    /// Resolve an inbound message to a session key based on dmScope config.
    pub fn resolve_key(
        &self,
        channel: &str,
        sender_id: &str,
        is_group: bool,
        group_id: Option<&str>,
    ) -> SessionKey {
        if is_group {
            if let Some(gid) = group_id {
                return SessionKey::group(channel, gid);
            }
        }

        match self.config.dm_scope {
            DmScope::Main => SessionKey::main_dm(),
            DmScope::PerChannelPeer => SessionKey::channel_peer(channel, sender_id),
        }
    }

    /// Get or create a session for the given key. Returns session_id.
    pub async fn get_or_create(
        &self,
        key: &SessionKey,
        channel: Option<&str>,
        display_name: Option<&str>,
    ) -> Result<String> {
        // Fast path: check if session already exists
        {
            let index = self.key_index.read().await;
            if let Some(session_id) = index.get(&key.0) {
                return Ok(session_id.clone());
            }
        }

        // Slow path: create new session
        let session_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();

        let meta = SessionMeta {
            session_id: session_id.clone(),
            key: key.0.clone(),
            created_at: now,
            updated_at: now,
            turn_count: 0,
            input_tokens: 0,
            output_tokens: 0,
            channel: channel.map(String::from),
            display_name: display_name.map(String::from),
            last_response_id: None,
        };

        let session = Session {
            meta,
            history: Vec::new(),
        };

        let mut sessions = self.sessions.write().await;
        let mut index = self.key_index.write().await;

        // Double-check after acquiring write lock
        if let Some(existing_id) = index.get(&key.0) {
            return Ok(existing_id.clone());
        }

        sessions.insert(session_id.clone(), Arc::new(Mutex::new(session)));
        index.insert(key.0.clone(), session_id.clone());

        info!("Created session {session_id} for key {}", key.0);
        self.persist_meta_inner(&sessions).await?;

        Ok(session_id)
    }

    /// Get a clone of the session history and the last response ID.
    pub async fn get_history(
        &self,
        session_id: &str,
    ) -> Result<(Vec<llm::Item>, Option<String>)> {
        let sessions = self.sessions.read().await;
        let session_lock = sessions
            .get(session_id)
            .ok_or_else(|| NekoError::Session(format!("Session not found: {session_id}")))?;
        let session = session_lock.lock().await;
        Ok((session.history.clone(), session.meta.last_response_id.clone()))
    }

    /// Update session history after an agent turn completes.
    pub async fn update_history(
        &self,
        session_id: &str,
        history: Vec<llm::Item>,
        usage: Option<&llm::Usage>,
        last_response_id: Option<String>,
    ) -> Result<()> {
        let sessions = self.sessions.read().await;
        let session_lock = sessions
            .get(session_id)
            .ok_or_else(|| NekoError::Session(format!("Session not found: {session_id}")))?;

        let mut session = session_lock.lock().await;

        // Compute new items to append to transcript (items added since last snapshot)
        let old_len = session.history.len();
        let new_items = if history.len() > old_len {
            &history[old_len..]
        } else {
            &history[..]
        };

        // Append new items to JSONL transcript
        if !new_items.is_empty() {
            self.append_to_transcript_inner(session_id, new_items)?;
        }

        session.history = history;
        session.meta.updated_at = Utc::now();
        session.meta.turn_count += 1;
        session.meta.last_response_id = last_response_id;

        if let Some(u) = usage {
            session.meta.input_tokens += u.input_tokens;
            session.meta.output_tokens += u.output_tokens;
        }

        drop(session);
        drop(sessions);
        self.persist_meta().await?;

        Ok(())
    }

    /// Check if a session should be reset (daily or idle).
    /// Returns true if the session was reset.
    pub async fn check_reset(&self, session_id: &str) -> Result<bool> {
        let sessions = self.sessions.read().await;
        let session_lock = match sessions.get(session_id) {
            Some(s) => s,
            None => return Ok(false),
        };

        let session = session_lock.lock().await;
        let should_reset = self.should_reset(&session.meta);
        drop(session);
        drop(sessions);

        if should_reset {
            self.reset(session_id).await?;
            return Ok(true);
        }

        Ok(false)
    }

    fn should_reset(&self, meta: &SessionMeta) -> bool {
        let now = Utc::now();

        let daily_triggered = match self.config.reset_mode {
            ResetMode::Daily | ResetMode::Both => {
                let local_now = Local::now();
                let local_updated: DateTime<Local> = meta.updated_at.into();

                // Reset if updated_at was before today's reset hour and now is after
                let reset_hour = self.config.reset_at_hour;
                if local_now.date_naive() > local_updated.date_naive() {
                    local_now.hour() >= reset_hour
                } else {
                    false
                }
            }
            ResetMode::Idle => false,
        };

        let idle_triggered = match self.config.reset_mode {
            ResetMode::Idle | ResetMode::Both => {
                if let Some(idle_mins) = self.config.idle_minutes {
                    let elapsed = now
                        .signed_duration_since(meta.updated_at)
                        .num_minutes();
                    elapsed >= idle_mins as i64
                } else {
                    false
                }
            }
            ResetMode::Daily => false,
        };

        daily_triggered || idle_triggered
    }

    /// Reset a session: archive old transcript, clear history.
    pub async fn reset(&self, session_id: &str) -> Result<()> {
        let sessions = self.sessions.read().await;
        let session_lock = sessions
            .get(session_id)
            .ok_or_else(|| NekoError::Session(format!("Session not found: {session_id}")))?;

        let mut session = session_lock.lock().await;

        // Archive old transcript
        let transcript_path = self.transcript_path(session_id);
        if transcript_path.exists() {
            let timestamp = Utc::now().format("%Y%m%dT%H%M%S");
            let archive_name = format!("{session_id}.{timestamp}.jsonl");
            let archive_path = self.sessions_dir.join(archive_name);
            if let Err(e) = std::fs::rename(&transcript_path, &archive_path) {
                warn!("Failed to archive transcript: {e}");
            } else {
                debug!("Archived transcript to {}", archive_path.display());
            }
        }

        session.history.clear();
        session.meta.updated_at = Utc::now();
        session.meta.turn_count = 0;
        session.meta.last_response_id = None;

        info!("Reset session {session_id}");

        drop(session);
        drop(sessions);
        self.persist_meta().await?;

        Ok(())
    }

    /// List all session metadata.
    pub async fn list(&self) -> Vec<SessionMeta> {
        let sessions = self.sessions.read().await;
        let mut metas = Vec::with_capacity(sessions.len());
        for session_lock in sessions.values() {
            let session = session_lock.lock().await;
            metas.push(session.meta.clone());
        }
        metas.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        metas
    }

    /// Delete a single session.
    pub async fn delete(&self, session_id: &str) -> Result<()> {
        let mut sessions = self.sessions.write().await;
        let mut index = self.key_index.write().await;

        if let Some(session_lock) = sessions.remove(session_id) {
            let session = session_lock.lock().await;
            index.remove(&session.meta.key);
        }

        // Remove transcript file
        let transcript = self.transcript_path(session_id);
        if transcript.exists() {
            std::fs::remove_file(&transcript)?;
        }

        self.persist_meta_inner(&sessions).await?;
        info!("Deleted session {session_id}");
        Ok(())
    }

    /// Clear all sessions.
    pub async fn clear_all(&self) -> Result<()> {
        let mut sessions = self.sessions.write().await;
        let mut index = self.key_index.write().await;

        sessions.clear();
        index.clear();

        // Remove all JSONL files
        if self.sessions_dir.exists() {
            for entry in std::fs::read_dir(&self.sessions_dir)? {
                let entry = entry?;
                if entry.path().extension().map_or(false, |e| e == "jsonl") {
                    std::fs::remove_file(entry.path())?;
                }
            }
        }

        // Write empty sessions.json
        self.persist_meta_inner(&sessions).await?;
        info!("Cleared all sessions");
        Ok(())
    }

    /// Get a session ID by key (if it exists).
    pub async fn get_session_id_by_key(&self, key: &SessionKey) -> Option<String> {
        let index = self.key_index.read().await;
        index.get(&key.0).cloned()
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn transcript_path(&self, session_id: &str) -> PathBuf {
        self.sessions_dir.join(format!("{session_id}.jsonl"))
    }

    fn append_to_transcript_inner(&self, session_id: &str, items: &[llm::Item]) -> Result<()> {
        use std::io::Write;
        let path = self.transcript_path(session_id);
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;

        for item in items {
            let json = serde_json::to_string(item)
                .map_err(|e| NekoError::Session(format!("Failed to serialize item: {e}")))?;
            writeln!(file, "{json}")?;
        }

        Ok(())
    }

    fn load_transcript(&self, session_id: &str) -> Result<Vec<llm::Item>> {
        let path = self.transcript_path(session_id);
        if !path.exists() {
            return Ok(Vec::new());
        }

        let content = std::fs::read_to_string(&path)?;
        let mut items = Vec::new();
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let item: llm::Item = serde_json::from_str(line)
                .map_err(|e| NekoError::Session(format!("Failed to parse transcript line: {e}")))?;
            items.push(item);
        }

        Ok(items)
    }

    async fn persist_meta(&self) -> Result<()> {
        let sessions = self.sessions.read().await;
        self.persist_meta_inner(&sessions).await
    }

    async fn persist_meta_inner(
        &self,
        sessions: &HashMap<String, Arc<Mutex<Session>>>,
    ) -> Result<()> {
        let _ = std::fs::create_dir_all(&self.sessions_dir);

        let mut meta_map: HashMap<String, SessionMeta> = HashMap::new();
        for session_lock in sessions.values() {
            let session = session_lock.lock().await;
            meta_map.insert(session.meta.key.clone(), session.meta.clone());
        }

        let json = serde_json::to_string_pretty(&meta_map)
            .map_err(|e| NekoError::Session(format!("Failed to serialize sessions: {e}")))?;

        // Atomic write: write to tmp, then rename
        let meta_path = self.sessions_dir.join("sessions.json");
        let tmp_path = self.sessions_dir.join("sessions.json.tmp");

        std::fs::write(&tmp_path, json.as_bytes())?;
        std::fs::rename(&tmp_path, &meta_path)?;

        Ok(())
    }
}
