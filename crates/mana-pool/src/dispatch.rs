use std::collections::{HashMap, HashSet};
use std::sync::{mpsc, Arc};
use std::time::Instant;

use anyhow::Result;
use mana_core::util::natural_cmp;

use crate::memory;
use crate::types::*;

fn drain_progress_events(
    progress_rx: &mpsc::Receiver<(String, AgentProgress)>,
    event_tx: &mpsc::Sender<PoolEvent>,
) {
    while let Ok((unit_id, progress)) = progress_rx.try_recv() {
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
    let started = Instant::now();
    let all_ids: HashSet<String> = units.iter().map(|u| u.id.clone()).collect();
    let mut completed = completed_ids.clone();
    let mut remaining: HashMap<String, DispatchUnit> =
        units.iter().map(|u| (u.id.clone(), u.clone())).collect();

    let mut results: Vec<AgentResult> = Vec::new();
    let mut running_count: usize = 0;
    let mut any_failed = false;

    // Track file paths of running units to avoid scheduling conflicts
    let mut running_paths: HashSet<String> = HashSet::new();
    let mut running_unit_paths: HashMap<String, Vec<String>> = HashMap::new();

    // Channel for completed agents to report back
    let (result_tx, result_rx) = mpsc::channel::<AgentResult>();
    // Channel for progress emitted by running agents
    let (progress_tx, progress_rx) = mpsc::channel::<(String, AgentProgress)>();

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
            drain_progress_events(&progress_rx, event_tx);
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
                        if let Ok(r) = result_rx.recv_timeout(std::time::Duration::from_millis(100))
                        {
                            running_count -= 1;
                            let _ = event_tx.send(PoolEvent::Completed { result: r.clone() });
                            results.push(r);
                        }
                    }
                    let total = results.len();
                    let passed = results.iter().filter(|r| r.success).count();
                    let _ = event_tx.send(PoolEvent::Finished {
                        total,
                        passed,
                        failed: total - passed,
                        duration: started.elapsed(),
                    });
                    return Ok(DispatchOutcome {
                        results,
                        any_failed: true,
                    });
                }
                drain_progress_events(&progress_rx, event_tx);
                match result_rx.recv_timeout(std::time::Duration::from_millis(200)) {
                    Ok(result) => break result,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        return Ok(DispatchOutcome {
                            results,
                            any_failed,
                        });
                    }
                }
            };

            running_count -= 1;

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
            drain_progress_events(&progress_rx, event_tx);

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
                    let total = results.len();
                    let passed = results.iter().filter(|r| r.success).count();
                    let _ = event_tx.send(PoolEvent::Finished {
                        total,
                        passed,
                        failed: total - passed,
                        duration: started.elapsed(),
                    });
                    return Ok(DispatchOutcome {
                        results,
                        any_failed: true,
                    });
                }
            }

            results.push(result);
        }
    }

    // Drain any straggler results
    drop(result_tx);
    drain_progress_events(&progress_rx, event_tx);
    while let Ok(result) = result_rx.try_recv() {
        let _ = event_tx.send(PoolEvent::Completed {
            result: result.clone(),
        });
        drain_progress_events(&progress_rx, event_tx);
        results.push(result);
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
    fn dispatches_independent_units() {
        let units = vec![unit("1", &[]), unit("2", &[]), unit("3", &[])];
        let (spawner, count) = MockSpawner::new();
        let (event_tx, _event_rx) = mpsc::channel();
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
        assert_eq!(outcome.results.len(), 3);
        assert!(!outcome.any_failed);
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
