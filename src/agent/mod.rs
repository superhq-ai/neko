pub mod context;
pub mod loop_runner;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tracing::{debug, info, warn};

use crate::config::AgentConfig;
use crate::error::{NekoError, Result};
use crate::llm;
use crate::tools::{ToolContext, ToolRegistry};
use crate::skills::Skill;

/// Return value from a completed agent turn.
pub struct TurnResult {
    pub text: String,
    pub history: Vec<llm::Item>,
    pub usage: Option<llm::Usage>,
    /// The last response ID — pass back on the next turn for seamless
    /// reasoning-item chaining via `previous_response_id`.
    pub last_response_id: Option<String>,
}

pub struct Agent {
    llm_client: llm::Client,
    tools: ToolRegistry,
    config: AgentConfig,
    workspace: PathBuf,
    skills: Vec<Skill>,
}

impl Agent {
    pub fn new(
        llm_client: llm::Client,
        tools: ToolRegistry,
        config: AgentConfig,
    ) -> Self {
        Self {
            llm_client,
            tools,
            config,
            workspace: PathBuf::new(),
            skills: Vec::new(),
        }
    }

    pub fn with_workspace(mut self, workspace: PathBuf) -> Self {
        self.workspace = workspace;
        self
    }

    pub fn with_skills(mut self, skills: Vec<Skill>) -> Self {
        self.skills = skills;
        self
    }

    /// Backward-compatible single-shot turn (no session, ephemeral history).
    /// Used by `neko message`.
    pub async fn run_turn(&self, user_message: &str) -> Result<String> {
        let result = self
            .run_turn_with_history(Vec::new(), user_message, None)
            .await?;
        Ok(result.text)
    }

    /// Run a single turn with externally-managed history.
    ///
    /// `previous_response_id` enables the API to automatically chain reasoning
    /// items from the prior response — no manual pass-through needed. When
    /// present, only the new user message is sent as input (iteration 0);
    /// follow-up tool-call iterations send only their function_call_outputs.
    ///
    /// When `previous_response_id` is `None` (first message or after restart),
    /// the full history is sent as input and the model re-reasons from scratch.
    pub async fn run_turn_with_history(
        &self,
        mut history: Vec<llm::Item>,
        user_message: &str,
        previous_response_id: Option<String>,
    ) -> Result<TurnResult> {
        let user_item = llm::Item::Message {
            role: llm::Role::User,
            content: user_message.to_string(),
        };
        history.push(user_item.clone());

        let instructions =
            context::build_instructions(&self.config, &self.workspace, &self.skills);
        let tool_defs = self.tools.tool_definitions();

        let max_iterations = self.config.max_iterations as usize;
        let mut last_usage: Option<llm::Usage>;
        let mut current_prev_id = previous_response_id;
        // Function-call outputs produced by the previous iteration,
        // sent as the sole input when chaining via previous_response_id.
        let mut pending_fc_outputs: Vec<llm::Item> = Vec::new();

        // Shared cwd — persists across iterations within a turn.
        let cwd = Arc::new(Mutex::new(self.workspace.clone()));

        for iteration in 0..max_iterations {
            debug!("Agent loop iteration {iteration}");

            // Build input:
            //   iteration 0 + has prev_id  → just the new user message
            //   iteration 0 + no prev_id   → full history (fallback)
            //   iteration N (tool follow-up)→ only the new function_call_outputs
            let input = if iteration == 0 {
                if current_prev_id.is_some() {
                    llm::Input::Items(vec![user_item.clone()])
                } else {
                    llm::Input::Items(history.clone())
                }
            } else {
                llm::Input::Items(std::mem::take(&mut pending_fc_outputs))
            };

            let request = llm::Request {
                model: self.config.model.clone(),
                input,
                instructions: Some(instructions.clone()),
                tools: if tool_defs.is_empty() {
                    None
                } else {
                    Some(tool_defs.clone())
                },
                tool_choice: None,
                stream: false,
                temperature: None,
                max_output_tokens: Some(self.config.max_tokens),
                previous_response_id: current_prev_id.clone(),
            };

            let response = self.llm_client.create_response(&request).await?;

            if response.status == llm::ResponseStatus::Failed {
                let err_msg = response
                    .error
                    .map(|e| e.message)
                    .unwrap_or_else(|| "Unknown LLM error".to_string());
                return Err(NekoError::Llm(err_msg));
            }

            // Chain subsequent requests through this response.
            current_prev_id = Some(response.id.clone());
            last_usage = response.usage.clone();

            let function_calls = response.function_calls();

            if function_calls.is_empty() {
                let text = response.text();
                // Append simplified output for the persistent transcript —
                // reasoning items are NOT included; the API handles them via
                // previous_response_id on the next turn.
                append_output_to_history(&mut history, &response.output);
                strip_reasoning(&mut history);
                trim_history(&mut history, self.config.max_history as usize);
                self.log_to_recall(user_message, &text);
                return Ok(TurnResult {
                    text,
                    history,
                    usage: last_usage,
                    last_response_id: current_prev_id,
                });
            }

            info!("Executing {} tool call(s)", function_calls.len());
            // Record function calls in persistent history (no reasoning).
            append_output_to_history(&mut history, &response.output);

            let tool_ctx = ToolContext {
                workspace: self.workspace.clone(),
                cwd: Arc::clone(&cwd),
            };

            let calls: Vec<(String, String, String)> = function_calls
                .into_iter()
                .map(|(id, name, args)| (id.to_string(), name.to_string(), args.to_string()))
                .collect();

            for (call_id, name, arguments) in calls {
                let result = loop_runner::execute_tool(
                    &self.tools,
                    &name,
                    &arguments,
                    &tool_ctx,
                )
                .await;

                let output = match result {
                    Ok(r) => {
                        if r.is_error {
                            format!("[ERROR] {}", r.output)
                        } else {
                            r.output
                        }
                    }
                    Err(e) => format!("[ERROR] {e}"),
                };

                debug!("Tool {name} returned {} bytes", output.len());

                let fc_output = llm::Item::FunctionCallOutput {
                    call_id,
                    output,
                };
                history.push(fc_output.clone());
                pending_fc_outputs.push(fc_output);
            }
        }

        Err(NekoError::Agent(format!(
            "Agent loop exceeded {max_iterations} iterations"
        )))
    }

    /// Log conversation turn to recall file for future search.
    fn log_to_recall(&self, user_message: &str, assistant_response: &str) {
        if self.workspace == PathBuf::new() {
            return;
        }

        let recall_dir = self.workspace.join("memory").join("recall");
        if let Err(e) = std::fs::create_dir_all(&recall_dir) {
            warn!("Failed to create recall dir: {e}");
            return;
        }

        let now = chrono::Local::now();
        let filename = now.format("%Y-%m-%d").to_string();
        let time = now.format("%H:%M:%S").to_string();

        // Truncate long responses
        let truncated = if assistant_response.len() > 500 {
            format!("{}...", &assistant_response[..500])
        } else {
            assistant_response.to_string()
        };

        let entry = format!(
            "### {time}\n**User:** {user_message}\n**Assistant:** {truncated}\n\n"
        );

        let recall_path = recall_dir.join(format!("{filename}.md"));

        use std::io::Write;
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&recall_path)
        {
            Ok(mut f) => {
                if let Err(e) = f.write_all(entry.as_bytes()) {
                    warn!("Failed to write recall log: {e}");
                }
            }
            Err(e) => {
                warn!("Failed to open recall log: {e}");
            }
        }
    }
}

/// Convert OutputItems to simplified history Items for the persistent transcript.
/// Reasoning and Other items are skipped — the API handles them via
/// `previous_response_id`.
pub fn append_output_to_history(history: &mut Vec<llm::Item>, output: &[llm::OutputItem]) {
    for item in output {
        match item {
            llm::OutputItem::FunctionCall {
                id,
                call_id,
                name,
                arguments,
            } => {
                history.push(llm::Item::FunctionCall {
                    id: id.clone(),
                    call_id: call_id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                });
            }
            llm::OutputItem::Message { role, content, .. } => {
                let text: String = content
                    .iter()
                    .filter_map(|p| match p {
                        llm::ContentPart::OutputText { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                if !text.is_empty() {
                    history.push(llm::Item::Message {
                        role: *role,
                        content: text,
                    });
                }
            }
            // Reasoning and Other are handled by previous_response_id;
            // skip them in the persistent transcript.
            llm::OutputItem::Reasoning(_) | llm::OutputItem::Other(_) => {}
        }
    }
}

/// Trim history to at most `max` items, dropping oldest first.
pub fn trim_history(history: &mut Vec<llm::Item>, max: usize) {
    if history.len() > max {
        let excess = history.len() - max;
        history.drain(0..excess);
    }
}

/// Remove any stray Reasoning/Other items from history.
/// Defensive — `append_output_to_history` already skips them, but this
/// catches items loaded from older transcripts.
pub fn strip_reasoning(history: &mut Vec<llm::Item>) {
    history.retain(|item| !matches!(item, llm::Item::Reasoning(_) | llm::Item::Other(_)));
}
