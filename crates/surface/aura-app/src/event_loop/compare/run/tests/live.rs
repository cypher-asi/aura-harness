//! What the BENCH tab says a run under way is doing.

use super::super::live::bench_doing;
use aura_agent_trace::{Activity, Doing};
use aura_ui::events::BenchDoing;

#[test]
fn a_run_under_way_says_what_its_model_is_doing() {
    let doing = |doing: Doing| {
        let mut activity = Activity::default();
        (activity.doing, activity.since_ms) = (doing, 1_000);
        (activity.calls, activity.restarts) = (2, 1);
        let BenchDoing {
            what, for_ms, call, ..
        } = bench_doing(&activity, 4_500);
        (what, for_ms, call)
    };
    let tools = Doing::Tools(vec!["run_command".to_string(), "task".to_string()]);
    assert_eq!(
        [Doing::Waiting, Doing::Thinking, Doing::Writing, tools].map(doing),
        [
            ("waiting for the model".to_string(), 3_500, 3),
            ("thinking".to_string(), 3_500, 3),
            ("writing".to_string(), 3_500, 3),
            ("running run_command, task".to_string(), 3_500, 0),
        ]
    );
    let mut activity = Activity::default();
    activity.restarts = 2;
    let shown = bench_doing(&activity, 0);
    assert_eq!((shown.restarts, shown.subtask), (2, None));
}
