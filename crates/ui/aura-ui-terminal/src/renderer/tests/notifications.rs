use super::*;

#[test]
fn test_notification_sits_under_the_right_column_leaving_the_panels_alone() {
    let mut app = app_with_model(None);
    let before = render_rows(&mut app, 120);
    app.notify_copied(4, &Ok(()));
    let rows = render_rows(&mut app, 120);

    let n = rows.len();
    assert_eq!(rows[..n - 1], before[..n - 1], "no panel is covered");
    let (_, _, chat_right) = chat_panel(&rows);
    let footer: Vec<char> = rows[n - 1].chars().collect();
    let notice: String = footer[chat_right + 1..].iter().collect();
    assert!(
        notice.trim_end().ends_with("✓ Copied 4 characters"),
        "{notice:?}"
    );
    assert_eq!(
        rows[n - 1].trim_end().chars().count(),
        119,
        "{:?}",
        rows[n - 1]
    );
    let chat_footer: String = footer[..=chat_right].iter().collect();
    assert_eq!(
        chat_footer,
        before[n - 1]
            .chars()
            .take(chat_right + 1)
            .collect::<String>(),
        "the footer stays visible"
    );
}

fn long_warning(app: &mut crate::App) {
    let words: Vec<String> = (0..40).map(|i| format!("w{i}")).collect();
    app.notifications.current = Some((words.join(" "), crate::app::NotificationType::Warning));
    app.tick();
}

#[test]
fn test_long_notification_wraps_over_the_bottom_third_of_the_lowest_right_panel() {
    let mut app = app_with_model(None);
    let before = render_rows(&mut app, 120);
    long_warning(&mut app);
    let rows = render_rows(&mut app, 120);

    let n = rows.len();
    let (_, _, chat_right) = chat_panel(&rows);
    let right = |row: &str| row.chars().skip(chat_right + 1).collect::<String>();
    let top = rows.iter().position(|row| row.contains("SBX")).unwrap() - 1;
    let bottom = n - 2;
    let third = (bottom + 1 - top) / 3;
    let changed: Vec<usize> = (0..n).filter(|&y| rows[y] != before[y]).collect();
    assert_eq!(changed.len(), third, "{changed:?}");
    assert!(
        changed.iter().all(|&y| y >= bottom - third && y < bottom),
        "only the bottom third of the panel, inside its border: {changed:?}"
    );

    let rule = right(&rows[changed[0]]);
    assert!(rule.trim().starts_with('├'), "{rule:?}");
    assert!(rule.trim().ends_with('┤'), "{rule:?}");
    assert!(right(&rows[changed[1]]).contains("! w0 w1 w2"), "{rows:?}");
    assert!(right(&rows[changed[2]]).contains("w7"), "wraps: {rows:?}");
    assert_eq!(rows[n - 1], before[n - 1], "the footer row stays clear");
}

#[test]
fn test_wrapped_notification_scrolls_up_to_its_end_and_stays_until_read() {
    let mut app = app_with_model(None);
    long_warning(&mut app);
    let notice = |rows: &[String]| rows[rows.len() - 5..rows.len() - 2].join("\n");
    let rows = render_rows(&mut app, 120);
    assert!(notice(&rows).contains("! w0 "), "{rows:?}");
    assert!(!notice(&rows).contains("w39"), "{rows:?}");

    let (_, until) = app.notifications.shown.as_ref().unwrap();
    let since = app.notifications.since.unwrap();
    assert!(
        *until > since + std::time::Duration::from_secs(5),
        "held for the scroll"
    );

    let long_ago = std::time::Instant::now() - std::time::Duration::from_secs(60);
    app.notifications.since = Some(long_ago);
    let rows = render_rows(&mut app, 120);
    let shown = notice(&rows);
    assert!(shown.contains("w39"), "ends at the end: {shown}");
    assert!(shown.contains("! "), "the icon stays: {shown}");
    assert!(!shown.contains("w0 "), "{shown}");
}

#[test]
fn test_long_notification_scrolls_along_the_footer_without_a_right_column() {
    let mut app = app_with_model(None);
    app.panels.record_visible = false;
    app.panels.sandbox_visible = false;
    long_warning(&mut app);
    let rows = render_rows(&mut app, 120);
    let last = rows.last().unwrap();
    assert!(last.contains("! w0 w1 w2"), "starts at the start: {last:?}");
    assert!(!last.contains("w39"), "{last:?}");

    let (_, until) = app.notifications.shown.as_ref().unwrap();
    let since = app.notifications.since.unwrap();
    assert!(
        *until > since + std::time::Duration::from_secs(5),
        "held for the scroll"
    );

    let long_ago = std::time::Instant::now() - std::time::Duration::from_secs(60);
    app.notifications.since = Some(long_ago);
    let rows = render_rows(&mut app, 120);
    let last = rows.last().unwrap();
    assert!(
        last.contains("w38 w39  ● Ready"),
        "ends at the end: {last:?}"
    );
    assert!(last.contains("! "), "the icon stays: {last:?}");
    assert!(!last.contains("w0 "), "{last:?}");
}

#[test]
fn test_notification_uses_the_footer_gap_without_a_right_column() {
    let mut app = app_with_model(None);
    app.panels.record_visible = false;
    app.panels.sandbox_visible = false;
    app.notify_copied(4, &Ok(()));
    let rows = render_rows(&mut app, 120);

    let footer = rows.last().unwrap();
    assert!(footer.contains("✓ Copied 4 characters"), "{footer:?}");
    assert!(footer.contains("qwen3:4b"), "the stats stay: {footer:?}");
    assert!(footer.contains("Ready"), "{footer:?}");
    assert!(footer.trim_end().ends_with("⌂ local"), "{footer:?}");
}
