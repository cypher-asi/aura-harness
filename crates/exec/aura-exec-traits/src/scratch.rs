//! A run's private folder for the temp files, caches and build output of
//! the commands it runs.
//!
//! Commands share whatever the environment points them at: `/tmp`, the
//! user's caches, a configured Cargo `target-dir`. Two runs meant to be
//! independent (the arms of a comparison) would read each other's files
//! through those. A [`RunScratch`] gives a run its own folder instead:
//! its commands get variables naming folders beneath it (see
//! [`RunScratch::vars`]), and confined commands may write beneath it in
//! place of the shared folders.
//!
//! Maven is not redirected: its local repository is only settable through
//! `-Dmaven.repo.local`, which would have to go in `MAVEN_OPTS` or
//! `MAVEN_ARGS` and so replace whatever the user keeps there. A confined
//! Maven build in a run with a scratch can't write `~/.m2`, so it works
//! only from an already populated repository.

use std::io;
use std::path::{Path, PathBuf};

/// Subfolder of the scratch folder each variable names.
const VAR_SUBDIRS: &[(&str, &str)] = &[
    ("TMPDIR", "tmp"),
    ("TMP", "tmp"),
    ("TEMP", "tmp"),
    ("XDG_CACHE_HOME", "cache"),
    ("npm_config_cache", "npm"),
    // pnpm's content-addressed store, which defaults to `~/.pnpm-store`
    // or `~/.local/share/pnpm`.
    ("npm_config_store_dir", "pnpm"),
    ("YARN_CACHE_FOLDER", "yarn"),
    ("BUN_INSTALL_CACHE_DIR", "bun"),
    ("PIP_CACHE_DIR", "pip"),
    ("GOCACHE", "go/build"),
    // Go writes its module cache read-only; see [`RunScratch::remove`].
    ("GOMODCACHE", "go/mod"),
    ("GRADLE_USER_HOME", "gradle"),
    // Overrides any `[build] target-dir` in the Cargo configs, which a
    // copy of a repository inherits and would otherwise share.
    ("CARGO_TARGET_DIR", "target"),
];

/// One run's private folder for command temp files, caches and builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunScratch {
    dir: PathBuf,
}

impl RunScratch {
    /// Scratch beneath `dir`, which [`Self::create`] makes.
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The folder everything goes beneath.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Variables to set on every command the run starts, each naming a
    /// folder beneath [`Self::dir`].
    #[must_use]
    pub fn vars(&self) -> Vec<(&'static str, PathBuf)> {
        VAR_SUBDIRS
            .iter()
            .map(|(var, sub)| (*var, self.dir.join(sub)))
            .collect()
    }

    /// Make the folder and every folder [`Self::vars`] names.
    ///
    /// # Errors
    ///
    /// The first folder that can't be created, with its path.
    pub fn create(&self) -> io::Result<()> {
        for (_, dir) in self.vars() {
            std::fs::create_dir_all(&dir).map_err(|e| {
                io::Error::new(e.kind(), format!("creating {}: {e}", dir.display()))
            })?;
        }
        Ok(())
    }

    /// Remove the folder and everything in it. Go leaves its module cache
    /// read-only, which a plain recursive removal can't empty, so when
    /// that is refused the folders beneath are made writable by their
    /// owner and the removal tried again.
    ///
    /// # Errors
    ///
    /// What the removal, or making the folders writable, failed with.
    pub fn remove(&self) -> io::Result<()> {
        match std::fs::remove_dir_all(&self.dir) {
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                make_folders_writable(&self.dir)?;
                std::fs::remove_dir_all(&self.dir)
            }
            other => other,
        }
    }
}

/// Give the owner write access to `dir` and every folder beneath it,
/// without following links.
#[cfg(unix)]
fn make_folders_writable(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::symlink_metadata(dir)?;
    if !metadata.is_dir() {
        return Ok(());
    }
    let mut permissions = metadata.permissions();
    permissions.set_mode(permissions.mode() | 0o700);
    std::fs::set_permissions(dir, permissions)?;
    for entry in std::fs::read_dir(dir)? {
        make_folders_writable(&entry?.path())?;
    }
    Ok(())
}

/// Elsewhere a read-only folder doesn't stop its entries being removed.
#[cfg(not(unix))]
fn make_folders_writable(_dir: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_var_names_a_folder_beneath_the_scratch_and_create_makes_them() {
        let root = tempfile::tempdir().unwrap();
        let scratch = RunScratch::new(root.path().join("a-1.tmp"));
        assert!(!scratch.dir().exists());
        scratch.create().unwrap();
        let vars = scratch.vars();
        for var in [
            "TMPDIR",
            "TMP",
            "TEMP",
            "XDG_CACHE_HOME",
            "npm_config_cache",
            "npm_config_store_dir",
            "YARN_CACHE_FOLDER",
            "BUN_INSTALL_CACHE_DIR",
            "PIP_CACHE_DIR",
            "GOCACHE",
            "GOMODCACHE",
            "GRADLE_USER_HOME",
            "CARGO_TARGET_DIR",
        ] {
            assert_eq!(
                vars.iter().filter(|(name, _)| *name == var).count(),
                1,
                "{var}"
            );
        }
        for (_, dir) in vars {
            assert!(
                dir.starts_with(scratch.dir()) && dir != scratch.dir(),
                "{dir:?}"
            );
            assert!(dir.is_dir(), "{dir:?}");
        }
        for sub in [
            "tmp", "cache", "npm", "pnpm", "yarn", "bun", "pip", "go/build", "go/mod", "gradle",
            "target",
        ] {
            assert!(scratch.dir().join(sub).is_dir(), "{sub}");
        }
    }

    #[test]
    fn the_caches_confinement_takes_away_are_redirected_into_the_scratch() {
        let scratch = RunScratch::new(PathBuf::from("/runs/a-1.tmp"));
        let vars = scratch.vars();
        let named = |var: &str| {
            vars.iter()
                .find(|(name, _)| *name == var)
                .map(|(_, dir)| dir.clone())
        };
        for (var, sub) in [
            ("GOMODCACHE", "go/mod"),
            ("GRADLE_USER_HOME", "gradle"),
            ("YARN_CACHE_FOLDER", "yarn"),
            ("BUN_INSTALL_CACHE_DIR", "bun"),
            ("npm_config_store_dir", "pnpm"),
        ] {
            assert_eq!(named(var), Some(scratch.dir().join(sub)), "{var}");
        }
        assert_ne!(named("GOCACHE"), named("GOMODCACHE"));
    }

    #[test]
    fn create_is_idempotent_and_reports_the_folder_it_could_not_make() {
        let root = tempfile::tempdir().unwrap();
        let scratch = RunScratch::new(root.path().join("b-1.tmp"));
        scratch.create().unwrap();
        scratch.create().unwrap();

        let blocked = root.path().join("file");
        std::fs::write(&blocked, "not a folder").unwrap();
        let error = RunScratch::new(blocked.clone()).create().unwrap_err();
        assert!(
            error.to_string().contains(&blocked.display().to_string()),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn remove_empties_read_only_folders_like_the_go_module_cache() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let scratch = RunScratch::new(root.path().join("c-1.tmp"));
        scratch.create().unwrap();
        let module = scratch.dir().join("go/mod/example.com/m@v1.0.0");
        std::fs::create_dir_all(&module).unwrap();
        std::fs::write(module.join("go.mod"), "module m\n").unwrap();
        for dir in [module.as_path(), module.parent().unwrap()] {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        }

        scratch.remove().unwrap();

        assert!(!scratch.dir().exists());
    }
}
