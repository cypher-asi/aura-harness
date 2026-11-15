//! `job_output` and `job_kill`: the agent's view of the commands it
//! started with `run_command` and `background: true`.
//!
//! The commands themselves belong to the context's
//! [`BackgroundProcessHook`](crate::BackgroundProcessHook): it tracks them
//! until they exit and tells the agent that started each one. These tools
//! read a command's log and stop it early.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use aura_core_types::{ToolDefinition, ToolResult};

use crate::error::ToolError;
use crate::tool::{BackgroundJob, BackgroundProcessHook, Tool, ToolContext};

/// Name of the tool that reads a job's output.
pub const JOB_OUTPUT_TOOL_NAME: &str = "job_output";
/// Name of the tool that stops a job.
pub const JOB_KILL_TOOL_NAME: &str = "job_kill";

/// Lines `job_output` returns when the model doesn't ask for a number.
const DEFAULT_TAIL_LINES: usize = 100;
/// Most of a log `job_output` reads from its end.
const MAX_TAIL_BYTES: u64 = 256 * 1024;

/// The last `lines` lines of the log at `path`, reading at most the last
/// 256 KiB of it.
///
/// # Errors
/// When the log can't be read.
pub fn log_tail(path: &Path, lines: usize) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(MAX_TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let all: Vec<&str> = text.lines().collect();
    let kept = &all[all.len().saturating_sub(lines)..];
    let mut out = String::new();
    if start > 0 {
        out.push_str(&format!("[{start} earlier bytes not shown]\n"));
    }
    out.push_str(&kept.join("\n"));
    Ok(out)
}

fn hook(ctx: &ToolContext) -> Result<&Arc<dyn BackgroundProcessHook>, ToolError> {
    ctx.background_processes
        .as_ref()
        .ok_or_else(|| ToolError::ToolDisabled("background jobs are not available here".into()))
}

/// The caller's running job `id`, if it has one.
fn running_job(
    ctx: &ToolContext,
    hook: &dyn BackgroundProcessHook,
    id: &str,
) -> Option<BackgroundJob> {
    hook.running(ctx.caller_agent_id)
        .into_iter()
        .find(|job| job.id == id)
}

fn job_id(args: &serde_json::Value) -> Option<&str> {
    args.get("job_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
}

/// `job_output`: status and recent output of a background job, or a list
/// of the caller's running jobs when no id is given.
pub struct JobOutputTool;

#[async_trait]
impl Tool for JobOutputTool {
    fn name(&self) -> &str {
        JOB_OUTPUT_TOOL_NAME
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: JOB_OUTPUT_TOOL_NAME.into(),
            description: "Show the status and latest output of a command started with \
                run_command background=true. Without job_id, lists your running jobs."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "job_id": {"type": "string", "description": "The process_id run_command returned"},
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
        let hook = hook(ctx)?;
        let Some(id) = job_id(&args) else {
            let running = hook.running(ctx.caller_agent_id);
            if running.is_empty() {
                return Ok(ToolResult::success(
                    JOB_OUTPUT_TOOL_NAME,
                    "No background jobs are running.",
                ));
            }
            let lines: Vec<String> = running
                .iter()
                .map(|job| format!("{} running `{}`", job.id, job.command))
                .collect();
            return Ok(ToolResult::success(JOB_OUTPUT_TOOL_NAME, lines.join("\n")));
        };
        let tail_lines = args
            .get("tail_lines")
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(DEFAULT_TAIL_LINES);
        let (status, log_path) = match running_job(ctx, hook.as_ref(), id) {
            Some(job) => (format!("running: `{}`", job.command), job.log_path),
            None => (
                "not running (its exit was reported in a message)".to_string(),
                hook.log_dir().join(format!("{id}.log")),
            ),
        };
        let tail = log_tail(&log_path, tail_lines)
            .map_err(|e| ToolError::InvalidArguments(format!("no output for job {id}: {e}")))?;
        Ok(ToolResult::success(
            JOB_OUTPUT_TOOL_NAME,
            format!("{id} {status}\n{tail}"),
        ))
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
            description: "Stop a command started with run_command background=true, and every \
                process it started."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "job_id": {"type": "string", "description": "The process_id run_command returned"}
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
        let hook = hook(ctx)?;
        let id =
            job_id(&args).ok_or_else(|| ToolError::InvalidArguments("missing 'job_id'".into()))?;
        if running_job(ctx, hook.as_ref(), id).is_none() {
            return Err(ToolError::InvalidArguments(format!(
                "no running job {id} of yours"
            )));
        }
        hook.kill(id).map_err(ToolError::CommandFailed)?;
        Ok(ToolResult::success(
            JOB_KILL_TOOL_NAME,
            format!("Stopped {id}."),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::BackgroundProcess;
    use aura_core_types::AgentId;
    use std::path::PathBuf;
    use std::sync::Mutex;

    struct FakeHook {
        dir: PathBuf,
        jobs: Mutex<Vec<BackgroundJob>>,
    }

    impl BackgroundProcessHook for FakeHook {
        fn log_dir(&self) -> PathBuf {
            self.dir.clone()
        }

        fn adopt(&self, _agent_id: AgentId, _process: BackgroundProcess) -> Result<(), String> {
            Ok(())
        }

        fn running(&self, agent_id: Option<AgentId>) -> Vec<BackgroundJob> {
            self.jobs
                .lock()
                .unwrap()
                .iter()
                .filter(|job| agent_id.is_none_or(|a| a == job.agent_id))
                .cloned()
                .collect()
        }

        fn kill(&self, id: &str) -> Result<(), String> {
            self.jobs.lock().unwrap().retain(|job| job.id != id);
            Ok(())
        }
    }

    fn text(result: &ToolResult) -> String {
        String::from_utf8_lossy(&result.stdout).into_owned()
    }

    fn ctx_with(agent: AgentId, dir: &Path) -> (ToolContext, Arc<FakeHook>) {
        let hook = Arc::new(FakeHook {
            dir: dir.to_path_buf(),
            jobs: Mutex::new(vec![BackgroundJob {
                id: "p1".into(),
                agent_id: agent,
                command: "cargo test".into(),
                pid: Some(1),
                log_path: dir.join("p1.log"),
                started_ms: 0,
            }]),
        });
        let mut ctx = ToolContext::new(
            crate::Sandbox::new(dir).unwrap(),
            crate::ToolConfig::default(),
        );
        ctx.caller_agent_id = Some(agent);
        ctx.background_processes = Some(Arc::clone(&hook) as Arc<dyn BackgroundProcessHook>);
        (ctx, hook)
    }

    #[test]
    fn log_tail_keeps_the_last_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.log");
        std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
        assert_eq!(log_tail(&path, 2).unwrap(), "two\nthree");
    }

    #[tokio::test]
    async fn output_reads_the_log_and_kill_needs_an_owned_running_job() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("p1.log"), "compiling\nok\n").unwrap();
        let agent = AgentId::generate();
        let (ctx, hook) = ctx_with(agent, dir.path());

        let listed = JobOutputTool
            .execute(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert!(text(&listed).contains("p1 running `cargo test`"));
        let out = JobOutputTool
            .execute(&ctx, serde_json::json!({"job_id": "p1"}))
            .await
            .unwrap();
        assert!(text(&out).contains("ok"), "{}", text(&out));

        let mut stranger = ctx_with(AgentId::generate(), dir.path()).0;
        stranger.background_processes = ctx.background_processes.clone();
        assert!(JobKillTool
            .execute(&stranger, serde_json::json!({"job_id": "p1"}))
            .await
            .is_err());
        JobKillTool
            .execute(&ctx, serde_json::json!({"job_id": "p1"}))
            .await
            .unwrap();
        assert!(hook.running(None).is_empty());

        let after = JobOutputTool
            .execute(&ctx, serde_json::json!({"job_id": "p1"}))
            .await
            .unwrap();
        assert!(text(&after).contains("not running"));
    }
}
