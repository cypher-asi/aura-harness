//! Terminal WebSocket handler.
//!
//! Exposes a single WebSocket endpoint (`/ws/terminal`) that manages the
//! full PTY lifecycle within a single connection:
//!
//! 1. Client sends `{"type":"spawn","cols":80,"rows":24}`.
//! 2. Server spawns a PTY, sends `{"type":"spawned","shell":"..."}`.
//! 3. Bidirectional I/O: `input`/`output`/`resize`/`exit` JSON frames,
//!    with binary data base64-encoded — same protocol as aura-os.
//! 4. Closing the WebSocket kills the PTY.

use std::io::Read;
use std::sync::{Arc, Mutex};

use axum::extract::ws::{Message, WebSocket};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{info, warn};

fn default_shell() -> String {
    #[cfg(windows)]
    {
        if which::which("powershell.exe").is_ok() {
            "powershell.exe".into()
        } else {
            std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())
        }
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into())
    }
}

fn default_cwd() -> String {
    dirs::home_dir().map_or_else(
        || ".".into(),
        |p: std::path::PathBuf| p.to_string_lossy().into_owned(),
    )
}

#[derive(Deserialize)]
struct SpawnMsg {
    cols: Option<u16>,
    rows: Option<u16>,
}

#[derive(Deserialize)]
struct ClientMsg {
    #[serde(rename = "type")]
    msg_type: String,
    data: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
}

/// Handle a terminal WebSocket connection.
pub async fn handle_terminal_ws(mut socket: WebSocket) {
    let spawn = match wait_for_spawn(&mut socket).await {
        Some(s) => s,
        None => return,
    };

    let (shell, master, reader, writer) = match open_pty(&mut socket, &spawn).await {
        Some(t) => t,
        None => return,
    };

    let _ = send_json(
        &mut socket,
        &serde_json::json!({"type": "spawned", "shell": shell}),
    )
    .await;
    info!(shell = %shell, "Terminal PTY spawned");

    bridge_pty_ws(socket, reader, writer, master).await;

    info!("Terminal WebSocket disconnected");
}

async fn open_pty(
    socket: &mut WebSocket,
    spawn: &SpawnMsg,
) -> Option<(
    String,
    Arc<Mutex<Box<dyn MasterPty + Send>>>,
    Box<dyn Read + Send>,
    Box<dyn std::io::Write + Send>,
)> {
    let cols = spawn.cols.unwrap_or(80);
    let rows = spawn.rows.unwrap_or(24);
    let setup = tokio::task::spawn_blocking(move || setup_pty(cols, rows)).await;
    let (shell, master, reader, writer) = match setup {
        Ok(Ok(values)) => values,
        Ok(Err(msg)) => {
            let _ = send_json(socket, &serde_json::json!({"type":"exit","code":-1})).await;
            warn!("{msg}");
            return None;
        }
        Err(e) => {
            let _ = send_json(socket, &serde_json::json!({"type":"exit","code":-1})).await;
            warn!("PTY setup task failed: {e}");
            return None;
        }
    };

    Some((shell, master, reader, writer))
}

fn setup_pty(
    cols: u16,
    rows: u16,
) -> Result<
    (
        String,
        Arc<Mutex<Box<dyn MasterPty + Send>>>,
        Box<dyn Read + Send>,
        Box<dyn std::io::Write + Send>,
    ),
    String,
> {
    let shell = default_shell();
    let cwd = default_cwd();
    let pty_system = native_pty_system();
    let size = PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    };
    let pair = pty_system
        .openpty(size)
        .map_err(|e| format!("Failed to open PTY: {e}"))?;

    let mut cmd = CommandBuilder::new(&shell);
    cmd.cwd(&cwd);
    cmd.env("TERM", "xterm-256color");
    let _child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("Failed to spawn shell: {e}"))?;

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("Failed to clone PTY reader: {e}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("Failed to take PTY writer: {e}"))?;
    let master: Arc<Mutex<Box<dyn MasterPty + Send>>> = Arc::new(Mutex::new(pair.master));

    Ok((shell, master, reader, writer))
}

async fn bridge_pty_ws(
    socket: WebSocket,
    reader: Box<dyn Read + Send>,
    mut writer: Box<dyn std::io::Write + Send>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
) {
    let (output_tx, mut output_rx) = mpsc::channel::<Vec<u8>>(256);
    let (exit_tx, mut exit_rx) = mpsc::channel::<i32>(1);

    tokio::task::spawn_blocking(move || {
        read_pty_loop(reader, output_tx, exit_tx);
    });

    let (mut ws_write, mut ws_read) = socket.split();

    let outbound = async {
        loop {
            tokio::select! {
                Some(data) = output_rx.recv() => {
                    let msg = serde_json::json!({"type":"output","data": B64.encode(&data)});
                    if ws_write.send(Message::Text(msg.to_string())).await.is_err() {
                        break;
                    }
                }
                Some(code) = exit_rx.recv() => {
                    let _ = ws_write.send(Message::Text(
                        serde_json::json!({"type":"exit","code":code}).to_string(),
                    )).await;
                    break;
                }
            }
        }
    };

    let inbound = async {
        while let Some(Ok(msg)) = ws_read.next().await {
            let text = match msg {
                Message::Text(t) => t,
                Message::Close(_) => break,
                _ => continue,
            };
            let Ok(cm) = serde_json::from_str::<ClientMsg>(&text) else {
                continue;
            };
            handle_inbound_frame(&cm, &mut writer, &master);
        }
    };

    tokio::select! {
        _ = outbound => {}
        _ = inbound => {}
    }
}

/// Process a single inbound WebSocket frame (input or resize).
///
/// Uses `std::sync::Mutex` intentionally: the lock is held only for a
/// brief, non-async `resize()` call with no `.await` while locked.
/// Switching to `tokio::sync::Mutex` would require making this function
/// async for no practical benefit.
fn handle_inbound_frame(
    cm: &ClientMsg,
    writer: &mut Box<dyn std::io::Write + Send>,
    master: &Arc<Mutex<Box<dyn MasterPty + Send>>>,
) {
    match cm.msg_type.as_str() {
        "input" => {
            if let Some(ref data) = cm.data {
                if let Ok(bytes) = B64.decode(data) {
                    use std::io::Write;
                    if writer.write_all(&bytes).is_err() {
                        return;
                    }
                    let _ = writer.flush();
                }
            }
        }
        "resize" => {
            if let (Some(c), Some(r)) = (cm.cols, cm.rows) {
                if let Ok(m) = master.lock() {
                    let _ = m.resize(PtySize {
                        rows: r,
                        cols: c,
                        pixel_width: 0,
                        pixel_height: 0,
                    });
                }
            }
        }
        _ => {}
    }
}

// ─── helpers ─────────────────────────────────────────────────────────────

async fn wait_for_spawn(socket: &mut WebSocket) -> Option<SpawnMsg> {
    while let Some(Ok(msg)) = socket.next().await {
        let text = match msg {
            Message::Text(t) => t,
            Message::Close(_) => return None,
            _ => continue,
        };
        if let Ok(cm) = serde_json::from_str::<ClientMsg>(&text) {
            if cm.msg_type == "spawn" {
                return Some(SpawnMsg {
                    cols: cm.cols,
                    rows: cm.rows,
                });
            }
        }
    }
    None
}

async fn send_json(socket: &mut WebSocket, value: &serde_json::Value) -> Result<(), axum::Error> {
    socket.send(Message::Text(value.to_string())).await
}

fn read_pty_loop(
    mut reader: Box<dyn Read + Send>,
    output_tx: mpsc::Sender<Vec<u8>>,
    exit_tx: mpsc::Sender<i32>,
) {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => {
                let _ = exit_tx.blocking_send(0);
                break;
            }
            Ok(n) => {
                if output_tx.blocking_send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
            Err(_) => {
                let _ = exit_tx.blocking_send(-1);
                break;
            }
        }
    }
}
