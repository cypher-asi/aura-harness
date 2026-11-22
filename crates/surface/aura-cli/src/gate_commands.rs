//! `aura gate`: reading Gate's attempt log, and asking Gate's reviewer
//! about the attempts in it.

use std::path::PathBuf;

use anyhow::Context as _;
use aura_agent_gate::attempt_log::{self, evaluate_where, Filter};

use aura_app::headless::ReplayOptions;

use crate::cli::{GateCommand, GateSubcommand};

/// The data directory a session here would use: `AURA_DATA_DIR`, else
/// `./aura_data`.
fn default_data_dir() -> PathBuf {
    std::env::var_os("AURA_DATA_DIR").map_or_else(|| PathBuf::from("./aura_data"), PathBuf::from)
}

/// The attempt log of [`default_data_dir`].
fn default_log() -> PathBuf {
    default_data_dir().join(attempt_log::FILE)
}

/// Run `aura gate`.
///
/// # Errors
///
/// When the log cannot be read, or a replay cannot reach a model.
pub async fn cmd_gate(args: GateCommand) -> anyhow::Result<()> {
    match args.action {
        GateSubcommand::Replay {
            data_dir,
            task,
            limit,
            thinking,
            all,
            judge,
            dry_run,
            again,
            jobs,
            criteria,
            writer,
        } => {
            aura_app::headless::gate_replay(ReplayOptions {
                data_dir: data_dir.unwrap_or_else(default_data_dir),
                task,
                limit,
                thinking,
                any_difficulty: all,
                judge,
                dry_run,
                again,
                jobs,
                criteria,
                writer,
            })
            .await
        }
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
