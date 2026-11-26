use crate::model_catalog::local::version_supports_messages_api;
use crate::model_catalog::*;

#[test]
fn live_plan_models_replace_the_built_in_list() {
    let live = [ModelOption {
        id: "claude/claude-opus-5-5".to_string(),
        label: "Claude Opus 5.5".to_string(),
        backend: ModelBackend::Claude,
        installed: true,
        tier: ModelTier::Frontier,
    }];
    let options = catalog_with_plans("local/qwen3:4b", &[], &live);
    let claude: Vec<&str> = options
        .iter()
        .filter(|o| o.backend == ModelBackend::Claude)
        .map(|o| o.id.as_str())
        .collect();
    assert_eq!(claude, ["claude/claude-opus-5-5"]);
    assert!(options.iter().any(|o| o.backend == ModelBackend::Router));
    assert!(options.iter().any(|o| o.backend == ModelBackend::Chatgpt));
}

#[test]
fn curated_tiers_match_inference() {
    for option in catalog("local/qwen3:4b", &[]) {
        assert_eq!(option.tier, infer_tier(&option.id), "{}", option.id);
    }
}

#[test]
fn catalog_offers_both_routes_for_claude() {
    let options = catalog("local/qwen3:4b", &[]);
    assert!(options
        .iter()
        .any(|o| o.id == "aura-claude-opus-4-7" && o.backend == ModelBackend::Router));
    assert!(options
        .iter()
        .any(|o| o.id == "claude/claude-opus-4-7" && o.backend == ModelBackend::Claude));
    assert!(options.iter().any(|o| o.backend == ModelBackend::Chatgpt));
}

#[test]
fn catalog_lists_default_local_first_and_dedupes() {
    let installed = vec!["qwen3:4b".to_string(), "llama3.2:3b".to_string()];
    let options = catalog("local/qwen3:4b", &installed);
    assert_eq!(options[0].id, "local/qwen3:4b");
    assert!(options[0].installed);
    assert_eq!(options[1].id, "local/llama3.2:3b");
    assert_eq!(
        options
            .iter()
            .filter(|o| o.backend == ModelBackend::Local)
            .count(),
        2
    );
    assert!(options
        .iter()
        .filter(|o| o.backend == ModelBackend::Router)
        .all(ModelOption::requires_login));
}

#[test]
fn catalog_marks_missing_default_as_not_installed() {
    let options = catalog("local/qwen3:4b", &[]);
    assert!(!options[0].installed);
}

#[test]
fn version_gate() {
    assert!(version_supports_messages_api("0.14.0"));
    assert!(version_supports_messages_api("0.15.2"));
    assert!(version_supports_messages_api("1.0.0"));
    assert!(!version_supports_messages_api("0.13.5"));
    assert!(version_supports_messages_api("dev"));
}

#[test]
fn installed_matches_latest_tag() {
    let installed = vec!["llama3.2:latest".to_string()];
    assert!(is_installed(&installed, "llama3.2"));
    assert!(!is_installed(&installed, "qwen3:4b"));
}

#[tokio::test]
async fn probe_unreachable_server() {
    let status = probe_local_server("http://127.0.0.1:9").await;
    assert_eq!(status, LocalServerStatus::Unreachable);
}
