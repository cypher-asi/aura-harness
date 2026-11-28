//! Telling a failing test's cause apart: the entry, a flaky test, or a
//! target that is broken already.
//!
//! When the test check fails on an entry's merge, the failing tests are
//! read from its log ([`crate::failure`]) and:
//!
//! 1. the whole test command runs once more on the same merge: passing now, they are flaky, and
//!    the entry goes on (its note names them);
//! 2. else run on the plain target: failing there too, the target is
//!    broken and the entry is not to blame ([`HoldKind::TargetBroken`],
//!    with the target's commits since it last passed as suspects);
//! 3. else the entry broke them, and fails with their names and output.
//!
//! Output that names no tests (not libtest's) fails the entry as before:
//! running a whole unknown suite three times costs more than it tells.

use super::{GitIntegrator, Step, Stop};
use crate::checks::run_check;
use crate::config::QueueConfig;
use crate::entry::short_sha;
use crate::failure::{failed_tests, rerun_command, test_excerpt};
use crate::git;
use crate::report::{Hold, HoldKind, Suspect};
use chrono::Utc;
use std::path::Path;

/// Bytes of a check's log read for its failing tests.
const LOG_BYTES: u64 = 4 * 1024 * 1024;
/// How long entries parked behind a broken target wait at most before
/// they are tried again, should the target never move.
const BROKEN_RETRY: chrono::Duration = chrono::Duration::hours(2);
/// Most suspects named when the target's last passing commit is unknown.
const RECENT_SUSPECTS: &str = "5";

impl GitIntegrator {
    /// Diagnose the test check `command` that failed (`stop`) on the
    /// merge in the worktree, onto `target`. `Ok` with a note when the
    /// tests were flaky; else the entry's failure, or a hold when the
    /// target fails them too.
    pub(super) async fn diagnose(
        &self,
        command: &str,
        config: &QueueConfig,
        target: &str,
        stop: Stop,
    ) -> Step<String> {
        let Stop::Fail(mut failure) = stop else {
            return Err(stop);
        };
        let output = read_log(&self.repo.check_log()).await;
        let tests = failed_tests(&output);
        let Some(rerun) = rerun_command(command, &tests) else {
            return Err(Stop::Fail(failure));
        };
        failure.excerpt = test_excerpt(&output, &tests).or(failure.excerpt.take());
        failure.tests.clone_from(&tests);
        let worktree = self.repo.worktree();
        let log = self.repo.check_log().with_extension("rerun.log");
        // The whole command again, not only the tests that failed: a pass
        // of just those could hide a test they break in others.
        if run_check(&worktree, command, config.check_timeout, &log)
            .await
            .is_ok()
        {
            tracing::warn!(tests = %tests.join(", "), "Flaky tests passed when run again");
            return Ok(format!(
                "flaky, passed when run again: {}",
                tests.join(", ")
            ));
        }
        let timeout = self.config.git_timeout;
        git::ok(
            &worktree,
            &["checkout", "--detach", "--force", target],
            timeout,
        )
        .await?;
        if run_check(&worktree, &rerun, config.check_timeout, &log)
            .await
            .is_ok()
        {
            failure.reason = format!(
                "{} fail on the merge and pass on {} at {}: {}",
                tests.join(", "),
                self.config.target,
                short_sha(target),
                failure.reason
            );
            return Err(Stop::Fail(failure));
        }
        let on_target = read_log(&log).await;
        Err(Stop::Hold(Box::new(
            self.broken(target, &tests, &on_target).await,
        )))
    }

    /// The hold for a target at `target` that fails `tests`.
    async fn broken(&self, target: &str, tests: &[String], output: &str) -> Hold {
        let now = Utc::now();
        let name = &self.config.target;
        Hold {
            reason: format!(
                "{name} at {} fails {} on its own; the entries that hit it wait until {name} moves",
                short_sha(target),
                tests.join(", ")
            ),
            kind: HoldKind::TargetBroken,
            since: now,
            retry_at: now + BROKEN_RETRY,
            target_tip: Some(target.to_string()),
            tests: tests.to_vec(),
            excerpt: test_excerpt(output, tests),
            suspects: self.suspects(target).await,
        }
    }

    /// The target's first-parent commits since it last passed the
    /// checks here, newest first; its last few when that is unknown.
    async fn suspects(&self, target: &str) -> Vec<Suspect> {
        let green = self.last_green();
        let range = match &green {
            Some(green) => format!("{green}..{target}"),
            None => target.to_string(),
        };
        let mut args = vec!["log", "--first-parent", "--format=%H%x09%an%x09%s"];
        if green.is_none() {
            args.extend(["-n", RECENT_SUSPECTS]);
        }
        args.push(&range);
        let listed = git::run(&self.repo.top, &args, self.config.git_timeout).await;
        let Ok(listed) = listed else {
            return Vec::new();
        };
        listed
            .stdout
            .lines()
            .filter_map(|line| {
                let mut parts = line.splitn(3, '\t');
                Some(Suspect {
                    sha: parts.next()?.to_string(),
                    author: parts.next()?.to_string(),
                    subject: parts.next().unwrap_or_default().to_string(),
                })
            })
            .take(20)
            .collect()
    }
}

/// The end of the log at `path` (all of it up to [`LOG_BYTES`]); empty
/// when it can't be read.
async fn read_log(path: &Path) -> String {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let Ok(mut file) = tokio::fs::File::open(path).await else {
        return String::new();
    };
    let length = file.metadata().await.map_or(0, |meta| meta.len());
    let start = length.saturating_sub(LOG_BYTES);
    if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return String::new();
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).await.is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&bytes).into_owned()
}
