pub mod telegram;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::error::Result;

/// An inbound message from any channel.
pub struct InboundMessage {
    pub channel: String,
    pub sender_id: String,
    pub text: String,
    pub is_group: bool,
    pub group_id: Option<String>,
    pub display_name: Option<String>,
    /// The chat/recipient ID to reply to (may differ from sender_id in groups).
    pub reply_to: String,
}

/// An outbound message to send back through a channel.
pub struct OutboundMessage {
    pub channel: String,
    pub recipient_id: String,
    pub text: String,
}

/// Trait for external channel integrations.
#[async_trait]
pub trait Channel: Send + Sync {
    fn name(&self) -> &str;

    async fn start(
        &self,
        inbound_tx: mpsc::Sender<InboundMessage>,
        mut outbound_rx: mpsc::Receiver<OutboundMessage>,
    ) -> Result<()>;

    async fn stop(&self) -> Result<()>;
}
