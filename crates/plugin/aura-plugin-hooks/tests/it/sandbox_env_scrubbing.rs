//! Phase 8 sandbox env-scrubbing integration test.
//!
//! Spawns a real OS process via [`aura_plugin_hooks::sandbox::scrubbed_env`]
//! that dumps its env to stdout and asserts:
//!
//! 1. Secret patterns from [`SECRET_PATTERNS`] / [`SECRET_NAMES`] are
//!    absent in the child's env.
//! 2. The canonical Aura vars (`AURA_HOME`, `PLUGIN_ROOT`,
//!    `AURA_SESSION_ID`, `AURA_AGENT_ID`, `AURA_PARENT_AGENT_ID`,
//!    `AURA_EVENT`) are present with the expected values.
//! 3. The Codex/Claude compat aliases (`CODEX_HOME`,
//!    `CLAUDE_PLUGIN_ROOT`, `CODEX_SESSION_ID`, `CODEX_AGENT_ID`,
//!    `CODEX_PARENT_ID`, `CODEX_EVENT`) are present and mirror the
//!    Aura values.

// Integration-test crate: test code may unwrap/expect.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use aura_plugin_hooks::sandbox::{scrubbed_env, InjectedEnv, ENV_TEST_LOCK};

#[cfg(unix)]
fn dump_env_script(dir: &std::path::Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join("dump-env.sh");
    std::fs::write(&p, "#!/bin/sh\nenv\n").unwrap();
    let mut perm = std::fs::metadata(&p).unwrap().permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(&p, perm).unwrap();
    p
}

#[cfg(windows)]
fn dump_env_script(dir: &std::path::Path) -> PathBuf {
    let p = dir.join("dump-env.cmd");
    // `set` lists every env var as `KEY=VALUE` lines, one per line —
    // exactly the shape `parse_env_dump` expects.
    std::fs::write(&p, "@echo off\r\nset\r\n").unwrap();
    p
}

fn run_dump(dir: &std::path::Path, env: &BTreeMap<String, String>) -> String {
    let script = dump_env_script(dir);
    let mut cmd = Command::new(if cfg!(windows) { "cmd.exe" } else { "/bin/sh" });
    if cfg!(windows) {
        cmd.args(["/C", script.to_string_lossy().as_ref()]);
    } else {
        cmd.arg(script.as_os_str());
    }
    cmd.env_clear();
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("spawn dump-env");
    assert!(out.status.success(), "dump-env exited non-zero");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn parse_env_dump(out: &str) -> BTreeMap<String, String> {
    out.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

struct EnvVarGuard {
    key: &'static str,
    previous: Option<String>,
}
impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(p) = self.previous.take() {
            std::env::set_var(self.key, p);
        } else {
            std::env::remove_var(self.key);
        }
    }
}
fn set_test_env(key: &'static str, value: &str) -> EnvVarGuard {
    let previous = std::env::var(key).ok();
    std::env::set_var(key, value);
    EnvVarGuard { key, previous }
}

fn injected(home: &std::path::Path, root: &std::path::Path) -> InjectedEnv {
    InjectedEnv {
        event_name: "pre_tool_use",
        aura_home: home.to_path_buf(),
        plugin_root: root.to_path_buf(),
        session_id: "sess-X".into(),
        agent_id: "agent-Y".into(),
        parent_agent_id: Some("parent-Z".into()),
        extra: BTreeMap::new(),
    }
}

/// Representative secrets planted in the parent env; every key must be
/// absent from the hook child's env.
const PLANTED_SECRETS: [(&str, &str); 13] = [
    ("AWS_SECRET_ACCESS_KEY", "akia-fake-leak"),
    ("GCP_SERVICE_ACCOUNT", "gcp-fake-leak"),
    ("AZURE_CLIENT_SECRET", "azure-fake-leak"),
    ("OPENAI_API_KEY", "sk-fake-leak"),
    ("ANTHROPIC_API_KEY", "sk-ant-fake-leak"),
    ("GOOGLE_API_KEY", "gak-fake-leak"),
    ("KUBECONFIG", "/etc/kube/config"),
    ("SSH_AUTH_SOCK", "/tmp/ssh.sock"),
    ("DOCKER_HOST", "tcp://example:1234"),
    ("MY_GITHUB_TOKEN", "ghp-fake-leak"),
    ("DB_HMAC_SECRET", "fake-leak"),
    ("ROOT_PASSWORD", "fake-leak"),
    ("APP_SIGNING_KEY", "fake-leak"),
];

fn plant_secrets() -> Vec<EnvVarGuard> {
    PLANTED_SECRETS
        .iter()
        .map(|(key, value)| set_test_env(key, value))
        .collect()
}

fn assert_env_values(observed: &BTreeMap<String, String>, expected: &[(&str, &str)]) {
    for (key, value) in expected {
        assert_eq!(
            observed.get(*key).map(String::as_str),
            Some(*value),
            "unexpected value for {key}"
        );
    }
}

#[test]
fn sandbox_env_scrubbing_removes_secrets_and_injects_canonicals() {
    let _lock = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _secret_guards = plant_secrets();

    let dir = tempfile::TempDir::new().unwrap();
    let aura_home = dir.path().join(".aura");
    let plugin_root = aura_home.join("plugins").join("demo").join("1.0.0");
    std::fs::create_dir_all(&plugin_root).unwrap();

    let env = scrubbed_env(&injected(&aura_home, &plugin_root));
    let dump = run_dump(dir.path(), &env);
    let observed = parse_env_dump(&dump);

    // (a) secrets must NOT appear in the child env.
    for (forbidden, _) in PLANTED_SECRETS {
        assert!(
            !observed.contains_key(forbidden),
            "secret env var '{forbidden}' leaked into hook child env"
        );
    }

    let aura_home_str = aura_home.to_string_lossy().into_owned();
    let plugin_root_str = plugin_root.to_string_lossy().into_owned();
    // (b) canonical Aura vars present with expected values.
    assert_env_values(
        &observed,
        &[
            ("AURA_HOME", &aura_home_str),
            ("PLUGIN_ROOT", &plugin_root_str),
            ("AURA_SESSION_ID", "sess-X"),
            ("AURA_AGENT_ID", "agent-Y"),
            ("AURA_PARENT_AGENT_ID", "parent-Z"),
            ("AURA_EVENT", "pre_tool_use"),
        ],
    );
    // (c) Codex / Claude aliases mirror the Aura values.
    assert_env_values(
        &observed,
        &[
            ("CODEX_HOME", &aura_home_str),
            ("CLAUDE_PLUGIN_ROOT", &plugin_root_str),
            ("CODEX_SESSION_ID", "sess-X"),
            ("CODEX_AGENT_ID", "agent-Y"),
            ("CODEX_PARENT_ID", "parent-Z"),
            ("CODEX_EVENT", "pre_tool_use"),
        ],
    );
}
