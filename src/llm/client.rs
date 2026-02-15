use futures::StreamExt;
use reqwest_eventsource::{Event, EventSource};
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

use crate::error::{NekoError, Result};

use super::types::{Request, Response, StreamEvent};

pub struct Client {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
}

impl Client {
    pub fn new(base_url: &str, api_key: Option<&str>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.map(|s| s.to_string()),
        }
    }

    /// Send a non-streaming request and get the full response.
    pub async fn create_response(&self, request: &Request) -> Result<Response> {
        let url = format!("{}/v1/responses", self.base_url);

        let mut req = self.http.post(&url).json(request);

        if let Some(key) = &self.api_key {
            req = req.header("Authorization", format!("Bearer {key}"));
        }

        debug!("POST {url} model={}", request.model);

        let resp = req.send().await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(NekoError::Llm(format!(
                "API returned {status}: {body}"
            )));
        }

        let response: Response = resp.json().await?;
        Ok(response)
    }

    /// Send a streaming request, returning a channel of stream events.
    pub async fn create_response_stream(
        &self,
        request: &Request,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        let url = format!("{}/v1/responses", self.base_url);

        let mut req_builder = self.http.post(&url).json(request);

        if let Some(key) = &self.api_key {
            req_builder = req_builder.header("Authorization", format!("Bearer {key}"));
        }

        debug!("POST {url} (streaming) model={}", request.model);

        let (tx, rx) = mpsc::channel(256);

        let mut es = EventSource::new(req_builder)
            .map_err(|e| NekoError::Llm(format!("Failed to create event source: {e}")))?;

        tokio::spawn(async move {
            while let Some(event) = es.next().await {
                match event {
                    Ok(Event::Open) => {
                        debug!("SSE stream opened");
                    }
                    Ok(Event::Message(msg)) => {
                        if msg.data == "[DONE]" {
                            break;
                        }
                        match serde_json::from_str::<StreamEvent>(&msg.data) {
                            Ok(stream_event) => {
                                if tx.send(stream_event).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                warn!("Failed to parse stream event: {e}, data: {}", msg.data);
                            }
                        }
                    }
                    Err(e) => {
                        error!("SSE error: {e}");
                        break;
                    }
                }
            }
            es.close();
        });

        Ok(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::Input;

    #[test]
    fn test_client_construction() {
        let client = Client::new("https://api.openai.com", Some("sk-test"));
        assert_eq!(client.base_url, "https://api.openai.com");
        assert_eq!(client.api_key.as_deref(), Some("sk-test"));
    }

    #[test]
    fn test_request_serialization() {
        let req = Request {
            model: "gpt-5-mini".to_string(),
            input: Input::Text("Hello".to_string()),
            instructions: None,
            tools: None,
            tool_choice: None,
            stream: false,
            temperature: None,
            max_output_tokens: None,
            previous_response_id: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("gpt-5-mini"));
        assert!(json.contains("Hello"));
    }
}
