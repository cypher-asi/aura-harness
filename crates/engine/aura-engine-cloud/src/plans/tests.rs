use super::*;
use crate::fake::FakeProvider;

/// Plans per owner: their current bearer, or an error.
#[derive(Default)]
pub(crate) struct FakeCredentials {
    pub tokens: Mutex<HashMap<(Owner, ProviderKind), Result<String, String>>>,
}

impl FakeCredentials {
    pub(crate) fn link(&self, owner: &Owner, kind: ProviderKind, token: &str) {
        self.tokens
            .lock()
            .insert((owner.clone(), kind), Ok(token.to_string()));
    }
}

#[async_trait]
impl PlanCredentials for FakeCredentials {
    async fn credential(
        &self,
        owner: &Owner,
        kind: ProviderKind,
        _margin_secs: i64,
    ) -> Result<Option<InferenceCredential>, String> {
        match self.tokens.lock().get(&(owner.clone(), kind)).cloned() {
            None => Ok(None),
            Some(Err(err)) => Err(err),
            Some(Ok(bearer)) => Ok(Some(InferenceCredential {
                provider: kind,
                bearer,
                headers: Vec::new(),
                system_preamble: None,
            })),
        }
    }

    fn linked(&self, owner: &Owner) -> Vec<ProviderKind> {
        self.tokens
            .lock()
            .keys()
            .filter(|(o, _)| o == owner)
            .map(|(_, kind)| *kind)
            .collect()
    }

    fn shape(&self, kind: ProviderKind) -> Option<InferenceCredential> {
        let headers = match kind {
            ProviderKind::Claude => vec![("anthropic-beta".into(), "oauth-2025-04-20".into())],
            _ => Vec::new(),
        };
        Some(InferenceCredential {
            provider: kind,
            bearer: String::new(),
            headers,
            system_preamble: (kind == ProviderKind::Claude).then(|| "You are Claude Code".into()),
        })
    }
}

fn alice() -> Owner {
    Owner::Zos {
        user_id: "alice".into(),
    }
}
fn bob() -> Owner {
    Owner::Zos {
        user_id: "bob".into(),
    }
}

fn sync() -> (PlanSync, FakeProvider, Arc<FakeCredentials>) {
    let fake = FakeProvider::new();
    let creds = Arc::new(FakeCredentials::default());
    let sync = PlanSync::new(Arc::new(fake.clone()), creds.clone(), "inst01");
    (sync, fake, creds)
}

#[tokio::test]
async fn an_owner_gets_three_host_bound_secrets_unlinked_ones_included() {
    let (sync, fake, creds) = sync();
    creds.link(&alice(), ProviderKind::Claude, "sk-ant-alice-1");
    let names = sync.ensure_owner(&alice()).await.expect("ensure");
    assert_eq!(names.len(), 3);
    let secrets = fake.secrets();
    assert_eq!(
        fake.secret_value(&names[0]).as_deref(),
        Some("sk-ant-alice-1")
    );
    assert_eq!(fake.secret_value(&names[1]).as_deref(), Some(UNLINKED));
    assert_eq!(fake.secret_value(&names[2]).as_deref(), Some(UNLINKED));
    assert_eq!(secrets[&names[0]].domains, ["api.anthropic.com"]);
    assert_eq!(secrets[&names[1]].domains, ["api.openai.com"]);
    assert_eq!(secrets[&names[2]].domains, ["openrouter.ai"]);
    assert!(secrets.values().all(|s| s.replace_in == ["headers"]));
    let status = sync.status(&alice());
    assert!(status[0].linked && !status[1].linked && !status[2].linked);
    assert!(status[0].synced_ms.is_some());
}

#[tokio::test]
async fn ticks_rotate_only_changed_tokens_of_that_owner() {
    let (sync, fake, creds) = sync();
    creds.link(&alice(), ProviderKind::Claude, "a1");
    creds.link(&bob(), ProviderKind::Claude, "b1");
    let alice_names = sync.ensure_owner(&alice()).await.expect("alice");
    let bob_names = sync.ensure_owner(&bob()).await.expect("bob");
    assert!(
        alice_names.iter().all(|n| !bob_names.contains(n)),
        "names are per owner"
    );
    let before = fake.calls_matching("SetSecret(").len();
    sync.tick(&[alice(), bob()]).await;
    assert_eq!(
        fake.calls_matching("SetSecret(").len(),
        before,
        "nothing changed"
    );
    creds.link(&alice(), ProviderKind::Claude, "a2");
    sync.tick(&[alice(), bob()]).await;
    let sets = fake.calls_matching("SetSecret(");
    assert_eq!(sets.len(), before + 1);
    assert!(sets.last().is_some_and(|c| c.contains(&alice_names[0])));
    assert_eq!(fake.secret_value(&alice_names[0]).as_deref(), Some("a2"));
    assert_eq!(fake.secret_value(&bob_names[0]).as_deref(), Some("b1"));
}

#[tokio::test]
async fn revoking_and_removing_touch_one_owner() {
    let (sync, fake, creds) = sync();
    creds.link(&alice(), ProviderKind::OpenRouter, "sk-or-a");
    creds.link(&bob(), ProviderKind::OpenRouter, "sk-or-b");
    let a = sync.ensure_owner(&alice()).await.expect("alice");
    let b = sync.ensure_owner(&bob()).await.expect("bob");
    sync.revoke(&bob(), ProviderKind::OpenRouter)
        .await
        .expect("revoke");
    assert_eq!(fake.secret_value(&b[2]).as_deref(), Some(UNLINKED));
    assert_eq!(fake.secret_value(&a[2]).as_deref(), Some("sk-or-a"));
    sync.remove_owner(&bob()).await.expect("remove");
    assert!(b.iter().all(|n| fake.secret_value(n).is_none()));
    assert!(a.iter().all(|n| fake.secret_value(n).is_some()));
}

#[tokio::test]
async fn a_failing_link_keeps_the_last_value_and_backs_off_alone() {
    let (sync, fake, creds) = sync();
    creds.link(&alice(), ProviderKind::Chatgpt, "c1");
    let names = sync.ensure_owner(&alice()).await.expect("alice");
    creds.tokens.lock().insert(
        (alice(), ProviderKind::Chatgpt),
        Err("sign in again".into()),
    );
    sync.tick(&[alice()]).await;
    assert_eq!(
        fake.secret_value(&names[1]).as_deref(),
        Some("c1"),
        "a still-valid token stays"
    );
    assert!(sync.status(&alice())[1]
        .error
        .as_deref()
        .is_some_and(|e| e.contains("sign in again")));
}

#[test]
fn templates_carry_headers_and_never_a_token() {
    let (sync, _fake, creds) = sync();
    creds.link(&alice(), ProviderKind::Claude, "sk-ant-secret");
    let templates = sync.templates(&alice());
    assert_eq!(templates.len(), 3);
    assert_eq!(templates[0].provider, "claude");
    assert_eq!(templates[0].headers[0].0, "anthropic-beta");
    assert!(templates[0].system_preamble.is_some());
    assert_eq!(templates[0].env, sync.secret_names(&alice())[0]);
    let json = serde_json::to_string(&templates).expect("json");
    assert!(!json.contains("sk-ant-secret"));
}
