//! Layer: gate
//!
//! Gate, the router behind the `gate` model id: it picks the model each
//! turn and subtask runs on from the models the user's sign-ins allow,
//! and checks the work before it stands.
//!
//! Each section is a crate of its own, re-exported here under one name:
//!
//! - [`route`]: the candidates, the model a turn or subtask goes to and
//!   at what effort, what Gate has learned of each model, and replay
//! - [`evidence`]: what an attempt left behind, and the signs of a failed
//!   attempt read out of it
//! - [`followup`]: what Gate does about a routed turn that ran no check
//! - [`judge`]: the classifier, criteria, judge and review requests
//! - [`attempts`]: the local log of attempts and the report that scores
//!   Gate against what later proved them right or wrong
//! - [`compare`]: `/compare`'s arguments, schedule, estimate and summary
//! - [`lead`]: what the lead agent may do itself while delegation is
//!   enforced, and how much of its work went to subtasks
//! - [`live`]: Gate while the app runs: the table every agent's turns
//!   share, the attempt log as attempts end, and Gate's own calls
//!
//! Gate never calls a provider itself: it chooses which model a turn
//! runs on and builds the classifier's and judge's requests, and the
//! calls go through the agent's kernel, which records them as usual.
#![deny(clippy::too_many_lines)]

pub use aura_gate_attempts as attempts;
pub use aura_gate_compare as compare;
pub use aura_gate_evidence as evidence;
pub use aura_gate_followup as followup;
pub use aura_gate_judge as judge;
pub use aura_gate_lead as lead;
pub use aura_gate_live as live;
pub use aura_gate_route as route;
