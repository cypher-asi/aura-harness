use colored::Colorize;
use rustyline::history::DefaultHistory;
use rustyline::Editor;

use crate::session::Session;

/// Submit a user prompt to the agent and display the response.
pub async fn handle_prompt(session: &mut Session, text: &str) {
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
pub fn handle_status(session: &Session) {
    println!("{}", "Session Status".cyan().bold());
    println!("  Agent ID: {}", session.agent_id());
    println!("  Sequence: {}", session.current_seq());
    println!("  Provider: {}", session.provider_name());
    println!();
}

/// TODO: Implement - currently prints placeholder message.
///
/// Display the last `n` conversation history entries.
pub fn handle_history(_session: &Session, _n: usize) {
    println!("{} History display not yet implemented", "ℹ".blue().bold());
    println!();
}

/// Approve the currently pending tool-execution request.
pub fn handle_approve(session: &Session) {
    if let Err(e) = session.approve_pending() {
        eprintln!("{} {}", "Error:".red().bold(), e);
    } else {
        println!("{} Approved", "✓".green().bold());
    }
    println!();
}

/// Deny the currently pending tool-execution request.
pub fn handle_deny(session: &Session) {
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
pub fn handle_diff(_session: &Session) {
    println!("{} Diff display not yet implemented", "ℹ".blue().bold());
    println!();
}

/// Prompt for zOS credentials and authenticate to obtain a JWT.
pub async fn handle_login(session: &mut Session, rl: &mut Editor<(), DefaultHistory>) {
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
pub async fn handle_logout(session: &mut Session) {
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
pub fn handle_whoami() {
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
