//! Read-only tasks run side by side; write tasks run one at a time.

use super::*;
use aura_core_types::SubagentExit;
use aura_ui::events::{ProcessKind, ProcessStatus};

/// Task rows of the Processes tab that are running.
fn count_running(fixture: &Fixture) -> usize {
    fixture
        .hub
        .processes
        .snapshot()
        .iter()
        .filter(|row| row.kind == ProcessKind::Task && row.status == ProcessStatus::Running)
        .count()
}

/// Wait until `n` task rows of the Processes tab are running.
async fn running_tasks(fixture: &Fixture, n: usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while count_running(fixture) != n {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{} tasks running, not {n}", count_running(fixture)));
}

#[tokio::test]
async fn read_only_tasks_run_side_by_side_and_write_tasks_one_at_a_time() {
    let (fixture, _commands) = fixture();
    let first = fixture.first;
    fixture.hub.set_permissions(first, yolo());
    let both = |kind: &'static str| {
        let fixture = &fixture;
        async move {
            tokio::join!(
                dispatch(fixture, task(first, kind, "slow look at a", None)),
                dispatch(fixture, task(first, kind, "slow look at b", None)),
            )
        }
    };

    let reads = both("explorer");
    let watch = async {
        running_tasks(&fixture, 2).await;
        fixture.gate.add_permits(2);
    };
    let ((a, b), ()) = tokio::join!(reads, watch);
    assert!(matches!(a.exit, SubagentExit::Completed));
    assert!(matches!(b.exit, SubagentExit::Completed));

    let writes = both("implement");
    let watch = async {
        running_tasks(&fixture, 1).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(count_running(&fixture), 1, "the second edit waits its turn");
        fixture.gate.add_permits(1);
        running_tasks(&fixture, 1).await;
        fixture.gate.add_permits(1);
    };
    let ((a, b), ()) = tokio::join!(writes, watch);
    assert!(matches!(a.exit, SubagentExit::Completed));
    assert!(matches!(b.exit, SubagentExit::Completed));
}
