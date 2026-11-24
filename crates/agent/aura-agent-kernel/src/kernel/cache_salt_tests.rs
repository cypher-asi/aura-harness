//! A kernel's cache salt reaches every model request it sends.

use crate::kernel::{Kernel, KernelConfig};
use crate::ExecutorRouter;
use async_trait::async_trait;
use aura_core_types::AgentId;
use aura_model::{MockProvider, ModelProvider, ModelRequest, ModelResponse, ReasonerError};
use aura_store_db::{RocksStore, Store};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

/// Answers like [`MockProvider`], keeping the requests it was sent.
#[derive(Default)]
struct Capturing {
    requests: Mutex<Vec<ModelRequest>>,
}

impl Capturing {
    fn salts(&self) -> Vec<Option<String>> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.cache_salt.clone())
            .collect()
    }
}

#[async_trait]
impl ModelProvider for Capturing {
    fn name(&self) -> &'static str {
        "capturing"
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        self.requests.lock().unwrap().push(request.clone());
        MockProvider::simple_response("answer")
            .complete(request)
            .await
    }

    async fn health_check(&self) -> bool {
        true
    }
}

fn request() -> ModelRequest {
    ModelRequest::builder("test-model", "system")
        .message(aura_model::Message::user("test"))
        .try_build()
        .unwrap()
}

struct Fixture {
    provider: Arc<Capturing>,
    kernel: Kernel,
    _dirs: (TempDir, TempDir),
}

fn fixture(salt: Option<&str>) -> Fixture {
    let (db, workspace) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(db.path(), false).unwrap());
    let provider = Arc::new(Capturing::default());
    let config = KernelConfig {
        workspace_base: workspace.path().to_path_buf(),
        ..KernelConfig::default()
    };
    let kernel = Kernel::new(
        store,
        Arc::clone(&provider) as Arc<dyn ModelProvider + Send + Sync>,
        ExecutorRouter::new(),
        config,
        AgentId::generate(),
    )
    .unwrap();
    let kernel = match salt {
        Some(salt) => kernel.with_cache_salt(salt),
        None => kernel,
    };
    Fixture {
        provider,
        kernel,
        _dirs: (db, workspace),
    }
}

#[tokio::test]
async fn a_salted_kernel_scopes_its_requests_to_the_salt() {
    let fixture = fixture(Some("0123456789abcdef"));
    assert_eq!(fixture.kernel.cache_salt(), Some("0123456789abcdef"));
    fixture.kernel.reason(request()).await.unwrap();
    let _ = fixture.kernel.reason_streaming(request()).await.unwrap();
    let salt = Some("0123456789abcdef".to_string());
    assert_eq!(fixture.provider.salts(), vec![salt.clone(), salt]);
    let keys: Vec<Option<String>> = (fixture.provider.requests.lock().unwrap().iter())
        .map(|r| r.prompt_cache_key.clone())
        .collect();
    assert!(
        keys.iter().all(Option::is_some),
        "unkeyed requests get the salt as their cache key: {keys:?}"
    );
}

#[tokio::test]
async fn a_kernel_without_a_salt_leaves_requests_unscoped() {
    let fixture = fixture(None);
    assert_eq!(fixture.kernel.cache_salt(), None);
    fixture.kernel.reason(request()).await.unwrap();
    let _ = fixture.kernel.reason_streaming(request()).await.unwrap();
    assert_eq!(fixture.provider.salts(), vec![None, None]);
}

#[tokio::test]
async fn the_salt_set_in_the_config_scopes_requests_too() {
    let (db, workspace) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(db.path(), false).unwrap());
    let provider = Arc::new(Capturing::default());
    let config = KernelConfig {
        workspace_base: workspace.path().to_path_buf(),
        cache_salt: Some("feedc0de".to_string()),
        ..KernelConfig::default()
    };
    let kernel = Kernel::new(
        store,
        Arc::clone(&provider) as Arc<dyn ModelProvider + Send + Sync>,
        ExecutorRouter::new(),
        config,
        AgentId::generate(),
    )
    .unwrap();
    kernel.reason(request()).await.unwrap();
    assert_eq!(provider.salts(), vec![Some("feedc0de".to_string())]);
}
