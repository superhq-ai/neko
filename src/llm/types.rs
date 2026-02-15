use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// OpenResponses-compatible request
#[derive(Debug, Clone, Serialize)]
pub struct Request {
    pub model: String,
    pub input: Input,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,
    #[serde(default)]
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_response_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Input {
    Text(String),
    Items(Vec<Item>),
}

// ---------------------------------------------------------------------------
// Item — input items sent to the API
// ---------------------------------------------------------------------------

/// An input item for the Responses API.
///
/// `Message`, `FunctionCall`, and `FunctionCallOutput` are the items we
/// construct ourselves. `Reasoning` and `Other` are opaque pass-throughs
/// that preserve output items (like reasoning tokens) when feeding them
/// back as input for the next turn.
#[derive(Debug, Clone)]
pub enum Item {
    Message {
        role: Role,
        content: String,
    },
    FunctionCall {
        id: String,
        call_id: String,
        name: String,
        arguments: String,
    },
    FunctionCallOutput {
        call_id: String,
        output: String,
    },
    /// Reasoning item — pass back as-is to maintain chain-of-thought across turns.
    Reasoning(serde_json::Value),
    /// Any other item type — preserved for round-tripping.
    Other(serde_json::Value),
}

impl Serialize for Item {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            Item::Message { role, content } => {
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("type", "message")?;
                map.serialize_entry("role", role)?;
                map.serialize_entry("content", content)?;
                map.end()
            }
            Item::FunctionCall {
                id,
                call_id,
                name,
                arguments,
            } => {
                let mut map = serializer.serialize_map(Some(5))?;
                map.serialize_entry("type", "function_call")?;
                map.serialize_entry("id", id)?;
                map.serialize_entry("call_id", call_id)?;
                map.serialize_entry("name", name)?;
                map.serialize_entry("arguments", arguments)?;
                map.end()
            }
            Item::FunctionCallOutput { call_id, output } => {
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("type", "function_call_output")?;
                map.serialize_entry("call_id", call_id)?;
                map.serialize_entry("output", output)?;
                map.end()
            }
            Item::Reasoning(value) | Item::Other(value) => value.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Item {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let item_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match item_type {
            "message" => {
                let role: Role = serde_json::from_value(
                    value.get("role").cloned().unwrap_or_default(),
                )
                .map_err(serde::de::Error::custom)?;
                let content = value
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                Ok(Item::Message { role, content })
            }
            "function_call" => {
                let id = str_field(&value, "id");
                let call_id = str_field(&value, "call_id");
                let name = str_field(&value, "name");
                let arguments = str_field(&value, "arguments");
                Ok(Item::FunctionCall {
                    id,
                    call_id,
                    name,
                    arguments,
                })
            }
            "function_call_output" => {
                let call_id = str_field(&value, "call_id");
                let output = str_field(&value, "output");
                Ok(Item::FunctionCallOutput { call_id, output })
            }
            "reasoning" => Ok(Item::Reasoning(value)),
            _ => Ok(Item::Other(value)),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Response — returned by the API
// ---------------------------------------------------------------------------

/// OpenResponses-compatible response
#[derive(Debug, Clone, Deserialize)]
pub struct Response {
    pub id: String,
    #[serde(default)]
    pub status: ResponseStatus,
    #[serde(default)]
    pub output: Vec<OutputItem>,
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(default)]
    pub error: Option<ApiError>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    #[default]
    InProgress,
    Completed,
    Failed,
    Incomplete,
}

// ---------------------------------------------------------------------------
// OutputItem — items in the response output array
// ---------------------------------------------------------------------------

/// An output item from the Responses API.
///
/// Handles `message`, `function_call`, and `reasoning` explicitly.
/// Any other type (e.g. `web_search_call`, `mcp_call`, `file_search_call`)
/// is captured as `Other` so deserialization never fails.
#[derive(Debug, Clone)]
pub enum OutputItem {
    Message {
        id: String,
        role: Role,
        content: Vec<ContentPart>,
    },
    FunctionCall {
        id: String,
        call_id: String,
        name: String,
        arguments: String,
    },
    /// Reasoning tokens — opaque, must be passed back in input for multi-turn.
    Reasoning(serde_json::Value),
    /// Any unrecognized output item type.
    Other(serde_json::Value),
}

impl<'de> Deserialize<'de> for OutputItem {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let item_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match item_type {
            "message" => {
                let id = str_field(&value, "id");
                let role: Role = serde_json::from_value(
                    value.get("role").cloned().unwrap_or_default(),
                )
                .map_err(serde::de::Error::custom)?;
                let content: Vec<ContentPart> = serde_json::from_value(
                    value
                        .get("content")
                        .cloned()
                        .unwrap_or(serde_json::Value::Array(vec![])),
                )
                .map_err(serde::de::Error::custom)?;
                Ok(OutputItem::Message { id, role, content })
            }
            "function_call" => {
                let id = str_field(&value, "id");
                let call_id = str_field(&value, "call_id");
                let name = str_field(&value, "name");
                let arguments = str_field(&value, "arguments");
                Ok(OutputItem::FunctionCall {
                    id,
                    call_id,
                    name,
                    arguments,
                })
            }
            "reasoning" => Ok(OutputItem::Reasoning(value)),
            _ => Ok(OutputItem::Other(value)),
        }
    }
}

// ---------------------------------------------------------------------------
// ContentPart — content within a message output item
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ContentPart {
    OutputText { text: String },
    Refusal { refusal: String },
    Other(serde_json::Value),
}

impl Serialize for ContentPart {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            ContentPart::OutputText { text } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("type", "output_text")?;
                map.serialize_entry("text", text)?;
                map.end()
            }
            ContentPart::Refusal { refusal } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("type", "refusal")?;
                map.serialize_entry("refusal", refusal)?;
                map.end()
            }
            ContentPart::Other(value) => value.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ContentPart {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let part_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match part_type {
            "output_text" => {
                let text = str_field(&value, "text");
                Ok(ContentPart::OutputText { text })
            }
            "refusal" => {
                let refusal = str_field(&value, "refusal");
                Ok(ContentPart::Refusal { refusal })
            }
            _ => Ok(ContentPart::Other(value)),
        }
    }
}

// ---------------------------------------------------------------------------
// Usage / Error
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Streaming events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum StreamEvent {
    #[serde(rename = "response.in_progress")]
    ResponseInProgress { response: Response },

    #[serde(rename = "response.output_item.added")]
    OutputItemAdded {
        output_index: usize,
        item: OutputItem,
    },

    #[serde(rename = "response.content_part.added")]
    ContentPartAdded {
        output_index: usize,
        content_index: usize,
        part: ContentPart,
    },

    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta {
        output_index: usize,
        content_index: usize,
        delta: String,
    },

    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionCallArgumentsDelta {
        output_index: usize,
        delta: String,
    },

    #[serde(rename = "response.function_call_arguments.done")]
    FunctionCallArgumentsDone {
        output_index: usize,
        arguments: String,
    },

    #[serde(rename = "response.content_part.done")]
    ContentPartDone {
        output_index: usize,
        content_index: usize,
        part: ContentPart,
    },

    #[serde(rename = "response.output_item.done")]
    OutputItemDone {
        output_index: usize,
        item: OutputItem,
    },

    #[serde(rename = "response.completed")]
    ResponseCompleted { response: Response },

    #[serde(rename = "response.failed")]
    ResponseFailed { response: Response },

    #[serde(other)]
    Unknown,
}

// ---------------------------------------------------------------------------
// Helpers on Response
// ---------------------------------------------------------------------------

impl Response {
    /// Extract text content from the response output items.
    pub fn text(&self) -> String {
        let mut result = String::new();
        for item in &self.output {
            if let OutputItem::Message { content, .. } = item {
                for part in content {
                    if let ContentPart::OutputText { text } = part {
                        if !result.is_empty() {
                            result.push('\n');
                        }
                        result.push_str(text);
                    }
                }
            }
        }
        result
    }

    /// Extract function calls from the response.
    pub fn function_calls(&self) -> Vec<(&str, &str, &str)> {
        self.output
            .iter()
            .filter_map(|item| {
                if let OutputItem::FunctionCall {
                    call_id,
                    name,
                    arguments,
                    ..
                } = item
                {
                    Some((call_id.as_str(), name.as_str(), arguments.as_str()))
                } else {
                    None
                }
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn str_field(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}
