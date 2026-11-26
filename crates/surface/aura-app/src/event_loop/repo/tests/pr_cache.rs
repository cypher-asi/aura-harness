//! How often GitHub is asked for a repo's PRs.

use super::super::github::{self, Failure, Pr, PrStatus, Slug, Source};
use super::super::{lock, prs, CACHE, PRS_FOR};
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

/// How many times [`Counted`] was asked for PRs.
static ASKED: AtomicUsize = AtomicUsize::new(0);

/// A source that lists one PR and counts how often it is asked.
struct Counted;

#[async_trait]
impl Source for Counted {
    async fn list(&self, _: &Slug) -> Result<Vec<Pr>, Failure> {
        ASKED.fetch_add(1, Ordering::SeqCst);
        Ok(vec![one_pr()])
    }
    async fn diff(&self, _: &Slug, _: u64) -> Result<String, Failure> {
        Err(Failure::Unavailable)
    }
}

fn counted() -> Vec<Box<dyn Source>> {
    vec![Box::new(Counted)]
}

fn one_pr() -> Pr {
    Pr {
        number: 1,
        title: "One".into(),
        status: PrStatus::Open,
        head: "one".into(),
        base: "main".into(),
        created: 1,
        merged: None,
        closed: None,
        url: String::new(),
    }
}

#[tokio::test]
async fn prs_are_asked_for_a_minute_at_a_time_unless_forced() {
    let slug = github::slug("git@github.com:acme/cached").unwrap();
    let asked = || ASKED.load(Ordering::SeqCst);

    let (answer, reading) = prs(&slug, false, counted).await;
    assert_eq!(answer, None, "a load doesn't wait for GitHub");
    let (_, again) = prs(&slug, false, counted).await;
    assert!(again.is_none(), "one ask at a time");
    reading.expect("GitHub is asked").await.unwrap();
    assert_eq!(asked(), 1);

    let (answer, reading) = prs(&slug, false, counted).await;
    assert_eq!(answer, Some(Ok(vec![one_pr()])));
    assert!(reading.is_none(), "a minute hasn't gone by");

    let (answer, reading) = prs(&slug, true, counted).await;
    assert_eq!(answer, Some(Ok(vec![one_pr()])));
    assert!(reading.is_none(), "forced, it waits for the answer");
    assert_eq!(asked(), 2);

    lock(&CACHE.prs).get_mut(&slug).unwrap().asked = Instant::now().checked_sub(PRS_FOR);
    let (_, reading) = prs(&slug, false, counted).await;
    reading.expect("a minute on, it asks again").await.unwrap();
    assert_eq!(asked(), 3);
}
