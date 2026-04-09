use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{mpsc, Arc};
use std::time::Instant;

use anyhow::Result;
use mana_core::util::natural_cmp;

use crate::memory;
use crate::types::*;

pub fn group_verify_commands(units: &[DispatchUnit]) -> Vec<VerifyGroup> {
    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for unit in units {
        let Some(command) = unit.verify_command.as_ref() else {
            continue;
        };
        if command.trim().is_empty() {
            continue;
        }
        grouped
            .entry(command.clone())
            .or_default()
            .push(unit.id.clone());
    }

    grouped
        .into_iter()
        .map(|(command, mut unit_ids)| {
            unit_ids.sort_by(|a, b| natural_cmp(a, b));
            VerifyGroup { command, unit_ids }
        })
        .collect()
}

fn run_verify_groups<F>(
    groups: &[VerifyGroup],
    event_tx: &mpsc::Sender<PoolEvent>,
    mut run_group: F,
) -> HashMap<String, bool>
where
    F: FnMut(&VerifyGroup) -> bool,
{
    let mut results = HashMap::new();

    for group in groups {
        let success = run_group(group);
        let _ = event_tx.send(PoolEvent::VerifyGroupRun {
            command: group.command.clone(),
            unit_ids: group.unit_ids.clone(),
            success,
        });
        for unit_id in &group.unit_ids {
            results.insert(unit_id.clone(), success);
        }
    }

    results
}

fn drain_progress_events(
    progress_rx: &mpsc::Receiver<(String, AgentProgress)>,
    event_tx: &mpsc::Sender<PoolEvent>,
    running_last_progress: &mut HashMap<String, Instant>,
    running_stuck_emitted: &mut HashSet<String>,
) {
    while let Ok((unit_id, progress)) = progress_rx.try_recv() {
        running_last_progress.insert(unit_id.clone(), Instant::now());
        running_stuck_emitted.remove(&unit_id);
        match progress {
            AgentProgress::Progress { phase, elapsed } => {
                let _ = event_tx.send(PoolEvent::Progress {
                    unit_id,
                    phase,
                    elapsed,
                });
            }
            AgentProgress::Heartbeat { elapsed } => {
                let _ = event_tx.send(PoolEvent::Heartbeat { unit_id, elapsed });
            }
        }
    }
}

fn emit_stuck_events(
    event_tx: &mpsc::Sender<PoolEvent>,
    running_last_progress: &HashMap<String, Instant>,
    running_stuck_emitted: &mut HashSet<String>,
    idle_timeout: std::time::Duration,
) {
    if idle_timeout.is_zero() {
        return;
    }
    for (unit_id, last_seen) in running_last_progress {
        let elapsed = last_seen.elapsed();
        if elapsed >= idle_timeout && !running_stuck_emitted.contains(unit_id) {
            let _ = event_tx.send(PoolEvent::AgentStuck {
                unit_id: unit_id.clone(),
                last_progress_secs_ago: elapsed.as_secs(),
            });
            running_stuck_emitted.insert(unit_id.clone());
        }
    }
}

fn execute_verify_groups(
    config: &PoolConfig,
    units: &[DispatchUnit],
    results: &mut [AgentResult],
    event_tx: &mpsc::Sender<PoolEvent>,
) -> Result<bool> {
    if !config.batch_verify {
        return Ok(false);
    }

    let successful_units: Vec<DispatchUnit> = units
        .iter()
        .filter(|unit| results.iter().any(|result| result.unit_id == unit.id && result.success))
        .cloned()
        .collect();
    let groups = group_verify_commands(&successful_units);
    if groups.is_empty() {
        return Ok(false);
    }

    let project_root = config
        .mana_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine project root from mana dir"))?;

    let verify_results = run_verify_groups(&groups, event_tx, |group| {
        mana_core::ops::verify::run_verify_command(&group.command, project_root, None)
            .map(|result| result.passed)
            .unwrap_or(false)
    });

    let mut any_failed = false;
    for result in results.iter_mut() {
        if verify_results.get(&result.unit_id) == Some(&false) {
            result.success = false;
            result.error.get_or_insert_with(|| "verify failed".to_string());
            result
                .failure_summary
                .get_or_insert_with(|| "verify failed".to_string());
            any_failed = true;
        }
    }

    Ok(any_failed)
}

fn finish_dispatch(
    config: &PoolConfig,
    units: &[DispatchUnit],
    mut results: Vec<AgentResult>,
    mut any_failed: bool,
    event_tx: &mpsc::Sender<PoolEvent>,
    started: Instant,
) -> Result<DispatchOutcome> {
    if execute_verify_groups(config, units, &mut results, event_tx)? {
        any_failed = true;
    }

    let total = results.len();
    let passed = results.iter().filter(|r| r.success).count();
    let _ = event_tx.send(PoolEvent::Finished {
        total,
        passed,
        failed: total - passed,
        duration: started.elapsed(),
    });

    Ok(DispatchOutcome {
        results,
        any_failed,
    })
}

/// Run a full dispatch cycle: schedule units respecting dependencies, concurrency,
/// and memory limits. Spawns agents via the provided `Spawner` implementation.
///
/// This is the core scheduling loop — the pool's main job. It:
/// 1. Computes dependency order and downstream weights
/// 2. Finds ready units each iteration
/// 3. Checks capacity (max_concurrent) and resources (memory)
/// 4. Spawns agents on worker threads via the Spawner trait
/// 5. Waits for completions, unlocks dependents, repeats
///
/// Returns the collected results and whether any unit failed.
pub fn run_dispatch(
    config: &PoolConfig,
    units: &[DispatchUnit],
    completed_ids: &HashSet<String>,
    spawner: Arc<dyn Spawner>,
    event_tx: &mpsc::Sender<PoolEvent>,
    shutdown: &dyn Fn() -> bool,
) -> Result<DispatchOutcome> {
    run_dispatch_with_options(
        config,
        units,
        completed_ids,
        spawner,
        event_tx,
        shutdown,
        std::time::Duration::from_millis(200),
        None,
    )
}

fn run_dispatch_with_options(
    config: &PoolConfig,
    units: &[DispatchUnit],
    completed_ids: &HashSet<String>,
    spawner: Arc<dyn Spawner>,
    event_tx: &mpsc::Sender<PoolEvent>,
    shutdown: &dyn Fn() -> bool,
    poll_interval: std::time::Duration,
    idle_timeout_override: Option<std::time::Duration>,
) -> Result<DispatchOutcome> {
    let started = Instant::now();
    let all_ids: HashSet<String> = units.iter().map(|u| u.id.clone()).collect();
    let mut completed = completed_ids.clone();
    let mut remaining: HashMap<String, DispatchUnit> =
        units.iter().map(|u| (u.id.clone(), u.clone())).collect();

    let mut results: Vec<AgentResult> = Vec::new();
    let mut running_count: usize = 0;
    let mut any_failed = false;
    let idle_timeout = idle_timeout_override.unwrap_or_else(|| {
        std::time::Duration::from_secs((config.idle_timeout_minutes as u64) * 60)
    });

    // Track file paths of running units to avoid scheduling conflicts
    let mut running_paths: HashSet<String> = HashSet::new();
    let mut running_unit_paths: HashMap<String, Vec<String>> = HashMap::new();

    // Channel for completed agents to report back
    let (result_tx, result_rx) = mpsc::channel::<AgentResult>();
    // Channel for progress emitted by running agents
    let (progress_tx, progress_rx) = mpsc::channel::<(String, AgentProgress)>();

    // Track last progress/heartbeat time and whether we've already emitted a stuck event.
    let mut running_last_progress: HashMap<String, Instant> = HashMap::new();
    let mut running_stuck_emitted: HashSet<String> = HashSet::new();

    // Compute downstream weights for critical-path prioritization
    let weight_map = compute_downstream_weights(units);

    // Assign wave numbers for display
    let wave_map = compute_wave_map(units, completed_ids);

    let spawn_config = SpawnConfig {
        mana_dir: config.mana_dir.clone(),
        timeout_minutes: config.timeout_minutes,
        idle_timeout_minutes: config.idle_timeout_minutes,
        run_model: config.run_model.clone(),
        file_locking: config.file_locking,
        batch_verify: config.batch_verify,
        retry: RetryContext {
            attempt_number: 0,
            previous_failure: None,
            previous_notes: vec![],
        },
    };

    loop {
        let mut newly_started = 0;

        // Find ready units
        let mut ready: Vec<DispatchUnit> = remaining
            .values()
            .filter(|u| is_ready(u, &completed, &all_ids, units))
            .cloned()
            .collect();

        // Sort: priority → downstream weight (heaviest first) → natural ID order
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

        let mut memory_blocked = false;

        for unit in ready {
            if running_count >= config.max_concurrent {
                break;
            }

            if shutdown() {
                break;
            }

            // Check system memory before spawning
            if !memory::has_sufficient_memory(config.memory_reserve_mb) {
                memory_blocked = true;
                let _ = event_tx.send(PoolEvent::MemoryPressure {
                    reserve_mb: config.memory_reserve_mb,
                    available_mb: memory::available_memory_mb(),
                });
                break;
            }

            // Skip units whose paths conflict with running agents
            if unit.paths.iter().any(|p| running_paths.contains(p)) {
                continue;
            }

            remaining.remove(&unit.id);
            running_count += 1;

            // Register paths as occupied
            for p in &unit.paths {
                running_paths.insert(p.clone());
            }
            running_unit_paths.insert(unit.id.clone(), unit.paths.clone());
            running_last_progress.insert(unit.id.clone(), Instant::now());
            running_stuck_emitted.remove(&unit.id);

            let wave = wave_map.get(&unit.id).copied().unwrap_or(1);
            let _ = event_tx.send(PoolEvent::Spawning {
                unit_id: unit.id.clone(),
                title: unit.title.clone(),
                wave,
            });

            // Spawn agent on a worker thread
            let tx = result_tx.clone();
            let progress_tx_for_unit = progress_tx.clone();
            let cfg = SpawnConfig {
                retry: unit.retry.clone(),
                ..spawn_config.clone()
            };
            let unit_clone = unit.clone();
            let spawner_handle = Arc::clone(&spawner); // Arc<dyn Spawner> is Send

            std::thread::spawn(move || {
                let progress_sender = Some(progress_tx_for_unit);
                let result = spawner_handle.spawn(&unit_clone, &cfg, progress_sender);
                let _ = tx.send(result);
            });
            newly_started += 1;
            drain_progress_events(
                &progress_rx,
                event_tx,
                &mut running_last_progress,
                &mut running_stuck_emitted,
            );
        }

        // Stuck detection: nothing running, nothing started
        if running_count == 0 && newly_started == 0 {
            if memory_blocked {
                let _ = event_tx.send(PoolEvent::MemoryExhausted {
                    reserve_mb: config.memory_reserve_mb,
                    available_mb: memory::available_memory_mb(),
                });
            } else if !remaining.is_empty() {
                let ids: Vec<String> = remaining.keys().cloned().collect();
                let _ = event_tx.send(PoolEvent::UnresolvableDeps { unit_ids: ids });
            }
            break;
        }

        // Wait for one agent to complete
        if running_count > 0 {
            let result = loop {
                if shutdown() {
                    // Drain remaining results without spawning more
                    while running_count > 0 {
                        if let Ok(r) = result_rx.recv_timeout(poll_interval)
                        {
                            running_count -= 1;
                            let _ = event_tx.send(PoolEvent::Completed { result: r.clone() });
                            results.push(r);
                        }
                    }
                    return finish_dispatch(
                        config,
                        units,
                        results,
                        true,
                        event_tx,
                        started,
                    );
                }
                drain_progress_events(
                    &progress_rx,
                    event_tx,
                    &mut running_last_progress,
                    &mut running_stuck_emitted,
                );
                emit_stuck_events(
                    event_tx,
                    &running_last_progress,
                    &mut running_stuck_emitted,
                    idle_timeout,
                );
                match result_rx.recv_timeout(poll_interval) {
                    Ok(result) => break result,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        return finish_dispatch(
                            config,
                            units,
                            results,
                            any_failed,
                            event_tx,
                            started,
                        );
                    }
                }
            };

            running_count -= 1;

            running_last_progress.remove(&result.unit_id);
            running_stuck_emitted.remove(&result.unit_id);

            // Release paths
            if let Some(paths) = running_unit_paths.remove(&result.unit_id) {
                for p in &paths {
                    running_paths.remove(p);
                }
            }

            let success = result.success;
            let unit_id = result.unit_id.clone();

            let _ = event_tx.send(PoolEvent::Completed {
                result: result.clone(),
            });
            drain_progress_events(
        &progress_rx,
        event_tx,
        &mut running_last_progress,
        &mut running_stuck_emitted,
    );

            if success {
                completed.insert(unit_id);
            } else {
                any_failed = true;
                if !config.keep_going {
                    results.push(result);
                    // Drain running agents
                    while running_count > 0 {
                        if let Ok(r) = result_rx.recv() {
                            running_count -= 1;
                            let _ = event_tx.send(PoolEvent::Completed { result: r.clone() });
                            results.push(r);
                        }
                    }
                    return finish_dispatch(
                        config,
                        units,
                        results,
                        true,
                        event_tx,
                        started,
                    );
                }
            }

            results.push(result);
        }
    }

    // Drain any straggler results
    drop(result_tx);
    drain_progress_events(
        &progress_rx,
        event_tx,
        &mut running_last_progress,
        &mut running_stuck_emitted,
    );
    while let Ok(result) = result_rx.try_recv() {
        let _ = event_tx.send(PoolEvent::Completed {
            result: result.clone(),
        });
        drain_progress_events(
            &progress_rx,
            event_tx,
            &mut running_last_progress,
            &mut running_stuck_emitted,
        );
        results.push(result);
    }

    return finish_dispatch(
        config,
        units,
        results,
        any_failed,
        event_tx,
        started,
    );
}

/// Check if a unit's dependencies are satisfied.
fn is_ready(
    unit: &DispatchUnit,
    completed: &HashSet<String>,
    all_ids: &HashSet<String>,
    all_units: &[DispatchUnit],
) -> bool {
    // Explicit deps must be completed or not in our dispatch set
    let explicit_ok = unit
        .dependencies
        .iter()
        .all(|d| completed.contains(d) || !all_ids.contains(d));

    // Produces/requires: producer must be completed or not in set
    let requires_ok = unit.requires.iter().all(|req| {
        if let Some(producer) = all_units.iter().find(|other| {
            other.id != unit.id && other.parent == unit.parent && other.produces.contains(req)
        }) {
            completed.contains(&producer.id)
        } else {
            true
        }
    });

    explicit_ok && requires_ok
}

/// Compute downstream weights for critical-path scheduling.
/// Units that block more downstream work get higher weights.
fn compute_downstream_weights(units: &[DispatchUnit]) -> HashMap<String, u32> {
    let mut weights: HashMap<String, u32> = HashMap::new();

    // Build a reverse dependency map: unit → units that depend on it
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
    for u in units {
        for dep in &u.dependencies {
            dependents
                .entry(dep.clone())
                .or_default()
                .push(u.id.clone());
        }
        // Also count produces/requires edges
        for req in &u.requires {
            for other in units {
                if other.id != u.id && other.parent == u.parent && other.produces.contains(req) {
                    dependents
                        .entry(other.id.clone())
                        .or_default()
                        .push(u.id.clone());
                }
            }
        }
    }

    fn weight_of(
        id: &str,
        dependents: &HashMap<String, Vec<String>>,
        cache: &mut HashMap<String, u32>,
    ) -> u32 {
        if let Some(&w) = cache.get(id) {
            return w;
        }
        let mut w = 1u32;
        if let Some(deps) = dependents.get(id) {
            for dep_id in deps {
                w += weight_of(dep_id, dependents, cache);
            }
        }
        cache.insert(id.to_string(), w);
        w
    }

    for u in units {
        weight_of(&u.id, &dependents, &mut weights);
    }
    weights
}

/// Assign wave numbers for display: which "round" would each unit run in
/// if dispatched wave-by-wave (all ready → wait → all ready → wait...).
fn compute_wave_map(
    units: &[DispatchUnit],
    initial_completed: &HashSet<String>,
) -> HashMap<String, usize> {
    let all_ids: HashSet<String> = units.iter().map(|u| u.id.clone()).collect();
    let mut completed = initial_completed.clone();
    let mut remaining: HashSet<String> = units.iter().map(|u| u.id.clone()).collect();
    let mut wave_map = HashMap::new();
    let mut wave_num = 1;

    loop {
        let ready: Vec<String> = remaining
            .iter()
            .filter(|id| {
                if let Some(u) = units.iter().find(|u| &u.id == *id) {
                    is_ready(u, &completed, &all_ids, units)
                } else {
                    false
                }
            })
            .cloned()
            .collect();

        if ready.is_empty() {
            break;
        }

        for id in &ready {
            wave_map.insert(id.clone(), wave_num);
            remaining.remove(id);
            completed.insert(id.clone());
        }
        wave_num += 1;
    }

    wave_map
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    /// Test spawner that immediately succeeds.
    struct MockSpawner {
        spawn_count: Arc<AtomicUsize>,
    }

    impl MockSpawner {
        fn new() -> (Self, Arc<AtomicUsize>) {
            let count = Arc::new(AtomicUsize::new(0));
            (
                Self {
                    spawn_count: count.clone(),
                },
                count,
            )
        }
    }

    impl Spawner for MockSpawner {
        fn spawn(
            &self,
            unit: &DispatchUnit,
            _config: &SpawnConfig,
            _progress_tx: Option<mpsc::Sender<(String, AgentProgress)>>,
        ) -> AgentResult {
            self.spawn_count.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(10));
            AgentResult {
                unit_id: unit.id.clone(),
                title: unit.title.clone(),
                success: true,
                duration: Duration::from_millis(10),
                tokens: None,
                cost: None,
                error: None,
                tool_count: 0,
                turns: 0,
                failure_summary: None,
            }
        }
    }

    fn unit(id: &str, deps: &[&str]) -> DispatchUnit {
        DispatchUnit {
            id: id.to_string(),
            title: format!("Unit {}", id),
            priority: 2,
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
            parent: None,
            produces: vec![],
            requires: vec![],
            paths: vec![],
            verify_command: None,
            retry: RetryContext {
                attempt_number: 0,
                previous_failure: None,
                previous_notes: vec![],
            },
        }
    }

    fn pool_config() -> PoolConfig {
        PoolConfig {
            max_concurrent: 4,
            memory_reserve_mb: 0,
            timeout_minutes: 30,
            idle_timeout_minutes: 5,
            keep_going: false,
            batch_verify: false,
            file_locking: false,
            run_model: None,
            mana_dir: std::path::PathBuf::from("/tmp/test-mana"),
        }
    }

    #[test]
    fn dedup_verify_groups_identical_commands() {
        let units = vec![
            DispatchUnit {
                verify_command: Some("cargo build".to_string()),
                ..unit("1", &[])
            },
            DispatchUnit {
                verify_command: Some("cargo build".to_string()),
                ..unit("2", &[])
            },
            DispatchUnit {
                verify_command: Some("cargo test".to_string()),
                ..unit("3", &[])
            },
        ];

        let groups = group_verify_commands(&units);

        assert_eq!(
            groups,
            vec![
                VerifyGroup {
                    command: "cargo build".to_string(),
                    unit_ids: vec!["1".to_string(), "2".to_string()],
                },
                VerifyGroup {
                    command: "cargo test".to_string(),
                    unit_ids: vec!["3".to_string()],
                },
            ]
        );
    }

    #[test]
    fn dedup_verify_runs_once_per_group() {
        let groups = vec![
            VerifyGroup {
                command: "cargo build".to_string(),
                unit_ids: vec!["1".to_string(), "2".to_string()],
            },
            VerifyGroup {
                command: "cargo test".to_string(),
                unit_ids: vec!["3".to_string()],
            },
        ];
        let (event_tx, event_rx) = mpsc::channel();
        let run_count = Arc::new(AtomicUsize::new(0));
        let run_count_for_closure = run_count.clone();

        let results = run_verify_groups(&groups, &event_tx, |_group| {
            run_count_for_closure.fetch_add(1, Ordering::SeqCst);
            true
        });

        assert_eq!(run_count.load(Ordering::SeqCst), 2);
        assert_eq!(results.get("1"), Some(&true));
        assert_eq!(results.get("2"), Some(&true));
        assert_eq!(results.get("3"), Some(&true));

        let events: Vec<PoolEvent> = event_rx.try_iter().collect();
        let verify_events: Vec<(String, Vec<String>, bool)> = events
            .into_iter()
            .filter_map(|event| match event {
                PoolEvent::VerifyGroupRun {
                    command,
                    unit_ids,
                    success,
                } => Some((command, unit_ids, success)),
                _ => None,
            })
            .collect();
        assert_eq!(verify_events.len(), 2);
        assert_eq!(verify_events[0].0, "cargo build");
        assert_eq!(verify_events[0].1, vec!["1".to_string(), "2".to_string()]);
        assert!(verify_events[0].2);
        assert_eq!(verify_events[1].0, "cargo test");
        assert_eq!(verify_events[1].1, vec!["3".to_string()]);
        assert!(verify_events[1].2);
    }

    #[test]
    fn dedup_verify_attributes_failure_to_all() {
        let groups = vec![VerifyGroup {
            command: "cargo build".to_string(),
            unit_ids: vec!["1".to_string(), "2".to_string()],
        }];
        let (event_tx, event_rx) = mpsc::channel();

        let results = run_verify_groups(&groups, &event_tx, |_group| false);

        assert_eq!(results.get("1"), Some(&false));
        assert_eq!(results.get("2"), Some(&false));

        let events: Vec<PoolEvent> = event_rx.try_iter().collect();
        assert!(events.iter().any(|event| matches!(
            event,
            PoolEvent::VerifyGroupRun { command, unit_ids, success }
                if command == "cargo build"
                && unit_ids == &vec!["1".to_string(), "2".to_string()]
                && !success
        )));
    }


    #[test]
    fn dedup_verify_run_dispatch_marks_shared_failures() {
        struct SuccessSpawner;
        impl Spawner for SuccessSpawner {
            fn spawn(
                &self,
                unit: &DispatchUnit,
                _config: &SpawnConfig,
                _progress_tx: Option<mpsc::Sender<(String, AgentProgress)>>,
            ) -> AgentResult {
                AgentResult {
                    unit_id: unit.id.clone(),
                    title: unit.title.clone(),
                    success: true,
                    duration: Duration::from_millis(1),
                    tokens: None,
                    cost: None,
                    error: None,
                    tool_count: 0,
                    turns: 0,
                    failure_summary: None,
                }
            }
        }

        let mut config = pool_config();
        config.batch_verify = true;
        config.mana_dir = std::path::PathBuf::from("/tmp");

        let units = vec![
            DispatchUnit {
                verify_command: Some("exit 1".to_string()),
                ..unit("1", &[])
            },
            DispatchUnit {
                verify_command: Some("exit 1".to_string()),
                ..unit("2", &[])
            },
            DispatchUnit {
                verify_command: Some("true".to_string()),
                ..unit("3", &[])
            },
        ];
        let (event_tx, event_rx) = mpsc::channel();

        let outcome = run_dispatch(
            &config,
            &units,
            &HashSet::new(),
            Arc::new(SuccessSpawner),
            &event_tx,
            &|| false,
        )
        .unwrap();

        assert!(outcome.any_failed);
        assert_eq!(
            outcome
                .results
                .iter()
                .filter(|result| !result.success)
                .map(|result| result.unit_id.clone())
                .collect::<Vec<_>>(),
            vec!["1".to_string(), "2".to_string()]
        );
        assert!(outcome
            .results
            .iter()
            .find(|result| result.unit_id == "1")
            .unwrap()
            .error
            .as_deref()
            .unwrap()
            .contains("verify failed"));
        assert!(outcome
            .results
            .iter()
            .find(|result| result.unit_id == "2")
            .unwrap()
            .failure_summary
            .as_deref()
            .unwrap()
            .contains("verify failed"));
        assert!(outcome
            .results
            .iter()
            .find(|result| result.unit_id == "3")
            .unwrap()
            .success);

        let events: Vec<PoolEvent> = event_rx.try_iter().collect();
        let verify_events: Vec<(String, Vec<String>, bool)> = events
            .into_iter()
            .filter_map(|event| match event {
                PoolEvent::VerifyGroupRun {
                    command,
                    unit_ids,
                    success,
                } => Some((command, unit_ids, success)),
                _ => None,
            })
            .collect();
        assert_eq!(verify_events.len(), 2);
        assert!(verify_events.iter().any(|(command, unit_ids, success)| {
            command == "exit 1" && unit_ids == &vec!["1".to_string(), "2".to_string()] && !success
        }));
        assert!(verify_events.iter().any(|(command, unit_ids, success)| {
            command == "true" && unit_ids == &vec!["3".to_string()] && *success
        }));
    }

    #[test]
    fn respects_dependencies() {
        // 2 depends on 1, 3 depends on 2
        let units = vec![unit("1", &[]), unit("2", &["1"]), unit("3", &["2"])];
        let (spawner, count) = MockSpawner::new();
        let (event_tx, event_rx) = mpsc::channel();
        let completed = HashSet::new();

        let outcome = run_dispatch(
            &pool_config(),
            &units,
            &completed,
            Arc::new(spawner),
            &event_tx,
            &|| false,
        )
        .unwrap();

        assert_eq!(count.load(Ordering::SeqCst), 3);
        assert!(!outcome.any_failed);

        // Verify ordering via events: spawns should be sequential
        let events: Vec<PoolEvent> = event_rx.try_iter().collect();
        let spawn_order: Vec<String> = events
            .iter()
            .filter_map(|e| {
                if let PoolEvent::Spawning { unit_id, .. } = e {
                    Some(unit_id.clone())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(spawn_order, vec!["1", "2", "3"]);
    }

    #[test]
    fn respects_max_concurrent() {
        let mut config = pool_config();
        config.max_concurrent = 1;

        let units = vec![unit("1", &[]), unit("2", &[]), unit("3", &[])];
        let (spawner, count) = MockSpawner::new();
        let (event_tx, _) = mpsc::channel();

        let outcome = run_dispatch(
            &config,
            &units,
            &HashSet::new(),
            Arc::new(spawner),
            &event_tx,
            &|| false,
        )
        .unwrap();

        assert_eq!(count.load(Ordering::SeqCst), 3);
        assert_eq!(outcome.results.len(), 3);
    }

    #[test]
    fn budget_enforces_max_concurrent_limit() {
        struct TrackConcurrency {
            in_flight: Arc<AtomicUsize>,
            max_seen: Arc<AtomicUsize>,
        }

        impl Spawner for TrackConcurrency {
            fn spawn(
                &self,
                unit: &DispatchUnit,
                _config: &SpawnConfig,
                _progress_tx: Option<mpsc::Sender<(String, AgentProgress)>>,
            ) -> AgentResult {
                let now_running = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                self.max_seen.fetch_max(now_running, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(40));
                self.in_flight.fetch_sub(1, Ordering::SeqCst);
                AgentResult {
                    unit_id: unit.id.clone(),
                    title: unit.title.clone(),
                    success: true,
                    duration: Duration::from_millis(40),
                    tokens: None,
                    cost: None,
                    error: None,
                    tool_count: 0,
                    turns: 0,
                    failure_summary: None,
                }
            }
        }

        let mut config = pool_config();
        config.max_concurrent = 1;

        let units = vec![unit("1", &[]), unit("2", &[]), unit("3", &[])];
        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));
        let spawner = TrackConcurrency {
            in_flight: in_flight.clone(),
            max_seen: max_seen.clone(),
        };
        let (event_tx, _) = mpsc::channel();

        let outcome = run_dispatch(
            &config,
            &units,
            &HashSet::new(),
            Arc::new(spawner),
            &event_tx,
            &|| false,
        )
        .unwrap();

        assert_eq!(outcome.results.len(), 3);
        assert_eq!(in_flight.load(Ordering::SeqCst), 0);
        assert_eq!(max_seen.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn spawner_receives_attempt_count() {
        struct AssertRetry;
        impl Spawner for AssertRetry {
            fn spawn(
                &self,
                unit: &DispatchUnit,
                config: &SpawnConfig,
                _progress_tx: Option<mpsc::Sender<(String, AgentProgress)>>,
            ) -> AgentResult {
                assert_eq!(unit.retry.attempt_number, 2);
                assert_eq!(config.retry.attempt_number, 2);
                AgentResult {
                    unit_id: unit.id.clone(),
                    title: unit.title.clone(),
                    success: true,
                    duration: Duration::from_millis(1),
                    tokens: None,
                    cost: None,
                    error: None,
                    tool_count: 0,
                    turns: 0,
                    failure_summary: None,
                }
            }
        }

        let mut retrying = unit("1", &[]);
        retrying.retry = RetryContext {
            attempt_number: 2,
            previous_failure: Some("verify failed".to_string()),
            previous_notes: vec!["try smaller patch".to_string()],
        };
        let (event_tx, _event_rx) = mpsc::channel();
        let outcome = run_dispatch(
            &pool_config(),
            &[retrying],
            &HashSet::new(),
            Arc::new(AssertRetry),
            &event_tx,
            &|| false,
        )
        .unwrap();
        assert_eq!(outcome.results.len(), 1);
    }

    #[test]
    fn spawner_receives_previous_failure_and_notes() {
        struct AssertFailure;
        impl Spawner for AssertFailure {
            fn spawn(
                &self,
                unit: &DispatchUnit,
                config: &SpawnConfig,
                _progress_tx: Option<mpsc::Sender<(String, AgentProgress)>>,
            ) -> AgentResult {
                assert_eq!(
                    unit.retry.previous_failure.as_deref(),
                    Some("tests failed in auth flow")
                );
                assert_eq!(
                    config.retry.previous_failure.as_deref(),
                    Some("tests failed in auth flow")
                );
                assert_eq!(
                    unit.retry.previous_notes,
                    vec!["first note".to_string(), "second note".to_string()]
                );
                AgentResult {
                    unit_id: unit.id.clone(),
                    title: unit.title.clone(),
                    success: true,
                    duration: Duration::from_millis(1),
                    tokens: None,
                    cost: None,
                    error: None,
                    tool_count: 0,
                    turns: 0,
                    failure_summary: None,
                }
            }
        }

        let mut retrying = unit("1", &[]);
        retrying.retry = RetryContext {
            attempt_number: 1,
            previous_failure: Some("tests failed in auth flow".to_string()),
            previous_notes: vec!["first note".to_string(), "second note".to_string()],
        };
        let (event_tx, _event_rx) = mpsc::channel();
        let outcome = run_dispatch(
            &pool_config(),
            &[retrying],
            &HashSet::new(),
            Arc::new(AssertFailure),
            &event_tx,
            &|| false,
        )
        .unwrap();
        assert_eq!(outcome.results.len(), 1);
    }

    #[test]
    fn progress_events_forwarded() {
        struct ProgressSpawner;
        impl Spawner for ProgressSpawner {
            fn spawn(
                &self,
                unit: &DispatchUnit,
                _config: &SpawnConfig,
                progress_tx: Option<mpsc::Sender<(String, AgentProgress)>>,
            ) -> AgentResult {
                if let Some(tx) = progress_tx {
                    let _ = tx.send((
                        unit.id.clone(),
                        AgentProgress::Progress {
                            phase: "planning".to_string(),
                            elapsed: Duration::from_millis(5),
                        },
                    ));
                    let _ = tx.send((
                        unit.id.clone(),
                        AgentProgress::Heartbeat {
                            elapsed: Duration::from_millis(7),
                        },
                    ));
                }
                AgentResult {
                    unit_id: unit.id.clone(),
                    title: unit.title.clone(),
                    success: true,
                    duration: Duration::from_millis(10),
                    tokens: None,
                    cost: None,
                    error: None,
                    tool_count: 0,
                    turns: 0,
                    failure_summary: None,
                }
            }
        }

        let units = vec![unit("1", &[])];
        let (event_tx, event_rx) = mpsc::channel();
        let outcome = run_dispatch(
            &pool_config(),
            &units,
            &HashSet::new(),
            Arc::new(ProgressSpawner),
            &event_tx,
            &|| false,
        )
        .unwrap();
        assert_eq!(outcome.results.len(), 1);
        let events: Vec<PoolEvent> = event_rx.try_iter().collect();
        assert!(events.iter().any(|e| matches!(
            e,
            PoolEvent::Progress { unit_id, phase, .. } if unit_id == "1" && phase == "planning"
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            PoolEvent::Heartbeat { unit_id, .. } if unit_id == "1"
        )));
    }

    #[test]
    fn no_progress_channel_still_works() {
        struct NoProgressSpawner;
        impl Spawner for NoProgressSpawner {
            fn spawn(
                &self,
                unit: &DispatchUnit,
                _config: &SpawnConfig,
                _progress_tx: Option<mpsc::Sender<(String, AgentProgress)>>,
            ) -> AgentResult {
                AgentResult {
                    unit_id: unit.id.clone(),
                    title: unit.title.clone(),
                    success: true,
                    duration: Duration::from_millis(1),
                    tokens: None,
                    cost: None,
                    error: None,
                    tool_count: 0,
                    turns: 0,
                    failure_summary: None,
                }
            }
        }

        let (event_tx, _event_rx) = mpsc::channel();
        let outcome = run_dispatch(
            &pool_config(),
            &[unit("1", &[])],
            &HashSet::new(),
            Arc::new(NoProgressSpawner),
            &event_tx,
            &|| false,
        )
        .unwrap();
        assert_eq!(outcome.results.len(), 1);
    }

    #[test]
    fn heartbeat_timeout_detects_stuck_agent() {
        struct SilentSlowSpawner;
        impl Spawner for SilentSlowSpawner {
            fn spawn(
                &self,
                unit: &DispatchUnit,
                _config: &SpawnConfig,
                _progress_tx: Option<mpsc::Sender<(String, AgentProgress)>>,
            ) -> AgentResult {
                std::thread::sleep(Duration::from_millis(120));
                AgentResult {
                    unit_id: unit.id.clone(),
                    title: unit.title.clone(),
                    success: true,
                    duration: Duration::from_millis(120),
                    tokens: None,
                    cost: None,
                    error: None,
                    tool_count: 0,
                    turns: 0,
                    failure_summary: None,
                }
            }
        }

        let config = pool_config();
        let units = vec![unit("1", &[])];
        let (event_tx, event_rx) = mpsc::channel();
        let outcome = run_dispatch_with_options(
            &config,
            &units,
            &HashSet::new(),
            Arc::new(SilentSlowSpawner),
            &event_tx,
            &|| false,
            Duration::from_millis(20),
            Some(Duration::from_millis(50)),
        )
        .unwrap();
        assert_eq!(outcome.results.len(), 1);
        let events: Vec<PoolEvent> = event_rx.try_iter().collect();
        assert!(events.iter().any(|e| matches!(
            e,
            PoolEvent::AgentStuck { unit_id, .. } if unit_id == "1"
        )));
    }

    #[test]
    fn first_attempt_has_zero_attempts() {
        struct AssertFirst;
        impl Spawner for AssertFirst {
            fn spawn(
                &self,
                unit: &DispatchUnit,
                config: &SpawnConfig,
                _progress_tx: Option<mpsc::Sender<(String, AgentProgress)>>,
            ) -> AgentResult {
                assert_eq!(unit.retry.attempt_number, 0);
                assert_eq!(config.retry.attempt_number, 0);
                assert!(unit.retry.previous_failure.is_none());
                assert!(unit.retry.previous_notes.is_empty());
                AgentResult {
                    unit_id: unit.id.clone(),
                    title: unit.title.clone(),
                    success: true,
                    duration: Duration::from_millis(1),
                    tokens: None,
                    cost: None,
                    error: None,
                    tool_count: 0,
                    turns: 0,
                    failure_summary: None,
                }
            }
        }

        let (event_tx, _event_rx) = mpsc::channel();
        let outcome = run_dispatch(
            &pool_config(),
            &[unit("1", &[])],
            &HashSet::new(),
            Arc::new(AssertFirst),
            &event_tx,
            &|| false,
        )
        .unwrap();
        assert_eq!(outcome.results.len(), 1);
    }

    #[test]
    fn stops_on_failure_without_keep_going() {
        struct FailSecond(AtomicUsize);
        impl Spawner for FailSecond {
            fn spawn(
                &self,
                unit: &DispatchUnit,
                _config: &SpawnConfig,
                _progress_tx: Option<mpsc::Sender<(String, AgentProgress)>>,
            ) -> AgentResult {
                let n = self.0.fetch_add(1, Ordering::SeqCst);
                AgentResult {
                    unit_id: unit.id.clone(),
                    title: unit.title.clone(),
                    success: n != 1, // second unit fails
                    duration: Duration::from_millis(5),
                    tokens: None,
                    cost: None,
                    error: if n == 1 {
                        Some("test failure".into())
                    } else {
                        None
                    },
                    tool_count: 0,
                    turns: 0,
                    failure_summary: None,
                }
            }
        }

        let mut config = pool_config();
        config.max_concurrent = 1; // sequential so ordering is deterministic

        let units = vec![unit("1", &[]), unit("2", &[]), unit("3", &[])];
        let spawner = FailSecond(AtomicUsize::new(0));
        let (event_tx, _) = mpsc::channel();

        let outcome = run_dispatch(
            &config,
            &units,
            &HashSet::new(),
            Arc::new(spawner),
            &event_tx,
            &|| false,
        )
        .unwrap();

        assert!(outcome.any_failed);
        // Should stop after 2 (first succeeds, second fails, third never runs)
        assert_eq!(outcome.results.len(), 2);
    }

    #[test]
    fn budget_circuit_breaker_stops_new_spawns_after_failure() {
        struct FailSecondBudget(AtomicUsize);
        impl Spawner for FailSecondBudget {
            fn spawn(
                &self,
                unit: &DispatchUnit,
                _config: &SpawnConfig,
                _progress_tx: Option<mpsc::Sender<(String, AgentProgress)>>,
            ) -> AgentResult {
                let n = self.0.fetch_add(1, Ordering::SeqCst);
                AgentResult {
                    unit_id: unit.id.clone(),
                    title: unit.title.clone(),
                    success: n != 1,
                    duration: Duration::from_millis(5),
                    tokens: None,
                    cost: None,
                    error: if n == 1 {
                        Some("test failure".into())
                    } else {
                        None
                    },
                    tool_count: 0,
                    turns: 0,
                    failure_summary: None,
                }
            }
        }

        let mut config = pool_config();
        config.max_concurrent = 1;

        let units = vec![unit("1", &[]), unit("2", &[]), unit("3", &[])];
        let spawner = FailSecondBudget(AtomicUsize::new(0));
        let (event_tx, event_rx) = mpsc::channel();

        let outcome = run_dispatch(
            &config,
            &units,
            &HashSet::new(),
            Arc::new(spawner),
            &event_tx,
            &|| false,
        )
        .unwrap();

        assert!(outcome.any_failed);
        assert_eq!(outcome.results.len(), 2);

        let events: Vec<PoolEvent> = event_rx.try_iter().collect();
        let spawn_order: Vec<String> = events
            .iter()
            .filter_map(|e| {
                if let PoolEvent::Spawning { unit_id, .. } = e {
                    Some(unit_id.clone())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(spawn_order, vec!["1", "2"]);
    }

    #[test]
    fn is_ready_no_deps() {
        let u = unit("1", &[]);
        let completed = HashSet::new();
        let all_ids: HashSet<String> = ["1"].iter().map(|s| s.to_string()).collect();
        assert!(is_ready(&u, &completed, &all_ids, &[u.clone()]));
    }

    #[test]
    fn is_ready_dep_not_met() {
        let u = unit("2", &["1"]);
        let completed = HashSet::new();
        let all_ids: HashSet<String> = ["1", "2"].iter().map(|s| s.to_string()).collect();
        let units = vec![unit("1", &[]), u.clone()];
        assert!(!is_ready(&u, &completed, &all_ids, &units));
    }

    #[test]
    fn is_ready_dep_met() {
        let u = unit("2", &["1"]);
        let mut completed = HashSet::new();
        completed.insert("1".to_string());
        let all_ids: HashSet<String> = ["1", "2"].iter().map(|s| s.to_string()).collect();
        let units = vec![unit("1", &[]), u.clone()];
        assert!(is_ready(&u, &completed, &all_ids, &units));
    }

    #[test]
    fn downstream_weights() {
        let units = vec![
            unit("1", &[]),
            unit("2", &["1"]),
            unit("3", &["1"]),
            unit("4", &["2", "3"]),
        ];
        let weights = compute_downstream_weights(&units);
        // Unit 1 blocks 2, 3, 4 → highest weight
        assert!(weights[&"1".to_string()] > weights[&"2".to_string()]);
        assert!(weights[&"1".to_string()] > weights[&"3".to_string()]);
        // Unit 4 is a leaf
        assert_eq!(weights[&"4".to_string()], 1);
    }
}
