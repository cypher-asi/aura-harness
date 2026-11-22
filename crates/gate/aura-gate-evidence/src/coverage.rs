//! Whether a check that passed could have checked what an attempt added.
//!
//! A test run passes whatever new code it never calls: an attempt that
//! adds `prepare_multipart` and runs the module's existing tests, none of
//! which name it, has a passing check that says nothing about the new
//! function. [`added_names`] reads what a change adds from its diff; the
//! harness looks for those names in the workspace's tests, and work whose
//! additions no test names counts as unchecked (see `aura-gate-followup`).

use crate::evidence::is_test_path;

/// Lines of an added test, after leading whitespace.
const TEST_STARTS: &[&str] = &[
    "#[test]",
    "#[cfg(test)]",
    "#[tokio::test",
    "def test_",
    "async def test_",
    "func Test",
    "describe(",
    "it(",
    "test(",
];

/// The definitions `diff`, a unified diff, adds to code that is not a
/// test: public functions, classes and types the same diff does not also
/// remove (one whose signature changed is not new). Empty when the diff
/// adds or changes a test, whose tests then name what they cover, or
/// adds no definition.
#[must_use]
pub fn added_names(diff: &str) -> Vec<String> {
    let (mut added, mut removed) = (Vec::new(), Vec::new());
    let mut path = String::new();
    let mut in_hunk = false;
    for line in diff.lines() {
        if let Some(names) = line.strip_prefix("diff --git ") {
            path = names
                .rsplit_once(" b/")
                .map_or(names, |(_, new)| new)
                .to_string();
            if is_test_path(&path) {
                return Vec::new();
            }
            in_hunk = false;
            continue;
        }
        if line.starts_with("@@") {
            in_hunk = true;
            continue;
        }
        if !in_hunk {
            continue;
        }
        let (adds, code) = match (line.strip_prefix('+'), line.strip_prefix('-')) {
            (Some(code), _) => (true, code),
            (_, Some(code)) => (false, code),
            _ => continue,
        };
        let code = code.trim_start();
        if adds && TEST_STARTS.iter().any(|start| code.starts_with(start)) {
            return Vec::new();
        }
        let into = if adds { &mut added } else { &mut removed };
        if let Some(name) = defined(&path, code) {
            if !into.contains(&name) {
                into.push(name);
            }
        }
    }
    added.retain(|name| !removed.contains(name));
    added
}

/// The public name `code`, a line of the file at `path` without its
/// indentation, defines, if it starts a definition.
fn defined(path: &str, code: &str) -> Option<String> {
    let ext = path.rsplit_once('.').map_or("", |(_, ext)| ext);
    let name = match ext {
        "py" => {
            let code = code.strip_prefix("async ").unwrap_or(code);
            ["def ", "class "]
                .iter()
                .find_map(|kw| code.strip_prefix(kw))
                .map(ident)
                .filter(|name| !name.starts_with('_'))
        }
        "rs" => {
            let mut code = code.strip_prefix("pub")?;
            if let Some(scoped) = code.strip_prefix('(') {
                code = scoped.split_once(')')?.1;
            }
            let mut code = code.trim_start();
            while let Some(rest) = ["async ", "const ", "unsafe ", "extern \"C\" "]
                .iter()
                .find_map(|kw| code.strip_prefix(kw))
            {
                code = rest.trim_start();
            }
            ["fn ", "struct ", "enum ", "trait "]
                .iter()
                .find_map(|kw| code.strip_prefix(kw))
                .map(ident)
        }
        "go" => {
            let mut code = code.strip_prefix("func ")?;
            if let Some(receiver) = code.strip_prefix('(') {
                code = receiver.split_once(')')?.1.trim_start();
            }
            Some(ident(code)).filter(|name| name.starts_with(|c: char| c.is_ascii_uppercase()))
        }
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => {
            let code = code.strip_prefix("export ")?;
            let code = code.strip_prefix("default ").unwrap_or(code);
            let code = code.strip_prefix("async ").unwrap_or(code);
            ["function* ", "function ", "class ", "const ", "let "]
                .iter()
                .find_map(|kw| code.strip_prefix(kw))
                .map(ident)
        }
        _ => None,
    }?;
    (name.len() > 1).then_some(name)
}

/// The identifier `code` starts with.
fn ident(code: &str) -> String {
    code.trim_start()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

/// Whether `text` has `name` as a whole word.
#[must_use]
pub fn mentions(text: &str, name: &str) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(name).any(|(at, _)| {
        !text[..at].chars().next_back().is_some_and(word)
            && !text[at + name.len()..].chars().next().is_some_and(word)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(path: &str, hunk: &str) -> String {
        format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1,3 +1,9 @@\n{hunk}")
    }

    #[test]
    fn a_new_public_function_is_added_and_a_private_helper_is_not() {
        let change = diff(
            "lib/ansible/module_utils/urls.py",
            " import os\n+import mimetypes\n+def _wrap_base64(data):\n+    pass\n+def prepare_multipart(fields):\n+    class Part:\n",
        );
        assert_eq!(added_names(&change), ["prepare_multipart", "Part"]);
    }

    #[test]
    fn a_changed_signature_is_not_new() {
        let change = diff(
            "src/lib.rs",
            "-pub fn parse(text: &str) -> Value {\n+pub fn parse(text: &str, strict: bool) -> Value {\n+pub(crate) async fn load() {}\n+fn private() {}\n",
        );
        assert_eq!(added_names(&change), ["load"]);
    }

    #[test]
    fn a_change_that_adds_or_changes_a_test_adds_nothing_to_look_for() {
        let with_file = format!(
            "{}{}",
            diff("src/parse.py", "+def parse(text):\n"),
            diff("tests/test_parse.py", "+def test_parse():\n")
        );
        assert!(added_names(&with_file).is_empty());
        let inline = diff(
            "src/lib.rs",
            "+pub fn parse() {}\n+#[cfg(test)]\n+mod tests {\n",
        );
        assert!(added_names(&inline).is_empty());
    }

    #[test]
    fn exported_go_and_js_definitions_are_added() {
        let go = diff(
            "pkg/semver.go",
            "+func (c *Constraint) Check(v string) bool {\n+func helper() {}\n",
        );
        assert_eq!(added_names(&go), ["Check"]);
        let js = diff(
            "src/index.ts",
            "+export async function fetchAll() {}\n+export const LIMIT = 3;\n+function local() {}\n",
        );
        assert_eq!(added_names(&js), ["fetchAll", "LIMIT"]);
    }

    #[test]
    fn a_name_counts_only_as_a_whole_word() {
        assert!(mentions(
            "from urls import prepare_multipart\n",
            "prepare_multipart"
        ));
        assert!(mentions(
            "p = prepare_multipart(fields)",
            "prepare_multipart"
        ));
        assert!(!mentions("prepare_multipart_body()", "prepare_multipart"));
        assert!(!mentions("_prepare_multipart()", "prepare_multipart"));
    }
}
