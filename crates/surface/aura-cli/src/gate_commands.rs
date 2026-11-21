//! `aura gate`: reading Gate's attempt log.

use std::path::PathBuf;

use anyhow::Context as _;
use aura_agent_gate::attempt_log::{self, evaluate_where, Filter};

use crate::cli::{GateCommand, GateSubcommand};

/// The attempt log of the data directory a session here would use:
/// `AURA_DATA_DIR`, else `./aura_data`.
fn default_log() -> PathBuf {
    std::env::var_os("AURA_DATA_DIR")
        .map_or_else(|| PathBuf::from("./aura_data"), PathBuf::from)
        .join(attempt_log::FILE)
}

/// Run `aura gate`.
///
/// # Errors
///
/// When the log cannot be read.
pub fn cmd_gate(args: GateCommand) -> anyhow::Result<()> {
    match args.action {
        GateSubcommand::Eval {
            log,
            routed,
            passed,
            task,
            json,
        } => {
            let path = log.unwrap_or_else(default_log);
            let read =
                attempt_log::read(&path).with_context(|| format!("reading {}", path.display()))?;
            let report = evaluate_where(
                &read.lines,
                &Filter {
                    routed,
                    passed,
                    task,
                },
            );
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
                return Ok(());
            }
            println!("{}", path.display());
            if read.unreadable > 0 {
                println!(
                    "{} lines could not be read and are left out",
                    read.unreadable
                );
            }
            print!("{}", report.render());
            Ok(())
        }
    }
}
