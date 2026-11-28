//! A forge that acts like GitHub over the test's bare origin: PRs of its
//! branches, comments and statuses kept for the test to read, and a
//! merge that makes a merge commit on `main` only while the PR's head is
//! the one it is given.

#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

use super::git;
use async_trait::async_trait;
use aura_exec_merge_queue::forge::{
    CheckState, Forge, ForgeError, MergeOutcome, PrRequest, PrState, PullRequest,
};
use aura_exec_merge_queue::PrRef;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// One PR of the fake forge.
#[derive(Debug, Clone)]
pub struct FakePr {
    pub number: u64,
    pub branch: String,
    pub target: String,
    pub title: String,
    pub body: String,
    pub state: PrState,
    pub head: String,
    pub merge_commit: Option<String>,
}

type Hook = Box<dyn FnOnce() + Send>;

/// GitHub, as far as the queue uses it, over `origin`.
pub struct FakeForge {
    origin: PathBuf,
    pub prs: Mutex<Vec<FakePr>>,
    pub comments: Mutex<Vec<(u64, String)>>,
    pub statuses: Mutex<Vec<(String, CheckState, String)>>,
    /// Every call fails as GitHub being unreachable while set
    pub unavailable: AtomicBool,
    /// Runs once, just before the next merge: someone pushes, or closes
    /// the PR, between the test and the merge
    before_merge: Mutex<Option<Hook>>,
}

impl FakeForge {
    pub fn new(origin: PathBuf) -> Self {
        Self {
            origin,
            prs: Mutex::default(),
            comments: Mutex::default(),
            statuses: Mutex::default(),
            unavailable: AtomicBool::new(false),
            before_merge: Mutex::default(),
        }
    }

    pub fn before_next_merge(&self, hook: impl FnOnce() + Send + 'static) {
        *self.before_merge.lock().unwrap() = Some(Box::new(hook));
    }

    pub fn close(&self, number: u64) {
        let mut prs = self.prs.lock().unwrap();
        let pr = prs.iter_mut().find(|pr| pr.number == number).unwrap();
        pr.state = PrState::Closed;
    }

    pub fn pr_of(&self, branch: &str) -> FakePr {
        self.prs
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|pr| pr.branch == branch)
            .cloned()
            .unwrap()
    }

    pub fn comments_on(&self, number: u64) -> Vec<String> {
        self.comments
            .lock()
            .unwrap()
            .iter()
            .filter(|(on, _)| *on == number)
            .map(|(_, body)| body.clone())
            .collect()
    }

    pub fn status_of(&self, sha: &str) -> Option<CheckState> {
        self.statuses
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(on, _, _)| on == sha)
            .map(|(_, state, _)| *state)
    }

    /// The tip of `branch` on origin; `None` once it is deleted.
    pub fn tip(&self, branch: &str) -> Option<String> {
        let out = Command::new("git")
            .args(["rev-parse", "--verify", "--quiet"])
            .arg(format!("refs/heads/{branch}"))
            .current_dir(&self.origin)
            .output()
            .unwrap();
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn up(&self) -> Result<(), ForgeError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(ForgeError::Unavailable("connection refused".to_string()));
        }
        Ok(())
    }

    fn view(&self, pr: &mut FakePr) -> PullRequest {
        if pr.state == PrState::Open {
            if let Some(tip) = self.tip(&pr.branch) {
                pr.head = tip;
            }
        }
        PullRequest {
            pr: PrRef {
                number: pr.number,
                url: format!("https://github.com/o/r/pull/{}", pr.number),
            },
            state: pr.state,
            head: pr.head.clone(),
            merge_commit: pr.merge_commit.clone(),
        }
    }

    /// GitHub's merge of `head` into `target`: a merge commit, the target
    /// first; `None` when they conflict.
    fn merge_commit(&self, target: &str, head: &str, number: u64) -> Option<String> {
        let tip = self.tip(target).unwrap();
        let out = Command::new("git")
            .args(["merge-tree", "--write-tree", &tip, head])
            .current_dir(&self.origin)
            .output()
            .unwrap();
        if !out.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let tree = stdout.lines().next().unwrap().to_string();
        let message = format!("Merge pull request #{number}");
        let commit = git(
            &self.origin,
            &["commit-tree", &tree, "-p", &tip, "-p", head, "-m", &message],
        );
        let branch = format!("refs/heads/{target}");
        git(&self.origin, &["update-ref", &branch, &commit, &tip]);
        Some(commit)
    }
}

#[async_trait]
impl Forge for FakeForge {
    async fn open_or_find(&self, request: &PrRequest<'_>) -> Result<PullRequest, ForgeError> {
        self.up()?;
        let mut prs = self.prs.lock().unwrap();
        if let Some(open) = prs
            .iter_mut()
            .find(|pr| pr.branch == request.branch && pr.state == PrState::Open)
        {
            return Ok(self.view(open));
        }
        let mut pr = FakePr {
            number: prs.len() as u64 + 1,
            branch: request.branch.to_string(),
            target: request.target.to_string(),
            title: request.title.to_string(),
            body: request.body.to_string(),
            state: PrState::Open,
            head: String::new(),
            merge_commit: None,
        };
        let view = self.view(&mut pr);
        prs.push(pr);
        Ok(view)
    }

    async fn pr(&self, number: u64) -> Result<PullRequest, ForgeError> {
        self.up()?;
        let mut prs = self.prs.lock().unwrap();
        let pr = prs.iter_mut().find(|pr| pr.number == number).unwrap();
        Ok(self.view(pr))
    }

    async fn comment(&self, number: u64, body: &str) -> Result<(), ForgeError> {
        self.up()?;
        self.comments
            .lock()
            .unwrap()
            .push((number, body.to_string()));
        Ok(())
    }

    async fn set_status(&self, sha: &str, state: CheckState, text: &str) -> Result<(), ForgeError> {
        self.up()?;
        self.statuses
            .lock()
            .unwrap()
            .push((sha.to_string(), state, text.to_string()));
        Ok(())
    }

    async fn merge(&self, number: u64, head: &str) -> Result<MergeOutcome, ForgeError> {
        self.up()?;
        let hook = self.before_merge.lock().unwrap().take();
        if let Some(hook) = hook {
            hook();
        }
        let mut prs = self.prs.lock().unwrap();
        let pr = prs.iter_mut().find(|pr| pr.number == number).unwrap();
        let now = self.view(pr);
        match now.state {
            PrState::Merged => return Ok(MergeOutcome::Refused("already merged".to_string())),
            PrState::Closed => return Ok(MergeOutcome::Refused("PR is closed".to_string())),
            PrState::Open => {}
        }
        if now.head != head {
            return Ok(MergeOutcome::HeadMoved { now: now.head });
        }
        let Some(sha) = self.merge_commit(&pr.target, head, number) else {
            return Ok(MergeOutcome::Refused(
                "Pull request is not mergeable".to_string(),
            ));
        };
        pr.state = PrState::Merged;
        pr.merge_commit = Some(sha.clone());
        Ok(MergeOutcome::Merged { sha })
    }
}
