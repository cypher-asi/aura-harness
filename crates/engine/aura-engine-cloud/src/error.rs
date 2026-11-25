//! Errors from the cloud layer. Every variant names the operation and,
//! where there is one, the machine or snapshot it was about.

use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;

use crate::glibc::GlibcVersion;

/// Result alias for this crate.
pub type CloudResult<T> = Result<T, CloudError>;

/// What went wrong talking to boxd or a cloud node.
#[derive(Debug, Error)]
pub enum CloudError {
    /// No `BOXD_API_KEY` / `BOXD_TOKEN` is set.
    #[error("{}", aura_config::MISSING_CREDENTIAL_NOTICE)]
    NotConfigured,
    /// Exchanging the API key for a token failed.
    #[error("boxd sign-in failed: {0}")]
    Auth(String),
    /// A gRPC call returned an error status.
    #[error("boxd {op} failed ({code}): {message}")]
    Rpc {
        /// The call (`CreateVm`, `Exec`, ...).
        op: &'static str,
        /// gRPC status code name.
        code: String,
        /// boxd's message.
        message: String,
    },
    /// A call did not finish in time.
    #[error("boxd {op} timed out after {after:?}")]
    Timeout {
        /// The call.
        op: &'static str,
        /// The limit it hit.
        after: Duration,
    },
    /// The connection to boxd could not be made.
    #[error("could not reach boxd at {endpoint}: {message}")]
    Transport {
        /// Endpoint URL.
        endpoint: String,
        /// Why.
        message: String,
    },
    /// boxd answered with something this build can't use.
    #[error("unexpected answer from boxd {op}: {message}")]
    InvalidResponse {
        /// The call.
        op: &'static str,
        /// What was wrong.
        message: String,
    },
    /// The org already has as many machines as it may.
    #[error("boxd machine limit reached: {count} of {max} machines in use")]
    MachineLimit {
        /// Machines in use.
        count: u32,
        /// The limit.
        max: u32,
    },
    /// A command run in a machine exited with an error.
    #[error("`{command}` on {machine} exited with {exit_code}: {stderr}")]
    ExecFailed {
        /// Machine name or id.
        machine: String,
        /// Command, without any secret input.
        command: String,
        /// Its exit code.
        exit_code: i32,
        /// The end of its stderr.
        stderr: String,
    },
    /// A machine or snapshot is not there.
    #[error("{what} not found: {name}")]
    NotFound {
        /// `machine` or `snapshot`.
        what: &'static str,
        /// Its name or id.
        name: String,
    },
    /// A call to a cloud node's own API failed.
    #[error("node {node} {op} failed: {message}")]
    Node {
        /// Node id (machine name).
        node: String,
        /// The call.
        op: &'static str,
        /// Why.
        message: String,
    },
    /// The fleet refused an operation in the machine's current state.
    #[error("machine {machine} is {state}: cannot {op}")]
    BadState {
        /// Machine name.
        machine: String,
        /// Its state.
        state: String,
        /// What was asked.
        op: &'static str,
    },
    /// Local file IO failed.
    #[error("{context}: {source}")]
    Io {
        /// What was being done.
        context: String,
        /// The error.
        #[source]
        source: std::io::Error,
    },
    /// The local aura binary needs a newer glibc than the machines have,
    /// so it would not start there. Found before any builder is made.
    #[error(
        "the aura binary at {} needs glibc {needs} but cloud machines have {has}; \
         build one with `scripts/build-cloud-binary.sh` (it installs to {}) \
         or set AURA_CLOUD_BINARY",
        path.display(),
        install_to.display()
    )]
    IncompatibleBinary {
        /// The binary that would have been uploaded.
        path: PathBuf,
        /// The newest glibc it references.
        needs: GlibcVersion,
        /// The machines' glibc.
        has: GlibcVersion,
        /// Where the hub looks for a cloud build of aura.
        install_to: PathBuf,
    },
    /// The uploaded aura binary failed to start on a builder machine.
    #[error(
        "uploaded aura binary does not run on the cloud image (builder {machine}, exit {exit_code}): {stderr}"
    )]
    BinaryDoesNotRun {
        /// The builder it was tried on.
        machine: String,
        /// Its exit code.
        exit_code: i32,
        /// The end of its stderr (e.g. the loader's complaint).
        stderr: String,
    },
}

impl CloudError {
    /// Map a gRPC status for `op`.
    #[must_use]
    pub fn rpc(op: &'static str, status: &tonic::Status) -> Self {
        Self::Rpc {
            op,
            code: format!("{:?}", status.code()),
            message: status.message().to_string(),
        }
    }

    /// Whether this is a gRPC `NOT_FOUND`.
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound { .. })
            || matches!(self, Self::Rpc { code, .. } if code == "NotFound")
    }

    /// Whether this is a gRPC `PERMISSION_DENIED` (e.g. usage calls with
    /// a key that is not an org admin's).
    #[must_use]
    pub fn is_permission_denied(&self) -> bool {
        matches!(self, Self::Rpc { code, .. } if code == "PermissionDenied")
    }

    /// Whether boxd refused a machine size as above the org's quota.
    #[must_use]
    pub fn is_size_refused(&self) -> bool {
        match self {
            Self::Rpc { code, message, .. } if code == "InvalidArgument" => {
                let message = message.to_ascii_lowercase();
                ["quota", "size", "vcpu", "memory", "ceiling"]
                    .iter()
                    .any(|word| message.contains(word))
            }
            _ => false,
        }
    }

    /// An IO error with context.
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}
