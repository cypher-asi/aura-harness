//! Numbers, names and short texts the benchmark views read.

use super::table::Cell;
use crate::app::BY_DIFFICULTY;
use crate::events::{
    arm_letter, BenchArm, BenchCost, BenchDoing, BenchLiveRun, BenchRun, BenchStatus,
    BenchTaskCell, BenchTaskSetup, BenchTokens, BenchTurn, BenchVerdict, BenchView, UNPRICED_MARK,
};
use crate::renderer::text::display_width;
use crate::Theme;
use ratatui::{
    style::{Color, Style},
    text::Span,
};
use unicode_width::UnicodeWidthChar;

/// `$0.004`, `$1.25`, or `free` for nothing.
pub(in crate::renderer) fn money(usd: f64) -> String {
    if usd <= 0.0 {
        "free".to_string()
    } else if usd < 0.01 {
        format!("${usd:.4}")
    } else {
        format!("${usd:.2}")
    }
}

/// `cost` as [`money`], marked `+?` when some of its calls had no known
/// price and `unknown` when none had one; `-` for no cost at all.
pub(super) fn cost_text(cost: Option<BenchCost>) -> String {
    cost.map_or_else(|| "-".to_string(), |cost| cost.read(money))
}

/// `850ms`, `42s`, `3m05s`, `1h05m`.
pub(super) fn duration(ms: u64) -> String {
    if ms < 1_000 {
        return format!("{ms}ms");
    }
    let secs = ms / 1_000;
    if secs < 60 {
        return format!("{secs}s");
    }
    if secs < 3_600 {
        return format!("{}m{:02}s", secs / 60, secs % 60);
    }
    format!("{}h{:02}m", secs / 3_600, secs % 3_600 / 60)
}

/// `thinking 1m40s · call 12 · restarted 2×`, or `running task 8m12s ›`
/// and what the subtask is doing.
pub(super) fn doing_text(doing: &BenchDoing) -> String {
    let mut text = format!("{} {}", doing.what, duration(doing.for_ms));
    if doing.call > 0 {
        text.push_str(&format!(" · call {}", doing.call));
    }
    if doing.restarts > 0 {
        text.push_str(&format!(" · restarted {}×", doing.restarts));
    }
    if let Some(subtask) = &doing.subtask {
        text.push_str(" › ");
        text.push_str(&doing_text(subtask));
    }
    text
}

/// `950`, `471k`, `1.5M`.
#[allow(clippy::cast_precision_loss)]
pub(super) fn tokens(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{}k", n / 1_000)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

/// `opus-5-5` for `claude/claude-opus-5-5`: no provider, no `claude-`.
/// An arm that runs work on a model per difficulty is `by difficulty`.
pub(in crate::renderer) fn short_model(model: &str) -> &str {
    if model.starts_with("difficulty:") {
        return BY_DIFFICULTY;
    }
    let name = model.rsplit('/').next().unwrap_or(model);
    name.strip_prefix("claude-").unwrap_or(name)
}

/// `multi-file`: what the classifier said a run's first prompt asks for;
/// empty when nothing was recorded.
pub(super) fn category(run: &BenchRun) -> String {
    category_name(run.category.as_deref())
}

/// `multi-file` for `multi_file_change`, as stored; empty for `None`.
pub(super) fn category_name(category: Option<&str>) -> String {
    category
        .map_or("", |category| match category {
            "small_change" => "small change",
            "multi_file_change" => "multi-file",
            other => other,
        })
        .to_string()
}

/// `to opus-4-7` for a run a failed attempt moved to a stronger model,
/// `to opus-5-5 max` for one it moved to a higher effort, else `no`.
pub(super) fn moved_up(run: &BenchRun) -> String {
    let Some(to) = run.escalated_to.as_deref() else {
        return "no".to_string();
    };
    match &run.escalated_effort {
        Some(effort) => format!("to {} {effort}", short_model(to)),
        None => format!("to {}", short_model(to)),
    }
}

/// `yes, $0.26 in subtasks` for a run whose model led a turn, handing
/// the work to subtasks, else `no`. The subtasks' cost is marked as the
/// run's is when some of its calls had no known price.
pub(super) fn led(run: &BenchRun) -> String {
    if run.led {
        let subtasks = BenchCost::new(run.subtask_cost_usd, run.unpriced_calls);
        format!("yes, {} in subtasks", subtasks.read(money))
    } else {
        "no".to_string()
    }
}

/// `-74%` for the cheaper of two costs, nothing for the other or when
/// they are about the same, either is unknown, or either leaves out
/// calls with no known price.
pub(super) fn saving(a: Option<BenchCost>, b: Option<BenchCost>) -> [String; 2] {
    let (Some(a), Some(b)) = (a.and_then(BenchCost::known), b.and_then(BenchCost::known)) else {
        return [String::new(), String::new()];
    };
    let larger = a.max(b);
    if larger <= 0.0 {
        return [String::new(), String::new()];
    }
    let less = |cost: f64| {
        let less = ((1.0 - cost / larger) * 100.0).round();
        if less < 1.0 {
            String::new()
        } else {
            format!("-{less:.0}%")
        }
    };
    [less(a), less(b)]
}

/// `text` in `style` for something under way: on frame `frame` a band of
/// light from `base` to `light` sweeps across it; without a frame, as
/// when animations are off, it is plain.
pub(in crate::renderer) fn under_way(
    text: &str,
    style: Style,
    frame: Option<usize>,
    colors: (Color, Color),
) -> Vec<Span<'static>> {
    match frame {
        Some(frame) => crate::animation::shimmer_spans(text, frame, style, colors),
        None => vec![Span::styled(text.to_string(), style)],
    }
}

/// `text` cut to `max` columns, ending in `…` when cut.
pub(in crate::renderer) fn fit(text: &str, max: usize) -> String {
    if display_width(text) <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > max - 1 {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

/// The directory every example task's id shares, as `swe_bench_pro_hard`.
pub(in crate::renderer) fn task_prefix(view: &BenchView) -> Option<String> {
    let (prefix, _) = view.tasks.first()?.id.rsplit_once('/')?;
    let all = view.tasks.iter().all(|t| {
        t.id.strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
    });
    all.then(|| prefix.to_string())
}

/// A task id without the directory every task shares.
pub(super) fn task_name<'s>(id: &'s str, prefix: Option<&str>) -> &'s str {
    prefix
        .and_then(|p| id.strip_prefix(p))
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(id)
}

/// The first line of `text`, cut to `max` characters.
pub(super) fn headline(text: &str, max: usize) -> String {
    let line = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.chars().count() <= max {
        return line.to_string();
    }
    let cut: String = line.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// What an arm's runs of one example task did and their mean cost:
/// `✓` or `✗` for one checked run, `2/3` for more, `2×` for unchecked runs.
pub(super) fn task_cells(cell: &BenchTaskCell, theme: &Theme) -> [Cell; 2] {
    let text = Style::default().fg(theme.colors.text);
    if cell.runs == 0 {
        return [(String::new(), text), ("-".to_string(), text)];
    }
    let cost = cost_text(cell.mean_cost());
    let check = match (cell.checked, cell.passed) {
        (0, _) if cell.runs == 1 => (String::new(), text),
        (0, _) => (format!("{}×", cell.runs), text),
        (1, 1) => ("✓".to_string(), Style::default().fg(theme.colors.success)),
        (1, _) => ("✗".to_string(), Style::default().fg(theme.colors.error)),
        (checked, passed) => (format!("{passed}/{checked}"), text),
    };
    [check, (cost, text)]
}

/// `setting up: 14/20 tasks ready · 3 fetching · ...` while a running
/// benchmark still fetches some example task's files.
pub(super) fn setup_line(view: &BenchView) -> Option<String> {
    setup_counts(view).map(|counts| format!("setting up: {counts}"))
}

/// `14/20 tasks ready · 3 fetching · ...` while a running benchmark still
/// fetches some example task's files.
pub(in crate::renderer) fn setup_counts(view: &BenchView) -> Option<String> {
    let count = |f: fn(&BenchTaskSetup) -> bool| view.tasks.iter().filter(|t| f(&t.setup)).count();
    let fetching = count(|s| matches!(s, BenchTaskSetup::Fetching { .. }));
    let queued = count(|s| *s == BenchTaskSetup::Queued);
    if view.status != BenchStatus::Running || fetching + queued == 0 {
        return None;
    }
    let ready = count(|s| *s == BenchTaskSetup::Ready);
    let failed = count(|s| matches!(s, BenchTaskSetup::Failed(_)));
    let mut parts = vec![format!("{ready}/{} tasks ready", view.tasks.len())];
    if fetching > 0 {
        parts.push(format!("{fetching} fetching"));
    }
    if queued > 0 {
        parts.push(format!("{queued} queued"));
    }
    if failed > 0 {
        parts.push(format!("{failed} failed"));
    }
    if count(|s| *s == BenchTaskSetup::Fetching { pull: true }) > 0 {
        parts.push("pulling an image can take minutes".to_string());
    }
    Some(parts.join(" · "))
}

/// `$0.03 2✓ 1✗ 1 err`: mean turn cost, then turns whose test or build
/// command passed, failed, or that ended in an error.
pub(super) fn turn_cell(turn: Option<&BenchTurn>) -> String {
    let Some(turn) = turn.filter(|t| t.runs > 0) else {
        return "-".to_string();
    };
    let mut text = cost_text(turn.mean_cost());
    for (n, what) in [
        (turn.passed, "✓"),
        (turn.failed, "✗"),
        (turn.errors, " err"),
    ] {
        if n > 0 {
            text.push_str(&format!(" {n}{what}"));
        }
    }
    text
}

/// What a run of arm A or B cost next to the session it replays, marked
/// `+?` when the run's cost leaves out calls with no known price.
#[allow(clippy::cast_precision_loss)]
pub(super) fn ratio(arm: &BenchArm, baseline: f64) -> String {
    arm.median_cost().filter(|_| baseline > 0.0).map_or_else(
        || "-".to_string(),
        |cost| {
            let mark = if cost.is_complete() {
                ""
            } else {
                UNPRICED_MARK
            };
            format!("{:.2}×{mark}", cost.usd / baseline)
        },
    )
}

/// `A` or `B` for the arm the judge found better, `=` for a tie, `?` when
/// its verdict could not be read.
pub(super) fn verdict_mark(verdict: &BenchVerdict) -> &'static str {
    match (verdict.parsed, verdict.winner) {
        (false, _) => "?",
        (true, Some(arm)) => arm_letter(arm),
        (true, None) => "=",
    }
}

/// `+`, `=` or `-` for a run rated better than, as good as, or worse than
/// what the user kept.
pub(super) fn reference_mark(rating: Option<&str>) -> &'static str {
    match rating {
        Some("better") => "+",
        Some("as good") => "=",
        Some("worse") => "-",
        _ => "?",
    }
}

/// An arm's tokens per model and purpose: its ended runs' `done` and
/// what its runs under way, `live`, have used so far, most expensive
/// first.
pub(super) fn tokens_so_far<'a>(
    done: &[BenchTokens],
    live: impl Iterator<Item = &'a BenchLiveRun>,
) -> Vec<BenchTokens> {
    let mut rows = done.to_vec();
    for t in live.flat_map(|r| &r.tokens) {
        match (rows.iter_mut()).find(|row| row.model == t.model && row.purpose == t.purpose) {
            Some(row) => {
                row.calls += t.calls;
                row.input_tokens += t.input_tokens;
                row.output_tokens += t.output_tokens;
                row.cost_usd += t.cost_usd;
                row.unpriced_calls += t.unpriced_calls;
            }
            None => rows.push(t.clone()),
        }
    }
    rows.sort_by(|x, y| y.cost_usd.total_cmp(&x.cost_usd));
    rows
}

/// `gpt-6.1-terra · criteria` for calls made beside the runs' own work,
/// `sonnet-5-5` for the work's.
pub(super) fn tokens_label(t: &BenchTokens) -> String {
    let model = short_model(&t.model);
    match &t.purpose {
        Some(purpose) => format!("{model} · {purpose}"),
        None => model.to_string(),
    }
}
