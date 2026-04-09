//! Review queue — list and rank units awaiting review.
//!
//! Scans `.mana/` for units that are closed and haven't been reviewed,
//! scores them, and returns a ranked queue.

use anyhow::Result;
use std::path::Path;

use crate::risk;
use crate::types::{FileChange, QueueEntry};

/// Build the review queue from the current `.mana/` state.
///
/// Returns units that are closed (verify passed) but haven't been
/// reviewed yet, ranked by risk level (highest first).
pub fn build(mana_dir: &Path, project_root: &Path) -> Result<Vec<QueueEntry>> {
    let index = mana_core::api::load_index(mana_dir)?;

    let mut entries = Vec::new();

    for unit_entry in &index.units {
        // Only show closed units (verify passed)
        if unit_entry.status != mana_core::api::Status::Closed {
            continue;
        }

        // Skip features — they're human-reviewed at a higher level
        if unit_entry.feature {
            continue;
        }

        // Skip units that already have a persisted review record.
        if crate::state::has_review(mana_dir, &unit_entry.id) {
            continue;
        }

        // Load the full unit for risk scoring
        let unit = mana_core::api::get_unit(mana_dir, &unit_entry.id)?;

        let file_changes = get_file_changes_for_unit(project_root, &unit)?;

        let (risk_level, risk_flags) = risk::score(&unit, &file_changes);

        let total_additions: u32 = file_changes.iter().map(|fc| fc.additions).sum();
        let total_deletions: u32 = file_changes.iter().map(|fc| fc.deletions).sum();

        entries.push(QueueEntry {
            unit_id: unit.id.clone(),
            title: unit.title.clone(),
            risk_level,
            risk_flags,
            attempt: unit.attempts,
            file_count: file_changes.len(),
            additions: total_additions,
            deletions: total_deletions,
        });
    }

    // Sort: Critical first, then High, Normal, Low
    entries.sort_by(|a, b| b.risk_level.cmp(&a.risk_level));

    Ok(entries)
}

/// Get file changes for a unit.
///
/// Uses the unit's checkpoint (if available) to compute the diff
/// against the state before work began.
fn get_file_changes_for_unit(
    project_root: &Path,
    unit: &mana_core::unit::Unit,
) -> Result<Vec<FileChange>> {
    let Some(checkpoint) = unit.checkpoint.as_deref() else {
        return Ok(vec![]);
    };

    let file_changes = crate::diff::compute(project_root, Some(checkpoint))
        .map(|(_, file_changes)| file_changes)
        .unwrap_or_default();

    Ok(file_changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state;
    use crate::types::{Review, ReviewDecision};
    use chrono::Utc;
    use mana_core::unit::{Status, Unit, UnitKind};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use tempfile::TempDir;

    fn run_git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {:?} failed", args);
    }

    fn git_stdout(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git").args(args).current_dir(dir).output().unwrap();
        assert!(output.status.success(), "git {:?} failed", args);
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn setup_git_project() -> (TempDir, PathBuf, PathBuf) {
        let dir = TempDir::new().unwrap();
        let project_root = dir.path().to_path_buf();
        let mana_dir = project_root.join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        run_git(&project_root, &["init"]);
        run_git(&project_root, &["config", "user.email", "test@test.com"]);
        run_git(&project_root, &["config", "user.name", "Test"]);

        fs::write(project_root.join("README.md"), "initial\n").unwrap();
        run_git(&project_root, &["add", "README.md"]);
        run_git(&project_root, &["commit", "-m", "initial"]);

        (dir, project_root, mana_dir)
    }

    fn write_unit(mana_dir: &Path, unit: &Unit) {
        let path = mana_dir.join(format!("{}-{}.md", unit.id, unit.title.to_lowercase().replace(' ', "-")));
        unit.to_file(path).unwrap();
    }

    fn make_closed_unit(id: &str, title: &str) -> Unit {
        let mut unit = Unit::new(id, title);
        unit.status = Status::Closed;
        unit.kind = UnitKind::Job;
        unit.closed_at = Some(Utc::now());
        unit
    }

    fn make_review(unit_id: &str) -> Review {
        Review {
            unit_id: unit_id.into(),
            attempt: 1,
            decision: ReviewDecision::Approved,
            summary: Some("Looks good".into()),
            annotations: vec![],
            reviewed_at: Utc::now(),
            reviewer: "human".into(),
        }
    }

    #[test]
    fn reviewed_unit_excluded_from_queue() {
        let (_dir, project_root, mana_dir) = setup_git_project();

        let unit = make_closed_unit("1", "Reviewed task");
        write_unit(&mana_dir, &unit);
        state::save(&mana_dir, &make_review("1")).unwrap();

        let queue = build(&mana_dir, &project_root).unwrap();
        assert!(queue.is_empty());
    }

    #[test]
    fn unreviewed_closed_unit_included_in_queue() {
        let (_dir, project_root, mana_dir) = setup_git_project();

        let unit = make_closed_unit("1", "Unreviewed task");
        write_unit(&mana_dir, &unit);

        let queue = build(&mana_dir, &project_root).unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].unit_id, "1");
        assert_eq!(queue[0].title, "Unreviewed task");
    }

    #[test]
    fn queue_uses_diff_backed_file_stats_when_checkpoint_exists() {
        let (_dir, project_root, mana_dir) = setup_git_project();

        let checkpoint = git_stdout(&project_root, &["rev-parse", "HEAD"]);
        fs::write(project_root.join("src.rs"), "fn main() {\n    println!(\"hi\");\n}\n").unwrap();
        run_git(&project_root, &["add", "src.rs"]);
        run_git(&project_root, &["commit", "-m", "add src"]);

        let mut unit = make_closed_unit("1", "Diff backed task");
        unit.checkpoint = Some(checkpoint);
        unit.paths = vec!["src.rs".into()];
        write_unit(&mana_dir, &unit);

        let queue = build(&mana_dir, &project_root).unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].file_count, 1);
        assert_eq!(queue[0].additions, 3);
        assert_eq!(queue[0].deletions, 0);
    }

    #[test]
    fn missing_checkpoint_keeps_unit_in_queue_with_empty_stats() {
        let (_dir, project_root, mana_dir) = setup_git_project();

        let unit = make_closed_unit("1", "No checkpoint task");
        write_unit(&mana_dir, &unit);

        let queue = build(&mana_dir, &project_root).unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].file_count, 0);
        assert_eq!(queue[0].additions, 0);
        assert_eq!(queue[0].deletions, 0);
    }

    #[test]
    fn invalid_checkpoint_keeps_unit_in_queue_with_empty_stats() {
        let (_dir, project_root, mana_dir) = setup_git_project();

        let mut unit = make_closed_unit("1", "Bad checkpoint task");
        unit.checkpoint = Some("not-a-real-ref".into());
        write_unit(&mana_dir, &unit);

        let queue = build(&mana_dir, &project_root).unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].file_count, 0);
        assert_eq!(queue[0].additions, 0);
        assert_eq!(queue[0].deletions, 0);
    }
}
