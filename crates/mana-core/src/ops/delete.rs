use std::fs;
use std::path::Path;

use anyhow::Context;
use anyhow::Result;

use serde::{Deserialize, Serialize};

use crate::discovery::find_unit_file;
use crate::index::Index;
use crate::unit::Unit;

/// Result of deleting a unit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteResult {
    pub id: String,
    pub title: String,
}

/// Delete a unit and clean up dependency references.
pub fn delete(mana_dir: &Path, id: &str) -> Result<DeleteResult> {
    let unit_path =
        find_unit_file(mana_dir, id).with_context(|| format!("Unit not found: {}", id))?;
    let unit =
        Unit::from_file(&unit_path).with_context(|| format!("Failed to load unit: {}", id))?;
    let title = unit.title.clone();
    fs::remove_file(&unit_path).with_context(|| format!("Failed to delete: {}", id))?;
    cleanup_dep_references(mana_dir, id)?;
    let index = Index::build(mana_dir)?;
    index.save(mana_dir)?;
    Ok(DeleteResult {
        id: id.to_string(),
        title,
    })
}

fn cleanup_dep_references(mana_dir: &Path, deleted_id: &str) -> Result<()> {
    let dir_entries = fs::read_dir(mana_dir)
        .with_context(|| format!("Failed to read: {}", mana_dir.display()))?;
    for entry in dir_entries {
        let entry = entry?;
        let path = entry.path();
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if filename == "index.yaml" || filename == "config.yaml" || filename == "unit.yaml" {
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str());
        let is_unit = match ext {
            Some("md") => filename.contains('-'),
            Some("yaml") => true,
            _ => false,
        };
        if !is_unit {
            continue;
        }
        if let Ok(mut unit) = Unit::from_file(&path) {
            let n = unit.dependencies.len();
            unit.dependencies.retain(|d| d != deleted_id);
            if unit.dependencies.len() < n {
                unit.to_file(&path)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::create::{self, tests::minimal_params};
    use tempfile::TempDir;

    fn setup() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let bd = dir.path().join(".mana");
        fs::create_dir(&bd).unwrap();
        crate::config::Config {
            project: "test".into(),
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

    #[test]
    fn delete_unit() {
        let (_dir, bd) = setup();
        let c = create::create(&bd, minimal_params("Task")).unwrap();
        assert!(c.path.exists());
        let r = delete(&bd, "1").unwrap();
        assert_eq!(r.title, "Task");
        assert!(!c.path.exists());
    }

    #[test]
    fn delete_nonexistent() {
        let (_dir, bd) = setup();
        assert!(delete(&bd, "99").is_err());
    }

    #[test]
    fn delete_cleans_deps() {
        let (_dir, bd) = setup();
        create::create(&bd, minimal_params("A")).unwrap();
        let mut p = minimal_params("B");
        p.dependencies = vec!["1".into()];
        create::create(&bd, p).unwrap();
        delete(&bd, "1").unwrap();
        let b2 = Unit::from_file(find_unit_file(&bd, "2").unwrap()).unwrap();
        assert!(!b2.dependencies.contains(&"1".to_string()));
    }

    #[test]
    fn delete_rebuilds_index() {
        let (_dir, bd) = setup();
        create::create(&bd, minimal_params("A")).unwrap();
        create::create(&bd, minimal_params("B")).unwrap();
        delete(&bd, "1").unwrap();
        let index = Index::load(&bd).unwrap();
        assert_eq!(index.units.len(), 1);
    }
}
