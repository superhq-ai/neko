use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::agent::Agent;
use crate::channels::OutboundMessage;
use crate::error::{NekoError, Result};

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub name: Option<String>,
    pub prompt: String,
    pub schedule: Schedule,
    pub announce: Option<AnnounceTarget>,
    pub enabled: bool,
    pub keep_after_run: bool,
    pub created_at: DateTime<Utc>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub retry: RetryState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Schedule {
    Cron { expr: String },
    At { datetime: DateTime<Utc> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnounceTarget {
    pub channel: String,
    pub recipient_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryState {
    pub consecutive_failures: u32,
    pub retry_after: Option<DateTime<Utc>>,
}

impl Default for RetryState {
    fn default() -> Self {
        Self {
            consecutive_failures: 0,
            retry_after: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub job_id: String,
    pub job_name: Option<String>,
    pub prompt: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub success: bool,
    pub response: Option<String>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

fn cron_dir(workspace: &Path) -> PathBuf {
    workspace.join("cron")
}

fn jobs_path(workspace: &Path) -> PathBuf {
    cron_dir(workspace).join("jobs.json")
}

fn history_path(workspace: &Path) -> PathBuf {
    cron_dir(workspace).join("history.jsonl")
}

pub fn load_jobs(workspace: &Path) -> Result<Vec<CronJob>> {
    let path = jobs_path(workspace);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data = std::fs::read_to_string(&path)?;
    let jobs: Vec<CronJob> =
        serde_json::from_str(&data).map_err(|e| NekoError::Cron(format!("parse jobs.json: {e}")))?;
    Ok(jobs)
}

pub fn save_jobs(workspace: &Path, jobs: &[CronJob]) -> Result<()> {
    let dir = cron_dir(workspace);
    std::fs::create_dir_all(&dir)?;
    let data = serde_json::to_string_pretty(jobs)
        .map_err(|e| NekoError::Cron(format!("serialize jobs: {e}")))?;
    std::fs::write(jobs_path(workspace), data)?;
    Ok(())
}

pub fn append_history(workspace: &Path, entry: &HistoryEntry) -> Result<()> {
    let dir = cron_dir(workspace);
    std::fs::create_dir_all(&dir)?;
    let line = serde_json::to_string(entry)
        .map_err(|e| NekoError::Cron(format!("serialize history: {e}")))?;
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(history_path(workspace))?;
    writeln!(f, "{line}")?;
    Ok(())
}

pub fn read_history(workspace: &Path, lines: usize) -> Result<Vec<HistoryEntry>> {
    let path = history_path(workspace);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data = std::fs::read_to_string(&path)?;
    let all: Vec<&str> = data.lines().collect();
    let start = all.len().saturating_sub(lines);
    let mut entries = Vec::new();
    for line in &all[start..] {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<HistoryEntry>(line) {
            Ok(e) => entries.push(e),
            Err(e) => warn!("Skipping malformed history line: {e}"),
        }
    }
    Ok(entries)
}

// ---------------------------------------------------------------------------
// Scheduling logic
// ---------------------------------------------------------------------------

fn should_fire(job: &CronJob, now: DateTime<Utc>) -> bool {
    if !job.enabled {
        return false;
    }

    // Respect backoff
    if let Some(retry_after) = job.retry.retry_after {
        if now < retry_after {
            return false;
        }
    }

    match &job.schedule {
        Schedule::Cron { expr } => {
            let Ok(schedule) = cron::Schedule::from_str(expr) else {
                warn!("Invalid cron expression for job {}: {}", job.id, expr);
                return false;
            };

            // Find the most recent scheduled time before `now`
            let Some(prev) = schedule.after(&(now - chrono::Duration::seconds(16))).next() else {
                return false;
            };

            // Fire if the scheduled time is within our tick window (15s) and
            // we haven't already run it.
            if prev > now {
                return false;
            }

            match job.last_run_at {
                Some(last) => prev > last,
                None => true,
            }
        }
        Schedule::At { datetime } => {
            if now < *datetime {
                return false;
            }
            // Fire if we haven't run yet
            job.last_run_at.is_none()
        }
    }
}

fn backoff_duration(consecutive_failures: u32) -> chrono::Duration {
    let secs = match consecutive_failures {
        0 => 0,
        1 => 30,
        2 => 60,
        3 => 300,
        4 => 900,
        _ => 3600,
    };
    chrono::Duration::seconds(secs)
}

// ---------------------------------------------------------------------------
// Scheduler
// ---------------------------------------------------------------------------

pub fn spawn_scheduler(
    agent: Arc<Agent>,
    workspace: PathBuf,
    outbound_tx: Option<mpsc::Sender<OutboundMessage>>,
) {
    tokio::spawn(async move {
        info!("Cron scheduler started");
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));

        loop {
            interval.tick().await;

            let jobs = match load_jobs(&workspace) {
                Ok(j) => j,
                Err(e) => {
                    error!("Failed to load cron jobs: {e}");
                    continue;
                }
            };

            if jobs.is_empty() {
                continue;
            }

            let now = Utc::now();
            let mut jobs_modified = false;
            let mut updated_jobs = jobs;

            for i in 0..updated_jobs.len() {
                if !should_fire(&updated_jobs[i], now) {
                    continue;
                }

                // Capture fields before borrowing mutably
                let job = &updated_jobs[i];
                let job_id = job.id.clone();
                let job_name = job.name.clone();
                let job_prompt = job.prompt.clone();
                let job_announce = job.announce.clone();
                let is_one_shot = matches!(job.schedule, Schedule::At { .. });
                let keep = job.keep_after_run;
                let label = job_name.clone().unwrap_or_else(|| job_id.clone());
                let _ = job;

                info!("Firing cron job: {label}");

                let started_at = Utc::now();
                let result = agent.run_turn(&job_prompt).await;
                let finished_at = Utc::now();

                match &result {
                    Ok(response) => {
                        info!(
                            "Cron job {label} completed ({:.1}s)",
                            (finished_at - started_at).num_milliseconds() as f64 / 1000.0
                        );

                        // Send announcement if configured
                        if let (Some(ref announce), Some(ref tx)) =
                            (&job_announce, &outbound_tx)
                        {
                            let msg = OutboundMessage {
                                channel: announce.channel.clone(),
                                recipient_id: announce.recipient_id.clone(),
                                text: response.clone(),
                                attachments: Vec::new(),
                            };
                            if let Err(e) = tx.send(msg).await {
                                error!("Failed to send cron announcement: {e}");
                            }
                        }

                        let entry = HistoryEntry {
                            job_id,
                            job_name,
                            prompt: job_prompt,
                            started_at,
                            finished_at,
                            success: true,
                            response: Some(truncate(response, 1000)),
                            error: None,
                        };
                        if let Err(e) = append_history(&workspace, &entry) {
                            error!("Failed to write cron history: {e}");
                        }

                        // Reset retry state on success
                        updated_jobs[i].last_run_at = Some(finished_at);
                        updated_jobs[i].retry = RetryState::default();
                        jobs_modified = true;

                        // Auto-delete one-shot jobs on success
                        if is_one_shot && !keep {
                            info!("Removing completed one-shot job: {label}");
                            updated_jobs[i].enabled = false;
                        }
                    }
                    Err(e) => {
                        error!("Cron job {label} failed: {e}");

                        let entry = HistoryEntry {
                            job_id,
                            job_name,
                            prompt: job_prompt,
                            started_at,
                            finished_at,
                            success: false,
                            response: None,
                            error: Some(e.to_string()),
                        };
                        if let Err(e) = append_history(&workspace, &entry) {
                            error!("Failed to write cron history: {e}");
                        }

                        let failures = updated_jobs[i].retry.consecutive_failures + 1;
                        let wait = backoff_duration(failures);
                        updated_jobs[i].retry = RetryState {
                            consecutive_failures: failures,
                            retry_after: Some(Utc::now() + wait),
                        };
                        updated_jobs[i].last_run_at = Some(finished_at);
                        jobs_modified = true;
                    }
                }
            }

            // Remove completed one-shot jobs that were disabled above
            let before_len = updated_jobs.len();
            updated_jobs.retain(|j| {
                !(matches!(j.schedule, Schedule::At { .. }) && !j.keep_after_run && !j.enabled)
            });
            if updated_jobs.len() != before_len {
                jobs_modified = true;
            }

            if jobs_modified {
                if let Err(e) = save_jobs(&workspace, &updated_jobs) {
                    error!("Failed to save cron jobs: {e}");
                }
            }
        }
    });
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

// ---------------------------------------------------------------------------
// CLI helpers
// ---------------------------------------------------------------------------

pub fn find_job<'a>(jobs: &'a [CronJob], id_or_name: &str) -> Option<usize> {
    jobs.iter().position(|j| {
        j.id == id_or_name || j.name.as_deref() == Some(id_or_name)
    })
}

pub fn parse_announce(s: &str) -> Result<AnnounceTarget> {
    let parts: Vec<&str> = s.splitn(2, ':').collect();
    if parts.len() != 2 {
        return Err(NekoError::Cron(
            "announce format: channel:recipient_id (e.g. telegram:123456)".into(),
        ));
    }
    Ok(AnnounceTarget {
        channel: parts[0].to_string(),
        recipient_id: parts[1].to_string(),
    })
}

pub fn new_job_id() -> String {
    uuid::Uuid::new_v4().to_string()[..8].to_string()
}

pub fn validate_cron_expr(expr: &str) -> Result<()> {
    cron::Schedule::from_str(expr)
        .map_err(|e| NekoError::Cron(format!("invalid cron expression '{expr}': {e}")))?;
    Ok(())
}
