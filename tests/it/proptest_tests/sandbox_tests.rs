//! Sandbox path tests.

use super::*;
use aura_exec_tools::Sandbox;
use tempfile::TempDir;

// Strategy for generating "safe" relative paths
fn safe_path_component() -> impl Strategy<Value = String> {
    prop::string::string_regex("[a-zA-Z0-9_-]{1,20}")
        .unwrap()
        .prop_filter("not empty", |s| !s.is_empty())
}

proptest! {
    /// Safe relative paths resolve within sandbox.
    #[test]
    fn safe_path_stays_in_sandbox(components in prop::collection::vec(safe_path_component(), 1..4)) {
        let dir = TempDir::new().unwrap();
        let sandbox = Sandbox::new(dir.path()).unwrap();

        let path = components.join("/");
        let resolved = sandbox.resolve(&path);

        // Should succeed (path is within sandbox)
        prop_assert!(resolved.is_ok());
        prop_assert!(resolved.unwrap().starts_with(sandbox.root()));
    }

    /// Paths with .. that escape are blocked.
    #[test]
    fn dotdot_escape_blocked(depth in 1usize..10) {
        let dir = TempDir::new().unwrap();
        let sandbox = Sandbox::new(dir.path()).unwrap();

        // Create a path that tries to escape
        let path = "../".repeat(depth) + "etc/passwd";
        let resolved = sandbox.resolve(&path);

        // Should fail (escapes sandbox)
        prop_assert!(resolved.is_err());
    }
}
