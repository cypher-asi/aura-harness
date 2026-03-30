//! # aura-cli
//!
//! Interactive CLI for Aura.
//!
//! Provides a REPL interface for interacting with Aura agents.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic, clippy::nursery)]

// TODO(wiring): approval.rs is fully implemented and tested, but not yet
// integrated into the interactive session loop. Wire into handle_approve/handle_deny
// once the session supports pending tool requests.
#[allow(dead_code)]
mod approval;
mod commands;
mod handlers;
mod session;
mod session_helpers;
mod ui;

use anyhow::Result;
use colored::Colorize;
use commands::Command;
use handlers::{
    handle_approve, handle_deny, handle_diff, handle_history, handle_login, handle_logout,
    handle_prompt, handle_status, handle_whoami,
};
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::Editor;
use session::SessionConfig;
use tracing::info;
use tracing_subscriber::EnvFilter;
use ui::{banner, print_help};

#[tokio::main]
async fn main() -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    println!("{}", banner());

    let config = SessionConfig::from_env();
    let mut session = session::Session::new(config).await?;

    info!(agent_id = %session.agent_id(), "Session started");

    let mut rl: Editor<(), DefaultHistory> = Editor::new()?;
    let history_path = dirs::data_local_dir().map(|p| p.join("aura").join("history.txt"));

    if let Some(ref path) = history_path {
        let _ = rl.load_history(path);
    }

    loop {
        let prompt = format!("{} ", "aura>".cyan().bold());
        match rl.readline(&prompt) {
            Ok(line) => {
                let _ = rl.add_history_entry(&line);

                match Command::parse(&line) {
                    Command::Prompt(text) => {
                        if text.is_empty() {
                            continue;
                        }
                        handle_prompt(&mut session, &text).await;
                    }
                    Command::Status => handle_status(&session),
                    Command::History(n) => handle_history(&session, n),
                    Command::Approve => handle_approve(&session),
                    Command::Deny => handle_deny(&session),
                    Command::Diff => handle_diff(&session),
                    Command::Login => handle_login(&mut session, &mut rl).await,
                    Command::Logout => handle_logout(&mut session).await,
                    Command::Whoami => handle_whoami(),
                    Command::Help => print_help(),
                    Command::Quit => {
                        println!("{}", "Goodbye!".yellow());
                        break;
                    }
                    Command::Unknown(cmd) => {
                        println!(
                            "{} Unknown command: {}. Type /help for available commands.",
                            "Error:".red().bold(),
                            cmd
                        );
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("{}", "Use /quit or Ctrl-D to exit.".yellow());
            }
            Err(ReadlineError::Eof) => {
                println!("{}", "Goodbye!".yellow());
                break;
            }
            Err(err) => {
                eprintln!("{} {:?}", "Error:".red().bold(), err);
                break;
            }
        }
    }

    if let Some(ref path) = history_path {
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let path = path.clone();
        let _ = tokio::task::spawn_blocking(move || rl.save_history(&path)).await;
    }

    Ok(())
}
