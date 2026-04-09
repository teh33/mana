use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::discovery::{find_archived_unit, find_unit_file};
use crate::unit::Unit;

/// Result of loading a unit.
pub struct GetResult {
    pub unit: Unit,
    pub path: PathBuf,
}

/// Load a unit by ID and return its full data.
pub fn get(mana_dir: &Path, id: &str) -> Result<GetResult> {
    let unit_path = find_unit_file(mana_dir, id)
        .or_else(|_| find_archived_unit(mana_dir, id))
        .with_context(|| format!("Unit not found: {}", id))?;
    let unit =
        Unit::from_file(&unit_path).with_context(|| format!("Failed to load unit: {}", id))?;
    Ok(GetResult {
        unit,
        path: unit_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::create::{self, tests::minimal_params};
    use crate::util::title_to_slug;
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
    fn get_existing() {
        let (_dir, bd) = setup();
        create::create(&bd, minimal_params("My task")).unwrap();
        let r = get(&bd, "1").unwrap();
        assert_eq!(r.unit.title, "My task");
        assert!(r.path.exists());
    }

    #[test]
    fn get_archived_when_active_missing() {
        let (_dir, bd) = setup();
        let archive_dir = bd.join("archive/2026/04");
        fs::create_dir_all(&archive_dir).unwrap();

        let mut unit = Unit::new("1", "Archived task");
        unit.is_archived = true;
        let slug = title_to_slug(&unit.title);
        let archived_path = archive_dir.join(format!("1-{}.md", slug));
        unit.to_file(&archived_path).unwrap();

        let r = get(&bd, "1").unwrap();
        assert_eq!(r.unit.title, "Archived task");
        assert_eq!(r.path, archived_path);
        assert!(r.unit.is_archived);
    }

    #[test]
    fn get_nonexistent() {
        let (_dir, bd) = setup();
        assert!(get(&bd, "99").is_err());
    }

}
