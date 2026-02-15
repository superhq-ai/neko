pub mod context;
pub mod loop_runner;

use std::path::PathBuf;

use tracing::{debug, info, warn};

use crate::config::AgentConfig;
use crate::error::{NekoError, Result};
use crate::llm;
use crate::tools::{ToolContext, ToolRegistry};
use crate::skills::Skill;

pub struct Agent {
    llm_client: llm::Client,
    tools: ToolRegistry,
    config: AgentConfig,
    history: Vec<llm::Item>,
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
            history: Vec::new(),
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

    /// Run a single turn: user message in, assistant response out.
    /// Handles the tool-use loop internally.
    pub async fn run_turn(&mut self, user_message: &str) -> Result<String> {
        self.history.push(llm::Item::Message {
            role: llm::Role::User,
            content: user_message.to_string(),
        });


        let instructions = context::build_instructions(&self.config, &self.workspace, &self.skills);
        let tool_defs = self.tools.tool_definitions();

        let max_iterations = 10;
        for iteration in 0..max_iterations {
            debug!("Agent loop iteration {iteration}");

            let request = llm::Request {
                model: self.config.model.clone(),
                input: llm::Input::Items(self.history.clone()),
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
                previous_response_id: None,
            };

            let response = self.llm_client.create_response(&request).await?;

            if response.status == llm::ResponseStatus::Failed {
                let err_msg = response
                    .error
                    .map(|e| e.message)
                    .unwrap_or_else(|| "Unknown LLM error".to_string());
                return Err(NekoError::Llm(err_msg));
            }

            let function_calls = response.function_calls();

            if function_calls.is_empty() {
                let text = response.text();
                self.append_output_to_history(&response.output);
                self.trim_history();
                self.log_to_recall(user_message, &text);
                return Ok(text);
            }

            info!("Executing {} tool call(s)", function_calls.len());
            self.append_output_to_history(&response.output);

            let tool_ctx = ToolContext {
                workspace: self.workspace.clone(),
            };

            // Re-extract function calls from history since we moved them
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

                self.history.push(llm::Item::FunctionCallOutput {
                    call_id,
                    output,
                });
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

    fn append_output_to_history(&mut self, output: &[llm::OutputItem]) {
        for item in output {
            match item {
                llm::OutputItem::FunctionCall {
                    id,
                    call_id,
                    name,
                    arguments,
                } => {
                    self.history.push(llm::Item::FunctionCall {
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
                        self.history.push(llm::Item::Message {
                            role: *role,
                            content: text,
                        });
                    }
                }
                llm::OutputItem::Reasoning(value) => {
                    self.history.push(llm::Item::Reasoning(value.clone()));
                }
                llm::OutputItem::Other(value) => {
                    self.history.push(llm::Item::Other(value.clone()));
                }
            }
        }
    }

    fn trim_history(&mut self) {
        let max = self.config.max_history as usize;
        if self.history.len() > max {
            let excess = self.history.len() - max;
            self.history.drain(0..excess);
        }
    }
}
