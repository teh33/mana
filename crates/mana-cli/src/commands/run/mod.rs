//! `mana run` — Dispatch ready units to agents.
//!
//! Finds ready units, groups them into waves by dependency order,
//! and spawns agents for each wave.
//!
//! Modes:
//! - `mana run` — one-shot: dispatch all ready units, then exit
//! - `mana run 5.1` — dispatch a single unit (or its ready children if parent)
//! - `mana run --dry-run` — show plan without spawning
//! - `mana run --loop` — keep running until no ready units remain
//! - `mana run --json-stream` — emit JSON stream events to stdout
//!
//! Spawning modes:
//! - **Template mode** (backward compat): If `config.run` is set, spawn via `sh -c <template>`.
//! - **Direct mode**: If no template is configured but `imp` is on PATH, spawn `imp run <id>`
//!   and monitor its JSON event stream with timeouts.

pub(super) mod memory;
mod plan;
mod ready_queue;
mod wave;

pub use plan::{DispatchPlan, SizedUnit};
pub use wave::Wave;

use std::fmt;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::Serialize;

use crate::commands::review::{cmd_review, ReviewArgs};
use crate::config::Config;
use crate::index::ArchiveIndex;
use crate::stream::{self, StreamEvent};
use crate::unit::{AttemptOutcome, Status, Unit};

use plan::{plan_dispatch, print_plan, print_plan_json};
use ready_queue::run_ready_queue_direct;
use wave::run_wave;

/// Shared config passed to wave/ready-queue runners.
pub(super) struct RunConfig {
    pub max_jobs: usize,
    pub timeout_minutes: u32,
    pub idle_timeout_minutes: u32,
    pub json_stream: bool,
    pub file_locking: bool,
    /// Config-level model for run/implement (substituted into `{model}` in templates).
    pub run_model: Option<String>,
    /// When true, agents defer verify by exiting with AwaitingVerify status.
    /// The runner then records that the unit is candidate-complete and runs each
    /// unique verify command once to advance those units toward full completion.
    pub batch_verify: bool,
    /// Minimum available system memory (MB) to reserve. 0 = disabled.
    pub memory_reserve_mb: u64,
}

/// Arguments for cmd_run, matching the CLI definition.
pub struct RunArgs {
    pub id: Option<String>,
    pub jobs: u32,
    pub dry_run: bool,
    pub loop_mode: bool,
    pub keep_going: bool,
    pub timeout: u32,
    pub idle_timeout: u32,
    pub json_stream: bool,
    /// If true, run adversarial review after each successful unit close.
    pub review: bool,
}

/// Canonical run target semantics for CLI and native consumers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum RunTarget {
    AllReady,
    Unit(String),
    Explicit(Vec<String>),
}

impl RunTarget {
    pub fn from_cli_id(id: Option<String>) -> Self {
        match id {
            Some(id) => Self::Unit(id),
            None => Self::AllReady,
        }
    }

    pub fn scope_label(&self) -> String {
        match self {
            Self::AllReady => "all".to_string(),
            Self::Unit(id) => id.clone(),
            Self::Explicit(ids) => {
                if ids.is_empty() {
                    "all".to_string()
                } else {
                    ids.join(",")
                }
            }
        }
    }
}

/// Embedding-oriented run params.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeRunParams {
    pub target: RunTarget,
    pub jobs: u32,
    pub dry_run: bool,
    pub loop_mode: bool,
    pub keep_going: bool,
    pub timeout: u32,
    pub idle_timeout: u32,
    pub json_stream: bool,
    pub review: bool,
}

impl From<RunArgs> for NativeRunParams {
    fn from(args: RunArgs) -> Self {
        Self {
            target: RunTarget::from_cli_id(args.id),
            jobs: args.jobs,
            dry_run: args.dry_run,
            loop_mode: args.loop_mode,
            keep_going: args.keep_going,
            timeout: args.timeout,
            idle_timeout: args.idle_timeout,
            json_stream: args.json_stream,
            review: args.review,
        }
    }
}

/// What action to take for a unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitAction {
    Implement,
}

impl fmt::Display for UnitAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnitAction::Implement => write!(f, "implement"),
        }
    }
}

/// Result of a completed agent.
///
/// `AwaitingVerify` means the worker produced a candidate-complete result and
/// handed it back for later verification. It is a pre-verification success
/// stage on the path to a fully `Closed` unit, not a separate completion model.
#[derive(Debug)]
#[allow(dead_code)]
struct AgentResult {
    id: String,
    title: String,
    action: UnitAction,
    success: bool,
    duration: Duration,
    total_tokens: Option<u64>,
    total_cost: Option<f64>,
    error: Option<String>,
    tool_count: usize,
    turns: usize,
    failure_summary: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UnitOutcome {
    Closed,
    Failed,
    Abandoned,
    AwaitingVerify,
}

impl UnitOutcome {
    fn is_closed(self) -> bool {
        matches!(self, Self::Closed)
    }

    fn is_failure(self) -> bool {
        matches!(self, Self::Failed | Self::Abandoned)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct OutcomeCounts {
    closed: u32,
    failed: u32,
    abandoned: u32,
    awaiting_verify: u32,
    skipped: u32,
}

impl OutcomeCounts {
    fn record(&mut self, outcome: UnitOutcome) {
        match outcome {
            UnitOutcome::Closed => self.closed += 1,
            UnitOutcome::Failed => self.failed += 1,
            UnitOutcome::Abandoned => self.abandoned += 1,
            UnitOutcome::AwaitingVerify => self.awaiting_verify += 1,
        }
    }

    fn total_failed_for_legacy_stream(self) -> usize {
        (self.failed + self.abandoned) as usize
    }

    fn has_failures(self) -> bool {
        self.failed > 0 || self.abandoned > 0
    }
}

pub(super) fn inspect_unit_outcome(mana_dir: &Path, unit_id: &str) -> UnitOutcome {
    if let Ok(unit_path) = crate::discovery::find_unit_file(mana_dir, unit_id) {
        if let Ok(unit) = Unit::from_file(&unit_path) {
            return match unit.status {
                Status::Closed => UnitOutcome::Closed,
                Status::AwaitingVerify => UnitOutcome::AwaitingVerify,
                Status::Open | Status::InProgress => {
                    if unit
                        .attempt_log
                        .last()
                        .map(|attempt| attempt.outcome == AttemptOutcome::Abandoned)
                        .unwrap_or(false)
                    {
                        UnitOutcome::Abandoned
                    } else {
                        UnitOutcome::Failed
                    }
                }
            };
        }
    }

    let archived = ArchiveIndex::load_or_rebuild(mana_dir)
        .map(|archive| archive.units.iter().any(|entry| entry.id == unit_id))
        .unwrap_or(false);

    if archived {
        UnitOutcome::Closed
    } else {
        UnitOutcome::Failed
    }
}

fn collect_outcome_counts(
    mana_dir: &Path,
    results: &[AgentResult],
    skipped_count: usize,
) -> (OutcomeCounts, Vec<String>) {
    let mut counts = OutcomeCounts {
        skipped: skipped_count as u32,
        ..OutcomeCounts::default()
    };
    let mut closed_ids = Vec::new();

    for result in results {
        let outcome = inspect_unit_outcome(mana_dir, &result.id);
        counts.record(outcome);
        if outcome.is_closed() {
            closed_ids.push(result.id.clone());
        }
    }

    (counts, closed_ids)
}

// ---------------------------------------------------------------------------
// Signal handling for clean agent shutdown
// ---------------------------------------------------------------------------

/// Global flag set by SIGINT/SIGTERM signal handlers to request clean shutdown.
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// PIDs of running child agent processes, for cleanup on shutdown.
static CHILD_PIDS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Returns true if a shutdown signal (SIGINT/SIGTERM) has been received.
fn shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
}

/// Install signal handlers for SIGINT and SIGTERM.
///
/// Instead of immediately terminating, the handlers set a flag that's checked
/// in the execution loops. This allows clean shutdown: kill child agents,
/// release claims, and print a summary.
fn install_signal_handlers() {
    unsafe {
        libc::signal(
            libc::SIGINT,
            signal_handler as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGTERM,
            signal_handler as *const () as libc::sighandler_t,
        );
    }
}

extern "C" fn signal_handler(_sig: libc::c_int) {
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
}

/// Register a child process PID for shutdown tracking.
fn register_child_pid(pid: u32) {
    if let Ok(mut pids) = CHILD_PIDS.lock() {
        pids.push(pid);
    }
}

/// Unregister a child process PID after it exits.
fn unregister_child_pid(pid: u32) {
    if let Ok(mut pids) = CHILD_PIDS.lock() {
        pids.retain(|&p| p != pid);
    }
}

/// Send SIGTERM to all tracked child processes for graceful shutdown.
fn kill_all_children() {
    if let Ok(pids) = CHILD_PIDS.lock() {
        for &pid in pids.iter() {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
    }
}

/// Send SIGKILL to all tracked child processes (forced shutdown).
fn force_kill_all_children() {
    if let Ok(pids) = CHILD_PIDS.lock() {
        for &pid in pids.iter() {
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
    }
}

/// Which spawning mode to use.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SpawnMode {
    /// Use shell template from config (backward compat).
    Template {
        run_template: String,
        plan_template: Option<String>,
    },
    /// Spawn the direct-mode agent with JSON output and monitoring.
    Direct,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DecisionWarning {
    id: String,
    title: String,
    decisions: Vec<String>,
}

fn collect_decision_warnings(
    mana_dir: &Path,
    units: &[SizedUnit],
    index: &crate::index::Index,
) -> Result<Vec<DecisionWarning>> {
    let mut warnings = Vec::new();

    for unit in units {
        let Some(entry) = index.units.iter().find(|entry| entry.id == unit.id) else {
            continue;
        };

        if !entry.has_decisions {
            continue;
        }

        let unit_path = crate::discovery::find_unit_file(mana_dir, &unit.id)?;
        let unit = Unit::from_file(&unit_path)?;
        if unit.decisions.is_empty() {
            continue;
        }

        warnings.push(DecisionWarning {
            id: unit.id,
            title: unit.title,
            decisions: unit.decisions,
        });
    }

    warnings.sort_by(|a, b| crate::util::natural_cmp(&a.id, &b.id));
    Ok(warnings)
}

fn format_decision_warning_message(warnings: &[DecisionWarning]) -> String {
    let mut message = String::new();

    if warnings.len() == 1 {
        let warning = &warnings[0];
        message.push_str(&format!(
            "⚠ Unit {} has {} unresolved decision{} — agent may make wrong choices:\n",
            warning.id,
            warning.decisions.len(),
            if warning.decisions.len() == 1 {
                ""
            } else {
                "s"
            }
        ));
        for (idx, decision) in warning.decisions.iter().enumerate() {
            message.push_str(&format!("  {}: {}\n", idx, decision));
        }
        return message;
    }

    message.push_str(&format!(
        "⚠ {} units have unresolved decisions — agents may make wrong choices:\n",
        warnings.len()
    ));
    for warning in warnings {
        message.push_str(&format!(
            "Unit {}: {} ({} unresolved)\n",
            warning.id,
            warning.title,
            warning.decisions.len()
        ));
        for (idx, decision) in warning.decisions.iter().enumerate() {
            message.push_str(&format!("  {}: {}\n", idx, decision));
        }
    }

    message
}

fn confirm_dispatch_with_decisions(
    warnings: &[DecisionWarning],
    json_stream: bool,
) -> Result<bool> {
    if warnings.is_empty() {
        return Ok(true);
    }

    eprint!("{}", format_decision_warning_message(warnings));

    if json_stream || !std::io::stdin().is_terminal() {
        return Ok(true);
    }

    eprint!("Dispatch anyway? [y/N] ");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    Ok(input.trim().eq_ignore_ascii_case("y"))
}

/// Execute the `mana run` command.
pub fn cmd_run(mana_dir: &Path, args: RunArgs) -> Result<()> {
    // Install signal handlers for clean shutdown on Ctrl+C / SIGTERM
    install_signal_handlers();

    // Determine spawn mode
    let config = Config::load_with_extends(mana_dir)?;
    let spawn_mode = determine_spawn_mode(&config);

    if spawn_mode == SpawnMode::Direct && !imp_available() {
        anyhow::bail!(
            "No direct agent configured and `imp` was not found on PATH.\n\n\
             Either:\n  \
               1. Install imp (Rust): cargo install imp-cli\n  \
               2. Set a run template: mana config set run \"<command>\"\n\n\
             The command template uses {{id}} as a placeholder for the unit ID.\n\n\
             Examples:\n  \
               mana config set run \"imp run {{id}} && mana close {{id}}\"\n  \
               mana config set run \"claude -p 'implement unit {{id}} and run mana close {{id}}'\""
        );
    }

    if let SpawnMode::Template {
        ref run_template, ..
    } = spawn_mode
    {
        let _ = run_template;

        if let Some(ref run_model) = config.run_model {
            if !run_template.contains("{model}") {
                eprintln!(
                    "Warning: run_model is set to `{}`, but the run template does not include `{{model}}`, so the model override will be ignored.\nUpdate the template (for example: `imp --model {{model}} run {{id}} && mana close {{id}}`) or remove `run` to use direct mode.",
                    run_model
                );
            }
        }
    }

    let params = NativeRunParams::from(args);
    if params.loop_mode {
        run_loop(mana_dir, &config, &params, &spawn_mode)
    } else {
        run_once(mana_dir, &config, &params, &spawn_mode)
    }
}

/// Execute mana orchestration through the embedding-oriented API and return structured run data.
pub fn run_native(mana_dir: &Path, params: NativeRunParams) -> Result<RunView> {
    let events: Arc<Mutex<Vec<StreamEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_events = Arc::clone(&events);
    let _guard = stream::install_sink(Arc::new(move |event| {
        if let Ok(mut buf) = sink_events.lock() {
            buf.push(event.clone());
        }
    }));

    // Install signal handlers for clean shutdown on Ctrl+C / SIGTERM
    install_signal_handlers();

    // Determine spawn mode
    let config = Config::load_with_extends(mana_dir)?;
    let spawn_mode = determine_spawn_mode(&config);

    if spawn_mode == SpawnMode::Direct && !imp_available() {
        anyhow::bail!(
            "No direct agent configured and `imp` was not found on PATH.\n\n\
             Either:\n  \
               1. Install imp (Rust): cargo install imp-cli\n  \
               2. Set a run template: mana config set run \"<command>\""
        );
    }

    if params.loop_mode {
        run_loop(mana_dir, &config, &params, &spawn_mode)?;
    } else {
        run_once(mana_dir, &config, &params, &spawn_mode)?;
    }

    let events = events.lock().map(|buf| buf.clone()).unwrap_or_default();
    Ok(build_run_view_from_events(
        events,
        Some(detect_effective_runtime(&config, &spawn_mode)),
    ))
}

/// Determine the spawn mode based on config.
fn determine_spawn_mode(config: &Config) -> SpawnMode {
    if let Some(ref run) = config.run {
        SpawnMode::Template {
            run_template: run.clone(),
            plan_template: config.plan.clone(),
        }
    } else {
        SpawnMode::Direct
    }
}

/// Check if `imp` is available on PATH.
fn imp_available() -> bool {
    Command::new("imp")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}


/// Single dispatch pass: plan → print/execute → report.
fn run_once(
    mana_dir: &Path,
    config: &Config,
    params: &NativeRunParams,
    spawn_mode: &SpawnMode,
) -> Result<()> {
    // Check for shutdown before starting execution
    if shutdown_requested() {
        if !params.json_stream {
            eprintln!("\nShutdown signal received, aborting.");
        }
        return Ok(());
    }

    let plan = plan_dispatch(mana_dir, config, &params.target, params.dry_run)?;

    if plan.waves.is_empty() && plan.skipped.is_empty() {
        if params.json_stream {
            stream::emit_error("No ready units");
        } else {
            eprintln!("No ready units. Use `mana status` to see what's going on.");
        }
        return Ok(());
    }

    if params.dry_run {
        if params.json_stream {
            print_plan_json(
                &plan,
                &params.target,
                Some(detect_effective_runtime(config, spawn_mode).into()),
            );
        } else {
            print_plan(&plan, config.run_model.as_deref());
            if let Some(agent) = ready_queue::detect_direct_agent().map(|agent| match agent {
                ready_queue::DirectAgent::Imp => "imp",
            }) {
                eprintln!(
                    "Runtime: direct agent={} model={}",
                    agent,
                    config.run_model.as_deref().unwrap_or("default")
                );
            } else if let SpawnMode::Template { .. } = spawn_mode {
                eprintln!(
                    "Runtime: template mode model={}",
                    config.run_model.as_deref().unwrap_or("default")
                );
            }
        }
        return Ok(());
    }

    let decision_warnings = collect_decision_warnings(mana_dir, &plan.all_units, &plan.index)?;
    if !confirm_dispatch_with_decisions(&decision_warnings, params.json_stream)? {
        if !params.json_stream {
            eprintln!("Dispatch cancelled.");
        }
        return Ok(());
    }

    // Report blocked units (oversized/unscoped)
    if !plan.skipped.is_empty() && !params.json_stream {
        eprintln!("{} unit(s) blocked:", plan.skipped.len());
        for bb in &plan.skipped {
            eprintln!("  ⚠ {}  {}  ({})", bb.id, bb.title, bb.reason);
        }
        eprintln!();
    }

    let total_units: usize = plan.waves.iter().map(|w| w.units.len()).sum();
    let total_waves = plan.waves.len();
    let parent_id = params.target.scope_label();

    if params.json_stream {
        let units_info: Vec<stream::UnitInfo> = plan
            .waves
            .iter()
            .enumerate()
            .flat_map(|(wave_idx, wave)| {
                wave.units.iter().map(move |b| stream::UnitInfo {
                    id: b.id.clone(),
                    title: b.title.clone(),
                    round: wave_idx + 1,
                })
            })
            .collect();
        stream::emit(&StreamEvent::RunStart {
            parent_id,
            total_units,
            total_rounds: total_waves,
            units: units_info,
            runtime: Some(detect_effective_runtime(config, spawn_mode).into()),
        });
    }

    let run_cfg = RunConfig {
        max_jobs: params.jobs.min(config.max_concurrent) as usize,
        timeout_minutes: params.timeout,
        idle_timeout_minutes: params.idle_timeout,
        json_stream: params.json_stream,
        file_locking: config.file_locking,
        run_model: config.run_model.clone(),
        batch_verify: config.batch_verify,
        memory_reserve_mb: config.memory_reserve_mb,
    };
    let run_start = Instant::now();
    let outcome_counts;
    let any_failed;
    let mut total_tokens: u64 = 0;
    let mut total_cost: f64 = 0.0;
    // Collect IDs of successfully closed units for --review post-processing
    let successful_ids: Vec<String>;

    match spawn_mode {
        SpawnMode::Direct => {
            if !params.json_stream {
                eprintln!("Dispatching {} unit(s)...", total_units);
            }

            // Ready-queue: start each unit as soon as its specific deps finish.
            // Progress (▸ start, ✓/✗ done) is printed in real-time by the queue.
            let (results, had_failure) = run_ready_queue_direct(
                mana_dir,
                &plan.all_units,
                &plan.index,
                &run_cfg,
                params.keep_going,
            )?;

            for result in &results {
                total_tokens += result.total_tokens.unwrap_or(0);
                total_cost += result.total_cost.unwrap_or(0.0);
                if params.json_stream {
                    stream::emit(&StreamEvent::UnitDone {
                        id: result.id.clone(),
                        success: result.success,
                        duration_secs: result.duration.as_secs(),
                        error: if result.success {
                            None
                        } else {
                            result.error.clone()
                        },
                        total_tokens: result.total_tokens,
                        total_cost: result.total_cost,
                        tool_count: Some(result.tool_count),
                        turns: Some(result.turns),
                        failure_summary: if result.success {
                            None
                        } else {
                            result.failure_summary.clone()
                        },
                    });
                }
            }
            let mut branch_any_failed = had_failure;

            // After all agents complete, run batch verification if enabled.
            // Each agent exits with AwaitingVerify status; the runner now resolves them.
            if run_cfg.batch_verify {
                match mana_core::ops::batch_verify::batch_verify(mana_dir) {
                    Ok(bv) => {
                        if params.json_stream {
                            stream::emit(&StreamEvent::BatchVerify {
                                commands_run: bv.commands_run,
                                passed: bv.passed.clone(),
                                failed: bv.failed.iter().map(|f| f.unit_id.clone()).collect(),
                            });
                        } else {
                            print_batch_verify_result(&bv);
                        }
                    }
                    Err(e) => {
                        eprintln!("Batch verify error: {}", e);
                        branch_any_failed = true;
                    }
                }
            }

            let (counts, closed_ids) =
                collect_outcome_counts(mana_dir, &results, plan.skipped.len());
            outcome_counts = counts;
            successful_ids = closed_ids;
            any_failed = branch_any_failed || outcome_counts.has_failures();
        }

        SpawnMode::Template { .. } => {
            // Template mode: wave-based execution (legacy)
            let mut template_results = Vec::new();
            let mut had_failure = false;

            for (wave_idx, wave) in plan.waves.iter().enumerate() {
                // Check for shutdown signal between waves
                if shutdown_requested() {
                    if !params.json_stream {
                        eprintln!("\nShutdown signal received, stopping.");
                    }
                    had_failure = true;
                    break;
                }

                if params.json_stream {
                    stream::emit(&StreamEvent::RoundStart {
                        round: wave_idx + 1,
                        total_rounds: total_waves,
                        unit_count: wave.units.len(),
                    });
                } else {
                    eprintln!("Wave {}: {} unit(s)", wave_idx + 1, wave.units.len());
                }

                let results = run_wave(mana_dir, &wave.units, spawn_mode, &run_cfg, wave_idx + 1)?;

                let mut wave_success = 0usize;
                let mut wave_failed = 0usize;

                for result in &results {
                    let duration = format_duration(result.duration);
                    total_tokens += result.total_tokens.unwrap_or(0);
                    total_cost += result.total_cost.unwrap_or(0.0);
                    if result.success {
                        if params.json_stream {
                            stream::emit(&StreamEvent::UnitDone {
                                id: result.id.clone(),
                                success: true,
                                duration_secs: result.duration.as_secs(),
                                error: None,
                                total_tokens: result.total_tokens,
                                total_cost: result.total_cost,
                                tool_count: Some(result.tool_count),
                                turns: Some(result.turns),
                                failure_summary: None,
                            });
                        } else {
                            eprintln!("  ✓ {}  {}  {}", result.id, result.title, duration);
                        }
                        wave_success += 1;
                    } else {
                        if params.json_stream {
                            stream::emit(&StreamEvent::UnitDone {
                                id: result.id.clone(),
                                success: false,
                                duration_secs: result.duration.as_secs(),
                                error: result.error.clone(),
                                total_tokens: result.total_tokens,
                                total_cost: result.total_cost,
                                tool_count: Some(result.tool_count),
                                turns: Some(result.turns),
                                failure_summary: result.failure_summary.clone(),
                            });
                        } else {
                            let err = result.error.as_deref().unwrap_or("failed");
                            eprintln!(
                                "  ✗ {}  {}  {} ({})",
                                result.id, result.title, duration, err
                            );
                        }
                        wave_failed += 1;
                        had_failure = true;
                    }
                }

                template_results.extend(results);

                if params.json_stream {
                    stream::emit(&StreamEvent::RoundEnd {
                        round: wave_idx + 1,
                        success_count: wave_success,
                        failed_count: wave_failed,
                    });
                }

                if had_failure && !params.keep_going {
                    break;
                }
            }

            let (counts, closed_ids) =
                collect_outcome_counts(mana_dir, &template_results, plan.skipped.len());
            outcome_counts = counts;
            successful_ids = closed_ids;
            any_failed = had_failure || outcome_counts.has_failures();
        }
    }

    // Trigger adversarial review for each successfully closed unit if --review is set.
    // Review runs synchronously after all units in this pass complete.
    if params.review && !successful_ids.is_empty() {
        for id in &successful_ids {
            if !params.json_stream {
                eprintln!("Review: checking {} ...", id);
            }
            if let Err(e) = cmd_review(
                mana_dir,
                ReviewArgs {
                    id: id.clone(),
                    model: None,
                    diff_only: false,
                },
            ) {
                eprintln!("Review: warning — review of {} failed: {}", id, e);
            }
        }
    }

    if params.json_stream {
        stream::emit(&StreamEvent::RunEnd {
            total_success: outcome_counts.closed as usize,
            total_closed: outcome_counts.closed as usize,
            total_failed: outcome_counts.total_failed_for_legacy_stream(),
            total_abandoned: outcome_counts.abandoned as usize,
            total_awaiting_verify: outcome_counts.awaiting_verify as usize,
            total_skipped: outcome_counts.skipped as usize,
            duration_secs: run_start.elapsed().as_secs(),
        });
    } else {
        let elapsed = format_duration(run_start.elapsed());
        let mut summary = format!(
            "\nDone: {} closed, {} failed, {} abandoned, {} awaiting verify, {} skipped  ({})",
            outcome_counts.closed,
            outcome_counts.failed,
            outcome_counts.abandoned,
            outcome_counts.awaiting_verify,
            outcome_counts.skipped,
            elapsed,
        );
        if total_tokens > 0 || total_cost > 0.0 {
            let token_str = if total_tokens >= 1_000_000 {
                format!("{:.1}M tokens", total_tokens as f64 / 1_000_000.0)
            } else if total_tokens >= 1_000 {
                format!("{}k tokens", total_tokens / 1_000)
            } else {
                format!("{} tokens", total_tokens)
            };
            summary.push_str(&format!("  [{}, ${:.2}]", token_str, total_cost));
        }
        eprintln!("{}", summary);
    }

    if any_failed && !params.keep_going {
        anyhow::bail!("Some agents failed");
    }

    Ok(())
}

/// Loop mode: keep dispatching until no ready units remain.
fn run_loop(
    mana_dir: &Path,
    config: &Config,
    params: &NativeRunParams,
    _spawn_mode: &SpawnMode,
) -> Result<()> {
    let max_loops = if config.max_loops == 0 {
        u32::MAX
    } else {
        config.max_loops
    };

    for iteration in 0..max_loops {
        // Check for shutdown signal between loop iterations
        if shutdown_requested() {
            if !params.json_stream {
                eprintln!("\nShutdown signal received, stopping.");
            }
            return Ok(());
        }

        if iteration > 0 && !params.json_stream {
            eprintln!("\n--- Loop iteration {} ---\n", iteration + 1);
        }

        let plan = plan_dispatch(mana_dir, config, &params.target, false)?;

        if plan.waves.is_empty() {
            if !params.json_stream {
                if iteration == 0 {
                    eprintln!("No ready units. Use `mana status` to see what's going on.");
                } else {
                    eprintln!("No more ready units. Stopping.");
                }
            }
            return Ok(());
        }

        let inner_params = NativeRunParams {
            loop_mode: false,
            ..params.clone()
        };

        // Reload config each iteration (agents may have changed units)
        let config = Config::load_with_extends(mana_dir)?;
        let spawn_mode = determine_spawn_mode(&config);
        match run_once(mana_dir, &config, &inner_params, &spawn_mode) {
            Ok(()) => {}
            Err(e) => {
                if params.keep_going {
                    eprintln!("Warning: {}", e);
                } else {
                    return Err(e);
                }
            }
        }
    }

    eprintln!("Reached max_loops ({}). Stopping.", max_loops);
    Ok(())
}

/// Print a human-readable summary of a batch verify run.
///
/// Example output:
///   Batch verify: 2 commands, 3/4 units passed
///     ✓ cargo check -p mana-cli  (units: 1.1, 1.2, 1.3)
///     ✗ cargo test -p mana-core  (unit: 1.4) — exit code 1
fn print_batch_verify_result(result: &mana_core::ops::batch_verify::BatchVerifyResult) {
    let total = result.passed.len() + result.failed.len();
    eprintln!(
        "\nBatch verify: {} command{}, {}/{} unit{} passed",
        result.commands_run,
        if result.commands_run == 1 { "" } else { "s" },
        result.passed.len(),
        total,
        if total == 1 { "" } else { "s" },
    );

    if !result.passed.is_empty() {
        eprintln!(
            "  ✓ {} unit{} passed",
            result.passed.len(),
            if result.passed.len() == 1 { "" } else { "s" }
        );
    }

    // Group failures by verify command for compact display.
    let mut by_cmd: std::collections::HashMap<&str, Vec<&str>> = std::collections::HashMap::new();
    for failure in &result.failed {
        by_cmd
            .entry(&failure.verify_command)
            .or_default()
            .push(&failure.unit_id);
    }

    // Sort for deterministic output.
    let mut cmd_entries: Vec<(&str, Vec<&str>)> = by_cmd.into_iter().collect();
    cmd_entries.sort_by_key(|(cmd, _)| *cmd);

    for (cmd, ids) in cmd_entries {
        let ids_str = ids.join(", ");
        let unit_word = if ids.len() == 1 { "unit" } else { "units" };
        // Find exit code for this command from the first matching failure
        let exit_info = result
            .failed
            .iter()
            .find(|f| f.verify_command == cmd)
            .map(|f| {
                if f.timed_out {
                    " — timed out".to_string()
                } else if let Some(code) = f.exit_code {
                    format!(" — exit code {}", code)
                } else {
                    String::new()
                }
            })
            .unwrap_or_default();
        eprintln!("  ✗ {}  ({}: {}){}", cmd, unit_word, ids_str, exit_info);
    }
}

/// Format a duration as M:SS.
pub(super) fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

#[derive(Debug, Clone, Serialize, serde::Deserialize, Default)]
pub struct RunSummary {
    pub total_units: usize,
    pub total_rounds: usize,
    pub total_closed: usize,
    pub total_failed: usize,
    pub total_abandoned: usize,
    pub total_awaiting_verify: usize,
    pub total_skipped: usize,
    pub duration_secs: u64,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct RunUnitStatus {
    pub id: String,
    pub title: String,
    pub status: String,
    pub round: Option<usize>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub duration_secs: Option<u64>,
    pub tool_count: Option<usize>,
    pub turns: Option<usize>,
    pub failure_summary: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RunRuntimeInfo {
    pub direct_agent: Option<String>,
    pub model: Option<String>,
}

impl From<RunRuntimeInfo> for stream::RunRuntimeInfo {
    fn from(value: RunRuntimeInfo) -> Self {
        Self {
            direct_agent: value.direct_agent,
            model: value.model,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RunView {
    pub summary: RunSummary,
    pub units: Vec<RunUnitStatus>,
    pub events: Vec<StreamEvent>,
    pub runtime: Option<RunRuntimeInfo>,
}

/// Execute `mana run` programmatically and capture structured stream events.
pub fn run_with_stream_capture(mana_dir: &Path, params: NativeRunParams) -> Result<RunView> {
    run_with_stream_capture_and_sink(mana_dir, params, None)
}

/// Execute `mana run` programmatically, optionally forwarding live events to a sink.
pub fn run_with_stream_capture_and_sink(
    mana_dir: &Path,
    params: NativeRunParams,
    sink: Option<stream::StreamSink>,
) -> Result<RunView> {
    let events: Arc<Mutex<Vec<StreamEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_events = Arc::clone(&events);
    let forward = sink.clone();
    let _guard = stream::install_sink(Arc::new(move |event| {
        if let Ok(mut buf) = sink_events.lock() {
            buf.push(event.clone());
        }
        if let Some(ref sink) = forward {
            sink(event);
        }
    }));

    // Install signal handlers for clean shutdown on Ctrl+C / SIGTERM
    install_signal_handlers();

    // Determine spawn mode
    let config = Config::load_with_extends(mana_dir)?;
    let spawn_mode = determine_spawn_mode(&config);

    if spawn_mode == SpawnMode::Direct && !imp_available() {
        anyhow::bail!(
            "No direct agent configured and `imp` was not found on PATH.\n\n\
             Either:\n  \
               1. Install imp (Rust): cargo install imp-cli\n  \
               2. Set a run template: mana config set run \"<command>\""
        );
    }

    let params = NativeRunParams {
        json_stream: true,
        ..params
    };

    if params.loop_mode {
        run_loop(mana_dir, &config, &params, &spawn_mode)?;
    } else {
        run_once(mana_dir, &config, &params, &spawn_mode)?;
    }

    let events = events.lock().map(|buf| buf.clone()).unwrap_or_default();
    Ok(build_run_view_from_events(
        events,
        Some(detect_effective_runtime(&config, &spawn_mode)),
    ))
}

fn detect_effective_runtime(config: &Config, spawn_mode: &SpawnMode) -> RunRuntimeInfo {
    let direct_agent = match spawn_mode {
        SpawnMode::Direct => ready_queue::detect_direct_agent().map(|agent| match agent {
            ready_queue::DirectAgent::Imp => "imp".to_string(),
        }),
        SpawnMode::Template { run_template, .. } => Some(if run_template.contains("imp") {
            "imp".to_string()
        } else if run_template.contains("pi") {
            "pi".to_string()
        } else {
            "template".to_string()
        }),
    };

    RunRuntimeInfo {
        direct_agent,
        model: config.run_model.clone(),
    }
}

fn build_run_view_from_events(
    events: Vec<StreamEvent>,
    runtime: Option<RunRuntimeInfo>,
) -> RunView {
    use std::collections::HashMap;

    let mut total_units = 0usize;
    let mut total_rounds = 0usize;
    let mut summary = RunSummary {
        total_units: 0,
        total_rounds: 0,
        total_closed: 0,
        total_failed: 0,
        total_abandoned: 0,
        total_awaiting_verify: 0,
        total_skipped: 0,
        duration_secs: 0,
    };
    let mut units: HashMap<String, RunUnitStatus> = HashMap::new();

    for event in &events {
        match event {
            StreamEvent::RunStart {
                total_units: tu,
                total_rounds: tr,
                units: infos,
                ..
            } => {
                total_units = *tu;
                total_rounds = *tr;
                summary.total_units = *tu;
                summary.total_rounds = *tr;
                for info in infos {
                    units
                        .entry(info.id.clone())
                        .or_insert_with(|| RunUnitStatus {
                            id: info.id.clone(),
                            title: info.title.clone(),
                            status: "queued".to_string(),
                            round: Some(info.round),
                            agent: None,
                            model: None,
                            duration_secs: None,
                            tool_count: None,
                            turns: None,
                            failure_summary: None,
                            error: None,
                        });
                }
            }
            StreamEvent::UnitStart {
                id, title, round, ..
            } => {
                let entry = units.entry(id.clone()).or_insert_with(|| RunUnitStatus {
                    id: id.clone(),
                    title: title.clone(),
                    status: "queued".to_string(),
                    round: Some(*round),
                    agent: None,
                    model: None,
                    duration_secs: None,
                    tool_count: None,
                    turns: None,
                    failure_summary: None,
                    error: None,
                });
                entry.title = title.clone();
                entry.round = Some(*round);
                entry.status = "running".to_string();
            }
            StreamEvent::UnitDone {
                id,
                success,
                duration_secs,
                error,
                tool_count,
                turns,
                failure_summary,
                ..
            } => {
                let entry = units.entry(id.clone()).or_insert_with(|| RunUnitStatus {
                    id: id.clone(),
                    title: id.clone(),
                    status: "queued".to_string(),
                    round: None,
                    agent: None,
                    model: None,
                    duration_secs: None,
                    tool_count: None,
                    turns: None,
                    failure_summary: None,
                    error: None,
                });
                entry.status = if *success { "done" } else { "failed" }.to_string();
                entry.duration_secs = Some(*duration_secs);
                entry.tool_count = *tool_count;
                entry.turns = *turns;
                entry.failure_summary = failure_summary.clone();
                entry.error = error.clone();
            }
            StreamEvent::RunEnd {
                total_closed,
                total_failed,
                total_abandoned,
                total_awaiting_verify,
                total_skipped,
                duration_secs,
                ..
            } => {
                summary.total_closed = *total_closed;
                summary.total_failed = *total_failed;
                summary.total_abandoned = *total_abandoned;
                summary.total_awaiting_verify = *total_awaiting_verify;
                summary.total_skipped = *total_skipped;
                summary.duration_secs = *duration_secs;
            }
            StreamEvent::BatchVerify { passed, failed, .. } => {
                for id in passed {
                    if let Some(entry) = units.get_mut(id) {
                        entry.status = "done".to_string();
                    }
                }
                for id in failed {
                    if let Some(entry) = units.get_mut(id) {
                        entry.status = "failed".to_string();
                    }
                }
            }
            _ => {}
        }
    }

    let mut unit_list: Vec<RunUnitStatus> = units.into_values().collect();
    unit_list.sort_by(|a, b| crate::util::natural_cmp(&a.id, &b.id));

    if summary.total_units == 0 {
        summary.total_units = total_units.max(unit_list.len());
    }
    if summary.total_rounds == 0 {
        summary.total_rounds = total_rounds;
    }

    RunView {
        summary,
        units: unit_list,
        events,
        runtime,
    }
}

/// Find the unit file path. Public wrapper for use in other commands.
pub fn find_unit_file(mana_dir: &Path, id: &str) -> Result<PathBuf> {
    crate::discovery::find_unit_file(mana_dir, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_mana_dir() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();
        (dir, mana_dir)
    }

    fn write_config(mana_dir: &std::path::Path, run: Option<&str>) {
        let run_line = match run {
            Some(r) => format!("run: \"{}\"\n", r),
            None => String::new(),
        };
        fs::write(
            mana_dir.join("config.yaml"),
            format!("project: test\nnext_id: 1\n{}", run_line),
        )
        .unwrap();
    }

    fn default_args() -> RunArgs {
        RunArgs {
            id: None,
            jobs: 4,
            dry_run: false,
            loop_mode: false,
            keep_going: false,
            timeout: 30,
            idle_timeout: 5,
            json_stream: false,
            review: false,
        }
    }

    #[test]
    fn run_target_scope_label_formats_explicit_targets() {
        let target = RunTarget::Explicit(vec!["1".to_string(), "2.1".to_string()]);
        assert_eq!(target.scope_label(), "1,2.1");
    }

    #[test]
    fn cmd_run_errors_when_no_run_template_and_no_imp() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, None);

        let args = default_args();

        let result = cmd_run(&mana_dir, args);
        // With no template and no imp on PATH, should error
        if !imp_available() {
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(
                err.contains("No agent configured") || err.contains("not found"),
                "Error should mention missing agent: {}",
                err
            );
        }
    }

    #[test]
    fn dry_run_does_not_spawn() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, Some("echo {id}"));

        // Create a ready unit
        let mut unit = crate::unit::Unit::new("1", "Test unit");
        unit.verify = Some("echo ok".to_string());
        unit.to_file(mana_dir.join("1-test.md")).unwrap();

        let args = RunArgs {
            dry_run: true,
            ..default_args()
        };

        // dry_run should succeed without spawning any processes
        let result = cmd_run(&mana_dir, args);
        assert!(result.is_ok());
    }

    #[test]
    fn dry_run_with_json_stream() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, Some("echo {id}"));

        let mut unit = crate::unit::Unit::new("1", "Test unit");
        unit.verify = Some("echo ok".to_string());
        unit.to_file(mana_dir.join("1-test.md")).unwrap();

        let args = RunArgs {
            dry_run: true,
            json_stream: true,
            ..default_args()
        };

        // Should succeed and emit JSON events (captured to stdout)
        let result = cmd_run(&mana_dir, args);
        assert!(result.is_ok());
    }

    #[test]
    fn format_duration_formats_correctly() {
        assert_eq!(format_duration(Duration::from_secs(0)), "0:00");
        assert_eq!(format_duration(Duration::from_secs(32)), "0:32");
        assert_eq!(format_duration(Duration::from_secs(62)), "1:02");
        assert_eq!(format_duration(Duration::from_secs(600)), "10:00");
    }

    #[test]
    fn determine_spawn_mode_template_when_run_set() {
        let config = Config {
            project: "test".to_string(),
            next_id: 1,
            auto_close_parent: true,
            run: Some("echo {id}".to_string()),
            plan: Some("plan {id}".to_string()),
            max_loops: 10,
            max_concurrent: 4,
            poll_interval: 30,
            extends: vec![],
            rules_file: None,
            file_locking: false,
            worktree: false,
            on_close: None,
            on_fail: None,
            verify_timeout: None,
            review: None,
            user: None,
            user_email: None,
            auto_commit: false,
            commit_template: None,
            research: None,
            run_model: None,
            plan_model: None,
            review_model: None,
            research_model: None,
            batch_verify: false,
            memory_reserve_mb: 0,
            notify: None,
        };
        let mode = determine_spawn_mode(&config);
        assert_eq!(
            mode,
            SpawnMode::Template {
                run_template: "echo {id}".to_string(),
                plan_template: Some("plan {id}".to_string()),
            }
        );
    }

    #[test]
    fn determine_spawn_mode_direct_when_no_run() {
        let config = Config {
            project: "test".to_string(),
            next_id: 1,
            auto_close_parent: true,
            run: None,
            plan: None,
            max_loops: 10,
            max_concurrent: 4,
            poll_interval: 30,
            extends: vec![],
            rules_file: None,
            file_locking: false,
            worktree: false,
            on_close: None,
            on_fail: None,
            verify_timeout: None,
            review: None,
            user: None,
            user_email: None,
            auto_commit: false,
            commit_template: None,
            research: None,
            run_model: None,
            plan_model: None,
            review_model: None,
            research_model: None,
            batch_verify: false,
            memory_reserve_mb: 0,
            notify: None,
        };
        let mode = determine_spawn_mode(&config);
        assert_eq!(mode, SpawnMode::Direct);
    }

    #[test]
    fn agent_result_tracks_tokens_and_cost() {
        let result = AgentResult {
            id: "1".to_string(),
            title: "Test".to_string(),
            action: UnitAction::Implement,
            success: true,
            duration: Duration::from_secs(10),
            total_tokens: Some(5000),
            total_cost: Some(0.03),
            error: None,
            tool_count: 5,
            turns: 2,
            failure_summary: None,
        };
        assert_eq!(result.total_tokens, Some(5000));
        assert_eq!(result.total_cost, Some(0.03));
    }

    #[test]
    fn collect_decision_warnings_only_returns_dispatch_units_with_decisions() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, Some("echo {id}"));

        let mut unit1 = crate::unit::Unit::new("1", "Has decisions");
        unit1.verify = Some("echo ok".to_string());
        unit1.decisions = vec!["JWT or session cookies?".to_string()];
        unit1.to_file(mana_dir.join("1-has-decisions.md")).unwrap();

        let mut unit2 = crate::unit::Unit::new("2", "No decisions");
        unit2.verify = Some("echo ok".to_string());
        unit2.to_file(mana_dir.join("2-no-decisions.md")).unwrap();

        let index = crate::index::Index::build(&mana_dir).unwrap();
        let units = vec![
            SizedUnit {
                id: "1".to_string(),
                title: "Has decisions".to_string(),
                action: UnitAction::Implement,
                priority: 2,
                dependencies: Vec::new(),
                parent: None,
                produces: Vec::new(),
                requires: Vec::new(),
                paths: Vec::new(),
                model: None,
            },
            SizedUnit {
                id: "2".to_string(),
                title: "No decisions".to_string(),
                action: UnitAction::Implement,
                priority: 2,
                dependencies: Vec::new(),
                parent: None,
                produces: Vec::new(),
                requires: Vec::new(),
                paths: Vec::new(),
                model: None,
            },
        ];

        let warnings = collect_decision_warnings(&mana_dir, &units, &index).unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].id, "1");
        assert_eq!(warnings[0].decisions, vec!["JWT or session cookies?"]);
    }

    #[test]
    fn format_decision_warning_message_matches_single_unit_prompt() {
        let message = format_decision_warning_message(&[DecisionWarning {
            id: "42".to_string(),
            title: "Implement auth".to_string(),
            decisions: vec![
                "JWT or session cookies?".to_string(),
                "Which JWT library?".to_string(),
            ],
        }]);

        assert!(message.contains("⚠ Unit 42 has 2 unresolved decisions"));
        assert!(message.contains("0: JWT or session cookies?"));
        assert!(message.contains("1: Which JWT library?"));
    }

    #[test]
    fn signal_flag_defaults_to_false() {
        SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
        assert!(!shutdown_requested());
    }

    #[test]
    fn signal_flag_can_be_toggled() {
        SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
        assert!(shutdown_requested());
        // Reset for other tests
        SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
        assert!(!shutdown_requested());
    }

    #[test]
    fn child_pid_tracking() {
        // Clear any existing PIDs
        if let Ok(mut pids) = CHILD_PIDS.lock() {
            pids.clear();
        }

        register_child_pid(1234);
        register_child_pid(5678);

        let count = CHILD_PIDS.lock().unwrap().len();
        assert_eq!(count, 2);

        unregister_child_pid(1234);
        let count = CHILD_PIDS.lock().unwrap().len();
        assert_eq!(count, 1);

        // Unregister non-existent PID is a no-op
        unregister_child_pid(9999);
        let count = CHILD_PIDS.lock().unwrap().len();
        assert_eq!(count, 1);

        unregister_child_pid(5678);
        let count = CHILD_PIDS.lock().unwrap().len();
        assert_eq!(count, 0);
    }

    #[test]
    fn run_summary_counts_only_closed_units_as_success() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, Some("echo {id}"));

        let mut closed = crate::unit::Unit::new("1", "Closed");
        closed.verify = Some("echo ok".to_string());
        closed.status = Status::Closed;
        closed.to_file(mana_dir.join("1-closed.md")).unwrap();

        let mut failed = crate::unit::Unit::new("2", "Failed");
        failed.verify = Some("echo ok".to_string());
        failed.to_file(mana_dir.join("2-failed.md")).unwrap();

        let mut abandoned = crate::unit::Unit::new("3", "Abandoned");
        abandoned.verify = Some("echo ok".to_string());
        abandoned.attempt_log.push(crate::unit::AttemptRecord {
            num: 1,
            outcome: AttemptOutcome::Abandoned,
            notes: None,
            agent: None,
            started_at: None,
            finished_at: None,
        });
        abandoned.to_file(mana_dir.join("3-abandoned.md")).unwrap();

        let mut awaiting_verify = crate::unit::Unit::new("4", "Awaiting verify");
        awaiting_verify.verify = Some("echo ok".to_string());
        awaiting_verify.status = Status::AwaitingVerify;
        awaiting_verify
            .to_file(mana_dir.join("4-awaiting-verify.md"))
            .unwrap();

        let make_result = |id: &str| AgentResult {
            id: id.to_string(),
            title: format!("Unit {id}"),
            action: UnitAction::Implement,
            success: true,
            duration: Duration::from_secs(1),
            total_tokens: None,
            total_cost: None,
            error: None,
            tool_count: 0,
            turns: 0,
            failure_summary: None,
        };

        let results = vec![
            make_result("1"),
            make_result("2"),
            make_result("3"),
            make_result("4"),
        ];

        let (counts, closed_ids) = collect_outcome_counts(&mana_dir, &results, 2);
        assert_eq!(counts.closed, 1);
        assert_eq!(counts.failed, 1);
        assert_eq!(counts.abandoned, 1);
        assert_eq!(counts.awaiting_verify, 1);
        assert_eq!(counts.skipped, 2);
        assert_eq!(closed_ids, vec!["1".to_string()]);
    }

    #[test]
    fn run_loop_target_subtree_drains_ready_descendants() {
        let (_dir, mana_dir) = make_mana_dir();
        let run = format!(
            "python3 -c 'from pathlib import Path; p = next(Path(\"{}\").glob(\"{{id}}-*.md\")); text = p.read_text(); p.write_text(text.replace(\"status: open\", \"status: closed\", 1))'",
            mana_dir.display()
        );
        let yaml_run = run.replace('\\', "\\\\").replace('"', "\\\"");
        fs::write(
            mana_dir.join("config.yaml"),
            format!(
                "project: test\nnext_id: 1\nmax_loops: 10\nrun: \"{}\"\n",
                yaml_run
            ),
        )
        .unwrap();

        let parent = crate::unit::Unit::new("1", "Parent");
        parent.to_file(mana_dir.join("1-parent.md")).unwrap();

        let mut intermediate = crate::unit::Unit::new("1.1", "Intermediate");
        intermediate.parent = Some("1".to_string());
        intermediate.verify = Some("echo ok".to_string());
        intermediate
            .to_file(mana_dir.join("1.1-intermediate.md"))
            .unwrap();

        let mut nested_leaf = crate::unit::Unit::new("1.1.1", "Nested leaf");
        nested_leaf.parent = Some("1.1".to_string());
        nested_leaf.verify = Some("echo ok".to_string());
        nested_leaf
            .to_file(mana_dir.join("1.1.1-nested-leaf.md"))
            .unwrap();

        let mut sibling_leaf = crate::unit::Unit::new("1.2", "Sibling leaf");
        sibling_leaf.parent = Some("1".to_string());
        sibling_leaf.verify = Some("echo ok".to_string());
        sibling_leaf
            .to_file(mana_dir.join("1.2-sibling-leaf.md"))
            .unwrap();

        let config = Config::load_with_extends(&mana_dir).unwrap();
        let args = RunArgs {
            id: Some("1".to_string()),
            loop_mode: true,
            ..default_args()
        };

        run_loop(
            &mana_dir,
            &config,
            &NativeRunParams::from(args),
            &determine_spawn_mode(&config),
        )
        .unwrap();

        let intermediate = Unit::from_file(&mana_dir.join("1.1-intermediate.md")).unwrap();
        let nested_leaf = Unit::from_file(&mana_dir.join("1.1.1-nested-leaf.md")).unwrap();
        let sibling_leaf = Unit::from_file(&mana_dir.join("1.2-sibling-leaf.md")).unwrap();
        let parent = Unit::from_file(&mana_dir.join("1-parent.md")).unwrap();

        assert_eq!(nested_leaf.status, Status::Closed);
        assert_eq!(sibling_leaf.status, Status::Closed);
        assert_eq!(intermediate.status, Status::Closed);
        assert_eq!(parent.status, Status::Open);
    }
}
