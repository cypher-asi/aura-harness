use super::*;

#[test]
fn a_run_under_way_shows_its_route_and_what_it_does() {
    let theme = Theme::default();
    let view = under_way_view();
    let shown = text(&detail_lines(&view, None, &theme, 120, None));
    let live = run_line(&shown, "a-2");
    let words: Vec<&str> = live.split_whitespace().collect();
    assert!(
        words
            .windows(3)
            .any(|w| w == ["opus-5-5", "multi-file", "hard"]),
        "the model it went to and how it was classified: {live}"
    );
    let status = live
        .find("writing 49 files")
        .unwrap_or_else(|| panic!("{live}"));
    let cost = live.find("$0.80").unwrap_or_else(|| panic!("{live}"));
    assert!(cost < status, "the status comes after the columns: {live}");
    assert!(shown.contains("ansible-b748edea"), "{shown}");
}

#[test]
fn a_longer_status_of_a_run_under_way_does_not_move_the_table() {
    let theme = Theme::default();
    let mut view = under_way_view();
    let shown = text(&detail_lines(&view, None, &theme, 120, None));
    view.live[0].doing = Some(BenchDoing {
        what: "running run_command, read_file, write_file, and a long list of tools".to_string(),
        for_ms: 3_000,
        ..BenchDoing::default()
    });
    let longer = text(&detail_lines(&view, None, &theme, 120, None));
    for name in ["a-1", "a-2"] {
        let (before, after) = (run_line(&shown, name), run_line(&longer, name));
        let at = |line: &str, cell: &str| line.find(cell).unwrap_or_else(|| panic!("{line}"));
        for cell in ["$", "ansible-"] {
            assert_eq!(
                at(before, cell),
                at(after, cell),
                "{name}:\n{shown}\n{longer}"
            );
        }
    }
    for line in longer.lines() {
        assert!(display_width(line) <= 120, "{line}");
    }
}

#[test]
fn a_running_benchmark_shows_how_far_its_setup_got() {
    let shown = running_setup_shown(&running_setup_view(), 140);
    assert!(
            shown.contains(
                "setting up: 1/4 tasks ready · 1 fetching · 1 queued · 1 failed · pulling an image can take minutes"
            ),
            "{shown}"
        );
    let row = |start: &str| line_of(&shown, start).trim_end().to_string();
    assert!(row("flipt-2 ").ends_with("pulling image"), "{shown}");
    assert!(row("flipt-3 ").ends_with("queued"), "{shown}");
    assert!(row("navidrome-4 ").ends_with("failed: no image"), "{shown}");
}

#[test]
fn a_running_benchmark_shows_the_runs_working_and_waiting() {
    let shown = running_setup_shown(&running_setup_view(), 140);
    let live = |status: &str| {
        shown
            .lines()
            .skip_while(|l| !l.contains("── runs"))
            .find(|l| l.contains(status))
            .unwrap_or_else(|| panic!("{status}: {shown}"))
            .to_string()
    };
    let working = live("working");
    assert!(working.contains(" a-1 "), "{working}");
    assert!(working.contains("ansible-1"), "{working}");
    assert!(working.contains("1m12s"), "{working}");
    assert!(working.contains("$0.04"), "{working}");
    assert!(
        live("waiting for its task's files").contains(" b-2 "),
        "{shown}"
    );
}

#[test]
fn a_running_benchmark_counts_the_runs_under_way_in_its_summary() {
    let shown = running_setup_shown(&running_setup_view(), 140);
    let row = |start: &str| line_of(&shown, start).trim_end().to_string();
    let finished = row("finished ");
    assert!(finished.contains("2/8, 1 working"), "{finished}");
    assert!(finished.ends_with("2/8"), "{finished}");
    assert!(row("cost ").contains("$0.16"), "spent so far: {shown}");
    assert!(row("A  sonnet-4-6 ").contains("42k"), "{shown}");
}

#[test]
fn a_run_under_way_shows_its_subtask_and_how_often_it_restarted() {
    let mut view = running_setup_view();
    view.live[0].doing = Some(BenchDoing {
        what: "running task".to_string(),
        for_ms: 492_000,
        subtask: Some(Box::new(BenchDoing {
            what: "thinking".to_string(),
            for_ms: 100_000,
            call: 4,
            restarts: 2,
            subtask: None,
        })),
        ..BenchDoing::default()
    });
    let shown = running_setup_shown(&view, 200);
    let a_1 = shown
        .lines()
        .find(|l| l.contains(" a-1 ") && l.contains("ansible-1"))
        .unwrap_or_else(|| panic!("{shown}"));
    assert!(
        a_1.contains("running task 8m12s › thinking 1m40s · call 4 · restarted 2×"),
        "{shown}"
    );
}

#[test]
fn runs_show_in_number_order_with_a_rule_between_numbers() {
    let theme = Theme::default();
    let mut view = view();
    view.status = BenchStatus::Running;
    view.runs[0].name = "b-1".to_string();
    view.runs[0].arm = 1;
    let live = |name: &str, arm| BenchLiveRun {
        name: name.to_string(),
        arm,
        wall_ms: Some(1_000),
        ..BenchLiveRun::default()
    };
    view.live = vec![live("b-2", 1), live("a-1", 0), live("a-2", 0)];
    let shown = text(&detail_lines(&view, None, &theme, 140, None));
    let runs: Vec<&str> = shown
        .lines()
        .skip_while(|l| !l.starts_with("── runs"))
        .skip(1)
        .take_while(|l| !l.starts_with("note:"))
        .collect();
    let named: Vec<&str> = runs
        .iter()
        .filter_map(|l| {
            ["a-1", "b-1", "a-2", "b-2"]
                .into_iter()
                .find(|name| l.contains(&format!(" {name} ")))
        })
        .collect();
    assert_eq!(named, ["a-1", "b-1", "a-2", "b-2"], "{shown}");
    let at = |name: &str| {
        runs.iter()
            .position(|l| l.contains(&format!(" {name} ")))
            .unwrap()
    };
    let between = |from: &str, to: &str| {
        runs[at(from) + 1..at(to)]
            .iter()
            .filter(|l| l.trim().chars().all(|c| c == '─') && !l.trim().is_empty())
            .count()
    };
    assert_eq!(between("a-1", "b-1"), 0, "{shown}");
    assert_eq!(between("b-1", "a-2"), 1, "{shown}");
    assert_eq!(between("a-2", "b-2"), 0, "{shown}");
}

#[test]
fn runs_under_way_add_to_tokens_used_so_far() {
    let token = |model: &str, cost: f64| BenchTokens {
        model: model.to_string(),
        purpose: None,
        calls: 1,
        input_tokens: 1_000,
        output_tokens: 100,
        cost_usd: cost,
        unpriced_calls: 0,
    };
    let live = [
        BenchLiveRun {
            tokens: vec![token("opus", 0.5), token("haiku", 0.01)],
            ..BenchLiveRun::default()
        },
        BenchLiveRun {
            tokens: vec![
                token("sonnet", 0.2),
                BenchTokens {
                    unpriced_calls: 1,
                    ..token("sonnet", 0.0)
                },
            ],
            ..BenchLiveRun::default()
        },
    ];
    let used = tokens_so_far(&[token("sonnet", 0.1)], live.iter());
    let models: Vec<&str> = used.iter().map(|t| t.model.as_str()).collect();
    assert_eq!(models, ["opus", "sonnet", "haiku"]);
    assert_eq!((used[1].calls, used[1].unpriced_calls), (3, 1));
    assert!((used[1].cost_usd - 0.3).abs() < 1e-9);
}

#[test]
fn calls_beside_the_work_are_named_for_what_they_were_for() {
    let theme = Theme::default();
    let token = |model: &str, purpose: Option<&str>, calls, cost_usd| BenchTokens {
        model: model.to_string(),
        purpose: purpose.map(str::to_string),
        calls,
        cost_usd,
        ..BenchTokens::default()
    };
    let sonnet = "claude/claude-sonnet-5-5";
    let terra = "openrouter/openai/gpt-6.1-terra";
    let mut view = view();
    view.arms[0].tokens = vec![
        token(sonnet, None, 20, 0.28),
        token(sonnet, Some("judge"), 1, 0.02),
        token(terra, Some("criteria"), 1, 0.01),
    ];
    view.live = vec![BenchLiveRun {
        name: "a-2".to_string(),
        tokens: vec![token(terra, Some("criteria"), 2, 0.02)],
        ..BenchLiveRun::default()
    }];
    let shown = text(&detail_lines(&view, None, &theme, 120, None));
    let calls = |label: &str| {
        let row = (shown.lines())
            .skip_while(|l| !l.starts_with("── tokens"))
            .find(|l| l.contains(label))
            .unwrap_or_else(|| panic!("no {label:?} row in\n{shown}"));
        let (_, after) = row.split_once('│').unwrap();
        after.split_whitespace().next().unwrap().to_string()
    };
    assert_eq!(calls("A  sonnet-5-5 "), "20", "the work's own row");
    assert_eq!(calls("sonnet-5-5 · judge"), "1");
    assert_eq!(
        calls("gpt-6.1-terra · criteria"),
        "3",
        "a run under way adds to its row"
    );
}

#[test]
fn runs_under_way_add_to_each_arms_total_time() {
    let theme = Theme::default();
    let mut view = view();
    view.status = BenchStatus::Running;
    view.runs.push(BenchRun {
        name: "b-1".to_string(),
        arm: 1,
        wall_ms: 3_600_000,
        ..BenchRun::default()
    });
    let live = |name: &str, arm, wall_ms| BenchLiveRun {
        name: name.to_string(),
        arm,
        wall_ms,
        ..BenchLiveRun::default()
    };
    view.live = vec![
        live("a-2", 0, Some(30_000)),
        live("b-2", 1, Some(300_000)),
        live("a-3", 0, None),
    ];
    let shown = text(&detail_lines(&view, None, &theme, 120, None));
    let row = |label: &str| {
        shown
            .lines()
            .find(|l| l.starts_with(&format!("{label} ")))
            .unwrap_or_else(|| panic!("no {label} row in\n{shown}"))
            .trim_end()
            .to_string()
    };
    let total = row("total time");
    assert!(total.contains(" 1m12s "), "42s ended, 30s so far: {total}");
    assert!(total.ends_with(" 1h05m"), "1h ended, 5m so far: {total}");
    assert!(row("time").contains(" 42s "), "the median run: {shown}");

    view.runs.clear();
    view.live.clear();
    let shown = text(&detail_lines(&view, None, &theme, 120, None));
    let total = shown
        .lines()
        .find(|l| l.starts_with("total time "))
        .unwrap_or_else(|| panic!("{shown}"));
    assert!(total.trim_end().ends_with(" -"), "nothing ran yet: {total}");
}

#[test]
fn running_and_working_shimmer_while_it_runs() {
    let theme = Theme::default();
    let mut view = view();
    view.status = BenchStatus::Running;
    view.live = vec![
        BenchLiveRun {
            name: "a-2".to_string(),
            wall_ms: Some(5_000),
            ..BenchLiveRun::default()
        },
        BenchLiveRun {
            name: "b-2".to_string(),
            arm: 1,
            ..BenchLiveRun::default()
        },
    ];
    // Lit when the word's letters are in more than one color.
    let lit = |line: &Line<'_>, word: &str| {
        let mut colors: Vec<_> = line
            .spans
            .iter()
            .filter(|s| !s.content.trim().is_empty() && word.contains(s.content.trim()))
            .map(|s| format!("{:?}", s.style.fg))
            .collect();
        colors.dedup();
        colors.len() > 1
    };
    let find = |lines: &[Line<'static>], status: &str| {
        let runs = lines
            .iter()
            .position(|l| text(std::slice::from_ref(l)).contains("── runs"))
            .unwrap_or_default();
        lines[runs..]
            .iter()
            .find(|l| text(std::slice::from_ref(*l)).contains(status))
            .cloned()
            .unwrap_or_else(|| panic!("{status}: {}", text(lines)))
    };

    // On frame 3 the band's center is on the label's first letter.
    let lines = detail_lines(&view, None, &theme, 140, Some(3));
    assert!(lit(&lines[0], "running"), "{:?}", lines[0]);
    assert!(lit(&find(&lines, "working"), "working"));
    let waiting = find(&lines, "waiting for its task's files");
    assert!(
        !lit(&waiting, "waiting for its task's files"),
        "{waiting:?}"
    );

    let still = detail_lines(&view, None, &theme, 140, None);
    assert!(!lit(&still[0], "running"), "{:?}", still[0]);
    assert!(!lit(&find(&still, "working"), "working"));

    view.status = BenchStatus::Done;
    let done = detail_lines(&view, None, &theme, 140, Some(3));
    assert!(!lit(&done[0], "done"), "{:?}", done[0]);
}
