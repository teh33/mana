use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

/// Retry context for a unit being dispatched.
#[derive(Debug, Clone)]
pub struct RetryContext {
    /// 0 for first attempt, >0 for retries.
    pub attempt_number: u32,
    /// Notes from the most recent failed/abandoned attempt, if any.
    pub previous_failure: Option<String>,
    /// Historical attempt notes in chronological order.
    pub previous_notes: Vec<String>,
}

/// Configuration for the dispatch pool.
///
/// Extracted from mana's Config — only the fields relevant to dispatch.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Maximum concurrent agents. Hard ceiling regardless of resources.
    pub max_concurrent: usize,
    /// Minimum available system memory (MB) to keep free. 0 = disabled.
    pub memory_reserve_mb: u64,
    /// Total timeout per agent in minutes.
    pub timeout_minutes: u32,
    /// Idle timeout per agent in minutes (no output = killed).
    pub idle_timeout_minutes: u32,
    /// Continue dispatching after a unit fails.
    pub keep_going: bool,
    /// Agents defer verify; runner batches verify commands after all complete.
    pub batch_verify: bool,
    /// Lock files listed in unit paths to prevent concurrent modification.
    pub file_locking: bool,
    /// Model override for agent spawning (substituted into templates).
    pub run_model: Option<String>,
    /// Path to the .mana/ directory.
    pub mana_dir: PathBuf,
}

/// A unit ready for dispatch, with enough metadata for scheduling decisions.
///
/// Mirrors the existing SizedUnit but owned by the pool crate.
#[derive(Debug, Clone)]
pub struct DispatchUnit {
    pub id: String,
    pub title: String,
    pub priority: u8,
    pub dependencies: Vec<String>,
    pub parent: Option<String>,
    pub produces: Vec<String>,
    pub requires: Vec<String>,
    /// File paths this unit touches — used for conflict detection.
    pub paths: Vec<String>,
    /// Retry context derived from the unit's attempt history.
    pub retry: RetryContext,
}

/// Result of a single agent's execution.
#[derive(Debug, Clone)]
pub struct AgentResult {
    pub unit_id: String,
    pub title: String,
    pub success: bool,
    pub duration: Duration,
    pub tokens: Option<u64>,
    pub cost: Option<f64>,
    pub error: Option<String>,
    pub tool_count: usize,
    pub turns: usize,
    pub failure_summary: Option<String>,
}

/// Events emitted by the pool during dispatch.
///
/// Consumers (CLI, daemon, UI) subscribe to these and decide how to render them.
/// The pool never prints to stderr or stdout directly.
#[derive(Debug, Clone)]
pub enum PoolEvent {
    /// An agent is about to be spawned for a unit.
    Spawning {
        unit_id: String,
        title: String,
        wave: usize,
    },

    /// Agent emitted progress while running.
    Progress {
        unit_id: String,
        phase: String,
        elapsed: Duration,
    },

    /// Agent emitted a heartbeat while running.
    Heartbeat { unit_id: String, elapsed: Duration },

    /// Agent appears stuck: no progress or heartbeat seen within idle timeout.
    AgentStuck {
        unit_id: String,
        last_progress_secs_ago: u64,
    },

    /// An agent completed (success or failure).
    Completed { result: AgentResult },

    /// Dispatch is paused due to memory pressure.
    MemoryPressure {
        reserve_mb: u64,
        available_mb: Option<u64>,
    },

    /// Cannot start — memory too low and no agents running to free it.
    MemoryExhausted {
        reserve_mb: u64,
        available_mb: Option<u64>,
    },

    /// Units remain but have unresolvable dependencies.
    UnresolvableDeps { unit_ids: Vec<String> },

    /// Dispatch finished. Summary of the full run.
    Finished {
        total: usize,
        passed: usize,
        failed: usize,
        duration: Duration,
    },
}

/// Outcome of a full dispatch cycle.
#[derive(Debug)]
pub struct DispatchOutcome {
    pub results: Vec<AgentResult>,
    pub any_failed: bool,
}

#[derive(Debug, Clone)]
pub enum AgentProgress {
    Progress { phase: String, elapsed: Duration },
    Heartbeat { elapsed: Duration },
}

/// Trait for spawning agents. Implementations control how agent processes
/// are actually started (CLI invokes pi/imp, daemon might use a different
/// strategy, tests can mock).
///
/// `spawn` is called from a worker thread — implementations must be Send + Sync.
/// The function should block until the agent completes and return the result.
pub trait Spawner: Send + Sync {
    fn spawn(
        &self,
        unit: &DispatchUnit,
        config: &SpawnConfig,
        progress_tx: Option<mpsc::Sender<(String, AgentProgress)>>,
    ) -> AgentResult;
}

/// Per-spawn configuration passed to the Spawner.
#[derive(Debug, Clone)]
pub struct SpawnConfig {
    pub mana_dir: PathBuf,
    pub timeout_minutes: u32,
    pub idle_timeout_minutes: u32,
    pub run_model: Option<String>,
    pub file_locking: bool,
    pub batch_verify: bool,
    /// Retry context derived from the unit's attempt history.
    pub retry: RetryContext,
}
