use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use teloxide::net::default_reqwest_settings;
use teloxide::payloads::GetUpdatesSetters;
use teloxide::payloads::{SendAudioSetters, SendDocumentSetters, SendPhotoSetters, SendVideoSetters};
use teloxide::requests::Requester;
use teloxide::types::{ChatId, ChatKind, InputFile, UpdateKind};
use teloxide::Bot;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::channels::{Channel, InboundMessage, OutboundMessage};
use crate::config::TelegramConfig;
use crate::error::{NekoError, Result};

pub struct TelegramChannel {
    config: TelegramConfig,
    bot: Bot,
    running: Arc<AtomicBool>,
}

impl TelegramChannel {
    pub fn new(config: TelegramConfig) -> Result<Self> {
        let token = config
            .bot_token
            .as_deref()
            .ok_or_else(|| NekoError::Channel("Telegram bot_token is required".to_string()))?;

        // Default teloxide client has a 17s timeout, too short for 30s long-poll.
        // Build a client with a 60s timeout to accommodate long-polling.
        let client = default_reqwest_settings()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| NekoError::Channel(format!("Failed to build HTTP client: {e}")))?;

        let bot = Bot::with_client(token, client);
        Ok(Self {
            config,
            bot,
            running: Arc::new(AtomicBool::new(false)),
        })
    }

}

#[async_trait]
impl Channel for TelegramChannel {
    fn name(&self) -> &str {
        "telegram"
    }

    async fn start(
        &self,
        inbound_tx: mpsc::Sender<InboundMessage>,
        mut outbound_rx: mpsc::Receiver<OutboundMessage>,
    ) -> Result<()> {
        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();
        let bot = self.bot.clone();
        let allowed_users = self.config.allowed_users.clone();

        // Spawn outbound message sender
        let send_bot = bot.clone();
        tokio::spawn(async move {
            while let Some(msg) = outbound_rx.recv().await {
                let chat_id: i64 = match msg.recipient_id.parse() {
                    Ok(id) => id,
                    Err(e) => {
                        error!("Invalid chat_id '{}': {e}", msg.recipient_id);
                        continue;
                    }
                };

                let cid = ChatId(chat_id);

                if msg.attachments.is_empty() {
                    // Text-only message
                    if let Err(e) = send_bot.send_message(cid, &msg.text).await {
                        error!("Failed to send Telegram message: {e}");
                    }
                    continue;
                }

                // Has attachments — decide caption strategy.
                // Telegram captions are limited to 1024 chars.
                let text = msg.text.trim();
                let text_fits_caption = text.len() <= 1024;

                // If text is too long for a caption, send it as a separate message first.
                if !text.is_empty() && !text_fits_caption {
                    if let Err(e) = send_bot.send_message(cid, text).await {
                        error!("Failed to send Telegram text message: {e}");
                    }
                }

                for (i, attachment) in msg.attachments.iter().enumerate() {
                    // First attachment gets caption if text fits
                    let caption = if i == 0 && !text.is_empty() && text_fits_caption {
                        Some(text)
                    } else {
                        None
                    };

                    let input_file = InputFile::file(&attachment.path);
                    let result = send_media(&send_bot, cid, input_file, &attachment.mime_type, caption).await;
                    if let Err(e) = result {
                        error!(
                            "Failed to send Telegram media {}: {e}",
                            attachment.path.display()
                        );
                    }
                }
            }
        });

        // Long-poll loop for updates
        let mut offset: i32 = 0;

        while running.load(Ordering::SeqCst) {
            let updates = match bot
                .get_updates()
                .offset(offset)
                .timeout(30)
                .await
            {
                Ok(updates) => updates,
                Err(e) => {
                    warn!("Telegram getUpdates error: {e}");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    continue;
                }
            };

            for update in &updates {
                offset = update.id.0 as i32 + 1;

                let UpdateKind::Message(ref message) = update.kind else {
                    continue;
                };

                let Some(text) = message.text() else {
                    continue;
                };

                let Some(from) = &message.from else {
                    continue;
                };

                let user_id = from.id.0 as i64;

                // Check allowed_users
                if !allowed_users.is_empty() && !allowed_users.contains(&user_id) {
                    debug!("Ignoring message from unauthorized user {user_id}");
                    continue;
                }

                let chat_id = message.chat.id.0;
                let is_group = matches!(
                    message.chat.kind,
                    ChatKind::Public(_)
                );

                let display_name = from.first_name.clone();
                let sender_id = user_id.to_string();

                let (group_id, reply_to) = if is_group {
                    (Some(chat_id.to_string()), chat_id.to_string())
                } else {
                    (None, chat_id.to_string())
                };

                let inbound = InboundMessage {
                    channel: "telegram".to_string(),
                    sender_id,
                    text: text.to_string(),
                    is_group,
                    group_id,
                    display_name: Some(display_name),
                    reply_to,
                };

                if let Err(e) = inbound_tx.send(inbound).await {
                    error!("Failed to forward inbound message: {e}");
                }
            }
        }

        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.running.store(false, Ordering::SeqCst);
        info!("Telegram channel stopped");
        Ok(())
    }
}

/// Dispatch a media file via the appropriate Telegram API based on MIME type.
async fn send_media(
    bot: &Bot,
    chat_id: ChatId,
    file: InputFile,
    mime_type: &str,
    caption: Option<&str>,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if mime_type.starts_with("image/") {
        let mut req = bot.send_photo(chat_id, file);
        if let Some(c) = caption {
            req = req.caption(c);
        }
        req.await?;
    } else if mime_type.starts_with("audio/") {
        let mut req = bot.send_audio(chat_id, file);
        if let Some(c) = caption {
            req = req.caption(c);
        }
        req.await?;
    } else if mime_type.starts_with("video/") {
        let mut req = bot.send_video(chat_id, file);
        if let Some(c) = caption {
            req = req.caption(c);
        }
        req.await?;
    } else {
        let mut req = bot.send_document(chat_id, file);
        if let Some(c) = caption {
            req = req.caption(c);
        }
        req.await?;
    }
    Ok(())
}
