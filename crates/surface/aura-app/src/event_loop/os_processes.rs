//! Live OS processes that agents started.
//!
//! Every command a tool runs carries [`AGENT_ID_VAR`] (and the tool call's
//! [`TOOL_USE_ID_VAR`]) in its environment, and everything it starts
//! inherits them. Scanning `/proc` for them finds a dev server an agent
//! launched even after it detached and was re-parented away from the
//! harness.

use aura_exec_tools::{AGENT_ID_VAR, HARNESS_PID_VAR, TOOL_USE_ID_VAR};
use std::path::PathBuf;

/// One process carrying an agent tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsProcess {
    pub pid: u32,
    pub ppid: u32,
    /// Process group; equal to `pid` when the process leads one.
    pub pgid: u32,
    /// Clock ticks after boot; with `pid`, unique across pid reuse.
    pub start_ticks: u64,
    /// Milliseconds since the epoch.
    pub started_ms: u64,
    pub agent_id: String,
    pub tool_use_id: Option<String>,
    pub harness_pid: Option<u32>,
    pub command: String,
    pub cwd: Option<PathBuf>,
}

impl OsProcess {
    /// Stable row id.
    #[must_use]
    pub fn key(&self) -> String {
        format!("pid:{}:{}", self.pid, self.start_ticks)
    }
}

/// Every live, tagged process this user can see.
#[cfg(target_os = "linux")]
#[must_use]
pub fn tagged_processes() -> Vec<OsProcess> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let boot_ms = boot_time_ms();
    let own = std::process::id();
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| *pid != own)
        .filter_map(|pid| read_process(pid, boot_ms))
        .collect()
}

#[cfg(not(target_os = "linux"))]
#[must_use]
pub fn tagged_processes() -> Vec<OsProcess> {
    Vec::new()
}

#[cfg(target_os = "linux")]
fn read_process(pid: u32, boot_ms: u64) -> Option<OsProcess> {
    let dir = PathBuf::from(format!("/proc/{pid}"));
    let environ = std::fs::read(dir.join("environ")).ok()?;
    let tags = parse_environ(&environ);
    let agent_id = tags.agent_id?;
    let stat = parse_stat(&std::fs::read_to_string(dir.join("stat")).ok()?)?;
    if stat.state == 'Z' || stat.state == 'X' {
        return None;
    }
    let command = std::fs::read(dir.join("cmdline"))
        .ok()
        .map(|raw| parse_cmdline(&raw))
        .filter(|c| !c.is_empty())?;
    Some(OsProcess {
        pid,
        ppid: stat.ppid,
        pgid: stat.pgid,
        start_ticks: stat.start_ticks,
        started_ms: boot_ms + stat.start_ticks * 1000 / CLOCK_TICKS_PER_SEC,
        agent_id,
        tool_use_id: tags.tool_use_id,
        harness_pid: tags.harness_pid,
        command,
        cwd: std::fs::read_link(dir.join("cwd")).ok(),
    })
}

/// `USER_HZ`, which Linux fixes at 100 for `/proc` on every architecture
/// it ships for.
#[cfg(target_os = "linux")]
const CLOCK_TICKS_PER_SEC: u64 = 100;

#[cfg(target_os = "linux")]
fn boot_time_ms() -> u64 {
    std::fs::read_to_string("/proc/stat")
        .ok()
        .and_then(|stat| {
            stat.lines()
                .find_map(|line| line.strip_prefix("btime "))
                .and_then(|secs| secs.trim().parse::<u64>().ok())
        })
        .map_or(0, |secs| secs * 1000)
}

/// Agent tags found in a NUL-separated environment block.
#[derive(Debug, Default, PartialEq, Eq)]
struct Tags {
    agent_id: Option<String>,
    tool_use_id: Option<String>,
    harness_pid: Option<u32>,
}

fn parse_environ(raw: &[u8]) -> Tags {
    let mut tags = Tags::default();
    for var in raw.split(|b| *b == 0) {
        let Ok(var) = std::str::from_utf8(var) else {
            continue;
        };
        let Some((key, value)) = var.split_once('=') else {
            continue;
        };
        match key {
            k if k == AGENT_ID_VAR => tags.agent_id = Some(value.to_string()),
            k if k == TOOL_USE_ID_VAR => tags.tool_use_id = Some(value.to_string()),
            k if k == HARNESS_PID_VAR => tags.harness_pid = value.parse().ok(),
            _ => {}
        }
    }
    tags
}

/// The fields of `/proc/<pid>/stat` the scan needs.
#[derive(Debug, PartialEq, Eq)]
struct Stat {
    state: char,
    ppid: u32,
    pgid: u32,
    start_ticks: u64,
}

/// Parse `/proc/<pid>/stat`. The command name is in parentheses and may
/// itself contain spaces and parentheses, so fields are counted from the
/// last `)`.
fn parse_stat(stat: &str) -> Option<Stat> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // `rest` starts at field 3 (state); starttime is field 22.
    Some(Stat {
        state: fields.first()?.chars().next()?,
        ppid: fields.get(1)?.parse().ok()?,
        pgid: fields.get(2)?.parse().ok()?,
        start_ticks: fields.get(19)?.parse().ok()?,
    })
}

fn parse_cmdline(raw: &[u8]) -> String {
    raw.split(|b| *b == 0)
        .filter(|arg| !arg.is_empty())
        .map(String::from_utf8_lossy)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Stop `process` and everything in its process group when it leads one.
///
/// # Errors
/// When `kill` can't be run or refuses.
pub fn kill(process: &OsProcess) -> Result<(), String> {
    let target = if process.pgid == process.pid {
        format!("-{}", process.pgid)
    } else {
        process.pid.to_string()
    };
    let status = std::process::Command::new("kill")
        .args(["-TERM", "--", &target])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("running kill: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("kill {target} failed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_fields_are_counted_after_the_command_name() {
        let stat = "4242 (node (vite) x) S 1 4242 4242 0 -1 4194560 1 0 0 0 3 1 0 0 20 0 \
                    11 0 987654 1000 100 18446744073709551615";
        assert_eq!(
            parse_stat(stat),
            Some(Stat {
                state: 'S',
                ppid: 1,
                pgid: 4242,
                start_ticks: 987_654,
            })
        );
    }

    #[test]
    fn environ_tags_are_read() {
        let raw = b"PATH=/bin\0AURA_AGENT_ID=abc\0AURA_TOOL_USE_ID=toolu_1\0AURA_HARNESS_PID=77\0";
        assert_eq!(
            parse_environ(raw),
            Tags {
                agent_id: Some("abc".into()),
                tool_use_id: Some("toolu_1".into()),
                harness_pid: Some(77),
            }
        );
        assert_eq!(parse_environ(b"PATH=/bin\0"), Tags::default());
    }

    #[test]
    fn cmdline_is_joined_with_spaces() {
        assert_eq!(
            parse_cmdline(b"node\0node_modules/.bin/vite\0--port=5173\0"),
            "node node_modules/.bin/vite --port=5173"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn finds_a_tagged_child_and_kills_its_group() {
        use std::os::unix::process::CommandExt;

        let agent = aura_core_types::AgentId::generate().to_hex();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .env(AGENT_ID_VAR, &agent)
            .env(TOOL_USE_ID_VAR, "toolu_scan")
            .process_group(0)
            .spawn()
            .unwrap();
        let found = listed_soon(&agent).expect("tagged child is listed");
        assert_eq!(found.pid, child.id());
        assert_eq!(found.pgid, child.id());
        assert_eq!(found.ppid, std::process::id());
        assert_eq!(found.tool_use_id.as_deref(), Some("toolu_scan"));
        assert_eq!(found.command, "sleep 30");
        assert!(found.started_ms > 0);

        kill(&found).unwrap();
        let status = child.wait().unwrap();
        assert!(!status.success());
    }

    /// The process tagged `agent`, once the scan lists it; `None` after
    /// ten seconds. `spawn` returns as soon as the child's `execve`
    /// closes its close-on-exec descriptors, before the kernel writes the
    /// new program's environment and arguments, so `/proc/<pid>/environ`
    /// can still read empty, untagged, for a moment, longer under load.
    /// The scan then rightly skips it, and lists it the next time.
    #[cfg(target_os = "linux")]
    fn listed_soon(agent: &str) -> Option<OsProcess> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let found = tagged_processes().into_iter().find(|p| p.agent_id == agent);
            if found.is_some() || std::time::Instant::now() >= deadline {
                return found;
            }
            std::thread::yield_now();
        }
    }
}
