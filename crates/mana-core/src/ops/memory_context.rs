use std::cmp::Reverse;
use std::path::Path;

use anyhow::Result;
use chrono::{Duration, Utc};

use crate::discovery::{find_archived_unit, find_unit_file};
use crate::index::Index;
use crate::relevance::relevance_score;
use crate::unit::{AttemptOutcome, Status, Unit};

/// A working unit with attempt context.
#[derive(Debug)]
pub struct WorkingUnit {
    pub unit: Unit,
    pub failed_attempts: usize,
    pub last_failure_notes: Option<String>,
}

/// A fact with its relevance score.
#[derive(Debug)]
pub struct RelevantFact {
    pub unit: Unit,
    pub score: u32,
}

/// A recently closed unit.
#[derive(Debug)]
pub struct RecentWork {
    pub unit: Unit,
}

/// Assembled memory context for session-start injection.
pub struct MemoryContext {
    pub warnings: Vec<String>,
    pub working_on: Vec<WorkingUnit>,
    pub relevant_facts: Vec<RelevantFact>,
    pub recent_work: Vec<RecentWork>,
}

/// Assemble memory context: warnings, working units, relevant facts, recent work.
///
/// This is the core logic behind `mana context` (without a unit ID) — it collects
/// information relevant to the current session without any formatting.
pub fn memory_context(mana_dir: &Path) -> Result<MemoryContext> {
    let now = Utc::now();
    let index = Index::load_or_rebuild(mana_dir)?;
    let archived = Index::collect_archived(mana_dir).unwrap_or_default();

    let mut working_paths: Vec<String> = Vec::new();
    let mut working_deps: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut working_on: Vec<WorkingUnit> = Vec::new();

    // Collect working units
    for entry in &index.units {
        if entry.status != Status::InProgress {
            continue;
        }

        let unit_path = match find_unit_file(mana_dir, &entry.id) {
            Ok(p) => p,
            Err(_) => continue,
        };

        let unit = match Unit::from_file(&unit_path) {
            Ok(b) => b,
            Err(_) => continue,
        };

        working_paths.extend(unit.paths.clone());
        working_deps.extend(unit.requires.clone());
        working_deps.extend(unit.produces.clone());

        let failed_attempts: Vec<_> = unit
            .attempt_log
            .iter()
            .filter(|a| a.outcome == AttemptOutcome::Failed)
            .collect();

        let last_failure_notes = failed_attempts.last().and_then(|a| a.notes.clone());

        if let Some(ref notes) = last_failure_notes {
            warnings.push(format!(
                "PAST FAILURE [{}]: \"{}\"",
                unit.id,
                notes.chars().take(80).collect::<String>()
            ));
        }

        working_on.push(WorkingUnit {
            failed_attempts: failed_attempts.len(),
            last_failure_notes,
            unit,
        });
    }

    // Check facts for staleness
    for entry in index.units.iter().chain(archived.iter()) {
        let unit_path = match find_unit_file(mana_dir, &entry.id)
            .or_else(|_| find_archived_unit(mana_dir, &entry.id))
        {
            Ok(p) => p,
            Err(_) => continue,
        };

        let unit = match Unit::from_file(&unit_path) {
            Ok(b) => b,
            Err(_) => continue,
        };

        if unit.unit_type != "fact" {
            continue;
        }

        if let Some(stale_after) = unit.stale_after {
            if now > stale_after {
                let days_stale = (now - stale_after).num_days();
                warnings.push(format!(
                    "STALE: \"{}\" — not verified in {}d",
                    unit.title, days_stale
                ));
            }
        }
    }

    // Score relevant facts
    let mut relevant_facts: Vec<RelevantFact> = Vec::new();

    for entry in index.units.iter().chain(archived.iter()) {
        let unit_path = match find_unit_file(mana_dir, &entry.id)
            .or_else(|_| find_archived_unit(mana_dir, &entry.id))
        {
            Ok(p) => p,
            Err(_) => continue,
        };

        let unit = match Unit::from_file(&unit_path) {
            Ok(b) => b,
            Err(_) => continue,
        };

        if unit.unit_type != "fact" {
            continue;
        }

        let score = relevance_score(&unit, &working_paths, &working_deps);
        if score > 0 {
            relevant_facts.push(RelevantFact { unit, score });
        }
    }

    relevant_facts.sort_by_key(|fact| Reverse(fact.score));

    // Recent work (closed in last 7 days)
    let mut recent_work: Vec<RecentWork> = Vec::new();
    let seven_days_ago = now - Duration::days(7);

    for entry in &archived {
        if entry.status != Status::Closed {
            continue;
        }

        let unit_path = match find_archived_unit(mana_dir, &entry.id) {
            Ok(p) => p,
            Err(_) => continue,
        };

        let unit = match Unit::from_file(&unit_path) {
            Ok(b) => b,
            Err(_) => continue,
        };

        if unit.unit_type == "fact" {
            continue;
        }

        if let Some(closed_at) = unit.closed_at {
            if closed_at > seven_days_ago {
                recent_work.push(RecentWork { unit });
            }
        }
    }

    recent_work.sort_by(|a, b| {
        b.unit
            .closed_at
            .unwrap_or(now)
            .cmp(&a.unit.closed_at.unwrap_or(now))
    });

    Ok(MemoryContext {
        warnings,
        working_on,
        relevant_facts,
        recent_work,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        crate::config::Config {
            project: "test".to_string(),
            next_id: 10,
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

        (dir, mana_dir)
    }

    #[test]
    fn memory_context_empty() {
        let (_dir, mana_dir) = setup();
        let result = memory_context(&mana_dir).unwrap();
        assert!(result.warnings.is_empty());
        assert!(result.working_on.is_empty());
        assert!(result.relevant_facts.is_empty());
        assert!(result.recent_work.is_empty());
    }

    #[test]
    fn memory_context_shows_claimed_units() {
        let (_dir, mana_dir) = setup();

        let mut unit = Unit::new("1", "Working on auth");
        unit.status = Status::InProgress;
        unit.claimed_by = Some("agent-1".to_string());
        unit.claimed_at = Some(Utc::now());
        let slug = crate::util::title_to_slug(&unit.title);
        unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
            .unwrap();

        let result = memory_context(&mana_dir).unwrap();
        assert_eq!(result.working_on.len(), 1);
        assert_eq!(result.working_on[0].unit.id, "1");
    }

    #[test]
    fn memory_context_shows_stale_facts() {
        let (_dir, mana_dir) = setup();

        let mut unit = Unit::new("1", "Auth uses RS256");
        unit.unit_type = "fact".to_string();
        unit.stale_after = Some(Utc::now() - Duration::days(5));
        unit.verify = Some("true".to_string());
        let slug = crate::util::title_to_slug(&unit.title);
        unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
            .unwrap();

        let result = memory_context(&mana_dir).unwrap();
        assert!(!result.warnings.is_empty());
        assert!(result.warnings[0].contains("STALE"));
    }
}
