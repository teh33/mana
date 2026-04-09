use std::path::Path;

use anyhow::Result;

use crate::blocking::{check_blocked_with_archive, check_scope_warning, BlockReason, ScopeWarning};
use crate::config::Config;
use crate::index::{ArchiveIndex, Index, IndexEntry};
use crate::stream::{self, StreamEvent};
use crate::unit::{AttemptOutcome, Status};

use mana_pool::RetryContext;

use super::ready_queue::all_deps_closed;
use super::wave::{
    compute_critical_path, compute_downstream_weights, compute_effective_parallelism,
    compute_file_conflicts, compute_waves, Wave,
};
use super::{RunTarget, UnitAction};

/// A unit ready for dispatch.
#[derive(Debug, Clone)]
pub struct SizedUnit {
    pub id: String,
    pub title: String,
    pub action: UnitAction,
    pub priority: u8,
    pub dependencies: Vec<String>,
    pub parent: Option<String>,
    pub produces: Vec<String>,
    pub requires: Vec<String>,
    pub paths: Vec<String>,
    /// Optional fast verify command to run before the full verify gate.
    pub verify_fast: Option<String>,
    /// Deferred verify command for grouped post-agent verification.
    pub verify_command: Option<String>,
    /// Retry context derived from the unit's attempt history.
    pub retry: RetryContext,
    /// Per-unit model override from frontmatter.
    pub model: Option<String>,
}

/// A unit that was excluded from dispatch due to scope issues.
#[derive(Debug, Clone)]
pub struct BlockedUnit {
    pub id: String,
    pub title: String,
    pub reason: BlockReason,
}

/// Result from planning dispatch.
pub struct DispatchPlan {
    pub waves: Vec<Wave>,
    pub skipped: Vec<BlockedUnit>,
    /// Scope warnings for units that will dispatch but have large scope.
    pub warnings: Vec<(String, ScopeWarning)>,
    /// Flat list of all units to dispatch (for ready-queue mode).
    pub all_units: Vec<SizedUnit>,
    /// The index snapshot used for planning.
    pub index: Index,
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

/// Plan dispatch: get ready units, filter by scope, compute waves.
pub(super) fn plan_dispatch(
    mana_dir: &Path,
    _config: &Config,
    target: &RunTarget,
    simulate: bool,
) -> Result<DispatchPlan> {
    let index = Index::load_or_rebuild(mana_dir)?;
    let archive = ArchiveIndex::load_or_rebuild(mana_dir)
        .unwrap_or_else(|_| ArchiveIndex { units: Vec::new() });

    // Get candidate units: open with verify.
    // In simulate mode (dry-run), include all open units with verify — even those
    // whose deps aren't met yet — so compute_waves can show the full execution plan.
    // In normal mode, only include units whose deps are already closed.
    let mut candidate_entries: Vec<&IndexEntry> = index
        .units
        .iter()
        .filter(|e| {
            e.has_verify
                && e.status == Status::Open
                && (simulate || all_deps_closed(e, &index, &archive))
        })
        .collect();

    // Exclude units that have open descendant units — they're parent/feature
    // containers and shouldn't be dispatched while children are still pending.
    candidate_entries.retain(|entry| !has_open_descendants(&index, &entry.id));

    // Filter by target if provided
    if !matches!(target, RunTarget::AllReady) {
        candidate_entries.retain(|entry| matches_target(&index, entry, target));
    }

    // Partition into dispatchable vs blocked.
    // In simulate mode, skip blocking checks — we want to show the full plan.
    // In normal mode, dependency blocking is already handled by all_deps_closed above,
    // but check_blocked catches edge cases (e.g., missing deps not in index).
    // Scope warnings (oversized) are non-blocking — units dispatch with a warning.
    let mut dispatch_units: Vec<SizedUnit> = Vec::new();
    let mut skipped: Vec<BlockedUnit> = Vec::new();
    let mut warnings: Vec<(String, ScopeWarning)> = Vec::new();

    for entry in &candidate_entries {
        if !simulate {
            if let Some(reason) = check_blocked_with_archive(entry, &index, Some(&archive)) {
                skipped.push(BlockedUnit {
                    id: entry.id.clone(),
                    title: entry.title.clone(),
                    reason,
                });
                continue;
            }
        }
        // Check for scope warnings (non-blocking)
        if let Some(warning) = check_scope_warning(entry) {
            warnings.push((entry.id.clone(), warning));
        }
        let unit_path = crate::discovery::find_unit_file(mana_dir, &entry.id)?;
        let unit = crate::unit::Unit::from_file(&unit_path)?;

        let retry = RetryContext {
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
        };

        dispatch_units.push(SizedUnit {
            id: entry.id.clone(),
            title: entry.title.clone(),
            action: UnitAction::Implement,
            priority: entry.priority,
            dependencies: entry.dependencies.clone(),
            parent: entry.parent.clone(),
            produces: entry.produces.clone(),
            requires: entry.requires.clone(),
            paths: entry.paths.clone(),
            verify_fast: unit.verify_fast.clone(),
            verify_command: unit.verify.clone(),
            retry,
            model: unit.model.clone(),
        });
    }

    let waves = compute_waves(&dispatch_units, &index);

    Ok(DispatchPlan {
        waves,
        skipped,
        warnings,
        all_units: dispatch_units,
        index,
    })
}

/// Print the dispatch plan without executing.
pub(super) fn print_plan(plan: &DispatchPlan, config_run_model: Option<&str>) {
    let weights = compute_downstream_weights(&plan.all_units);
    let critical_path = compute_critical_path(&plan.all_units);
    let critical_set: std::collections::HashSet<&str> =
        critical_path.iter().map(|s| s.as_str()).collect();

    // Critical path summary at the top
    if critical_path.len() > 1 {
        println!(
            "Critical path: {} ({} steps)",
            critical_path.join(" → "),
            critical_path.len()
        );
        println!();
    }

    for (wave_idx, wave) in plan.waves.iter().enumerate() {
        let eff_par = compute_effective_parallelism(&wave.units);
        let par_note = if eff_par < wave.units.len() {
            format!(", effective concurrency: {}/{}", eff_par, wave.units.len())
        } else {
            String::new()
        };
        println!(
            "Wave {}: {} unit(s){}",
            wave_idx + 1,
            wave.units.len(),
            par_note
        );

        // Precompute file conflicts for this wave so we can annotate per-unit
        let wave_conflicts = compute_file_conflicts(&wave.units);

        for sb in &wave.units {
            let weight = weights.get(&sb.id).copied().unwrap_or(1);
            let weight_note = if weight > 1 {
                format!("  [weight: {}]", weight)
            } else {
                String::new()
            };
            let critical_note = if critical_set.contains(sb.id.as_str()) && critical_path.len() > 1
            {
                "  ⚡ critical"
            } else {
                ""
            };
            // Collect conflicts for this unit: other units sharing a file in this wave
            let mut conflict_parts: Vec<String> = Vec::new();
            for (file, ids) in &wave_conflicts {
                if ids.contains(&sb.id) {
                    for other_id in ids {
                        if other_id != &sb.id {
                            conflict_parts.push(format!("{} ({})", other_id, file));
                        }
                    }
                }
            }
            let conflict_str = if conflict_parts.is_empty() {
                String::new()
            } else {
                format!("  ⊘ conflicts: {}", conflict_parts.join(", "))
            };
            let warning = plan
                .warnings
                .iter()
                .find(|(id, _)| id == &sb.id)
                .map(|(_, w)| format!("  ⚠ {}", w))
                .unwrap_or_default();
            let model_note = sb
                .model
                .as_deref()
                .or(config_run_model)
                .map(|model| format!("  [model: {}]", model))
                .unwrap_or_default();
            println!(
                "  {}  {}  {}{}{}{}{}{}",
                sb.id,
                sb.title,
                sb.action,
                model_note,
                weight_note,
                critical_note,
                conflict_str,
                warning
            );
        }
    }

    if !plan.skipped.is_empty() {
        println!();
        println!("Blocked ({}):", plan.skipped.len());
        for bb in &plan.skipped {
            println!("  ⚠ {}  {}  ({})", bb.id, bb.title, bb.reason);
        }
    }
}

/// Print the dispatch plan as JSON stream events.
pub(super) fn print_plan_json(
    plan: &DispatchPlan,
    target: &RunTarget,
    runtime: Option<stream::RunRuntimeInfo>,
) {
    let parent_id = target.scope_label();
    let critical_path = compute_critical_path(&plan.all_units);
    let rounds: Vec<stream::RoundPlan> = plan
        .waves
        .iter()
        .enumerate()
        .map(|(i, wave)| {
            let eff_par = compute_effective_parallelism(&wave.units);
            let conflicts = compute_file_conflicts(&wave.units);
            let effective_concurrency = if eff_par < wave.units.len() {
                Some(eff_par)
            } else {
                None
            };
            stream::RoundPlan {
                round: i + 1,
                units: wave
                    .units
                    .iter()
                    .map(|b| stream::UnitInfo {
                        id: b.id.clone(),
                        title: b.title.clone(),
                        round: i + 1,
                    })
                    .collect(),
                effective_concurrency,
                conflicts,
            }
        })
        .collect();

    stream::emit(&StreamEvent::DryRun {
        parent_id,
        rounds,
        critical_path,
        runtime,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
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

    #[test]
    fn plan_dispatch_excludes_parents_with_open_children_from_ready_set() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, Some("echo {id}"));

        let mut parent = crate::unit::Unit::new("1", "Parent");
        parent.verify = Some("echo ok".to_string());
        parent.paths = vec!["src/parent.rs".to_string()];
        parent.to_file(mana_dir.join("1-parent.md")).unwrap();

        let mut child = crate::unit::Unit::new("1.1", "Child");
        child.parent = Some("1".to_string());
        child.verify = Some("echo ok".to_string());
        child.paths = vec!["src/child.rs".to_string()];
        child.to_file(mana_dir.join("1.1-child.md")).unwrap();

        let config = Config::load_with_extends(&mana_dir).unwrap();
        let plan = plan_dispatch(&mana_dir, &config, &RunTarget::AllReady, false).unwrap();

        assert_eq!(plan.waves.len(), 1);
        let dispatched_ids: Vec<&str> = plan.waves[0]
            .units
            .iter()
            .map(|unit| unit.id.as_str())
            .collect();
        assert_eq!(dispatched_ids, vec!["1.1"]);
    }

    #[test]
    fn plan_dispatch_no_ready_units() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, Some("echo {id}"));

        let config = Config::load_with_extends(&mana_dir).unwrap();
        let plan = plan_dispatch(&mana_dir, &config, &RunTarget::AllReady, false).unwrap();

        assert!(plan.waves.is_empty());
        assert!(plan.skipped.is_empty());
    }

    #[test]
    fn plan_dispatch_returns_ready_units() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, Some("echo {id}"));

        let mut unit = crate::unit::Unit::new("1", "Task one");
        unit.verify = Some("echo ok".to_string());
        unit.produces = vec!["X".to_string()];
        unit.paths = vec!["src/x.rs".to_string()];
        unit.to_file(mana_dir.join("1-task-one.md")).unwrap();

        let mut unit2 = crate::unit::Unit::new("2", "Task two");
        unit2.verify = Some("echo ok".to_string());
        unit2.produces = vec!["Y".to_string()];
        unit2.paths = vec!["src/y.rs".to_string()];
        unit2.to_file(mana_dir.join("2-task-two.md")).unwrap();

        let config = Config::load_with_extends(&mana_dir).unwrap();
        let plan = plan_dispatch(&mana_dir, &config, &RunTarget::AllReady, false).unwrap();

        assert_eq!(plan.waves.len(), 1);
        assert_eq!(plan.waves[0].units.len(), 2);
    }

    #[test]
    fn plan_dispatch_filters_by_id() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, Some("echo {id}"));

        let mut unit = crate::unit::Unit::new("1", "Task one");
        unit.verify = Some("echo ok".to_string());
        unit.produces = vec!["X".to_string()];
        unit.paths = vec!["src/x.rs".to_string()];
        unit.to_file(mana_dir.join("1-task-one.md")).unwrap();

        let mut unit2 = crate::unit::Unit::new("2", "Task two");
        unit2.verify = Some("echo ok".to_string());
        unit2.produces = vec!["Y".to_string()];
        unit2.paths = vec!["src/y.rs".to_string()];
        unit2.to_file(mana_dir.join("2-task-two.md")).unwrap();

        let config = Config::load_with_extends(&mana_dir).unwrap();
        let plan =
            plan_dispatch(&mana_dir, &config, &RunTarget::Unit("1".to_string()), false).unwrap();

        assert_eq!(plan.waves.len(), 1);
        assert_eq!(plan.waves[0].units.len(), 1);
        assert_eq!(plan.waves[0].units[0].id, "1");
    }

    #[test]
    fn plan_dispatch_includes_unit_model_override() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, Some("echo {id}"));

        let mut unit = crate::unit::Unit::new("1", "Task one");
        unit.verify = Some("echo ok".to_string());
        unit.model = Some("opus".to_string());
        unit.to_file(mana_dir.join("1-task-one.md")).unwrap();

        let config = Config::load_with_extends(&mana_dir).unwrap();
        let plan =
            plan_dispatch(&mana_dir, &config, &RunTarget::Unit("1".to_string()), false).unwrap();

        assert_eq!(plan.waves.len(), 1);
        assert_eq!(plan.waves[0].units[0].model.as_deref(), Some("opus"));
    }

    #[test]
    fn plan_dispatch_parent_id_gets_children() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, Some("echo {id}"));

        let parent = crate::unit::Unit::new("1", "Parent");
        parent.to_file(mana_dir.join("1-parent.md")).unwrap();

        let mut child1 = crate::unit::Unit::new("1.1", "Child one");
        child1.parent = Some("1".to_string());
        child1.verify = Some("echo ok".to_string());
        child1.produces = vec!["A".to_string()];
        child1.paths = vec!["src/a.rs".to_string()];
        child1.to_file(mana_dir.join("1.1-child-one.md")).unwrap();

        let mut child2 = crate::unit::Unit::new("1.2", "Child two");
        child2.parent = Some("1".to_string());
        child2.verify = Some("echo ok".to_string());
        child2.produces = vec!["B".to_string()];
        child2.paths = vec!["src/b.rs".to_string()];
        child2.to_file(mana_dir.join("1.2-child-two.md")).unwrap();

        let config = Config::load_with_extends(&mana_dir).unwrap();
        let plan =
            plan_dispatch(&mana_dir, &config, &RunTarget::Unit("1".to_string()), false).unwrap();

        assert_eq!(plan.waves.len(), 1);
        assert_eq!(plan.waves[0].units.len(), 2);
    }

    #[test]
    fn plan_dispatch_target_parent_recurses_to_ready_leaf_descendants() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, Some("echo {id}"));

        let parent = crate::unit::Unit::new("1", "Parent");
        parent.to_file(mana_dir.join("1-parent.md")).unwrap();

        let mut intermediate = crate::unit::Unit::new("1.1", "Intermediate parent");
        intermediate.parent = Some("1".to_string());
        intermediate.verify = Some("echo ok".to_string());
        intermediate.paths = vec!["src/intermediate.rs".to_string()];
        intermediate
            .to_file(mana_dir.join("1.1-intermediate-parent.md"))
            .unwrap();

        let mut nested_leaf = crate::unit::Unit::new("1.1.1", "Nested leaf");
        nested_leaf.parent = Some("1.1".to_string());
        nested_leaf.verify = Some("echo ok".to_string());
        nested_leaf.paths = vec!["src/nested.rs".to_string()];
        nested_leaf
            .to_file(mana_dir.join("1.1.1-nested-leaf.md"))
            .unwrap();

        let mut sibling_leaf = crate::unit::Unit::new("1.2", "Sibling leaf");
        sibling_leaf.parent = Some("1".to_string());
        sibling_leaf.verify = Some("echo ok".to_string());
        sibling_leaf.paths = vec!["src/sibling.rs".to_string()];
        sibling_leaf
            .to_file(mana_dir.join("1.2-sibling-leaf.md"))
            .unwrap();

        let config = Config::load_with_extends(&mana_dir).unwrap();
        let plan =
            plan_dispatch(&mana_dir, &config, &RunTarget::Unit("1".to_string()), false).unwrap();

        assert_eq!(plan.waves.len(), 1);
        let dispatched_ids: Vec<&str> = plan.waves[0]
            .units
            .iter()
            .map(|unit| unit.id.as_str())
            .collect();
        assert_eq!(dispatched_ids, vec!["1.1.1", "1.2"]);
    }

    #[test]
    fn plan_dispatch_explicit_target_set_unions_and_deduplicates() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, Some("echo {id}"));

        let parent = crate::unit::Unit::new("1", "Parent");
        parent.to_file(mana_dir.join("1-parent.md")).unwrap();

        let mut nested_parent = crate::unit::Unit::new("1.1", "Nested parent");
        nested_parent.parent = Some("1".to_string());
        nested_parent.verify = Some("echo ok".to_string());
        nested_parent
            .to_file(mana_dir.join("1.1-nested-parent.md"))
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
        let plan = plan_dispatch(
            &mana_dir,
            &config,
            &RunTarget::Explicit(vec!["1".to_string(), "1.1.1".to_string()]),
            false,
        )
        .unwrap();

        assert_eq!(plan.waves.len(), 1);
        let dispatched_ids: Vec<&str> = plan.waves[0]
            .units
            .iter()
            .map(|unit| unit.id.as_str())
            .collect();
        assert_eq!(dispatched_ids, vec!["1.1.1", "1.2"]);
    }

    #[test]
    fn oversized_unit_dispatched_with_warning() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, Some("echo {id}"));

        let mut unit = crate::unit::Unit::new("1", "Oversized unit");
        unit.verify = Some("echo ok".to_string());
        // 4 produces exceeds MAX_PRODUCES (3) — warning but not blocked
        unit.produces = vec![
            "A".to_string(),
            "B".to_string(),
            "C".to_string(),
            "D".to_string(),
        ];
        unit.paths = vec!["src/a.rs".to_string()];
        unit.to_file(mana_dir.join("1-oversized.md")).unwrap();

        let config = Config::load_with_extends(&mana_dir).unwrap();
        let plan = plan_dispatch(&mana_dir, &config, &RunTarget::AllReady, false).unwrap();

        assert_eq!(plan.waves.len(), 1);
        assert_eq!(plan.waves[0].units.len(), 1);
        assert!(plan.skipped.is_empty());
        assert_eq!(plan.warnings.len(), 1);
        assert_eq!(plan.warnings[0].0, "1");
    }

    #[test]
    fn unscoped_unit_dispatched_normally() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, Some("echo {id}"));

        let mut unit = crate::unit::Unit::new("1", "Unscoped unit");
        unit.verify = Some("echo ok".to_string());
        // No produces, no paths — dispatched normally
        unit.to_file(mana_dir.join("1-unscoped.md")).unwrap();

        let config = Config::load_with_extends(&mana_dir).unwrap();
        let plan = plan_dispatch(&mana_dir, &config, &RunTarget::AllReady, false).unwrap();

        assert_eq!(plan.waves.len(), 1);
        assert_eq!(plan.waves[0].units.len(), 1);
        assert!(plan.skipped.is_empty());
        assert!(plan.warnings.is_empty());
    }

    #[test]
    fn well_scoped_unit_dispatched() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, Some("echo {id}"));

        let mut unit = crate::unit::Unit::new("1", "Well scoped");
        unit.verify = Some("echo ok".to_string());
        unit.produces = vec!["Widget".to_string()];
        unit.paths = vec!["src/widget.rs".to_string()];
        unit.to_file(mana_dir.join("1-well-scoped.md")).unwrap();

        let config = Config::load_with_extends(&mana_dir).unwrap();
        let plan = plan_dispatch(&mana_dir, &config, &RunTarget::AllReady, false).unwrap();

        assert_eq!(plan.waves.len(), 1);
        assert_eq!(plan.waves[0].units.len(), 1);
        assert!(plan.skipped.is_empty());
    }

    #[test]
    fn dry_run_simulate_shows_all_waves() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, Some("echo {id}"));

        // Create a chain: 1.1 → 1.2 → 1.3 (parent=1)
        let parent = crate::unit::Unit::new("1", "Parent");
        parent.to_file(mana_dir.join("1-parent.md")).unwrap();

        let mut a = crate::unit::Unit::new("1.1", "Step A");
        a.parent = Some("1".to_string());
        a.verify = Some("echo ok".to_string());
        a.produces = vec!["A".to_string()];
        a.paths = vec!["src/a.rs".to_string()];
        a.to_file(mana_dir.join("1.1-step-a.md")).unwrap();

        let mut b = crate::unit::Unit::new("1.2", "Step B");
        b.parent = Some("1".to_string());
        b.verify = Some("echo ok".to_string());
        b.dependencies = vec!["1.1".to_string()];
        b.produces = vec!["B".to_string()];
        b.paths = vec!["src/b.rs".to_string()];
        b.to_file(mana_dir.join("1.2-step-b.md")).unwrap();

        let mut c = crate::unit::Unit::new("1.3", "Step C");
        c.parent = Some("1".to_string());
        c.verify = Some("echo ok".to_string());
        c.dependencies = vec!["1.2".to_string()];
        c.produces = vec!["C".to_string()];
        c.paths = vec!["src/c.rs".to_string()];
        c.to_file(mana_dir.join("1.3-step-c.md")).unwrap();

        // Without simulate: only wave 1 (1.1) is ready
        let config = Config::load_with_extends(&mana_dir).unwrap();
        let plan =
            plan_dispatch(&mana_dir, &config, &RunTarget::Unit("1".to_string()), false).unwrap();
        assert_eq!(plan.waves.len(), 1);
        assert_eq!(plan.waves[0].units.len(), 1);
        assert_eq!(plan.waves[0].units[0].id, "1.1");

        // With simulate: all 3 waves shown
        let plan =
            plan_dispatch(&mana_dir, &config, &RunTarget::Unit("1".to_string()), true).unwrap();
        assert_eq!(plan.waves.len(), 3);
        assert_eq!(plan.waves[0].units[0].id, "1.1");
        assert_eq!(plan.waves[1].units[0].id, "1.2");
        assert_eq!(plan.waves[2].units[0].id, "1.3");
    }

    #[test]
    fn dry_run_simulate_respects_produces_requires() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, Some("echo {id}"));

        let parent = crate::unit::Unit::new("1", "Parent");
        parent.to_file(mana_dir.join("1-parent.md")).unwrap();

        let mut a = crate::unit::Unit::new("1.1", "Types");
        a.parent = Some("1".to_string());
        a.verify = Some("echo ok".to_string());
        a.produces = vec!["types".to_string()];
        a.paths = vec!["src/types.rs".to_string()];
        a.to_file(mana_dir.join("1.1-types.md")).unwrap();

        let mut b = crate::unit::Unit::new("1.2", "Impl");
        b.parent = Some("1".to_string());
        b.verify = Some("echo ok".to_string());
        b.requires = vec!["types".to_string()];
        b.produces = vec!["impl".to_string()];
        b.paths = vec!["src/impl.rs".to_string()];
        b.to_file(mana_dir.join("1.2-impl.md")).unwrap();

        // Without simulate: only 1.1 is ready (1.2 blocked on requires)
        let config = Config::load_with_extends(&mana_dir).unwrap();
        let plan =
            plan_dispatch(&mana_dir, &config, &RunTarget::Unit("1".to_string()), false).unwrap();
        assert_eq!(plan.waves.len(), 1);
        assert_eq!(plan.waves[0].units[0].id, "1.1");

        // With simulate: both shown in correct wave order
        let plan =
            plan_dispatch(&mana_dir, &config, &RunTarget::Unit("1".to_string()), true).unwrap();
        assert_eq!(plan.waves.len(), 2);
        assert_eq!(plan.waves[0].units[0].id, "1.1");
        assert_eq!(plan.waves[1].units[0].id, "1.2");
    }

    #[test]
    fn plan_dispatch_sorts_wave_by_downstream_weight() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, Some("echo {id}"));

        let parent = crate::unit::Unit::new("1", "Parent");
        parent.to_file(mana_dir.join("1-parent.md")).unwrap();

        // A has no dependents (weight 1)
        let mut a = crate::unit::Unit::new("1.1", "A leaf");
        a.parent = Some("1".to_string());
        a.verify = Some("echo ok".to_string());
        a.paths = vec!["src/a.rs".to_string()];
        a.to_file(mana_dir.join("1.1-a-leaf.md")).unwrap();

        // B has two dependents D, E (weight 3)
        let mut b = crate::unit::Unit::new("1.2", "B root");
        b.parent = Some("1".to_string());
        b.verify = Some("echo ok".to_string());
        b.paths = vec!["src/b.rs".to_string()];
        b.to_file(mana_dir.join("1.2-b-root.md")).unwrap();

        // C has one dependent F (weight 2)
        let mut c = crate::unit::Unit::new("1.3", "C mid");
        c.parent = Some("1".to_string());
        c.verify = Some("echo ok".to_string());
        c.paths = vec!["src/c.rs".to_string()];
        c.to_file(mana_dir.join("1.3-c-mid.md")).unwrap();

        // D depends on B
        let mut d = crate::unit::Unit::new("1.4", "D dep B");
        d.parent = Some("1".to_string());
        d.verify = Some("echo ok".to_string());
        d.dependencies = vec!["1.2".to_string()];
        d.paths = vec!["src/d.rs".to_string()];
        d.to_file(mana_dir.join("1.4-d.md")).unwrap();

        // E depends on B
        let mut e = crate::unit::Unit::new("1.5", "E dep B");
        e.parent = Some("1".to_string());
        e.verify = Some("echo ok".to_string());
        e.dependencies = vec!["1.2".to_string()];
        e.paths = vec!["src/e.rs".to_string()];
        e.to_file(mana_dir.join("1.5-e.md")).unwrap();

        // F depends on C
        let mut f = crate::unit::Unit::new("1.6", "F dep C");
        f.parent = Some("1".to_string());
        f.verify = Some("echo ok".to_string());
        f.dependencies = vec!["1.3".to_string()];
        f.paths = vec!["src/f.rs".to_string()];
        f.to_file(mana_dir.join("1.6-f.md")).unwrap();

        // Simulate dry-run: shows all waves
        let config = Config::load_with_extends(&mana_dir).unwrap();
        let plan =
            plan_dispatch(&mana_dir, &config, &RunTarget::Unit("1".to_string()), true).unwrap();

        // Wave 1 should be: B(weight 3), C(weight 2), A(weight 1)
        assert_eq!(plan.waves[0].units.len(), 3);
        assert_eq!(plan.waves[0].units[0].id, "1.2"); // B — weight 3
        assert_eq!(plan.waves[0].units[1].id, "1.3"); // C — weight 2
        assert_eq!(plan.waves[0].units[2].id, "1.1"); // A — weight 1
    }

    #[test]
    fn plan_dispatch_file_conflict_in_wave() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, Some("echo {id}"));

        // Two units in the same wave that share a file
        let mut a = crate::unit::Unit::new("1", "Touches lib");
        a.verify = Some("echo ok".to_string());
        a.paths = vec!["src/lib.rs".to_string(), "src/a.rs".to_string()];
        a.to_file(mana_dir.join("1-touches-lib.md")).unwrap();

        let mut b = crate::unit::Unit::new("2", "Also lib");
        b.verify = Some("echo ok".to_string());
        b.paths = vec!["src/lib.rs".to_string(), "src/b.rs".to_string()];
        b.to_file(mana_dir.join("2-also-lib.md")).unwrap();

        let mut c = crate::unit::Unit::new("3", "Independent");
        c.verify = Some("echo ok".to_string());
        c.paths = vec!["src/c.rs".to_string()];
        c.to_file(mana_dir.join("3-independent.md")).unwrap();

        let config = Config::load_with_extends(&mana_dir).unwrap();
        let plan = plan_dispatch(&mana_dir, &config, &RunTarget::AllReady, false).unwrap();

        // All 3 in wave 1 (no deps)
        assert_eq!(plan.waves.len(), 1);
        assert_eq!(plan.waves[0].units.len(), 3);

        // Verify file conflict detection works on the wave
        let conflicts = super::super::wave::compute_file_conflicts(&plan.waves[0].units);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].0, "src/lib.rs");

        // Effective parallelism: 2 (A+C or B+C, not A+B)
        let eff = super::super::wave::compute_effective_parallelism(&plan.waves[0].units);
        assert_eq!(eff, 2);
    }

    #[test]
    fn print_plan_shows_critical_path() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, Some("echo {id}"));

        let parent = crate::unit::Unit::new("1", "Parent");
        parent.to_file(mana_dir.join("1-parent.md")).unwrap();

        // Chain: 1.1 → 1.2 (critical path has len 2)
        let mut a = crate::unit::Unit::new("1.1", "Step A");
        a.parent = Some("1".to_string());
        a.verify = Some("echo ok".to_string());
        a.paths = vec!["src/a.rs".to_string()];
        a.to_file(mana_dir.join("1.1-step-a.md")).unwrap();

        let mut b = crate::unit::Unit::new("1.2", "Step B");
        b.parent = Some("1".to_string());
        b.verify = Some("echo ok".to_string());
        b.dependencies = vec!["1.1".to_string()];
        b.paths = vec!["src/b.rs".to_string()];
        b.to_file(mana_dir.join("1.2-step-b.md")).unwrap();

        let config = Config::load_with_extends(&mana_dir).unwrap();
        let plan =
            plan_dispatch(&mana_dir, &config, &RunTarget::Unit("1".to_string()), true).unwrap();

        // The critical path computed from the plan must include both 1.1 and 1.2
        let critical_path = compute_critical_path(&plan.all_units);
        assert!(
            critical_path.len() >= 2,
            "expected critical path of length >= 2, got {:?}",
            critical_path
        );
        assert!(
            critical_path.contains(&"1.1".to_string()),
            "expected 1.1 in critical path"
        );
        assert!(
            critical_path.contains(&"1.2".to_string()),
            "expected 1.2 in critical path"
        );
    }

    #[test]
    fn print_plan_shows_file_conflicts() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, Some("echo {id}"));

        // Two units sharing src/lib.rs
        let mut a = crate::unit::Unit::new("1", "Alpha");
        a.verify = Some("echo ok".to_string());
        a.paths = vec!["src/lib.rs".to_string()];
        a.to_file(mana_dir.join("1-alpha.md")).unwrap();

        let mut b = crate::unit::Unit::new("2", "Beta");
        b.verify = Some("echo ok".to_string());
        b.paths = vec!["src/lib.rs".to_string()];
        b.to_file(mana_dir.join("2-beta.md")).unwrap();

        let config = Config::load_with_extends(&mana_dir).unwrap();
        let plan = plan_dispatch(&mana_dir, &config, &RunTarget::AllReady, false).unwrap();

        // Both in wave 1; confirm conflict is detected
        assert_eq!(plan.waves.len(), 1);
        let conflicts = compute_file_conflicts(&plan.waves[0].units);
        assert_eq!(conflicts.len(), 1, "expected one conflict group");
        assert_eq!(conflicts[0].0, "src/lib.rs");
        assert!(conflicts[0].1.contains(&"1".to_string()));
        assert!(conflicts[0].1.contains(&"2".to_string()));
    }

    #[test]
    fn print_plan_shows_effective_concurrency() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, Some("echo {id}"));

        // Three units: 1 and 2 share a file, 3 is independent
        let mut a = crate::unit::Unit::new("1", "Conflict A");
        a.verify = Some("echo ok".to_string());
        a.paths = vec!["src/shared.rs".to_string()];
        a.to_file(mana_dir.join("1-conflict-a.md")).unwrap();

        let mut b = crate::unit::Unit::new("2", "Conflict B");
        b.verify = Some("echo ok".to_string());
        b.paths = vec!["src/shared.rs".to_string()];
        b.to_file(mana_dir.join("2-conflict-b.md")).unwrap();

        let mut c = crate::unit::Unit::new("3", "Independent");
        c.verify = Some("echo ok".to_string());
        c.paths = vec!["src/other.rs".to_string()];
        c.to_file(mana_dir.join("3-independent.md")).unwrap();

        let config = Config::load_with_extends(&mana_dir).unwrap();
        let plan = plan_dispatch(&mana_dir, &config, &RunTarget::AllReady, false).unwrap();

        assert_eq!(plan.waves.len(), 1);
        assert_eq!(plan.waves[0].units.len(), 3);

        // Effective concurrency must be less than 3 due to the file conflict
        let eff = compute_effective_parallelism(&plan.waves[0].units);
        assert!(eff < 3, "expected effective concurrency < 3, got {}", eff);
        assert!(eff >= 2, "expected effective concurrency >= 2, got {}", eff);
    }

    #[test]
    fn print_plan_no_conflicts_shows_full_concurrency() {
        let (_dir, mana_dir) = make_mana_dir();
        write_config(&mana_dir, Some("echo {id}"));

        // Three units with no shared files — full concurrency
        let mut a = crate::unit::Unit::new("1", "A");
        a.verify = Some("echo ok".to_string());
        a.paths = vec!["src/a.rs".to_string()];
        a.to_file(mana_dir.join("1-a.md")).unwrap();

        let mut b = crate::unit::Unit::new("2", "B");
        b.verify = Some("echo ok".to_string());
        b.paths = vec!["src/b.rs".to_string()];
        b.to_file(mana_dir.join("2-b.md")).unwrap();

        let mut c = crate::unit::Unit::new("3", "C");
        c.verify = Some("echo ok".to_string());
        c.paths = vec!["src/c.rs".to_string()];
        c.to_file(mana_dir.join("3-c.md")).unwrap();

        let config = Config::load_with_extends(&mana_dir).unwrap();
        let plan = plan_dispatch(&mana_dir, &config, &RunTarget::AllReady, false).unwrap();

        assert_eq!(plan.waves.len(), 1);
        assert_eq!(plan.waves[0].units.len(), 3);

        // No conflicts — effective concurrency equals unit count
        let eff = compute_effective_parallelism(&plan.waves[0].units);
        assert_eq!(eff, 3, "expected full concurrency of 3, got {}", eff);
    }
}
