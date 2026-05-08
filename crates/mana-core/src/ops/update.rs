use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use chrono::Utc;

use crate::hooks::{execute_hook, HookEvent};
use crate::index::{Index, LockedIndex};
use crate::resolve::resolve_unit;
use crate::unit::{validate_priority, Unit};
use crate::util::parse_status;

/// Parameters for updating a unit.
#[derive(Default)]
pub struct UpdateParams {
    pub title: Option<String>,
    pub description: Option<String>,
    pub acceptance: Option<String>,
    pub notes: Option<String>,
    pub design: Option<String>,
    pub status: Option<String>,
    pub priority: Option<u8>,
    pub assignee: Option<String>,
    pub add_label: Option<String>,
    pub remove_label: Option<String>,
    pub decisions: Vec<String>,
    pub resolve_decisions: Vec<String>,
}

/// Result of updating a unit.
#[derive(serde::Serialize)]
pub struct UpdateResult {
    pub unit: Unit,
    pub path: PathBuf,
}

/// Update a unit's fields and persist changes.
pub fn update(mana_dir: &Path, id: &str, params: UpdateParams) -> Result<UpdateResult> {
    if let Some(p) = params.priority {
        validate_priority(p)?;
    }

    let resolved = resolve_unit(mana_dir, id)?;
    let unit_path = resolved.path;
    let mut unit = resolved.unit;

    let project_root = mana_dir
        .parent()
        .ok_or_else(|| anyhow!("Cannot determine project root from units dir"))?;

    let pre_passed = execute_hook(HookEvent::PreUpdate, &unit, project_root, None)
        .context("Pre-update hook execution failed")?;
    if !pre_passed {
        return Err(anyhow!("Pre-update hook rejected unit update"));
    }

    if let Some(v) = params.title {
        unit.title = v;
    }
    if let Some(v) = params.description {
        unit.description = Some(v);
    }
    if let Some(v) = params.acceptance {
        unit.acceptance = Some(v);
    }

    if let Some(new_notes) = params.notes {
        let timestamp = Utc::now().to_rfc3339();
        unit.notes = Some(match unit.notes {
            Some(existing) => format!("{}\n\n---\n{}\n{}", existing, timestamp, new_notes),
            None => format!("---\n{}\n{}", timestamp, new_notes),
        });
    }

    if let Some(v) = params.design {
        unit.design = Some(v);
    }

    if let Some(new_status) = params.status {
        unit.status =
            parse_status(&new_status).ok_or_else(|| anyhow!("Invalid status: {}", new_status))?;
    }

    if let Some(v) = params.priority {
        unit.priority = v;
    }
    if let Some(v) = params.assignee {
        unit.assignee = Some(v);
    }

    if let Some(label) = params.add_label {
        if !unit.labels.contains(&label) {
            unit.labels.push(label);
        }
    }
    if let Some(label) = params.remove_label {
        unit.labels.retain(|l| l != &label);
    }

    for decision in params.decisions {
        unit.decisions.push(decision);
    }

    for resolve in &params.resolve_decisions {
        if let Ok(idx) = resolve.parse::<usize>() {
            if idx < unit.decisions.len() {
                unit.decisions.remove(idx);
            } else {
                return Err(anyhow!(
                    "Decision index {} out of range (unit has {} decisions)",
                    idx,
                    unit.decisions.len()
                ));
            }
        } else {
            let before = unit.decisions.len();
            unit.decisions.retain(|d| d != resolve);
            if unit.decisions.len() == before {
                return Err(anyhow!("No decision matching '{}' found", resolve));
            }
        }
    }

    unit.updated_at = Utc::now();
    unit.to_file(&unit_path)
        .with_context(|| format!("Failed to save unit: {}", id))?;

    let mut locked = LockedIndex::acquire(mana_dir)?;
    locked.index = Index::build(mana_dir)?;
    locked.save_and_release()?;

    if let Err(e) = execute_hook(HookEvent::PostUpdate, &unit, project_root, None) {
        eprintln!("Warning: post-update hook failed: {}", e);
    }

    Ok(UpdateResult {
        unit,
        path: unit_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::create::{self, tests::minimal_params};
    use crate::unit::Status;
    use std::fs;
    use tempfile::TempDir;

    fn setup() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let bd = dir.path().join(".mana");
        fs::create_dir(&bd).unwrap();
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
        .save(&bd)
        .unwrap();
        (dir, bd)
    }

    fn empty_params() -> UpdateParams {
        UpdateParams {
            title: None,
            description: None,
            acceptance: None,
            notes: None,
            design: None,
            status: None,
            priority: None,
            assignee: None,
            add_label: None,
            remove_label: None,
            decisions: vec![],
            resolve_decisions: vec![],
        }
    }

    #[test]
    fn update_title() {
        let (_dir, bd) = setup();
        create::create(&bd, minimal_params("Old")).unwrap();
        let r = update(
            &bd,
            "1",
            UpdateParams {
                title: Some("New".into()),
                ..empty_params()
            },
        )
        .unwrap();
        assert_eq!(r.unit.title, "New");
    }

    #[test]
    fn update_status() {
        let (_dir, bd) = setup();
        create::create(&bd, minimal_params("Task")).unwrap();
        let r = update(
            &bd,
            "1",
            UpdateParams {
                status: Some("in_progress".into()),
                ..empty_params()
            },
        )
        .unwrap();
        assert_eq!(r.unit.status, Status::InProgress);
    }

    #[test]
    fn update_appends_notes() {
        let (_dir, bd) = setup();
        create::create(&bd, minimal_params("Task")).unwrap();
        update(
            &bd,
            "1",
            UpdateParams {
                notes: Some("First".into()),
                ..empty_params()
            },
        )
        .unwrap();
        let r = update(
            &bd,
            "1",
            UpdateParams {
                notes: Some("Second".into()),
                ..empty_params()
            },
        )
        .unwrap();
        let notes = r.unit.notes.unwrap();
        assert!(notes.contains("First"));
        assert!(notes.contains("Second"));
    }

    #[test]
    fn update_nonexistent() {
        let (_dir, bd) = setup();
        assert!(update(
            &bd,
            "99",
            UpdateParams {
                title: Some("x".into()),
                ..empty_params()
            }
        )
        .is_err());
    }

    #[test]
    fn update_rebuilds_index() {
        let (_dir, bd) = setup();
        create::create(&bd, minimal_params("Task")).unwrap();
        update(
            &bd,
            "1",
            UpdateParams {
                title: Some("Updated".into()),
                ..empty_params()
            },
        )
        .unwrap();
        let index = Index::load(&bd).unwrap();
        assert_eq!(index.units[0].title, "Updated");
    }
}
