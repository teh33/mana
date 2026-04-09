use std::fs;
use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::index::Index;
use crate::unit::{RunResult, Status, Unit};

/// Cost and token statistics aggregated from RunRecord history.
#[derive(Debug, Serialize)]
pub struct CostStats {
    pub total_tokens: u64,
    pub total_cost: f64,
    pub avg_tokens_per_unit: f64,
    /// Rate at which closed units passed on their first attempt (0.0–1.0).
    pub first_pass_rate: f64,
    /// Rate at which attempted units eventually closed (0.0–1.0).
    pub overall_pass_rate: f64,
    pub most_expensive_unit: Option<UnitRef>,
    pub most_retried_unit: Option<UnitRef>,
    pub units_with_history: usize,
}

/// Lightweight unit reference for reporting.
#[derive(Debug, Serialize)]
pub struct UnitRef {
    pub id: String,
    pub title: String,
    pub value: u64,
}

/// Project statistics snapshot.
#[derive(Debug, Serialize)]
pub struct StatsResult {
    pub total: usize,
    pub open: usize,
    pub in_progress: usize,
    pub closed: usize,
    pub blocked: usize,
    pub completion_pct: f64,
    pub priority_counts: [usize; 5],
    pub cost: Option<CostStats>,
}

/// Load all units from disk (non-recursive, skips non-unit files).
fn load_all_units(mana_dir: &Path) -> Vec<Unit> {
    let Ok(entries) = fs::read_dir(mana_dir) else {
        return vec![];
    };
    let mut units = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if !(filename.ends_with(".yaml") || filename.ends_with(".md")) {
            continue;
        }
        if let Ok(unit) = Unit::from_file(&path) {
            units.push(unit);
        }
    }
    units
}

/// Aggregate cost/token statistics from unit history.
pub fn aggregate_cost(units: &[Unit]) -> Option<CostStats> {
    let mut total_tokens: u64 = 0;
    let mut total_cost: f64 = 0.0;
    let mut units_with_history: usize = 0;
    let mut closed_with_history: usize = 0;
    let mut first_pass_count: usize = 0;
    let mut attempted: usize = 0;
    let mut closed_count: usize = 0;
    let mut most_expensive: Option<(&Unit, u64)> = None;
    let mut most_retried: Option<(&Unit, usize)> = None;

    for unit in units {
        if unit.history.is_empty() {
            continue;
        }

        units_with_history += 1;
        attempted += 1;

        if unit.status == Status::Closed {
            closed_count += 1;
        }

        let unit_tokens: u64 = unit.history.iter().filter_map(|r| r.tokens).sum();
        let unit_cost: f64 = unit.history.iter().filter_map(|r| r.cost).sum();

        total_tokens += unit_tokens;
        total_cost += unit_cost;

        if unit.status == Status::Closed {
            closed_with_history += 1;
            if unit
                .history
                .first()
                .map(|r| r.result == RunResult::Pass)
                .unwrap_or(false)
            {
                first_pass_count += 1;
            }
        }

        if unit_tokens > 0 && most_expensive.is_none_or(|(_, t)| unit_tokens > t) {
            most_expensive = Some((unit, unit_tokens));
        }

        let attempt_count = unit.history.len();
        if attempt_count > 1 && most_retried.is_none_or(|(_, c)| attempt_count > c) {
            most_retried = Some((unit, attempt_count));
        }
    }

    if units_with_history == 0 {
        return None;
    }

    let avg_tokens_per_unit = if units_with_history > 0 {
        total_tokens as f64 / units_with_history as f64
    } else {
        0.0
    };

    let first_pass_rate = if closed_with_history > 0 {
        first_pass_count as f64 / closed_with_history as f64
    } else {
        0.0
    };

    let overall_pass_rate = if attempted > 0 {
        closed_count as f64 / attempted as f64
    } else {
        0.0
    };

    Some(CostStats {
        total_tokens,
        total_cost,
        avg_tokens_per_unit,
        first_pass_rate,
        overall_pass_rate,
        most_expensive_unit: most_expensive.map(|(b, tokens)| UnitRef {
            id: b.id.clone(),
            title: b.title.clone(),
            value: tokens,
        }),
        most_retried_unit: most_retried.map(|(b, count)| UnitRef {
            id: b.id.clone(),
            title: b.title.clone(),
            value: count as u64,
        }),
        units_with_history,
    })
}

/// Compute project statistics: counts by status, priority, cost metrics.
pub fn stats(mana_dir: &Path) -> Result<StatsResult> {
    let index = Index::load_or_rebuild(mana_dir)?;

    let total = index.units.len();
    let open = index
        .units
        .iter()
        .filter(|e| e.status == Status::Open)
        .count();
    let in_progress = index
        .units
        .iter()
        .filter(|e| e.status == Status::InProgress)
        .count();
    let closed = index
        .units
        .iter()
        .filter(|e| e.status == Status::Closed)
        .count();

    let blocked = index
        .units
        .iter()
        .filter(|e| {
            if e.status != Status::Open {
                return false;
            }
            for dep_id in &e.dependencies {
                if let Some(dep) = index.units.iter().find(|d| &d.id == dep_id) {
                    if dep.status != Status::Closed {
                        return true;
                    }
                } else {
                    return true;
                }
            }
            false
        })
        .count();

    let mut priority_counts = [0usize; 5];
    for entry in &index.units {
        if (entry.priority as usize) < 5 {
            priority_counts[entry.priority as usize] += 1;
        }
    }

    let completion_pct = if total > 0 {
        (closed as f64 / total as f64) * 100.0
    } else {
        0.0
    };

    let all_units = load_all_units(mana_dir);
    let cost = aggregate_cost(&all_units);

    Ok(StatsResult {
        total,
        open,
        in_progress,
        closed,
        blocked,
        completion_pct,
        priority_counts,
        cost,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unit::{RunRecord, RunResult, Unit};
    use chrono::Utc;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_units() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        let mut b1 = Unit::new("1", "Open P0");
        b1.priority = 0;
        let mut b2 = Unit::new("2", "In Progress P1");
        b2.status = Status::InProgress;
        b2.priority = 1;
        let mut b3 = Unit::new("3", "Closed P2");
        b3.status = Status::Closed;
        b3.priority = 2;

        b1.to_file(mana_dir.join("1.yaml")).unwrap();
        b2.to_file(mana_dir.join("2.yaml")).unwrap();
        b3.to_file(mana_dir.join("3.yaml")).unwrap();

        (dir, mana_dir)
    }

    #[test]
    fn stats_computes_counts() {
        let (_dir, mana_dir) = setup_test_units();

        let result = stats(&mana_dir).unwrap();

        assert_eq!(result.total, 3);
        assert_eq!(result.open, 1);
        assert_eq!(result.in_progress, 1);
        assert_eq!(result.closed, 1);
    }

    #[test]
    fn stats_empty_project() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        let result = stats(&mana_dir).unwrap();

        assert_eq!(result.total, 0);
        assert_eq!(result.completion_pct, 0.0);
    }

    #[test]
    fn aggregate_cost_no_history() {
        let units = vec![Unit::new("1", "No history")];
        let result = aggregate_cost(&units);
        assert!(result.is_none());
    }

    #[test]
    fn aggregate_cost_with_history() {
        let mut unit = Unit::new("1", "With history");
        unit.status = Status::Closed;
        unit.history = vec![RunRecord {
            attempt: 1,
            started_at: Utc::now(),
            finished_at: None,
            duration_secs: None,
            agent: None,
            result: RunResult::Pass,
            exit_code: Some(0),
            tokens: Some(1000),
            cost: Some(0.05),
            output_snippet: None,
            autonomy_observation: None,
        }];

        let stats = aggregate_cost(&[unit]).unwrap();
        assert_eq!(stats.total_tokens, 1000);
        assert!((stats.total_cost - 0.05).abs() < 1e-9);
        assert!((stats.first_pass_rate - 1.0).abs() < 1e-9);
    }
}
