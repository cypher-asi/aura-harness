//! Integration tests for Spec 01: Hello World
//!
//! Verifies that `aura hello` produces exact stdout output "Hello, World!\n"
//! and exits with code 0.

use std::process::Command;

#[test]
fn hello_subcommand_prints_exact_message() {
    let binary = env!("CARGO_BIN_EXE_aura");

    let output = Command::new(binary)
        .arg("hello")
        .output()
        .expect("failed to execute binary");

    assert!(
        output.status.success(),
        "binary exited with non-zero status: {:?}",
        output.status
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim(),
        "Hello, World!",
        "stdout did not contain the exact expected message"
    );
}

#[test]
fn hello_subcommand_exits_with_code_zero() {
    let binary = env!("CARGO_BIN_EXE_aura");

    let output = Command::new(binary)
        .arg("hello")
        .output()
        .expect("failed to execute binary");

    let code = output.status.code().expect("process terminated by signal");
    assert_eq!(code, 0, "expected exit code 0, got {code}");
}

#[test]
fn hello_subcommand_stderr_is_empty() {
    let binary = env!("CARGO_BIN_EXE_aura");

    let output = Command::new(binary)
        .arg("hello")
        .output()
        .expect("failed to execute binary");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.is_empty(),
        "expected no stderr output, got: {stderr}"
    );
}
