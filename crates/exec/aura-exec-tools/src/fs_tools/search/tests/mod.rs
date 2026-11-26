use super::file_search::is_text_file;

use super::*;

use std::fs;

use tempfile::TempDir;

fn create_test_sandbox() -> (Sandbox, TempDir) {
    let dir = TempDir::new().unwrap();
    let sandbox = Sandbox::new(dir.path()).unwrap();
    (sandbox, dir)
}

mod diagnostics;
mod options;
mod output_modes;
mod patterns;
