//! The task records a run's subtasks left, kept out of its check's way.

use super::super::check::RecordsAside;
use crate::event_loop::tasks::TASKS_DIR;

#[test]
fn task_records_are_out_of_the_copy_while_its_check_runs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("a-3");
    let records = root.join(TASKS_DIR);
    std::fs::create_dir_all(&records).unwrap();
    std::fs::write(records.join("1.json"), "rename calc_total").unwrap();

    let aside = RecordsAside::take(&root, &root, "a-3");
    assert!(
        !records.exists(),
        "a check grepping the copy can't see them"
    );
    aside.put_back();
    assert_eq!(
        std::fs::read_to_string(records.join("1.json")).unwrap(),
        "rename calc_total"
    );
    assert!(!dir.path().join("a-3.tasks").exists());

    let none = tempfile::tempdir().unwrap();
    RecordsAside::take(none.path(), none.path(), "b-1").put_back();
    assert!(!none.path().join(TASKS_DIR).exists());
}
