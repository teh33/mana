use std::fs;
use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::index::Index;
use crate::unit::{RunResult, Status, Unit};

// ---------------------------------------------------------------------------
// Output types (used for both text rendering and JSON serialization)
// ---------------------------------------------------------------------------

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

/// Machine-readable snapshot of all stats.
#[derive(Debug, Serialize)]
pub struct StatsOutput {
    pub total: usize,
    pub open: usize,
    pub in_progress: usize,
    pub closed: usize,
    pub blocked: usize,
    pub completion_pct: f64,
    pub priority_counts: [usize; 5],
    pub cost: Option<CostStats>,
}

// ---------------------------------------------------------------------------
// Unit file discovery
// ---------------------------------------------------------------------------

/// Returns all units loaded from YAML files in `mana_dir` (non-recursive,
/// skips files that don't look like unit files or fail to parse).
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
        if !is_unit_file(filename) {
            continue;
        }
        if let Ok(unit) = Unit::from_file(&path) {
            units.push(unit);
        }
    }
    units
}

/// Returns true for files that look like unit YAML files.
fn is_unit_file(filename: &str) -> bool {
    filename.ends_with(".yaml") || filename.ends_with(".md")
}

// ---------------------------------------------------------------------------
// Aggregation
// ---------------------------------------------------------------------------

fn aggregate_cost(units: &[Unit]) -> Option<CostStats> {
    let mut total_tokens: u64 = 0;
    let mut total_cost: f64 = 0.0;
    let mut units_with_history: usize = 0;

    // For first-pass rate: closed units where first RunRecord result is Pass
    let mut closed_with_history: usize = 0;
    let mut first_pass_count: usize = 0;

    // For overall pass rate: closed / attempted (has any history)
    let mut attempted: usize = 0;
    let mut closed_count: usize = 0;

    // For most expensive and most retried
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

        // Accumulate tokens/cost from all RunRecords
        let unit_tokens: u64 = unit.history.iter().filter_map(|r| r.tokens).sum();
        let unit_cost: f64 = unit.history.iter().filter_map(|r| r.cost).sum();

        total_tokens += unit_tokens;
        total_cost += unit_cost;

        // First-pass rate: closed units where first RunRecord is a Pass
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

        // Track most expensive (by total tokens across all attempts)
        if unit_tokens > 0 && most_expensive.is_none_or(|(_, t)| unit_tokens > t) {
            most_expensive = Some((unit, unit_tokens));
        }

        // Track most retried (by number of history entries)
        let attempt_count = unit.history.len();
        if attempt_count > 1 && most_retried.is_none_or(|(_, c)| attempt_count > c) {
            most_retried = Some((unit, attempt_count));
        }
    }

    // Don't show the section at all when nothing has been tracked
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

// ---------------------------------------------------------------------------
// Command entry point
// ---------------------------------------------------------------------------

/// Show project statistics: counts by status, priority, and completion percentage.
/// When `--json` is passed, emits machine-readable JSON instead.
pub fn cmd_stats(mana_dir: &Path, json: bool) -> Result<()> {
    let index = Index::load_or_rebuild(mana_dir)?;

    // Count by status
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

    // Count blocked (open with unresolved dependencies)
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

    // Count by priority
    let mut priority_counts = [0usize; 5];
    for entry in &index.units {
        if (entry.priority as usize) < 5 {
            priority_counts[entry.priority as usize] += 1;
        }
    }

    // Calculate completion percentage
    let completion_pct = if total > 0 {
        (closed as f64 / total as f64) * 100.0
    } else {
        0.0
    };

    // Aggregate cost/token data from full unit files
    let all_units = load_all_units(mana_dir);
    let cost = aggregate_cost(&all_units);

    if json {
        let output = StatsOutput {
            total,
            open,
            in_progress,
            closed,
            blocked,
            completion_pct,
            priority_counts,
            cost,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    // Human-readable output
    println!("=== Unit Statistics ===");
    println!();
    println!("Total:        {}", total);
    println!("Open:         {}", open);
    println!("In Progress:  {}", in_progress);
    println!("Closed:       {}", closed);
    println!("Blocked:      {}", blocked);
    println!();
    println!("Completion:   {:.1}%", completion_pct);
    println!();
    println!("By Priority:");
    println!("  P0: {}", priority_counts[0]);
    println!("  P1: {}", priority_counts[1]);
    println!("  P2: {}", priority_counts[2]);
    println!("  P3: {}", priority_counts[3]);
    println!("  P4: {}", priority_counts[4]);

    if let Some(c) = &cost {
        println!();
        println!("=== Tokens & Cost ===");
        println!();
        println!("Units tracked:    {}", c.units_with_history);
        println!("Total tokens:     {}", c.total_tokens);
        if c.total_cost > 0.0 {
            println!("Total cost:       ${:.4}", c.total_cost);
        }
        println!("Avg tokens/unit:  {:.0}", c.avg_tokens_per_unit);
        println!();
        println!("First-pass rate:  {:.1}%", c.first_pass_rate * 100.0);
        println!("Overall pass rate:{:.1}%", c.overall_pass_rate * 100.0);
        if let Some(ref unit) = c.most_expensive_unit {
            println!();
            println!(
                "Most expensive:   {} — {} ({} tokens)",
                unit.id, unit.title, unit.value
            );
        }
        if let Some(ref unit) = c.most_retried_unit {
            println!(
                "Most retried:     {} — {} ({} attempts)",
                unit.id, unit.title, unit.value
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unit::Unit;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_units() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create units with different statuses and priorities
        let mut b1 = Unit::new("1", "Open P0");
        b1.priority = 0;

        let mut b2 = Unit::new("2", "In Progress P1");
        b2.status = Status::InProgress;
        b2.priority = 1;

        let mut b3 = Unit::new("3", "Closed P2");
        b3.status = Status::Closed;
        b3.priority = 2;

        let mut b4 = Unit::new("4", "Open P3");
        b4.priority = 3;

        let mut b5 = Unit::new("5", "Open depends on 1");
        b5.dependencies = vec!["1".to_string()];

        b1.to_file(mana_dir.join("1.yaml")).unwrap();
        b2.to_file(mana_dir.join("2.yaml")).unwrap();
        b3.to_file(mana_dir.join("3.yaml")).unwrap();
        b4.to_file(mana_dir.join("4.yaml")).unwrap();
        b5.to_file(mana_dir.join("5.yaml")).unwrap();

        (dir, mana_dir)
    }

    #[test]
    fn stats_calculates_counts() {
        let (_dir, mana_dir) = setup_test_units();
        let index = Index::load_or_rebuild(&mana_dir).unwrap();

        // Verify counts
        assert_eq!(
            index
                .units
                .iter()
                .filter(|e| e.status == Status::Open)
                .count(),
            3
        ); // 1, 4, 5
        assert_eq!(
            index
                .units
                .iter()
                .filter(|e| e.status == Status::InProgress)
                .count(),
            1
        ); // 2
        assert_eq!(
            index
                .units
                .iter()
                .filter(|e| e.status == Status::Closed)
                .count(),
            1
        ); // 3
    }

    #[test]
    fn stats_command_works() {
        let (_dir, mana_dir) = setup_test_units();
        let result = cmd_stats(&mana_dir, false);
        assert!(result.is_ok());
    }

    #[test]
    fn stats_command_json() {
        let (_dir, mana_dir) = setup_test_units();
        let result = cmd_stats(&mana_dir, true);
        assert!(result.is_ok());
    }

    #[test]
    fn empty_project() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        let result = cmd_stats(&mana_dir, false);
        assert!(result.is_ok());
    }

    #[test]
    fn aggregate_cost_no_history() {
        let units = vec![Unit::new("1", "No history")];
        let result = aggregate_cost(&units);
        assert!(
            result.is_none(),
            "Should return None when no units have history"
        );
    }

    #[test]
    fn aggregate_cost_with_history() {
        use crate::unit::{RunRecord, RunResult};
        use chrono::Utc;

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
        assert_eq!(stats.units_with_history, 1);
        assert!((stats.first_pass_rate - 1.0).abs() < 1e-9);
        assert!((stats.overall_pass_rate - 1.0).abs() < 1e-9);
    }

    #[test]
    fn aggregate_cost_most_expensive_and_retried() {
        use crate::unit::{RunRecord, RunResult};
        use chrono::Utc;

        let make_record = |tokens: u64, result: RunResult| RunRecord {
            attempt: 1,
            started_at: Utc::now(),
            finished_at: None,
            duration_secs: None,
            agent: None,
            result,
            exit_code: None,
            tokens: Some(tokens),
            cost: None,
            output_snippet: None,
            autonomy_observation: None,
        };

        let mut cheap = Unit::new("1", "Cheap unit");
        cheap.history = vec![make_record(100, RunResult::Fail)];

        let mut expensive = Unit::new("2", "Expensive unit");
        expensive.history = vec![
            make_record(5000, RunResult::Fail),
            make_record(3000, RunResult::Pass),
        ];
        expensive.status = Status::Closed;

        let stats = aggregate_cost(&[cheap, expensive]).unwrap();
        assert_eq!(stats.total_tokens, 8100);
        let exp = stats.most_expensive_unit.unwrap();
        assert_eq!(exp.id, "2");
        assert_eq!(exp.value, 8000);

        let retried = stats.most_retried_unit.unwrap();
        assert_eq!(retried.id, "2");
        assert_eq!(retried.value, 2);
    }
}
