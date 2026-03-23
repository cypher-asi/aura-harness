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
mod session;

use anyhow::Result;
use colored::Colorize;
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::Editor;
use session::{Session, SessionConfig};
use tracing::info;
use tracing_subscriber::EnvFilter;

// ============================================================================
// Commands
// ============================================================================

/// Parsed CLI command from user input.
///
/// Input that doesn't start with `/` is treated as a [`Command::Prompt`].
/// Slash-prefixed input is parsed as a named command (e.g. `/status`, `/quit`).
#[derive(Debug)]
enum Command {
    /// Free-text prompt sent to the agent for processing.
    Prompt(String),
    /// Display session status (agent ID, sequence, provider).
    Status,
    /// Show the last *n* history entries.
    History(usize),
    /// Approve the current pending tool-execution request.
    Approve,
    /// Deny the current pending tool-execution request.
    Deny,
    /// Show pending file-level changes (diff).
    Diff,
    /// Authenticate with zOS to obtain a JWT for proxy mode.
    Login,
    /// Clear stored zOS credentials and log out.
    Logout,
    /// Show current authentication status.
    Whoami,
    /// Print the help message listing available commands.
    Help,
    /// Exit the CLI gracefully.
    Quit,
    /// Unrecognised slash-command.
    Unknown(String),
}

impl Command {
    fn parse(input: &str) -> Self {
        let input = input.trim();

        if input.is_empty() {
            return Self::Prompt(String::new());
        }

        if !input.starts_with('/') {
            return Self::Prompt(input.to_string());
        }

        let parts: Vec<&str> = input[1..].splitn(2, ' ').collect();
        let cmd = parts[0].to_lowercase();
        let arg = parts.get(1).unwrap_or(&"").trim();

        match cmd.as_str() {
            "status" | "s" => Self::Status,
            "history" | "h" => {
                let n = arg.parse().unwrap_or(10);
                Self::History(n)
            }
            "approve" | "yes" | "y" => Self::Approve,
            "deny" | "no" | "n" => Self::Deny,
            "diff" | "d" => Self::Diff,
            "login" => Self::Login,
            "logout" => Self::Logout,
            "whoami" | "me" => Self::Whoami,
            "help" | "?" => Self::Help,
            "quit" | "exit" | "q" => Self::Quit,
            _ => Self::Unknown(cmd),
        }
    }
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    println!("{}", banner());

    let config = SessionConfig::from_env();
    let mut session = Session::new(config).await?;

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

// ============================================================================
// Command Handlers
// ============================================================================

/// Submit a user prompt to the agent and display the response.
async fn handle_prompt(session: &mut Session, text: &str) {
    println!("{} Processing...\n", "▶".blue().bold());

    match session.submit_prompt(text).await {
        Ok(result) => {
            if !result.total_text.is_empty() {
                println!("{}", result.total_text);
            }

            if let Some(ref err) = result.llm_error {
                println!("{} LLM error: {}", "⚠".yellow().bold(), err);
            }

            println!(
                "\n{} Iterations: {}, Input tokens: {}, Output tokens: {}",
                "✓".green().bold(),
                result.iterations,
                result.total_input_tokens,
                result.total_output_tokens
            );

            if result.timed_out {
                println!("{} Agent loop timed out", "⚠".yellow().bold());
            }
            if result.insufficient_credits {
                println!("{} Insufficient credits", "⚠".yellow().bold());
            }
        }
        Err(e) => {
            eprintln!("{} {}", "Error:".red().bold(), e);
        }
    }
    println!();
}

/// Print current session status (agent ID, sequence, provider).
fn handle_status(session: &Session) {
    println!("{}", "Session Status".cyan().bold());
    println!("  Agent ID: {}", session.agent_id());
    println!("  Sequence: {}", session.current_seq());
    println!("  Provider: {}", session.provider_name());
    println!();
}

/// TODO: Implement - currently prints placeholder message.
///
/// Display the last `n` conversation history entries.
fn handle_history(_session: &Session, _n: usize) {
    println!("{} History display not yet implemented", "ℹ".blue().bold());
    println!();
}

/// Approve the currently pending tool-execution request.
fn handle_approve(session: &Session) {
    if let Err(e) = session.approve_pending() {
        eprintln!("{} {}", "Error:".red().bold(), e);
    } else {
        println!("{} Approved", "✓".green().bold());
    }
    println!();
}

/// Deny the currently pending tool-execution request.
fn handle_deny(session: &Session) {
    if let Err(e) = session.deny_pending() {
        eprintln!("{} {}", "Error:".red().bold(), e);
    } else {
        println!("{} Denied", "✗".red().bold());
    }
    println!();
}

/// TODO: Implement - currently prints placeholder message.
///
/// Display pending file-level changes (not yet implemented).
fn handle_diff(_session: &Session) {
    println!("{} Diff display not yet implemented", "ℹ".blue().bold());
    println!();
}

/// Prompt for zOS credentials and authenticate to obtain a JWT.
async fn handle_login(session: &mut Session, rl: &mut Editor<(), DefaultHistory>) {
    let email = match rl.readline("  Email: ") {
        Ok(e) if !e.trim().is_empty() => e.trim().to_string(),
        Ok(_) => {
            println!("{} Email cannot be empty", "Error:".red().bold());
            println!();
            return;
        }
        Err(_) => {
            println!("{} Login cancelled", "ℹ".blue().bold());
            println!();
            return;
        }
    };

    let password = match rpassword::prompt_password_stdout("  Password: ") {
        Ok(ref p) if !p.is_empty() => p.clone(),
        Ok(_) => {
            println!("{} Password cannot be empty", "Error:".red().bold());
            println!();
            return;
        }
        Err(e) => {
            eprintln!("{} Failed to read password: {e}", "Error:".red().bold());
            println!();
            return;
        }
    };

    println!("{} Authenticating...", "▶".blue().bold());

    let client = match aura_auth::ZosClient::new() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{} {e}", "Error:".red().bold());
            println!();
            return;
        }
    };

    match client.login(&email, &password).await {
        Ok(stored) => {
            let display = stored.display_name.clone();
            let zid = stored.primary_zid.clone();
            let token = stored.access_token.clone();

            if let Err(e) = aura_auth::CredentialStore::save(&stored) {
                eprintln!("{} Failed to save credentials: {e}", "Error:".red().bold());
                println!();
                return;
            }

            session.set_auth_token(Some(token));

            println!(
                "{} Logged in as {} ({})",
                "✓".green().bold(),
                display.green(),
                zid,
            );
        }
        Err(e) => {
            eprintln!("{} Login failed: {e}", "Error:".red().bold());
        }
    }
    println!();
}

/// Clear stored credentials and invalidate the remote session.
async fn handle_logout(session: &mut Session) {
    if let Some(stored) = aura_auth::CredentialStore::load() {
        let client = match aura_auth::ZosClient::new() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{} {e}", "Error:".red().bold());
                println!();
                return;
            }
        };
        client.logout(&stored.access_token).await;
    }

    if let Err(e) = aura_auth::CredentialStore::clear() {
        eprintln!("{} Failed to clear credentials: {e}", "Error:".red().bold());
        println!();
        return;
    }

    session.set_auth_token(None);
    println!("{} Logged out", "✓".green().bold());
    println!();
}

/// Show current authentication status from stored credentials.
fn handle_whoami() {
    match aura_auth::CredentialStore::load() {
        Some(session) => {
            println!("{}", "Authentication".cyan().bold());
            println!("  Name:    {}", session.display_name);
            println!("  zID:     {}", session.primary_zid);
            println!("  User ID: {}", session.user_id);
            println!(
                "  Since:   {}",
                session.created_at.format("%Y-%m-%d %H:%M UTC")
            );
        }
        None => {
            println!(
                "{} Not logged in. Use /login to authenticate.",
                "ℹ".blue().bold()
            );
        }
    }
    println!();
}

/// Print the available commands and their descriptions.
fn print_help() {
    println!("{}", "Available Commands".cyan().bold());
    println!();
    println!("  {}    Submit a prompt to the agent", "<text>".green());
    println!("  {}   Show agent status", "/status".green());
    println!("  {} Show last N history entries", "/history [n]".green());
    println!("  {}  Approve pending tool request", "/approve".green());
    println!("  {}     Deny pending tool request", "/deny".green());
    println!("  {}     Show pending file changes", "/diff".green());
    println!("  {}    Login to zOS for proxy mode", "/login".green());
    println!("  {}   Clear stored credentials", "/logout".green());
    println!("  {}   Show auth status", "/whoami".green());
    println!("  {}     Show this help message", "/help".green());
    println!("  {}     Exit the CLI", "/quit".green());
    println!();
    println!("  Shortcuts: /s, /h, /y, /n, /d, /me, /?, /q");
    println!();
}

fn banner() -> String {
    format!(
        r"
{}
Version: {}
Type /help for available commands.
",
        r"
    _   _   _ ____      _    
   / \ | | | |  _ \    / \   
  / _ \| | | | |_) |  / _ \  
 / ___ \ |_| |  _ <  / ___ \ 
/_/   \_\___/|_| \_\/_/   \_\
"
        .cyan()
        .bold(),
        env!("CARGO_PKG_VERSION")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_prompt_text() {
        match Command::parse("hello world") {
            Command::Prompt(t) => assert_eq!(t, "hello world"),
            _ => panic!("Expected Prompt"),
        }
    }

    #[test]
    fn test_parse_empty_input() {
        match Command::parse("") {
            Command::Prompt(t) => assert!(t.is_empty()),
            _ => panic!("Expected empty Prompt"),
        }
    }

    #[test]
    fn test_parse_whitespace_only() {
        match Command::parse("   ") {
            Command::Prompt(t) => assert!(t.is_empty()),
            _ => panic!("Expected empty Prompt"),
        }
    }

    #[test]
    fn test_parse_status_commands() {
        assert!(matches!(Command::parse("/status"), Command::Status));
        assert!(matches!(Command::parse("/s"), Command::Status));
        assert!(matches!(Command::parse("/STATUS"), Command::Status));
    }

    #[test]
    fn test_parse_history_default() {
        match Command::parse("/history") {
            Command::History(n) => assert_eq!(n, 10),
            _ => panic!("Expected History"),
        }
    }

    #[test]
    fn test_parse_history_with_arg() {
        match Command::parse("/history 5") {
            Command::History(n) => assert_eq!(n, 5),
            _ => panic!("Expected History"),
        }
    }

    #[test]
    fn test_parse_history_shortcut() {
        match Command::parse("/h 20") {
            Command::History(n) => assert_eq!(n, 20),
            _ => panic!("Expected History"),
        }
    }

    #[test]
    fn test_parse_history_invalid_arg() {
        match Command::parse("/history abc") {
            Command::History(n) => assert_eq!(n, 10),
            _ => panic!("Expected History with default"),
        }
    }

    #[test]
    fn test_parse_approve_commands() {
        assert!(matches!(Command::parse("/approve"), Command::Approve));
        assert!(matches!(Command::parse("/yes"), Command::Approve));
        assert!(matches!(Command::parse("/y"), Command::Approve));
    }

    #[test]
    fn test_parse_deny_commands() {
        assert!(matches!(Command::parse("/deny"), Command::Deny));
        assert!(matches!(Command::parse("/no"), Command::Deny));
        assert!(matches!(Command::parse("/n"), Command::Deny));
    }

    #[test]
    fn test_parse_diff() {
        assert!(matches!(Command::parse("/diff"), Command::Diff));
        assert!(matches!(Command::parse("/d"), Command::Diff));
    }

    #[test]
    fn test_parse_help() {
        assert!(matches!(Command::parse("/help"), Command::Help));
        assert!(matches!(Command::parse("/?"), Command::Help));
    }

    #[test]
    fn test_parse_quit_commands() {
        assert!(matches!(Command::parse("/quit"), Command::Quit));
        assert!(matches!(Command::parse("/exit"), Command::Quit));
        assert!(matches!(Command::parse("/q"), Command::Quit));
    }

    #[test]
    fn test_parse_unknown_command() {
        match Command::parse("/foobar") {
            Command::Unknown(cmd) => assert_eq!(cmd, "foobar"),
            _ => panic!("Expected Unknown"),
        }
    }

    #[test]
    fn test_parse_case_insensitive() {
        assert!(matches!(Command::parse("/QUIT"), Command::Quit));
        assert!(matches!(Command::parse("/Help"), Command::Help));
        assert!(matches!(Command::parse("/DIFF"), Command::Diff));
    }

    #[test]
    fn test_parse_login() {
        assert!(matches!(Command::parse("/login"), Command::Login));
        assert!(matches!(Command::parse("/LOGIN"), Command::Login));
    }

    #[test]
    fn test_parse_logout() {
        assert!(matches!(Command::parse("/logout"), Command::Logout));
        assert!(matches!(Command::parse("/LOGOUT"), Command::Logout));
    }

    #[test]
    fn test_parse_whoami() {
        assert!(matches!(Command::parse("/whoami"), Command::Whoami));
        assert!(matches!(Command::parse("/me"), Command::Whoami));
        assert!(matches!(Command::parse("/WHOAMI"), Command::Whoami));
    }
}
