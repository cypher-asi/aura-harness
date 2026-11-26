use super::*;

#[test]
fn github_remotes_give_their_repo() {
    let repo = |owner: &str, repo: &str| {
        Some(Slug {
            owner: owner.into(),
            repo: repo.into(),
        })
    };
    assert_eq!(
        slug("https://github.com/acme/aura.git"),
        repo("acme", "aura")
    );
    assert_eq!(slug("https://github.com/acme/aura/"), repo("acme", "aura"));
    assert_eq!(slug("git@github.com:acme/aura.git"), repo("acme", "aura"));
    assert_eq!(
        slug("ssh://git@github.com:22/acme/aura"),
        repo("acme", "aura")
    );
    assert_eq!(
        slug("https://user@GitHub.com/acme/my.repo"),
        repo("acme", "my.repo")
    );
    assert_eq!(slug("https://gitlab.com/acme/aura.git"), None);
    assert_eq!(slug("git@example.com:acme/aura.git"), None);
    assert_eq!(slug("https://github.com/acme"), None);
    assert_eq!(slug("https://github.com/acme/aura/pulls"), None);
    assert_eq!(slug("/srv/git/aura.git"), None);
}

#[test]
fn gh_and_the_api_read_alike() {
    let gh = gh_prs(
        r#"[{"number":7,"title":"Add REPO","state":"OPEN","isDraft":true,
                 "headRefName":"repo","baseRefName":"main","createdAt":"2026-11-20T10:00:00Z",
                 "mergedAt":null,"closedAt":"0001-01-01T00:00:00Z","url":"https://github.com/a/b/pull/7"},
                {"number":6,"title":"Fix","state":"MERGED","isDraft":false,"headRefName":"fix",
                 "baseRefName":"main","createdAt":"2026-11-19T10:00:00Z",
                 "mergedAt":"2026-11-19T12:00:00Z","closedAt":"2026-11-19T12:00:00Z","url":""}]"#,
    )
    .unwrap();
    let api = api_prs(
        r#"[{"number":7,"title":"Add REPO","state":"open","draft":true,"head":{"ref":"repo"},
                 "base":{"ref":"main"},"created_at":"2026-11-20T10:00:00Z","merged_at":null,
                 "closed_at":null,"html_url":"https://github.com/a/b/pull/7"},
                {"number":6,"title":"Fix","state":"closed","draft":false,"head":{"ref":"fix"},
                 "base":{"ref":"main"},"created_at":"2026-11-19T10:00:00Z",
                 "merged_at":"2026-11-19T12:00:00Z","closed_at":"2026-11-19T12:00:00Z","html_url":""}]"#,
    )
    .unwrap();
    assert_eq!(gh, api);
    assert_eq!(gh[0].status, PrStatus::Draft);
    assert_eq!((gh[0].closed, gh[0].at()), (None, gh[0].created));
    assert_eq!(gh[1].status, PrStatus::Merged);
    assert_eq!(gh[1].at(), gh[1].merged.unwrap());
}

/// A source that answers as told.
struct Fake(Result<Vec<Pr>, Failure>);

#[async_trait]
impl Source for Fake {
    async fn list(&self, _: &Slug) -> Result<Vec<Pr>, Failure> {
        self.0.clone()
    }
    async fn diff(&self, _: &Slug, number: u64) -> Result<String, Failure> {
        self.0.clone().map(|_| format!("diff of #{number}"))
    }
}

fn fake(answer: Result<Vec<Pr>, Failure>) -> Box<dyn Source> {
    Box::new(Fake(answer))
}

#[tokio::test]
async fn prs_come_from_the_first_source_that_can_read_them() {
    let slug = slug("git@github.com:a/b").unwrap();
    let pr = Pr {
        number: 1,
        title: "One".into(),
        status: PrStatus::Open,
        head: "one".into(),
        base: "main".into(),
        created: 1,
        merged: None,
        closed: None,
        url: String::new(),
    };
    let signed_in = [
        fake(Ok(vec![pr.clone()])),
        fake(Err(Failure::Failed("asked".into()))),
    ];
    assert_eq!(
        list(&signed_in, &slug).await,
        Ok(vec![pr.clone()]),
        "gh, signed in, is the only one asked"
    );

    let both = [fake(Err(Failure::Unavailable)), fake(Ok(vec![pr.clone()]))];
    assert_eq!(list(&both, &slug).await, Ok(vec![pr]));
    assert_eq!(diff(&both, &slug, 4).await.as_deref(), Ok("diff of #4"));

    let failing = [
        fake(Err(Failure::Failed("HTTP 502".into()))),
        fake(Ok(Vec::new())),
    ];
    assert_eq!(list(&failing, &slug).await, Err("HTTP 502".to_string()));

    let neither = [
        fake(Err(Failure::Unavailable)),
        fake(Err(Failure::Unavailable)),
    ];
    assert_eq!(list(&neither, &slug).await, Err(SIGN_IN.to_string()));
}

#[tokio::test]
async fn the_api_without_a_token_is_not_used() {
    let api = Api { token: None };
    let slug = slug("git@github.com:a/b").unwrap();
    assert_eq!(api.list(&slug).await, Err(Failure::Unavailable));
}
