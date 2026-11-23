//! A run's diff split per file.

use super::super::patch::split_patch;

#[test]
fn patches_split_into_files_with_their_hunks() {
    let patch = "diff --git a/src/lib.rs b/src/lib.rs\nindex 1..2 100644\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-a\n+b\ndiff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+hi\ndiff --git a/logo.png b/logo.png\nGIT binary patch\nliteral 3\nabc\n";
    let files = split_patch(patch);
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["src/lib.rs", "new.txt", "logo.png"]);
    assert_eq!(files[0].diff, "@@ -1 +1 @@\n-a\n+b\n");
    assert_eq!(files[1].diff, "@@ -0,0 +1 @@\n+hi\n");
    assert_eq!(files[2].diff, "(binary file)");
    assert!(split_patch("").is_empty());
}
