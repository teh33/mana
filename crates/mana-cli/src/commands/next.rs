use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Result;
use chrono::Utc;
use serde::Serialize;

use crate::blocking::check_blocked;
use crate::index::{Index, IndexEntry};
use crate::unit::{Status, UnitType};

/// A scored unit with metadata for display.
#[derive(Debug, Serialize)]
pub struct ScoredUnit {
    pub id: String,
    pub title: String,
    pub priority: u8,
    pub score: f64,
    /// IDs of units this one unblocks (directly depends-on this unit).
    pub unblocks: Vec<String>,
    /// Age in days since creation.
    pub age_days: u64,
    /// Number of verify attempts so far.
    pub attempts: u32,
}

/// Score a ready unit based on the ranking criteria.
///
/// Higher score = should be worked on first.
///
/// Components:
/// 1. Priority weight (P0 = 50, P1 = 40, P2 = 30, P3 = 20, P4 = 10)
/// 2. Dependency depth — how many other units does this unblock (transitive)
/// 3. Age in days (capped at 30 to prevent runaway scores)
/// 4. Fewer attempts = higher score (fresh units preferred)
fn score_unit(entry: &IndexEntry, unblock_count: usize) -> f64 {
    // Priority: P0=50, P1=40, P2=30, P3=20, P4=10
    let priority_score = (5u8.saturating_sub(entry.priority)) as f64 * 10.0;

    // Dependency depth: each unit unblocked adds 5 points (capped at 50)
    let unblock_score = (unblock_count as f64 * 5.0).min(50.0);

    // Age: 1 point per day, capped at 30
    let age_days = Utc::now()
        .signed_duration_since(entry.created_at)
        .num_days()
        .max(0) as f64;
    let age_score = age_days.min(30.0);

    // Attempts: penalize 3 points per attempt (capped at 15)
    let attempt_penalty = (entry.attempts as f64 * 3.0).min(15.0);

    priority_score + unblock_score + age_score - attempt_penalty
}

/// Count how many units a given unit transitively unblocks.
///
/// Walks the reverse dependency graph from `unit_id` counting all
/// units that are (transitively) waiting on this unit.
fn count_transitive_unblocks(unit_id: &str, reverse_deps: &HashMap<String, Vec<String>>) -> usize {
    let mut visited = HashSet::new();
    let mut stack = vec![unit_id.to_string()];

    while let Some(current) = stack.pop() {
        if let Some(dependents) = reverse_deps.get(&current) {
            for dep in dependents {
                if visited.insert(dep.clone()) {
                    stack.push(dep.clone());
                }
            }
        }
    }

    visited.len()
}

/// Get direct unblock IDs (units that directly depend on this one).
fn direct_unblocks(unit_id: &str, reverse_deps: &HashMap<String, Vec<String>>) -> Vec<String> {
    reverse_deps.get(unit_id).cloned().unwrap_or_default()
}

/// Pick the top N recommended units to work on next.
pub fn cmd_next(n: usize, json: bool, mana_dir: &Path) -> Result<()> {
    let index = Index::load_or_rebuild(mana_dir)?;

    // Find ready units: open, has verify, not blocked, not a feature
    let ready: Vec<&IndexEntry> = index
        .units
        .iter()
        .filter(|e| {
            e.status == Status::Open
                && e.kind == UnitType::Task
                && e.has_verify
                && !e.feature
                && check_blocked(e, &index).is_none()
        })
        .collect();

    if ready.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("No ready units. Create one with: mana create \"task\" --verify \"cmd\"");
        }
        return Ok(());
    }

    // Build reverse dependency map: unit_id -> list of units that depend on it
    let mut reverse_deps: HashMap<String, Vec<String>> = HashMap::new();
    for entry in &index.units {
        for dep_id in &entry.dependencies {
            reverse_deps
                .entry(dep_id.clone())
                .or_default()
                .push(entry.id.clone());
        }
    }

    // Score and sort
    let mut scored: Vec<ScoredUnit> = ready
        .iter()
        .map(|entry| {
            let transitive_count = count_transitive_unblocks(&entry.id, &reverse_deps);
            let unblocks = direct_unblocks(&entry.id, &reverse_deps);
            let score = score_unit(entry, transitive_count);
            let age_days = Utc::now()
                .signed_duration_since(entry.created_at)
                .num_days()
                .max(0) as u64;

            ScoredUnit {
                id: entry.id.clone(),
                title: entry.title.clone(),
                priority: entry.priority,
                score,
                unblocks,
                age_days,
                attempts: entry.attempts,
            }
        })
        .collect();

    // Sort by score descending
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Take top N
    scored.truncate(n);

    if json {
        let json_str = serde_json::to_string_pretty(&scored)?;
        println!("{}", json_str);
    } else {
        for unit in &scored {
            let priority_label = format!("P{}", unit.priority);
            println!("{}  {:.1}  {}", priority_label, unit.score, unit.title);

            if !unit.unblocks.is_empty() {
                println!("      Unblocks: {}", unit.unblocks.join(", "));
            }

            let attempts_str = if unit.attempts > 0 {
                format!(" | Attempts: {}", unit.attempts)
            } else {
                String::new()
            };

            println!(
                "      ID: {} | Age: {} days{}",
                unit.id, unit.age_days, attempts_str
            );
            println!();
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    fn make_entry(id: &str, priority: u8) -> IndexEntry {
        IndexEntry {
            handle: None,
            id: id.to_string(),
            title: format!("Unit {}", id),
            status: Status::Open,
            priority,
            parent: None,
            dependencies: vec![],
            labels: vec![],
            assignee: None,
            updated_at: Utc::now(),
            produces: vec![],
            requires: vec![],
            has_verify: true,
            verify: None,
            created_at: Utc::now(),
            claimed_by: None,
            attempts: 0,
            paths: vec![],
            kind: crate::unit::UnitType::Task,
            feature: false,
            has_decisions: false,
        }
    }

    #[test]
    fn next_only_recommends_jobs() {
        let index = Index {
            units: vec![
                IndexEntry {
                    id: "1".to_string(),
                    title: "Epic".to_string(),
                    handle: None,
                    status: Status::Open,
                    priority: 1,
                    parent: None,
                    dependencies: vec![],
                    labels: vec![],
                    assignee: None,
                    updated_at: Utc::now(),
                    produces: vec![],
                    requires: vec![],
                    has_verify: true,
                    verify: Some("echo nope".to_string()),
                    created_at: Utc::now(),
                    claimed_by: None,
                    attempts: 0,
                    paths: vec![],
                    kind: UnitType::Epic,
                    feature: false,
                    has_decisions: false,
                },
                make_entry("2", 0),
            ],
        };

        let ready: Vec<&IndexEntry> = index
            .units
            .iter()
            .filter(|e| {
                e.status == Status::Open
                    && e.kind == UnitType::Task
                    && e.has_verify
                    && !e.feature
                    && check_blocked(e, &index).is_none()
            })
            .collect();

        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "2");
    }

    #[test]
    fn higher_priority_scores_higher() {
        let p0 = make_entry("1", 0);
        let p2 = make_entry("2", 2);
        let p4 = make_entry("3", 4);

        let reverse_deps = HashMap::new();

        let s0 = score_unit(&p0, count_transitive_unblocks("1", &reverse_deps));
        let s2 = score_unit(&p2, count_transitive_unblocks("2", &reverse_deps));
        let s4 = score_unit(&p4, count_transitive_unblocks("3", &reverse_deps));

        assert!(s0 > s2, "P0 ({}) should score higher than P2 ({})", s0, s2);
        assert!(s2 > s4, "P2 ({}) should score higher than P4 ({})", s2, s4);
    }

    #[test]
    fn more_unblocks_scores_higher() {
        let entry = make_entry("1", 2);

        let s_none = score_unit(&entry, 0);
        let s_some = score_unit(&entry, 3);
        let s_many = score_unit(&entry, 10);

        assert!(
            s_some > s_none,
            "3 unblocks ({}) > 0 unblocks ({})",
            s_some,
            s_none
        );
        assert!(
            s_many > s_some,
            "10 unblocks ({}) > 3 unblocks ({})",
            s_many,
            s_some
        );
    }

    #[test]
    fn older_unit_scores_higher() {
        let mut old = make_entry("1", 2);
        old.created_at = Utc::now() - Duration::days(10);

        let new = make_entry("2", 2);

        let s_old = score_unit(&old, 0);
        let s_new = score_unit(&new, 0);

        assert!(
            s_old > s_new,
            "Old ({}) should score higher than new ({})",
            s_old,
            s_new
        );
    }

    #[test]
    fn more_attempts_scores_lower() {
        let fresh = make_entry("1", 2);
        let mut retried = make_entry("2", 2);
        retried.attempts = 3;

        let s_fresh = score_unit(&fresh, 0);
        let s_retried = score_unit(&retried, 0);

        assert!(
            s_fresh > s_retried,
            "Fresh ({}) should score higher than retried ({})",
            s_fresh,
            s_retried
        );
    }

    #[test]
    fn transitive_unblock_count() {
        // A -> B -> C (A unblocks B, B unblocks C, so A transitively unblocks B and C)
        let mut reverse_deps = HashMap::new();
        reverse_deps.insert("A".to_string(), vec!["B".to_string()]);
        reverse_deps.insert("B".to_string(), vec!["C".to_string()]);

        assert_eq!(count_transitive_unblocks("A", &reverse_deps), 2);
        assert_eq!(count_transitive_unblocks("B", &reverse_deps), 1);
        assert_eq!(count_transitive_unblocks("C", &reverse_deps), 0);
    }

    #[test]
    fn direct_unblocks_returns_correct_ids() {
        let mut reverse_deps = HashMap::new();
        reverse_deps.insert("A".to_string(), vec!["B".to_string(), "C".to_string()]);

        let unblocks = direct_unblocks("A", &reverse_deps);
        assert_eq!(unblocks, vec!["B".to_string(), "C".to_string()]);

        let empty = direct_unblocks("Z", &reverse_deps);
        assert!(empty.is_empty());
    }
}
