//! Core orchestration operations for `mana run`.
//!
//! This module exposes the dependency-aware scheduling logic that imp_orch
//! and other library consumers need to compute ready queues and execution plans
//! without depending on the CLI layer.
//!
//! # Key types
//!
//! - [`ReadyQueue`] — result of computing which units are ready to dispatch
//! - [`ReadyUnit`] — a single dispatchable unit with scheduling metadata
//! - [`RunPlan`] — a full execution plan grouped into waves
//! - [`RunWave`] — a group of units that can run concurrently
//!
//! # Key functions
//!
//! - [`compute_ready_queue`] — find all dispatchable units with priority/weight ordering
//! - [`compute_run_plan`] — group ready units into dependency-ordered waves

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Result;

use crate::blocking::{check_blocked_with_archive, check_scope_warning, ScopeWarning};
use crate::discovery::find_unit_file;
use crate::index::{ArchiveIndex, Index, IndexEntry};
use crate::unit::{AttemptOutcome, AutonomyBlockerCode, Status, Unit};
use crate::util::natural_cmp;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunTarget {
    AllReady,
    Unit(String),
    Explicit(Vec<String>),
}

fn parent_id_for(index: &Index, unit_id: &str) -> Option<String> {
    index
        .units
        .iter()
        .find(|entry| entry.id == unit_id)
        .and_then(|entry| entry.parent.clone())
}

fn is_descendant_of(index: &Index, unit_id: &str, ancestor_id: &str) -> bool {
    let mut current = parent_id_for(index, unit_id);

    while let Some(parent_id) = current {
        if parent_id == ancestor_id {
            return true;
        }
        current = parent_id_for(index, &parent_id);
    }

    false
}

fn has_open_descendants(index: &Index, unit_id: &str) -> bool {
    index
        .units
        .iter()
        .any(|entry| entry.status != Status::Closed && is_descendant_of(index, &entry.id, unit_id))
}

fn matches_target(index: &Index, entry: &IndexEntry, target: &RunTarget) -> bool {
    match target {
        RunTarget::AllReady => true,
        RunTarget::Unit(filter_id) => {
            let target_has_open_descendants = index.units.iter().any(|candidate| {
                candidate.status != Status::Closed
                    && is_descendant_of(index, &candidate.id, filter_id)
            });

            if target_has_open_descendants {
                is_descendant_of(index, &entry.id, filter_id)
                    && !has_open_descendants(index, &entry.id)
            } else {
                entry.id == *filter_id
            }
        }
        RunTarget::Explicit(ids) => ids
            .iter()
            .any(|id| matches_target(index, entry, &RunTarget::Unit(id.clone()))),
    }
}

/// A unit that is ready to be dispatched.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadyUnit {
    pub id: String,
    pub title: String,
    /// Lower is higher priority (1 = P1, etc.).
    pub priority: u8,
    /// Downstream dependency weight for critical-path scheduling.
    /// Higher weight = more downstream units blocked = schedule first.
    pub critical_path_weight: u32,
    /// Files this unit will modify (for conflict detection).
    pub paths: Vec<String>,
    /// Artifacts this unit produces.
    pub produces: Vec<String>,
    /// Artifacts this unit requires from siblings.
    pub requires: Vec<String>,
    /// Explicit dependency IDs.
    pub dependencies: Vec<String>,
    /// Parent unit ID (for sibling produces/requires resolution).
    pub parent: Option<String>,
    /// Optional fast verify command to run before the full verify gate.
    pub verify_fast: Option<String>,
    /// Deferred verify command for grouped post-agent verification.
    pub verify_command: Option<String>,
    /// Retry context derived from prior attempts without depending on pool/runtime crates.
    pub retry: RunRetryContext,
    /// Per-unit model override from frontmatter.
    pub model: Option<String>,
}

/// Retry context derived from a unit's attempt history.
#[derive(Debug, Clone, PartialEq)]
pub struct RunRetryContext {
    pub attempt_number: u32,
    pub previous_failure: Option<String>,
    pub previous_notes: Vec<String>,
}

/// A non-blocking warning for a unit that will still dispatch.
#[derive(Debug, Clone, PartialEq)]
pub struct RunScopeWarning {
    pub id: String,
    pub warning: ScopeWarning,
}

/// A unit that was excluded from dispatch.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockedUnit {
    pub id: String,
    pub title: String,
    pub reason: String,
    /// Canonical autonomy blocker code when this blocked reason maps to the
    /// scheduler-visible autonomy contract.
    pub blocker: Option<AutonomyBlockerCode>,
    /// Unresolved decision prompts when `blocker == Some(UnresolvedDecision)`.
    pub decisions: Vec<String>,
}

/// The result of computing the ready queue.
#[derive(Debug, Clone)]
pub struct ReadyQueue {
    /// Units ready to dispatch, sorted by priority then critical-path weight.
    pub units: Vec<ReadyUnit>,
    /// Units that are blocked by autonomy/scope/dependency guardrails.
    pub blocked: Vec<BlockedUnit>,
    /// Scope warnings for units that will dispatch.
    pub warnings: Vec<RunScopeWarning>,
}

/// A wave of units that can run concurrently (no inter-wave dependencies).
#[derive(Debug, Clone)]
pub struct RunWave {
    /// Units in this wave, sorted by priority then critical-path weight.
    pub units: Vec<ReadyUnit>,
}

/// A full execution plan grouped into dependency-ordered waves.
#[derive(Debug, Clone)]
pub struct RunPlan {
    /// Ordered waves (wave 0 has no deps, wave 1 depends on wave 0, etc.).
    pub waves: Vec<RunWave>,
    /// Total number of dispatchable units across all waves.
    pub total_units: usize,
    /// Units that cannot be dispatched.
    pub blocked: Vec<BlockedUnit>,
    /// Scope warnings for units that will dispatch.
    pub warnings: Vec<RunScopeWarning>,
}

// ---------------------------------------------------------------------------
// Core computation
// ---------------------------------------------------------------------------

/// Check if all dependencies of a unit are satisfied.
///
/// A dependency is satisfied if it is closed in the active index, or present
/// in the archive (archived units are always considered closed). Dependencies
/// not found in either index are treated as unsatisfied to catch typos.
pub fn all_deps_closed(entry: &IndexEntry, index: &Index, archive: &ArchiveIndex) -> bool {
    for dep_id in &entry.dependencies {
        match index.units.iter().find(|e| e.id == *dep_id) {
            Some(dep) if dep.status == Status::Closed => {}
            Some(_) => return false,
            None => {
                if !archive.units.iter().any(|e| e.id == *dep_id) {
                    return false;
                }
            }
        }
    }

    for required in &entry.requires {
        if let Some(producer) = index
            .units
            .iter()
            .find(|e| e.id != entry.id && e.parent == entry.parent && e.produces.contains(required))
        {
            if producer.status != Status::Closed {
                return false;
            }
        }
        // If producer is in archive (archived = closed) or not found, treat as satisfied
    }

    true
}

/// Compute downstream dependency weights for critical-path scheduling.
///
/// Each unit's weight is `1 + count of all transitively dependent units`.
/// Units on the critical path (most blocked work downstream) get the highest weight.
pub fn compute_downstream_weights(units: &[ReadyUnit]) -> HashMap<String, u32> {
    let unit_ids: HashSet<String> = units.iter().map(|u| u.id.clone()).collect();

    // Build reverse dependency graph: dep → Vec<dependents>
    let mut reverse_deps: HashMap<String, Vec<String>> = HashMap::new();

    for u in units {
        reverse_deps.entry(u.id.clone()).or_default();

        for dep in &u.dependencies {
            if unit_ids.contains(dep) {
                reverse_deps
                    .entry(dep.clone())
                    .or_default()
                    .push(u.id.clone());
            }
        }

        for req in &u.requires {
            if let Some(producer) = units.iter().find(|other| {
                other.id != u.id && other.parent == u.parent && other.produces.contains(req)
            }) {
                if unit_ids.contains(&producer.id) {
                    reverse_deps
                        .entry(producer.id.clone())
                        .or_default()
                        .push(u.id.clone());
                }
            }
        }
    }

    let mut weights: HashMap<String, u32> = HashMap::new();

    for u in units {
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: Vec<String> = Vec::new();

        for dep in reverse_deps.get(&u.id).unwrap_or(&Vec::new()) {
            if visited.insert(dep.clone()) {
                queue.push(dep.clone());
            }
        }

        while let Some(current) = queue.pop() {
            for next in reverse_deps.get(&current).unwrap_or(&Vec::new()) {
                if visited.insert(next.clone()) {
                    queue.push(next.clone());
                }
            }
        }

        weights.insert(u.id.clone(), 1 + visited.len() as u32);
    }

    weights
}

/// Check if a unit's dependencies are all satisfied within a dispatch set.
fn is_unit_ready(
    unit: &ReadyUnit,
    completed: &HashSet<String>,
    all_unit_ids: &HashSet<String>,
    all_units: &[ReadyUnit],
) -> bool {
    let explicit_ok = unit
        .dependencies
        .iter()
        .all(|d| completed.contains(d) || !all_unit_ids.contains(d));

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

/// Sort a list of units by priority (ascending) then critical-path weight (descending) then ID.
fn sort_units(units: &mut [ReadyUnit], weights: &HashMap<String, u32>) {
    units.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| {
                let wa = weights.get(&a.id).copied().unwrap_or(1);
                let wb = weights.get(&b.id).copied().unwrap_or(1);
                wb.cmp(&wa)
            })
            .then_with(|| natural_cmp(&a.id, &b.id))
    });
}

fn build_retry_context(unit: &Unit) -> RunRetryContext {
    RunRetryContext {
        attempt_number: unit.attempts,
        previous_failure: unit.attempt_log.iter().rev().find_map(|attempt| {
            match attempt.outcome {
                AttemptOutcome::Failed | AttemptOutcome::Abandoned => attempt.notes.clone(),
                AttemptOutcome::Success => None,
            }
        }),
        previous_notes: unit
            .attempt_log
            .iter()
            .filter_map(|attempt| attempt.notes.clone())
            .collect(),
    }
}

/// Build a `ReadyUnit` from an index entry and the loaded unit file.
fn build_ready_unit(entry: &IndexEntry, unit: &Unit, weight: u32) -> ReadyUnit {
    ReadyUnit {
        id: entry.id.clone(),
        title: entry.title.clone(),
        priority: entry.priority,
        critical_path_weight: weight,
        paths: entry.paths.clone(),
        produces: entry.produces.clone(),
        requires: entry.requires.clone(),
        dependencies: entry.dependencies.clone(),
        parent: entry.parent.clone(),
        verify_fast: unit.verify_fast.clone(),
        verify_command: unit.verify.clone(),
        retry: build_retry_context(unit),
        model: unit.model.clone(),
    }
}

/// Build a canonical blocked unit for unresolved durable decisions.
pub fn blocked_unit_for_unresolved_decisions(
    entry: &IndexEntry,
    unit: &Unit,
) -> Option<BlockedUnit> {
    if unit.decisions.is_empty() {
        return None;
    }

    Some(BlockedUnit {
        id: entry.id.clone(),
        title: entry.title.clone(),
        reason: "unresolved_decision".to_string(),
        blocker: Some(AutonomyBlockerCode::UnresolvedDecision),
        decisions: unit.decisions.clone(),
    })
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compute which units are ready to dispatch.
///
/// Returns a [`ReadyQueue`] with units sorted by priority then critical-path
/// weight (highest-weight first within same priority). Optionally filters to
/// a specific unit ID or its ready children if `filter_id` is a parent.
///
/// Set `simulate = true` to include all open units with verify commands —
/// even those whose deps are not yet met. This is the dry-run mode.
pub fn compute_ready_queue(
    mana_dir: &Path,
    target: &RunTarget,
    simulate: bool,
) -> Result<ReadyQueue> {
    let index = Index::load_or_rebuild(mana_dir)?;
    let archive = ArchiveIndex::load_or_rebuild(mana_dir)
        .unwrap_or_else(|_| ArchiveIndex { units: Vec::new() });

    let candidates: Vec<&IndexEntry> = index
        .units
        .iter()
        .filter(|e| {
            e.kind == crate::unit::UnitType::Task
                && e.has_verify
                && e.status == Status::Open
                && (simulate || all_deps_closed(e, &index, &archive))
                && !has_open_descendants(&index, &e.id)
                && matches_target(&index, e, target)
        })
        .collect();

    let mut blocked: Vec<BlockedUnit> = Vec::new();
    let mut warnings: Vec<RunScopeWarning> = Vec::new();

    // Collect dispatchable entries
    let mut entries_and_units: Vec<(&IndexEntry, Unit)> = Vec::new();
    for entry in &candidates {
        let unit_path = find_unit_file(mana_dir, &entry.id)?;
        let unit = Unit::from_file(&unit_path)?;

        if !simulate {
            if let Some(unresolved_blocked) = blocked_unit_for_unresolved_decisions(entry, &unit) {
                blocked.push(unresolved_blocked);
                continue;
            }

            if let Some(reason) = check_blocked_with_archive(entry, &index, Some(&archive)) {
                blocked.push(BlockedUnit {
                    id: entry.id.clone(),
                    title: entry.title.clone(),
                    reason: reason.to_string(),
                    blocker: None,
                    decisions: Vec::new(),
                });
                continue;
            }
        }
        if let Some(warning) = check_scope_warning(entry) {
            warnings.push(RunScopeWarning {
                id: entry.id.clone(),
                warning,
            });
        }
        entries_and_units.push((entry, unit));
    }

    // Build provisional list (weight = 1), then compute real weights and update
    let mut ready_units: Vec<ReadyUnit> = entries_and_units
        .iter()
        .map(|(entry, unit)| build_ready_unit(entry, unit, 1))
        .collect();

    let weights = compute_downstream_weights(&ready_units);
    for unit in &mut ready_units {
        unit.critical_path_weight = weights.get(&unit.id).copied().unwrap_or(1);
    }
    sort_units(&mut ready_units, &weights);

    Ok(ReadyQueue {
        units: ready_units,
        blocked,
        warnings,
    })
}

/// Compute a full execution plan grouped into dependency-ordered waves.
///
/// Wave 0 contains units with no unsatisfied deps. Wave 1 depends on wave 0.
/// And so on. Within each wave, units are sorted by priority then critical-path weight.
///
/// Set `simulate = true` for dry-run mode (includes units whose deps are not yet met).
pub fn compute_run_plan(
    mana_dir: &Path,
    target: &RunTarget,
    simulate: bool,
) -> Result<RunPlan> {
    let queue = compute_ready_queue(mana_dir, target, simulate)?;
    let total_units = queue.units.len();
    let blocked = queue.blocked;
    let warnings = queue.warnings;

    let waves = group_into_waves(queue.units);

    Ok(RunPlan {
        waves,
        total_units,
        blocked,
        warnings,
    })
}

/// Group a flat list of ready units into dependency-ordered waves.
///
/// Wave 0 has no deps on other units in the set.
/// Wave N depends only on units in waves 0..N-1.
///
/// The full `all_units` slice is passed to `is_unit_ready` so that
/// sibling produces/requires resolution works correctly across waves.
fn group_into_waves(units: Vec<ReadyUnit>) -> Vec<RunWave> {
    let mut waves: Vec<RunWave> = Vec::new();
    let all_units = units.clone();
    let unit_ids: HashSet<String> = units.iter().map(|u| u.id.clone()).collect();

    let mut completed: HashSet<String> = HashSet::new();
    let mut remaining: Vec<ReadyUnit> = units;

    while !remaining.is_empty() {
        let (ready, blocked): (Vec<ReadyUnit>, Vec<ReadyUnit>) = remaining
            .into_iter()
            .partition(|u| is_unit_ready(u, &completed, &unit_ids, &all_units));

        if ready.is_empty() {
            // Cycle or unresolvable deps — add remaining as a final wave
            let mut leftover = blocked;
            let weights = compute_downstream_weights(&leftover);
            sort_units(&mut leftover, &weights);
            waves.push(RunWave { units: leftover });
            break;
        }

        for u in &ready {
            completed.insert(u.id.clone());
        }

        // Sort wave by global weights (not just within the wave) for consistent ordering
        let weights = compute_downstream_weights(&all_units);
        let mut wave_units = ready;
        sort_units(&mut wave_units, &weights);
        waves.push(RunWave { units: wave_units });
        remaining = blocked;
    }

    waves
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unit::UnitType;
    use std::collections::HashSet;

    fn make_unit(id: &str, deps: Vec<&str>, produces: Vec<&str>, requires: Vec<&str>) -> ReadyUnit {
        ReadyUnit {
            id: id.to_string(),
            title: format!("Unit {}", id),
            priority: 2,
            critical_path_weight: 1,
            paths: vec![],
            produces: produces.into_iter().map(|s| s.to_string()).collect(),
            requires: requires.into_iter().map(|s| s.to_string()).collect(),
            dependencies: deps.into_iter().map(|s| s.to_string()).collect(),
            parent: Some("parent".to_string()),
            verify_fast: None,
            verify_command: None,
            retry: RunRetryContext {
                attempt_number: 0,
                previous_failure: None,
                previous_notes: Vec::new(),
            },
            model: None,
        }
    }

    // -- all_deps_closed tests --

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
            kind: crate::unit::UnitType::Task,
            feature: false,
            has_decisions: false,
        }
    }

    #[test]
    fn all_deps_closed_archived_dep_satisfied() {
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
    fn all_deps_closed_missing_dep_unsatisfied() {
        let entry_a = make_index_entry("A", Status::Open, vec!["B"], None, vec![], vec![]);
        let index = Index {
            units: vec![entry_a.clone()],
        };
        let archive = ArchiveIndex { units: vec![] };
        assert!(!all_deps_closed(&entry_a, &index, &archive));
    }

    #[test]
    fn all_deps_closed_active_closed_dep_satisfied() {
        let entry_a = make_index_entry("A", Status::Open, vec!["B"], None, vec![], vec![]);
        let entry_b = make_index_entry("B", Status::Closed, vec![], None, vec![], vec![]);
        let index = Index {
            units: vec![entry_a.clone(), entry_b],
        };
        let archive = ArchiveIndex { units: vec![] };
        assert!(all_deps_closed(&entry_a, &index, &archive));
    }

    #[test]
    fn all_deps_closed_active_open_dep_unsatisfied() {
        let entry_a = make_index_entry("A", Status::Open, vec!["B"], None, vec![], vec![]);
        let entry_b = make_index_entry("B", Status::Open, vec![], None, vec![], vec![]);
        let index = Index {
            units: vec![entry_a.clone(), entry_b],
        };
        let archive = ArchiveIndex { units: vec![] };
        assert!(!all_deps_closed(&entry_a, &index, &archive));
    }

    // -- compute_downstream_weights tests --

    #[test]
    fn unresolved_decisions_become_canonical_blocked_reason() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");
        std::fs::create_dir(&mana_dir).unwrap();

        crate::config::Config {
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
        }
        .save(&mana_dir)
        .unwrap();

        let mut unit = Unit::new("2", "Dispatchable task with unresolved decisions");
        unit.kind = UnitType::Task;
        unit.verify = Some("cargo test unresolved_decision_blocker".to_string());
        unit.decisions = vec![
            "JWT or sessions?".to_string(),
            "Which provider should be the default?".to_string(),
        ];
        unit.to_file(mana_dir.join("2-dispatchable-task-with-unresolved-decisions.md"))
            .unwrap();

        let queue = compute_ready_queue(&mana_dir, &RunTarget::AllReady, false).unwrap();
        assert!(queue.units.is_empty());
        assert_eq!(queue.blocked.len(), 1);
        assert_eq!(queue.blocked[0].id, "2");
        assert_eq!(queue.blocked[0].reason, "unresolved_decision");
        assert_eq!(
            queue.blocked[0].blocker,
            Some(AutonomyBlockerCode::UnresolvedDecision)
        );
        assert_eq!(
            queue.blocked[0].decisions,
            vec![
                "JWT or sessions?".to_string(),
                "Which provider should be the default?".to_string(),
            ]
        );

        let simulated = compute_ready_queue(&mana_dir, &RunTarget::AllReady, true).unwrap();
        assert_eq!(simulated.units.len(), 1);
        assert!(simulated.blocked.is_empty());
    }

    #[test]
    fn run_only_dispatches_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");
        std::fs::create_dir(&mana_dir).unwrap();

        crate::config::Config {
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
        }
        .save(&mana_dir)
        .unwrap();

        let mut epic = Unit::new("1", "Epic parent");
        epic.kind = UnitType::Epic;
        epic.verify = Some("cargo test should_not_dispatch_epic".to_string());
        epic.to_file(mana_dir.join("1-epic-parent.md")).unwrap();

        let mut task = Unit::new("2", "Dispatchable task");
        task.kind = UnitType::Task;
        task.verify = Some("cargo test dispatchable_task".to_string());
        task.to_file(mana_dir.join("2-dispatchable-task.md"))
            .unwrap();

        let queue = compute_ready_queue(&mana_dir, &RunTarget::AllReady, false).unwrap();
        assert_eq!(queue.units.len(), 1);
        assert_eq!(queue.units[0].id, "2");
    }

    #[test]
    fn weights_single_unit() {
        let units = vec![make_unit("A", vec![], vec![], vec![])];
        let weights = compute_downstream_weights(&units);
        assert_eq!(weights.get("A").copied(), Some(1));
    }

    #[test]
    fn weights_linear_chain() {
        let units = vec![
            make_unit("A", vec![], vec![], vec![]),
            make_unit("B", vec!["A"], vec![], vec![]),
            make_unit("C", vec!["B"], vec![], vec![]),
        ];
        let weights = compute_downstream_weights(&units);
        assert_eq!(weights.get("A").copied(), Some(3));
        assert_eq!(weights.get("B").copied(), Some(2));
        assert_eq!(weights.get("C").copied(), Some(1));
    }

    #[test]
    fn weights_diamond() {
        let units = vec![
            make_unit("A", vec![], vec![], vec![]),
            make_unit("B", vec!["A"], vec![], vec![]),
            make_unit("C", vec!["A"], vec![], vec![]),
            make_unit("D", vec!["B", "C"], vec![], vec![]),
        ];
        let weights = compute_downstream_weights(&units);
        assert_eq!(weights.get("D").copied(), Some(1));
        assert_eq!(weights.get("B").copied(), Some(2));
        assert_eq!(weights.get("C").copied(), Some(2));
        assert_eq!(weights.get("A").copied(), Some(4));
    }

    // -- is_unit_ready tests --

    #[test]
    fn unit_ready_no_deps() {
        let unit = make_unit("1", vec![], vec![], vec![]);
        let all = vec![unit.clone()];
        let ids: HashSet<String> = all.iter().map(|u| u.id.clone()).collect();
        assert!(is_unit_ready(&unit, &HashSet::new(), &ids, &all));
    }

    #[test]
    fn unit_not_ready_dep_not_completed() {
        let unit = make_unit("2", vec!["1"], vec![], vec![]);
        let dep = make_unit("1", vec![], vec![], vec![]);
        let all = vec![dep, unit.clone()];
        let ids: HashSet<String> = all.iter().map(|u| u.id.clone()).collect();
        assert!(!is_unit_ready(&unit, &HashSet::new(), &ids, &all));
    }

    #[test]
    fn unit_ready_dep_completed() {
        let unit = make_unit("2", vec!["1"], vec![], vec![]);
        let dep = make_unit("1", vec![], vec![], vec![]);
        let all = vec![dep, unit.clone()];
        let ids: HashSet<String> = all.iter().map(|u| u.id.clone()).collect();
        let mut completed = HashSet::new();
        completed.insert("1".to_string());
        assert!(is_unit_ready(&unit, &completed, &ids, &all));
    }

    #[test]
    fn unit_ready_dep_outside_dispatch_set() {
        let unit = make_unit("2", vec!["external"], vec![], vec![]);
        let all = vec![unit.clone()];
        let ids: HashSet<String> = all.iter().map(|u| u.id.clone()).collect();
        // "external" is not in ids → treated as satisfied
        assert!(is_unit_ready(&unit, &HashSet::new(), &ids, &all));
    }

    // -- sort_units tests --

    #[test]
    fn sort_units_by_priority_then_weight() {
        let mut units = vec![
            {
                let mut u = make_unit("B", vec![], vec![], vec![]);
                u.priority = 2;
                u.critical_path_weight = 3;
                u
            },
            {
                let mut u = make_unit("A", vec![], vec![], vec![]);
                u.priority = 1;
                u.critical_path_weight = 1;
                u
            },
        ];
        let weights: HashMap<String, u32> = [("A".to_string(), 1), ("B".to_string(), 3)]
            .into_iter()
            .collect();
        sort_units(&mut units, &weights);
        // Priority 1 before priority 2
        assert_eq!(units[0].id, "A");
        assert_eq!(units[1].id, "B");
    }

    #[test]
    fn sort_units_same_priority_higher_weight_first() {
        let mut units = vec![
            {
                let mut u = make_unit("A", vec![], vec![], vec![]);
                u.priority = 2;
                u.critical_path_weight = 1;
                u
            },
            {
                let mut u = make_unit("B", vec![], vec![], vec![]);
                u.priority = 2;
                u.critical_path_weight = 5;
                u
            },
        ];
        let weights: HashMap<String, u32> = [("A".to_string(), 1), ("B".to_string(), 5)]
            .into_iter()
            .collect();
        sort_units(&mut units, &weights);
        // Higher weight first (B before A)
        assert_eq!(units[0].id, "B");
        assert_eq!(units[1].id, "A");
    }
}
