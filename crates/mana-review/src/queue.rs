//! Review queue — list and rank units awaiting review.
//!
//! Scans `.mana/` for units that are closed and haven't been reviewed,
//! scores them, and returns a ranked queue.

use anyhow::Result;
use std::path::Path;

use crate::types::{FileChange, QueueEntry};
use crate::{diff, risk, state};

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
        if state::has_review(mana_dir, &unit_entry.id) {
            continue;
        }

        // Load the full unit for risk scoring
        let unit = mana_core::api::get_unit(mana_dir, &unit_entry.id)?;

        // Queue generation should stay robust even if diff evidence can't be computed
        // for this unit (missing checkpoint, unavailable git, bad repo state, etc.).
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

    let file_changes = diff::compute(project_root, Some(checkpoint))
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
    use mana_core::index::Index;
    use mana_core::unit::{Status, Unit};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use tempfile::TempDir;

    fn setup_project() -> (TempDir, PathBuf, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let project_root = tmp.path().to_path_buf();
        let mana_dir = project_root.join(".mana");
        fs::create_dir_all(&mana_dir).unwrap();
        (tmp, project_root, mana_dir)
    }

    fn write_unit(mana_dir: &Path, unit: &Unit) {
        let slug = unit
            .title
            .to_lowercase()
            .replace(|c: char| !c.is_ascii_alphanumeric(), "-")
            .trim_matches('-')
            .to_string();
        unit.to_file(mana_dir.join(format!("{}-{}.md", unit.id, slug)))
            .unwrap();
    }

    fn save_index(mana_dir: &Path) {
        let index = Index::build(mana_dir).unwrap();
        index.save(mana_dir).unwrap();
    }

    fn make_closed_unit(id: &str, title: &str) -> Unit {
        let mut unit = Unit::new(id, title);
        unit.status = Status::Closed;
        unit.closed_at = Some(Utc::now());
        unit.updated_at = Utc::now();
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

    fn git(project_root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(project_root)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );

        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn init_git_repo(project_root: &Path) {
        git(project_root, &["init"]);
        git(project_root, &["config", "user.name", "Mana Review Tests"]);
        git(
            project_root,
            &["config", "user.email", "mana-review-tests@example.com"],
        );
    }

    fn commit_all(project_root: &Path, message: &str) -> String {
        git(project_root, &["add", "."]);
        git(project_root, &["commit", "-m", message]);
        git(project_root, &["rev-parse", "HEAD"])
    }

    #[test]
    fn reviewed_unit_excluded_from_queue() {
        let (_tmp, project_root, mana_dir) = setup_project();

        let unit = make_closed_unit("1", "Reviewed unit");
        write_unit(&mana_dir, &unit);
        save_index(&mana_dir);
        state::save(&mana_dir, &make_review("1")).unwrap();

        let entries = build(&mana_dir, &project_root).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn unreviewed_closed_unit_included_in_queue() {
        let (_tmp, project_root, mana_dir) = setup_project();

        let unit = make_closed_unit("1", "Needs review");
        write_unit(&mana_dir, &unit);
        save_index(&mana_dir);

        let entries = build(&mana_dir, &project_root).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].unit_id, "1");
        assert_eq!(entries[0].title, "Needs review");
    }

    #[test]
    fn diff_backed_stats_populate_when_checkpoint_exists() {
        let (_tmp, project_root, mana_dir) = setup_project();
        init_git_repo(&project_root);

        let src_dir = project_root.join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("lib.rs"), "old\nkeep\n").unwrap();
        let checkpoint = commit_all(&project_root, "base");

        fs::write(src_dir.join("lib.rs"), "new\nkeep\nplus\n").unwrap();
        commit_all(&project_root, "change");

        let mut unit = make_closed_unit("1", "Diff-backed review");
        unit.checkpoint = Some(checkpoint);
        unit.paths = vec!["src/lib.rs".into()];
        write_unit(&mana_dir, &unit);
        save_index(&mana_dir);

        let entries = build(&mana_dir, &project_root).unwrap();
        assert_eq!(entries.len(), 1);

        let entry = &entries[0];
        assert_eq!(entry.file_count, 1);
        assert_eq!(entry.additions, 2);
        assert_eq!(entry.deletions, 1);
    }

    #[test]
    fn missing_checkpoint_keeps_unit_in_queue_with_empty_stats() {
        let (_tmp, project_root, mana_dir) = setup_project();
        init_git_repo(&project_root);

        fs::write(project_root.join("README.md"), "hello\n").unwrap();
        commit_all(&project_root, "init");

        let unit = make_closed_unit("1", "No checkpoint");
        write_unit(&mana_dir, &unit);
        save_index(&mana_dir);

        let entries = build(&mana_dir, &project_root).unwrap();
        assert_eq!(entries.len(), 1);

        let entry = &entries[0];
        assert_eq!(entry.file_count, 0);
        assert_eq!(entry.additions, 0);
        assert_eq!(entry.deletions, 0);
    }

    #[test]
    fn invalid_checkpoint_keeps_unit_in_queue_with_empty_stats() {
        let (_tmp, project_root, mana_dir) = setup_project();
        init_git_repo(&project_root);

        fs::write(project_root.join("README.md"), "hello\n").unwrap();
        commit_all(&project_root, "init");

        let mut unit = make_closed_unit("1", "Bad checkpoint");
        unit.checkpoint = Some("not-a-real-ref".into());
        write_unit(&mana_dir, &unit);
        save_index(&mana_dir);

        let entries = build(&mana_dir, &project_root).unwrap();
        assert_eq!(entries.len(), 1);

        let entry = &entries[0];
        assert_eq!(entry.file_count, 0);
        assert_eq!(entry.additions, 0);
        assert_eq!(entry.deletions, 0);
    }
}
