use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;

use crate::discovery::find_unit_file;
use crate::hooks::{execute_hook, HookEvent};
use crate::index::{Index, LockedIndex};
use crate::unit::Unit;

#[derive(Debug, Clone, Default)]
pub struct ReparentParams {
    pub parent: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ReparentResult {
    pub unit: Unit,
    pub path: PathBuf,
    pub old_parent: Option<String>,
    pub new_parent: Option<String>,
    pub reason: Option<String>,
}

pub fn reparent(mana_dir: &Path, id: &str, params: ReparentParams) -> Result<ReparentResult> {
    let unit_path =
        find_unit_file(mana_dir, id).with_context(|| format!("Unit not found: {}", id))?;
    let mut unit =
        Unit::from_file(&unit_path).with_context(|| format!("Failed to load unit: {}", id))?;

    if matches!(params.parent.as_deref(), Some(parent) if parent == id) {
        bail!("A unit cannot be its own parent");
    }

    if let Some(parent_id) = params.parent.as_deref() {
        find_unit_file(mana_dir, parent_id)
            .with_context(|| format!("Parent unit not found: {}", parent_id))?;
        ensure_not_descendant(mana_dir, id, parent_id)?;
    }

    let project_root = mana_dir
        .parent()
        .ok_or_else(|| anyhow!("Cannot determine project root from units dir"))?;

    let pre_passed = execute_hook(HookEvent::PreUpdate, &unit, project_root, None)
        .context("Pre-update hook execution failed")?;
    if !pre_passed {
        return Err(anyhow!("Pre-update hook rejected unit reparent"));
    }

    let old_parent = unit.parent.clone();
    let new_parent = params.parent.filter(|parent| !parent.trim().is_empty());
    unit.parent = new_parent.clone();
    unit.updated_at = Utc::now();
    if let Some(reason) = params
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let timestamp = Utc::now().to_rfc3339();
        let note = format!(
            "---\n{}\nReparented from {} to {}: {}",
            timestamp,
            old_parent.as_deref().unwrap_or("<root>"),
            new_parent.as_deref().unwrap_or("<root>"),
            reason
        );
        unit.notes = Some(match unit.notes.take() {
            Some(existing) => format!("{}\n\n{}", existing, note),
            None => note,
        });
    }

    unit.to_file(&unit_path)
        .with_context(|| format!("Failed to save unit: {}", id))?;

    let mut locked = LockedIndex::acquire(mana_dir)?;
    locked.index = Index::build(mana_dir)?;
    locked.save_and_release()?;

    if let Err(e) = execute_hook(HookEvent::PostUpdate, &unit, project_root, None) {
        eprintln!("Warning: post-update hook failed: {}", e);
    }

    Ok(ReparentResult {
        unit,
        path: unit_path,
        old_parent,
        new_parent,
        reason: params.reason,
    })
}

fn ensure_not_descendant(mana_dir: &Path, id: &str, proposed_parent: &str) -> Result<()> {
    let mut current = Some(proposed_parent.to_string());
    while let Some(current_id) = current {
        if current_id == id {
            bail!("Cannot reparent unit under its own descendant");
        }
        let current_path = find_unit_file(mana_dir, &current_id)
            .with_context(|| format!("Parent chain unit not found: {}", current_id))?;
        current = Unit::from_file(&current_path)?.parent;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::create::{self, CreateParams};
    use std::fs;
    use tempfile::TempDir;

    fn setup() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();
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
        (dir, mana_dir)
    }

    fn create_unit(mana_dir: &Path, title: &str, parent: Option<&str>) -> String {
        create::create(
            mana_dir,
            CreateParams {
                title: title.to_string(),
                parent: parent.map(str::to_string),
                ..Default::default()
            },
        )
        .unwrap()
        .unit
        .id
    }

    #[test]
    fn reparent_moves_child_between_parents_and_rebuilds_index() {
        let (_dir, mana_dir) = setup();
        let old_parent = create_unit(&mana_dir, "Old Parent", None);
        let new_parent = create_unit(&mana_dir, "New Parent", None);
        let child = create_unit(&mana_dir, "Child", Some(&old_parent));

        let result = reparent(
            &mana_dir,
            &child,
            ReparentParams {
                parent: Some(new_parent.clone()),
                reason: Some("Wrong active epic".to_string()),
            },
        )
        .unwrap();

        assert_eq!(result.old_parent.as_deref(), Some(old_parent.as_str()));
        assert_eq!(result.new_parent.as_deref(), Some(new_parent.as_str()));
        assert_eq!(result.unit.parent.as_deref(), Some(new_parent.as_str()));
        assert!(result.unit.notes.unwrap().contains("Wrong active epic"));

        let index = Index::load_or_rebuild(&mana_dir).unwrap();
        let child_entry = index.units.iter().find(|entry| entry.id == child).unwrap();
        assert_eq!(child_entry.parent.as_deref(), Some(new_parent.as_str()));
        assert!(
            !index
                .units
                .iter()
                .any(|entry| entry.id == child
                    && entry.parent.as_deref() == Some(old_parent.as_str()))
        );
    }

    #[test]
    fn reparent_rejects_missing_parent_and_cycles() {
        let (_dir, mana_dir) = setup();
        let parent = create_unit(&mana_dir, "Parent", None);
        let child = create_unit(&mana_dir, "Child", Some(&parent));

        let missing = reparent(
            &mana_dir,
            &child,
            ReparentParams {
                parent: Some("404".to_string()),
                reason: None,
            },
        )
        .unwrap_err();
        assert!(missing.to_string().contains("Parent unit not found"));

        let cycle = reparent(
            &mana_dir,
            &parent,
            ReparentParams {
                parent: Some(child),
                reason: None,
            },
        )
        .unwrap_err();
        assert!(cycle.to_string().contains("own descendant"));
    }
}
