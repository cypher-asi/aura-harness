//! The signed-in user, their plans and the models they can pick from.

use super::model_picker::{ModelPicker, PickerLayout};
use crate::events::{ModelOptionView, PlanView};

/// The signed-in user, plans and models.
#[derive(Debug, Default)]
pub(crate) struct Account {
    /// Whether the API is currently active
    pub(crate) api_active: bool,
    /// Stored email during login flow (between email and password steps)
    pub(crate) login_email: String,
    /// Models offered by the selector
    pub(crate) models: Vec<ModelOptionView>,
    /// Id of the selected model (`gate` when Gate picks it)
    pub(crate) current_model: String,
    /// The model and route Gate sends the next turn to
    pub(crate) model_route: Option<String>,
    /// Thinking level pinned for every turn; `None` when each model picks
    pub(crate) thinking: Option<String>,
    /// Logged-in user's display name (`None` = guest)
    pub(crate) user: Option<String>,
    /// Labels of the usable linked plans
    pub(crate) plans: Vec<String>,
    /// The active agent's current plan
    pub(crate) active_plan: Option<PlanView>,
    /// Model selector state: tab, filter and highlighted row
    pub(crate) model_picker: ModelPicker,
    /// Where the model selector's rows were drawn, for clicks
    pub(crate) model_picker_layout: PickerLayout,
}
