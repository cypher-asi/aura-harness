//! What a chat says about plan mode and a written plan. A plan the
//! software factory builds on its own (a channel member's task) waits for
//! no one, so its notes don't ask the user to say "implement" or run
//! `/build`.

/// The note shown as an agent enters plan mode. `factory`: the software
/// factory builds and ships what it plans.
#[must_use]
pub(crate) fn plan_mode_note(factory: bool) -> String {
    if factory {
        "Plan mode: the agent researches and writes a plan instead of changing code, then \
         builds it and ships it to the merge queue on its own."
            .to_string()
    } else {
        "Plan mode: the agent researches and writes a plan instead of changing code. Say \
         \"implement\" or run /build to build the plan; /plan again returns to the previous \
         mode."
            .to_string()
    }
}

/// The note shown once plan `name` was written to `path`.
#[must_use]
pub(crate) fn plan_ready_note(name: &str, path: &str, factory: bool) -> String {
    if factory {
        format!("Plan ready: {name} ({path}). The agent builds it now, then ships it.")
    } else {
        format!(
            "Plan ready: {name} ({path}). Keep chatting to revise it, or say \"implement\" \
             (or run /build) to build it."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_factory_s_notes_ask_nothing_of_the_user() {
        for note in [plan_mode_note(true), plan_ready_note("Fix", "p.md", true)] {
            assert!(!note.contains("implement\""), "{note}");
            assert!(!note.contains("/build"), "{note}");
        }
        assert!(plan_mode_note(true).contains("on its own"));
        assert!(plan_ready_note("Fix", "p.md", true).contains("builds it now"));
    }

    #[test]
    fn a_users_own_plan_still_says_how_to_build_it() {
        for note in [plan_mode_note(false), plan_ready_note("Fix", "p.md", false)] {
            assert!(note.contains("\"implement\""), "{note}");
            assert!(note.contains("/build"), "{note}");
        }
    }
}
