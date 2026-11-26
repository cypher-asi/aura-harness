//! The aura binary a base snapshot gets: where the hub finds a build made
//! for the machines, and whether that build can start there.

use std::path::{Path, PathBuf};

use crate::error::{CloudError, CloudResult};
use crate::glibc::{self, GlibcVersion, MACHINE_GLIBC};

/// The first 16 hex characters of `path`'s blake3 hash. Blocking.
///
/// # Errors
///
/// When the file can't be read.
pub fn binary_digest(path: &Path) -> CloudResult<String> {
    let mut file = std::fs::File::open(path)
        .map_err(|err| CloudError::io(format!("open {}", path.display()), err))?;
    let mut hasher = blake3::Hasher::new();
    std::io::copy(&mut file, &mut hasher)
        .map_err(|err| CloudError::io(format!("read {}", path.display()), err))?;
    Ok(hasher.finalize().to_hex()[..16].to_string())
}

/// Where the hub looks for a Linux build of aura made for the machines
/// (`scripts/build-cloud-binary.sh` installs there), under the cloud
/// home (`AURA_HOME/cloud`).
#[must_use]
pub fn cloud_binary_path(home: &Path) -> PathBuf {
    home.join("bin").join("aura")
}

/// Run on a builder right after the binary is installed: fails, with
/// the loader's complaint on stderr, when the binary can't start there.
/// `--help` is handled by argument parsing and exits 0 without touching
/// the machine.
pub const CHECK_BINARY: &str = "/usr/local/bin/aura --help >/dev/null";

/// The aura binary to upload, read once for its digest, its glibc needs
/// and the upload itself.
pub(super) struct LocalBinary {
    path: PathBuf,
    pub(super) bytes: Vec<u8>,
    pub(super) digest: String,
    glibc: Option<GlibcVersion>,
}

impl LocalBinary {
    /// Read and inspect `path`. Blocking.
    pub(super) fn read(path: PathBuf) -> CloudResult<Self> {
        let bytes = std::fs::read(&path)
            .map_err(|err| CloudError::io(format!("read aura binary {}", path.display()), err))?;
        let digest = blake3::hash(&bytes).to_hex()[..16].to_string();
        let glibc = glibc::required_glibc(&bytes);
        Ok(Self {
            path,
            bytes,
            digest,
            glibc,
        })
    }

    /// [`CloudError::IncompatibleBinary`] when the machines' glibc is
    /// too old for it.
    pub(super) fn ensure_loads_on_machines(&self, home: &Path) -> CloudResult<()> {
        match self.glibc {
            Some(needs) if needs > MACHINE_GLIBC => Err(CloudError::IncompatibleBinary {
                path: self.path.clone(),
                needs,
                has: MACHINE_GLIBC,
                install_to: cloud_binary_path(home),
            }),
            needs => {
                tracing::debug!(
                    path = %self.path.display(),
                    needs = ?needs.map(|version| version.to_string()),
                    has = %MACHINE_GLIBC,
                    "aura binary's glibc fits the cloud image"
                );
                Ok(())
            }
        }
    }
}
