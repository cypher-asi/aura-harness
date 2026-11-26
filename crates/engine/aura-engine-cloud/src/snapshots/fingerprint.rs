//! What decides whether a snapshot is still current: the fingerprints of
//! the base and of a project, and the build a project gets by default.

use std::path::Path;

use aura_config::MachineSize;

use super::BOOTSTRAP_VERSION;

/// Files whose contents decide a project snapshot.
const FINGERPRINT_FILES: &[&str] = &[
    "rust-toolchain.toml",
    "rust-toolchain",
    "Cargo.lock",
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lockb",
    "go.sum",
    "uv.lock",
    "poetry.lock",
    "requirements.txt",
];

/// Fingerprint of the base snapshot.
#[must_use]
pub fn base_fingerprint(size: MachineSize, aura_version: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    for part in [BOOTSTRAP_VERSION, aura_version, size.as_spec()] {
        hasher.update(part.as_bytes());
        hasher.update(&[0]);
    }
    hasher.finalize().to_hex()[..16].to_string()
}

/// Fingerprint of a project snapshot. Reads the lockfiles (blocking).
#[must_use]
pub fn project_fingerprint(project: &Path, base: &str, build_command: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    for part in [base, build_command, crate::workspace::FORMAT] {
        hasher.update(part.as_bytes());
        hasher.update(&[0]);
    }
    for file in FINGERPRINT_FILES {
        hasher.update(file.as_bytes());
        match std::fs::read(project.join(file)) {
            Ok(bytes) => {
                hasher.update(&[1]);
                hasher.update(&bytes);
            }
            Err(_) => {
                hasher.update(&[0]);
            }
        }
    }
    hasher.finalize().to_hex()[..16].to_string()
}

/// The build a project gets when none is configured.
#[must_use]
pub fn default_build_command(project: &Path) -> String {
    if project.join("Cargo.toml").is_file() {
        "cargo fetch && cargo test --workspace --no-run".into()
    } else if project.join("package-lock.json").is_file() {
        "npm ci".into()
    } else if project.join("package.json").is_file() {
        "npm install".into()
    } else {
        "true".into()
    }
}
