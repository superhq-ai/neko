use async_trait::async_trait;
use serde_json::json;

use super::{schema_object, Tool, ToolContext, ToolResult};
use crate::cron;
use crate::error::Result;

pub struct CronManageTool;

#[async_trait]
impl Tool for CronManageTool {
    fn name(&self) -> &str {
        "cron_manage"
    }

    fn description(&self) -> &str {
        "Manage scheduled cron jobs. Actions: add (create a recurring or one-shot job), list (show all jobs), edit (modify a job), remove (delete a job). Jobs run their prompt through the agent on schedule. Results are automatically delivered back to the current channel unless 'announce' overrides it."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        schema_object(
            json!({
                "action": {
                    "type": "string",
                    "enum": ["add", "list", "edit", "remove"],
                    "description": "The action to perform"
                },
                "prompt": {
                    "type": "string",
                    "description": "(add) The prompt the agent will execute on each run"
                },
                "schedule": {
                    "type": "string",
                    "description": "(add/edit) Cron expression with 6 fields: 'sec min hour day month weekday' (e.g. '0 0 9 * * *' for daily at 9am)"
                },
                "at": {
                    "type": "string",
                    "description": "(add) One-shot datetime in 'YYYY-MM-DD HH:MM' format (local time). Mutually exclusive with schedule."
                },
                "name": {
                    "type": "string",
                    "description": "(add/edit) Human-readable label for the job"
                },
                "announce": {
                    "type": "string",
                    "description": "(add/edit) Deliver results to channel:recipient_id (e.g. 'telegram:123456'). Use 'none' to clear."
                },
                "id": {
                    "type": "string",
                    "description": "(edit/remove) Job ID or name to target"
                },
                "enabled": {
                    "type": "boolean",
                    "description": "(edit) Enable or disable the job"
                }
            }),
            &["action"],
        )
    }

    async fn execute(&self, params: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult> {
        let action = params["action"].as_str().unwrap_or_default();

        match action {
            "add" => self.action_add(&params, ctx),
            "list" => self.action_list(ctx),
            "edit" => self.action_edit(&params, ctx),
            "remove" => self.action_remove(&params, ctx),
            _ => Ok(ToolResult::error(format!(
                "Unknown action '{action}'. Use: add, list, edit, remove"
            ))),
        }
    }
}

impl CronManageTool {
    fn action_add(&self, params: &serde_json::Value, ctx: &ToolContext) -> Result<ToolResult> {
        let prompt = match params["prompt"].as_str() {
            Some(p) if !p.is_empty() => p,
            _ => return Ok(ToolResult::error("'prompt' is required for add")),
        };

        let schedule_str = params["schedule"].as_str().filter(|s| !s.is_empty());
        let at_str = params["at"].as_str().filter(|s| !s.is_empty());

        let schedule = match (schedule_str, at_str) {
            (Some(expr), None) => {
                if let Err(e) = cron::validate_cron_expr(expr) {
                    return Ok(ToolResult::error(format!("{e}")));
                }
                cron::Schedule::Cron {
                    expr: expr.to_string(),
                }
            }
            (None, Some(dt_str)) => {
                let datetime = match parse_datetime_tool(dt_str) {
                    Ok(dt) => dt,
                    Err(msg) => return Ok(ToolResult::error(msg)),
                };
                cron::Schedule::At { datetime }
            }
            (Some(_), Some(_)) => {
                return Ok(ToolResult::error(
                    "Specify either 'schedule' or 'at', not both",
                ));
            }
            (None, None) => {
                return Ok(ToolResult::error(
                    "Must specify 'schedule' (cron expr) or 'at' (datetime)",
                ));
            }
        };

        let name = params["name"].as_str().filter(|s| !s.is_empty()).map(String::from);
        let announce = match params["announce"].as_str().filter(|s| !s.is_empty()) {
            Some(s) if s == "none" => None,
            Some(s) => match cron::parse_announce(s) {
                Ok(a) => Some(a),
                Err(e) => return Ok(ToolResult::error(format!("{e}"))),
            },
            // Default to the current channel so results go back to the user
            None => ctx.channel.as_ref().map(|ch| cron::AnnounceTarget {
                channel: ch.channel.clone(),
                recipient_id: ch.recipient_id.clone(),
            }),
        };

        let job = cron::CronJob {
            id: cron::new_job_id(),
            name: name.clone(),
            prompt: prompt.to_string(),
            schedule,
            announce,
            enabled: true,
            keep_after_run: false,
            created_at: chrono::Utc::now(),
            last_run_at: None,
            retry: cron::RetryState::default(),
        };

        let mut jobs = match cron::load_jobs(&ctx.workspace) {
            Ok(j) => j,
            Err(e) => return Ok(ToolResult::error(format!("Failed to load jobs: {e}"))),
        };

        let label = name.unwrap_or_else(|| job.id.clone());
        let id = job.id.clone();
        jobs.push(job);

        if let Err(e) = cron::save_jobs(&ctx.workspace, &jobs) {
            return Ok(ToolResult::error(format!("Failed to save jobs: {e}")));
        }

        Ok(ToolResult::success(format!(
            "Created cron job '{label}' (id: {id}). It will be picked up by the scheduler within 15 seconds."
        )))
    }

    fn action_list(&self, ctx: &ToolContext) -> Result<ToolResult> {
        let jobs = match cron::load_jobs(&ctx.workspace) {
            Ok(j) => j,
            Err(e) => return Ok(ToolResult::error(format!("Failed to load jobs: {e}"))),
        };

        if jobs.is_empty() {
            return Ok(ToolResult::success("No cron jobs configured."));
        }

        let mut lines = Vec::new();
        for job in &jobs {
            let name = job.name.as_deref().unwrap_or("-");
            let status = if job.enabled { "enabled" } else { "disabled" };
            let sched = match &job.schedule {
                cron::Schedule::Cron { expr } => format!("cron: {expr}"),
                cron::Schedule::At { datetime } => {
                    format!("at: {}", datetime.format("%Y-%m-%d %H:%M"))
                }
            };
            let announce = job
                .announce
                .as_ref()
                .map(|a| format!("{}:{}", a.channel, a.recipient_id))
                .unwrap_or_else(|| "none".into());
            lines.push(format!(
                "- {id} | {name} | {status} | {sched} | announce: {announce} | prompt: {prompt}",
                id = job.id,
                prompt = truncate(&job.prompt, 60),
            ));
        }

        Ok(ToolResult::success(lines.join("\n")))
    }

    fn action_edit(&self, params: &serde_json::Value, ctx: &ToolContext) -> Result<ToolResult> {
        let id_or_name = match params["id"].as_str().filter(|s| !s.is_empty()) {
            Some(id) => id,
            None => return Ok(ToolResult::error("'id' is required for edit")),
        };

        let mut jobs = match cron::load_jobs(&ctx.workspace) {
            Ok(j) => j,
            Err(e) => return Ok(ToolResult::error(format!("Failed to load jobs: {e}"))),
        };

        let idx = match cron::find_job(&jobs, id_or_name) {
            Some(i) => i,
            None => {
                return Ok(ToolResult::error(format!("Job '{id_or_name}' not found")))
            }
        };

        if let Some(p) = params["prompt"].as_str().filter(|s| !s.is_empty()) {
            jobs[idx].prompt = p.to_string();
        }
        if let Some(expr) = params["schedule"].as_str().filter(|s| !s.is_empty()) {
            if let Err(e) = cron::validate_cron_expr(expr) {
                return Ok(ToolResult::error(format!("{e}")));
            }
            jobs[idx].schedule = cron::Schedule::Cron {
                expr: expr.to_string(),
            };
        }
        if let Some(n) = params["name"].as_str().filter(|s| !s.is_empty()) {
            jobs[idx].name = Some(n.to_string());
        }
        if let Some(e) = params["enabled"].as_bool() {
            jobs[idx].enabled = e;
            if e {
                jobs[idx].retry = cron::RetryState::default();
            }
        }
        if let Some(a) = params["announce"].as_str().filter(|s| !s.is_empty()) {
            if a == "none" {
                jobs[idx].announce = None;
            } else {
                match cron::parse_announce(a) {
                    Ok(target) => jobs[idx].announce = Some(target),
                    Err(e) => return Ok(ToolResult::error(format!("{e}"))),
                }
            }
        }

        let label = jobs[idx]
            .name
            .as_deref()
            .unwrap_or(&jobs[idx].id)
            .to_string();

        if let Err(e) = cron::save_jobs(&ctx.workspace, &jobs) {
            return Ok(ToolResult::error(format!("Failed to save jobs: {e}")));
        }

        Ok(ToolResult::success(format!("Updated job '{label}'.")))
    }

    fn action_remove(&self, params: &serde_json::Value, ctx: &ToolContext) -> Result<ToolResult> {
        let id_or_name = match params["id"].as_str().filter(|s| !s.is_empty()) {
            Some(id) => id,
            None => return Ok(ToolResult::error("'id' is required for remove")),
        };

        let mut jobs = match cron::load_jobs(&ctx.workspace) {
            Ok(j) => j,
            Err(e) => return Ok(ToolResult::error(format!("Failed to load jobs: {e}"))),
        };

        let idx = match cron::find_job(&jobs, id_or_name) {
            Some(i) => i,
            None => {
                return Ok(ToolResult::error(format!("Job '{id_or_name}' not found")))
            }
        };

        let removed = jobs.remove(idx);
        let label = removed
            .name
            .as_deref()
            .unwrap_or(&removed.id)
            .to_string();

        if let Err(e) = cron::save_jobs(&ctx.workspace, &jobs) {
            return Ok(ToolResult::error(format!("Failed to save jobs: {e}")));
        }

        Ok(ToolResult::success(format!("Removed job '{label}'.")))
    }
}

fn parse_datetime_tool(s: &str) -> std::result::Result<chrono::DateTime<chrono::Utc>, String> {
    use chrono::Utc;

    let formats = ["%Y-%m-%d %H:%M", "%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"];
    for fmt in &formats {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            let local = chrono::Local::now().timezone();
            if let Some(local_dt) = naive.and_local_timezone(local).single() {
                return Ok(local_dt.with_timezone(&Utc));
            }
        }
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    Err(format!(
        "Could not parse datetime: '{s}' (expected YYYY-MM-DD HH:MM)"
    ))
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
