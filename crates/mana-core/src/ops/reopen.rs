use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;

use serde::{Deserialize, Serialize};

use crate::discovery::find_unit_file;
use crate::index::Index;
use crate::unit::{Status, Unit};

/// Result of reopening a unit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReopenResult {
    pub unit: Unit,
    pub path: PathBuf,
}

/// Reopen a closed unit.
pub fn reopen(mana_dir: &Path, id: &str) -> Result<ReopenResult> {
    let unit_path =
        find_unit_file(mana_dir, id).with_context(|| format!("Unit not found: {}", id))?;
    let mut unit =
        Unit::from_file(&unit_path).with_context(|| format!("Failed to load unit: {}", id))?;

    unit.status = Status::Open;
    unit.closed_at = None;
    unit.close_reason = None;
    unit.updated_at = Utc::now();

    unit.to_file(&unit_path)
        .with_context(|| format!("Failed to save unit: {}", id))?;

    let index = Index::build(mana_dir)?;
    index.save(mana_dir)?;

    Ok(ReopenResult {
        unit,
        path: unit_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::create::{self, tests::minimal_params};
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

    #[test]
    fn reopen_closed_unit() {
        let (_dir, bd) = setup();
        create::create(&bd, minimal_params("Task")).unwrap();
        let bp = find_unit_file(&bd, "1").unwrap();
        let mut unit = Unit::from_file(&bp).unwrap();
        unit.status = Status::Closed;
        unit.closed_at = Some(Utc::now());
        unit.close_reason = Some("Done".into());
        unit.to_file(&bp).unwrap();

        let r = reopen(&bd, "1").unwrap();
        assert_eq!(r.unit.status, Status::Open);
        assert!(r.unit.closed_at.is_none());
        assert!(r.unit.close_reason.is_none());
    }

    #[test]
    fn reopen_nonexistent() {
        let (_dir, bd) = setup();
        assert!(reopen(&bd, "99").is_err());
    }

    #[test]
    fn reopen_rebuilds_index() {
        let (_dir, bd) = setup();
        create::create(&bd, minimal_params("Task")).unwrap();
        let bp = find_unit_file(&bd, "1").unwrap();
        let mut unit = Unit::from_file(&bp).unwrap();
        unit.status = Status::Closed;
        unit.to_file(&bp).unwrap();

        reopen(&bd, "1").unwrap();
        let index = Index::load(&bd).unwrap();
        assert_eq!(
            index.units.iter().find(|e| e.id == "1").unwrap().status,
            Status::Open
        );
    }
}
