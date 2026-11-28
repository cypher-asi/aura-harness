use super::*;
use chrono::{Local, TimeZone};

/// Noon of March `day` 2026, local time, in ms since the epoch.
fn on(day: u32) -> u64 {
    let noon = Local
        .with_ymd_and_hms(2026, 3, day, 12, 0, 0)
        .single()
        .unwrap();
    u64::try_from(noon.timestamp_millis()).unwrap()
}

/// `task`, last moved on at `ms`.
fn at(ms: u64, task: TaskView) -> TaskView {
    TaskView {
        updated_ms: ms,
        ..task
    }
}

/// `plan`, its file last changed at `ms`.
fn changed(ms: u64, plan: PlanView) -> PlanView {
    PlanView {
        changed_ms: Some(ms),
        ..plan
    }
}

#[test]
fn groups_go_under_the_day_they_last_moved_on_newest_first() {
    let on_b = TaskView {
        plan: Some(path("b")),
        state: C,
        ..request("rb", "lead", Vec::new())
    };
    let list = list_of(
        vec![
            changed(on(2), plan("a", &[Done])),
            changed(on(1), plan("b", &[Done])),
            plan("c", &[Done]),
        ],
        vec![
            at(on(4), on_b),
            at(on(3), request("r", "lead", vec![item("x", Done, None)])),
        ],
    );
    assert_eq!(
        shape(&list),
        [
            "=2026-03-04",
            "+b",
            "=2026-03-03",
            "-No Plan",
            "x",
            "=2026-03-02",
            "+a",
            "=Earlier",
            "+c"
        ],
        "b's file is older than a's, but a task moved it on since"
    );
}

#[test]
fn within_a_day_the_plan_being_worked_on_comes_first_then_the_last_moved() {
    let hour = 3_600_000;
    let mut list = list_of(
        vec![
            changed(on(4) - hour, plan("early", &[Done])),
            changed(on(4) + hour, plan("late", &[Done])),
            changed(on(4) - hour, plan("built", &[Done])),
        ],
        vec![at(on(4), request("r", "lead", vec![item("x", Done, None)]))],
    );
    list.set_current_plan(Some(&changed(on(4) - hour, plan("built", &[Done]))));
    assert_eq!(
        shape(&list),
        [
            "=2026-03-04",
            "-built",
            "#built 0",
            "+late",
            "+early",
            "-No Plan",
            "x"
        ]
    );
}

#[test]
fn an_agents_list_carried_over_goes_under_its_newest_day_and_folds_by_day() {
    let lead = |id: &str, items: &[&str]| {
        let items = items.iter().map(|content| item(content, Done, None));
        request(id, "lead", items.collect())
    };
    let mut list = list_of(
        Vec::new(),
        vec![
            at(on(4), lead("r2", &["y", "z"])),
            at(on(2), request("o1", "other", vec![item("x", Done, None)])),
            at(on(2), lead("r1", &["x", "y"])),
            at(on(1), lead("r0", &["w"])),
        ],
    );
    assert_eq!(
        shape(&list),
        [
            "=2026-03-04",
            "-No Plan",
            "x",
            "y",
            "z",
            "=2026-03-02",
            "-No Plan",
            "x",
            "=2026-03-01",
            "-No Plan",
            "w"
        ],
        "r1's list goes on in r2, so both are listed on r2's day; other's x is its own"
    );

    list.select(6);
    list.toggle_selected();
    list.set_tasks(list.tasks().to_vec());
    assert_eq!(
        shape(&list),
        [
            "=2026-03-04",
            "-No Plan",
            "x",
            "y",
            "z",
            "=2026-03-02",
            "+No Plan",
            "=2026-03-01",
            "-No Plan",
            "w"
        ],
        "each day's No Plan folds on its own, and stays folded"
    );
}

/// An app on TASKS with work of no plan on March 4 and on March 2.
fn two_days() -> App {
    app_on_tasks(
        Vec::new(),
        vec![
            at(on(4), request("new", "lead", vec![item("n", Todo, None)])),
            at(on(2), request("old", "lead", vec![item("o", Todo, None)])),
        ],
    )
}

#[test]
fn the_highlight_passes_over_a_days_header() {
    let mut app = two_days();
    assert_eq!(
        shape(app.task_list()),
        [
            "=2026-03-04",
            "-No Plan",
            "n",
            "=2026-03-02",
            "-No Plan",
            "o"
        ]
    );
    assert_eq!(app.task_list().selected(), 1, "the first header under it");
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    assert_eq!(app.task_list().selected(), 4, "past the day going down");
    press(&mut app, KeyCode::Up);
    assert_eq!(app.task_list().selected(), 2, "and going up");
    press(&mut app, KeyCode::PageUp);
    assert_eq!(app.task_list().selected(), 1);
    press(&mut app, KeyCode::End);
    assert_eq!(app.task_list().selected(), 5);
    press(&mut app, KeyCode::Home);
    assert_eq!(app.task_list().selected(), 1);
    app.task_list.select(3);
    assert_eq!(
        app.task_list().selected(),
        4,
        "a day picked lights the row under it"
    );
}

#[test]
fn a_days_header_scrolls_in_with_the_first_row_under_it() {
    let mut app = two_days();
    press(&mut app, KeyCode::End);
    assert_eq!(app.sync_tasks_scroll(2), 4, "the last two rows");
    press(&mut app, KeyCode::Up);
    assert_eq!(
        app.sync_tasks_scroll(2),
        3,
        "the day comes in with its No Plan"
    );
    press(&mut app, KeyCode::Up);
    assert_eq!(app.sync_tasks_scroll(2), 2);
    press(&mut app, KeyCode::Up);
    assert_eq!(app.sync_tasks_scroll(2), 0, "so does the first day");
}

#[test]
fn selecting_a_task_opens_the_no_plan_group_of_its_day() {
    let mut list = list_of(
        Vec::new(),
        vec![
            at(on(4), request("new", "lead", vec![item("n", Todo, None)])),
            at(on(2), part("p", "old", C)),
            at(
                on(2),
                request("old", "lead", vec![item("o", Done, Some("p"))]),
            ),
        ],
    );
    list.select(4);
    list.toggle_selected();
    assert_eq!(
        shape(&list),
        ["=2026-03-04", "-No Plan", "n", "=2026-03-02", "+No Plan"]
    );

    assert!(list.select_task("p"));
    assert_eq!(
        shape(&list),
        [
            "=2026-03-04",
            "-No Plan",
            "n",
            "=2026-03-02",
            "-No Plan",
            "-o",
            "  @p"
        ]
    );
    assert_eq!(list.selected(), 6);

    list.select(4);
    list.toggle_selected();
    assert!(!list.select_task("old"), "a request has no row");
    assert_eq!(list.selected(), 4, "its day's No Plan header is lit");
    assert_eq!(shape(&list)[4], "-No Plan", "and opened");
}
