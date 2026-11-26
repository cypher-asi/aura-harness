use super::*;

/// A cache plus a counter of how often the underlying `fs_read` ran.
#[derive(Default)]
struct ReadProbe {
    cache: ToolResultCache,
    fs_read_calls: usize,
}

impl ReadProbe {
    /// Read `start..=end` of `path`; on a miss the executor renders every
    /// line as `body`.
    fn read(
        &mut self,
        id: &str,
        path: &str,
        start: usize,
        end: usize,
        body: &str,
    ) -> ToolCallResult {
        drive_read_file(
            &mut self.cache,
            id,
            path,
            Some(start),
            Some(end),
            &mut self.fs_read_calls,
            |s, e| fake_read_file_rendered(s.unwrap(), e.unwrap(), body),
        )
    }

    /// Read `start..=end` of `path`, panicking with `miss_msg` if the
    /// cache misses.
    fn read_hit(
        &mut self,
        id: &str,
        path: &str,
        start: usize,
        end: usize,
        miss_msg: &str,
    ) -> ToolCallResult {
        drive_read_file(
            &mut self.cache,
            id,
            path,
            Some(start),
            Some(end),
            &mut self.fs_read_calls,
            |_, _| panic!("{miss_msg}"),
        )
    }
}

#[test]
fn read_file_range_cache_serves_subset_from_superset() {
    let mut probe = ReadProbe::default();
    let path = "src/lib.rs";

    let r_full = probe.read("tool_1", path, 1, 150, "alpha");
    let r_subset1 = probe.read_hit(
        "tool_2",
        path,
        1,
        99,
        "subset 1..99 must hit the range cache without re-executing fs_read",
    );
    let r_subset2 = probe.read_hit(
        "tool_3",
        path,
        30,
        100,
        "subset 30..100 must hit the range cache without re-executing fs_read",
    );

    assert_eq!(
        probe.fs_read_calls, 1,
        "only the superset read should trigger the underlying tool"
    );

    let canonical = "src/lib.rs";
    let entries = probe
        .cache
        .read_file_by_path
        .get(canonical)
        .expect("per-path index must have a vec for the superset path");
    assert_eq!(
        entries.len(),
        1,
        "subset hits must NOT mint new entries; superset is the only one"
    );

    // The superset's rendered body is the single source of truth for
    // every subset response; checking both subsets came from the same
    // entry (`fs_read_calls == 1` above) plus the line-number prefix
    // assertions below covers the contract end-to-end without
    // requiring the entry to carry a redundant `content_hash`.
    let body1 = r_subset1.content;
    let body2 = r_subset2.content;
    assert!(body1.contains("     1|alpha"));
    assert!(body1.contains("    99|alpha"));
    assert!(!body1.contains("   100|alpha"));
    assert!(body2.contains("    30|alpha"));
    assert!(body2.contains("   100|alpha"));
    assert!(!body2.contains("    29|alpha"));
    // The full superset response must contain every line both subsets
    // claim — confirms both subsets really did slice from the same
    // cached bytes.
    for line in 1..=100 {
        let needle = format!("{:>6}|alpha", line);
        assert!(
            r_full.content.contains(&needle),
            "superset must include line {line}"
        );
    }
}

#[test]
fn write_invalidates_only_overlapping_path() {
    let mut probe = ReadProbe::default();

    // Cache reads of two unrelated paths.
    probe.read("read_a", "crates/A/foo.rs", 1, 50, "A");
    probe.read("read_b", "crates/B/bar.rs", 1, 50, "B");
    assert_eq!(probe.fs_read_calls, 2);
    assert_eq!(probe.cache.read_file_by_path.len(), 2);
    assert_eq!(probe.cache.exact.len(), 2);

    // Successful write to crates/A/foo.rs only.
    let write_call = call(
        "tool_w",
        "write_file",
        serde_json::json!({"path": "crates/A/foo.rs", "content": "x"}),
    );
    let write_result = ok_result("tool_w", "wrote");
    update_cache(
        &mut probe.cache,
        std::slice::from_ref(&write_call),
        std::slice::from_ref(&write_result),
    );

    assert!(
        !probe
            .cache
            .read_file_by_path
            .contains_key("crates/A/foo.rs"),
        "overlapping read entry must be dropped"
    );
    assert!(
        probe
            .cache
            .read_file_by_path
            .contains_key("crates/B/bar.rs"),
        "non-overlapping read entry must be retained"
    );

    // Subsequent read on crates/B/bar.rs is a HIT (no new fs_read).
    probe.read_hit(
        "read_b2",
        "crates/B/bar.rs",
        1,
        50,
        "crates/B/bar.rs read should hit the surviving cache entry",
    );

    // Subsequent read on crates/A/foo.rs is a MISS (fresh fs_read).
    probe.read("read_a2", "crates/A/foo.rs", 1, 50, "A2");

    assert_eq!(
        probe.fs_read_calls, 3,
        "only the invalidated path should have triggered a re-read"
    );
}
