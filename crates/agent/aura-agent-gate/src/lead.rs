//! What the lead agent under Gate may do itself.
//!
//! Under Gate the main agent is a lead: it reads enough to decide, hands
//! the work to subtasks Gate routes to cheaper models, and integrates
//! their results. While delegation is enforced it has no file-changing
//! tools ([`keeps_tool`]), reads and searches come back capped
//! ([`cap_read`], [`cap_search`]) so heavy reading goes to an explore
//! subtask, and `run_command` only runs checks, linters in check mode and
//! inspection commands ([`allows_command`]). Subagents are never
//! restricted this way.

use aura_config::GateConfig;
use aura_core_types::{Difficulty, PromptCategory};
use aura_model::model_catalog::{infer_tier, ModelTier};
use serde_json::Value;

use crate::checks::is_check_part;
use crate::router::Random;
use crate::stats::{GateStats, Row};

/// Whether a turn asking for `category` may be led at all: work that can
/// be handed to subtasks. A question, a small change or prose is always
/// done by the model it is routed to.
#[must_use]
pub const fn may_lead(category: PromptCategory) -> bool {
    matches!(
        category,
        PromptCategory::MultiFileChange
            | PromptCategory::Planning
            | PromptCategory::Debugging
            | PromptCategory::Research
    )
}

/// How a turn is done: led, or by its model itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Way {
    pub led: bool,
    /// Whether it was taken to learn about it rather than as the best
    /// known way
    pub explored: bool,
}

/// How a turn asking for `category` on `model` is done, given what
/// `stats` has learned of both ways on the category and model, judged on
/// success and on the cost of the whole work, the subtasks' included. A
/// turn of known `difficulty` goes by the turns of that difficulty alone
/// ([`GateStats::way_row_of`]), as leading cheap work costs little and
/// says nothing of what leading hard work costs; one of unknown
/// difficulty goes by every turn ([`GateStats::way_row`]):
///
/// 1. the cheaper of the two ways once both are proven: at least
///    `min_tries` outcomes with a success rate of at least `target`;
/// 2. else by the model itself, unless its outcomes rule that out
///    ([`Row::can_still_reach`]) and leading is proven or not ruled out
///    either. Leading proven alone does not win: the model doing the work
///    itself gets its tries until it is proven too, or ruled out;
/// 3. with probability `explore_rate`, the other way instead, unless its
///    outcomes rule it out.
///
/// Leading is unproven by default because the lead's own reading,
/// planning and checking cost about what the model doing the work itself
/// does, before any subtask runs. On a frontier model it is taken only
/// once proven cheaper, never to explore or as a fallback. Never led
/// when the category [`may_lead`] not.
pub fn choose_way(
    stats: &GateStats,
    (category, difficulty): (PromptCategory, Option<Difficulty>),
    model: &str,
    settings: &GateConfig,
    rng: &mut impl Random,
) -> Way {
    if !may_lead(category) {
        return Way {
            led: false,
            explored: false,
        };
    }
    let row = |led| match difficulty {
        Some(difficulty) => stats.way_row_of(category, difficulty, model, led),
        None => stats.way_row(category, model, led),
    };
    let proven = |row: Option<&Row>| {
        row.filter(|row| row.settled(settings.min_tries))
            .and_then(|row| Some((row.success_rate()?, row.mean_total_cost_usd()?)))
            .filter(|(rate, _)| *rate >= settings.target)
            .map(|(_, cost)| cost)
    };
    let frontier = infer_tier(model) == ModelTier::Frontier;
    let open = |led: bool| {
        !(led && frontier)
            && row(led).is_none_or(|row| row.can_still_reach(settings.target, settings.min_tries))
    };
    let led = match (proven(row(true)), proven(row(false))) {
        (Some(led), Some(direct)) if led != direct => led < direct,
        (Some(_), None) => !open(false),
        (None, Some(_)) => false,
        _ => !open(false) && open(true),
    };
    let rate = settings.explore_rate.clamp(0.0, 1.0);
    if rate > 0.0 && open(!led) && rng.unit() < rate {
        return Way {
            led: !led,
            explored: true,
        };
    }
    Way {
        led,
        explored: false,
    }
}

/// Lines of one `read_file` result the lead sees.
pub const READ_LINES: usize = 200;

/// Hits of one search the lead sees.
pub const SEARCH_HITS: usize = 50;

/// Tools that change files or commit them, which the lead hands to
/// subtasks instead.
pub const FILE_TOOLS: &[&str] = &[
    "write_file",
    "edit_file",
    "delete_file",
    "apply_patch",
    "git_commit",
    "git_commit_push",
];

/// Tools whose results are file contents, capped at [`READ_LINES`].
pub const READ_TOOLS: &[&str] = &["read_file"];

/// Tools whose results are one hit per line, capped at [`SEARCH_HITS`].
pub const SEARCH_TOOLS: &[&str] = &["search_code", "find_files"];

/// Name of the tool whose commands [`allows_command`] limits.
pub const RUN_COMMAND: &str = "run_command";

/// Whether the lead keeps the tool `name`.
#[must_use]
pub fn keeps_tool(name: &str) -> bool {
    !FILE_TOOLS.contains(&name)
}

/// Programs that may read a pipeline's output (never a file of their
/// own choosing first).
const FILTERS: &[&str] = &["head", "tail", "grep", "wc", "sort", "uniq", "cut"];

/// `git` subcommands that only look.
const GIT_LOOKS: &[&str] = &[
    "status",
    "diff",
    "log",
    "show",
    "rev-parse",
    "ls-files",
    "blame",
];

/// Whether the lead may run `command`: every command in it, as split at
/// `&&`, `||`, `;` and new lines, is a check (tests, builds, linters,
/// type checks) or an inspection command (`git status`, `git diff`,
/// `git log`, `git show`, `ls`, `pwd`, `cd`), optionally piped into
/// filters such as `tail` or `grep`. Command substitution, file
/// redirection and background jobs are refused.
#[must_use]
pub fn allows_command(command: &str) -> bool {
    if ["`", "$(", "<(", ">("].iter().any(|s| command.contains(s)) {
        return false;
    }
    let marked = command
        .replace("&&", "\u{1}")
        .replace("||", "\u{1}")
        .replace([';', '\n'], "\u{1}")
        .replace('|', "\u{2}");
    let mut any = false;
    for sequence in marked.split('\u{1}') {
        if sequence.trim().is_empty() {
            continue;
        }
        for (i, stage) in sequence.split('\u{2}').enumerate() {
            let Some(words) = stage_words(stage) else {
                return false;
            };
            let allowed = if i == 0 {
                is_check_part(words.clone()) || is_lint(&words) || is_inspection(&words)
            } else {
                words.first().is_some_and(|w| FILTERS.contains(&program(w)))
            };
            if !allowed {
                return false;
            }
        }
        any = true;
    }
    any
}

/// The words of one pipeline stage without its harmless redirections
/// (`2>&1`, `2>/dev/null`); `None` when it redirects to or from a file,
/// runs in the background, or is empty.
fn stage_words(stage: &str) -> Option<Vec<&str>> {
    let mut words = Vec::new();
    for word in stage.split_whitespace() {
        let fd_dup = word
            .trim_start_matches(|c: char| c.is_ascii_digit() || c == '&')
            .strip_prefix(">&")
            .is_some_and(|fd| !fd.is_empty() && fd.chars().all(|c| c.is_ascii_digit()));
        let to_null = word
            .trim_start_matches(|c: char| c.is_ascii_digit() || c == '&')
            .strip_prefix('>')
            .is_some_and(|target| target == "/dev/null");
        if fd_dup || to_null {
            continue;
        }
        if word == "&" || word.contains(['>', '<', '&']) {
            return None;
        }
        words.push(word);
    }
    (!words.is_empty()).then_some(words)
}

fn program(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// Whether `words` run a linter or formatter in check mode.
fn is_lint(words: &[&str]) -> bool {
    let words = match words.first() {
        Some(&"npx" | &"bunx") => &words[1..],
        _ => words,
    };
    let Some(&first) = words.first() else {
        return false;
    };
    let checking = words.contains(&"--check");
    match program(first) {
        "eslint" | "flake8" | "pylint" | "golangci-lint" | "shellcheck" | "hadolint" => true,
        "ruff" => words.get(1) == Some(&"check"),
        "cargo" => words.get(1) == Some(&"fmt") && checking,
        "black" | "prettier" | "rustfmt" | "gofmt" => checking || words.contains(&"-l"),
        _ => false,
    }
}

/// Whether `words` only look at the workspace or its history.
fn is_inspection(words: &[&str]) -> bool {
    let Some(&first) = words.first() else {
        return false;
    };
    match program(first) {
        "ls" | "pwd" | "cd" => true,
        "git" => {
            let mut rest = words[1..].iter();
            while let Some(&word) = rest.next() {
                match word {
                    "-C" | "-c" => {
                        rest.next();
                    }
                    flag if flag.starts_with('-') => {}
                    sub => {
                        return GIT_LOOKS.contains(&sub)
                            && rest.all(|w| !w.starts_with("--output"));
                    }
                }
            }
            false
        }
        _ => false,
    }
}

/// What the lead is told when it runs a command it may not.
#[must_use]
pub fn refused_command(command: &str) -> String {
    format!(
        "Not run: under Gate your own run_command only runs checks (tests, builds, linters, \
         type checks) and inspection commands (git status, git diff, git log, git show, ls, \
         pwd). Delegate `{command}` with the task tool: an implement subtask for changes, \
         an explorer subtask for reading."
    )
}

/// What the lead is told when it calls a file-changing tool.
#[must_use]
pub fn refused_tool(tool: &str) -> String {
    format!(
        "Not run: under Gate you do not change files yourself. Delegate the edit with the \
         task tool (an implement subtask, with a check command); `{tool}` is for subagents."
    )
}

/// The command line of a `run_command` call with arguments `input`.
#[must_use]
pub fn command_of(input: &Value) -> String {
    crate::checks::command_line(input)
}

/// `content` cut to its first `limit` lines, with a note naming what was
/// left out; `None` when it fits.
fn cap_lines(content: &str, limit: usize, what: &str, advice: &str) -> Option<String> {
    let total = content.lines().count();
    if total <= limit {
        return None;
    }
    let kept: Vec<&str> = content.lines().take(limit).collect();
    Some(format!(
        "{}\n… [{what} capped at {limit} for the lead; {} more left out. {advice}]",
        kept.join("\n"),
        total - limit,
    ))
}

/// A `read_file` result cut to [`READ_LINES`] lines; `None` when it fits.
#[must_use]
pub fn cap_read(content: &str) -> Option<String> {
    cap_lines(
        content,
        READ_LINES,
        "lines",
        "Read a narrower start_line/end_line range, or delegate heavy reading to an explorer \
         subtask.",
    )
}

/// A search result cut to [`SEARCH_HITS`] hits; `None` when it fits.
#[must_use]
pub fn cap_search(content: &str) -> Option<String> {
    cap_lines(
        content,
        SEARCH_HITS,
        "hits",
        "Narrow the search, or delegate the search to an explorer subtask.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core_types::{
        AgentId, OutcomePayload, OutcomeSignal, PromptPayload, RecordEntry, Route, RouteReason,
        Transaction,
    };
    use serde_json::json;

    mod ways {
        use super::*;

        const SONNET: &str = "claude/claude-sonnet-4-6";
        const OPUS: &str = "claude/claude-opus-4-7";

        /// `n` turns on Sonnet asking for `category`, led or not, that
        /// all passed or all failed, each costing `cost_usd` in the lead
        /// and `subtask_cost_usd` in subtasks.
        fn turns(
            entries: &mut Vec<RecordEntry>,
            n: usize,
            category: PromptCategory,
            led: bool,
            ok: bool,
            costs: (f64, f64),
        ) {
            turns_of(entries, n, (category, None), led, ok, costs);
        }

        /// [`turns`] the classifier judged of `difficulty`.
        fn turns_of(
            entries: &mut Vec<RecordEntry>,
            n: usize,
            (category, difficulty): (PromptCategory, Option<Difficulty>),
            led: bool,
            ok: bool,
            (cost_usd, subtask_cost_usd): (f64, f64),
        ) {
            let signal = if ok {
                OutcomeSignal::ChecksPassed
            } else {
                OutcomeSignal::ChecksFailed
            };
            for _ in 0..n {
                let agent = AgentId::generate();
                let mut prompt = PromptPayload::new(format!("turn {}", entries.len()));
                prompt.category = Some(category);
                prompt.route = Route::turn(SONNET, RouteReason::Default);
                prompt.led = Some(led);
                prompt.difficulty = difficulty;
                let prompt = Transaction::user_prompt_with(agent, &prompt);
                let outcome = OutcomePayload {
                    prompt_tx: prompt.hash,
                    category,
                    model: SONNET.to_string(),
                    success: signal.success(),
                    signal,
                    cost_usd,
                    subtask_cost_usd,
                    latency_ms: 10,
                };
                for tx in [prompt, Transaction::outcome(agent, &outcome)] {
                    let seq = u64::try_from(entries.len()).unwrap() + 1;
                    entries.push(RecordEntry::builder(seq, tx).build());
                }
            }
        }

        fn settings(explore_rate: f64) -> GateConfig {
            GateConfig {
                explore_rate,
                ..GateConfig::defaults()
            }
        }

        fn way(entries: &[RecordEntry], category: PromptCategory, roll: f64) -> Way {
            let stats = GateStats::from_entries(entries);
            choose_way(
                &stats,
                (category, None),
                SONNET,
                &settings(0.05),
                &mut || roll,
            )
        }

        const KEPT: f64 = 0.99;
        const EXPLORED: f64 = 0.0;

        #[test]
        fn with_nothing_learned_the_model_does_the_work() {
            for category in PromptCategory::CLASSES {
                assert!(!way(&[], category, KEPT).led, "{category}");
            }
            let tried = way(&[], PromptCategory::MultiFileChange, EXPLORED);
            assert_eq!((tried.led, tried.explored), (true, true));
            let never = way(&[], PromptCategory::Question, EXPLORED);
            assert_eq!((never.led, never.explored), (false, false));
        }

        #[test]
        fn a_frontier_model_leads_only_once_leading_is_proven_cheaper() {
            let category = PromptCategory::Planning;
            let on_opus = |stats: &GateStats, roll: f64| {
                choose_way(stats, (category, None), OPUS, &settings(0.05), &mut || roll)
            };
            let empty = GateStats::default();
            let kept = on_opus(&empty, EXPLORED);
            assert_eq!((kept.led, kept.explored), (false, false), "never explored");
            assert!(!on_opus(&empty, KEPT).led);
        }

        #[test]
        fn of_two_proven_ways_the_cheaper_in_total_wins() {
            let category = PromptCategory::MultiFileChange;
            let mut entries = Vec::new();
            turns(&mut entries, 10, category, true, true, (0.10, 0.50));
            turns(&mut entries, 10, category, false, true, (0.40, 0.0));
            assert!(!way(&entries, category, KEPT).led, "0.40 beats 0.60");

            let mut entries = Vec::new();
            turns(&mut entries, 10, category, true, true, (0.10, 0.20));
            turns(&mut entries, 10, category, false, true, (0.40, 0.0));
            assert!(way(&entries, category, KEPT).led, "0.30 beats 0.40");
        }

        #[test]
        fn proven_leading_waits_until_working_directly_is_proven_or_ruled_out() {
            let category = PromptCategory::Debugging;
            let mut entries = Vec::new();
            turns(&mut entries, 10, category, true, true, (0.1, 0.1));
            assert!(
                !way(&entries, category, KEPT).led,
                "only the led way is proven: working directly gets its tries"
            );
            turns(&mut entries, 9, category, false, true, (0.4, 0.0));
            assert!(!way(&entries, category, KEPT).led, "not proven yet");
            turns(&mut entries, 1, category, false, true, (0.4, 0.0));
            assert!(way(&entries, category, KEPT).led, "0.20 beats 0.40");

            let mut entries = Vec::new();
            turns(&mut entries, 10, category, true, true, (1.0, 1.0));
            turns(&mut entries, 3, category, false, false, (0.1, 0.0));
            assert!(
                way(&entries, category, KEPT).led,
                "working directly is ruled out"
            );

            let category = PromptCategory::MultiFileChange;
            let mut entries = Vec::new();
            turns(&mut entries, 3, category, false, false, (0.1, 0.0));
            assert!(
                way(&entries, category, KEPT).led,
                "3 failures rule working directly out"
            );
            turns(&mut entries, 3, category, true, false, (0.1, 0.1));
            assert!(
                !way(&entries, category, KEPT).led,
                "both ruled out: the model does the work"
            );
        }

        #[test]
        fn leading_proven_cheaper_on_easy_work_says_nothing_of_hard_work() {
            let category = PromptCategory::MultiFileChange;
            let (easy, hard) = (Some(Difficulty::Easy), Some(Difficulty::Hard));
            let mut entries = Vec::new();
            turns_of(&mut entries, 10, (category, easy), true, true, (0.06, 0.03));
            turns_of(&mut entries, 10, (category, hard), false, true, (0.50, 0.0));
            let stats = GateStats::from_entries(&entries);
            let way = |difficulty| {
                choose_way(
                    &stats,
                    (category, difficulty),
                    SONNET,
                    &settings(0.0),
                    &mut || KEPT,
                )
                .led
            };
            assert!(!way(hard), "nothing proves leading hard work cheaper");
            assert!(way(None), "over every turn, leading looks cheaper");

            turns_of(&mut entries, 10, (category, hard), true, true, (0.30, 0.40));
            let stats = GateStats::from_entries(&entries);
            assert!(
                !choose_way(
                    &stats,
                    (category, hard),
                    SONNET,
                    &settings(0.0),
                    &mut || KEPT
                )
                .led,
                "led hard work costs 0.70 in all, done directly 0.50"
            );
        }

        #[test]
        fn exploring_never_takes_a_ruled_out_way() {
            let category = PromptCategory::Research;
            let mut entries = Vec::new();
            turns(&mut entries, 3, category, true, false, (0.1, 0.1));
            let way = way(&entries, category, EXPLORED);
            assert_eq!((way.led, way.explored), (false, false));
        }
    }

    #[test]
    fn the_lead_keeps_every_tool_but_the_file_changing_ones() {
        for tool in ["write_file", "edit_file", "delete_file", "git_commit"] {
            assert!(!keeps_tool(tool), "{tool}");
        }
        for tool in [
            "read_file",
            "search_code",
            "run_command",
            "task",
            "todo_write",
            "create_plan",
        ] {
            assert!(keeps_tool(tool), "{tool}");
        }
    }

    #[test]
    fn checks_and_inspection_commands_are_allowed() {
        for command in [
            "cargo test -p aura-agent-gate",
            "cargo clippy --workspace -- -D warnings",
            "npm test",
            "pytest -x tests/",
            "git status",
            "git diff --stat",
            "git -C crates/foo log --oneline -5",
            "git --no-pager show HEAD",
            "ls -la src",
            "pwd",
            "cd crates/foo && cargo test",
            "cargo test 2>&1 | tail -20",
            "cargo build 2>/dev/null | grep -c warning",
            "git diff; git status",
            "cargo fmt --all -- --check",
            "npx eslint src",
            "ruff check .",
        ] {
            assert!(allows_command(command), "{command}");
        }
    }

    #[test]
    fn other_commands_are_refused() {
        for command in [
            "cat src/main.rs",
            "rm -rf target",
            "sed -i s/a/b/ src/lib.rs",
            "echo hi > notes.txt",
            "cargo test > out.log",
            "git commit -am wip",
            "git checkout -- src",
            "git branch -D main",
            "git diff --output=patch.diff",
            "cargo test && rm -rf src",
            "cargo test $(rm -rf src)",
            "cargo test `whoami`",
            "npm install",
            "cargo run",
            "cargo fmt",
            "ruff format .",
            "head -n 500 src/lib.rs",
            "ls | xargs rm",
            "cargo test &",
            "",
            "  ;  ",
        ] {
            assert!(!allows_command(command), "{command}");
        }
    }

    #[test]
    fn the_command_line_comes_from_any_argument_form() {
        assert_eq!(
            command_of(&json!({"program": "git", "args": ["status"]})),
            "git status"
        );
        assert_eq!(
            command_of(&json!({"shell_script": "cargo test | tail"})),
            "cargo test | tail"
        );
        assert!(refused_command("rm x").contains("Delegate `rm x`"));
        assert!(refused_tool("write_file").contains("implement subtask"));
    }

    #[test]
    fn long_reads_and_searches_are_capped_with_a_note() {
        let file: String = (1..=250).map(|n| format!("line {n}\n")).collect();
        let capped = cap_read(&file).unwrap();
        assert_eq!(capped.lines().count(), READ_LINES + 1);
        assert!(
            capped.contains("line 200\n… [lines capped at 200"),
            "{capped}"
        );
        assert!(capped.contains("50 more left out"));
        assert!(capped.contains("explorer subtask"));
        assert_eq!(cap_read("short\nfile"), None);

        let hits: String = (1..=60).map(|n| format!("src/a.rs:{n}: x\n")).collect();
        let capped = cap_search(&hits).unwrap();
        assert_eq!(capped.lines().count(), SEARCH_HITS + 1);
        assert!(capped.contains("10 more left out"));
        assert_eq!(cap_search("one hit"), None);
    }
}
