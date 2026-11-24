//! What a comparison's runs do on their own, by topic.
//!
//! - `setup`: the example tasks' files, put in place or not
//! - `live`: what the BENCH tab says a run under way is doing
//! - `arm`: what an arm's agent and trace are called
//! - `check`: the task records kept out of a check's way
//! - `schedule`: what the user's chat hears as each run ends
//! - `labels`: the label of each prompt, asked for once and shared

mod arm;
mod check;
mod labels;
mod live;
mod schedule;
mod setup;
