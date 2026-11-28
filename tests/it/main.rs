//! aura's integration tests. They share one binary, since each test binary
//! links the crate and all of its dependencies again; add a test file here as a
//! module, not as a file of its own in `tests/`.

mod async_process_tests;
mod aura_home_resolution;
mod automaton_reasoning_recording;
mod changelog_fragments;
mod cli_golden;
mod common;
mod config_boundary;
mod config_doc;
mod e2e_full;
mod e2e_live;
mod eval_suite;
mod gate_cli;
mod headless_print;
mod hello_world_test;
mod kernel_integration;
mod layer_boundary;
mod pipeline_tests;
mod prompts_boundary;
mod proptest_tests;
mod protocol_roundtrip;
mod run_traces_cli;
