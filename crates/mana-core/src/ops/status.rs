use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::blocking::check_blocked_with_archive;
use crate::index::{ArchiveIndex, Index, IndexEntry};
use crate::unit::{Status, UnitType};
use crate::util::natural_cmp;

/// Categorized view of project status.
#[derive(Debug, Serialize)]
pub struct StatusSummary {
    pub epics: Vec<IndexEntry>,
    pub features: Vec<IndexEntry>,
    pub claimed: Vec<IndexEntry>,
    pub ready: Vec<IndexEntry>,
    pub goals: Vec<IndexEntry>,
    pub blocked: Vec<BlockedEntry>,
}

/// An entry that is blocked with its reason.
#[derive(Debug, Serialize)]
pub struct BlockedEntry {
    #[serde(flatten)]
    pub entry: IndexEntry,
    pub block_reason: String,
}

/// Compute the project status summary: categorize units into claimed, ready,
/// goals (need decomposition), and blocked.
pub fn status(mana_dir: &Path) -> Result<StatusSummary> {
    let index = Index::load_or_rebuild(mana_dir)?;
    let archive = ArchiveIndex::load_or_rebuild(mana_dir)
        .unwrap_or_else(|_| ArchiveIndex { units: Vec::new() });

    let mut epics: Vec<IndexEntry> = Vec::new();
    let mut features: Vec<IndexEntry> = Vec::new();
    let mut claimed: Vec<IndexEntry> = Vec::new();
    let mut ready: Vec<IndexEntry> = Vec::new();
    let mut goals: Vec<IndexEntry> = Vec::new();
    let mut blocked: Vec<BlockedEntry> = Vec::new();

    for entry in &index.units {
        if entry.feature {
            features.push(entry.clone());
            continue;
        }
        if entry.kind == UnitType::Epic {
            epics.push(entry.clone());
            continue;
        }
        match entry.status {
            Status::InProgress | Status::AwaitingVerify => {
                claimed.push(entry.clone());
            }
            Status::Open => {
                if let Some(reason) = check_blocked_with_archive(entry, &index, Some(&archive)) {
                    blocked.push(BlockedEntry {
                        entry: entry.clone(),
                        block_reason: reason.to_string(),
                    });
                } else if entry.kind == UnitType::Task && entry.has_verify {
                    ready.push(entry.clone());
                } else {
                    goals.push(entry.clone());
                }
            }
            Status::Closed => {}
        }
    }

    sort_entries(&mut epics);
    sort_entries(&mut features);
    sort_entries(&mut claimed);
    sort_entries(&mut ready);
    sort_entries(&mut goals);
    blocked.sort_by(|a, b| match a.entry.priority.cmp(&b.entry.priority) {
        std::cmp::Ordering::Equal => natural_cmp(&a.entry.id, &b.entry.id),
        other => other,
    });

    Ok(StatusSummary {
        epics,
        features,
        claimed,
        ready,
        goals,
        blocked,
    })
}

fn sort_entries(entries: &mut [IndexEntry]) {
    entries.sort_by(|a, b| match a.priority.cmp(&b.priority) {
        std::cmp::Ordering::Equal => natural_cmp(&a.id, &b.id),
        other => other,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unit::Unit;
    use crate::util::title_to_slug;
    use std::fs;
    use tempfile::TempDir;

    fn setup() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();
        (dir, mana_dir)
    }

    fn write_unit(mana_dir: &Path, unit: &Unit) {
        let slug = title_to_slug(&unit.title);
        let path = mana_dir.join(format!("{}-{}.md", unit.id, slug));
        unit.to_file(path).unwrap();
    }

    #[test]
    fn status_groups_by_kind() {
        let (_dir, mana_dir) = setup();

        let mut epic = Unit::new("1", "Epic");
        epic.kind = UnitType::Epic;
        write_unit(&mana_dir, &epic);

        let mut task = Unit::new("2", "Task");
        task.kind = UnitType::Task;
        task.verify = Some("cargo test task".to_string());
        write_unit(&mana_dir, &task);

        let mut feature = Unit::new("3", "Feature");
        feature.kind = UnitType::Epic;
        feature.feature = true;
        write_unit(&mana_dir, &feature);

        let result = status(&mana_dir).unwrap();
        assert_eq!(result.epics.len(), 1);
        assert_eq!(result.epics[0].id, "1");
        assert_eq!(result.ready.len(), 1);
        assert_eq!(result.ready[0].id, "2");
        assert_eq!(result.features.len(), 1);
        assert_eq!(result.features[0].id, "3");
    }

    #[test]
    fn status_categorizes_units() {
        let (_dir, mana_dir) = setup();

        // Open unit with verify -> ready
        let mut ready_unit = Unit::new("1", "Ready task");
        ready_unit.verify = Some("cargo test unit::check".to_string());
        write_unit(&mana_dir, &ready_unit);

        // Open unit without verify -> goals
        let goal_unit = Unit::new("2", "Goal task");
        write_unit(&mana_dir, &goal_unit);

        // In progress -> claimed
        let mut claimed_unit = Unit::new("3", "Claimed task");
        claimed_unit.status = Status::InProgress;
        write_unit(&mana_dir, &claimed_unit);

        let result = status(&mana_dir).unwrap();

        assert_eq!(result.ready.len(), 1);
        assert_eq!(result.ready[0].id, "1");

        assert_eq!(result.goals.len(), 1);
        assert_eq!(result.goals[0].id, "2");

        assert_eq!(result.claimed.len(), 1);
        assert_eq!(result.claimed[0].id, "3");

        assert!(result.blocked.is_empty());
    }

    #[test]
    fn status_detects_blocked() {
        let (_dir, mana_dir) = setup();

        // Create a dependency that's still open
        let mut dep = Unit::new("1", "Dependency");
        dep.verify = Some("true".to_string());
        write_unit(&mana_dir, &dep);

        // Create unit depending on the open dep
        let mut blocked_unit = Unit::new("2", "Blocked task");
        blocked_unit.verify = Some("true".to_string());
        blocked_unit.dependencies = vec!["1".to_string()];
        write_unit(&mana_dir, &blocked_unit);

        let result = status(&mana_dir).unwrap();

        assert_eq!(result.blocked.len(), 1);
        assert_eq!(result.blocked[0].entry.id, "2");
    }

    #[test]
    fn status_empty_project() {
        let (_dir, mana_dir) = setup();

        let result = status(&mana_dir).unwrap();

        assert!(result.features.is_empty());
        assert!(result.claimed.is_empty());
        assert!(result.ready.is_empty());
        assert!(result.goals.is_empty());
        assert!(result.blocked.is_empty());
    }

    #[test]
    fn status_skips_closed() {
        let (_dir, mana_dir) = setup();

        let mut unit = Unit::new("1", "Closed task");
        unit.status = Status::Closed;
        write_unit(&mana_dir, &unit);

        let result = status(&mana_dir).unwrap();

        assert!(result.claimed.is_empty());
        assert!(result.ready.is_empty());
        assert!(result.goals.is_empty());
        assert!(result.blocked.is_empty());
    }

    #[test]
    fn awaiting_verify_appears_in_claimed() {
        let (_dir, mana_dir) = setup();

        let mut unit = Unit::new("1", "Awaiting verify task");
        unit.verify = Some("cargo test unit::check".to_string());
        unit.status = Status::AwaitingVerify;
        write_unit(&mana_dir, &unit);

        let result = status(&mana_dir).unwrap();

        assert_eq!(result.claimed.len(), 1);
        assert_eq!(result.claimed[0].id, "1");
        assert!(result.ready.is_empty());
        assert!(result.goals.is_empty());
    }

    #[test]
    fn status_archived_dep_not_blocking() {
        let (_dir, mana_dir) = setup();

        // Write an archived dep into .mana/archive/
        let archive_dir = mana_dir.join("archive");
        fs::create_dir(&archive_dir).unwrap();
        let mut archived_dep = Unit::new("1", "Archived dep");
        archived_dep.status = Status::Closed;
        archived_dep
            .to_file(archive_dir.join("1-archived-dep.md"))
            .unwrap();

        // Unit depending on the archived dep should NOT be blocked
        let mut unit = Unit::new("2", "Dependent task");
        unit.verify = Some("true".to_string());
        unit.dependencies = vec!["1".to_string()];
        write_unit(&mana_dir, &unit);

        let result = status(&mana_dir).unwrap();

        assert!(
            result.blocked.is_empty(),
            "expected no blocked units, got: {:?}",
            result
                .blocked
                .iter()
                .map(|b| &b.entry.id)
                .collect::<Vec<_>>()
        );
        assert_eq!(result.ready.len(), 1);
        assert_eq!(result.ready[0].id, "2");
    }
}
