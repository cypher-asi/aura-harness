//! Heap profiling of the load harness, with `--features dhat-heap`:
//! dhat replaces the allocator of the whole test binary, so it is off by
//! default and only ever built for this.
//!
//! ```sh
//! AURA_LOAD_AGENTS=200 cargo test --release -p aura-app --features dhat-heap --lib \
//!     -- load::the_app_with_many_agents --ignored --nocapture
//! ```
//!
//! Each N writes `dhat-heap-<N>.json` (open it in dhat's viewer, or rank
//! its allocation sites by bytes live at the peak) and prints the live
//! heap at the end of the warm-up and of the measured span.

#[global_allocator]
static ALLOCATOR: dhat::Alloc = dhat::Alloc;

/// The profiler of one N's run; it writes its file when dropped.
pub(super) fn profile(n: usize) -> dhat::Profiler {
    dhat::Profiler::builder()
        .file_name(format!("dhat-heap-{n}.json"))
        .build()
}

/// Print the live heap now, as of `phase`.
pub(super) fn snapshot(phase: &str) {
    let stats = dhat::HeapStats::get();
    #[allow(clippy::cast_precision_loss)]
    let mib = |bytes: usize| bytes as f64 / (1024.0 * 1024.0);
    println!(
        "heap at {phase}: {:.1} MiB live in {} blocks, peak {:.1} MiB",
        mib(stats.curr_bytes),
        stats.curr_blocks,
        mib(stats.max_bytes)
    );
}
