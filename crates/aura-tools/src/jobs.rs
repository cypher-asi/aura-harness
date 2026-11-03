//! Background commands: `run_command` with `background: true`, plus the
//! `job_output` and `job_kill` tools.
//!
//! A background job keeps running after its tool call returns, so the
//! model can start a long build, test run or dev server and keep working.
//! When a job exits, a [`JobFinished`] goes out on the channel given to
//! [`BackgroundJobs::new`] so the surface can tell the agent that started
//! it.

use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use aura_core_types::{AgentId, ToolDefinition, ToolResult};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};

/// Name of the tool that reads a job's output.
pub const JOB_OUTPUT_TOOL_NAME: &str = "job_output";
/// Name of the tool that stops a job.
pub const JOB_KILL_TOOL_NAME: &str = "job_kill";

/// Output kept per job; older output is dropped.
const MAX_OUTPUT_BYTES: usize = 256 * 1024;
/// Lines `job_output` returns when the model doesn't ask for a number.
const DEFAULT_TAIL_LINES: usize = 100;
/// Lines of output included in a completion notice.
const NOTICE_TAIL_LINES: usize = 40;

/// A job that exited.
#[derive(Debug, Clone)]
pub struct JobFinished {
    pub job_id: String,
    /// Agent whose tool call started the job.
    pub agent_id: Option<AgentId>,
    pub command: String,
    /// `None` when the process was killed by a signal.
    pub exit_code: Option<i32>,
    pub killed: bool,
    pub elapsed: Duration,
    /// Last lines of combined stdout and stderr.
    pub tail: String,
}

impl JobFinished {
    /// What the agent is told when the job exits.
    #[must_use]
    pub fn notice(&self) -> String {
        let how = if self.killed {
            "was stopped".to_string()
        } else {
            match self.exit_code {
                Some(code) => format!("finished with exit code {code}"),
                None => "was terminated by a signal".to_string(),
            }
        };
        let mut text = format!(
            "Background job {} (`{}`) {how} after {}.",
            self.job_id,
            self.command,
            format_elapsed(self.elapsed)
        );
        if self.tail.trim().is_empty() {
            text.push_str(" It printed nothing.");
        } else {
            text.push_str(" Last output:\n```\n");
            text.push_str(self.tail.trim_end());
            text.push_str("\n```");
        }
        text
    }
}

/// Snapshot of a job for listings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobSummary {
    pub job_id: String,
    pub agent_id: Option<AgentId>,
    pub command: String,
    pub running: bool,
    pub elapsed_secs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobState {
    Running,
    Exited { code: Option<i32> },
}

struct Job {
    id: String,
    agent_id: Option<AgentId>,
    command: String,
    pid: u32,
    started: Instant,
    output: Mutex<OutputBuffer>,
    state: Mutex<JobState>,
    finished_at: Mutex<Option<Instant>>,
    killed: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
struct OutputBuffer {
    bytes: VecDeque<u8>,
    dropped: usize,
}

impl OutputBuffer {
    fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend(chunk);
        let excess = self.bytes.len().saturating_sub(MAX_OUTPUT_BYTES);
        if excess > 0 {
            self.bytes.drain(..excess);
            self.dropped = self.dropped.saturating_add(excess);
        }
    }

    fn tail_lines(&self, lines: usize) -> String {
        let (a, b) = self.bytes.as_slices();
        let mut all = Vec::with_capacity(a.len() + b.len());
        all.extend_from_slice(a);
        all.extend_from_slice(b);
        let text = String::from_utf8_lossy(&all);
        let kept: Vec<&str> = text.lines().collect();
        let start = kept.len().saturating_sub(lines);
        kept[start..].join("\n")
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Every background job of a surface.
pub struct BackgroundJobs {
    jobs: Mutex<HashMap<String, Arc<Job>>>,
    next_id: AtomicU64,
    finished: mpsc::Sender<JobFinished>,
}

impl BackgroundJobs {
    /// A registry that reports exited jobs on `finished`.
    #[must_use]
    pub fn new(finished: mpsc::Sender<JobFinished>) -> Arc<Self> {
        Arc::new(Self {
            jobs: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            finished,
        })
    }

    /// Track `child` as a new job and return its id. `child` must have
    /// piped stdout and stderr.
    pub fn start(
        self: &Arc<Self>,
        mut child: Child,
        command: String,
        agent_id: Option<AgentId>,
    ) -> String {
        let id = format!("job-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let job = Arc::new(Job {
            id: id.clone(),
            agent_id,
            command,
            pid: child.id(),
            started: Instant::now(),
            output: Mutex::new(OutputBuffer::default()),
            state: Mutex::new(JobState::Running),
            finished_at: Mutex::new(None),
            killed: std::sync::atomic::AtomicBool::new(false),
        });
        lock(&self.jobs).insert(id.clone(), Arc::clone(&job));

        let readers: Vec<std::thread::JoinHandle<()>> = [
            child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn Read + Send>),
            child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn Read + Send>),
        ]
        .into_iter()
        .flatten()
        .map(|mut stream| {
            let job = Arc::clone(&job);
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => lock(&job.output).push(&buf[..n]),
                    }
                }
            })
        })
        .collect();

        let finished = self.finished.clone();
        std::thread::spawn(move || {
            let code = match child.wait() {
                Ok(status) => status.code(),
                Err(e) => {
                    warn!(job = %job.id, error = %e, "Waiting for background job failed");
                    None
                }
            };
            for reader in readers {
                let _ = reader.join();
            }
            *lock(&job.state) = JobState::Exited { code };
            *lock(&job.finished_at) = Some(Instant::now());
            let notice = JobFinished {
                job_id: job.id.clone(),
                agent_id: job.agent_id,
                command: job.command.clone(),
                exit_code: code,
                killed: job.killed.load(Ordering::Acquire),
                elapsed: job.started.elapsed(),
                tail: lock(&job.output).tail_lines(NOTICE_TAIL_LINES),
            };
            debug!(job = %notice.job_id, ?code, "Background job exited");
            if finished.try_send(notice).is_err() {
                warn!(job = %job.id, "Dropped background job completion notice");
            }
        });
        id
    }

    fn get(&self, job_id: &str) -> Option<Arc<Job>> {
        lock(&self.jobs).get(job_id).cloned()
    }

    /// Jobs started by `agent_id` (every job when `None`), oldest first.
    #[must_use]
    pub fn list(&self, agent_id: Option<AgentId>) -> Vec<JobSummary> {
        let mut jobs: Vec<JobSummary> = lock(&self.jobs)
            .values()
            .filter(|job| agent_id.is_none() || job.agent_id == agent_id)
            .map(|job| {
                let end = (*lock(&job.finished_at)).unwrap_or_else(Instant::now);
                JobSummary {
                    job_id: job.id.clone(),
                    agent_id: job.agent_id,
                    command: job.command.clone(),
                    running: *lock(&job.state) == JobState::Running,
                    elapsed_secs: end.duration_since(job.started).as_secs(),
                }
            })
            .collect();
        jobs.sort_by_key(|job| {
            job.job_id
                .trim_start_matches("job-")
                .parse::<u64>()
                .unwrap_or(u64::MAX)
        });
        jobs
    }

    /// Stop a running job and everything it started.
    pub fn kill(&self, job_id: &str) -> Result<(), String> {
        let job = self
            .get(job_id)
            .ok_or_else(|| format!("no job named {job_id}"))?;
        if *lock(&job.state) != JobState::Running {
            return Err(format!("{job_id} already exited"));
        }
        job.killed.store(true, Ordering::Release);
        kill_process_tree(job.pid);
        Ok(())
    }

    /// Stop every running job; used when the surface shuts down.
    pub fn kill_all(&self) {
        let running: Vec<Arc<Job>> = lock(&self.jobs)
            .values()
            .filter(|job| *lock(&job.state) == JobState::Running)
            .cloned()
            .collect();
        for job in running {
            job.killed.store(true, Ordering::Release);
            kill_process_tree(job.pid);
        }
    }
}

#[cfg(unix)]
fn kill_process_tree(pid: u32) {
    // Jobs are spawned as process-group leaders, so the negative pid
    // reaches the shell and everything it started.
    let _ = std::process::Command::new("kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .status();
}

#[cfg(not(unix))]
fn kill_process_tree(pid: u32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .status();
}

fn format_elapsed(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs >= 3600 {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

/// Add the `background` option to a `run_command` definition, for
/// surfaces that wire up [`BackgroundJobs`].
pub fn add_background_option(definition: &mut ToolDefinition) {
    if let Some(props) = definition
        .input_schema
        .get_mut("properties")
        .and_then(serde_json::Value::as_object_mut)
    {
        props.insert(
            "background".into(),
            serde_json::json!({
                "type": "boolean",
                "description": "Run without waiting: returns a job id at once and you are told when it exits. Use for long builds, test suites, and servers; check progress with job_output and stop with job_kill. timeout_ms does not apply."
            }),
        );
    }
}

fn caller_jobs(ctx: &ToolContext) -> Result<&Arc<BackgroundJobs>, ToolError> {
    ctx.background_jobs
        .as_ref()
        .ok_or_else(|| ToolError::ToolDisabled("background jobs are not available here".into()))
}

fn owned_job(
    ctx: &ToolContext,
    jobs: &BackgroundJobs,
    job_id: &str,
) -> Result<Arc<Job>, ToolError> {
    jobs.get(job_id)
        .filter(|job| ctx.caller_agent_id.is_none() || job.agent_id == ctx.caller_agent_id)
        .ok_or_else(|| ToolError::InvalidArguments(format!("no job named {job_id}")))
}

/// `job_output`: status and recent output of a background job, or a list
/// of the caller's jobs when no id is given.
pub struct JobOutputTool;

#[async_trait]
impl Tool for JobOutputTool {
    fn name(&self) -> &str {
        JOB_OUTPUT_TOOL_NAME
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: JOB_OUTPUT_TOOL_NAME.into(),
            description: "Show the status and latest output of a background job started with \
                run_command background=true. Without job_id, lists your jobs."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "job_id": {"type": "string", "description": "Job id, e.g. \"job-1\""},
                    "tail_lines": {"type": "integer", "description": "How many of the latest lines to return (default 100)"}
                }
            }),
            cache_control: None,
            eager_input_streaming: None,
        }
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let jobs = caller_jobs(ctx)?;
        let Some(job_id) = args.get("job_id").and_then(serde_json::Value::as_str) else {
            let listed = jobs.list(ctx.caller_agent_id);
            if listed.is_empty() {
                return Ok(ToolResult::success(
                    JOB_OUTPUT_TOOL_NAME,
                    "No background jobs.",
                ));
            }
            let lines: Vec<String> = listed
                .iter()
                .map(|job| {
                    format!(
                        "{} {} {}s `{}`",
                        job.job_id,
                        if job.running { "running" } else { "exited" },
                        job.elapsed_secs,
                        job.command
                    )
                })
                .collect();
            return Ok(ToolResult::success(JOB_OUTPUT_TOOL_NAME, lines.join("\n")));
        };
        let job = owned_job(ctx, jobs, job_id)?;
        let tail_lines = args
            .get("tail_lines")
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(DEFAULT_TAIL_LINES);
        let status = match *lock(&job.state) {
            JobState::Running => format!("running for {}", format_elapsed(job.started.elapsed())),
            JobState::Exited { code: Some(code) } => format!("exited with code {code}"),
            JobState::Exited { code: None } => "terminated by a signal".to_string(),
        };
        let output = lock(&job.output);
        let mut text = format!("{} (`{}`): {status}\n", job.id, job.command);
        if output.dropped > 0 {
            text.push_str(&format!("[{} earlier bytes dropped]\n", output.dropped));
        }
        text.push_str(&output.tail_lines(tail_lines));
        Ok(ToolResult::success(JOB_OUTPUT_TOOL_NAME, text))
    }
}

/// `job_kill`: stop a background job.
pub struct JobKillTool;

#[async_trait]
impl Tool for JobKillTool {
    fn name(&self) -> &str {
        JOB_KILL_TOOL_NAME
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: JOB_KILL_TOOL_NAME.into(),
            description: "Stop a background job and every process it started.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "job_id": {"type": "string", "description": "Job id, e.g. \"job-1\""}
                },
                "required": ["job_id"]
            }),
            cache_control: None,
            eager_input_streaming: None,
        }
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let jobs = caller_jobs(ctx)?;
        let job_id = args
            .get("job_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("missing 'job_id'".into()))?;
        owned_job(ctx, jobs, job_id)?;
        jobs.kill(job_id).map_err(ToolError::CommandFailed)?;
        Ok(ToolResult::success(
            JOB_KILL_TOOL_NAME,
            format!("Stopping {job_id}."),
        ))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn spawn(script: &str) -> Child {
        use std::os::unix::process::CommandExt;
        let mut cmd = Command::new("sh");
        cmd.args(["-c", script])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        cmd.spawn().unwrap()
    }

    #[tokio::test]
    async fn reports_exit_and_output() {
        let (tx, mut rx) = mpsc::channel(4);
        let jobs = BackgroundJobs::new(tx);
        let id = jobs.start(
            spawn("echo hello; echo oops >&2; exit 3"),
            "test".into(),
            None,
        );

        let done = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(done.job_id, id);
        assert_eq!(done.exit_code, Some(3));
        assert!(!done.killed);
        assert!(
            done.tail.contains("hello") && done.tail.contains("oops"),
            "{}",
            done.tail
        );
        assert!(done.notice().contains("exit code 3"));
        assert!(!jobs.list(None)[0].running);
    }

    #[tokio::test]
    async fn kill_stops_a_running_job() {
        let (tx, mut rx) = mpsc::channel(4);
        let jobs = BackgroundJobs::new(tx);
        let id = jobs.start(spawn("sleep 30"), "sleep".into(), None);
        assert!(jobs.list(None)[0].running);

        jobs.kill(&id).unwrap();
        let done = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert!(done.killed);
        assert!(done.notice().contains("was stopped"));
    }

    #[test]
    fn output_buffer_keeps_the_tail() {
        let mut buf = OutputBuffer::default();
        buf.push(&vec![b'x'; MAX_OUTPUT_BYTES]);
        buf.push(b"\nlast line");
        assert!(buf.dropped > 0);
        assert_eq!(buf.tail_lines(1), "last line");
    }
}
