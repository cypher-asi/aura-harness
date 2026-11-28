//! The shell that keeps a cloud machine's workspace a real clone of its
//! project, each agent on a branch of its own.
//!
//! - **Snapshot build** ([`clone_snapshot`]): the builder clones a bundle
//!   of the hub's history at HEAD ([`crate::delta::bundle_head`]) and
//!   checks that commit out, detached.
//! - **A new agent** ([`start_branch`]): its machine fetches the commits
//!   the hub made since the snapshot, checks out `aura/<name>-<code>`
//!   ([`agent_branch`]) at the hub's HEAD, unpacks the hub's uncommitted
//!   changes there (left uncommitted) and marks what the hub sent as
//!   [`BASE_REF`].
//! - **A fork** ([`fork_branch`]) keeps its parent's disk and moves to a
//!   branch of its own.
//! - **Latest code** ([`pull_latest`]): either way, the machine then
//!   pulls `origin`'s merge queue target (main unless the repo's committed
//!   `.aura/merge-queue.toml` names another) from GitHub into the agent's
//!   branch, so it starts on the code the merge queue landed, not only on
//!   what the hub had.
//! - **Pull** ([`mark_pulled`]) moves [`BASE_REF`] to the tree the hub
//!   took, and [`bundle_branch`] packs the branch's commits the hub
//!   doesn't have.
//!
//! ## Invariants
//!
//! - `refs/aura/*` is never on a branch and never pushed: git tools push
//!   branches only.
//! - Nothing here commits on the agent's branch or touches its index; the
//!   base is written from a scratch index.
//! - Every value put into a script is checked first: commit and tree ids
//!   are hex, branch names are plain git branch names, remote URLs are
//!   GitHub https URLs.

use crate::error::{CloudError, CloudResult};
use crate::names;

mod pull;
pub use pull::{parse_pulled, pull_latest, start_from_latest, PULLED, PULL_BRANCH};

/// Where a snapshot build's bundle is uploaded.
pub const SNAPSHOT_BUNDLE: &str = "/tmp/aura-project.bundle";
/// Where a new machine's bundle of the hub's newer commits is uploaded.
pub const HEAD_BUNDLE: &str = "/tmp/aura-head.bundle";
/// Where a new machine's uncommitted changes are uploaded.
pub const DELTA_ARCHIVE: &str = "/tmp/aura-delta.tgz";
/// Where a machine writes the bundle of its branch for the hub.
pub const OUT_BUNDLE: &str = "/tmp/aura-out.bundle";
/// What the hub last had of the machine's working tree; patches are
/// taken against it.
pub const BASE_REF: &str = "refs/aura/base";
/// The hub's HEAD when the agent was placed: what the hub has of the
/// agent's branch.
pub const HUB_HEAD_REF: &str = "refs/aura/hub-head";
/// Joins the project snapshot's fingerprint, so snapshots of an older
/// workspace layout are rebuilt.
pub const FORMAT: &str = "git-bundle-1";
/// Prefix of [`bundle_branch`]'s answer line.
const BUNDLED: &str = "aura-bundle: ";
/// Identity of the base commits, which are on no branch.
const BASE_IDENT: &str = "-c user.name=aura -c user.email=aura@localhost";

/// Whether `id` is a full commit or tree id (SHA-1 or SHA-256).
#[must_use]
pub fn is_object_id(id: &str) -> bool {
    matches!(id.len(), 40 | 64) && id.chars().all(|c| c.is_ascii_hexdigit())
}

/// Whether `name` is a branch name safe to put in a script and accepted
/// by git (`git check-ref-format --branch`, minus anything a shell or a
/// revision expression would read).
#[must_use]
pub fn is_branch_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with(['-', '/', '.'])
        && !name.ends_with(['/', '.'])
        && !name.ends_with(".lock")
        && !name.contains("..")
        && !name.contains("//")
        && !name.contains("@{")
        && !name.contains("/.")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '.'))
}

/// Whether `url` is a remote URL [`start_branch`] sets as `origin`.
fn is_remote_url(url: &str) -> bool {
    url.strip_prefix("https://github.com/").is_some_and(|rest| {
        !rest.is_empty()
            && !rest.contains("..")
            && rest
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '.'))
    })
}

fn bad(what: &str, value: &str) -> CloudError {
    CloudError::InvalidResponse {
        op: "workspace script",
        message: format!("refusing {what} {value:?}"),
    }
}

fn object_id(id: &str) -> CloudResult<&str> {
    if is_object_id(id) {
        Ok(id)
    } else {
        Err(bad("object id", id))
    }
}

fn branch(name: &str) -> CloudResult<&str> {
    if is_branch_name(name) {
        Ok(name)
    } else {
        Err(bad("branch", name))
    }
}

/// The branch an agent works on: `aura/<name>-<code>`, from its name and
/// code (the first four hex characters of its id without one).
#[must_use]
pub fn agent_branch(name: &str, code: Option<&str>, agent_hex: &str) -> String {
    let mut name: String = names::dns_fragment(name).chars().take(40).collect();
    while name.ends_with('-') {
        name.pop();
    }
    if name.is_empty() {
        name = "agent".into();
    }
    let code = code
        .map(names::dns_fragment)
        .filter(|code| !code.is_empty())
        .unwrap_or_else(|| agent_hex.chars().take(4).collect::<String>().to_lowercase());
    format!("aura/{name}-{code}")
}

/// Shell that writes the working tree as a commit on no branch (from a
/// scratch copy of the index) and points [`BASE_REF`] at it.
fn mark_base(message: &str) -> String {
    format!(
        "gd=$(git rev-parse --git-dir); idx=\"$gd/aura-base.index\"; rm -f \"$idx\"; \
         if [ -f \"$gd/index\" ]; then cp \"$gd/index\" \"$idx\"; fi; \
         GIT_INDEX_FILE=\"$idx\" git add -A; \
         tree=$(GIT_INDEX_FILE=\"$idx\" git write-tree); rm -f \"$idx\"; \
         git update-ref {BASE_REF} \"$(git {BASE_IDENT} commit-tree \"$tree\" -p HEAD -m '{message}')\""
    )
}

/// Shell run in a snapshot builder: clone the bundle uploaded to
/// [`SNAPSHOT_BUNDLE`] into the workspace and check `commit` out,
/// detached. `upstream` (`refs/remotes/origin/<branch>`, when the bundle
/// carries it) is kept, so pushes later know what `origin` has.
///
/// # Errors
///
/// When `commit` or `upstream` is not safe to run.
pub fn clone_snapshot(commit: &str, upstream: Option<&str>) -> CloudResult<String> {
    let commit = object_id(commit)?;
    let upstream = match upstream {
        Some(name) => {
            let short = name
                .strip_prefix("refs/remotes/origin/")
                .ok_or_else(|| bad("upstream", name))?;
            branch(short)?;
            format!("git fetch -q {SNAPSHOT_BUNDLE} '+{name}:{name}'; ")
        }
        None => String::new(),
    };
    Ok(format!(
        "set -e; mkdir -p \"$AURA_WORKSPACE\"; cd \"$AURA_WORKSPACE\"; \
         git clone -q --no-checkout {SNAPSHOT_BUNDLE} .; git remote remove origin; \
         git checkout -q --detach {commit}; {upstream}\
         git update-ref {HUB_HEAD_REF} {commit}; rm -f {SNAPSHOT_BUNDLE}"
    ))
}

/// What a new agent's machine gets on top of its snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BranchStart<'a> {
    /// The agent's branch.
    pub branch: &'a str,
    /// The hub's HEAD, where the branch starts.
    pub head: &'a str,
    /// The merge queue's target, pulled from `origin` on top.
    pub target: &'a str,
    /// `origin` from now on; `None` leaves the machine without one.
    pub remote_url: Option<&'a str>,
    /// Whether [`HEAD_BUNDLE`] was uploaded (the hub has commits the
    /// snapshot lacks).
    pub bundle: bool,
    /// Whether [`DELTA_ARCHIVE`] was uploaded (the hub has uncommitted
    /// changes).
    pub delta: bool,
    /// Start the branch from `origin`'s latest `target` instead of
    /// the hub's HEAD, leaving out the hub's unpushed commits and its
    /// uncommitted changes ([`start_from_latest`]). Without `origin`, or
    /// when it can't be fetched, the branch starts at the hub's HEAD,
    /// still without the uncommitted changes.
    pub clean: bool,
}

/// Shell run in a new agent's machine: fetch the hub's newer commits,
/// check the agent's branch out at the hub's HEAD, unpack the hub's
/// uncommitted changes (left uncommitted), pull the latest
/// [`BranchStart::target`] from GitHub on top ([`pull_latest`]) and mark it all
/// as [`BASE_REF`].
///
/// # Errors
///
/// When a value is not safe to run.
pub fn start_branch(start: &BranchStart<'_>) -> CloudResult<String> {
    let head = object_id(start.head)?;
    let name = branch(start.branch)?;
    let fetch = if start.bundle {
        format!("git fetch -q {HEAD_BUNDLE} '+HEAD:{HUB_HEAD_REF}'; rm -f {HEAD_BUNDLE}; ")
    } else {
        String::new()
    };
    let origin = match start.remote_url {
        Some(url) if is_remote_url(url) => format!(
            "if git remote get-url origin >/dev/null 2>&1; then git remote set-url origin '{url}'; \
             else git remote add origin '{url}'; fi; "
        ),
        Some(url) => return Err(bad("remote", url)),
        None => "git remote remove origin >/dev/null 2>&1 || true; ".to_string(),
    };
    let delta = if start.delta && !start.clean {
        format!(
            "tar -xzf {DELTA_ARCHIVE}; \
             if [ -s .aura-deleted ]; then tr '\\n' '\\0' < .aura-deleted | xargs -0 rm -f --; fi; \
             rm -f .aura-deleted {DELTA_ARCHIVE}; "
        )
    } else {
        String::new()
    };
    let pull = match (start.remote_url.is_some(), start.clean) {
        (true, true) => start_from_latest(start.target, name)?,
        (true, false) => pull_latest(start.target)?,
        (false, _) => String::new(),
    };
    Ok(format!(
        "set -e; cd \"$AURA_WORKSPACE\"; {fetch}git update-ref {HUB_HEAD_REF} {head}; \
         git checkout -q -f -B '{name}' {head}; {origin}{delta}{pull}{}",
        mark_base("aura: hub working tree")
    ))
}

/// Shell run in a fork: the parent's disk, on the child's own branch,
/// with the latest `target` (the merge queue's) pulled from GitHub on top
/// ([`pull_latest`]).
///
/// # Errors
///
/// When `name` or `target` is not a safe branch name.
pub fn fork_branch(name: &str, target: &str) -> CloudResult<String> {
    let name = branch(name)?;
    Ok(format!(
        "set -e; cd \"$AURA_WORKSPACE\"; git checkout -q -B '{name}'; {}",
        pull_latest(target)?
    ))
}

/// Shell run after the hub applied a patch taken against `tree` (the
/// machine's working tree then): that tree is the base from now on.
///
/// # Errors
///
/// When `tree` is not an object id.
pub fn mark_pulled(tree: &str) -> CloudResult<String> {
    let tree = object_id(tree)?;
    Ok(format!(
        "set -e; cd \"$AURA_WORKSPACE\"; \
         git update-ref {BASE_REF} \"$(git {BASE_IDENT} commit-tree {tree} -p HEAD -m 'aura: pulled to the hub')\""
    ))
}

/// Shell that bundles `name`'s commits the hub lacks (past
/// [`HUB_HEAD_REF`] and `fetched`, the tip it last took) into
/// [`OUT_BUNDLE`]. Prints one line [`parse_bundled`] reads.
///
/// # Errors
///
/// When a value is not safe to run.
pub fn bundle_branch(name: &str, fetched: Option<&str>) -> CloudResult<String> {
    let name = branch(name)?;
    let fetched = match fetched {
        Some(tip) => {
            let tip = object_id(tip)?;
            format!(
                "if git cat-file -e '{tip}^{{commit}}' 2>/dev/null; then not=\"$not {tip}\"; fi; "
            )
        }
        None => String::new(),
    };
    Ok(format!(
        "set -e; cd \"$AURA_WORKSPACE\"; rm -f {OUT_BUNDLE}; \
         if ! git rev-parse -q --verify 'refs/heads/{name}' >/dev/null || \
         ! git rev-parse -q --verify {HUB_HEAD_REF} >/dev/null; then echo '{BUNDLED}none'; exit 0; fi; \
         not={HUB_HEAD_REF}; {fetched}\
         n=$(git rev-list --count 'refs/heads/{name}' --not $not); \
         if [ \"$n\" -eq 0 ]; then echo '{BUNDLED}none'; exit 0; fi; \
         git bundle create -q {OUT_BUNDLE} 'refs/heads/{name}' --not $not; \
         echo \"{BUNDLED}$n $(git rev-parse 'refs/heads/{name}')\""
    ))
}

/// What [`bundle_branch`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bundled {
    /// No commits the hub lacks.
    Nothing,
    /// `commits` commits up to `tip` are in [`OUT_BUNDLE`].
    Commits {
        /// How many.
        commits: u64,
        /// The branch's tip.
        tip: String,
    },
}

/// Read [`bundle_branch`]'s output; `None` when it printed no answer.
#[must_use]
pub fn parse_bundled(stdout: &str) -> Option<Bundled> {
    let line = stdout
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix(BUNDLED))?;
    if line.trim() == "none" {
        return Some(Bundled::Nothing);
    }
    let (count, tip) = line.trim().split_once(' ')?;
    let commits = count.parse().ok()?;
    is_object_id(tip).then(|| Bundled::Commits {
        commits,
        tip: tip.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "e91dfbe90eb7b51f7b0459ce0c829d439a150396";

    #[test]
    fn branches_are_named_after_the_agent() {
        assert_eq!(
            agent_branch("Reviewer", Some("RE36"), "abcd1234"),
            "aura/reviewer-re36"
        );
        assert_eq!(agent_branch("", None, "ABCD1234"), "aura/agent-abcd");
        assert_eq!(
            agent_branch("Fix the login page!!", Some("FX1"), "00"),
            "aura/fix-the-login-page-fx1"
        );
        let long = agent_branch(&"x".repeat(100), Some("c"), "00");
        assert!(long.len() <= "aura/".len() + 40 + 2, "{long}");
        assert!(is_branch_name(&long));
    }

    #[test]
    fn only_plain_values_reach_a_script() {
        for good in ["aura/reviewer-re36", "main", "feature/x_y.z"] {
            assert!(is_branch_name(good), "{good}");
        }
        for evil in [
            "",
            "-x",
            "a b",
            "a;rm -rf /",
            "a'b",
            "$(id)",
            "a..b",
            "a/",
            "a.lock",
            "a@{1}",
            "/a",
            ".a",
            "a/.b",
            "a\nb",
        ] {
            assert!(!is_branch_name(evil), "{evil:?}");
            assert!(fork_branch(evil, "main").is_err(), "{evil:?}");
        }
        assert!(mark_pulled("HEAD").is_err());
        assert!(mark_pulled(&format!("{SHA}; id")).is_err());
        assert!(clone_snapshot(SHA, Some("refs/heads/main")).is_err());
        assert!(clone_snapshot(SHA, Some("refs/remotes/origin/a'b")).is_err());
        let start = BranchStart {
            branch: "aura/a-b",
            head: SHA,
            target: "main",
            remote_url: Some("https://github.com/a/b.git'; id; '"),
            bundle: false,
            delta: false,
            clean: false,
        };
        assert!(start_branch(&start).is_err());
        let start = BranchStart {
            remote_url: Some("git@github.com:a/b.git"),
            ..start
        };
        assert!(start_branch(&start).is_err());
    }

    #[test]
    fn bundle_answers_are_read() {
        assert_eq!(parse_bundled("aura-bundle: none\n"), Some(Bundled::Nothing));
        assert_eq!(
            parse_bundled(&format!("noise\naura-bundle: 3 {SHA}\n")),
            Some(Bundled::Commits {
                commits: 3,
                tip: SHA.into()
            })
        );
        assert_eq!(parse_bundled("aura-bundle: 3 HEAD"), None);
        assert_eq!(parse_bundled(""), None);
    }
}
