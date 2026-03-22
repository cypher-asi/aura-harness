//! Numeric constants matching aura-app's operational parameters.

/// Maximum tool-use iterations before the loop terminates.
pub const MAX_ITERATIONS: usize = 25;

/// Default exploration allowance (read-only tool calls before warnings).
pub const DEFAULT_EXPLORATION_ALLOWANCE: usize = 12;

/// Auto-build cooldown: minimum iterations between automatic build checks.
pub const AUTO_BUILD_COOLDOWN: usize = 2;

/// Thinking budget taper: after this many iterations, reduce thinking budget.
pub const THINKING_TAPER_AFTER: usize = 2;

/// Factor by which to reduce the thinking budget each iteration after taper threshold.
pub const THINKING_TAPER_FACTOR: f64 = 0.6;

/// Minimum thinking budget after tapering.
pub const THINKING_MIN_BUDGET: u32 = 1024;

/// Maximum full reads of the same file before blocking.
pub const MAX_READS_PER_FILE: usize = 3;

/// Maximum range reads of the same file before blocking.
pub const MAX_RANGE_READS_PER_FILE: usize = 5;

/// Consecutive command failures before blocking all commands.
pub const CMD_FAILURE_BLOCK_THRESHOLD: usize = 5;

/// Consecutive write failures on a single file before blocking writes to it.
pub const WRITE_FAILURE_BLOCK_THRESHOLD: usize = 3;

/// Stall detection: identical write targets for this many iterations triggers fail-fast.
pub const STALL_STREAK_THRESHOLD: usize = 3;

/// Budget warning at 30% utilization.
pub const BUDGET_WARNING_30: f64 = 0.30;

/// Budget warning at 40% (no writes yet) utilization.
pub const BUDGET_WARNING_40_NO_WRITE: f64 = 0.40;

/// Budget warning at 60% utilization (wrap up).
pub const BUDGET_WARNING_60: f64 = 0.60;

/// Exploration warning (mild) at allowance minus this value.
pub const EXPLORATION_WARNING_MILD_OFFSET: usize = 4;

/// Exploration warning (strong) at allowance minus this value.
pub const EXPLORATION_WARNING_STRONG_OFFSET: usize = 2;

/// Characters per token estimate for context budget calculations.
pub const CHARS_PER_TOKEN: usize = 4;

/// Compaction tier thresholds (percentage of context used).
pub const COMPACTION_TIER_HISTORY: f64 = 0.85;

/// Aggressive compaction tier threshold.
pub const COMPACTION_TIER_AGGRESSIVE: f64 = 0.70;

/// 60% compaction tier threshold.
pub const COMPACTION_TIER_60: f64 = 0.60;

/// 30% compaction tier threshold.
pub const COMPACTION_TIER_30: f64 = 0.30;

/// Micro compaction tier threshold.
pub const COMPACTION_TIER_MICRO: f64 = 0.15;

/// Write file cooldown in iterations after a write failure.
pub const WRITE_COOLDOWN_ITERATIONS: usize = 2;

/// Tools classified as exploration (read-only, non-modifying).
pub const EXPLORATION_TOOLS: &[&str] = &[
    "fs_read",
    "read_file",
    "fs_ls",
    "list_files",
    "fs_find",
    "find_files",
    "fs_stat",
    "search_code",
];

/// Tools that perform writes (mutations).
pub const WRITE_TOOLS: &[&str] = &[
    "fs_write",
    "write_file",
    "fs_edit",
    "edit_file",
    "fs_delete",
    "delete_file",
];

/// Tools that run commands.
pub const COMMAND_TOOLS: &[&str] = &["cmd_run", "run_command"];
