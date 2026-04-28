use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::index::Index;
use crate::stream::{self, StreamEvent};
use crate::unit::Status;
use crate::util::natural_cmp;

use super::plan::SizedUnit;
use super::ready_queue::run_single_direct;
use super::{AgentResult, SpawnMode, UnitAction};

/// A wave of units that can be dispatched in parallel.
pub struct Wave {
    pub units: Vec<SizedUnit>,
}

/// Compute waves of units grouped by dependency order.
/// Wave 0: no deps. Wave 1: deps all in wave 0. Etc.
pub(super) fn compute_waves(units: &[SizedUnit], index: &Index) -> Vec<Wave> {
    let mut waves = Vec::new();
    let unit_ids: HashSet<String> = units.iter().map(|b| b.id.clone()).collect();

    // Already-closed units count as completed
    let mut completed: HashSet<String> = index
        .units
        .iter()
        .filter(|e| e.status == Status::Closed)
        .map(|e| e.id.clone())
        .collect();

    let mut remaining: Vec<SizedUnit> = units.to_vec();

    while !remaining.is_empty() {
        let (ready, blocked): (Vec<SizedUnit>, Vec<SizedUnit>) =
            remaining.into_iter().partition(|b| {
                // All explicit deps must be completed or not in our dispatch set
                let explicit_ok = b
                    .dependencies
                    .iter()
                    .all(|d| completed.contains(d) || !unit_ids.contains(d));

                // All requires must be satisfied (producer completed or not in set)
                let requires_ok = b.requires.iter().all(|req| {
                    // Find the sibling producer for this artifact
                    if let Some(producer) = units.iter().find(|other| {
                        other.id != b.id && other.parent == b.parent && other.produces.contains(req)
                    }) {
                        completed.contains(&producer.id)
                    } else {
                        true // No producer in set, assume satisfied
                    }
                });

                explicit_ok && requires_ok
            });

        if ready.is_empty() {
            // Remaining units have unresolvable deps (cycle or missing)
            // Add them all as a final wave to avoid losing them
            eprintln!(
                "Warning: {} unit(s) have unresolvable dependencies, adding to final wave",
                blocked.len()
            );
            waves.push(Wave { units: blocked });
            break;
        }

        for b in &ready {
            completed.insert(b.id.clone());
        }

        waves.push(Wave { units: ready });
        remaining = blocked;
    }

    // Compute downstream weights for critical-path-aware sorting
    let weights = compute_downstream_weights(units);

    // Sort units within each wave: priority first, then downstream weight
    // (descending — units that block the most work get scheduled first),
    // then ID for stability.
    for wave in &mut waves {
        wave.units.sort_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then_with(|| {
                    let wa = weights.get(&a.id).copied().unwrap_or(1);
                    let wb = weights.get(&b.id).copied().unwrap_or(1);
                    wb.cmp(&wa) // higher weight first
                })
                .then_with(|| natural_cmp(&a.id, &b.id))
        });
    }

    waves
}

/// Compute downstream weight for each unit.
/// Weight = 1 + count of all transitively dependent units.
/// Units on the critical path will have the highest weights.
pub(super) fn compute_downstream_weights(units: &[SizedUnit]) -> HashMap<String, u32> {
    let unit_ids: HashSet<String> = units.iter().map(|b| b.id.clone()).collect();

    // Build reverse dependency graph: for each unit, which units directly depend on it.
    let mut reverse_deps: HashMap<String, Vec<String>> = HashMap::new();

    for b in units {
        reverse_deps.entry(b.id.clone()).or_default();

        // Explicit dependencies: b depends on dep → dep's reverse_deps includes b
        for dep in &b.dependencies {
            if unit_ids.contains(dep) {
                reverse_deps
                    .entry(dep.clone())
                    .or_default()
                    .push(b.id.clone());
            }
        }

        // Requires/produces: if b requires artifact X and producer makes X
        // (same parent), then b depends on producer.
        for req in &b.requires {
            if let Some(producer) = units.iter().find(|other| {
                other.id != b.id && other.parent == b.parent && other.produces.contains(req)
            }) {
                if unit_ids.contains(&producer.id) {
                    reverse_deps
                        .entry(producer.id.clone())
                        .or_default()
                        .push(b.id.clone());
                }
            }
        }
    }

    // For each unit, count all transitively reachable descendants via BFS.
    let mut weights: HashMap<String, u32> = HashMap::new();

    for b in units {
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: Vec<String> = Vec::new();

        // Seed with direct dependents
        for dep in reverse_deps.get(&b.id).unwrap_or(&Vec::new()) {
            if visited.insert(dep.clone()) {
                queue.push(dep.clone());
            }
        }

        // BFS to find all transitive dependents
        while let Some(current) = queue.pop() {
            for next in reverse_deps.get(&current).unwrap_or(&Vec::new()) {
                if visited.insert(next.clone()) {
                    queue.push(next.clone());
                }
            }
        }

        weights.insert(b.id.clone(), 1 + visited.len() as u32);
    }

    weights
}

/// Compute file conflict groups: files touched by more than one unit.
/// Returns pairs of (file_path, vec_of_unit_ids).
pub(super) fn compute_file_conflicts(units: &[SizedUnit]) -> Vec<(String, Vec<String>)> {
    let mut file_to_units: HashMap<String, Vec<String>> = HashMap::new();
    for b in units {
        for p in &b.paths {
            file_to_units
                .entry(p.clone())
                .or_default()
                .push(b.id.clone());
        }
    }
    let mut conflicts: Vec<(String, Vec<String>)> = file_to_units
        .into_iter()
        .filter(|(_, ids)| ids.len() > 1)
        .collect();
    conflicts.sort_by(|a, b| a.0.cmp(&b.0));
    conflicts
}

/// Compute effective parallelism: max units that can run simultaneously
/// without file path conflicts. Uses greedy selection.
pub(super) fn compute_effective_parallelism(units: &[SizedUnit]) -> usize {
    if units.is_empty() {
        return 0;
    }
    let mut occupied: HashSet<String> = HashSet::new();
    let mut count = 0;
    for b in units {
        if b.paths.is_empty() || !b.paths.iter().any(|p| occupied.contains(p)) {
            for p in &b.paths {
                occupied.insert(p.clone());
            }
            count += 1;
        }
    }
    count
}

/// Find the critical path through the dependency graph.
/// Returns the longest chain of unit IDs from root to leaf.
pub(super) fn compute_critical_path(units: &[SizedUnit]) -> Vec<String> {
    if units.is_empty() {
        return vec![];
    }

    let weights = compute_downstream_weights(units);
    let unit_ids: HashSet<String> = units.iter().map(|b| b.id.clone()).collect();

    // Build forward dependency map: unit_id → units that depend on it
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
    for b in units {
        for dep in &b.dependencies {
            if unit_ids.contains(dep) {
                dependents
                    .entry(dep.clone())
                    .or_default()
                    .push(b.id.clone());
            }
        }
        for req in &b.requires {
            if let Some(producer) = units.iter().find(|other| {
                other.id != b.id && other.parent == b.parent && other.produces.contains(req)
            }) {
                if unit_ids.contains(&producer.id) {
                    dependents
                        .entry(producer.id.clone())
                        .or_default()
                        .push(b.id.clone());
                }
            }
        }
    }

    // Start from unit with highest weight
    let start = units
        .iter()
        .max_by(|a, b| {
            let wa = weights.get(&a.id).copied().unwrap_or(0);
            let wb = weights.get(&b.id).copied().unwrap_or(0);
            wa.cmp(&wb).then_with(|| natural_cmp(&b.id, &a.id))
        })
        .unwrap();

    let mut path = vec![start.id.clone()];
    let mut current = start.id.clone();

    // Follow the dependent with highest weight (greedy critical path)
    loop {
        let Some(deps) = dependents.get(&current) else {
            break;
        };
        if deps.is_empty() {
            break;
        }
        // Sort dependents: highest weight first, then natural ID for stability
        let mut deps_sorted = deps.clone();
        deps_sorted.sort_by(|a, b| {
            let wa = weights.get(a).copied().unwrap_or(0);
            let wb = weights.get(b).copied().unwrap_or(0);
            wb.cmp(&wa).then_with(|| natural_cmp(a, b))
        });
        let next = &deps_sorted[0];
        path.push(next.clone());
        current = next.clone();
    }

    path
}

// ---------------------------------------------------------------------------
// Wave execution
// ---------------------------------------------------------------------------

/// Spawn agents for a wave of units, respecting max parallelism.
pub(super) fn run_wave(
    mana_dir: &Path,
    units: &[SizedUnit],
    spawn_mode: &SpawnMode,
    cfg: &super::RunConfig,
    wave_number: usize,
) -> Result<Vec<AgentResult>> {
    match spawn_mode {
        SpawnMode::Template {
            run_template,
            plan_template,
        } => run_wave_template(
            units,
            run_template,
            plan_template.as_deref(),
            cfg.max_jobs,
            cfg.timeout_minutes,
            cfg.idle_timeout_minutes,
            cfg.run_model.as_deref(),
            cfg.json_stream,
        ),
        SpawnMode::Direct => run_wave_direct(
            mana_dir,
            units,
            cfg.max_jobs,
            cfg.timeout_minutes,
            cfg.idle_timeout_minutes,
            cfg.run_model.as_deref(),
            cfg.json_stream,
            wave_number,
            cfg.file_locking,
        ),
    }
}

/// Template mode: spawn agents via `sh -c <template>` (backward compat).
#[allow(clippy::too_many_arguments)]
fn run_wave_template(
    units: &[SizedUnit],
    run_template: &str,
    _plan_template: Option<&str>,
    max_jobs: usize,
    timeout_minutes: u32,
    idle_timeout_minutes: u32,
    config_run_model: Option<&str>,
    json_stream: bool,
) -> Result<Vec<AgentResult>> {
    let total_timeout = Duration::from_secs(timeout_minutes as u64 * 60);
    let _idle_timeout = Duration::from_secs(idle_timeout_minutes as u64 * 60);
    // Note: idle timeout based on stdout activity is only enforced in Direct mode
    // (via timeout::monitor_process). Template mode enforces total timeout only.

    let mut results = Vec::new();
    // Track: unit, child process, start time, last stdout activity time
    let mut children: Vec<(SizedUnit, std::process::Child, Instant, Instant)> = Vec::new();

    let mut pending: Vec<&SizedUnit> = units.iter().collect();

    while !pending.is_empty() || !children.is_empty() {
        // Check for shutdown signal
        if super::shutdown_requested() {
            for (sb, mut child, started, _) in children {
                let _ = child.kill();
                let _ = child.wait();
                results.push(AgentResult {
                    id: sb.id.clone(),
                    title: sb.title.clone(),
                    action: sb.action,
                    success: false,
                    duration: started.elapsed(),
                    total_tokens: None,
                    total_cost: None,
                    error: Some("Interrupted by shutdown signal".to_string()),
                    tool_count: 0,
                    turns: 0,
                    failure_summary: None,
                });
            }
            return Ok(results);
        }

        // Spawn up to max_jobs
        while children.len() < max_jobs && !pending.is_empty() {
            let sb = pending.remove(0);
            let template = match sb.action {
                UnitAction::Implement => run_template,
            };

            // Model precedence: unit-level override > config-level > no substitution
            let effective_model = sb.model.as_deref().or(config_run_model);
            let cmd =
                crate::spawner::substitute_template_with_model(template, &sb.id, effective_model);
            let mut command = Command::new("sh");
            command.args(["-c", &cmd]);

            // When mana is acting as a machine-readable stream producer
            // (for the native imp mana tool, wizard, or any other embedded
            // consumer), child template commands must not inherit the current
            // terminal stdout/stderr. Inheriting stdio lets nested agent
            // processes dump raw text/JSON directly into the parent TUI.
            if json_stream {
                command.stdout(Stdio::null());
                command.stderr(Stdio::null());
            }

            match command.spawn() {
                Ok(child) => {
                    let now = Instant::now();
                    children.push((sb.clone(), child, now, now));
                }
                Err(e) => {
                    eprintln!("  Failed to spawn agent for {}: {}", sb.id, e);
                    results.push(AgentResult {
                        id: sb.id.clone(),
                        title: sb.title.clone(),
                        action: sb.action,
                        success: false,
                        duration: Duration::ZERO,
                        total_tokens: None,
                        total_cost: None,
                        error: Some(format!("Failed to spawn: {}", e)),
                        tool_count: 0,
                        turns: 0,
                        failure_summary: None,
                    });
                }
            }
        }

        if children.is_empty() {
            break;
        }

        // Poll for completions and enforce timeouts
        let mut still_running = Vec::new();
        for (sb, mut child, started, last_activity) in children {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let err = if status.success() {
                        None
                    } else {
                        Some(format!("Exit code {}", status.code().unwrap_or(-1)))
                    };
                    results.push(AgentResult {
                        id: sb.id.clone(),
                        title: sb.title.clone(),
                        action: sb.action,
                        success: status.success(),
                        duration: started.elapsed(),
                        total_tokens: None,
                        total_cost: None,
                        error: err,
                        tool_count: 0,
                        turns: 0,
                        failure_summary: None,
                    });
                }
                Ok(None) => {
                    // Process still running — check timeouts
                    let elapsed = started.elapsed();

                    if !total_timeout.is_zero() && elapsed > total_timeout {
                        eprintln!(
                            "  ⚠ {} total timeout ({}m) — killing",
                            sb.id, timeout_minutes
                        );
                        let _ = child.kill();
                        let _ = child.wait();
                        results.push(AgentResult {
                            id: sb.id.clone(),
                            title: sb.title.clone(),
                            action: sb.action,
                            success: false,
                            duration: elapsed,
                            total_tokens: None,
                            total_cost: None,
                            error: Some(format!("Total timeout exceeded ({}m)", timeout_minutes)),
                            tool_count: 0,
                            turns: 0,
                            failure_summary: None,
                        });
                    } else {
                        still_running.push((sb, child, started, last_activity));
                    }
                }
                Err(e) => {
                    eprintln!("  Error checking agent for {}: {}", sb.id, e);
                    results.push(AgentResult {
                        id: sb.id.clone(),
                        title: sb.title.clone(),
                        action: sb.action,
                        success: false,
                        duration: started.elapsed(),
                        total_tokens: None,
                        total_cost: None,
                        error: Some(format!("Error checking process: {}", e)),
                        tool_count: 0,
                        turns: 0,
                        failure_summary: None,
                    });
                }
            }
        }
        children = still_running;

        if !children.is_empty() {
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    Ok(results)
}

/// Direct mode: spawn the direct-mode agent with JSON output and monitoring.
#[allow(clippy::too_many_arguments)]
fn run_wave_direct(
    mana_dir: &Path,
    units: &[SizedUnit],
    max_jobs: usize,
    timeout_minutes: u32,
    idle_timeout_minutes: u32,
    config_run_model: Option<&str>,
    json_stream: bool,
    wave_number: usize,
    file_locking: bool,
) -> Result<Vec<AgentResult>> {
    let results = Arc::new(Mutex::new(Vec::new()));
    let mut pending: Vec<SizedUnit> = units.to_vec();
    let mut handles: Vec<std::thread::JoinHandle<()>> = Vec::new();

    while !pending.is_empty() || !handles.is_empty() {
        // Check for shutdown signal
        if super::shutdown_requested() {
            super::kill_all_children();
            // Wait for threads to finish (they should exit after children are killed)
            for handle in handles {
                let _ = handle.join();
            }
            return Ok(Arc::try_unwrap(results).unwrap().into_inner().unwrap());
        }

        // Spawn up to max_jobs threads
        while handles.len() < max_jobs && !pending.is_empty() {
            let sb = pending.remove(0);
            let mana_dir = mana_dir.to_path_buf();
            let results = Arc::clone(&results);
            let timeout_min = timeout_minutes;
            let idle_min = idle_timeout_minutes;
            let config_run_model = config_run_model.map(str::to_string);

            if json_stream {
                stream::emit(&StreamEvent::UnitStart {
                    id: sb.id.clone(),
                    title: sb.title.clone(),
                    round: wave_number,
                    file_overlaps: None,
                    attempt: None,
                    priority: None,
                });
            }

            let handle = std::thread::spawn(move || {
                let result = run_single_direct(
                    &mana_dir,
                    &sb,
                    timeout_min,
                    idle_min,
                    config_run_model.as_deref(),
                    json_stream,
                    file_locking,
                    false, // batch_verify not used in template/wave mode
                );
                results.lock().unwrap().push(result);
            });
            handles.push(handle);
        }

        // Wait for at least one thread to finish
        let prev_count = handles.len();
        let mut still_running = Vec::new();
        for handle in handles.drain(..) {
            if handle.is_finished() {
                let _ = handle.join();
            } else {
                still_running.push(handle);
            }
        }

        // If nothing finished, wait briefly before polling again
        if still_running.len() == prev_count && !still_running.is_empty() {
            std::thread::sleep(Duration::from_millis(200));
        }

        handles = still_running;
    }

    // Wait for any remaining threads
    for handle in handles {
        let _ = handle.join();
    }

    Ok(Arc::try_unwrap(results).unwrap().into_inner().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::run::UnitAction;
    use crate::index::Index;

    #[test]
    fn compute_waves_no_deps() {
        let index = Index { units: vec![] };
        let units = vec![
            SizedUnit {
                id: "1".to_string(),
                title: "A".to_string(),
                action: UnitAction::Implement,
                priority: 2,
                dependencies: vec![],
                parent: None,
                produces: vec![],
                requires: vec![],
                paths: vec![],
                verify_fast: None,
                verify_command: None,
                retry: mana_pool::RetryContext {
                    attempt_number: 0,
                    previous_failure: None,
                    previous_notes: vec![],
                },
                model: None,
            },
            SizedUnit {
                id: "2".to_string(),
                title: "B".to_string(),
                action: UnitAction::Implement,
                priority: 2,
                dependencies: vec![],
                parent: None,
                produces: vec![],
                requires: vec![],
                paths: vec![],
                verify_fast: None,
                verify_command: None,
                retry: mana_pool::RetryContext {
                    attempt_number: 0,
                    previous_failure: None,
                    previous_notes: vec![],
                },
                model: None,
            },
        ];
        let waves = compute_waves(&units, &index);
        assert_eq!(waves.len(), 1);
        assert_eq!(waves[0].units.len(), 2);
    }

    #[test]
    fn compute_waves_linear_chain() {
        let index = Index { units: vec![] };
        let units = vec![
            SizedUnit {
                id: "1".to_string(),
                title: "A".to_string(),
                action: UnitAction::Implement,
                priority: 2,
                dependencies: vec![],
                parent: None,
                produces: vec![],
                requires: vec![],
                paths: vec![],
                verify_fast: None,
                verify_command: None,
                retry: mana_pool::RetryContext {
                    attempt_number: 0,
                    previous_failure: None,
                    previous_notes: vec![],
                },
                model: None,
            },
            SizedUnit {
                id: "2".to_string(),
                title: "B".to_string(),
                action: UnitAction::Implement,
                priority: 2,
                dependencies: vec!["1".to_string()],
                parent: None,
                produces: vec![],
                requires: vec![],
                paths: vec![],
                verify_fast: None,
                verify_command: None,
                retry: mana_pool::RetryContext {
                    attempt_number: 0,
                    previous_failure: None,
                    previous_notes: vec![],
                },
                model: None,
            },
            SizedUnit {
                id: "3".to_string(),
                title: "C".to_string(),
                action: UnitAction::Implement,
                priority: 2,
                dependencies: vec!["2".to_string()],
                parent: None,
                produces: vec![],
                requires: vec![],
                paths: vec![],
                verify_fast: None,
                verify_command: None,
                retry: mana_pool::RetryContext {
                    attempt_number: 0,
                    previous_failure: None,
                    previous_notes: vec![],
                },
                model: None,
            },
        ];
        let waves = compute_waves(&units, &index);
        assert_eq!(waves.len(), 3);
        assert_eq!(waves[0].units[0].id, "1");
        assert_eq!(waves[1].units[0].id, "2");
        assert_eq!(waves[2].units[0].id, "3");
    }

    #[test]
    fn compute_waves_diamond() {
        let index = Index { units: vec![] };
        // 1 → (2, 3) → 4
        let units = vec![
            SizedUnit {
                id: "1".to_string(),
                title: "Root".to_string(),
                action: UnitAction::Implement,
                priority: 2,
                dependencies: vec![],
                parent: None,
                produces: vec![],
                requires: vec![],
                paths: vec![],
                verify_fast: None,
                verify_command: None,
                retry: mana_pool::RetryContext {
                    attempt_number: 0,
                    previous_failure: None,
                    previous_notes: vec![],
                },
                model: None,
            },
            SizedUnit {
                id: "2".to_string(),
                title: "Left".to_string(),
                action: UnitAction::Implement,
                priority: 2,
                dependencies: vec!["1".to_string()],
                parent: None,
                produces: vec![],
                requires: vec![],
                paths: vec![],
                verify_fast: None,
                verify_command: None,
                retry: mana_pool::RetryContext {
                    attempt_number: 0,
                    previous_failure: None,
                    previous_notes: vec![],
                },
                model: None,
            },
            SizedUnit {
                id: "3".to_string(),
                title: "Right".to_string(),
                action: UnitAction::Implement,
                priority: 2,
                dependencies: vec!["1".to_string()],
                parent: None,
                produces: vec![],
                requires: vec![],
                paths: vec![],
                verify_fast: None,
                verify_command: None,
                retry: mana_pool::RetryContext {
                    attempt_number: 0,
                    previous_failure: None,
                    previous_notes: vec![],
                },
                model: None,
            },
            SizedUnit {
                id: "4".to_string(),
                title: "Join".to_string(),
                action: UnitAction::Implement,
                priority: 2,
                dependencies: vec!["2".to_string(), "3".to_string()],
                parent: None,
                produces: vec![],
                requires: vec![],
                paths: vec![],
                verify_fast: None,
                verify_command: None,
                retry: mana_pool::RetryContext {
                    attempt_number: 0,
                    previous_failure: None,
                    previous_notes: vec![],
                },
                model: None,
            },
        ];
        let waves = compute_waves(&units, &index);
        assert_eq!(waves.len(), 3);
        assert_eq!(waves[0].units.len(), 1); // 1
        assert_eq!(waves[1].units.len(), 2); // 2, 3
        assert_eq!(waves[2].units.len(), 1); // 4
    }

    #[test]
    fn template_wave_execution_with_echo() {
        let units = vec![SizedUnit {
            id: "1".to_string(),
            title: "Test".to_string(),
            action: UnitAction::Implement,
            priority: 2,
            dependencies: vec![],
            parent: None,
            produces: vec![],
            requires: vec![],
            paths: vec![],
            verify_fast: None,
            verify_command: None,
            retry: mana_pool::RetryContext {
                attempt_number: 0,
                previous_failure: None,
                previous_notes: vec![],
            },
            model: None,
        }];

        let results = run_wave_template(&units, "echo {id}", None, 4, 30, 5, None, false).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].success);
        assert_eq!(results[0].id, "1");
    }

    #[test]
    fn template_wave_runs_implement_action() {
        let units = vec![SizedUnit {
            id: "1".to_string(),
            title: "Test".to_string(),
            action: UnitAction::Implement,
            priority: 2,
            dependencies: vec![],
            parent: None,
            produces: vec![],
            requires: vec![],
            paths: vec![],
            verify_fast: None,
            verify_command: None,
            retry: mana_pool::RetryContext {
                attempt_number: 0,
                previous_failure: None,
                previous_notes: vec![],
            },
            model: None,
        }];

        let results = run_wave_template(&units, "echo {id}", None, 4, 30, 5, None, false).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].success);
        assert_eq!(results[0].id, "1");
    }

    #[test]
    fn template_wave_failed_command() {
        let units = vec![SizedUnit {
            id: "1".to_string(),
            title: "Fail".to_string(),
            action: UnitAction::Implement,
            priority: 2,
            dependencies: vec![],
            parent: None,
            produces: vec![],
            requires: vec![],
            paths: vec![],
            verify_fast: None,
            verify_command: None,
            retry: mana_pool::RetryContext {
                attempt_number: 0,
                previous_failure: None,
                previous_notes: vec![],
            },
            model: None,
        }];

        let results = run_wave_template(&units, "false", None, 4, 30, 5, None, false).unwrap();
        assert_eq!(results.len(), 1);
        assert!(!results[0].success);
        assert!(results[0].error.is_some());
    }

    // -- downstream weight tests --

    fn make_unit(id: &str, deps: Vec<&str>, produces: Vec<&str>, requires: Vec<&str>) -> SizedUnit {
        SizedUnit {
            id: id.to_string(),
            title: format!("Unit {}", id),
            action: UnitAction::Implement,
            priority: 2,
            dependencies: deps.into_iter().map(|s| s.to_string()).collect(),
            parent: Some("p".to_string()),
            produces: produces.into_iter().map(|s| s.to_string()).collect(),
            requires: requires.into_iter().map(|s| s.to_string()).collect(),
            paths: vec![],
            verify_fast: None,
            verify_command: None,
            retry: mana_pool::RetryContext {
                attempt_number: 0,
                previous_failure: None,
                previous_notes: vec![],
            },
            model: None,
        }
    }

    #[test]
    fn downstream_weights_single_unit() {
        let units = vec![make_unit("A", vec![], vec![], vec![])];
        let weights = compute_downstream_weights(&units);
        assert_eq!(weights.get("A").copied(), Some(1));
    }

    #[test]
    fn downstream_weights_linear_chain() {
        // A → B → C (B depends on A, C depends on B)
        let units = vec![
            make_unit("A", vec![], vec![], vec![]),
            make_unit("B", vec!["A"], vec![], vec![]),
            make_unit("C", vec!["B"], vec![], vec![]),
        ];
        let weights = compute_downstream_weights(&units);
        assert_eq!(weights.get("A").copied(), Some(3)); // blocks B and C
        assert_eq!(weights.get("B").copied(), Some(2)); // blocks C
        assert_eq!(weights.get("C").copied(), Some(1)); // leaf
    }

    #[test]
    fn downstream_weights_diamond() {
        // A → (B, C) → D
        let units = vec![
            make_unit("A", vec![], vec![], vec![]),
            make_unit("B", vec!["A"], vec![], vec![]),
            make_unit("C", vec!["A"], vec![], vec![]),
            make_unit("D", vec!["B", "C"], vec![], vec![]),
        ];
        let weights = compute_downstream_weights(&units);
        assert_eq!(weights.get("D").copied(), Some(1)); // leaf
        assert_eq!(weights.get("B").copied(), Some(2)); // blocks D
        assert_eq!(weights.get("C").copied(), Some(2)); // blocks D
        assert_eq!(weights.get("A").copied(), Some(4)); // blocks B, C, D (3 descendants + 1)
    }

    #[test]
    fn downstream_weights_independent() {
        let units = vec![
            make_unit("A", vec![], vec![], vec![]),
            make_unit("B", vec![], vec![], vec![]),
        ];
        let weights = compute_downstream_weights(&units);
        assert_eq!(weights.get("A").copied(), Some(1));
        assert_eq!(weights.get("B").copied(), Some(1));
    }

    #[test]
    fn downstream_weights_wide_fan() {
        // A → (B, C, D)
        let units = vec![
            make_unit("A", vec![], vec![], vec![]),
            make_unit("B", vec!["A"], vec![], vec![]),
            make_unit("C", vec!["A"], vec![], vec![]),
            make_unit("D", vec!["A"], vec![], vec![]),
        ];
        let weights = compute_downstream_weights(&units);
        assert_eq!(weights.get("A").copied(), Some(4)); // 1 + 1 + 1 + 1
        assert_eq!(weights.get("B").copied(), Some(1));
        assert_eq!(weights.get("C").copied(), Some(1));
        assert_eq!(weights.get("D").copied(), Some(1));
    }

    // -- wave sorting by downstream weight tests --

    #[test]
    fn compute_waves_sorts_by_downstream_weight() {
        let index = Index { units: vec![] };
        // Wave 1: A, B, C (no deps among each other, same priority)
        // D depends on A → A has weight 2
        // E and F depend on B → B has weight 3
        // C is leaf → weight 1
        let units = vec![
            make_unit("A", vec![], vec![], vec![]),
            make_unit("B", vec![], vec![], vec![]),
            make_unit("C", vec![], vec![], vec![]),
            make_unit("D", vec!["A"], vec![], vec![]),
            make_unit("E", vec!["B"], vec![], vec![]),
            make_unit("F", vec!["B"], vec![], vec![]),
        ];
        let waves = compute_waves(&units, &index);
        assert_eq!(waves.len(), 2);
        // Wave 1 sorted by weight desc: B(3), A(2), C(1)
        assert_eq!(waves[0].units[0].id, "B");
        assert_eq!(waves[0].units[1].id, "A");
        assert_eq!(waves[0].units[2].id, "C");
    }

    #[test]
    fn compute_waves_weight_sorting_preserves_priority() {
        let index = Index { units: vec![] };
        // A has priority 1, B has priority 2 — A first despite lower weight
        let mut a = make_unit("A", vec![], vec![], vec![]);
        a.priority = 1;
        let mut b = make_unit("B", vec![], vec![], vec![]);
        b.priority = 2;
        // C depends on B → B has weight 2, A has weight 1
        let c = make_unit("C", vec!["B"], vec![], vec![]);
        let units = vec![a, b, c];
        let waves = compute_waves(&units, &index);
        // Wave 1: A (pri 1) before B (pri 2), despite B having higher weight
        assert_eq!(waves[0].units[0].id, "A");
        assert_eq!(waves[0].units[1].id, "B");
    }

    // -- file conflict tests --

    fn make_unit_with_paths(id: &str, deps: Vec<&str>, paths: Vec<&str>) -> SizedUnit {
        SizedUnit {
            id: id.to_string(),
            title: format!("Unit {}", id),
            action: UnitAction::Implement,
            priority: 2,
            dependencies: deps.into_iter().map(|s| s.to_string()).collect(),
            parent: Some("p".to_string()),
            produces: vec![],
            requires: vec![],
            paths: paths.into_iter().map(|s| s.to_string()).collect(),
            verify_fast: None,
            verify_command: None,
            retry: mana_pool::RetryContext {
                attempt_number: 0,
                previous_failure: None,
                previous_notes: vec![],
            },
            model: None,
        }
    }

    #[test]
    fn file_conflicts_detected() {
        let units = vec![
            make_unit_with_paths("A", vec![], vec!["src/lib.rs", "src/a.rs"]),
            make_unit_with_paths("B", vec![], vec!["src/lib.rs", "src/b.rs"]),
            make_unit_with_paths("C", vec![], vec!["src/c.rs"]),
        ];
        let conflicts = compute_file_conflicts(&units);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].0, "src/lib.rs");
        assert!(conflicts[0].1.contains(&"A".to_string()));
        assert!(conflicts[0].1.contains(&"B".to_string()));
    }

    #[test]
    fn file_conflicts_empty_when_no_overlap() {
        let units = vec![
            make_unit_with_paths("A", vec![], vec!["src/a.rs"]),
            make_unit_with_paths("B", vec![], vec!["src/b.rs"]),
        ];
        let conflicts = compute_file_conflicts(&units);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn file_conflicts_multiple_files() {
        let units = vec![
            make_unit_with_paths("A", vec![], vec!["src/lib.rs", "src/mod.rs"]),
            make_unit_with_paths("B", vec![], vec!["src/lib.rs"]),
            make_unit_with_paths("C", vec![], vec!["src/mod.rs"]),
        ];
        let conflicts = compute_file_conflicts(&units);
        assert_eq!(conflicts.len(), 2);
        // Sorted by file path
        assert_eq!(conflicts[0].0, "src/lib.rs");
        assert_eq!(conflicts[1].0, "src/mod.rs");
    }

    // -- effective parallelism tests --

    #[test]
    fn effective_parallelism_no_conflicts() {
        let units = vec![
            make_unit_with_paths("A", vec![], vec!["src/a.rs"]),
            make_unit_with_paths("B", vec![], vec!["src/b.rs"]),
            make_unit_with_paths("C", vec![], vec!["src/c.rs"]),
        ];
        assert_eq!(compute_effective_parallelism(&units), 3);
    }

    #[test]
    fn effective_parallelism_with_conflict() {
        let units = vec![
            make_unit_with_paths("A", vec![], vec!["src/lib.rs"]),
            make_unit_with_paths("B", vec![], vec!["src/lib.rs"]),
            make_unit_with_paths("C", vec![], vec!["src/c.rs"]),
        ];
        // A takes src/lib.rs, B is blocked, C can run → 2
        assert_eq!(compute_effective_parallelism(&units), 2);
    }

    #[test]
    fn effective_parallelism_all_conflict() {
        let units = vec![
            make_unit_with_paths("A", vec![], vec!["src/shared.rs"]),
            make_unit_with_paths("B", vec![], vec!["src/shared.rs"]),
            make_unit_with_paths("C", vec![], vec!["src/shared.rs"]),
        ];
        // Only one can run at a time
        assert_eq!(compute_effective_parallelism(&units), 1);
    }

    #[test]
    fn effective_parallelism_empty_paths_no_conflict() {
        let units = vec![
            make_unit_with_paths("A", vec![], vec![]),
            make_unit_with_paths("B", vec![], vec![]),
            make_unit_with_paths("C", vec![], vec!["src/c.rs"]),
        ];
        // Empty paths never conflict
        assert_eq!(compute_effective_parallelism(&units), 3);
    }

    #[test]
    fn effective_parallelism_empty_input() {
        assert_eq!(compute_effective_parallelism(&[]), 0);
    }

    // -- critical path tests --

    #[test]
    fn critical_path_single_unit() {
        let units = vec![make_unit("A", vec![], vec![], vec![])];
        let path = compute_critical_path(&units);
        assert_eq!(path, vec!["A"]);
    }

    #[test]
    fn critical_path_linear_chain() {
        let units = vec![
            make_unit("A", vec![], vec![], vec![]),
            make_unit("B", vec!["A"], vec![], vec![]),
            make_unit("C", vec!["B"], vec![], vec![]),
        ];
        let path = compute_critical_path(&units);
        assert_eq!(path, vec!["A", "B", "C"]);
    }

    #[test]
    fn critical_path_diamond() {
        // A → (B, C) → D
        let units = vec![
            make_unit("A", vec![], vec![], vec![]),
            make_unit("B", vec!["A"], vec![], vec![]),
            make_unit("C", vec!["A"], vec![], vec![]),
            make_unit("D", vec!["B", "C"], vec![], vec![]),
        ];
        let path = compute_critical_path(&units);
        assert_eq!(path.len(), 3);
        assert_eq!(path[0], "A");
        // B and C have equal weight; tie broken by natural ID order → B
        assert_eq!(path[1], "B");
        assert_eq!(path[2], "D");
    }

    #[test]
    fn critical_path_picks_heaviest_branch() {
        // A → B → C (long branch)
        // A → D (short branch)
        // Critical path should be A → B → C
        let units = vec![
            make_unit("A", vec![], vec![], vec![]),
            make_unit("B", vec!["A"], vec![], vec![]),
            make_unit("C", vec!["B"], vec![], vec![]),
            make_unit("D", vec!["A"], vec![], vec![]),
        ];
        let path = compute_critical_path(&units);
        assert_eq!(path, vec!["A", "B", "C"]);
    }

    #[test]
    fn critical_path_independent_units() {
        // No deps — all have weight 1. Path is just the first one (by ID).
        let units = vec![
            make_unit("A", vec![], vec![], vec![]),
            make_unit("B", vec![], vec![], vec![]),
        ];
        let path = compute_critical_path(&units);
        assert_eq!(path.len(), 1);
    }
}
