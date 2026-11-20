//! The repositories example tasks set in an existing codebase start from: a
//! public GitHub repository at a commit, downloaded once as an archive into
//! a cache and copied into each run's workspace from there.

use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A public GitHub repository at a commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSource {
    /// `https://github.com/<owner>/<name>`
    pub repo: String,
    /// Full commit hash
    pub commit: String,
}

impl RepoSource {
    /// Its owner and name, when `repo` is a GitHub repository and `commit`
    /// a full hash; both end up in a path and a URL.
    fn parts(&self) -> io::Result<(&str, &str)> {
        let bad = |what: &str| io::Error::other(format!("{what} in {self:?}"));
        let path = self
            .repo
            .strip_prefix("https://github.com/")
            .ok_or_else(|| bad("not a GitHub repository"))?;
        let (owner, name) = path
            .trim_end_matches('/')
            .split_once('/')
            .ok_or_else(|| bad("no repository name"))?;
        let plain = |part: &str| {
            !part.is_empty()
                && !part.starts_with('.')
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        };
        if !plain(owner) || !plain(name) {
            return Err(bad("an unexpected repository name"));
        }
        if self.commit.len() != 40 || !self.commit.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(bad("not a full commit hash"));
        }
        Ok((owner, name))
    }

    /// Where its files are kept under `cache`.
    ///
    /// # Errors
    /// When it is not a GitHub repository at a full commit hash.
    pub fn dir(&self, cache: &Path) -> io::Result<PathBuf> {
        let (owner, name) = self.parts()?;
        Ok(cache.join(format!("{owner}-{name}-{}", self.commit)))
    }

    /// Its files under `cache`, downloaded first when they are not there.
    ///
    /// # Errors
    /// When it is not a GitHub repository at a full commit hash, or the
    /// download or unpacking failed.
    pub async fn fetch(&self, cache: &Path) -> io::Result<PathBuf> {
        let dir = self.dir(cache)?;
        if dir.is_dir() {
            return Ok(dir);
        }
        let (owner, name) = self.parts()?;
        let url = format!(
            "https://codeload.github.com/{owner}/{name}/tar.gz/{}",
            self.commit
        );
        let download = |e: reqwest::Error| io::Error::other(format!("downloading {url}: {e}"));
        let archive = reqwest::get(&url)
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(download)?
            .bytes()
            .await
            .map_err(download)?;
        let target = dir.clone();
        tokio::task::spawn_blocking(move || unpack(&archive, &target))
            .await
            .map_err(io::Error::other)??;
        Ok(dir)
    }
}

/// Where downloaded repositories are kept: `AURA_HOME/evals/sources`, else
/// a folder in the system's temporary directory.
#[must_use]
pub fn cache_dir() -> PathBuf {
    aura_config::AuraHome::resolve().map_or_else(
        |_| std::env::temp_dir().join("aura-evals-sources"),
        |home| in_home(&home.path),
    )
}

/// Where downloaded repositories are kept under the aura home `home`.
#[must_use]
pub fn in_home(home: &Path) -> PathBuf {
    home.join("evals").join("sources")
}

/// Unpack a GitHub archive (`.tar.gz`, everything under one top folder)
/// into `dir`, keeping only files and folders. It is unpacked next to
/// `dir` first and moved there at the end, so a half-written copy is never
/// taken for a whole one.
fn unpack(archive: &[u8], dir: &Path) -> io::Result<()> {
    let name = dir
        .file_name()
        .ok_or_else(|| io::Error::other(format!("{} has no name", dir.display())))?;
    let partial = dir.with_file_name(format!("{}.partial", name.to_string_lossy()));
    if partial.exists() {
        std::fs::remove_dir_all(&partial)?;
    }
    std::fs::create_dir_all(&partial)?;
    let mut entries = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in entries.entries()? {
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            continue;
        }
        let inner: PathBuf = entry.path()?.components().skip(1).collect();
        if inner.as_os_str().is_empty()
            || inner
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            continue;
        }
        let target = partial.join(&inner);
        if kind.is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        entry.unpack(&target)?;
    }
    match std::fs::rename(&partial, dir) {
        Err(_) if dir.is_dir() => std::fs::remove_dir_all(&partial),
        moved => moved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(repo: &str, commit: &str) -> RepoSource {
        RepoSource {
            repo: repo.to_string(),
            commit: commit.to_string(),
        }
    }

    const COMMIT: &str = "670dd01c53e7d4a9beb426e76a4a982010f07af7";

    #[test]
    fn only_a_github_repository_at_a_full_commit_is_taken() {
        let cache = Path::new("/cache");
        assert_eq!(
            source("https://github.com/tobymao/sqlglot", COMMIT)
                .dir(cache)
                .unwrap(),
            cache.join(format!("tobymao-sqlglot-{COMMIT}"))
        );
        for (repo, commit) in [
            ("https://gitlab.com/a/b", COMMIT),
            ("https://github.com/a", COMMIT),
            ("https://github.com/../b", COMMIT),
            ("https://github.com/a/b/../../c", COMMIT),
            ("https://github.com/a/b", "main"),
            ("https://github.com/a/b", &COMMIT[..12]),
        ] {
            assert!(source(repo, commit).dir(cache).is_err(), "{repo} {commit}");
        }
    }

    fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (path, bytes) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, *bytes).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn an_archive_is_unpacked_without_its_top_folder() {
        let cache = tempfile::tempdir().unwrap();
        let dir = cache.path().join("a-b-c");
        let bytes = archive(&[("b-c/README.md", b"hello"), ("b-c/src/lib.py", b"x = 1\n")]);
        unpack(&bytes, &dir).unwrap();
        assert_eq!(std::fs::read(dir.join("README.md")).unwrap(), b"hello");
        assert_eq!(std::fs::read(dir.join("src/lib.py")).unwrap(), b"x = 1\n");
        assert!(!cache.path().join("a-b-c.partial").exists());

        unpack(&archive(&[("b-c/other", b"")]), &dir).unwrap();
        assert!(dir.join("README.md").is_file(), "the first copy stays");
        assert!(!cache.path().join("a-b-c.partial").exists());
    }
}
