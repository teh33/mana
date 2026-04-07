use std::collections::HashSet;
use std::path::Path;

use anyhow::{anyhow, Result};
use serde::Serialize;

use mana_core::ops::context::summarize_child_units;

use crate::discovery::find_unit_file;
use crate::index::Index;
use crate::unit::{AttemptOutcome, Status, Unit};

// ---------------------------------------------------------------------------
// Output types (text + JSON)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct TraceOutput {
    pub unit: UnitSummary,
    pub parent_chain: Vec<UnitSummary>,
    pub children: Vec<UnitSummary>,
    pub child_summaries: Vec<ChildSummaryOutput>,
    pub dependencies: Vec<UnitSummary>,
    pub dependents: Vec<UnitSummary>,
    pub produces: Vec<String>,
    pub requires: Vec<String>,
    pub attempts: AttemptSummary,
}

#[derive(Debug, Serialize)]
pub struct UnitSummary {
    pub id: String,
    pub title: String,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct ChildSummaryOutput {
    pub id: String,
    pub title: String,
    pub status: String,
    pub recent_outcome: Option<String>,
    pub summary: Option<String>,
    pub follow_up: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AttemptSummary {
    pub total: usize,
    pub successful: usize,
    pub failed: usize,
    pub abandoned: usize,
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

/// Handle `mana trace <id>` command.
///
/// Walks the unit graph from the given unit: parent chain up to root,
/// direct children, dependencies (what this unit waits on), dependents
/// (what waits on this unit), produces/requires artifacts, and attempt history.
pub fn cmd_trace(id: &str, json: bool, mana_dir: &Path) -> Result<()> {
    let index = Index::load_or_rebuild(mana_dir)?;

    let entry = index
        .units
        .iter()
        .find(|e| e.id == id)
        .ok_or_else(|| anyhow!("Unit {} not found", id))?;

    // Load full unit for attempt log and tokens
    let unit_path = find_unit_file(mana_dir, id)?;
    let unit = Unit::from_file(&unit_path)?;

    // Build reverse graph: dep_id -> list of unit IDs that depend on it
    let mut dependents_map: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for e in &index.units {
        for dep in &e.dependencies {
            dependents_map
                .entry(dep.clone())
                .or_default()
                .push(e.id.clone());
        }
    }

    // --- Parent chain (up to root, cycle-safe) ---
    let parent_chain = collect_parent_chain(&index, &entry.parent, &mut HashSet::new());

    // --- Direct children ---
    let children: Vec<UnitSummary> = index
        .units
        .iter()
        .filter(|e| e.parent.as_deref() == Some(id))
        .map(|e| unit_summary(e.id.clone(), e.title.clone(), &e.status))
        .collect();

    // --- Dependencies (what this unit waits on) ---
    let child_summaries = summarize_child_units(mana_dir, id)
        .into_iter()
        .map(|child| ChildSummaryOutput {
            id: child.id,
            title: child.title,
            status: child.status,
            recent_outcome: child.recent_outcome,
            summary: child.summary,
            follow_up: child.follow_up,
        })
        .collect();

    // --- Dependencies (what this unit waits on) ---
    let dependencies: Vec<UnitSummary> = entry
        .dependencies
        .iter()
        .filter_map(|dep_id| {
            index
                .units
                .iter()
                .find(|e| &e.id == dep_id)
                .map(|e| unit_summary(e.id.clone(), e.title.clone(), &e.status))
        })
        .collect();

    // --- Dependents (what waits on this unit) ---
    let dependents: Vec<UnitSummary> = dependents_map
        .get(id)
        .map(|ids| {
            ids.iter()
                .filter_map(|dep_id| {
                    index
                        .units
                        .iter()
                        .find(|e| &e.id == dep_id)
                        .map(|e| unit_summary(e.id.clone(), e.title.clone(), &e.status))
                })
                .collect()
        })
        .unwrap_or_default();

    // --- Attempt summary ---
    let attempts = build_attempt_summary(&unit);

    // --- Build output ---
    let this_summary = unit_summary(entry.id.clone(), entry.title.clone(), &entry.status);

    let output = TraceOutput {
        unit: this_summary,
        parent_chain,
        children,
        child_summaries,
        dependencies,
        dependents,
        produces: entry.produces.clone(),
        requires: entry.requires.clone(),
        attempts,
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        print_trace(&output);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn collect_parent_chain(
    index: &Index,
    parent_id: &Option<String>,
    visited: &mut HashSet<String>,
) -> Vec<UnitSummary> {
    let Some(pid) = parent_id else {
        return vec![];
    };

    // Guard against circular references (shouldn't happen but don't crash)
    if visited.contains(pid) {
        return vec![];
    }
    visited.insert(pid.clone());

    if let Some(entry) = index.units.iter().find(|e| &e.id == pid) {
        let mut chain = vec![unit_summary(
            entry.id.clone(),
            entry.title.clone(),
            &entry.status,
        )];
        chain.extend(collect_parent_chain(index, &entry.parent, visited));
        chain
    } else {
        vec![]
    }
}

fn unit_summary(id: String, title: String, status: &Status) -> UnitSummary {
    UnitSummary {
        id,
        title,
        status: status.to_string(),
    }
}

fn build_attempt_summary(unit: &Unit) -> AttemptSummary {
    let total = unit.attempt_log.len();
    let successful = unit
        .attempt_log
        .iter()
        .filter(|a| matches!(a.outcome, AttemptOutcome::Success))
        .count();
    let failed = unit
        .attempt_log
        .iter()
        .filter(|a| matches!(a.outcome, AttemptOutcome::Failed))
        .count();
    let abandoned = unit
        .attempt_log
        .iter()
        .filter(|a| matches!(a.outcome, AttemptOutcome::Abandoned))
        .count();

    AttemptSummary {
        total,
        successful,
        failed,
        abandoned,
    }
}

fn status_indicator(status: &str) -> &str {
    match status {
        "closed" => "✓",
        "in_progress" => "⚡",
        _ => "○",
    }
}

fn print_trace(output: &TraceOutput) {
    let b = &output.unit;
    println!("Unit {}: \"{}\" [{}]", b.id, b.title, b.status);

    // Parent chain
    if output.parent_chain.is_empty() {
        println!("  Parent: (root)");
    } else {
        let mut indent = "  ".to_string();
        for parent in &output.parent_chain {
            println!(
                "{}Parent: {} {} \"{}\" [{}]",
                indent,
                status_indicator(&parent.status),
                parent.id,
                parent.title,
                parent.status
            );
            indent.push_str("  ");
        }
        println!("{}Parent: (root)", indent);
    }

    // Children
    if !output.children.is_empty() {
        println!("  Children:");
        for child in &output.children {
            println!(
                "    {} {} \"{}\" [{}]",
                status_indicator(&child.status),
                child.id,
                child.title,
                child.status
            );
        }
    }

    if !output.child_summaries.is_empty() {
        println!("  Child summaries:");
        for child in &output.child_summaries {
            let mut line = format!("    {} {} \"{}\" [{}]",
                status_indicator(&child.status),
                child.id,
                child.title,
                child.status
            );
            if let Some(outcome) = &child.recent_outcome {
                line.push_str(&format!(" recent={}", outcome));
            }
            println!("{}", line);
            if let Some(summary) = &child.summary {
                println!("      summary: {}", summary);
            }
            if let Some(follow_up) = &child.follow_up {
                println!("      follow-up: {}", follow_up);
            }
        }
    }

    // Dependencies
    if output.dependencies.is_empty() {
        println!("  Dependencies: (none)");
    } else {
        println!("  Dependencies:");
        for dep in &output.dependencies {
            println!(
                "    → {} {} \"{}\" [{}]",
                status_indicator(&dep.status),
                dep.id,
                dep.title,
                dep.status
            );
        }
    }

    // Dependents
    if output.dependents.is_empty() {
        println!("  Dependents: (none)");
    } else {
        println!("  Dependents:");
        for dep in &output.dependents {
            println!(
                "    ← {} {} \"{}\" [{}]",
                status_indicator(&dep.status),
                dep.id,
                dep.title,
                dep.status
            );
        }
    }

    // Produces / Requires
    if output.produces.is_empty() {
        println!("  Produces: (none)");
    } else {
        println!("  Produces: {}", output.produces.join(", "));
    }

    if output.requires.is_empty() {
        println!("  Requires: (none)");
    } else {
        println!("  Requires: {}", output.requires.join(", "));
    }

    // Attempts
    let a = &output.attempts;
    if a.total == 0 {
        println!("  Attempts: (none)");
    } else {
        println!(
            "  Attempts: {} total ({} success, {} failed, {} abandoned)",
            a.total, a.successful, a.failed, a.abandoned
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unit::{AttemptOutcome, AttemptRecord, Unit};
    use tempfile::TempDir;

    /// Write a unit as a legacy `.yaml` file so `find_unit_file` can locate it.
    fn write_unit(mana_dir: &Path, unit: &Unit) {
        let path = mana_dir.join(format!("{}.yaml", unit.id));
        unit.to_file(&path).expect("write unit file");
    }

    #[test]
    fn test_trace_no_parent_no_deps() {
        let tmp = TempDir::new().unwrap();
        let mana_dir = tmp.path();

        let mut unit = Unit::new("42", "test unit");
        unit.produces = vec!["artifact-a".to_string()];
        unit.attempt_log = vec![AttemptRecord {
            num: 1,
            outcome: AttemptOutcome::Abandoned,
            notes: None,
            agent: None,
            started_at: None,
            finished_at: None,
        }];
        write_unit(mana_dir, &unit);

        // Index is rebuilt from unit files
        let result = cmd_trace("42", false, mana_dir);
        assert!(result.is_ok(), "cmd_trace failed: {:?}", result);
    }

    #[test]
    fn test_trace_json_output() {
        let tmp = TempDir::new().unwrap();
        let mana_dir = tmp.path();

        let unit = Unit::new("1", "root unit");
        write_unit(mana_dir, &unit);

        let result = cmd_trace("1", true, mana_dir);
        assert!(result.is_ok(), "cmd_trace --json failed: {:?}", result);
    }

    #[test]
    fn test_trace_with_parent_and_deps() {
        let tmp = TempDir::new().unwrap();
        let mana_dir = tmp.path();

        // Parent unit
        let parent_unit = Unit::new("10", "parent task");
        write_unit(mana_dir, &parent_unit);

        // Dependency unit
        let mut dep_unit = Unit::new("11", "dep task");
        dep_unit.status = Status::Closed;
        write_unit(mana_dir, &dep_unit);

        // Main unit with parent, deps, produces, requires, attempts
        let mut main_unit = Unit::new("12", "main task");
        main_unit.parent = Some("10".to_string());
        main_unit.dependencies = vec!["11".to_string()];
        main_unit.produces = vec!["api.rs".to_string()];
        main_unit.requires = vec!["Config".to_string()];
        main_unit.attempt_log = vec![
            AttemptRecord {
                num: 1,
                outcome: AttemptOutcome::Failed,
                notes: None,
                agent: None,
                started_at: None,
                finished_at: None,
            },
            AttemptRecord {
                num: 2,
                outcome: AttemptOutcome::Success,
                notes: None,
                agent: None,
                started_at: None,
                finished_at: None,
            },
        ];
        write_unit(mana_dir, &main_unit);

        let result = cmd_trace("12", false, mana_dir);
        assert!(
            result.is_ok(),
            "cmd_trace with parent/deps failed: {:?}",
            result
        );
    }

    #[test]
    fn trace_output_includes_child_summaries() {
        let tmp = TempDir::new().unwrap();
        let mana_dir = tmp.path();

        let parent = Unit::new("20", "parent");
        write_unit(mana_dir, &parent);

        let mut child = Unit::new("20.1", "child");
        child.parent = Some("20".to_string());
        child.status = Status::Open;
        child.verify = Some("cargo test child".to_string());
        child.notes = Some("Found useful child detail".to_string());
        child.attempt_log = vec![AttemptRecord {
            num: 1,
            outcome: AttemptOutcome::Failed,
            notes: Some("latest child attempt failed".to_string()),
            agent: None,
            started_at: None,
            finished_at: None,
        }];
        write_unit(mana_dir, &child);

        let index = Index::load_or_rebuild(mana_dir).unwrap();
        let entry = index.units.iter().find(|e| e.id == "20").unwrap();
        let unit_path = find_unit_file(mana_dir, "20").unwrap();
        let unit = Unit::from_file(&unit_path).unwrap();

        let output = TraceOutput {
            unit: unit_summary(entry.id.clone(), entry.title.clone(), &entry.status),
            parent_chain: vec![],
            children: index
                .units
                .iter()
                .filter(|e| e.parent.as_deref() == Some("20"))
                .map(|e| unit_summary(e.id.clone(), e.title.clone(), &e.status))
                .collect(),
            child_summaries: summarize_child_units(mana_dir, "20")
                .into_iter()
                .map(|child| ChildSummaryOutput {
                    id: child.id,
                    title: child.title,
                    status: child.status,
                    recent_outcome: child.recent_outcome,
                    summary: child.summary,
                    follow_up: child.follow_up,
                })
                .collect(),
            dependencies: vec![],
            dependents: vec![],
            produces: unit.produces.clone(),
            requires: unit.requires.clone(),
            attempts: build_attempt_summary(&unit),
        };

        assert_eq!(output.child_summaries.len(), 1);
        assert_eq!(output.child_summaries[0].id, "20.1");
        assert_eq!(output.child_summaries[0].recent_outcome.as_deref(), Some("failed"));
        assert!(output.child_summaries[0]
            .summary
            .as_deref()
            .unwrap()
            .contains("Found useful child detail"));
    }

    #[test]
    fn test_trace_not_found() {
        let tmp = TempDir::new().unwrap();
        let mana_dir = tmp.path();

        // Empty directory — no units
        let result = cmd_trace("999", false, mana_dir);
        assert!(result.is_err(), "Should error for missing unit");
    }
}
