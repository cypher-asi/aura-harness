//! Whether a command names a file, or imports it as a module.

/// Extensions of the files a script imports by name.
const MODULES: &[&str] = &[
    "py", "js", "mjs", "cjs", "ts", "mts", "cts", "jsx", "tsx", "rb", "pl", "pm", "php", "lua",
];

/// Extensions of the other scripts an interpreter runs.
const SCRIPTS: &[&str] = &["sh", "bash", "zsh", "ksh", "awk"];

/// Whether the file at `path` is a script an interpreter runs.
pub(super) fn is_script(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, extension)| MODULES.contains(&extension) || SCRIPTS.contains(&extension))
}

/// Whether `command` names the file at `path`: its name, which has an
/// extension and so is not an ordinary word, on its own or ending a path
/// (`backup.sh`, `"$OLDPWD/backup.sh"`, `'store.py'`).
pub(super) fn names(command: &str, path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    if !name.trim_start_matches('.').contains('.') {
        return false;
    }
    command.match_indices(name).any(|(at, _)| {
        let before = command[..at].chars().next_back();
        let after = command[at + name.len()..].chars().next();
        !before.is_some_and(is_name_char) && !after.is_some_and(is_name_char)
    })
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.')
}

/// The name a script imports the file at `path` by: its name without the
/// extension, or its folder's for a package's `__init__.py` or a
/// folder's `index.js`. `None` for a file not imported by name.
pub(super) fn module(path: &str) -> Option<&str> {
    let mut parts = path.rsplit('/');
    let (stem, extension) = parts.next()?.rsplit_once('.')?;
    if !MODULES.contains(&extension) {
        return None;
    }
    let stem = if matches!(stem, "__init__" | "index") {
        parts.next()?
    } else {
        stem
    };
    (!stem.is_empty()).then_some(stem)
}

/// Whether code in `command` imports `module`, or `command` runs it:
/// `import shifts`, `import pricing,api`, `from shifts import
/// shift_hours`, `from .shifts import …`, `python -m pkg.shifts`,
/// `require('./money')`, `import x from './money.js'` or
/// `require_relative 'money'`.
pub(super) fn imports(command: &str, module: &str) -> bool {
    // A path names the module by its last part (`./money.js`), a dotted
    // name by any part (`pkg.money`).
    let is_module = |word: &str| match word.rsplit_once('/') {
        Some((_, name)) => {
            name.rsplit_once('.')
                .filter(|(_, extension)| MODULES.contains(extension))
                .map_or(name, |(stem, _)| stem)
                == module
        }
        None => word.split('.').any(|part| part == module),
    };
    command.split(['\n', ';']).any(|statement| {
        let words: Vec<&str> = statement
            .split(|c: char| c.is_whitespace() || matches!(c, ',' | '(' | ')' | '{' | '}' | '='))
            .map(|word| word.trim_matches(['\'', '"', '`']))
            .filter(|word| !word.is_empty())
            .collect();
        words.iter().enumerate().any(|(at, word)| {
            let rest = &words[at + 1..];
            match *word {
                "from" | "-m" | "require" | "require_relative" => {
                    rest.first().is_some_and(|next| is_module(next))
                }
                "import" => rest
                    .iter()
                    .take_while(|next| **next != "from")
                    .enumerate()
                    .any(|(n, next)| (n == 0 || rest[n - 1] != "as") && is_module(next)),
                _ => false,
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_named_by_its_whole_name() {
        assert!(names("sh \"$OLDPWD/backup.sh\" src", "backup.sh"));
        assert!(names(
            "py_compile.compile('store.py',doraise=True)",
            "store.py"
        ));
        assert!(!names("cp backup.sh.bak x", "backup.sh"));
        assert!(!names("sh mybackup.sh", "backup.sh"));
        assert!(
            !names("make && ./run check", "run"),
            "a name with no extension"
        );
    }
}
