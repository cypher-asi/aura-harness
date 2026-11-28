#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;

fn pr(state: PrState, head: &str, merge_commit: Option<&str>) -> PullRequest {
    PullRequest {
        pr: PrRef {
            number: 7,
            url: "https://github.com/o/r/pull/7".to_string(),
        },
        state,
        head: head.to_string(),
        merge_commit: merge_commit.map(str::to_string),
    }
}

#[test]
fn slugs_come_from_github_remote_urls_only() {
    let slug = |url| RepoSlug::from_remote_url(url).map(|slug| slug.to_string());
    for url in [
        "https://github.com/cypher-asi/aura-harness.git",
        "https://x-access-token:secret@github.com/cypher-asi/aura-harness",
        "git@github.com:cypher-asi/aura-harness.git",
        "ssh://git@github.com/cypher-asi/aura-harness/",
    ] {
        assert_eq!(
            slug(url).as_deref(),
            Some("cypher-asi/aura-harness"),
            "{url}"
        );
    }
    for url in [
        "https://gitlab.com/o/r",
        "/tmp/origin.git",
        "https://github.com/o",
        "https://github.com/../r",
    ] {
        assert_eq!(slug(url), None, "{url}");
    }
}

#[test]
fn the_pr_number_is_read_from_the_address_gh_prints() {
    assert_eq!(
        number_in_url("Creating pull request\nhttps://github.com/o/r/pull/121\n"),
        Some(121)
    );
    assert_eq!(number_in_url("nothing"), None);
}

#[test]
fn a_pr_is_read_from_gh_json() {
    let read = parse_pr(
        r#"{"number":5,"url":"https://github.com/o/r/pull/5","state":"MERGED",
            "headRefOid":"abc","mergeCommit":{"oid":"def"}}"#,
    )
    .unwrap();
    assert_eq!(read.pr.number, 5);
    assert_eq!(read.state, PrState::Merged);
    assert_eq!(read.merge_commit.as_deref(), Some("def"));
    let listed = parse_prs(
        r#"[{"number":6,"url":"u","state":"OPEN","headRefOid":"abc","mergeCommit":null}]"#,
    )
    .unwrap();
    assert_eq!(listed[0].state, PrState::Open);
    assert!(listed[0].merge_commit.is_none());
}

#[test]
fn merge_outcomes_tell_a_merge_a_moved_head_and_a_refusal_apart() {
    assert_eq!(
        merge_outcome(&pr(PrState::Merged, "abc", Some("m1")), "abc", None),
        MergeOutcome::Merged {
            sha: "m1".to_string()
        }
    );
    // `--match-head-commit` refused it: someone pushed since the test.
    assert_eq!(
        merge_outcome(
            &pr(PrState::Open, "new", None),
            "abc",
            Some("head moved".to_string())
        ),
        MergeOutcome::HeadMoved {
            now: "new".to_string()
        }
    );
    assert!(matches!(
        merge_outcome(&pr(PrState::Closed, "abc", None), "abc", None),
        MergeOutcome::Refused(why) if why.contains("closed")
    ));
    assert!(matches!(
        merge_outcome(
            &pr(PrState::Open, "abc", None),
            "abc",
            Some("base branch policy prohibits the merge".to_string())
        ),
        MergeOutcome::Refused(why) if why.contains("policy")
    ));
}

#[test]
fn sign_in_and_network_errors_hold_the_queue_other_errors_fail() {
    for stderr in [
        "To get started with GitHub CLI, please run:  gh auth login",
        "HTTP 401: Bad credentials",
        "dial tcp: lookup api.github.com: could not resolve host",
    ] {
        assert!(
            matches!(classify("gh pr list", stderr), ForgeError::Unavailable(_)),
            "{stderr}"
        );
    }
    assert!(matches!(
        classify("gh pr merge", "Pull request is not mergeable"),
        ForgeError::Failed(_)
    ));
}

/// A `gh` that records its arguments and prints a merged PR.
#[cfg(unix)]
fn fake_gh(dir: &std::path::Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = dir.join("gh");
    let log = dir.join("calls");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n[ -n \"$FAKE_GH_PROBE\" ] && exit 0\necho \"$GH_TOKEN|$*\" >> '{}'\ncase \"$2\" in view) \
             echo '{{\"number\":3,\"url\":\"u\",\"state\":\"MERGED\",\"headRefOid\":\"abc\",\
             \"mergeCommit\":{{\"oid\":\"m3\"}}}}';; esac\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Another test's fork may still hold the script open for writing for
    // a moment ("text file busy"): wait until it runs.
    for _ in 0..200 {
        let probe = std::process::Command::new(&script)
            .env("FAKE_GH_PROBE", "1")
            .status();
        match probe {
            Err(e) if e.raw_os_error() == Some(26) => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            _ => break,
        }
    }
    script
}

#[cfg(unix)]
#[tokio::test]
async fn a_merge_is_pinned_to_the_tested_head_and_names_the_repo() {
    let dir = tempfile::tempdir().unwrap();
    let forge = GhForge::new(RepoSlug::from_remote_url("git@github.com:o/r").unwrap())
        .with_token(Some("t0k".to_string()))
        .with_program(fake_gh(dir.path()));
    let outcome = forge.merge(3, "abc").await.unwrap();
    assert_eq!(
        outcome,
        MergeOutcome::Merged {
            sha: "m3".to_string()
        }
    );
    let calls = std::fs::read_to_string(dir.path().join("calls")).unwrap();
    let first = calls.lines().next().unwrap();
    assert_eq!(
        first,
        "t0k|pr merge 3 --repo o/r --merge --match-head-commit abc"
    );
    assert!(
        !format!("{forge:?}").contains("t0k"),
        "the token is never shown"
    );
}

#[tokio::test]
async fn a_missing_gh_is_unavailable_not_a_failure() {
    let forge = GhForge::new(RepoSlug::from_remote_url("git@github.com:o/r").unwrap())
        .with_program(PathBuf::from("/nonexistent/gh"));
    assert!(matches!(forge.pr(1).await, Err(ForgeError::Unavailable(_))));
}

#[cfg(unix)]
#[tokio::test]
async fn a_token_source_is_asked_on_each_call_and_a_fixed_token_wins() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    let dir = tempfile::tempdir().unwrap();
    let asked = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&asked);
    let source: TokenSource = Arc::new(move || {
        let n = counter.fetch_add(1, Ordering::SeqCst);
        Some(format!("stored{n}"))
    });
    let slug = RepoSlug::from_remote_url("git@github.com:o/r").unwrap();
    let forge = GhForge::new(slug.clone())
        .with_token_source(Arc::clone(&source))
        .with_program(fake_gh(dir.path()));
    forge.comment(3, "one").await.unwrap();
    forge.comment(3, "two").await.unwrap();
    let fixed = GhForge::new(slug)
        .with_token(Some("fixed".to_string()))
        .with_token_source(source)
        .with_program(fake_gh(dir.path()));
    fixed.comment(3, "three").await.unwrap();
    let calls = std::fs::read_to_string(dir.path().join("calls")).unwrap();
    let tokens: Vec<&str> = calls.lines().filter_map(|l| l.split('|').next()).collect();
    assert_eq!(tokens, ["stored0", "stored1", "fixed"]);
}
