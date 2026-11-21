//! Gate's attempt log: a local file of what each attempt at a prompt
//! left behind and of what later proved it right or wrong, kept so Gate's
//! verdicts, and any new way of reaching one, can be scored against what
//! happened.
//!
//! The file is `gate/attempts.jsonl` under the data directory, one JSON
//! object per line, appended to and never rewritten. It stays on this
//! machine: the data directory is not under version control. A line is
//! one of two kinds, told apart by its `kind`:
//!
//! - `attempt` ([`Attempt`]): one model's work on a prompt, written when
//!   its `Outcome` is recorded: what Gate made of it, and its
//!   [`Evidence`]
//! - `truth` ([`Truth`]): something outside the attempt that says whether
//!   its work was right: a benchmark task's own check, or the user
//!   rewinding it, sending it again to a stronger model, or picking
//!   between runs
//!
//! A truth belongs to the attempt with the same `agent` and `prompt_tx`.
//! A turn Gate moves to a stronger model is a new attempt under the
//! prompt recorded for the move, so the pair names one attempt.
//!
//! [`evaluate`] joins the two and reports how often Gate let failed work
//! stand, and for each [`Flag`] how many of those failures it would have
//! caught against how many good attempts it would have questioned.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use aura_core_types::{Difficulty, OutcomePayload, OutcomeSignal, PromptCategory};
use serde::{Deserialize, Serialize};

use crate::evidence::{flags, Evidence, Flag};

/// The log's path under the data directory.
pub const FILE: &str = "gate/attempts.jsonl";

/// One model's work on a prompt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attempt {
    /// When the line was written, in Unix milliseconds
    pub at_ms: u64,
    /// The agent that ran it, in hex
    pub agent: String,
    /// Hash of the prompt it answers, in hex: the user's, or the one
    /// recorded when Gate moved the turn to this model
    pub prompt_tx: String,
    /// What the prompt asked for
    pub category: PromptCategory,
    /// How hard the classifier judged it, when it said
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<Difficulty>,
    /// Whether the agent led it, handing its work to subtasks
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led: Option<bool>,
    /// Whether Gate chose the model
    pub routed: bool,
    /// The model that did the work
    pub model: String,
    /// What Gate recorded of how it went, after any follow-up
    pub signal: OutcomeSignal,
    /// Whether that counts as success; absent when nothing said
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    /// List-price cost of its model calls in USD
    pub cost_usd: f64,
    /// List-price cost of its subtasks' model calls in USD
    #[serde(default)]
    pub subtask_cost_usd: f64,
    /// Its wall time
    pub latency_ms: u64,
    /// What it left behind
    pub evidence: Evidence,
    /// Whether the line was rebuilt from the record and a saved session
    /// rather than written as the attempt ended
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub backfilled: bool,
}

/// What an attempt ran under, the part its `Outcome` does not hold.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Terms {
    /// How hard the classifier judged the prompt
    pub difficulty: Option<Difficulty>,
    /// Whether the agent led it
    pub led: Option<bool>,
    /// Whether Gate chose the model
    pub routed: bool,
}

impl Attempt {
    /// The attempt of `agent` (in hex) whose outcome is `outcome`.
    #[must_use]
    pub fn new(
        at_ms: u64,
        agent: String,
        outcome: &OutcomePayload,
        terms: Terms,
        evidence: Evidence,
    ) -> Self {
        Self {
            at_ms,
            agent,
            prompt_tx: outcome.prompt_tx.to_hex(),
            category: outcome.category,
            difficulty: terms.difficulty,
            led: terms.led,
            routed: terms.routed,
            model: outcome.model.clone(),
            signal: outcome.signal,
            success: outcome.success,
            cost_usd: outcome.cost_usd,
            subtask_cost_usd: outcome.subtask_cost_usd,
            latency_ms: outcome.latency_ms,
            evidence,
            backfilled: false,
        }
    }
}

/// What said whether an attempt's work was right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TruthSource {
    /// A benchmark task's own check, run on the work after the run
    TaskCheck,
    /// The user rewound the turn
    Rewind,
    /// The user sent the prompt again to a stronger model
    RetryStronger,
    /// The user picked between two runs of `/compare`
    Picked,
}

impl TruthSource {
    /// The source an `Outcome` with `signal` is, when the signal is the
    /// user's.
    #[must_use]
    pub const fn of_user(signal: OutcomeSignal) -> Option<Self> {
        match signal {
            OutcomeSignal::Rewind => Some(Self::Rewind),
            OutcomeSignal::RetryStronger => Some(Self::RetryStronger),
            OutcomeSignal::Good | OutcomeSignal::Bad => Some(Self::Picked),
            _ => None,
        }
    }

    /// The task's own check outranks what the user did, which says less
    /// about the work itself.
    const fn rank(self) -> u8 {
        match self {
            Self::TaskCheck => 2,
            Self::Rewind | Self::RetryStronger | Self::Picked => 1,
        }
    }
}

/// Something outside an attempt that says whether its work was right.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Truth {
    /// When the line was written, in Unix milliseconds
    pub at_ms: u64,
    /// The attempt's agent, in hex
    pub agent: String,
    /// The attempt's prompt, in hex
    pub prompt_tx: String,
    /// What said so
    pub source: TruthSource,
    /// Whether the work was right
    pub passed: bool,
    /// The comparison the attempt's run belongs to, e.g. `Compare #22`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compare: Option<String>,
    /// The run, e.g. `a-4`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// The benchmark task the run did
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

/// A line of the log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Line {
    /// See [`Attempt`]
    Attempt(Attempt),
    /// See [`Truth`]
    Truth(Truth),
}

impl Line {
    /// The agent and prompt the line is about, and whether it is an
    /// attempt.
    #[must_use]
    pub fn key(&self) -> (&str, &str, bool) {
        match self {
            Self::Attempt(a) => (&a.agent, &a.prompt_tx, true),
            Self::Truth(t) => (&t.agent, &t.prompt_tx, false),
        }
    }
}

/// The log file, appended to one line at a time.
#[derive(Debug)]
pub struct AttemptLog {
    path: PathBuf,
    /// Held while a line is written, so lines of concurrent turns do not
    /// interleave
    writing: Mutex<()>,
}

impl AttemptLog {
    /// The log at `path`.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            writing: Mutex::new(()),
        }
    }

    /// The log of the data directory `data_dir`.
    #[must_use]
    pub fn in_data_dir(data_dir: &Path) -> Self {
        Self::new(data_dir.join(FILE))
    }

    /// Where the log is.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append `line`.
    ///
    /// # Errors
    ///
    /// When the file or its directory cannot be written.
    pub fn append(&self, line: &Line) -> io::Result<()> {
        let mut text = serde_json::to_string(line)?;
        text.push('\n');
        let _held = self.writing.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?
            .write_all(text.as_bytes())
    }

    /// The log's lines, oldest first; none when there is no file yet.
    ///
    /// # Errors
    ///
    /// When the file cannot be read.
    pub fn read(&self) -> io::Result<Read> {
        read(&self.path)
    }
}

/// The lines of a log, and how many of its lines could not be read as
/// one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Read {
    /// The lines, oldest first
    pub lines: Vec<Line>,
    /// Lines that are not a line of the log
    pub unreadable: usize,
}

/// The lines of the log at `path`, oldest first; none when there is no
/// file.
///
/// # Errors
///
/// When the file cannot be read.
pub fn read(path: &Path) -> io::Result<Read> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Read::default()),
        Err(e) => return Err(e),
    };
    let mut out = Read::default();
    for text in BufReader::new(file).lines() {
        let text = text?;
        if text.trim().is_empty() {
            continue;
        }
        match serde_json::from_str(&text) {
            Ok(line) => out.lines.push(line),
            Err(_) => out.unreadable += 1,
        }
    }
    Ok(out)
}

/// How Gate's verdicts on a set of attempts held up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Verdicts {
    /// Attempts whose work was later shown right or wrong
    pub known: u32,
    /// Of those, the ones whose work was right
    pub right: u32,
    /// Gate counted the attempt as passed, and its work was wrong
    pub false_passes: u32,
    /// Gate had nothing to judge the attempt by, and its work was wrong
    pub unjudged_failures: u32,
    /// Gate counted the attempt as failed, and its work was right
    pub false_failures: u32,
}

impl Verdicts {
    /// Failed work Gate let stand.
    #[must_use]
    pub const fn missed(self) -> u32 {
        self.false_passes + self.unjudged_failures
    }

    fn take(&mut self, said: Option<bool>, right: bool) {
        self.known += 1;
        self.right += u32::from(right);
        match (said, right) {
            (Some(true), false) => self.false_passes += 1,
            (None, false) => self.unjudged_failures += 1,
            (Some(false), true) => self.false_failures += 1,
            _ => {}
        }
    }
}

/// How a [`Flag`] did on the attempts Gate let stand whose work was
/// later shown right or wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FlagScore {
    /// The flag
    pub flag: Flag,
    /// Attempts it was raised on
    pub raised: u32,
    /// Of those, the ones whose work was wrong: failures it would have
    /// caught
    pub caught: u32,
    /// Of those, the ones whose work was right: good attempts it would
    /// have questioned
    pub false_alarms: u32,
}

impl FlagScore {
    /// Share of the attempts it was raised on whose work was wrong.
    #[must_use]
    pub fn precision(self) -> Option<f64> {
        (self.raised > 0).then(|| f64::from(self.caught) / f64::from(self.raised))
    }
}

/// An attempt Gate let stand whose work was later shown right or wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Scored {
    /// The comparison and run it belongs to, when it has one, e.g.
    /// `Compare #22 a-4`
    pub run: String,
    /// The benchmark task it did, when it did one
    pub task: String,
    /// The model that did the work
    pub model: String,
    /// Whether Gate chose the model
    pub routed: bool,
    /// What Gate recorded of how it went
    pub signal: OutcomeSignal,
    /// Whether the work was right
    pub right: bool,
    /// What said so
    pub source: TruthSource,
    /// The flags its evidence raises
    pub flags: Vec<Flag>,
}

/// What the log says of Gate's verdicts and of each [`Flag`].
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Report {
    /// Attempts in the log, counted once each
    pub attempts: u32,
    /// Attempts Gate chose the model of
    pub routed: u32,
    /// How Gate's verdicts held up, over every attempt
    pub verdicts: Verdicts,
    /// The same per model, over the attempts Gate let stand
    pub models: BTreeMap<String, Verdicts>,
    /// Attempts Gate let stand whose work was later shown right or wrong:
    /// what the flags are scored on
    pub stood: u32,
    /// Of those, the ones whose work was wrong
    pub stood_wrong: u32,
    /// Each flag's score, in [`Flag::ALL`]'s order
    pub flags: Vec<FlagScore>,
    /// The attempts the flags are scored on, oldest first
    pub scored: Vec<Scored>,
}

impl Report {
    /// The failed attempts Gate let stand, oldest first.
    pub fn missed(&self) -> impl Iterator<Item = &Scored> {
        self.scored.iter().filter(|scored| !scored.right)
    }
}

/// Which attempts a report is about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// Only attempts Gate chose the model of
    pub routed: bool,
    /// Only attempts Gate counted as passed
    pub passed: bool,
    /// Only attempts at a benchmark task whose id contains this
    pub task: Option<String>,
}

impl Filter {
    fn takes(&self, attempt: &Attempt, truth: Option<&Truth>) -> bool {
        let task = self.task.as_deref().is_none_or(|wanted| {
            truth
                .and_then(|truth| truth.task.as_deref())
                .is_some_and(|task| task.contains(wanted))
        });
        task && (attempt.routed || !self.routed) && (attempt.success == Some(true) || !self.passed)
    }
}

/// Score Gate's verdicts and each flag on every attempt in `lines`; see
/// [`evaluate_where`].
#[must_use]
pub fn evaluate(lines: &[Line]) -> Report {
    evaluate_where(lines, &Filter::default())
}

/// Score Gate's verdicts and each flag on the attempts in `lines` that
/// `filter` takes. An attempt logged more than once counts once: as it
/// was last logged while the app ran (a follow-up that changes what Gate
/// makes of an attempt logs it again), else as it was rebuilt. Of several
/// truths about one attempt, a task's own check outranks what the user
/// did, and the later of equals counts.
#[must_use]
pub fn evaluate_where(lines: &[Line], filter: &Filter) -> Report {
    type Key<'a> = (&'a str, &'a str);
    let mut attempts: Vec<&Attempt> = Vec::new();
    let mut at: HashMap<Key<'_>, usize> = HashMap::new();
    let mut truths: HashMap<Key<'_>, &Truth> = HashMap::new();
    for line in lines {
        match line {
            Line::Attempt(attempt) => {
                let key = (attempt.agent.as_str(), attempt.prompt_tx.as_str());
                match at.get(&key) {
                    Some(&i) if !attempt.backfilled => attempts[i] = attempt,
                    Some(_) => {}
                    None => {
                        at.insert(key, attempts.len());
                        attempts.push(attempt);
                    }
                }
            }
            Line::Truth(truth) => {
                let key = (truth.agent.as_str(), truth.prompt_tx.as_str());
                let outranked = truths
                    .get(&key)
                    .is_some_and(|held| held.source.rank() > truth.source.rank());
                if !outranked {
                    truths.insert(key, truth);
                }
            }
        }
    }

    attempts.retain(|attempt| {
        let truth = truths.get(&(attempt.agent.as_str(), attempt.prompt_tx.as_str()));
        filter.takes(attempt, truth.copied())
    });
    let mut report = Report {
        attempts: u32::try_from(attempts.len()).unwrap_or(u32::MAX),
        flags: Flag::ALL
            .into_iter()
            .map(|flag| FlagScore {
                flag,
                raised: 0,
                caught: 0,
                false_alarms: 0,
            })
            .collect(),
        ..Report::default()
    };
    for attempt in attempts {
        report.routed += u32::from(attempt.routed);
        let Some(truth) = truths.get(&(attempt.agent.as_str(), attempt.prompt_tx.as_str())) else {
            continue;
        };
        report.verdicts.take(attempt.success, truth.passed);
        if attempt.success == Some(false) {
            continue;
        }
        report
            .models
            .entry(attempt.model.clone())
            .or_default()
            .take(attempt.success, truth.passed);
        report.stood += 1;
        report.stood_wrong += u32::from(!truth.passed);
        let raised = flags(&attempt.evidence, attempt.category);
        for score in &mut report.flags {
            if raised.contains(&score.flag) {
                score.raised += 1;
                score.caught += u32::from(!truth.passed);
                score.false_alarms += u32::from(truth.passed);
            }
        }
        let run = [truth.compare.as_deref(), truth.run.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        report.scored.push(Scored {
            run,
            task: truth.task.clone().unwrap_or_default(),
            model: attempt.model.clone(),
            routed: attempt.routed,
            signal: attempt.signal,
            right: truth.passed,
            source: truth.source,
            flags: raised,
        });
    }
    report
}

fn share(part: u32, of: u32) -> String {
    if of == 0 {
        "-".to_string()
    } else {
        format!("{:.0}%", 100.0 * f64::from(part) / f64::from(of))
    }
}

impl Report {
    /// The report as text, for a terminal.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        let v = self.verdicts;
        let _ = writeln!(
            out,
            "{} attempts logged ({} routed by Gate), {} later shown right or wrong: {} right, {} wrong",
            self.attempts,
            self.routed,
            v.known,
            v.right,
            v.known - v.right
        );
        if v.known == 0 {
            out.push_str(
                "Nothing to score yet: run /compare on benchmark tasks, whose own checks say whether each run's work was right.\n",
            );
            return out;
        }
        let _ = writeln!(
            out,
            "Gate let stand {} of them: {} right, {} wrong ({} it counted as passed, {} it had nothing to judge by)",
            self.stood, self.stood - self.stood_wrong, self.stood_wrong, v.false_passes, v.unjudged_failures
        );
        let _ = writeln!(
            out,
            "Gate counted {} as failed whose work was right",
            v.false_failures
        );

        let _ = writeln!(out, "\nPer model, attempts Gate let stand:");
        let width = self
            .models
            .keys()
            .map(String::len)
            .max()
            .unwrap_or(5)
            .max(5);
        let _ = writeln!(out, "  {:width$}  stood  wrong  share wrong", "model");
        for (model, m) in &self.models {
            let wrong = m.known - m.right;
            let _ = writeln!(
                out,
                "  {model:width$}  {:>5}  {:>5}  {:>11}",
                m.known,
                wrong,
                share(wrong, m.known)
            );
        }

        let _ = writeln!(
            out,
            "\nSigns of failure, scored on the {} attempts Gate let stand ({} wrong, {} right):",
            self.stood,
            self.stood_wrong,
            self.stood - self.stood_wrong
        );
        let _ = writeln!(
            out,
            "  {:26}  raised  caught  of wrong  false alarms  of right  precision",
            "sign"
        );
        for score in &self.flags {
            let _ = writeln!(
                out,
                "  {:26}  {:>6}  {:>6}  {:>8}  {:>12}  {:>8}  {:>9}",
                score.flag.name(),
                score.raised,
                score.caught,
                share(score.caught, self.stood_wrong),
                score.false_alarms,
                share(score.false_alarms, self.stood - self.stood_wrong),
                share(score.caught, score.raised),
            );
        }

        if self.stood_wrong > 0 {
            let _ = writeln!(out, "\nFailed work Gate let stand:");
            for missed in self.missed() {
                let names: Vec<&str> = missed.flags.iter().map(|flag| flag.name()).collect();
                let flags = if names.is_empty() {
                    "no sign raised".to_string()
                } else {
                    names.join(", ")
                };
                let what = [missed.run.as_str(), missed.task.as_str()]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ");
                let _ = writeln!(
                    out,
                    "  {what} · {} · gate: {} · {flags}",
                    missed.model, missed.signal
                );
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::CheckEvidence;

    fn evidence(command: &str, output: &str, reply: &str) -> Evidence {
        Evidence {
            calls: 3,
            commands: 1,
            checks_run: 1,
            checks: vec![CheckEvidence {
                command: command.to_string(),
                passed: true,
                output: output.to_string(),
            }],
            files_changed: Some(1),
            reply: reply.to_string(),
            ..Evidence::default()
        }
    }

    fn attempt(prompt: &str, signal: OutcomeSignal, evidence: Evidence) -> Attempt {
        Attempt {
            at_ms: 1,
            agent: "aa".to_string(),
            prompt_tx: prompt.to_string(),
            category: PromptCategory::MultiFileChange,
            difficulty: Some(Difficulty::Hard),
            led: Some(false),
            routed: true,
            model: "claude/claude-sonnet-5-5".to_string(),
            signal,
            success: signal.success(),
            cost_usd: 0.1,
            subtask_cost_usd: 0.0,
            latency_ms: 48_000,
            evidence,
            backfilled: false,
        }
    }

    fn truth(prompt: &str, source: TruthSource, passed: bool) -> Truth {
        Truth {
            at_ms: 2,
            agent: "aa".to_string(),
            prompt_tx: prompt.to_string(),
            source,
            passed,
            compare: Some("Compare #22".to_string()),
            run: Some("a-4".to_string()),
            task: Some("ansible-d33bedc4".to_string()),
        }
    }

    fn clean() -> Evidence {
        evidence("pytest -q", "9 passed", "Done; the tests pass.")
    }

    fn masked() -> Evidence {
        evidence(
            "pytest -q 2>&1 | grep -E 'passed|failed' | head -20",
            "TypeError: boom\n9 passed",
            "I rewrote it, but it isn't verified.",
        )
    }

    #[test]
    fn lines_round_trip_through_the_file() {
        let dir = std::env::temp_dir().join(format!("aura-attempt-log-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let log = AttemptLog::in_data_dir(&dir);
        assert_eq!(log.read().unwrap(), Read::default(), "no file yet");
        let lines = [
            Line::Attempt(attempt("p1", OutcomeSignal::ChecksPassed, masked())),
            Line::Truth(truth("p1", TruthSource::TaskCheck, false)),
        ];
        for line in &lines {
            log.append(line).unwrap();
        }
        fs::OpenOptions::new()
            .append(true)
            .open(log.path())
            .unwrap()
            .write_all(b"not json\n\n")
            .unwrap();
        let read = log.read().unwrap();
        assert_eq!(read.lines, lines);
        assert_eq!(read.unreadable, 1);
        let text = fs::read_to_string(log.path()).unwrap();
        assert!(
            text.starts_with(r#"{"kind":"attempt","at_ms":1,"agent":"aa","prompt_tx":"p1""#),
            "{text}"
        );
        assert!(text.contains(r#"{"kind":"truth","#), "{text}");
        assert!(log.path().ends_with("gate/attempts.jsonl"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_report_scores_gate_and_each_flag_against_what_happened() {
        let lines = [
            // Let stand, wrong, and flagged.
            Line::Attempt(attempt("p1", OutcomeSignal::ChecksPassed, masked())),
            Line::Truth(truth("p1", TruthSource::TaskCheck, false)),
            // Let stand and right: clean, then one that pipes its check.
            Line::Attempt(attempt("p2", OutcomeSignal::ChecksPassed, clean())),
            Line::Truth(truth("p2", TruthSource::TaskCheck, true)),
            Line::Attempt(attempt(
                "p3",
                OutcomeSignal::ChecksPassed,
                evidence("pytest -q | tail -5", "9 passed", "Done."),
            )),
            Line::Truth(truth("p3", TruthSource::TaskCheck, true)),
            // Nothing to judge by, wrong.
            Line::Attempt(attempt("p4", OutcomeSignal::None, Evidence::default())),
            Line::Truth(truth("p4", TruthSource::Rewind, false)),
            // Counted as failed (and moved up): not what flags are for.
            Line::Attempt(attempt("p5", OutcomeSignal::ChecksFailed, clean())),
            Line::Truth(truth("p5", TruthSource::TaskCheck, true)),
            // No truth: not scored.
            Line::Attempt(attempt("p6", OutcomeSignal::ChecksPassed, masked())),
        ];
        let report = evaluate(&lines);
        assert_eq!((report.attempts, report.routed), (6, 6));
        assert_eq!(
            report.verdicts,
            Verdicts {
                known: 5,
                right: 3,
                false_passes: 1,
                unjudged_failures: 1,
                false_failures: 1,
            }
        );
        assert_eq!((report.stood, report.stood_wrong), (4, 2));
        let score = |flag: Flag| {
            let s = report.flags.iter().find(|s| s.flag == flag).unwrap();
            (s.raised, s.caught, s.false_alarms)
        };
        assert_eq!(score(Flag::MaskedCheck), (2, 1, 1));
        assert_eq!(score(Flag::MaskedCheckWithFailures), (1, 1, 0));
        assert_eq!(score(Flag::ReplyUnverified), (1, 1, 0));
        assert_eq!(score(Flag::NoCheck), (1, 1, 0));
        assert_eq!(score(Flag::HitLimit), (0, 0, 0));
        assert_eq!(report.models["claude/claude-sonnet-5-5"].known, 4);
        assert_eq!(report.scored.len(), 4);
        let missed: Vec<&Scored> = report.missed().collect();
        assert_eq!(missed.len(), 2);
        assert_eq!(missed[0].run, "Compare #22 a-4");
        assert_eq!(
            missed[0].flags,
            [
                Flag::MaskedCheck,
                Flag::CheckOutputFailures,
                Flag::MaskedCheckWithFailures,
                Flag::ReplyUnverified
            ]
        );

        let text = report.render();
        assert!(text.contains("6 attempts logged (6 routed by Gate), 5 later shown right or wrong: 3 right, 2 wrong"), "{text}");
        assert!(text.contains("Gate let stand 4 of them: 2 right, 2 wrong (1 it counted as passed, 1 it had nothing to judge by)"), "{text}");
        assert!(text.contains("masked_check_with_failures"), "{text}");
        assert!(text.contains("Compare #22 a-4 · ansible-d33bedc4 · claude/claude-sonnet-5-5 · gate: checks_passed · masked_check"), "{text}");
    }

    #[test]
    fn an_attempt_logged_twice_counts_once_and_a_task_check_outranks_the_user() {
        let mut rebuilt = attempt("p1", OutcomeSignal::ChecksPassed, clean());
        rebuilt.backfilled = true;
        let lines = [
            Line::Attempt(rebuilt.clone()),
            Line::Attempt(attempt("p1", OutcomeSignal::None, clean())),
            Line::Attempt(attempt("p1", OutcomeSignal::ChecksPassed, masked())),
            Line::Attempt(rebuilt),
            Line::Truth(truth("p1", TruthSource::TaskCheck, false)),
            Line::Truth(truth("p1", TruthSource::Picked, true)),
        ];
        let report = evaluate(&lines);
        assert_eq!(report.attempts, 1);
        assert_eq!(
            (
                report.verdicts.false_passes,
                report.verdicts.unjudged_failures
            ),
            (1, 0),
            "the task's check counts, against what Gate last made of the attempt"
        );
        assert_eq!(
            report.missed().next().unwrap().flags.first(),
            Some(&Flag::MaskedCheck),
            "the last line written while the app ran counts"
        );
    }

    #[test]
    fn a_report_can_be_about_some_attempts_only() {
        let mut pinned = attempt("p2", OutcomeSignal::ChecksPassed, clean());
        pinned.routed = false;
        let mut other = truth("p2", TruthSource::TaskCheck, true);
        other.task = Some("coding/rename-symbol".to_string());
        let lines = [
            Line::Attempt(attempt("p1", OutcomeSignal::ChecksPassed, masked())),
            Line::Truth(truth("p1", TruthSource::TaskCheck, false)),
            Line::Attempt(pinned),
            Line::Truth(other),
            Line::Attempt(attempt("p3", OutcomeSignal::ChecksPassed, clean())),
        ];
        let all = evaluate(&lines);
        assert_eq!((all.attempts, all.stood, all.stood_wrong), (3, 2, 1));
        let routed = evaluate_where(
            &lines,
            &Filter {
                routed: true,
                ..Filter::default()
            },
        );
        assert_eq!(
            (routed.attempts, routed.stood, routed.stood_wrong),
            (2, 1, 1)
        );
        let task = evaluate_where(
            &lines,
            &Filter {
                task: Some("coding/".to_string()),
                ..Filter::default()
            },
        );
        assert_eq!((task.attempts, task.stood, task.stood_wrong), (1, 1, 0));
        let mut unjudged = lines.to_vec();
        unjudged.push(Line::Attempt(attempt(
            "p4",
            OutcomeSignal::None,
            Evidence::default(),
        )));
        unjudged.push(Line::Truth(truth("p4", TruthSource::Rewind, false)));
        let passed = Filter {
            passed: true,
            ..Filter::default()
        };
        assert_eq!(evaluate(&unjudged).stood, 3);
        assert_eq!(evaluate_where(&unjudged, &passed).stood, 2);
    }

    #[test]
    fn an_empty_log_says_what_fills_it() {
        let text = evaluate(&[]).render();
        assert!(text.starts_with("0 attempts logged"), "{text}");
        assert!(text.contains("Nothing to score yet"), "{text}");
    }

    #[test]
    fn the_users_signals_are_truths() {
        assert_eq!(
            TruthSource::of_user(OutcomeSignal::Rewind),
            Some(TruthSource::Rewind)
        );
        assert_eq!(
            TruthSource::of_user(OutcomeSignal::Good),
            Some(TruthSource::Picked)
        );
        assert_eq!(TruthSource::of_user(OutcomeSignal::ChecksPassed), None);
        assert_eq!(TruthSource::of_user(OutcomeSignal::JudgedFail), None);
    }
}
