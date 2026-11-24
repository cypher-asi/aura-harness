//! Running a `/compare` (a benchmark): every run of both arms in its own
//! copy of the workspace, then the blind judge, then the record and the
//! view.
//!
//! Each run is an agent of its own, registered under the comparison's
//! agent with the copy as its folder, so it shows in the Agents panel
//! and the Processes tab while it works, its subtasks work in the same
//! copy, and its records feed Gate's table like any turn. Its turn runs
//! on the same turn driver as the terminal's turns and `task` spawns:
//!
//! - a Gate arm's turns go where Gate sends them with exploration off,
//!   and their subtasks are routed the same way;
//! - a pinned arm's turns and subtasks run on the model it is pinned to;
//! - a by-difficulty arm's turns and subtasks run on the model it gives
//!   their difficulty, as the classifier rates it, and are not followed
//!   up, judged or moved to stronger models.
//!
//! A run sends its prompts one after the other on the same conversation,
//! as a replayed session did. Each prompt is recorded with route reason
//! `compare` and its turn ends with its `Outcome`; what each turn cost is
//! kept per prompt. A Gate arm's turns are followed up, judged and moved
//! to stronger models as the terminal's routed turns are, so a run costs
//! what the same work would outside a benchmark; and when a model's route
//! refuses the user's sign-in, the turn goes on on Gate's next choice and
//! the run's later turns leave that model out, where a pinned or
//! by-difficulty arm's run stops. After the last turn the
//! check command runs in the copy as a `run_command` call through the
//! run's kernel, so it is in the run's log, and its verdict is recorded as
//! a later `Outcome` once every run has ended, so it counts in the table
//! without steering the comparison's other runs.
//! Cost and tokens per model come from the records of the run and its
//! subtasks: every model call there, classifier calls included. So do
//! the plan readings those calls recorded; once every run has ended they
//! are shared out between the runs with `plan_shares`.
//!
//! - `plan`: the comparison as the user confirmed it
//! - `context`: what the runs of a comparison share, which run each is,
//!   and what each gives back
//! - `labels`: the classifier's label of each prompt, asked for once and
//!   shared by every run that sends it
//! - `arm`: what an arm is called, and how its runs are routed
//! - `host`: the comparison on its own agent, from its first record to
//!   its last
//! - `setup`: the example tasks' files, put in place for their runs
//! - `schedule`: the runs started as the schedule allows, each recorded
//!   as it ends
//! - `live`: the runs under way, as the BENCH tab shows them
//! - `start`: one run getting going: its copy, its agent and its session
//! - `copy`: the copies the runs work in, what each changed, and their
//!   removal
//! - `turn`: a run's prompts one after the other, then its check
//! - `check`: the check of a run's work
//! - `prompt`: one prompt's turn, and the attempts at it
//! - `judge`: the blind judge of each pair of runs

mod arm;
mod check;
mod context;
mod copy;
mod host;
mod judge;
mod labels;
mod live;
mod plan;
mod prompt;
mod schedule;
mod setup;
mod start;
#[cfg(test)]
mod tests;
mod turn;

pub(super) use copy::{patches, remove_copies};
pub(super) use host::run;
pub(crate) use host::save;
pub(super) use plan::ComparePlan;
