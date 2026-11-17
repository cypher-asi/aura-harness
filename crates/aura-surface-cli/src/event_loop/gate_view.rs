//! `/gate`: what Gate has learned per level, category and model, the
//! model it picks for each category now, and its latest decisions.

use super::gate_live::RecentRoute;
use super::handlers::show_system_message;
use super::LoopState;
use crate::record_loader::format_local_time;
use aura_agent_gate::router::by_expected_cost;
use aura_agent_gate::stats::Row;
use aura_agent_gate::{Candidate, GateStats};
use aura_config::GateConfig;
use aura_core_types::{PromptCategory, RouteLevel};
use aura_model_reasoner::model_catalog::route_label;
use std::fmt::Write;

/// Show `/gate` in the active agent's chat.
pub(super) async fn show(state: &LoopState<'_>) {
    let live = &state.hub.gate;
    let view = GateView {
        stats: &live.stats(),
        candidates: &state.models.gate_candidates(),
        settings: live.settings(),
        recent: &live.recent(),
        pinned: (!state.models.is_gate()).then(|| state.models.current().to_string()),
    };
    show_system_message(state, view.render()).await;
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
}

/// One model's line under a category.
struct Line {
    model: String,
    row: Option<Row>,
    pick: Option<&'static str>,
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
        self.render_recent(&mut out);
        out
    }

    /// The table for `level`. Turns list every category; subtasks only
    /// those with outcomes, until there are any.
    fn render_level(&self, out: &mut String, level: RouteLevel) {
        let categories: Vec<(PromptCategory, Vec<Line>)> = PromptCategory::CLASSES
            .into_iter()
            .chain([PromptCategory::Unclassified])
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
                if let Some(reason) = line.pick {
                    let _ = write!(out, "  pick ({reason})");
                }
                name = "";
            }
        }
    }

    /// Lines for `category` at `level`: Gate's pick now, then every other
    /// model with outcomes there, cheapest first.
    fn lines(&self, level: RouteLevel, category: PromptCategory) -> Vec<Line> {
        let no_exploring = GateConfig {
            explore_rate: 0.0,
            ..*self.settings
        };
        let pick = aura_agent_gate::pick(
            self.stats,
            level,
            category,
            self.candidates,
            &no_exploring,
            &mut || 1.0,
        );
        let row = |model: &str| self.stats.row(level, category, model).copied();
        let mut lines: Vec<Line> = pick
            .iter()
            .map(|decision| Line {
                model: decision.model.route_label(),
                row: row(&decision.model.id),
                pick: Some(decision.reason.as_str()),
            })
            .collect();
        let picked = pick.as_ref().map(|d| d.model.id.as_str());
        let order = by_expected_cost(self.stats, level, self.candidates);
        let mut tried: Vec<(Option<usize>, &str, Row)> = self
            .stats
            .rows()
            .into_iter()
            .filter(|(key, _)| key.level == level && key.category == category)
            .filter(|(key, _)| Some(key.model.as_str()) != picked)
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
        aura_agent_gate::candidates(
            &aura_model_reasoner::model_catalog::catalog("local/qwen3:4b", &[]),
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
                prompt.category = category;
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
            "question → claude-haiku-4-5 (claude plan) 10 100% $0.020 pick (proven)"
        );
        assert_eq!(
            line("claude-sonnet-4-6 (claude plan)  "),
            "claude-sonnet-4-6 (claude plan) 3 67% $0.020"
        );
        assert_eq!(
            line("debugging"),
            "debugging → claude-opus-4-7 (claude plan) - - - pick (default)"
        );
        assert!(line("writing").ends_with("pick (default)"));
        assert!(!text.contains("Subtasks"), "no subtask outcomes yet");
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
        }
        .render();
        assert!(
            text.contains("Your turns run on claude/claude-opus-4-7"),
            "{text}"
        );
        assert!(text.contains("Nothing to route to yet"), "{text}");
        assert!(text.contains("No routing decisions yet"), "{text}");
    }

    #[test]
    fn costs_read_as_money() {
        assert_eq!(money(0.0), "free");
        assert_eq!(money(0.0042), "$0.0042");
        assert_eq!(money(0.25), "$0.250");
    }
}
