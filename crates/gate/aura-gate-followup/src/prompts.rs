//! What the model is told when Gate sends it back to its work, or the
//! work on to another attempt.

use aura_core_types::OutcomeSignal;
use aura_gate_evidence::transcript::said::UNCOVERED_OPENING;
use aura_gate_evidence::Flag;

/// How every message that sends work back to its model starts; tells
/// them apart from the user's prompts.
pub const REPAIR_OPENING: &str = "Before your work was accepted, ";

/// How every message that sends the model back to its work ends. The
/// reply it then writes is the one the user reads, and the one a judge
/// weighs, so it has to be the whole answer rather than a report on the
/// check alone.
const FULL_ANSWER: &str = "Then write your final answer in full, as the reply the user will \
read: what you changed and why, and what you ran to check it and how it went.";

/// What the model is told when a review found its work unfinished.
#[must_use]
pub fn repair_prompt(findings: &str) -> String {
    format!(
        "{REPAIR_OPENING}a reviewer went through your change against the request and found it \
         unfinished:\n\n{}\n\nFix what it found. If a finding is wrong, say why and leave that \
         part as it is. Run the tests that cover the change (the way this project or task says \
         commands run). {FULL_ANSWER}",
        findings.trim()
    )
}

/// What the model is told when `signs` alone send its work back.
#[must_use]
pub fn sign_repair_prompt(signs: &[Flag]) -> String {
    let listed: Vec<String> = signs
        .iter()
        .map(|sign| format!("- {}", sign.describe()))
        .collect();
    format!(
        "{REPAIR_OPENING}the harness found signs that it is not finished:\n{}\n\nGo back over \
         the request and your change, and fix what is missing or wrong. Run the tests that cover \
         the change (the way this project or task says commands run). {FULL_ANSWER}",
        listed.join("\n")
    )
}

/// What the model is told when it ended a turn having changed files
/// without running a test or build. A project with no tests is checked
/// by running the changed code: test files there are a change nobody
/// asked for.
pub const NUDGE_PROMPT: &str = "\
You changed files but did not run any test or build to check the change. Before you finish, run \
the tests that cover what you changed (run them the way this project or task says commands run, \
for example through its own wrapper script), and fix anything that fails. If no test covers it, \
run the build or type check, or run the changed code on a few cases from the request with a short \
script; if the project has no tests, do not add test files the request did not ask for. Then \
write your final answer in full, as the reply the user will read: what you changed and why, and \
what you ran to check it and how it went.";

/// Names [`uncovered_prompt`] lists.
const UNCOVERED_LISTED: usize = 8;

/// What the model is told when the tests it ran passed while none names
/// `names`, what its change added.
#[must_use]
pub fn uncovered_prompt(names: &[String]) -> String {
    let mut listed: Vec<String> = names
        .iter()
        .take(UNCOVERED_LISTED)
        .map(|name| format!("`{name}`"))
        .collect();
    if names.len() > UNCOVERED_LISTED {
        listed.push(format!("{} more", names.len() - UNCOVERED_LISTED));
    }
    format!(
        "{UNCOVERED_OPENING}{}, which your change added, so they never ran the new code. Before you \
         finish, write tests that call it and check what it returns against what the request \
         asks for, taking the expected values from the request (including the exact format of \
         any output it specifies) rather than from what your code prints. Run them the way this \
         project or task says commands run and fix anything that fails. {FULL_ANSWER}",
        listed.join(", ")
    )
}

/// What the model that goes on with a turn is told, the work on `model`
/// having ended with `signal`, with what the judge found wrong when it
/// reviewed the work: a `stronger` model that it takes over, or the same
/// model, reasoning harder, that it goes over its work again. The
/// conversation and the files stay as that work left them.
#[must_use]
pub fn takeover_prompt(
    model: &str,
    stronger: bool,
    signal: OutcomeSignal,
    review: Option<&str>,
) -> String {
    let how = match signal {
        OutcomeSignal::ChecksFailed => "with its last test or build command failing",
        OutcomeSignal::JudgedFail => "without checking its work, which looked unfinished",
        _ => "in an error",
    };
    let review = review
        .map(str::trim)
        .filter(|review| !review.is_empty())
        .map(|review| format!(" A reviewer of that work found: {review}"))
        .unwrap_or_default();
    if !stronger {
        return format!(
            "The work above ended {how}.{review} No stronger model is left to take it over, so \
             go over it once more yourself, reasoning harder this time. The files are as that \
             work left them: check what was changed, find what is wrong and finish the request."
        );
    }
    format!(
        "The work above was done on {model} and ended {how}.{review} You are a stronger model \
         taking over this turn. The files are as that work left them: check what it changed, \
         find what is wrong and finish the request."
    )
}

/// What the model a subtask is retried on is told: `prompt`, and that
/// the first attempt on `model` failed with `why`, its file changes
/// undone.
#[must_use]
pub fn retry_prompt(prompt: &str, model: &str, why: &str) -> String {
    format!(
        "{prompt}\n\n---\nA first attempt at this task on {model} failed: {why}\n\
         Its file changes were undone, so start again from the files as they are now."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_gate_evidence::transcript::said::{is_nudge, NUDGE_OPENING};

    #[test]
    fn a_takeover_says_whether_the_model_is_a_stronger_one() {
        let stronger = takeover_prompt(
            "claude-sonnet-5-5 (claude plan)",
            true,
            OutcomeSignal::ChecksFailed,
            Some("the new test fails"),
        );
        assert!(stronger.starts_with(
            "The work above was done on claude-sonnet-5-5 (claude plan) and ended with its last \
             test or build command failing. A reviewer of that work found: the new test fails"
        ));
        assert!(stronger.contains("You are a stronger model taking over this turn."));
        let same = takeover_prompt(
            "claude-opus-5-5 (claude plan)",
            false,
            OutcomeSignal::JudgedFail,
            None,
        );
        assert!(same.starts_with(
            "The work above ended without checking its work, which looked unfinished. No \
             stronger model is left"
        ));
        assert!(!same.contains("You are a stronger model"), "{same}");
    }

    #[test]
    fn a_retry_repeats_the_prompt_and_says_the_first_attempt_failed() {
        assert_eq!(
            retry_prompt("Fix it", "m", "boom"),
            "Fix it\n\n---\nA first attempt at this task on m failed: boom\nIts file changes \
             were undone, so start again from the files as they are now."
        );
    }

    #[test]
    fn what_goes_back_to_the_model_says_what_was_found() {
        let told = repair_prompt("1. `int` accepts `'1.1'`.\n");
        assert!(told.starts_with(REPAIR_OPENING), "{told}");
        assert!(
            told.contains("1. `int` accepts `'1.1'`.\n\nFix what it found"),
            "{told}"
        );
        let signs = sign_repair_prompt(&[Flag::StaleCheck, Flag::ShellEdit]);
        assert!(signs.starts_with(REPAIR_OPENING), "{signs}");
        assert!(signs.contains(Flag::StaleCheck.describe()), "{signs}");
        assert!(signs.contains(Flag::ShellEdit.describe()), "{signs}");
    }

    #[test]
    fn whatever_sends_the_model_back_asks_for_its_whole_answer_again() {
        let names = ["prepare_multipart".to_string()];
        for told in [
            NUDGE_PROMPT.to_string(),
            uncovered_prompt(&names),
            repair_prompt("1. it is wrong"),
            sign_repair_prompt(&[Flag::ShellEdit]),
        ] {
            assert!(told.ends_with(FULL_ANSWER), "{told}");
        }
        assert!(NUDGE_PROMPT.starts_with(NUDGE_OPENING));
        assert!(
            NUDGE_PROMPT.contains("if the project has no tests, do not add test files"),
            "{NUDGE_PROMPT}"
        );
    }

    #[test]
    fn a_message_naming_what_no_test_names_is_read_as_a_nudge() {
        let told = uncovered_prompt(&["prepare_multipart".to_string()]);
        assert!(told.starts_with(UNCOVERED_OPENING), "{told}");
        assert!(told.contains("`prepare_multipart`"), "{told}");
        assert!(is_nudge(&told) && is_nudge(NUDGE_PROMPT));
        assert!(!is_nudge(&repair_prompt("1. it is wrong")));
    }

    /// What the model is told goes to it as it is, so its bytes are
    /// pinned.
    #[test]
    fn the_messages_to_the_model_are_pinned() {
        assert_eq!(
            NUDGE_PROMPT,
            "You changed files but did not run any test or build to check the change. Before \
             you finish, run the tests that cover what you changed (run them the way this \
             project or task says commands run, for example through its own wrapper script), \
             and fix anything that fails. If no test covers it, run the build or type check, or \
             run the changed code on a few cases from the request with a short script; if the \
             project has no tests, do not add test files the request did not ask for. Then \
             write your final answer in full, as the reply the user will read: what you changed \
             and why, and what you ran to check it and how it went."
        );
        assert_eq!(
            repair_prompt("\n1. `int` accepts `'1.1'`.\n2. The docs are stale.\n\n"),
            "Before your work was accepted, a reviewer went through your change against the \
             request and found it unfinished:\n\n1. `int` accepts `'1.1'`.\n2. The docs are \
             stale.\n\nFix what it found. If a finding is wrong, say why and leave that part as \
             it is. Run the tests that cover the change (the way this project or task says \
             commands run). Then write your final answer in full, as the reply the user will \
             read: what you changed and why, and what you ran to check it and how it went."
        );
    }

    /// The names of `n` definitions a change added.
    fn added(n: usize) -> Vec<String> {
        (1..=n).map(|at| format!("name_{at}")).collect()
    }

    /// What the model is told of what no test names, after the names.
    const UNCOVERED_REST: &str = ", which your change added, so they never ran the new code. \
        Before you finish, write tests that call it and check what it returns against what the \
        request asks for, taking the expected values from the request (including the exact \
        format of any output it specifies) rather than from what your code prints. Run them \
        the way this project or task says commands run and fix anything that fails. Then write \
        your final answer in full, as the reply the user will read: what you changed and why, \
        and what you ran to check it and how it went.";

    #[test]
    fn the_message_naming_what_no_test_names_is_pinned() {
        let eight = "`name_1`, `name_2`, `name_3`, `name_4`, `name_5`, `name_6`, `name_7`, \
                     `name_8`";
        for (names, listed) in [
            (1, "`name_1`".to_string()),
            (8, eight.to_string()),
            (10, format!("{eight}, 2 more")),
        ] {
            assert_eq!(
                uncovered_prompt(&added(names)),
                format!("The tests you ran passed, but no test names {listed}{UNCOVERED_REST}")
            );
        }
    }

    /// The signs a repair lists are told the model as they are, so their
    /// words are pinned.
    #[test]
    fn a_repair_for_every_sign_is_pinned() {
        assert_eq!(
            sign_repair_prompt(&Flag::ALL),
            "Before your work was accepted, the harness found signs that it is not finished:\n\
             - the last test or build ran through a pipe or a chain of commands, so its exit \
             status is not the check's own\n\
             - the last test or build exited with success, but what it printed shows failures \
             or errors\n\
             - the last test or build printed failures or errors, and its exit status was \
             another command's\n\
             - the final reply says the work was not verified or not tested\n\
             - the final reply says tests still fail\n\
             - no test or build was run to its end\n\
             - the request asks for a code change, but no file was changed\n\
             - the final reply says the work could not be done or was left unfinished\n\
             - the attempt stopped on a limit (iterations, budget or a stall) instead of \
             finishing\n\
             - files were changed by shell commands rather than the file tools\n\
             - a file was changed after the last test or build ran\n\
             - the final reply says the tests that were run do not cover the change\n\
             - no test was added or changed\n\
             - the last test or build ran only tests in files the attempt wrote itself, so no \
             test it did not write checked the work\n\n\
             Go back over the request and your change, and fix what is missing or wrong. Run \
             the tests that cover the change (the way this project or task says commands run). \
             Then write your final answer in full, as the reply the user will read: what you \
             changed and why, and what you ran to check it and how it went."
        );
    }
}
