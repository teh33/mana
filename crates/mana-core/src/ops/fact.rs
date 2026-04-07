use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command as ShellCommand;

use anyhow::{anyhow, Result};
use chrono::{Duration, Utc};

use serde::{Deserialize, Serialize};

use crate::discovery::{find_archived_unit, find_unit_file};
use crate::index::Index;
use crate::ops::create::{create, CreateParams};
use crate::unit::{Status, Unit};

/// Default TTL for facts: 30 days.
const DEFAULT_TTL_DAYS: i64 = 30;

/// Parameters for creating a fact.
pub struct FactParams {
    pub title: String,
    pub verify: String,
    pub description: Option<String>,
    pub paths: Option<String>,
    pub ttl_days: Option<i64>,
    pub pass_ok: bool,
}

/// Result of creating a fact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactResult {
    pub unit_id: String,
    pub unit: Unit,
}

/// Result of a single fact verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactVerifyEntry {
    pub id: String,
    pub title: String,
    pub stale: bool,
    pub verify_passed: Option<bool>,
    pub error: Option<String>,
}

/// Aggregated result of verifying all facts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyFactsResult {
    pub total_facts: usize,
    pub verified_count: usize,
    pub stale_count: usize,
    pub failing_count: usize,
    pub suspect_count: usize,
    pub entries: Vec<FactVerifyEntry>,
    pub suspect_entries: Vec<(String, String)>,
}

/// Create a verified fact (unit with unit_type=fact).
///
/// Facts require a verify command — that's the point. If you can't write a
/// verify command, the knowledge belongs in agents.md, not in a fact.
pub fn create_fact(mana_dir: &Path, params: FactParams) -> Result<FactResult> {
    if params.verify.trim().is_empty() {
        return Err(anyhow!(
            "Facts require a verify command. If you can't write one, \
             this belongs in agents.md, not mana fact."
        ));
    }

    let create_result = create(
        mana_dir,
        CreateParams {
            title: params.title,
            description: params.description,
            acceptance: None,
            notes: None,
            design: None,
            verify: Some(params.verify),
            priority: Some(3),
            labels: vec!["fact".to_string()],
            assignee: None,
            dependencies: vec![],
            parent: None,
            produces: vec![],
            requires: vec![],
            paths: vec![],
            on_fail: None,
            fail_first: false,
            feature: false,
            kind: None,
            verify_timeout: None,
            decisions: vec![],
            force: false,
        },
    )?;

    let unit_id = create_result.unit.id.clone();
    let unit_path = create_result.path;
    let mut unit = create_result.unit;

    unit.unit_type = "fact".to_string();

    let ttl = params.ttl_days.unwrap_or(DEFAULT_TTL_DAYS);
    unit.stale_after = Some(Utc::now() + Duration::days(ttl));

    if let Some(paths_str) = params.paths {
        unit.paths = paths_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }

    unit.to_file(&unit_path)?;

    let index = Index::build(mana_dir)?;
    index.save(mana_dir)?;

    Ok(FactResult { unit_id, unit })
}

/// Verify all facts and return structured results.
///
/// Re-runs verify commands for all units with unit_type=fact.
/// Reports which facts are stale (past their stale_after date)
/// and which have failing verify commands.
///
/// Suspect propagation: facts that require artifacts from failing/stale facts
/// are marked as suspect (up to depth 3).
pub fn verify_facts(mana_dir: &Path) -> Result<VerifyFactsResult> {
    let project_root = mana_dir
        .parent()
        .ok_or_else(|| anyhow!("Cannot determine project root from units dir"))?;

    let index = Index::load_or_rebuild(mana_dir)?;
    let archived = Index::collect_archived(mana_dir).unwrap_or_default();

    let now = Utc::now();
    let mut stale_count = 0;
    let mut failing_count = 0;
    let mut verified_count = 0;
    let mut total_facts = 0;

    let mut invalid_artifacts: HashSet<String> = HashSet::new();
    let mut fact_requires: HashMap<String, Vec<String>> = HashMap::new();
    let mut fact_titles: HashMap<String, String> = HashMap::new();
    let mut entries: Vec<FactVerifyEntry> = Vec::new();

    for entry in index.units.iter().chain(archived.iter()) {
        let unit_path = if entry.status == Status::Closed {
            find_archived_unit(mana_dir, &entry.id).ok()
        } else {
            find_unit_file(mana_dir, &entry.id).ok()
        };

        let unit_path = match unit_path {
            Some(p) => p,
            None => continue,
        };

        let mut unit = match Unit::from_file(&unit_path) {
            Ok(b) => b,
            Err(_) => continue,
        };

        if unit.unit_type != "fact" {
            continue;
        }

        total_facts += 1;
        fact_titles.insert(unit.id.clone(), unit.title.clone());
        if !unit.requires.is_empty() {
            fact_requires.insert(unit.id.clone(), unit.requires.clone());
        }

        let is_stale = unit.stale_after.map(|sa| now > sa).unwrap_or(false);

        if is_stale {
            stale_count += 1;
            for prod in &unit.produces {
                invalid_artifacts.insert(prod.clone());
            }
        }

        // Re-run verify command
        let (verify_passed, error) = if let Some(ref verify_cmd) = unit.verify {
            let output = ShellCommand::new("sh")
                .args(["-c", verify_cmd])
                .current_dir(project_root)
                .output();

            match output {
                Ok(o) if o.status.success() => {
                    verified_count += 1;
                    unit.last_verified = Some(now);
                    if unit.stale_after.is_some() {
                        unit.stale_after = Some(now + Duration::days(DEFAULT_TTL_DAYS));
                    }
                    unit.to_file(&unit_path)?;
                    (Some(true), None)
                }
                Ok(_) => {
                    failing_count += 1;
                    for prod in &unit.produces {
                        invalid_artifacts.insert(prod.clone());
                    }
                    (Some(false), None)
                }
                Err(e) => {
                    failing_count += 1;
                    for prod in &unit.produces {
                        invalid_artifacts.insert(prod.clone());
                    }
                    (Some(false), Some(e.to_string()))
                }
            }
        } else {
            (None, None)
        };

        entries.push(FactVerifyEntry {
            id: unit.id.clone(),
            title: unit.title.clone(),
            stale: is_stale,
            verify_passed,
            error,
        });
    }

    // Suspect propagation
    let mut suspect_entries: Vec<(String, String)> = Vec::new();
    let mut suspect_count = 0;

    if !invalid_artifacts.is_empty() {
        let mut suspect_ids: HashSet<String> = HashSet::new();
        let mut current_invalid = invalid_artifacts.clone();

        for _depth in 0..3 {
            let mut newly_invalid: HashSet<String> = HashSet::new();

            for (fact_id, requires) in &fact_requires {
                if suspect_ids.contains(fact_id) {
                    continue;
                }
                for req in requires {
                    if current_invalid.contains(req) {
                        suspect_ids.insert(fact_id.clone());
                        if let Some(entry) = index
                            .units
                            .iter()
                            .chain(archived.iter())
                            .find(|e| e.id == *fact_id)
                        {
                            let bp = if entry.status == Status::Closed {
                                find_archived_unit(mana_dir, &entry.id).ok()
                            } else {
                                find_unit_file(mana_dir, &entry.id).ok()
                            };
                            if let Some(bp) = bp {
                                if let Ok(b) = Unit::from_file(&bp) {
                                    for prod in &b.produces {
                                        newly_invalid.insert(prod.clone());
                                    }
                                }
                            }
                        }
                        break;
                    }
                }
            }

            if newly_invalid.is_empty() {
                break;
            }
            current_invalid = newly_invalid;
        }

        for suspect_id in &suspect_ids {
            suspect_count += 1;
            let title = fact_titles
                .get(suspect_id)
                .map(|s| s.as_str())
                .unwrap_or("?")
                .to_string();
            suspect_entries.push((suspect_id.clone(), title));
        }
    }

    Ok(VerifyFactsResult {
        total_facts,
        verified_count,
        stale_count,
        failing_count,
        suspect_count,
        entries,
        suspect_entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::fs;
    use tempfile::TempDir;

    fn setup_mana_dir() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        Config {
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

        (dir, mana_dir)
    }

    #[test]
    fn create_fact_sets_unit_type() {
        let (_dir, mana_dir) = setup_mana_dir();

        let result = create_fact(
            &mana_dir,
            FactParams {
                title: "Auth uses RS256".to_string(),
                verify: "grep -q RS256 src/auth.rs".to_string(),
                description: None,
                paths: None,
                ttl_days: None,
                pass_ok: true,
            },
        )
        .unwrap();

        assert_eq!(result.unit.unit_type, "fact");
        assert!(result.unit.labels.contains(&"fact".to_string()));
        assert!(result.unit.stale_after.is_some());
        assert!(result.unit.verify.is_some());
    }

    #[test]
    fn create_fact_with_paths() {
        let (_dir, mana_dir) = setup_mana_dir();

        let result = create_fact(
            &mana_dir,
            FactParams {
                title: "Config file format".to_string(),
                verify: "grep -q 'project: test' .mana/config.yaml".to_string(),
                description: None,
                paths: Some("src/config.rs, src/main.rs".to_string()),
                ttl_days: None,
                pass_ok: true,
            },
        )
        .unwrap();

        assert_eq!(result.unit.paths, vec!["src/config.rs", "src/main.rs"]);
    }

    #[test]
    fn create_fact_with_custom_ttl() {
        let (_dir, mana_dir) = setup_mana_dir();

        let result = create_fact(
            &mana_dir,
            FactParams {
                title: "Short-lived fact".to_string(),
                verify: "grep -q 'project: test' .mana/config.yaml".to_string(),
                description: None,
                paths: None,
                ttl_days: Some(7),
                pass_ok: true,
            },
        )
        .unwrap();

        let stale = result.unit.stale_after.unwrap();
        let diff = stale - Utc::now();
        assert!(diff.num_days() >= 6 && diff.num_days() <= 7);
    }

    #[test]
    fn create_fact_requires_verify() {
        let (_dir, mana_dir) = setup_mana_dir();

        let result = create_fact(
            &mana_dir,
            FactParams {
                title: "No verify fact".to_string(),
                verify: "  ".to_string(),
                description: None,
                paths: None,
                ttl_days: None,
                pass_ok: true,
            },
        );

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("verify command"));
    }
}
