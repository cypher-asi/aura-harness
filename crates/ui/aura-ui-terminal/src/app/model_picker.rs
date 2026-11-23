//! State of the `/model` selector: a Ready / Sign-in tab, a type-to-filter
//! query and the highlighted row.
//!
//! The picker never owns the model list; every method that needs it takes
//! the slice so the list can change underneath (e.g. a `SetModels` after a
//! sign-in) without the picker going stale. The cursor is an index into
//! [`ModelPicker::visible`] and is kept within it.

use crate::events::ModelOptionView;

/// Rows the cursor jumps for PageUp / PageDown.
pub const PAGE_STEP: isize = 10;

/// The selector's entry for Gate itself, which picks a model per turn.
const GATE_ID: &str = "gate";

/// Which half of the model list the selector shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModelTab {
    /// Models usable right now (their sign-in is present, or none is needed).
    #[default]
    Ready,
    /// Models that need a sign-in first.
    SignIn,
}

impl ModelTab {
    /// Tab heading.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::SignIn => "Sign in",
        }
    }

    /// Whether `model` belongs on this tab.
    const fn holds(self, model: &ModelOptionView) -> bool {
        match self {
            Self::Ready => model.available,
            Self::SignIn => !model.available,
        }
    }

    const fn other(self) -> Self {
        match self {
            Self::Ready => Self::SignIn,
            Self::SignIn => Self::Ready,
        }
    }
}

/// What the model picked in the selector is for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PickerTarget {
    /// The session's model, as `/model` selects it
    #[default]
    Session,
    /// The model of one difficulty of a benchmark arm on the BENCH tab's
    /// form: `arm` 0 for A, 1 for B; `difficulty` 0 easy, 1 medium, 2 hard
    BenchDifficulty { arm: usize, difficulty: usize },
    /// The model that classifies a benchmark's prompts, on the BENCH
    /// tab's form
    BenchClassifier,
}

/// Where the selector and its model rows were drawn in the last frame,
/// for clicks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PickerLayout {
    /// The whole selector, borders included
    pub modal: ratatui::layout::Rect,
    /// The screen row of each model row shown, with its index in
    /// [`ModelPicker::visible`]
    pub rows: Vec<(u16, usize)>,
}

impl PickerLayout {
    /// The visible index of the model drawn on screen row `row`.
    #[must_use]
    pub fn model_at(&self, row: u16) -> Option<usize> {
        self.rows.iter().find(|(y, _)| *y == row).map(|(_, i)| *i)
    }
}

/// Selector state: tab, filter query, cursor and what the pick is for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelPicker {
    tab: ModelTab,
    filter: String,
    cursor: usize,
    target: PickerTarget,
}

impl ModelPicker {
    /// Tab being shown.
    #[must_use]
    pub const fn tab(&self) -> ModelTab {
        self.tab
    }

    /// What the pick is for.
    #[must_use]
    pub const fn target(&self) -> PickerTarget {
        self.target
    }

    /// Filter typed so far.
    #[must_use]
    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// Highlighted row in [`Self::visible`].
    #[must_use]
    pub const fn cursor(&self) -> usize {
        self.cursor
    }

    /// Models on the current tab matching the filter, in list order. A
    /// benchmark row takes a model, so Gate is left out for one.
    #[must_use]
    pub fn visible<'a>(&self, models: &'a [ModelOptionView]) -> Vec<&'a ModelOptionView> {
        let query = self.filter.to_lowercase();
        models
            .iter()
            .filter(|m| self.offers(m) && self.tab.holds(m) && matches_query(m, &query))
            .collect()
    }

    /// Number of models on the Ready and Sign-in tabs, ignoring the filter.
    #[must_use]
    pub fn counts(&self, models: &[ModelOptionView]) -> (usize, usize) {
        let offered = models.iter().filter(|m| self.offers(m));
        let ready = offered.clone().filter(|m| m.available).count();
        (ready, offered.count() - ready)
    }

    /// Whether the selector lists `model` for what the pick is for.
    fn offers(&self, model: &ModelOptionView) -> bool {
        self.target == PickerTarget::Session || model.id != GATE_ID
    }

    /// Reset for a fresh open for `target`: show the tab holding
    /// `current`, set the filter to `filter` and highlight `current` (or
    /// the first row).
    pub fn open_for(
        &mut self,
        models: &[ModelOptionView],
        current: &str,
        target: PickerTarget,
        filter: &str,
    ) {
        self.open(models, current);
        self.target = target;
        self.filter = filter.trim().to_string();
        self.cursor = self
            .visible(models)
            .iter()
            .position(|m| m.id == current)
            .unwrap_or(0);
    }

    /// Reset for a fresh open: show the tab holding `current`, clear the
    /// filter and highlight `current` (or the first row). The pick is the
    /// session's model.
    pub fn open(&mut self, models: &[ModelOptionView], current: &str) {
        self.target = PickerTarget::Session;
        self.tab = models
            .iter()
            .find(|m| m.id == current)
            .map_or(ModelTab::Ready, |m| {
                if m.available {
                    ModelTab::Ready
                } else {
                    ModelTab::SignIn
                }
            });
        self.filter.clear();
        self.cursor = self
            .visible(models)
            .iter()
            .position(|m| m.id == current)
            .unwrap_or(0);
    }

    /// Show the other tab, cursor on its first row.
    pub fn switch_tab(&mut self) {
        self.tab = self.tab.other();
        self.cursor = 0;
    }

    /// Append `c` to the filter.
    pub fn push(&mut self, c: char) {
        self.filter.push(c);
        self.cursor = 0;
    }

    /// Drop the filter's last character.
    pub fn pop(&mut self) {
        self.filter.pop();
        self.cursor = 0;
    }

    /// Clear the filter; `true` if there was one.
    pub fn clear_filter(&mut self) -> bool {
        let had_filter = !self.filter.is_empty();
        self.filter.clear();
        self.cursor = 0;
        had_filter
    }

    /// Move the cursor by `delta` rows, staying within the visible list.
    pub fn move_by(&mut self, models: &[ModelOptionView], delta: isize) {
        let last = self.last(models);
        self.cursor = self.cursor.saturating_add_signed(delta).min(last);
    }

    /// Cursor to the first row.
    pub fn home(&mut self) {
        self.cursor = 0;
    }

    /// Cursor to row `index` of the visible list, clamped to it.
    pub fn select(&mut self, models: &[ModelOptionView], index: usize) {
        self.cursor = index.min(self.last(models));
    }

    /// Cursor to the last visible row.
    pub fn end(&mut self, models: &[ModelOptionView]) {
        self.cursor = self.last(models);
    }

    /// Pull the cursor back inside the visible list after it shrank.
    pub fn clamp(&mut self, models: &[ModelOptionView]) {
        self.cursor = self.cursor.min(self.last(models));
    }

    /// Highlighted model, if the visible list is not empty.
    #[must_use]
    pub fn selected<'a>(&self, models: &'a [ModelOptionView]) -> Option<&'a ModelOptionView> {
        let visible = self.visible(models);
        visible
            .get(self.cursor.min(visible.len().saturating_sub(1)))
            .copied()
    }

    fn last(&self, models: &[ModelOptionView]) -> usize {
        self.visible(models).len().saturating_sub(1)
    }
}

/// The model `/model <query>` names exactly: the one with that id, else
/// one with that label the user can run now (labels such as `Claude Opus
/// 4.7` are shared across routes), else any with that label. Case is
/// ignored. Other names go to the event loop, which resolves aliases.
#[must_use]
pub fn exact_match<'a>(models: &'a [ModelOptionView], query: &str) -> Option<&'a ModelOptionView> {
    let query = query.trim();
    let labelled = || {
        models
            .iter()
            .filter(|m| m.label.eq_ignore_ascii_case(query))
    };
    models
        .iter()
        .find(|m| m.id.eq_ignore_ascii_case(query))
        .or_else(|| labelled().find(|m| m.available))
        .or_else(|| labelled().next())
}

/// Case-insensitive substring match on id or label; `query` is lowercase.
fn matches_query(model: &ModelOptionView, query: &str) -> bool {
    query.is_empty()
        || model.id.to_lowercase().contains(query)
        || model.label.to_lowercase().contains(query)
}

#[cfg(test)]
mod tests {
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
}
