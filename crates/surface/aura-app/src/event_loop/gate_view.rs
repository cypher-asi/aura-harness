//! `/gate`: what Gate has learned per level, category and model, the
//! model it picks for each category now, how other ways of routing turns
//! would have done on the recorded ones, how much of the active agent's
//! work went to subtasks, and Gate's latest decisions.

use super::handlers::show_system_message;
use super::subagents::{lock, AgentHub};
use super::LoopState;
use crate::record_loader::{format_local_time, model_call};
use aura_config::GateConfig;
use aura_core_types::{AgentId, Difficulty, PromptCategory, RouteLevel, RouteReason};
use aura_gate::compare::list_price;
use aura_gate::lead::delegation::{self, Call, Delegation, Share, Subtask};
use aura_gate::live::RecentRoute;
use aura_gate::route::by_expected_cost;
use aura_gate::route::replay::{self, Estimate, Logged};
use aura_gate::route::stats::GateStats;
use aura_gate::route::stats::Row;
use aura_gate::route::Candidate;
use aura_model::model_catalog::route_label;
use aura_store_db::ReadStore;
use std::collections::HashMap;
use std::fmt::Write;
use tracing::warn;

/// Record entries read per scan for the delegation figures.
const SCAN_BATCH: usize = 500;

/// Turns of the lead `/gate` lists one by one.
const RECENT_TURNS: usize = 5;

/// Show `/gate` in the active agent's chat.
pub(super) async fn show(state: &LoopState<'_>) {
    let live = &state.hub.gate;
    let view = GateView {
        stats: &live.stats(),
        candidates: &state.models.gate_candidates(),
        settings: live.settings(),
        recent: &live.recent(),
        pinned: (!state.models.is_gate()).then(|| state.models.current().to_string()),
        delegation: &delegation_of(&state.hub, state.active),
    };
    show_system_message(state, view.render()).await;
}

/// What one agent's record says for the delegation figures.
#[derive(Default)]
struct Record {
    /// Prompts: when, at what level, why routed there, and whether it was
    /// a turn's own prompt (not a message sent while it ran)
    prompts: Vec<(u64, RouteLevel, RouteReason, bool)>,
    /// Model calls: when, and their list-price cost
    calls: Vec<Call>,
}

fn read_record(store: &dyn ReadStore, agent: AgentId) -> Record {
    let mut record = Record::default();
    let mut from = 1;
    loop {
        let entries = match store.scan_record(agent, from, SCAN_BATCH) {
            Ok(entries) => entries,
            Err(e) => {
                warn!(%agent, error = %e, "Could not read a record for /gate");
                break;
            }
        };
        for entry in &entries {
            let ts_ms = entry.tx.ts_ms;
            if let Some(prompt) = entry.tx.prompt_payload() {
                let own = !prompt.route.model.is_empty();
                record
                    .prompts
                    .push((ts_ms, prompt.route.level, prompt.route.reason, own));
            } else if let Some((model, usage)) = model_call(&entry.tx) {
                record.calls.push(Call {
                    ts_ms,
                    cost_usd: list_price(&model, &usage).unwrap_or(0.0),
                });
            }
        }
        match entries.last() {
            Some(last) if entries.len() == SCAN_BATCH => from = last.seq + 1,
            _ => break,
        }
    }
    record
}

/// How much of `lead`'s work went to subtasks: its turns and model
/// calls from its record, and each subagent below it whose nearest
/// agent with turns of its own is `lead` (a `/compare` run leads its own
/// subtasks) as one subtask, from that subagent's record.
pub(super) fn delegation_of(hub: &AgentHub, lead: AgentId) -> Delegation {
    let store = hub.factory.store.as_ref();
    let below: Vec<(AgentId, Vec<AgentId>)> = {
        let registry = lock(&hub.registry);
        registry
            .descendants(lead)
            .into_iter()
            .map(|agent| (agent, registry.ancestry(agent)))
            .collect()
    };
    let own = read_record(store, lead);
    let records: HashMap<AgentId, Record> = below
        .iter()
        .map(|(agent, _)| (*agent, read_record(store, *agent)))
        .collect();
    let has_turns = |record: &Record| {
        record
            .prompts
            .iter()
            .any(|(_, level, _, own)| *level == RouteLevel::Turn && *own)
    };
    let mut subtasks = Vec::new();
    for (agent, ancestry) in &below {
        let led_by = ancestry[1..]
            .iter()
            .find(|a| **a == lead || records.get(a).is_some_and(has_turns));
        if led_by != Some(&lead) {
            continue;
        }
        let record = &records[agent];
        let Some(&(ts_ms, _, reason, _)) = record
            .prompts
            .iter()
            .find(|(_, level, _, _)| *level == RouteLevel::Subtask)
        else {
            continue;
        };
        subtasks.push(Subtask {
            ts_ms,
            escalated: reason == RouteReason::Escalate,
            calls: u32::try_from(record.calls.len()).unwrap_or(u32::MAX),
            cost_usd: record.calls.iter().map(|c| c.cost_usd).sum(),
        });
    }
    let turns: Vec<u64> = own
        .prompts
        .iter()
        .filter(|(_, level, _, own)| *level == RouteLevel::Turn && *own)
        .map(|(ts_ms, ..)| *ts_ms)
        .collect();
    delegation::summarize(&turns, &own.calls, &subtasks)
}

/// Everything `/gate` shows.
struct GateView<'a> {
    stats: &'a GateStats,
    candidates: &'a [Candidate],
    settings: &'a GateConfig,
    /// Oldest first
    recent: &'a [RecentRoute],
    /// The model the user pinned, when Gate isn't routing their turns
    pinned: Option<String>,
    /// How much of the active agent's work went to subtasks
    delegation: &'a Delegation,
}

/// One model's line under a category.
struct Line {
    model: String,
    row: Option<Row>,
    /// What Gate picks the model for and why, e.g. `pick (proven)` or
    /// `pick for easy work (default)`
    pick: Option<String>,
}

impl GateView<'_> {
    fn render(&self) -> String {
        let mut out = format!(
            "Gate: a model becomes a category's pick after {} counted outcomes at {:.0}% \
             success or better; {:.0}% of routes try the next cheaper model.",
            self.settings.min_tries,
            self.settings.target * 100.0,
            self.settings.explore_rate * 100.0,
        );
        if let Some(model) = &self.pinned {
            let _ = write!(
                out,
                "\nYour turns run on {model}, which you chose; pick gate in /model to let \
                 Gate route them."
            );
        }
        if self.candidates.is_empty() {
            out.push_str("\nNothing to route to yet: /login, link a plan, or start a local model.");
        }
        for level in [RouteLevel::Turn, RouteLevel::Subtask] {
            self.render_level(&mut out, level);
        }
        self.render_replay(&mut out);
        self.render_delegation(&mut out);
        self.render_recent(&mut out);
        out
    }

    /// How the recorded turns went, and how Gate's picks now and each
    /// model on its own would have done on them.
    fn render_replay(&self, out: &mut String) {
        let logged: Vec<Logged> = self
            .stats
            .logged()
            .into_iter()
            .filter(|prompt| prompt.level == RouteLevel::Turn)
            .collect();
        if logged.is_empty() {
            return;
        }
        let no_exploring = self.settings.without_exploring();
        let gate_now = replay::estimate(&logged, |prompt| {
            aura_gate::route::pick(
                self.stats,
                &prompt.work(),
                self.candidates,
                &no_exploring,
                &mut || 1.0,
            )
            .is_some_and(|decision| decision.model.id == prompt.model)
        });
        let mut models: Vec<&str> = logged.iter().map(|p| p.model.as_str()).collect();
        models.sort_unstable();
        models.dedup();
        let mut policies: Vec<(String, Option<Estimate>)> = vec![
            ("as recorded".to_string(), replay::as_recorded(&logged)),
            ("Gate's picks now".to_string(), gate_now),
        ];
        policies.extend(models.into_iter().map(|model| {
            (
                format!("all on {}", route_label(model)),
                replay::estimate(&logged, |prompt| prompt.model == model),
            )
        }));
        let width = policies
            .iter()
            .map(|(name, _)| name.chars().count())
            .max()
            .unwrap_or(0);
        let _ = write!(
            out,
            "\n\nReplay of {} recorded turns, each weighted by how unlikely its route was; \
             a policy is judged on the turns it would have routed the same way\n  \
             {:<width$}  {:>7}  {:>7}  {:>9}",
            logged.len(),
            "policy",
            "matched",
            "success",
            "avg cost"
        );
        for (name, estimate) in policies {
            let (matched, success, cost) = estimate.map_or_else(
                || ("0".to_string(), "-".to_string(), "-".to_string()),
                |e| {
                    (
                        e.matched.to_string(),
                        format!("{:.0}%", e.success_rate * 100.0),
                        money(e.mean_cost_usd),
                    )
                },
            );
            let _ = write!(
                out,
                "\n  {name:<width$}  {matched:>7}  {success:>7}  {cost:>9}"
            );
        }
    }

    /// The share of the active agent's cost and model calls spent in
    /// subtasks, overall and for its last turns.
    fn render_delegation(&self, out: &mut String) {
        let delegation = self.delegation;
        if delegation.turns.is_empty() {
            return;
        }
        let overall = &delegation.overall;
        let _ = write!(
            out,
            "\n\nDelegation in this agent's {} turn{}: {:.1} subtasks per turn, escalation rate {}\n  {}",
            delegation.turns.len(),
            if delegation.turns.len() == 1 { "" } else { "s" },
            delegation.subtasks_per_turn().unwrap_or(0.0),
            rate(overall.escalation_rate(), overall.escalations, overall.subtasks),
            share_line(overall),
        );
        let start = delegation.turns.len().saturating_sub(RECENT_TURNS);
        for turn in &delegation.turns[start..] {
            let _ = write!(
                out,
                "\n  {}  {} subtask{} · {}",
                format_local_time(turn.ts_ms),
                turn.share.subtasks,
                if turn.share.subtasks == 1 { "" } else { "s" },
                share_line(&turn.share),
            );
        }
    }

    /// The table for `level`. Turns list every category; subtasks only
    /// those with outcomes, until there are any.
    fn render_level(&self, out: &mut String, level: RouteLevel) {
        let categories: Vec<(PromptCategory, Vec<Line>)> = PromptCategory::CLASSES
            .into_iter()
            .map(|category| (category, self.lines(level, category)))
            .filter(|(_, lines)| {
                level == RouteLevel::Turn || lines.iter().any(|line| line.row.is_some())
            })
            .collect();
        if categories.is_empty() {
            return;
        }
        let width = categories
            .iter()
            .flat_map(|(_, lines)| lines.iter().map(|line| line.model.chars().count()))
            .max()
            .unwrap_or(0);
        let title = match level {
            RouteLevel::Turn => "Turns",
            RouteLevel::Subtask => "Subtasks",
        };
        let _ = write!(
            out,
            "\n\n{title}\n  {:<20}{:<width$}  {:>5}  {:>7}  {:>9}",
            "category", "  model", "tries", "success", "avg cost"
        );
        for (category, lines) in categories {
            let mut name = category.as_str();
            for line in lines {
                let (tries, success, cost) = line.row.map_or_else(
                    || ("-".to_string(), "-".to_string(), "-".to_string()),
                    |row| {
                        (
                            row.tries.to_string(),
                            row.success_rate()
                                .map_or_else(String::new, |r| format!("{:.0}%", r * 100.0)),
                            row.mean_cost_usd().map_or_else(String::new, money),
                        )
                    },
                );
                let marker = if line.pick.is_some() { "→ " } else { "  " };
                let _ = write!(
                    out,
                    "\n  {name:<20}{marker}{:<width$}  {tries:>5}  {success:>7}  {cost:>9}",
                    line.model
                );
                if let Some(pick) = &line.pick {
                    let _ = write!(out, "  {pick}");
                }
                name = "";
            }
        }
    }

    /// Lines for `category` at `level`: Gate's pick now, then its pick for
    /// easy work when that is another model, then every other model with
    /// outcomes there, cheapest first.
    fn lines(&self, level: RouteLevel, category: PromptCategory) -> Vec<Line> {
        let no_exploring = self.settings.without_exploring();
        let pick = |work: &aura_gate::route::Work<'_>| {
            aura_gate::route::pick(
                self.stats,
                work,
                self.candidates,
                &no_exploring,
                &mut || 1.0,
            )
        };
        let work = aura_gate::route::Work::new(level, category, None);
        let picked = pick(&work);
        let easy = pick(&work.of(Some(Difficulty::Easy)))
            .filter(|easy| picked.as_ref().is_none_or(|p| p.model.id != easy.model.id));
        let row = |model: &str| self.stats.row(level, category, model).copied();
        let mut lines: Vec<Line> = picked
            .iter()
            .map(|decision| (decision, "pick"))
            .chain(easy.iter().map(|decision| (decision, "pick for easy work")))
            .map(|(decision, pick)| Line {
                model: decision.model.route_label(),
                row: row(&decision.model.id),
                pick: Some(format!("{pick} ({})", decision.reason.as_str())),
            })
            .collect();
        let shown: Vec<&str> = picked
            .iter()
            .chain(easy.iter())
            .map(|d| d.model.id.as_str())
            .collect();
        let order = by_expected_cost(self.stats, level, self.candidates);
        let mut tried: Vec<(Option<usize>, &str, Row)> = self
            .stats
            .rows()
            .into_iter()
            .filter(|(key, _)| key.level == level && key.category == category)
            .filter(|(key, _)| !shown.contains(&key.model.as_str()))
            .map(|(key, row)| {
                let rank = order.iter().position(|c| c.id == key.model);
                (rank, key.model.as_str(), *row)
            })
            .collect();
        // Models that can't run now (signed out, dropped) come last.
        tried.sort_by(|a, b| match (a.0, b.0) {
            (Some(a), Some(b)) => a.cmp(&b),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.1.cmp(b.1),
        });
        lines.extend(tried.into_iter().map(|(_, model, row)| Line {
            model: route_label(model),
            row: Some(row),
            pick: None,
        }));
        lines
    }

    fn render_recent(&self, out: &mut String) {
        if self.recent.is_empty() {
            out.push_str("\n\nNo routing decisions yet in this session.");
            return;
        }
        let _ = write!(out, "\n\nLast {} decisions, newest last", self.recent.len());
        for route in self.recent {
            let _ = write!(
                out,
                "\n  {}  {:<7}  {:<17}  {}  {}",
                format_local_time(route.ts_ms),
                route.level.as_str(),
                route.category.as_str(),
                route.model,
                route.reason,
            );
        }
    }
}

/// `40% (2 of 5)`, or `-` when there is nothing to count.
fn rate(share: Option<f64>, part: u32, whole: u32) -> String {
    share.map_or_else(
        || "-".to_string(),
        |share| format!("{:.0}% ({part} of {whole})", share * 100.0),
    )
}

/// Cost and model calls in subtasks against the lead's.
fn share_line(share: &Share) -> String {
    let cost = share.cost_share().map_or_else(
        || "no cost".to_string(),
        |part| {
            format!(
                "{:.0}% of cost in subtasks ({} of {})",
                part * 100.0,
                money(share.subtask_cost_usd),
                money(share.lead_cost_usd + share.subtask_cost_usd)
            )
        },
    );
    let calls = rate(
        share.call_share(),
        share.subtask_calls,
        share.lead_calls + share.subtask_calls,
    );
    format!("{cost}, {calls} of model calls")
}

/// `$0.004`, or `free` for models that cost nothing.
fn money(usd: f64) -> String {
    if usd <= 0.0 {
        "free".to_string()
    } else if usd < 0.01 {
        format!("${usd:.4}")
    } else {
        format!("${usd:.3}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core_types::{
        AgentId, OutcomePayload, OutcomeSignal, PromptPayload, RecordEntry, Route, RouteReason,
        Transaction,
    };

    fn claude() -> Vec<Candidate> {
        aura_gate::route::candidates(
            &aura_model::model_catalog::catalog("local/qwen3:4b", &[]),
            |p| p == aura_auth::ProviderKind::Claude,
            |_| false,
        )
    }

    /// Stats with `ok` successful and `failed` failed turns of `model` on
    /// `category`.
    fn learned(rows: &[(PromptCategory, &str, usize, usize)]) -> GateStats {
        let agent = AgentId::generate();
        let mut entries = Vec::new();
        let mut seq = 0;
        for &(category, model, ok, failed) in rows {
            for n in 0..ok + failed {
                let mut prompt = PromptPayload::new(format!("{category} {model} {n}"));
                prompt.category = Some(category);
                prompt.route = Route::turn(model, RouteReason::Default);
                let tx = Transaction::user_prompt_with(agent, &prompt);
                let prompt_tx = tx.hash;
                seq += 1;
                entries.push(RecordEntry::builder(seq, tx).build());
                let signal = if n < ok {
                    OutcomeSignal::ChecksPassed
                } else {
                    OutcomeSignal::ChecksFailed
                };
                let outcome = OutcomePayload {
                    prompt_tx,
                    category,
                    model: model.to_string(),
                    success: signal.success(),
                    signal,
                    cost_usd: 0.02,
                    subtask_cost_usd: 0.0,
                    latency_ms: 100,
                };
                seq += 1;
                entries
                    .push(RecordEntry::builder(seq, Transaction::outcome(agent, &outcome)).build());
            }
        }
        GateStats::from_entries(&entries)
    }

    #[test]
    fn the_view_shows_rows_picks_and_recent_decisions() {
        let stats = learned(&[
            (PromptCategory::Question, "claude/claude-haiku-4-5", 10, 0),
            (PromptCategory::Question, "claude/claude-sonnet-4-6", 2, 1),
            (PromptCategory::Debugging, "claude/claude-haiku-4-5", 1, 3),
        ]);
        let recent = [RecentRoute {
            ts_ms: 0,
            level: RouteLevel::Turn,
            category: PromptCategory::Question,
            model: "claude-haiku-4-5 (claude plan)".to_string(),
            reason: RouteReason::Proven,
        }];
        let text = GateView {
            stats: &stats,
            candidates: &claude(),
            settings: &GateConfig::defaults(),
            recent: &recent,
            pinned: None,
            delegation: &Delegation::default(),
        }
        .render();

        assert!(text.contains("after 10 counted outcomes at 85%"), "{text}");
        let line = |needle: &str| {
            text.lines()
                .find(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("no line with {needle:?} in:\n{text}"))
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert_eq!(
            line("question"),
            "question → claude-sonnet-4-6 (claude plan) 3 67% $0.020 pick (default)",
            "work that is not easy starts no lower than sonnet, proven haiku or not"
        );
        assert_eq!(
            line("pick for easy work (proven)"),
            "→ claude-haiku-4-5 (claude plan) 10 100% $0.020 pick for easy work (proven)"
        );
        assert_eq!(
            line("debugging"),
            "debugging → claude-sonnet-4-6 (claude plan) - - - pick (default)"
        );
        let lines: Vec<String> = text
            .lines()
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect();
        assert!(
            lines.contains(&"claude-haiku-4-5 (claude plan) 4 25% $0.020".to_string()),
            "haiku is out of reach on debugging, so easy debugging goes to sonnet too:\n{text}"
        );
        assert!(line("writing").ends_with("pick (default)"));
        assert!(
            lines.contains(
                &"→ claude-haiku-4-5 (claude plan) - - - pick for easy work (default)".to_string()
            ),
            "{text}"
        );
        assert!(!text.contains("Subtasks"), "no subtask outcomes yet");
        assert!(text.contains("Replay of 17 recorded turns"), "{text}");
        assert_eq!(line("as recorded"), "as recorded 17 76% $0.020");
        assert_eq!(
            line("Gate's picks now"),
            "Gate's picks now 3 67% $0.020",
            "Gate now sends unrated questions and debugging to sonnet"
        );
        assert_eq!(
            line("all on claude-haiku-4-5"),
            "all on claude-haiku-4-5 (claude plan) 14 79% $0.020"
        );
        assert_eq!(
            line("all on claude-sonnet-4-6"),
            "all on claude-sonnet-4-6 (claude plan) 3 67% $0.020"
        );
        assert!(text.contains("Last 1 decisions"), "{text}");
        assert!(line("turn     question").ends_with("claude-haiku-4-5 (claude plan) proven"));
    }

    #[test]
    fn a_pinned_model_and_no_candidates_are_explained() {
        let text = GateView {
            stats: &GateStats::default(),
            candidates: &[],
            settings: &GateConfig::defaults(),
            recent: &[],
            pinned: Some("claude/claude-opus-4-7".to_string()),
            delegation: &Delegation::default(),
        }
        .render();
        assert!(
            text.contains("Your turns run on claude/claude-opus-4-7"),
            "{text}"
        );
        assert!(text.contains("Nothing to route to yet"), "{text}");
        assert!(text.contains("No routing decisions yet"), "{text}");
        assert!(!text.contains("Replay"), "nothing recorded to replay");
    }

    #[test]
    fn the_view_shows_the_share_of_work_in_subtasks() {
        let delegation = delegation::summarize(
            &[1_000, 2_000],
            &[
                Call {
                    ts_ms: 1_100,
                    cost_usd: 0.10,
                },
                Call {
                    ts_ms: 2_100,
                    cost_usd: 0.10,
                },
            ],
            &[
                Subtask {
                    ts_ms: 1_200,
                    escalated: false,
                    calls: 3,
                    cost_usd: 0.20,
                },
                Subtask {
                    ts_ms: 1_300,
                    escalated: true,
                    calls: 3,
                    cost_usd: 0.40,
                },
            ],
        );
        let text = GateView {
            stats: &GateStats::default(),
            candidates: &claude(),
            settings: &GateConfig::defaults(),
            recent: &[],
            pinned: None,
            delegation: &delegation,
        }
        .render();
        assert!(
            text.contains(
                "Delegation in this agent's 2 turns: 0.5 subtasks per turn, escalation rate \
                 100% (1 of 1)\n  75% of cost in subtasks ($0.600 of $0.800), 75% (6 of 8) of \
                 model calls"
            ),
            "{text}"
        );
        assert!(
            text.contains("1 subtask · 86% of cost in subtasks"),
            "{text}"
        );
        assert!(
            text.contains("0 subtasks · 0% of cost in subtasks"),
            "{text}"
        );
    }

    /// The first line of the view under the default settings.
    const SETTINGS: &str = "Gate: a model becomes a category's pick after 10 counted outcomes \
                            at 85% success or better; 5% of routes try the next cheaper model.";

    /// `/gate` while Gate routes among Claude's models under the default
    /// settings, having learned `stats`, with `recent` decisions and
    /// `delegation`'s figures.
    fn render(stats: &GateStats, recent: &[RecentRoute], delegation: &Delegation) -> String {
        GateView {
            stats,
            candidates: &claude(),
            settings: &GateConfig::defaults(),
            recent,
            pinned: None,
            delegation,
        }
        .render()
    }

    /// The section of `text` titled `title`, up to the blank line after it.
    fn section<'a>(text: &'a str, title: &str) -> &'a str {
        let start = text
            .find(&format!("\n\n{title}"))
            .unwrap_or_else(|| panic!("no {title:?} section in:\n{text}"))
            + 2;
        let rest = &text[start..];
        rest.find("\n\n").map_or(rest, |end| &rest[..end])
    }

    #[test]
    fn every_column_of_the_view_is_pinned() {
        let stats = learned(&[
            (PromptCategory::Question, "claude/claude-haiku-4-5", 10, 0),
            (PromptCategory::Question, "claude/claude-sonnet-4-6", 2, 1),
            (PromptCategory::Debugging, "claude/claude-haiku-4-5", 1, 3),
        ]);
        let recent = [RecentRoute {
            ts_ms: 0,
            level: RouteLevel::Turn,
            category: PromptCategory::Question,
            model: "claude-haiku-4-5 (claude plan)".to_string(),
            reason: RouteReason::Proven,
        }];
        let decision = format!(
            "  {}  turn     question           claude-haiku-4-5 (claude plan)  proven",
            format_local_time(0)
        );
        let expected = [
            SETTINGS,
            "",
            "Turns",
            "  category              model                          tries  success   avg cost",
            "  question            → claude-sonnet-4-6 (claude plan)      3      67%     $0.020  pick (default)",
            "                      → claude-haiku-4-5 (claude plan)      10     100%     $0.020  pick for easy work (proven)",
            "  small_change        → claude-sonnet-4-6 (claude plan)      -        -          -  pick (default)",
            "                      → claude-haiku-4-5 (claude plan)       -        -          -  pick for easy work (default)",
            "  multi_file_change   → claude-sonnet-4-6 (claude plan)      -        -          -  pick (default)",
            "                      → claude-haiku-4-5 (claude plan)       -        -          -  pick for easy work (default)",
            "  debugging           → claude-sonnet-4-6 (claude plan)      -        -          -  pick (default)",
            "                        claude-haiku-4-5 (claude plan)       4      25%     $0.020",
            "  research            → claude-sonnet-4-6 (claude plan)      -        -          -  pick (default)",
            "                      → claude-haiku-4-5 (claude plan)       -        -          -  pick for easy work (default)",
            "  planning            → claude-sonnet-4-6 (claude plan)      -        -          -  pick (default)",
            "  writing             → claude-sonnet-4-6 (claude plan)      -        -          -  pick (default)",
            "                      → claude-haiku-4-5 (claude plan)       -        -          -  pick for easy work (default)",
            "  other               → claude-sonnet-4-6 (claude plan)      -        -          -  pick (default)",
            "                      → claude-haiku-4-5 (claude plan)       -        -          -  pick for easy work (default)",
            "",
            "Replay of 17 recorded turns, each weighted by how unlikely its route was; a policy is judged on the turns it would have routed the same way",
            "  policy                                  matched  success   avg cost",
            "  as recorded                                  17      76%     $0.020",
            "  Gate's picks now                              3      67%     $0.020",
            "  all on claude-haiku-4-5 (claude plan)        14      79%     $0.020",
            "  all on claude-sonnet-4-6 (claude plan)        3      67%     $0.020",
            "",
            "Last 1 decisions, newest last",
            &decision,
        ];
        assert_eq!(
            render(&stats, &recent, &Delegation::default()),
            expected.join("\n")
        );
    }

    /// Take in a subtask asking for `category` on `model` that the user
    /// cancelled, which counts for what it cost and not as a try. The
    /// route crate's builder only learns outcomes that passed or failed.
    fn cancelled(stats: &mut GateStats, category: PromptCategory, model: &str) {
        let agent = AgentId::generate();
        let mut prompt = PromptPayload::new("cancelled");
        prompt.category = Some(category);
        prompt.route = Route::subtask(model, RouteReason::Default);
        let prompt = Transaction::user_prompt_with(agent, &prompt);
        let outcome = OutcomePayload {
            prompt_tx: prompt.hash,
            category,
            model: model.to_string(),
            success: None,
            signal: OutcomeSignal::Cancelled,
            cost_usd: 0.03,
            subtask_cost_usd: 0.0,
            latency_ms: 10,
        };
        stats.observe(&RecordEntry::builder(1, prompt).build());
        stats.observe(&RecordEntry::builder(2, Transaction::outcome(agent, &outcome)).build());
    }

    #[test]
    fn the_subtask_table_lists_the_categories_with_outcomes_column_for_column() {
        let mut stats = GateStats::default();
        let rows = [
            (
                PromptCategory::Question,
                "claude/claude-haiku-4-5",
                (3, true),
                0.02,
            ),
            (
                PromptCategory::Debugging,
                "claude/claude-sonnet-4-6",
                (1, true),
                0.004,
            ),
            (
                PromptCategory::Debugging,
                "claude/claude-sonnet-4-6",
                (2, false),
                0.004,
            ),
            (PromptCategory::Debugging, "openai/gpt-5.4", (1, true), 0.05),
            (
                PromptCategory::Debugging,
                "claude/claude-opus-4-7",
                (2, true),
                0.1,
            ),
            (PromptCategory::Debugging, "local/qwen3:4b", (1, false), 0.0),
        ];
        for (category, model, outcomes, cost_usd) in rows {
            let work = aura_gate::route::Work::new(RouteLevel::Subtask, category, None);
            stats.learn_costing(&work, model, None, outcomes, (cost_usd, 0.0));
        }
        cancelled(
            &mut stats,
            PromptCategory::Research,
            "claude/claude-opus-4-7",
        );
        let expected = [
            "Subtasks",
            "  category              model                          tries  success   avg cost",
            "  question            → claude-sonnet-4-6 (claude plan)      -        -          -  pick (default)",
            "                      → claude-haiku-4-5 (claude plan)       3     100%     $0.020  pick for easy work (default)",
            "  debugging           → claude-sonnet-4-6 (claude plan)      3      33%    $0.0040  pick (default)",
            "                      → claude-haiku-4-5 (claude plan)       -        -          -  pick for easy work (default)",
            "                        claude-opus-4-7 (claude plan)        2     100%     $0.100",
            "                        qwen3:4b (local)                     1       0%       free",
            "                        openai/gpt-5.4 (cloud)               1     100%     $0.050",
            "  research            → claude-sonnet-4-6 (claude plan)      -        -          -  pick (default)",
            "                      → claude-haiku-4-5 (claude plan)       -        -          -  pick for easy work (default)",
            "                        claude-opus-4-7 (claude plan)        0              $0.030",
        ];
        let text = render(&stats, &[], &Delegation::default());
        assert_eq!(section(&text, "Subtasks"), expected.join("\n"));
    }

    #[test]
    fn with_nothing_to_route_to_the_view_is_its_headings() {
        let text = GateView {
            stats: &GateStats::default(),
            candidates: &[],
            settings: &GateConfig::defaults(),
            recent: &[],
            pinned: Some("claude/claude-opus-4-7".to_string()),
            delegation: &Delegation::default(),
        }
        .render();
        let expected = [
            SETTINGS,
            "Your turns run on claude/claude-opus-4-7, which you chose; pick gate in /model to let Gate route them.",
            "Nothing to route to yet: /login, link a plan, or start a local model.",
            "",
            "Turns",
            "  category              model  tries  success   avg cost",
            "",
            "No routing decisions yet in this session.",
        ];
        assert_eq!(text, expected.join("\n"));
    }

    #[test]
    fn the_delegation_figures_are_pinned() {
        let calls = [1_100, 2_100].map(|ts_ms| Call {
            ts_ms,
            cost_usd: 0.10,
        });
        let subtask = |ts_ms, escalated, cost_usd| Subtask {
            ts_ms,
            escalated,
            calls: 3,
            cost_usd,
        };
        let subtasks = [subtask(1_200, false, 0.20), subtask(1_300, true, 0.40)];
        let delegation = delegation::summarize(&[1_000, 2_000], &calls, &subtasks);
        let expected = format!(
            "Delegation in this agent's 2 turns: 0.5 subtasks per turn, escalation rate 100% (1 of 1)\n  \
             75% of cost in subtasks ($0.600 of $0.800), 75% (6 of 8) of model calls\n  \
             {}  1 subtask · 86% of cost in subtasks ($0.600 of $0.700), 86% (6 of 7) of model calls\n  \
             {}  0 subtasks · 0% of cost in subtasks (free of $0.100), 0% (0 of 1) of model calls",
            format_local_time(1_000),
            format_local_time(2_000),
        );
        let text = render(&GateStats::default(), &[], &delegation);
        assert_eq!(section(&text, "Delegation"), expected);
    }

    #[test]
    fn turns_with_no_calls_have_no_shares_and_only_the_last_five_are_listed() {
        let starts: Vec<u64> = (1..=7).map(|n| n * 1_000).collect();
        let delegation = delegation::summarize(&starts, &[], &[]);
        let mut expected = "Delegation in this agent's 7 turns: 0.0 subtasks per turn, \
                            escalation rate -\n  no cost, - of model calls"
            .to_string();
        for ts_ms in &starts[2..] {
            expected.push_str(&format!(
                "\n  {}  0 subtasks · no cost, - of model calls",
                format_local_time(*ts_ms)
            ));
        }
        let text = render(&GateStats::default(), &[], &delegation);
        assert_eq!(section(&text, "Delegation"), expected);
    }

    #[test]
    fn costs_read_as_money() {
        assert_eq!(money(0.0), "free");
        assert_eq!(money(0.0042), "$0.0042");
        assert_eq!(money(0.25), "$0.250");
    }
}
