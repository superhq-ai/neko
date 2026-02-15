use std::sync::Arc;

use tracing::{debug, info};

use crate::agent::Agent;
use crate::channels::{InboundMessage, OutboundMessage};
use crate::config::Config;
use crate::error::Result;
use crate::session::SessionStore;
use crate::tools::ChannelContext;

pub struct Gateway {
    pub agent: Arc<Agent>,
    pub session_store: Arc<SessionStore>,
    pub config: Arc<Config>,
}

impl Gateway {
    pub fn new(
        agent: Arc<Agent>,
        session_store: Arc<SessionStore>,
        config: Arc<Config>,
    ) -> Self {
        Self {
            agent,
            session_store,
            config,
        }
    }

    /// Core routing: inbound message → session → agent → outbound message.
    pub async fn handle_message(&self, inbound: InboundMessage) -> Result<OutboundMessage> {
        let text = inbound.text.trim().to_string();

        // Resolve session key
        let key = self.session_store.resolve_key(
            &inbound.channel,
            &inbound.sender_id,
            inbound.is_group,
            inbound.group_id.as_deref(),
        );

        debug!("Resolved session key: {}", key);

        // Get or create session
        let session_id = self
            .session_store
            .get_or_create(&key, Some(&inbound.channel), inbound.display_name.as_deref())
            .await?;

        // Handle /new and /reset commands
        if text == "/new" || text == "/reset" {
            self.session_store.reset(&session_id).await?;
            return Ok(OutboundMessage {
                channel: inbound.channel,
                recipient_id: inbound.reply_to,
                text: "Session reset. Starting fresh.".to_string(),
                attachments: Vec::new(),
            });
        }

        // Check automatic reset (daily/idle)
        if self.session_store.check_reset(&session_id).await? {
            info!("Auto-reset triggered for session {session_id}");
        }

        // Get history + previous response ID for reasoning chaining
        let (history, prev_response_id) =
            self.session_store.get_history(&session_id).await?;

        let channel_ctx = ChannelContext {
            channel: inbound.channel.clone(),
            recipient_id: inbound.reply_to.clone(),
        };

        let result = self
            .agent
            .run_turn_with_history(history, &text, prev_response_id, Some(channel_ctx))
            .await?;

        // Persist updated history + new response ID
        self.session_store
            .update_history(
                &session_id,
                result.history,
                result.usage.as_ref(),
                result.last_response_id,
            )
            .await?;

        Ok(OutboundMessage {
            channel: inbound.channel,
            recipient_id: inbound.reply_to,
            text: result.text,
            attachments: result.attachments,
        })
    }

    /// Handle a message for an explicitly specified session ID (HTTP API).
    pub async fn handle_message_with_session(
        &self,
        session_id: &str,
        text: &str,
    ) -> Result<(String, String)> {
        let (history, prev_response_id) =
            self.session_store.get_history(session_id).await?;

        let result = self
            .agent
            .run_turn_with_history(history, text, prev_response_id, None)
            .await?;

        self.session_store
            .update_history(
                session_id,
                result.history,
                result.usage.as_ref(),
                result.last_response_id,
            )
            .await?;

        Ok((result.text, session_id.to_string()))
    }

    /// Handle message from HTTP channel (may or may not have session_id).
    pub async fn handle_http_message(
        &self,
        text: &str,
        session_id: Option<&str>,
        sender_id: Option<&str>,
    ) -> Result<(String, String)> {
        let sid = if let Some(id) = session_id {
            // Verify it exists
            let _ = self.session_store.get_history(id).await?;
            id.to_string()
        } else {
            // Create/get a session for the HTTP channel
            let peer = sender_id.unwrap_or("http-default");
            let key = self.session_store.resolve_key("http", peer, false, None);
            self.session_store
                .get_or_create(&key, Some("http"), None)
                .await?
        };

        // Check automatic reset
        let _ = self.session_store.check_reset(&sid).await;

        let (history, prev_response_id) =
            self.session_store.get_history(&sid).await?;

        let channel_ctx = ChannelContext {
            channel: "http".to_string(),
            recipient_id: sender_id.unwrap_or("http-default").to_string(),
        };

        let result = self
            .agent
            .run_turn_with_history(history, text, prev_response_id, Some(channel_ctx))
            .await?;

        self.session_store
            .update_history(&sid, result.history, result.usage.as_ref(), result.last_response_id)
            .await?;

        Ok((result.text, sid))
    }
}
