use async_trait::async_trait;
use serde_json::json;

use super::{schema_object, Tool, ToolContext, ToolResult};
use crate::error::Result;

pub struct HttpRequestTool {
    allowed_domains: Vec<String>,
}

impl HttpRequestTool {
    pub fn new(allowed_domains: Vec<String>) -> Self {
        Self { allowed_domains }
    }
}

#[async_trait]
impl Tool for HttpRequestTool {
    fn name(&self) -> &str {
        "http_request"
    }

    fn description(&self) -> &str {
        "Make an HTTP request. Supports GET and POST methods."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        schema_object(
            json!({
                "url": {
                    "type": "string",
                    "description": "The URL to request"
                },
                "method": {
                    "type": "string",
                    "enum": ["GET", "POST", "PUT", "DELETE"],
                    "description": "HTTP method (default: GET)"
                },
                "body": {
                    "type": "string",
                    "description": "Request body (for POST/PUT)"
                },
                "headers": {
                    "type": "object",
                    "description": "Additional headers as key-value pairs"
                }
            }),
            &["url"],
        )
    }

    async fn execute(&self, params: serde_json::Value, _ctx: &ToolContext) -> Result<ToolResult> {
        let url = params["url"].as_str().unwrap_or_default();
        let method = params["method"].as_str().unwrap_or("GET");

        // Check domain allowlist
        if !self.allowed_domains.is_empty() {
            let domain = url::Url::parse(url)
                .ok()
                .and_then(|u| u.host_str().map(|s| s.to_string()));

            if let Some(domain) = domain {
                if !self.allowed_domains.iter().any(|d| domain.ends_with(d)) {
                    return Ok(ToolResult::error(format!(
                        "Domain '{domain}' is not in the allowed domains list"
                    )));
                }
            }
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap();

        let mut req = match method.to_uppercase().as_str() {
            "POST" => client.post(url),
            "PUT" => client.put(url),
            "DELETE" => client.delete(url),
            _ => client.get(url),
        };

        if let Some(body) = params["body"].as_str() {
            req = req.body(body.to_string());
        }

        if let Some(headers) = params["headers"].as_object() {
            for (key, value) in headers {
                if let Some(v) = value.as_str() {
                    req = req.header(key, v);
                }
            }
        }

        match req.send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                // Truncate very long responses
                let body = if body.len() > 10_000 {
                    format!("{}... [truncated, {} total bytes]", &body[..10_000], body.len())
                } else {
                    body
                };
                Ok(ToolResult::success(format!("HTTP {status}\n{body}")))
            }
            Err(e) => Ok(ToolResult::error(format!("HTTP request failed: {e}"))),
        }
    }
}
