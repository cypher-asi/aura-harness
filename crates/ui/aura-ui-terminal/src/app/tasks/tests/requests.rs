use super::*;

#[test]
fn requests_have_no_row_their_items_and_parts_do_at_the_left_edge() {
    let list = list_of(
        Vec::new(),
        vec![
            part("p", "r", R),
            request(
                "r",
                "lead",
                vec![item("x", Done, None), item("y", Todo, None)],
            ),
        ],
    );
    assert_eq!(shape(&list), ["-No Plan", "x", "y", "@p"]);
    assert!(
        list.rows().iter().all(|row| row.depth() == 0),
        "icons sit under the header's arrow"
    );
}

#[test]
fn a_request_with_nothing_to_list_adds_no_rows() {
    let list = list_of(Vec::new(), vec![request("r", "lead", Vec::new())]);
    assert!(list.rows().is_empty(), "{:?}", shape(&list));
}

#[test]
fn an_agents_list_is_merged_across_its_requests_newest_state_wins() {
    let list = list_of(
        Vec::new(),
        vec![
            request(
                "commit",
                "lead",
                vec![
                    item("a", Done, None),
                    item("b", Done, None),
                    item("c", Todo, None),
                ],
            ),
            request(
                "impl",
                "lead",
                vec![item("a", Done, None), item("b", Doing, None)],
            ),
        ],
    );
    assert_eq!(shape(&list), ["-No Plan", "a", "b", "c"]);
    let TaskRow::Item { task, item, .. } = list.rows()[2] else {
        panic!("an item row");
    };
    assert_eq!(list.item(task, item).map(|i| i.state), Some(Done));
}

#[test]
fn the_agent_last_at_work_comes_first() {
    let list = list_of(
        Vec::new(),
        vec![
            request("new", "b", vec![item("b's", Todo, None)]),
            request("old", "a", vec![item("a's", Todo, None)]),
        ],
    );
    assert_eq!(shape(&list), ["-No Plan", "b's", "a's"]);
}

#[test]
fn a_task_sits_one_level_under_the_item_it_does_even_after_the_list_moves_on() {
    let tasks = vec![
        request(
            "later",
            "lead",
            vec![item("x", Todo, None), item("y", Todo, None)],
        ),
        part("p2", "first", R),
        part("p1", "first", R),
        request(
            "first",
            "lead",
            vec![item("x", Todo, Some("p1")), item("y", Todo, None)],
        ),
    ];
    let list = list_of(Vec::new(), tasks);
    assert_eq!(
        shape(&list),
        ["-No Plan", "-x", "  @p1", "y", "@p2"],
        "p1 does x, which the later list carries without the link"
    );
}

#[test]
fn a_tasks_own_parts_nest_under_it_and_its_items_are_not_listed() {
    let list = list_of(
        Vec::new(),
        vec![
            part("q", "p", R),
            part("p", "r", R),
            request("r", "lead", Vec::new()),
        ],
    );
    assert_eq!(shape(&list), ["-No Plan", "-@p", "  @q"]);
}

#[test]
fn done_items_fold_their_tasks_and_folds_stick() {
    let tasks = || {
        vec![
            part("p1", "r", C),
            request("r", "lead", vec![item("x", Done, Some("p1"))]),
        ]
    };
    let mut app = app_on_tasks(Vec::new(), tasks());
    assert_eq!(shape(app.task_list()), ["-No Plan", "+x"]);

    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Right);
    assert_eq!(shape(app.task_list()), ["-No Plan", "-x", "  @p1"]);
    app.process_command(UiCommand::SetTasks(tasks()));
    assert_eq!(
        shape(app.task_list()),
        ["-No Plan", "-x", "  @p1"],
        "the fold survives updates"
    );
    assert_eq!(selected_shape(&app), "-x", "and so does the highlight");

    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Left);
    assert_eq!(
        shape(app.task_list()),
        ["-No Plan", "+x"],
        "Left on a task with nothing under it folds the item it is under"
    );
    assert_eq!(selected_shape(&app), "+x");
}

#[test]
fn a_new_request_keeps_the_highlight_on_its_item() {
    let mut app = app_on_tasks(
        Vec::new(),
        vec![request(
            "one",
            "lead",
            vec![item("x", Todo, None), item("y", Todo, None)],
        )],
    );
    press(&mut app, KeyCode::End);
    assert_eq!(selected_shape(&app), "y");
    app.process_command(UiCommand::UpsertTask(request(
        "two",
        "other",
        vec![item("z", Todo, None)],
    )));
    assert_eq!(shape(app.task_list()), ["-No Plan", "z", "x", "y"]);
    assert_eq!(selected_shape(&app), "y");
}
