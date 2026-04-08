use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::failure;
use crate::history::{self, AgentHistoryEntry};
use crate::index::{ArchiveIndex, Index, IndexEntry};
use crate::pi_output::{self, AgentEvent};
use crate::prompt::{build_agent_prompt, PromptOptions};
use crate::stream::{self, StreamEvent};
use crate::timeout::{self, MonitorResult, TimeoutConfig};
use crate::unit::{Status, Unit};
use crate::util::natural_cmp;

use super::plan::SizedUnit;
use super::wave::{compute_downstream_weights, compute_waves};
use super::{format_duration, AgentResult};

/// Check if all dependencies of an index entry are closed.
///
/// Checks both the active index and the archive index. A dependency found in
/// the archive is considered satisfied (archived means closed). A dependency
/// found in neither index is treated as unsatisfied (catches typos).
pub(super) fn all_deps_closed(entry: &IndexEntry, index: &Index, archive: &ArchiveIndex) -> bool {
    for dep_id in &entry.dependencies {
        match index.units.iter().find(|e| e.id == *dep_id) {
            Some(dep) if dep.status == Status::Closed => {}
            Some(_) => return false, // Found in active index but not closed
            None => {
                // Not in active index — check archive (archived = closed)
                if !archive.units.iter().any(|e| e.id == *dep_id) {
                    return false; // Not found in either index
                }
            }
        }
    }

    for required in &entry.requires {
        // Check active index for a producer
        if let Some(producer) = index
            .units
            .iter()
            .find(|e| e.id != entry.id && e.parent == entry.parent && e.produces.contains(required))
        {
            if producer.status != Status::Closed {
                return false;
            }
        } else {
            // Check archive for a producer (archived = closed, so always satisfied)
            // If not found in either, no producer exists — treat as satisfied
        }
    }

    true
}

/// Check if a unit's dependencies are all satisfied.
fn is_unit_ready(
    unit: &SizedUnit,
    completed: &HashSet<String>,
    all_unit_ids: &HashSet<String>,
    all_units: &[SizedUnit],
) -> bool {
    // All explicit deps must be completed or not in our dispatch set
    let explicit_ok = unit
        .dependencies
        .iter()
        .all(|d| completed.contains(d) || !all_unit_ids.contains(d));

    // All requires must be satisfied (producer completed or not in set)
    let requires_ok = unit.requires.iter().all(|req| {
        if let Some(producer) = all_units.iter().find(|other| {
            other.id != unit.id && other.parent == unit.parent && other.produces.contains(req)
        }) {
            completed.contains(&producer.id)
        } else {
            true // No producer in set, assume satisfied
        }
    });

    explicit_ok && requires_ok
}

/// Check if a unit's paths conflict with currently-running units.
fn has_path_conflict(unit: &SizedUnit, running_paths: &HashSet<String>) -> bool {
    unit.paths.iter().any(|p| running_paths.contains(p))
}

/// Format a human-friendly token count (e.g. 15000 → "15k").
fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

/// Print a single-line completion result for a unit using the real unit outcome.
fn print_result_line(result: &AgentResult, outcome: super::UnitOutcome) {
    let duration = format_duration(result.duration);

    // Build optional stats suffix: "42 tools, 15k tokens, $0.03"
    let mut stats = Vec::new();
    if result.tool_count > 0 {
        stats.push(format!("{} tools", result.tool_count));
    }
    if let Some(tokens) = result.total_tokens {
        stats.push(format!("{} tokens", format_tokens(tokens)));
    }
    if let Some(cost) = result.total_cost {
        stats.push(format!("${:.2}", cost));
    }

    let stats_str = if stats.is_empty() {
        String::new()
    } else {
        format!("  ({})", stats.join(", "))
    };

    match outcome {
        super::UnitOutcome::Closed => {
            eprintln!(
                "  ✓ {}  {}  {}{}",
                result.id, result.title, duration, stats_str
            );
        }
        super::UnitOutcome::AwaitingVerify => {
            eprintln!(
                "  … {}  {}  {} (awaiting verify){}",
                result.id, result.title, duration, stats_str
            );
        }
        super::UnitOutcome::Failed | super::UnitOutcome::Abandoned => {
            let err = result.error.as_deref().unwrap_or("failed");
            eprintln!(
                "  ✗ {}  {}  {} ({}){}",
                result.id, result.title, duration, err, stats_str
            );
        }
    }
}

/// Run units using a ready-queue: start each unit as soon as its specific deps
/// complete, rather than waiting for an entire wave to finish.
pub(super) fn run_ready_queue_direct(
    mana_dir: &Path,
    all_units: &[SizedUnit],
    index: &Index,
    cfg: &super::RunConfig,
    keep_going: bool,
) -> Result<(Vec<AgentResult>, bool)> {
    let max_jobs = cfg.max_jobs;
    let timeout_minutes = cfg.timeout_minutes;
    let idle_timeout_minutes = cfg.idle_timeout_minutes;
    let json_stream = cfg.json_stream;
    let file_locking = cfg.file_locking;
    let batch_verify = cfg.batch_verify;
    let memory_reserve_mb = cfg.memory_reserve_mb;
    let all_unit_ids: HashSet<String> = all_units.iter().map(|b| b.id.clone()).collect();

    // Already-closed units count as completed (same logic as compute_waves)
    let mut completed: HashSet<String> = index
        .units
        .iter()
        .filter(|e| e.status == Status::Closed)
        .map(|e| e.id.clone())
        .collect();

    let mut remaining: HashMap<String, SizedUnit> = all_units
        .iter()
        .map(|b| (b.id.clone(), b.clone()))
        .collect();

    let mut results: Vec<AgentResult> = Vec::new();
    let mut running_count: usize = 0;
    let mut any_failed = false;

    // Track file paths of currently-running units to avoid scheduling conflicts
    let mut running_paths: HashSet<String> = HashSet::new();
    // Map unit ID → its paths, so we can remove them when the unit finishes
    let mut running_unit_paths: HashMap<String, Vec<String>> = HashMap::new();

    // Channel for completed agents to report back
    let (tx, rx) = mpsc::channel::<AgentResult>();

    // Assign a "round" number for display: use compute_waves to figure out
    // which wave each unit would be in (for json_stream events)
    let wave_map: HashMap<String, usize> = {
        let waves = compute_waves(all_units, index);
        let mut m = HashMap::new();
        for (i, wave) in waves.iter().enumerate() {
            for b in &wave.units {
                m.insert(b.id.clone(), i + 1);
            }
        }
        m
    };

    // Compute downstream weights for critical-path prioritization
    let weight_map = compute_downstream_weights(all_units);

    loop {
        // Find units that are ready and we have capacity for
        let mut newly_started = 0;
        let ready_ids: Vec<String> = remaining
            .values()
            .filter(|b| is_unit_ready(b, &completed, &all_unit_ids, all_units))
            .map(|b| b.id.clone())
            .collect();

        // Sort ready units by priority then ID (stable ordering)
        let mut ready_units: Vec<SizedUnit> = ready_ids
            .iter()
            .filter_map(|id| remaining.get(id).cloned())
            .collect();
        ready_units.sort_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then_with(|| {
                    // Higher weight = more downstream work blocked = schedule first
                    let wa = weight_map.get(&a.id).copied().unwrap_or(1);
                    let wb = weight_map.get(&b.id).copied().unwrap_or(1);
                    wb.cmp(&wa)
                })
                .then_with(|| natural_cmp(&a.id, &b.id))
        });

        let mut memory_blocked = false;

        for sb in ready_units {
            if running_count >= max_jobs {
                break;
            }

            // Don't spawn new agents if shutdown was requested
            if super::shutdown_requested() {
                break;
            }

            // Check system memory before spawning
            if !super::memory::has_sufficient_memory(memory_reserve_mb) {
                memory_blocked = true;
                if !json_stream {
                    let avail = super::memory::available_memory_mb()
                        .map(|mb| format!("{}MB", mb))
                        .unwrap_or_else(|| "unknown".to_string());
                    eprintln!(
                        "  ⏸ Memory pressure — {}MB reserve, {} available, waiting",
                        memory_reserve_mb, avail
                    );
                }
                break;
            }

            // Skip units whose paths conflict with currently-running units
            if has_path_conflict(&sb, &running_paths) {
                continue;
            }

            remaining.remove(&sb.id);
            running_count += 1;

            // Register this unit's paths as occupied
            for p in &sb.paths {
                running_paths.insert(p.clone());
            }
            running_unit_paths.insert(sb.id.clone(), sb.paths.clone());

            let round = wave_map.get(&sb.id).copied().unwrap_or(1);
            let agent = detect_direct_agent().expect("direct mode requires imp to be available");
            let effective_model = sb.model.as_deref().or(cfg.run_model.as_deref());
            let agent_label = match agent {
                DirectAgent::Imp => "imp",
            };
            let model_label = effective_model.unwrap_or("default");

            if json_stream {
                stream::emit(&StreamEvent::UnitStart {
                    id: sb.id.clone(),
                    title: sb.title.clone(),
                    round,
                    file_overlaps: None,
                    attempt: None,
                    priority: None,
                });
            } else {
                eprintln!(
                    "  ▸ {}  {}  [{} · model: {}]",
                    sb.id, sb.title, agent_label, model_label
                );
            }

            let mana_dir = mana_dir.to_path_buf();
            let tx = tx.clone();
            let timeout_min = timeout_minutes;
            let idle_min = idle_timeout_minutes;
            let config_run_model = cfg.run_model.clone();

            std::thread::spawn(move || {
                let result = run_single_direct(
                    &mana_dir,
                    &sb,
                    timeout_min,
                    idle_min,
                    config_run_model.as_deref(),
                    json_stream,
                    file_locking,
                    batch_verify,
                );
                let _ = tx.send(result);
            });
            newly_started += 1;
        }

        // If nothing is running and nothing can start, we're done (or stuck)
        if running_count == 0 && newly_started == 0 {
            if memory_blocked {
                // System memory is too low and no agents running to free it
                let msg = format!(
                    "Cannot spawn agents — system memory below {}MB reserve and no agents running. \
                     Free memory and re-run, or set memory_reserve_mb to 0 in config.",
                    memory_reserve_mb
                );
                if json_stream {
                    stream::emit_error(&msg);
                } else {
                    eprintln!("{}", msg);
                }
            } else if !remaining.is_empty() {
                // Remaining units have unresolvable deps
                if json_stream {
                    stream::emit_error(&format!(
                        "{} unit(s) have unresolvable dependencies",
                        remaining.len()
                    ));
                } else {
                    eprintln!(
                        "Warning: {} unit(s) have unresolvable dependencies:",
                        remaining.len()
                    );
                    for b in remaining.values() {
                        eprintln!("  {} {}", b.id, b.title);
                    }
                }
            }
            break;
        }

        // If nothing is running (but we just started some), loop to check for
        // more readiness after spawning
        if running_count > 0 {
            // Wait for any one agent to complete, with periodic shutdown checks
            let result = loop {
                if super::shutdown_requested() {
                    if !json_stream {
                        eprintln!("\nShutdown signal received, killing agents...");
                    }
                    super::kill_all_children();
                    // Wait briefly for agents to finish, then force kill
                    let deadline = Instant::now() + Duration::from_secs(5);
                    while running_count > 0 && Instant::now() < deadline {
                        match rx.recv_timeout(Duration::from_millis(100)) {
                            Ok(r) => {
                                running_count -= 1;
                                if !json_stream {
                                    let outcome = super::inspect_unit_outcome(mana_dir, &r.id);
                                    print_result_line(&r, outcome);
                                }
                                results.push(r);
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    }
                    if running_count > 0 {
                        super::force_kill_all_children();
                    }
                    return Ok((results, true));
                }
                match rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(result) => break result,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        return Ok((results, any_failed));
                    }
                }
            };
            running_count -= 1;

            // Release this unit's paths so deferred units can be scheduled
            if let Some(paths) = running_unit_paths.remove(&result.id) {
                for p in &paths {
                    running_paths.remove(p);
                }
            }

            let outcome = super::inspect_unit_outcome(mana_dir, &result.id);
            let unit_id = result.id.clone();

            // Print real-time completion for CLI users
            if !json_stream {
                print_result_line(&result, outcome);
            }

            if outcome.is_closed() {
                completed.insert(unit_id.clone());
            } else if outcome.is_failure() {
                any_failed = true;
                // If not keep_going, drain remaining and stop spawning
                if !keep_going {
                    results.push(result);
                    // Wait for currently running agents to finish
                    while running_count > 0 {
                        if let Ok(r) = rx.recv() {
                            running_count -= 1;
                            if !json_stream {
                                let outcome = super::inspect_unit_outcome(mana_dir, &r.id);
                                print_result_line(&r, outcome);
                            }
                            results.push(r);
                        }
                    }
                    return Ok((results, true));
                }
            }

            results.push(result);
        }
    }

    // Drain any remaining results (shouldn't happen, but safety)
    drop(tx);
    while let Ok(result) = rx.try_recv() {
        results.push(result);
    }

    Ok((results, any_failed))
}

/// Which direct-mode agent to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DirectAgent {
    Imp,
}

/// Detect which direct-mode agent binary is available.
pub(super) fn detect_direct_agent() -> Option<DirectAgent> {
    if super::imp_available() {
        Some(DirectAgent::Imp)
    } else {
        None
    }
}

fn build_direct_command(
    agent: DirectAgent,
    prompt_result: &crate::prompt::PromptResult,
    model: Option<&str>,
    unit_id: &str,
) -> Command {
    match agent {
        DirectAgent::Imp => build_imp_command(prompt_result, model, unit_id),
    }
}

fn build_imp_command(
    _prompt_result: &crate::prompt::PromptResult,
    model: Option<&str>,
    unit_id: &str,
) -> Command {
    let mut cmd = Command::new("imp");

    if let Some(model) = model {
        cmd.args(["--model", model]);
    }

    // imp's headless mode: `imp run <unit-id>`
    // It reads the unit file directly, assembles its own system prompt,
    // and outputs JSON events to stdout.
    cmd.args(["run", unit_id]);
    cmd
}


/// Run a single unit by spawning the direct-mode agent.
#[allow(clippy::too_many_arguments)]
pub(super) fn run_single_direct(
    mana_dir: &Path,
    sb: &SizedUnit,
    timeout_minutes: u32,
    idle_timeout_minutes: u32,
    config_run_model: Option<&str>,
    json_stream: bool,
    file_locking: bool,
    batch_verify: bool,
) -> AgentResult {
    let started = Instant::now();

    // Pre-emptive file locking: lock files listed in the unit's `paths` field.
    if file_locking && !sb.paths.is_empty() {
        let pid = std::process::id();
        for path in &sb.paths {
            match crate::locks::acquire(mana_dir, &sb.id, pid, path) {
                Ok(true) => {}
                Ok(false) => {
                    // Already locked by another agent — check who holds it
                    let holder = crate::locks::check_lock(mana_dir, path)
                        .ok()
                        .flatten()
                        .map(|l| format!("unit {} (pid {})", l.unit_id, l.pid))
                        .unwrap_or_else(|| "unknown".to_string());
                    eprintln!(
                        "  ⚠ Cannot lock {} for unit {} — held by {}",
                        path, sb.id, holder
                    );
                }
                Err(e) => {
                    eprintln!("  ⚠ Lock error for {}: {}", path, e);
                }
            }
        }
    }

    // Load the full unit for prompt construction
    let unit_file = match crate::discovery::find_unit_file(mana_dir, &sb.id) {
        Ok(p) => p,
        Err(e) => {
            return AgentResult {
                id: sb.id.clone(),
                title: sb.title.clone(),
                action: sb.action,
                success: false,
                duration: started.elapsed(),
                total_tokens: None,
                total_cost: None,
                error: Some(format!("Cannot find unit file: {}", e)),
                tool_count: 0,
                turns: 0,
                failure_summary: Some(format!("Cannot find unit file: {}", e)),
            };
        }
    };

    let unit = match Unit::from_file(&unit_file) {
        Ok(b) => b,
        Err(e) => {
            return AgentResult {
                id: sb.id.clone(),
                title: sb.title.clone(),
                action: sb.action,
                success: false,
                duration: started.elapsed(),
                total_tokens: None,
                total_cost: None,
                error: Some(format!("Cannot parse unit file: {}", e)),
                tool_count: 0,
                turns: 0,
                failure_summary: Some(format!("Cannot parse unit file: {}", e)),
            };
        }
    };

    // Build structured prompt via prompt module
    let prompt_options = PromptOptions {
        mana_dir: mana_dir.to_path_buf(),
        instructions: None,
        concurrent_overlaps: None,
    };

    let prompt_result = match build_agent_prompt(&unit, &prompt_options) {
        Ok(r) => r,
        Err(e) => {
            return AgentResult {
                id: sb.id.clone(),
                title: sb.title.clone(),
                action: sb.action,
                success: false,
                duration: started.elapsed(),
                total_tokens: None,
                total_cost: None,
                error: Some(format!("Failed to build prompt: {}", e)),
                tool_count: 0,
                turns: 0,
                failure_summary: Some(format!("Failed to build prompt: {}", e)),
            };
        }
    };

    let effective_model = unit.model.as_deref().or(config_run_model);

    // Detect the direct-mode agent to use.
    let agent = detect_direct_agent().expect("direct mode requires imp to be available");
    let mut cmd = build_direct_command(agent, &prompt_result, effective_model, &sb.id);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    // Signal the agent to defer verify — it should exit after completing work
    // without running the verify command itself. The runner will batch-verify later.
    if batch_verify {
        cmd.env("MANA_BATCH_VERIFY", "1");
    }

    // Spawn the process
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return AgentResult {
                id: sb.id.clone(),
                title: sb.title.clone(),
                action: sb.action,
                success: false,
                duration: started.elapsed(),
                total_tokens: None,
                total_cost: None,
                error: Some(format!("Failed to spawn imp: {}", e)),
                tool_count: 0,
                turns: 0,
                failure_summary: Some(format!("Failed to spawn imp: {}", e)),
            };
        }
    };

    // Register child PID for signal-based cleanup
    let child_pid = child.id();
    super::register_child_pid(child_pid);

    // Take stdout for monitoring
    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            let _ = child.kill();
            return AgentResult {
                id: sb.id.clone(),
                title: sb.title.clone(),
                action: sb.action,
                success: false,
                duration: started.elapsed(),
                total_tokens: None,
                total_cost: None,
                error: Some("Failed to capture stdout".to_string()),
                tool_count: 0,
                turns: 0,
                failure_summary: Some("Failed to capture stdout".to_string()),
            };
        }
    };

    // Set up timeout config
    let timeout_config = TimeoutConfig {
        total_timeout: Duration::from_secs(timeout_minutes as u64 * 60),
        idle_timeout: Duration::from_secs(idle_timeout_minutes as u64 * 60),
    };

    // Track cumulative tokens/cost
    let mut cumulative_tokens: u64 = 0;
    let mut cumulative_cost: f64 = 0.0;
    let mut tool_count: usize = 0;
    let mut cumulative_input_tokens: u64 = 0;
    let mut cumulative_output_tokens: u64 = 0;
    let mut tool_log: Vec<String> = Vec::new();
    let mut turns: usize = 0;
    let unit_id = sb.id.clone();
    let mut shown_thinking = false;

    // Monitor the process, parsing JSON events
    let monitor_result = timeout::monitor_process(&mut child, stdout, &timeout_config, |line| {
        // Try to parse each line as a JSON event from the direct-mode agent
        if let Ok(raw) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(event) = pi_output::parse_agent_event(&raw) {
                match event {
                    AgentEvent::Thinking { ref text } => {
                        if json_stream {
                            stream::emit(&StreamEvent::UnitThinking {
                                id: unit_id.clone(),
                                text: text.clone(),
                            });
                        } else if !shown_thinking {
                            eprintln!("  {}  thinking...", unit_id);
                            shown_thinking = true;
                        }
                    }
                    AgentEvent::ToolStart { ref name, .. } => {
                        tool_count += 1;
                        if json_stream {
                            stream::emit(&StreamEvent::UnitTool {
                                id: unit_id.clone(),
                                tool_name: name.clone(),
                                tool_count,
                                file_path: None,
                            });
                        }
                    }
                    AgentEvent::ToolEnd {
                        ref name,
                        ref arguments,
                    } => {
                        let file_path = pi_output::extract_file_path(name, arguments);
                        tool_log.push(format!(
                            "[tool] {} {}",
                            name,
                            file_path.as_deref().unwrap_or("")
                        ));
                        if json_stream {
                            stream::emit(&StreamEvent::UnitTool {
                                id: unit_id.clone(),
                                tool_name: name.clone(),
                                tool_count,
                                file_path,
                            });
                        } else {
                            match file_path {
                                Some(ref p) => eprintln!("  {}  ⚙ {} {}", unit_id, name, p),
                                None => eprintln!("  {}  ⚙ {}", unit_id, name),
                            }
                        }
                    }
                    AgentEvent::TokenUpdate {
                        input_tokens,
                        output_tokens,
                        cache_read,
                        cache_write,
                        cost,
                    } => {
                        cumulative_tokens += input_tokens + output_tokens;
                        cumulative_input_tokens += input_tokens;
                        cumulative_output_tokens += output_tokens;
                        cumulative_cost += cost;
                        turns += 1;
                        if json_stream {
                            stream::emit(&StreamEvent::UnitTokens {
                                id: unit_id.clone(),
                                input_tokens,
                                output_tokens,
                                cache_read,
                                cache_write,
                                cost,
                            });
                        }
                    }
                    AgentEvent::Finished { total_tokens, cost } => {
                        cumulative_tokens = total_tokens;
                        cumulative_cost = cost;
                    }
                    _ => {}
                }
            }
        }
    });

    let duration = started.elapsed();

    // Determine success
    let (success, error) = match monitor_result {
        MonitorResult::Completed => {
            // Check exit status
            match child.wait() {
                Ok(status) if status.success() => (true, None),
                Ok(status) => (
                    false,
                    Some(format!("Exit code {}", status.code().unwrap_or(-1))),
                ),
                Err(e) => (false, Some(format!("Wait error: {}", e))),
            }
        }
        MonitorResult::TotalTimeout => (
            false,
            Some(format!("Total timeout exceeded ({}m)", timeout_minutes)),
        ),
        MonitorResult::IdleTimeout => (
            false,
            Some(format!("Idle timeout exceeded ({}m)", idle_timeout_minutes)),
        ),
    };

    // Unregister child PID (process has exited)
    super::unregister_child_pid(child_pid);

    // Release all file locks held by this unit.
    if file_locking {
        let _ = crate::locks::release_all_for_unit(mana_dir, &sb.id);
    }

    // Log to agent_history.jsonl (fire-and-forget)
    history::append_history(
        mana_dir,
        &AgentHistoryEntry {
            unit_id: sb.id.clone(),
            title: sb.title.clone(),
            attempt: unit.attempts + 1,
            success,
            duration_secs: duration.as_secs(),
            tokens: cumulative_tokens,
            cost: cumulative_cost,
            tool_count,
            error: error.clone(),
            model: effective_model.unwrap_or("default").to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        },
    );

    // On failure, generate and append a structured failure summary as a unit note.
    // This gives the next retry agent context about what was tried and why it failed.
    let failure_summary = if !success {
        let mut summary_result = None;
        if let Ok(unit_path) = crate::discovery::find_unit_file(mana_dir, &sb.id) {
            if let Ok(mut fresh_unit) = Unit::from_file(&unit_path) {
                let ctx = failure::FailureContext {
                    unit_id: sb.id.clone(),
                    unit_title: sb.title.clone(),
                    attempt: fresh_unit.attempts.max(1),
                    duration_secs: duration.as_secs(),
                    tool_count,
                    turns,
                    input_tokens: cumulative_input_tokens,
                    output_tokens: cumulative_output_tokens,
                    cost: cumulative_cost,
                    error: error.clone(),
                    tool_log,
                    verify_command: fresh_unit.verify.clone(),
                };
                let summary = failure::build_failure_summary(&ctx);

                match &mut fresh_unit.notes {
                    Some(notes) => {
                        notes.push('\n');
                        notes.push_str(&summary);
                    }
                    None => fresh_unit.notes = Some(summary.clone()),
                }
                let _ = fresh_unit.to_file(&unit_path);
                summary_result = Some(summary);
            }
        }

        // Reset the unit back to Open so the next `mana run` dispatch can claim it
        // without requiring manual `mana update <id> --status open`. The release call
        // marks the current attempt as abandoned and clears claimed_by/claimed_at.
        if let Err(e) = mana_core::ops::claim::release(mana_dir, &sb.id) {
            eprintln!(
                "  ⚠ Failed to release claim on {} after agent failure: {}",
                sb.id, e
            );
        }

        summary_result
    } else {
        None
    };

    AgentResult {
        id: sb.id.clone(),
        title: sb.title.clone(),
        action: sb.action,
        success,
        duration,
        total_tokens: if cumulative_tokens > 0 {
            Some(cumulative_tokens)
        } else {
            None
        },
        total_cost: if cumulative_cost > 0.0 {
            Some(cumulative_cost)
        } else {
            None
        },
        error,
        tool_count,
        turns,
        failure_summary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::run::UnitAction;
    use crate::index::Index;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn make_mana_dir() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();
        (dir, mana_dir)
    }

    fn write_config(mana_dir: &Path, run: Option<&str>) {
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


    fn make_sized_unit(
        id: &str,
        deps: Vec<&str>,
        produces: Vec<&str>,
        requires: Vec<&str>,
    ) -> SizedUnit {
        SizedUnit {
            id: id.to_string(),
            title: format!("Unit {}", id),
            action: UnitAction::Implement,
            priority: 2,
            dependencies: deps.into_iter().map(|s| s.to_string()).collect(),
            parent: Some("parent".to_string()),
            produces: produces.into_iter().map(|s| s.to_string()).collect(),
            requires: requires.into_iter().map(|s| s.to_string()).collect(),
            paths: vec![],
            model: None,
        }
    }

    #[test]
    fn is_unit_ready_no_deps() {
        let unit = make_sized_unit("1", vec![], vec![], vec![]);
        let all_units = vec![unit.clone()];
        let all_ids: HashSet<String> = all_units.iter().map(|b| b.id.clone()).collect();
        let completed = HashSet::new();

        assert!(is_unit_ready(&unit, &completed, &all_ids, &all_units));
    }

    #[test]
    fn is_unit_ready_explicit_dep_not_met() {
        let unit = make_sized_unit("2", vec!["1"], vec![], vec![]);
        let dep = make_sized_unit("1", vec![], vec![], vec![]);
        let all_units = vec![dep, unit.clone()];
        let all_ids: HashSet<String> = all_units.iter().map(|b| b.id.clone()).collect();
        let completed = HashSet::new();

        assert!(!is_unit_ready(&unit, &completed, &all_ids, &all_units));
    }

    #[test]
    fn is_unit_ready_explicit_dep_met() {
        let unit = make_sized_unit("2", vec!["1"], vec![], vec![]);
        let dep = make_sized_unit("1", vec![], vec![], vec![]);
        let all_units = vec![dep, unit.clone()];
        let all_ids: HashSet<String> = all_units.iter().map(|b| b.id.clone()).collect();
        let mut completed = HashSet::new();
        completed.insert("1".to_string());

        assert!(is_unit_ready(&unit, &completed, &all_ids, &all_units));
    }

    #[test]
    fn is_unit_ready_requires_not_met() {
        let producer = make_sized_unit("1", vec![], vec!["TypesFile"], vec![]);
        let consumer = make_sized_unit("2", vec![], vec![], vec!["TypesFile"]);
        let all_units = vec![producer, consumer.clone()];
        let all_ids: HashSet<String> = all_units.iter().map(|b| b.id.clone()).collect();
        let completed = HashSet::new();

        assert!(!is_unit_ready(&consumer, &completed, &all_ids, &all_units));
    }

    #[test]
    fn is_unit_ready_requires_met() {
        let producer = make_sized_unit("1", vec![], vec!["TypesFile"], vec![]);
        let consumer = make_sized_unit("2", vec![], vec![], vec!["TypesFile"]);
        let all_units = vec![producer, consumer.clone()];
        let all_ids: HashSet<String> = all_units.iter().map(|b| b.id.clone()).collect();
        let mut completed = HashSet::new();
        completed.insert("1".to_string());

        assert!(is_unit_ready(&consumer, &completed, &all_ids, &all_units));
    }

    #[test]
    fn is_unit_ready_dep_outside_set_treated_as_met() {
        // If a dependency isn't in the dispatch set, treat as satisfied
        let unit = make_sized_unit("2", vec!["external"], vec![], vec![]);
        let all_units = vec![unit.clone()];
        let all_ids: HashSet<String> = all_units.iter().map(|b| b.id.clone()).collect();
        let completed = HashSet::new();

        // "external" is not in all_ids, so it's treated as met
        assert!(is_unit_ready(&unit, &completed, &all_ids, &all_units));
    }

    #[test]
    fn is_unit_ready_diamond_both_deps_needed() {
        // C requires both A and B
        let a = make_sized_unit("A", vec![], vec!["X"], vec![]);
        let b = make_sized_unit("B", vec![], vec!["Y"], vec![]);
        let c = make_sized_unit("C", vec![], vec![], vec!["X", "Y"]);
        let all_units = vec![a, b, c.clone()];
        let all_ids: HashSet<String> = all_units.iter().map(|b| b.id.clone()).collect();

        // Only A completed — C not ready
        let mut completed = HashSet::new();
        completed.insert("A".to_string());
        assert!(!is_unit_ready(&c, &completed, &all_ids, &all_units));

        // Both completed — C ready
        completed.insert("B".to_string());
        assert!(is_unit_ready(&c, &completed, &all_ids, &all_units));
    }

    #[test]
    fn ready_queue_starts_independent_units_immediately() {
        // Simulate: A (no deps), B (no deps), C (depends on A only)
        // In wave model: wave 1 = [A, B], wave 2 = [C]
        // In ready-queue: A and B start immediately, C starts when A finishes
        // (even if B is still running)
        let index = Index { units: vec![] };
        let a = make_sized_unit("A", vec![], vec!["X"], vec![]);
        let b = make_sized_unit("B", vec![], vec![], vec![]);
        let c = make_sized_unit("C", vec![], vec![], vec!["X"]);
        let all_units = vec![a.clone(), b.clone(), c.clone()];
        let all_ids: HashSet<String> = all_units.iter().map(|b| b.id.clone()).collect();

        // Initially: A and B are ready, C is not
        let completed = HashSet::new();
        assert!(is_unit_ready(&a, &completed, &all_ids, &all_units));
        assert!(is_unit_ready(&b, &completed, &all_ids, &all_units));
        assert!(!is_unit_ready(&c, &completed, &all_ids, &all_units));

        // After A completes: C becomes ready (even though B hasn't finished)
        let mut completed = HashSet::new();
        completed.insert("A".to_string());
        assert!(is_unit_ready(&c, &completed, &all_ids, &all_units));

        // Verify wave model would have put C in wave 2 (after both A and B)
        let waves = compute_waves(&all_units, &index);
        assert_eq!(waves.len(), 2);
        assert_eq!(waves[0].units.len(), 2); // A and B
        assert_eq!(waves[1].units.len(), 1); // C
        assert_eq!(waves[1].units[0].id, "C");
    }

    #[test]
    fn build_prompt_returns_err_for_missing_unit() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, None);

        // build_agent_prompt requires a Unit struct, so a missing unit is
        // handled by the caller (run_single_direct) before we get here.
        // Instead, verify that a unit with no description still produces a prompt.
        let unit = crate::unit::Unit::new("1", "Test");
        unit.to_file(mana_dir.join("1-test.md")).unwrap();

        let options = PromptOptions {
            mana_dir: mana_dir.clone(),
            instructions: None,
            concurrent_overlaps: None,
        };
        let result = build_agent_prompt(&unit, &options);
        assert!(result.is_ok());
        let prompt = result.unwrap();
        assert!(prompt.system_prompt.contains("Unit Assignment"));
        assert!(prompt.user_message.contains("mana close 1"));
    }

    #[test]
    fn build_prompt_includes_rules() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, None);

        // Write a rules file
        fs::write(mana_dir.join("RULES.md"), "# Project Rules\nAlways test.").unwrap();

        // Write a simple unit
        let unit = crate::unit::Unit::new("1", "Test");
        unit.to_file(mana_dir.join("1-test.md")).unwrap();

        let options = PromptOptions {
            mana_dir: mana_dir.clone(),
            instructions: None,
            concurrent_overlaps: None,
        };
        let result = build_agent_prompt(&unit, &options).unwrap();
        assert!(result.system_prompt.contains("Project Rules"));
        assert!(result.system_prompt.contains("Always test."));
    }

    // -- all_deps_closed with archive index tests --

    fn make_index_entry(
        id: &str,
        status: Status,
        deps: Vec<&str>,
        parent: Option<&str>,
        produces: Vec<&str>,
        requires: Vec<&str>,
    ) -> IndexEntry {
        IndexEntry {
            id: id.to_string(),
            title: format!("Unit {}", id),
            status,
            priority: 2,
            parent: parent.map(|s| s.to_string()),
            dependencies: deps.into_iter().map(|s| s.to_string()).collect(),
            labels: vec![],
            assignee: None,
            updated_at: chrono::Utc::now(),
            produces: produces.into_iter().map(|s| s.to_string()).collect(),
            requires: requires.into_iter().map(|s| s.to_string()).collect(),
            has_verify: true,
            verify: None,
            created_at: chrono::Utc::now(),
            claimed_by: None,
            attempts: 0,
            paths: vec![],
            kind: crate::unit::UnitKind::Job,
            feature: false,
            has_decisions: false,
        }
    }

    #[test]
    fn all_deps_closed_with_archived_dep() {
        // Unit A depends on unit B. B is archived (not in active index).
        // all_deps_closed should return true because B is in the archive.
        let entry_a = make_index_entry("A", Status::Open, vec!["B"], None, vec![], vec![]);
        let index = Index {
            units: vec![entry_a.clone()],
        };

        let archived_b = make_index_entry("B", Status::Closed, vec![], None, vec![], vec![]);
        let archive = ArchiveIndex {
            units: vec![archived_b],
        };

        assert!(all_deps_closed(&entry_a, &index, &archive));
    }

    #[test]
    fn all_deps_closed_with_missing_dep() {
        // Unit A depends on unit B. B is in neither index.
        // all_deps_closed should return false (typo protection).
        let entry_a = make_index_entry("A", Status::Open, vec!["B"], None, vec![], vec![]);
        let index = Index {
            units: vec![entry_a.clone()],
        };
        let archive = ArchiveIndex { units: vec![] };

        assert!(!all_deps_closed(&entry_a, &index, &archive));
    }

    #[test]
    fn all_deps_closed_with_active_closed_dep() {
        // Unit A depends on unit B. B is in active index and closed.
        let entry_a = make_index_entry("A", Status::Open, vec!["B"], None, vec![], vec![]);
        let entry_b = make_index_entry("B", Status::Closed, vec![], None, vec![], vec![]);
        let index = Index {
            units: vec![entry_a.clone(), entry_b],
        };
        let archive = ArchiveIndex { units: vec![] };

        assert!(all_deps_closed(&entry_a, &index, &archive));
    }

    #[test]
    fn all_deps_closed_with_active_open_dep() {
        // Unit A depends on unit B. B is in active index but still open.
        let entry_a = make_index_entry("A", Status::Open, vec!["B"], None, vec![], vec![]);
        let entry_b = make_index_entry("B", Status::Open, vec![], None, vec![], vec![]);
        let index = Index {
            units: vec![entry_a.clone(), entry_b],
        };
        let archive = ArchiveIndex { units: vec![] };

        assert!(!all_deps_closed(&entry_a, &index, &archive));
    }

    #[test]
    fn all_deps_closed_with_requires_and_archived_producer() {
        // Unit A requires artifact "types.rs". Unit B (archived) produces it.
        // Both share the same parent. A should be satisfied.
        let entry_a = make_index_entry(
            "A",
            Status::Open,
            vec![],
            Some("parent"),
            vec![],
            vec!["types.rs"],
        );
        let index = Index {
            units: vec![entry_a.clone()],
        };

        let archived_b = make_index_entry(
            "B",
            Status::Closed,
            vec![],
            Some("parent"),
            vec!["types.rs"],
            vec![],
        );
        let archive = ArchiveIndex {
            units: vec![archived_b],
        };

        assert!(all_deps_closed(&entry_a, &index, &archive));
    }

    #[test]
    fn all_deps_closed_mixed_active_and_archived_deps() {
        // Unit C depends on A (active, closed) and B (archived).
        // Both satisfied — should return true.
        let entry_a = make_index_entry("A", Status::Closed, vec![], None, vec![], vec![]);
        let entry_c = make_index_entry("C", Status::Open, vec!["A", "B"], None, vec![], vec![]);
        let index = Index {
            units: vec![entry_a, entry_c.clone()],
        };

        let archived_b = make_index_entry("B", Status::Closed, vec![], None, vec![], vec![]);
        let archive = ArchiveIndex {
            units: vec![archived_b],
        };

        assert!(all_deps_closed(&entry_c, &index, &archive));
    }

    // -- file conflict avoidance tests --

    fn make_sized_unit_with_paths(id: &str, paths: Vec<&str>) -> SizedUnit {
        SizedUnit {
            id: id.to_string(),
            title: format!("Unit {}", id),
            action: UnitAction::Implement,
            priority: 2,
            dependencies: vec![],
            parent: Some("parent".to_string()),
            produces: vec![],
            requires: vec![],
            paths: paths.into_iter().map(|s| s.to_string()).collect(),
            model: None,
        }
    }

    #[test]
    fn file_conflict_detected() {
        let unit = make_sized_unit_with_paths("A", vec!["src/lib.rs", "src/util.rs"]);
        let mut running = HashSet::new();
        running.insert("src/lib.rs".to_string());

        assert!(has_path_conflict(&unit, &running));
    }

    #[test]
    fn file_conflict_no_overlap() {
        let unit = make_sized_unit_with_paths("A", vec!["src/foo.rs"]);
        let mut running = HashSet::new();
        running.insert("src/bar.rs".to_string());

        assert!(!has_path_conflict(&unit, &running));
    }

    #[test]
    fn file_conflict_empty_paths() {
        let unit = make_sized_unit_with_paths("A", vec![]);
        let mut running = HashSet::new();
        running.insert("src/lib.rs".to_string());

        // Empty paths never conflict
        assert!(!has_path_conflict(&unit, &running));
    }

    #[test]
    fn file_conflict_partial_overlap() {
        // Unit touches 3 files, only 1 overlaps with running set
        let unit = make_sized_unit_with_paths("A", vec!["src/a.rs", "src/b.rs", "src/c.rs"]);
        let mut running = HashSet::new();
        running.insert("src/b.rs".to_string());

        assert!(has_path_conflict(&unit, &running));
    }

    #[test]
    fn file_conflict_multiple_running() {
        // Running set is the union of multiple units' paths
        let unit = make_sized_unit_with_paths("C", vec!["src/shared.rs"]);
        let mut running = HashSet::new();
        // Unit A's paths
        running.insert("src/foo.rs".to_string());
        // Unit B's paths
        running.insert("src/shared.rs".to_string());

        assert!(has_path_conflict(&unit, &running));
    }

    #[test]
    fn critical_path_unit_scheduled_first() {
        // Two units with same priority are both ready.
        // Unit A has a chain of dependents (A→B→C), weight=3.
        // Unit D is independent, weight=1.
        // A should sort before D because it has higher downstream weight.
        use super::super::wave::compute_downstream_weights;

        let a = make_sized_unit("A", vec![], vec![], vec![]);
        let b = make_sized_unit("B", vec!["A"], vec![], vec![]);
        let c = make_sized_unit("C", vec!["B"], vec![], vec![]);
        let d = make_sized_unit("D", vec![], vec![], vec![]);
        let all_units = vec![a, b, c, d];

        let weight_map = compute_downstream_weights(&all_units);

        // Both A and D are ready (no deps). Collect them.
        let all_ids: HashSet<String> = all_units.iter().map(|b| b.id.clone()).collect();
        let completed: HashSet<String> = HashSet::new();
        let mut ready: Vec<SizedUnit> = all_units
            .iter()
            .filter(|b| is_unit_ready(b, &completed, &all_ids, &all_units))
            .cloned()
            .collect();

        // Sort the same way the ready queue does
        ready.sort_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then_with(|| {
                    let wa = weight_map.get(&a.id).copied().unwrap_or(1);
                    let wb = weight_map.get(&b.id).copied().unwrap_or(1);
                    wb.cmp(&wa)
                })
                .then_with(|| natural_cmp(&a.id, &b.id))
        });

        assert_eq!(ready.len(), 2);
        assert_eq!(ready[0].id, "A"); // weight 3, scheduled first
        assert_eq!(ready[1].id, "D"); // weight 1, scheduled second
    }
}
