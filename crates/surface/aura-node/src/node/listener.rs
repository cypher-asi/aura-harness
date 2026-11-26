//! TCP listener binding with port self-heal, and graceful-shutdown signals.

use anyhow::Context;
use std::io;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tracing::{info, warn};

/// Env var that disables the auto-kill self-heal on `AddrInUse`. Set
/// `AURA_NODE_DISABLE_PORT_SELF_HEAL=1` to force the bind to fail
/// immediately when the port is taken (useful when the operator
/// explicitly wants to run a second instance side-by-side on a
/// different port via `AURA_NODE_BIND_ADDR` and would rather get a
/// loud error than have `aura-node` reach over and kill the
/// already-running instance).
pub(super) const DISABLE_SELF_HEAL_ENV: &str = "AURA_NODE_DISABLE_PORT_SELF_HEAL";

/// Bind the public HTTP listener with two layers of recovery on
/// `AddrInUse`:
///
/// 1. **Self-heal**: when the port is held by another process whose
///    image name starts with `aura-node` (i.e. an orphan from a
///    previous dev-run that did not exit cleanly — see
///    [`Node::run`](super::Node::run)'s `with_graceful_shutdown` comment for why that
///    happens in the first place on Windows), kill it and retry the
///    bind exactly once. This automates the manual
///    `Get-Process aura-node* | Stop-Process -Force` workflow that
///    operators had to run on every collision pre-fix.
///
/// 2. **Actionable error**: if the holder is something other than an
///    `aura-node` process (so we deliberately do not kill it — the
///    user might intentionally have another service on 8080), fall
///    through to [`format_addr_in_use_error`] which names the
///    conflict, prints the kill one-liner the operator can run, and
///    suggests the `AURA_NODE_BIND_ADDR` escape hatch.
///
/// Self-heal is gated on [`DISABLE_SELF_HEAL_ENV`] so an operator who
/// is intentionally running two aura-nodes can disable the auto-kill
/// without losing the actionable error message.
pub(super) async fn bind_listener(addr: SocketAddr) -> anyhow::Result<TcpListener> {
    match TcpListener::bind(addr).await {
        Ok(listener) => Ok(listener),
        Err(err) if err.kind() == io::ErrorKind::AddrInUse => {
            if !self_heal_disabled() {
                match try_self_heal_orphan(addr).await {
                    Ok(true) => match TcpListener::bind(addr).await {
                        Ok(listener) => {
                            info!(%addr, "rebound after killing orphaned aura-node holder");
                            return Ok(listener);
                        }
                        Err(retry_err) => {
                            warn!(
                                %addr,
                                error = %retry_err,
                                "self-heal killed orphan but rebind still failed; reporting original error"
                            );
                        }
                    },
                    Ok(false) => {
                        // Holder is not an aura-node process, or we
                        // could not identify it — fall through to the
                        // actionable error below rather than killing
                        // an unrelated server.
                    }
                    Err(probe_err) => {
                        warn!(
                            %addr,
                            error = %probe_err,
                            "could not probe port holder for self-heal; falling through to actionable error"
                        );
                    }
                }
            }
            Err(anyhow::anyhow!("{}", format_addr_in_use_error(addr, &err)))
        }
        Err(err) => Err(anyhow::Error::new(err).context(format!("binding TCP listener on {addr}"))),
    }
}

pub(super) fn self_heal_disabled() -> bool {
    std::env::var(DISABLE_SELF_HEAL_ENV)
        .ok()
        .is_some_and(|v| matches!(v.trim(), "1" | "true" | "True" | "TRUE" | "yes" | "on"))
}

/// Returns `Ok(true)` if the port holder was an `aura-node*` process
/// and was successfully killed; `Ok(false)` if the holder is not an
/// aura-node (so the caller should preserve the operator's other
/// service) or could not be identified; `Err` only for unexpected
/// failures probing the OS process table.
async fn try_self_heal_orphan(addr: SocketAddr) -> anyhow::Result<bool> {
    let Some((pid, image)) = find_port_holder(addr).await? else {
        return Ok(false);
    };
    if !is_aura_node_image(&image) {
        info!(
            %addr,
            holder_pid = pid,
            holder_image = %image,
            "port held by non-aura-node process; not self-healing"
        );
        return Ok(false);
    }
    warn!(
        %addr,
        orphan_pid = pid,
        orphan_image = %image,
        "self-healing: killing orphaned aura-node holder before retrying bind"
    );
    kill_pid(pid).await?;
    // Give the OS a beat to actually release the socket. Windows in
    // particular sometimes holds the port briefly after the owner
    // exits.
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    Ok(true)
}

/// Predicate factored out for unit testing: any process whose image
/// name starts with `aura-node` is treated as one of "ours" for the
/// purpose of self-heal. Matches both `aura-node.exe` (the installed
/// or release binary) and cargo's `aura-node-0.1.0-<hash>.exe`
/// scratch builds that orphan most often during iterative dev.
pub(super) fn is_aura_node_image(image: &str) -> bool {
    image.trim().to_ascii_lowercase().starts_with("aura-node")
}

#[cfg(windows)]
async fn find_port_holder(addr: SocketAddr) -> anyhow::Result<Option<(u32, String)>> {
    let needle = format!("{}:{}", addr.ip(), addr.port());
    let output = tokio::process::Command::new("netstat")
        .args(["-ano", "-p", "TCP"])
        .output()
        .await
        .context("running netstat to probe port holder")?;
    if !output.status.success() {
        anyhow::bail!(
            "netstat exited with status {}",
            output.status.code().unwrap_or(-1)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let pid_opt = stdout
        .lines()
        .filter(|line| line.contains("LISTENING"))
        .filter(|line| line.contains(&needle))
        .filter_map(|line| line.split_whitespace().last())
        .filter_map(|tok| tok.parse::<u32>().ok())
        .next();
    let Some(pid) = pid_opt else {
        return Ok(None);
    };

    let image = tokio::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .await
        .context("running tasklist to identify port holder")?;
    if !image.status.success() {
        return Ok(Some((pid, String::new())));
    }
    let csv = String::from_utf8_lossy(&image.stdout);
    // tasklist CSV row: "Image Name","PID","Session Name","Session#","Mem Usage"
    let image_name = csv
        .lines()
        .find(|l| !l.trim().is_empty())
        .and_then(|line| line.split(',').next())
        .map(|raw| raw.trim().trim_matches('"').to_string())
        .unwrap_or_default();
    Ok(Some((pid, image_name)))
}

#[cfg(unix)]
async fn find_port_holder(addr: SocketAddr) -> anyhow::Result<Option<(u32, String)>> {
    let target = format!("{}:{}", addr.ip(), addr.port());
    let output = tokio::process::Command::new("lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-Fpc"])
        .output()
        .await
        .context("running lsof to probe port holder")?;
    if !output.status.success() {
        // lsof returns non-zero when nothing matches; treat as "no holder identified".
        return Ok(None);
    }
    // lsof -Fpc emits records like:
    //   p1234
    //   caura-node
    //   p5678
    //   ...
    // We need the (pid, image) tuple whose later TCP entry mentions our address;
    // simpler: run a second targeted query.
    let _ = output;
    let detail = tokio::process::Command::new("lsof")
        .args(["-tnP", &format!("-iTCP@{target}"), "-sTCP:LISTEN"])
        .output()
        .await
        .context("running targeted lsof for port holder")?;
    let pid_opt = String::from_utf8_lossy(&detail.stdout)
        .lines()
        .find_map(|l| l.trim().parse::<u32>().ok());
    let Some(pid) = pid_opt else {
        return Ok(None);
    };
    let comm = tokio::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .await
        .context("running ps to identify port holder image")?;
    let image_name = String::from_utf8_lossy(&comm.stdout).trim().to_string();
    Ok(Some((pid, image_name)))
}

#[cfg(not(any(unix, windows)))]
async fn find_port_holder(_addr: SocketAddr) -> anyhow::Result<Option<(u32, String)>> {
    Ok(None)
}

#[cfg(windows)]
async fn kill_pid(pid: u32) -> anyhow::Result<()> {
    let status = tokio::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .status()
        .await
        .context("running taskkill")?;
    if !status.success() {
        anyhow::bail!(
            "taskkill exited with status {}",
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}

#[cfg(unix)]
async fn kill_pid(pid: u32) -> anyhow::Result<()> {
    let status = tokio::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .await
        .context("running kill -9")?;
    if !status.success() {
        anyhow::bail!("kill exited with status {}", status.code().unwrap_or(-1));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
async fn kill_pid(_pid: u32) -> anyhow::Result<()> {
    anyhow::bail!("kill is not implemented on this platform")
}

/// Render the actionable `AddrInUse` message. Factored out so the
/// formatting can be unit-tested without a live `TcpListener`.
pub(super) fn format_addr_in_use_error(addr: SocketAddr, err: &io::Error) -> String {
    let killer = if cfg!(windows) {
        "powershell -Command \"Get-Process -Name aura-node* -ErrorAction SilentlyContinue | Stop-Process -Force\""
    } else {
        "pkill -f aura-node"
    };
    format!(
        "binding TCP listener on {addr}: address already in use ({err}).\n\
         \n\
         A previous `aura-node` process is most likely still holding the port. \
         To kill it and retry:\n\
         \n  {killer}\n\n\
         Set `AURA_NODE_BIND_ADDR` to a different `host:port` if you want \
         to run a second instance side-by-side instead."
    )
}

/// Await the first OS shutdown signal and return. Resolves on Ctrl-C
/// everywhere; on Unix also resolves on `SIGTERM` (so `docker stop`
/// /  systemd `Stop=` close the listener cleanly); on Windows also
/// resolves on `Ctrl-Break`.
///
/// Awaiting any of these returns `()` so `axum::serve(...)
/// .with_graceful_shutdown(shutdown_signal())` drains in-flight
/// requests, closes the TCP listener, and lets `run` return `Ok(())`
/// instead of leaking the process across `cargo run` invocations.
pub(super) async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            warn!(error = %err, "failed to install Ctrl-C handler; graceful shutdown disabled");
            // Park forever so the `select!` falls through to whichever
            // platform-specific signal we did install.
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(err) => {
                warn!(error = %err, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(windows)]
    let terminate = async {
        match tokio::signal::windows::ctrl_break() {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(err) => {
                warn!(error = %err, "failed to install Ctrl-Break handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(any(unix, windows)))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {
            info!("Ctrl-C received; shutting down HTTP server");
        }
        () = terminate => {
            info!("termination signal received; shutting down HTTP server");
        }
    }
}
