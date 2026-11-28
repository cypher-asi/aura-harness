//! The software factory: a channel member plans its task, builds the
//! plan, and ships it into its repo's merge queue, which merges pushes
//! into main one at a time. See `docs/software-factory.md`.
//!
//! - [`flow`]: plan, build, ship, as pure stages.
//! - [`hooks`]: where the event loop applies them.
//! - [`observe`]: every push any agent makes, from its tool results.
//! - [`turn`]: what one turn did: the plan it wrote, the pushes it made.
//! - [`service`]: the repos' merge queues and their processors.
//! - [`notes`]: what the queue tells the agents.
//! - [`repos`]: the channels' repos, their coordinators and forges.
//! - [`overlap`]: warning agents whose work changes the same files.
//! - [`view`]: a queue as the QUEUE tab shows it.
//! - [`pages`]: how much of its history each QUEUE tab shows.

pub(crate) mod flow;
pub(crate) mod hooks;
pub(crate) mod notes;
pub(crate) mod observe;
pub(crate) mod overlap;
pub(crate) mod pages;
pub(crate) mod repos;
pub(crate) mod saved;
pub(crate) mod service;
pub(crate) mod turn;
pub(crate) mod view;

pub(crate) use flow::Flows;

/// How a channel member works in the factory, in its prompt after its
/// `<channel>` block.
pub(crate) const FACTORY_RULES: &str = "<software_factory>
Every task you are given runs plan, then build, then ship, on its own:
- Your first turn on a task plans it: research, then write the plan with `create_plan`. Once \
the plan exists it is built right away; nobody approves it, so don't ask whether to start.
- Ask a question only when a wrong guess could cause harm, lose data, or leave major \
inconsistencies. Otherwise pick the safe, conventional default, say which in the plan or report, \
and keep going.
- When you start building a task, call `git_start_task` with a few words naming it: it makes \
your own fresh branch (`aura/<task>-<your code>`) off the latest target. Every task gets a new \
branch; never reuse an earlier task's branch.
- When the work is done, call `git_ship`: it commits every change, pulls the merge queue's target \
branch (main unless the repo's committed `.aura/merge-queue.toml` names another) into your \
branch, pushes your branch and puts it in the merge queue. The queue opens or updates the \
branch's PR, tests it merged with the target and merges it, one at a time. Never push the \
target and never merge a PR yourself. If it fails, the PR stays open with a comment and you are \
told why: fix it on the same branch and ship again.
- If the repo has a `changelog.d/` folder, write your changelog entry as a new file there, named \
after your branch (`changelog.d/<last part of the branch>.md`); never edit CHANGELOG.md.
- When the merge queue says another agent's work changes the same files as yours, agree with it \
(`send_to_agent`) who lands first before you both ship.
</software_factory>";

#[cfg(test)]
mod tests {
    #[test]
    fn the_rules_send_changelog_entries_to_fragments() {
        let rules = super::FACTORY_RULES;
        assert!(rules.contains("changelog.d/"), "{rules}");
        assert!(rules.contains("never edit CHANGELOG.md"), "{rules}");
        assert!(rules.contains("send_to_agent"), "{rules}");
        assert!(rules.contains("git_start_task"), "{rules}");
        assert!(rules.contains("never merge a PR yourself"), "{rules}");
    }
}
