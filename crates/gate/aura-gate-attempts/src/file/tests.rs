//! Appending to the log and reading it back.

use std::fs;
use std::io::Write as _;

use super::{read, AttemptLog, Read};
use crate::samples::{log_path, older, written, LOG};

#[test]
fn lines_round_trip_through_the_file() {
    let dir = std::env::temp_dir().join(format!("aura-attempt-log-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let log = AttemptLog::in_data_dir(&dir);
    assert_eq!(log.read().unwrap(), Read::default(), "no file yet");
    let lines = written();
    for line in &lines {
        log.append(line).unwrap();
    }
    fs::OpenOptions::new()
        .append(true)
        .open(log.path())
        .unwrap()
        .write_all(b"not json\n\n")
        .unwrap();
    let read = log.read().unwrap();
    assert_eq!(read.lines, lines);
    assert_eq!(read.unreadable, 1);
    let held: String = LOG
        .lines()
        .take(lines.len())
        .map(|line| format!("{line}\n"))
        .collect();
    assert_eq!(
        fs::read_to_string(log.path()).unwrap(),
        held + "not json\n\n",
        "each line is written as the sample log holds it"
    );
    assert!(log.path().ends_with("gate/attempts.jsonl"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_sample_log_reads_back_as_written() {
    let mut lines = written();
    lines.extend(older());
    assert_eq!(
        read(&log_path()).unwrap(),
        Read {
            lines,
            unreadable: 2
        }
    );
}
