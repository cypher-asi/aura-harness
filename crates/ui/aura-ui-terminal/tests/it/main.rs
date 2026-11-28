//! aura-ui-terminal's integration tests. They share one binary, since each test
//! binary links the crate and all of its dependencies again; add a test file
//! here as a module, not as a file of its own in `tests/`. `ui_memory.rs` is
//! the exception: it makes dhat the global allocator to weigh the heap, and
//! every test sharing its binary would add to the weight.

mod chat_load_perf;
mod ui_load;
