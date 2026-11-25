//! How many threads each pool gets, from one budget.
//!
//! Three pools share the machine: the async runtime's workers (network,
//! streaming and coordinating the agents), its blocking threads (file
//! tools, commands, store scans) and the compute pool, rayon's global
//! pool, for work that is all CPU (searching files, decoding records,
//! rebuilding Gate's table). Sized on their own, each would take every
//! core. The compute pool leaves one core for the UI thread, and with a
//! UI on screen it runs below the UI in priority, as the agents do
//! ([`crate::priority`]).

use std::num::NonZeroUsize;

/// Threads for each pool on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// Cores the machine has
    pub cores: usize,
    /// Async runtime workers
    pub workers: usize,
    /// Compute pool threads
    pub compute: usize,
}

impl Budget {
    /// The budget for the cores this process may use.
    #[must_use]
    pub fn of_machine() -> Self {
        Self::for_cores(std::thread::available_parallelism().map_or(1, NonZeroUsize::get))
    }

    /// The budget for `cores` cores.
    #[must_use]
    pub fn for_cores(cores: usize) -> Self {
        let cores = cores.max(1);
        Self {
            cores,
            workers: cores,
            compute: cores.saturating_sub(1).max(1),
        }
    }

    /// A multi-threaded runtime builder with this budget's workers and
    /// named threads.
    #[must_use]
    pub fn runtime(&self) -> tokio::runtime::Builder {
        let mut builder = tokio::runtime::Builder::new_multi_thread();
        builder
            .enable_all()
            .worker_threads(self.workers)
            .thread_name("aura-agent");
        builder
    }

    /// Size rayon's global pool to this budget, its threads `nice`
    /// levels below the caller's (0 leaves them as they are). Only the
    /// first call has an effect, and none once something already used
    /// the global pool.
    pub fn start_compute_pool(&self, nice: i32) {
        let built = rayon::ThreadPoolBuilder::new()
            .num_threads(self.compute)
            .thread_name(|index| format!("aura-compute-{index}"))
            .start_handler(move |_| crate::priority::lower_this_thread(nice))
            .build_global();
        if let Err(error) = built {
            tracing::debug!(%error, "compute pool already started");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_compute_pool_leaves_a_core_for_the_ui() {
        assert_eq!(
            Budget::for_cores(8),
            Budget {
                cores: 8,
                workers: 8,
                compute: 7
            }
        );
        assert_eq!(Budget::for_cores(1).compute, 1);
        assert_eq!(Budget::for_cores(0).cores, 1);
    }
}
