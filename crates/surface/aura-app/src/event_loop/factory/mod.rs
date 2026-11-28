//! The software factory: a channel member plans its task, builds the
//! plan, and ships it into its repo's merge queue, which merges pushes
//! into main one at a time. See `docs/software-factory.md`.
//!
//! - [`flow`]: plan, build, ship, as pure stages.
//! - [`hooks`]: where the event loop applies them.
//! - [`observe`]: every push any agent makes, from its tool results.
//! - [`turn`]: what one turn did: the plan it wrote, the pushes it made.
//! - [`service`]: the repos' merge queues and their processors.
//! - [`overlap`]: warning agents whose work changes the same files.
//! - [`view`]: a queue as the QUEUE tab shows it.

pub(crate) mod flow;
pub(crate) mod hooks;
pub(crate) mod observe;
pub(crate) mod overlap;
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
- When the work is done, call `git_ship`: it commits every change, pulls main into your branch, \
pushes your branch and puts it in the merge queue. Never push main; the queue merges your \
branch into main once the build and tests pass, one merge at a time, and tells you if it fails.
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
    }
}
