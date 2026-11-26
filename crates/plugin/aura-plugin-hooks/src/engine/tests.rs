use super::process::resolve_command;
use super::*;

fn root() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\plugin\root")
    } else {
        PathBuf::from("/plugin/root")
    }
}

#[test]
fn resolve_bare_filename_does_not_join_root() {
    let r = root();
    let p = resolve_command("python3", &r);
    assert_eq!(p, PathBuf::from("python3"));
}

#[test]
fn resolve_relative_path_with_separator_joins_root() {
    let r = root();
    let p = resolve_command("./hooks/pre.sh", &r);
    assert_eq!(p, r.join("./hooks/pre.sh"));
}

#[test]
fn resolve_relative_subdir_path_joins_root() {
    let r = root();
    let p = resolve_command("hooks/pre.sh", &r);
    assert_eq!(p, r.join("hooks/pre.sh"));
}

#[cfg(windows)]
#[test]
fn resolve_windows_backslash_path_joins_root() {
    let r = root();
    let p = resolve_command(r"hooks\pre.cmd", &r);
    assert_eq!(p, r.join(r"hooks\pre.cmd"));
}

#[cfg(unix)]
#[test]
fn resolve_absolute_path_used_as_is_unix() {
    let r = root();
    let p = resolve_command("/usr/bin/true", &r);
    assert_eq!(p, PathBuf::from("/usr/bin/true"));
}

#[cfg(windows)]
#[test]
fn resolve_absolute_path_used_as_is_windows() {
    let r = root();
    let p = resolve_command(r"C:\Windows\System32\cmd.exe", &r);
    assert_eq!(p, PathBuf::from(r"C:\Windows\System32\cmd.exe"));
}

#[test]
fn register_buckets_by_event_and_is_empty_short_circuits() {
    let engine = HookEngine::new();
    assert!(engine.is_empty(HookEvent::SessionStart));
    assert!(engine.is_empty(HookEvent::PreToolUse));
    engine.register(RegisteredHook {
        plugin_id: "p1".into(),
        event: HookEvent::SessionStart,
        command: "noop".into(),
        args: vec![],
        plugin_root: root(),
        env: BTreeMap::new(),
    });
    engine.register(RegisteredHook {
        plugin_id: "p2".into(),
        event: HookEvent::SessionStart,
        command: "noop".into(),
        args: vec![],
        plugin_root: root(),
        env: BTreeMap::new(),
    });
    engine.register(RegisteredHook {
        plugin_id: "p3".into(),
        event: HookEvent::Stop,
        command: "noop".into(),
        args: vec![],
        plugin_root: root(),
        env: BTreeMap::new(),
    });
    assert_eq!(engine.registered_count(HookEvent::SessionStart), 2);
    assert_eq!(engine.registered_count(HookEvent::Stop), 1);
    assert!(engine.is_empty(HookEvent::PreToolUse));
    assert!(!engine.is_empty(HookEvent::SessionStart));
}
