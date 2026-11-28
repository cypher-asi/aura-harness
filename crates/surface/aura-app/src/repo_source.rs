//! The repositories example tasks set in an existing codebase start from,
//! fetched once into a cache and copied into each run's workspace from
//! there: a public GitHub repository at a commit, downloaded as an archive,
//! or the repository in a container image, for tasks whose dependencies and
//! tests live in that image (SWE-Bench Pro).

use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use aura_core_types::ErrorChain;
use serde::{Deserialize, Serialize};

/// Where a task's fixture starts from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Source {
    /// A public GitHub repository at a commit
    Repo(RepoSource),
    /// The repository in a container image
    Image(ImageSource),
}

/// A public GitHub repository at a commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSource {
    /// `https://github.com/<owner>/<name>`
    pub repo: String,
    /// Full commit hash
    pub commit: String,
}

/// The git repository at `/app` (else `/testbed`) of a container image;
/// its tracked files are what a run starts from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageSource {
    /// Image reference, as `ghcr.io/<owner>/<name>:<tag>`
    pub image: String,
}

impl Source {
    /// Where its files are kept under `cache`.
    ///
    /// # Errors
    /// When it names something that can't be fetched safely.
    pub fn dir(&self, cache: &Path) -> io::Result<PathBuf> {
        match self {
            Self::Repo(repo) => repo.dir(cache),
            Self::Image(image) => image.dir(cache),
        }
    }

    /// Its files under `cache`, fetched first when they are not there.
    ///
    /// # Errors
    /// When it names something that can't be fetched safely, or fetching
    /// or unpacking failed.
    pub async fn fetch(&self, cache: &Path) -> io::Result<PathBuf> {
        match self {
            Self::Repo(repo) => repo.fetch(cache).await,
            Self::Image(image) => image.fetch(cache).await,
        }
    }

    /// Whether fetching it pulls a container image: its files are not
    /// under `cache` and Docker does not have its image. Runs Docker.
    #[must_use]
    pub fn pulls(&self, cache: &Path) -> bool {
        let Self::Image(image) = self else {
            return false;
        };
        image.dir(cache).is_ok_and(|dir| !dir.is_dir())
            && !Command::new("docker")
                .args(["image", "inspect", "--format", "{{.Id}}", &image.image])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
    }
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
        let download =
            |e: reqwest::Error| io::Error::other(format!("downloading {url}: {}", ErrorChain(&e)));
        let archive = reqwest::get(&url)
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(download)?
            .bytes()
            .await
            .map_err(download)?;
        let target = dir.clone();
        tokio::task::spawn_blocking(move || {
            unpack(flate2::read::GzDecoder::new(&archive[..]), &target, 1)
        })
        .await
        .map_err(io::Error::other)??;
        Ok(dir)
    }
}

/// Lists the tracked files of the image's repository as a tar archive.
const TRACKED_FILES: &str = "cd /app 2>/dev/null || cd /testbed || exit 1; \
     git ls-files -z | tar --null --no-recursion -T - -cf -";

impl ImageSource {
    /// Where its files are kept under `cache`, when `image` is a plain
    /// image reference (it ends up in a path and a command line).
    ///
    /// # Errors
    /// When `image` is not a plain image reference.
    pub fn dir(&self, cache: &Path) -> io::Result<PathBuf> {
        let plain = !self.image.is_empty()
            && self.image.len() <= 255
            && self
                .image
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric())
            && self.image.chars().all(|c| {
                c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | '@')
            })
            && !self.image.contains("..");
        if !plain {
            return Err(io::Error::other(format!(
                "not a plain image reference in {self:?}"
            )));
        }
        let name: String = self
            .image
            .chars()
            .map(|c| if matches!(c, '/' | ':' | '@') { '-' } else { c })
            .collect();
        Ok(cache.join(format!("image-{name}")))
    }

    /// Its repository's tracked files under `cache`, taken from the image
    /// (pulled first when Docker does not have it) when they are not there.
    ///
    /// # Errors
    /// When `image` is not a plain image reference, Docker is missing, or
    /// pulling or copying failed.
    pub async fn fetch(&self, cache: &Path) -> io::Result<PathBuf> {
        let dir = self.dir(cache)?;
        if dir.is_dir() {
            return Ok(dir);
        }
        let (image, target) = (self.image.clone(), dir.clone());
        tokio::task::spawn_blocking(move || copy_out(&image, &target))
            .await
            .map_err(io::Error::other)??;
        Ok(dir)
    }
}

/// Pull `image` unless Docker has it, then unpack its repository's tracked
/// files into `dir`.
fn copy_out(image: &str, dir: &Path) -> io::Result<()> {
    let docker = |args: &[&str]| {
        Command::new("docker").args(args).output().map_err(|e| {
            io::Error::other(format!(
                "running docker: {e} (tasks that run in a container image need Docker)"
            ))
        })
    };
    if !docker(&["image", "inspect", "--format", "{{.Id}}", image])?
        .status
        .success()
    {
        let pulled = docker(&["pull", "--quiet", image])?;
        if !pulled.status.success() {
            return Err(io::Error::other(format!(
                "docker pull {image}: {}",
                String::from_utf8_lossy(&pulled.stderr).trim()
            )));
        }
    }
    let mut child = Command::new("docker")
        .args([
            "run",
            "--rm",
            "--network",
            "none",
            "--entrypoint",
            "/bin/sh",
        ])
        .args([image, "-c", TRACKED_FILES])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let files = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("docker run gave no output"))?;
    let unpacked = unpack(files, dir, 0);
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "copying the repository out of {image}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    unpacked
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

/// Unpack a tar archive into `dir`, dropping the first `strip` folders of
/// every path and keeping only files and folders. It is unpacked next to
/// `dir` first and moved there at the end, so a half-written copy is never
/// taken for a whole one.
fn unpack(archive: impl Read, dir: &Path, strip: usize) -> io::Result<()> {
    let name = dir
        .file_name()
        .ok_or_else(|| io::Error::other(format!("{} has no name", dir.display())))?;
    let partial = dir.with_file_name(format!("{}.partial", name.to_string_lossy()));
    if partial.exists() {
        std::fs::remove_dir_all(&partial)?;
    }
    std::fs::create_dir_all(&partial)?;
    let mut archive = tar::Archive::new(archive);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            continue;
        }
        let inner: PathBuf = entry.path()?.components().skip(strip).collect();
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
    // The archive's end marker can come before the end of the stream (tar
    // pads to whole records); a writer cut off there fails with a broken pipe.
    io::copy(&mut archive.into_inner(), &mut io::sink())?;
    match std::fs::rename(&partial, dir) {
        Err(_) if dir.is_dir() => std::fs::remove_dir_all(&partial),
        moved => moved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(repo: &str, commit: &str) -> Source {
        Source::Repo(RepoSource {
            repo: repo.to_string(),
            commit: commit.to_string(),
        })
    }

    fn image(image: &str) -> Source {
        Source::Image(ImageSource {
            image: image.to_string(),
        })
    }

    const COMMIT: &str = "670dd01c53e7d4a9beb426e76a4a982010f07af7";

    #[test]
    fn only_a_github_repository_at_a_full_commit_is_taken() {
        let cache = Path::new("/cache");
        assert_eq!(
            repo("https://github.com/tobymao/sqlglot", COMMIT)
                .dir(cache)
                .unwrap(),
            cache.join(format!("tobymao-sqlglot-{COMMIT}"))
        );
        for (name, commit) in [
            ("https://gitlab.com/a/b", COMMIT),
            ("https://github.com/a", COMMIT),
            ("https://github.com/../b", COMMIT),
            ("https://github.com/a/b/../../c", COMMIT),
            ("https://github.com/a/b", "main"),
            ("https://github.com/a/b", &COMMIT[..12]),
        ] {
            assert!(repo(name, commit).dir(cache).is_err(), "{name} {commit}");
        }
    }

    #[test]
    fn only_a_plain_image_reference_is_taken() {
        let cache = Path::new("/cache");
        assert_eq!(
            image("ghcr.io/scaleapi/swe-bench_pro-v2:instance_a__b-1")
                .dir(cache)
                .unwrap(),
            cache.join("image-ghcr.io-scaleapi-swe-bench_pro-v2-instance_a__b-1")
        );
        for name in ["", "-v", "a b", "a;rm", "../x", "a/../../b", "$(x)"] {
            assert!(image(name).dir(cache).is_err(), "{name}");
        }
    }

    #[test]
    fn a_task_names_a_repository_or_an_image() {
        let parse = |text: &str| toml::from_str::<Source>(text).unwrap();
        assert_eq!(
            parse(&format!(
                "repo = \"https://github.com/a/b\"\ncommit = \"{COMMIT}\""
            )),
            repo("https://github.com/a/b", COMMIT)
        );
        assert_eq!(parse("image = \"ghcr.io/a/b:c\""), image("ghcr.io/a/b:c"));
    }

    fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, bytes) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, *bytes).unwrap();
        }
        builder.into_inner().unwrap()
    }

    #[test]
    fn an_archive_is_unpacked_without_its_top_folder() {
        let cache = tempfile::tempdir().unwrap();
        let dir = cache.path().join("a-b-c");
        let bytes = archive(&[("b-c/README.md", b"hello"), ("b-c/src/lib.py", b"x = 1\n")]);
        unpack(&bytes[..], &dir, 1).unwrap();
        assert_eq!(std::fs::read(dir.join("README.md")).unwrap(), b"hello");
        assert_eq!(std::fs::read(dir.join("src/lib.py")).unwrap(), b"x = 1\n");
        assert!(!cache.path().join("a-b-c.partial").exists());

        unpack(&archive(&[("b-c/other", b"")])[..], &dir, 1).unwrap();
        assert!(dir.join("README.md").is_file(), "the first copy stays");
        assert!(!cache.path().join("a-b-c.partial").exists());

        let flat = cache.path().join("flat");
        unpack(&archive(&[("app.js", b"1")])[..], &flat, 0).unwrap();
        assert!(flat.join("app.js").is_file());
    }

    #[test]
    fn the_padding_after_the_end_marker_is_read_too() {
        let cache = tempfile::tempdir().unwrap();
        let mut bytes = archive(&[("app.js", b"1")]);
        bytes.extend_from_slice(&[0; 9 * 512]);
        let mut stream = &bytes[..];
        unpack(&mut stream, &cache.path().join("padded"), 0).unwrap();
        assert!(stream.is_empty(), "{} bytes left unread", stream.len());
    }
}
