//! `aura gate`: reading Gate's attempt log, and asking Gate's reviewer
//! about the attempts in it.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use aura_gate::attempts::{self, evaluate_where, Filter};

use aura_app::headless::ReplayOptions;

use crate::cli::{GateCommand, GateSubcommand};

/// The data directory a session here would use: `AURA_DATA_DIR`, else
/// `./aura_data`.
fn default_data_dir() -> PathBuf {
    std::env::var_os("AURA_DATA_DIR").map_or_else(|| PathBuf::from("./aura_data"), PathBuf::from)
}

/// The attempt log of [`default_data_dir`].
fn default_log() -> PathBuf {
    default_data_dir().join(attempts::FILE)
}

/// Run `aura gate`: hand each subcommand's flags, with their defaults
/// filled in, to the subcommand.
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
            let filter = Filter {
                routed,
                passed,
                task,
            };
            eval(&log.unwrap_or_else(default_log), &filter, json)
        }
    }
}

/// `aura gate eval`: score Gate on the attempts in the log at `path`
/// that `filter` lets through, and print the report, as JSON when `json`
/// says so, else as tables under the log's path and how many of its lines
/// could not be read.
fn eval(path: &Path, filter: &Filter, json: bool) -> anyhow::Result<()> {
    let read = attempts::read(path).with_context(|| format!("reading {}", path.display()))?;
    let report = evaluate_where(&read.lines, filter);
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
