use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::AsyncBufReadExt;
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex as TokioMutex, RwLock};

/// Maximum output buffer size per session (1 MB).
const MAX_OUTPUT_BYTES: usize = 1_048_576;

/// Exited sessions older than this are removed during lazy cleanup.
const CLEANUP_AGE: Duration = Duration::from_secs(300);

pub struct ProcessManager {
    sessions: RwLock<HashMap<String, Arc<BackgroundSession>>>,
    next_id: AtomicU64,
    yield_ms: u64,
}

pub struct BackgroundSession {
    pub id: String,
    pub command: String,
    pub started_at: Instant,
    pub timeout: Duration,
    output_buf: Arc<TokioMutex<String>>,
    cursor: TokioMutex<usize>,
    exit_status: Arc<TokioMutex<Option<i32>>>,
    child: TokioMutex<Option<Child>>,
    stdin: TokioMutex<Option<ChildStdin>>,
}

pub enum SpawnResult {
    Completed { output: String, success: bool },
    Backgrounded { session_id: String, output_so_far: String },
}

impl ProcessManager {
    pub fn new(yield_ms: u64) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            yield_ms,
        }
    }

    pub async fn spawn_or_yield(
        &self,
        command: &str,
        cwd: &Path,
        timeout_secs: u64,
    ) -> Result<SpawnResult, String> {
        self.cleanup_stale().await;

        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn: {e}"))?;

        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        // Shared buffer — reader tasks and eventual session share this Arc.
        let output_buf: Arc<TokioMutex<String>> = Arc::new(TokioMutex::new(String::new()));
        let exit_status: Arc<TokioMutex<Option<i32>>> = Arc::new(TokioMutex::new(None));

        // Spawn reader tasks
        if let Some(stdout) = stdout {
            let buf = Arc::clone(&output_buf);
            tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let mut b = buf.lock().await;
                    if b.len() < MAX_OUTPUT_BYTES {
                        b.push_str(&line);
                        b.push('\n');
                    }
                }
            });
        }

        if let Some(stderr) = stderr {
            let buf = Arc::clone(&output_buf);
            tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let mut b = buf.lock().await;
                    if b.len() < MAX_OUTPUT_BYTES {
                        b.push_str("[stderr] ");
                        b.push_str(&line);
                        b.push('\n');
                    }
                }
            });
        }

        // Wait up to yield_ms for completion
        let yield_duration = Duration::from_millis(self.yield_ms);
        let wait_result = tokio::time::timeout(yield_duration, child.wait()).await;

        match wait_result {
            Ok(Ok(status)) => {
                // Completed within yield window — let readers flush
                tokio::time::sleep(Duration::from_millis(50)).await;
                let buf = output_buf.lock().await;
                let output = if buf.is_empty() {
                    format!("Command exited with code {}", status.code().unwrap_or(-1))
                } else {
                    buf.clone()
                };
                Ok(SpawnResult::Completed {
                    output,
                    success: status.success(),
                })
            }
            Ok(Err(e)) => Err(format!("Process error: {e}")),
            Err(_) => {
                // Yield timeout — background it
                let id_num = self.next_id.fetch_add(1, Ordering::Relaxed);
                let session_id = format!("bg_{id_num}");
                let timeout = Duration::from_secs(timeout_secs);

                let output_so_far = {
                    let buf = output_buf.lock().await;
                    buf.clone()
                };

                let session = Arc::new(BackgroundSession {
                    id: session_id.clone(),
                    command: command.to_string(),
                    started_at: Instant::now(),
                    timeout,
                    output_buf: Arc::clone(&output_buf),
                    cursor: TokioMutex::new(0),
                    exit_status: Arc::clone(&exit_status),
                    child: TokioMutex::new(Some(child)),
                    stdin: TokioMutex::new(stdin),
                });

                // Spawn exit-status watcher
                let session_ref = Arc::clone(&session);
                tokio::spawn(async move {
                    let mut child_guard = session_ref.child.lock().await;
                    if let Some(ref mut c) = *child_guard {
                        let code = match c.wait().await {
                            Ok(s) => s.code().unwrap_or(-1),
                            Err(_) => -1,
                        };
                        *session_ref.exit_status.lock().await = Some(code);
                    }
                });

                // Spawn timeout watchdog
                let session_ref = Arc::clone(&session);
                tokio::spawn(async move {
                    tokio::time::sleep(timeout).await;
                    let status = session_ref.exit_status.lock().await;
                    if status.is_none() {
                        drop(status);
                        let mut child_guard = session_ref.child.lock().await;
                        if let Some(ref mut c) = *child_guard {
                            let _ = c.kill().await;
                        }
                    }
                });

                self.sessions.write().await.insert(session_id.clone(), session);

                Ok(SpawnResult::Backgrounded {
                    session_id,
                    output_so_far,
                })
            }
        }
    }

    pub async fn get_session(&self, id: &str) -> Option<Arc<BackgroundSession>> {
        self.sessions.read().await.get(id).cloned()
    }

    pub async fn remove_session(&self, id: &str) -> Option<Arc<BackgroundSession>> {
        self.sessions.write().await.remove(id)
    }

    pub async fn list_sessions(&self) -> Vec<SessionInfo> {
        let sessions = self.sessions.read().await;
        let mut infos = Vec::with_capacity(sessions.len());
        for session in sessions.values() {
            let status = session.exit_status.lock().await;
            infos.push(SessionInfo {
                id: session.id.clone(),
                command: session.command.clone(),
                elapsed_secs: session.started_at.elapsed().as_secs(),
                exit_status: *status,
            });
        }
        infos
    }

    async fn cleanup_stale(&self) {
        let mut sessions = self.sessions.write().await;
        sessions.retain(|_, session| {
            match session.exit_status.try_lock() {
                Ok(status) => {
                    if status.is_some() {
                        session.started_at.elapsed() < CLEANUP_AGE
                    } else {
                        true // still running
                    }
                }
                Err(_) => true, // locked = active, keep
            }
        });
    }
}

impl BackgroundSession {
    /// Return output accumulated since the last poll.
    pub async fn poll_output(&self) -> (String, Option<i32>) {
        let buf = self.output_buf.lock().await;
        let mut cursor = self.cursor.lock().await;
        let new_output = if *cursor < buf.len() {
            buf[*cursor..].to_string()
        } else {
            String::new()
        };
        *cursor = buf.len();
        let status = *self.exit_status.lock().await;
        (new_output, status)
    }

    /// Write data to the process's stdin. If `eof` is true, drop the stdin
    /// handle after writing (signals end-of-input).
    pub async fn write_stdin(&self, data: &str, eof: bool) -> Result<(), String> {
        use tokio::io::AsyncWriteExt;

        let mut stdin_guard = self.stdin.lock().await;
        let stdin = stdin_guard
            .as_mut()
            .ok_or_else(|| "stdin is closed".to_string())?;

        let mut to_write = data.to_string();
        if !to_write.ends_with('\n') {
            to_write.push('\n');
        }
        stdin
            .write_all(to_write.as_bytes())
            .await
            .map_err(|e| format!("stdin write failed: {e}"))?;
        stdin
            .flush()
            .await
            .map_err(|e| format!("stdin flush failed: {e}"))?;

        if eof {
            *stdin_guard = None;
        }

        Ok(())
    }

    /// Kill the process.
    pub async fn kill(&self) -> Result<(), String> {
        let mut child_guard = self.child.lock().await;
        if let Some(ref mut c) = *child_guard {
            c.kill().await.map_err(|e| format!("kill failed: {e}"))?;
        }
        Ok(())
    }

    /// Drain all remaining output.
    pub async fn drain_output(&self) -> String {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let buf = self.output_buf.lock().await;
        buf.clone()
    }
}

pub struct SessionInfo {
    pub id: String,
    pub command: String,
    pub elapsed_secs: u64,
    pub exit_status: Option<i32>,
}
