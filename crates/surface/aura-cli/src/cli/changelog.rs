//! `aura changelog`: the changelog fragments of `changelog.d/`, put
//! together.

use clap::Args;
use std::path::PathBuf;

/// `aura changelog`: show the `[Unreleased]` entries the fragments in
/// `changelog.d/` make, newest first; `--write` puts them into
/// CHANGELOG.md and deletes them, for a release.
#[derive(Args, Debug)]
pub struct ChangelogArgs {
    /// Write CHANGELOG.md and delete the fragments, instead of showing
    /// what would go in.
    #[arg(long)]
    pub write: bool,
    /// Folder in the repo (default: this one).
    #[arg(long, value_name = "DIR")]
    pub repo: Option<PathBuf>,
}
