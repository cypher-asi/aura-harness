
use super::*;

fn model(id: &str, label: &str, available: bool) -> ModelOptionView {
    ModelOptionView {
        id: id.to_string(),
        label: label.to_string(),
        local: false,
        installed: true,
        group: "Cloud".to_string(),
        provider: (!available).then(|| "claude".to_string()),
        available,
        context_window: None,
    }
}

fn models() -> Vec<ModelOptionView> {
    vec![
        model("gate", "Gate", true),
        model("local/qwen3:4b", "qwen3:4b", true),
        model("aura-claude-sonnet-4-6", "Claude Sonnet 4.6", true),
        model("claude/claude-sonnet-4-6", "Claude Sonnet 4.6", false),
        model("chatgpt/gpt-5", "GPT-5", false),
    ]
}

fn ids<'a>(visible: &[&'a ModelOptionView]) -> Vec<&'a str> {
    visible.iter().map(|m| m.id.as_str()).collect()
}

#[test]
fn open_puts_the_cursor_on_the_current_model() {
    let models = models();
    let mut picker = ModelPicker::default();
    picker.open(&models, "aura-claude-sonnet-4-6");
    assert_eq!(picker.tab(), ModelTab::Ready);
    assert_eq!(picker.cursor(), 2);

    picker.open(&models, "chatgpt/gpt-5");
    assert_eq!(picker.tab(), ModelTab::SignIn);
    assert_eq!(picker.cursor(), 1);
    assert_eq!(
        picker.selected(&models).map(|m| m.id.as_str()),
        Some("chatgpt/gpt-5")
    );
}

#[test]
fn open_clears_the_filter_and_falls_back_to_the_first_row() {
    let models = models();
    let mut picker = ModelPicker::default();
    picker.push('x');
    picker.open(&models, "unlisted");
    assert_eq!(picker.filter(), "");
    assert_eq!(picker.tab(), ModelTab::Ready);
    assert_eq!(picker.cursor(), 0);
}

#[test]
fn ready_holds_available_models_and_sign_in_the_rest() {
    let models = models();
    let mut picker = ModelPicker::default();
    assert_eq!(
        ids(&picker.visible(&models)),
        ["gate", "local/qwen3:4b", "aura-claude-sonnet-4-6"]
    );
    picker.switch_tab();
    assert_eq!(
        ids(&picker.visible(&models)),
        ["claude/claude-sonnet-4-6", "chatgpt/gpt-5"]
    );
}

#[test]
fn counts_ignore_the_filter() {
    let models = models();
    let picker = ModelPicker::default();
    assert_eq!(picker.counts(&models), (3, 2));
    assert_eq!(picker.counts(&[]), (0, 0));
}

#[test]
fn a_benchmark_row_is_offered_models_only_filtered_by_what_was_typed() {
    let models = models();
    let mut picker = ModelPicker::default();
    let target = PickerTarget::BenchDifficulty {
        arm: 1,
        difficulty: 2,
    };
    picker.open_for(&models, "", target, "");
    assert_eq!(picker.target(), target);
    assert_eq!(
        ids(&picker.visible(&models)),
        ["local/qwen3:4b", "aura-claude-sonnet-4-6"],
        "no Gate"
    );
    assert_eq!(picker.counts(&models), (2, 2));

    picker.open_for(&models, "chatgpt/gpt-5", target, " gpt ");
    assert_eq!(picker.filter(), "gpt");
    assert_eq!(picker.tab(), ModelTab::SignIn);
    assert_eq!(
        picker.selected(&models).map(|m| m.id.as_str()),
        Some("chatgpt/gpt-5")
    );

    picker.open(&models, "gate");
    assert_eq!(picker.target(), PickerTarget::Session, "/model again");
    assert_eq!(picker.visible(&models)[0].id, "gate");
}

#[test]
fn typing_filters_on_label_and_id_case_insensitively() {
    let models = models();
    let mut picker = ModelPicker::default();
    for c in "sonnet".chars() {
        picker.push(c);
    }
    assert_eq!(ids(&picker.visible(&models)), ["aura-claude-sonnet-4-6"]);

    let mut picker = ModelPicker::default();
    for c in "QWEN".chars() {
        picker.push(c);
    }
    assert_eq!(ids(&picker.visible(&models)), ["local/qwen3:4b"]);

    let mut picker = ModelPicker::default();
    for c in "aura-".chars() {
        picker.push(c);
    }
    assert_eq!(ids(&picker.visible(&models)), ["aura-claude-sonnet-4-6"]);
}

#[test]
fn backspace_widens_the_filter() {
    let models = models();
    let mut picker = ModelPicker::default();
    for c in "gatex".chars() {
        picker.push(c);
    }
    assert!(picker.visible(&models).is_empty());
    assert!(picker.selected(&models).is_none());
    picker.pop();
    assert_eq!(ids(&picker.visible(&models)), ["gate"]);
    assert!(picker.clear_filter());
    assert!(!picker.clear_filter());
    assert_eq!(picker.visible(&models).len(), 3);
}

#[test]
fn editing_the_filter_resets_the_cursor() {
    let models = models();
    let mut picker = ModelPicker::default();
    picker.end(&models);
    picker.push('a');
    assert_eq!(picker.cursor(), 0);
    picker.end(&models);
    picker.pop();
    assert_eq!(picker.cursor(), 0);
}

#[test]
fn switching_tabs_resets_the_cursor() {
    let models = models();
    let mut picker = ModelPicker::default();
    picker.end(&models);
    assert_eq!(picker.cursor(), 2);
    picker.switch_tab();
    assert_eq!(picker.tab(), ModelTab::SignIn);
    assert_eq!(picker.cursor(), 0);
    picker.switch_tab();
    assert_eq!(picker.tab(), ModelTab::Ready);
}

#[test]
fn move_is_clamped_to_the_visible_list() {
    let models = models();
    let mut picker = ModelPicker::default();
    picker.move_by(&models, -1);
    assert_eq!(picker.cursor(), 0);
    picker.move_by(&models, PAGE_STEP);
    assert_eq!(picker.cursor(), 2);
    picker.move_by(&models, -1);
    assert_eq!(picker.cursor(), 1);
    picker.home();
    assert_eq!(picker.cursor(), 0);
}

#[test]
fn exact_match_prefers_the_id_then_a_runnable_label() {
    let models = models();
    let id = |m: Option<&ModelOptionView>| m.map(|m| m.id.clone());
    assert_eq!(
        id(exact_match(&models, "CLAUDE/claude-sonnet-4-6")),
        Some("claude/claude-sonnet-4-6".to_string())
    );
    // Both Sonnet routes share the label; the runnable one wins even
    // when it is listed second.
    let mut reordered = models.clone();
    reordered.swap(2, 3);
    assert_eq!(
        id(exact_match(&reordered, "claude sonnet 4.6")),
        Some("aura-claude-sonnet-4-6".to_string())
    );
    // With no runnable route the label still names one.
    assert_eq!(
        id(exact_match(&models, "gpt-5")),
        Some("chatgpt/gpt-5".to_string())
    );
    assert_eq!(id(exact_match(&models, "opus")), None);
}

#[test]
fn exact_label_prefers_a_linked_plan_over_other_runnable_routes() {
    let routed = |id: &str, provider: &str| ModelOptionView {
        provider: Some(provider.to_string()),
        ..model(id, "Claude Opus 4.7", true)
    };
    let models = vec![
        routed("aura-claude-opus-4-7", "zos"),
        routed("openrouter/anthropic/claude-opus-4.7", "openrouter"),
        routed("claude/claude-opus-4-7", "claude"),
    ];
    assert_eq!(
        exact_match(&models, "claude opus 4.7").map(|m| m.id.as_str()),
        Some("claude/claude-opus-4-7")
    );
    // Without the plan linked, the first runnable route wins.
    let mut unlinked = models.clone();
    unlinked[2].available = false;
    assert_eq!(
        exact_match(&unlinked, "claude opus 4.7").map(|m| m.id.as_str()),
        Some("aura-claude-opus-4-7")
    );
}

#[test]
fn clamp_follows_a_shrinking_list() {
    let mut models = models();
    let mut picker = ModelPicker::default();
    picker.end(&models);
    models.truncate(1);
    picker.clamp(&models);
    assert_eq!(picker.cursor(), 0);
    assert_eq!(
        picker.selected(&models).map(|m| m.id.as_str()),
        Some("gate")
    );
}
