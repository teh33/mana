use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// Walk up from `start` looking for the nearest `.mana/` directory.
/// Returns the path to the `.mana/` directory if found.
/// Errors if no `.mana/` directory exists in any ancestor.
pub fn find_mana_dir(start: &Path) -> Result<PathBuf> {
    if let Some(found) = find_mana_dir_in_ancestors(start) {
        return Ok(found);
    }

    if let Ok(canonical_start) = start.canonicalize() {
        if canonical_start != start {
            if let Some(found) = find_mana_dir_in_ancestors(&canonical_start) {
                return Ok(found);
            }
        }
    }

    bail!("No .mana/ directory found. Run `mana init` first.");
}

/// Walk up from `start` looking for the outermost `.mana/` directory.
/// This is useful for distinguishing a nested project `.mana/` from an
/// ecosystem/root `.mana/` when both exist.
pub fn find_outermost_mana_dir(start: &Path) -> Result<PathBuf> {
    if let Some(found) = find_outermost_mana_dir_in_ancestors(start) {
        return Ok(found);
    }

    if let Ok(canonical_start) = start.canonicalize() {
        if canonical_start != start {
            if let Some(found) = find_outermost_mana_dir_in_ancestors(&canonical_start) {
                return Ok(found);
            }
        }
    }

    bail!("No .mana/ directory found. Run `mana init` first.");
}

fn find_mana_dir_in_ancestors(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        let candidate = current.join(".mana");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn find_outermost_mana_dir_in_ancestors(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    let mut found = None;
    loop {
        let candidate = current.join(".mana");
        if candidate.is_dir() {
            found = Some(candidate);
        }
        if !current.pop() {
            return found;
        }
    }
}

/// Find a unit file by ID, supporting both new and legacy naming conventions.
///
/// Searches for unit files in this order:
/// 1. New format: `{id}-{slug}.md` (e.g., "1-my-task.md", "11.1-refactor-parser.md")
/// 2. Legacy format: `{id}.yaml` (e.g., "1.yaml", "11.1.yaml")
///
/// Returns the full path if found.
///
/// # Examples
/// - `find_unit_file(mana_dir, "1")` → `.mana/1-my-task.md` or `.mana/1.yaml`
/// - `find_unit_file(mana_dir, "11.1")` → `.mana/11.1-refactor-parser.md` or `.mana/11.1.yaml`
///
/// # Arguments
/// * `mana_dir` - Path to the `.mana/` directory
/// * `id` - The unit ID to find (e.g., "1", "11.1", "3.2.1")
///
/// # Errors
/// * If the ID is invalid (empty, contains path traversal, etc.)
/// * If no unit file is found for the given ID
/// * If glob pattern matching fails
pub fn find_unit_file(mana_dir: &Path, id: &str) -> Result<PathBuf> {
    // Validate ID to prevent path traversal attacks
    crate::util::validate_unit_id(id)?;

    // First, try the new naming convention: {id}-{slug}.md
    let md_pattern = format!("{}/*{}-*.md", mana_dir.display(), id);
    for entry in glob::glob(&md_pattern).context("glob pattern failed")? {
        let path = entry?;
        if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
            // Check if filename matches {id}-*.md pattern exactly
            if filename.starts_with(&format!("{}-", id)) && filename.ends_with(".md") {
                return Ok(path);
            }
        }
    }

    // Fallback to legacy naming convention: {id}.yaml
    let yaml_path = mana_dir.join(format!("{}.yaml", id));
    if yaml_path.exists() {
        return Ok(yaml_path);
    }

    Err(anyhow::anyhow!("Unit {} not found", id))
}

/// Compute the archive path for a unit given its ID, slug, and date.
///
/// Returns the path: `.mana/archive/YYYY/MM/<id>-<slug>.md`
///
/// # Arguments
/// * `mana_dir` - Path to the `.mana/` directory
/// * `id` - The unit ID (e.g., "1", "11.1", "3.2.1")
/// * `slug` - The unit slug (derived from title)
/// * `ext` - The file extension (e.g., "md", "yaml")
/// * `date` - The date to use for year/month subdirectories
///
/// # Returns
/// A PathBuf representing `.mana/archive/YYYY/MM/<id>-<slug>.<ext>`
///
/// # Examples
/// ```ignore
/// let path = archive_path_for_unit(
///     Path::new(".mana"),
///     "12",
///     "unit-archive-system",
///     "md",
///     chrono::NaiveDate::from_ymd_opt(2026, 1, 31).unwrap()
/// );
/// // Returns: .mana/archive/2026/01/12-unit-archive-system.md
/// ```
pub fn archive_path_for_unit(
    mana_dir: &Path,
    id: &str,
    slug: &str,
    ext: &str,
    date: chrono::NaiveDate,
) -> PathBuf {
    let year = date.format("%Y").to_string();
    let month = date.format("%m").to_string();
    let filename = format!("{}-{}.{}", id, slug, ext);
    mana_dir
        .join("archive")
        .join(&year)
        .join(&month)
        .join(filename)
}

/// Find an archived unit by ID within the `.mana/archive/` directory tree.
///
/// Searches recursively through `.mana/archive/**/` for a unit file matching the given ID.
/// Returns the full path to the first matching unit file.
///
/// # Arguments
/// * `mana_dir` - Path to the `.mana/` directory
/// * `id` - The unit ID to search for
///
/// # Returns
/// `Ok(PathBuf)` with the path to the archived unit file if found
/// `Err` if the unit is not found in the archive
///
/// # Examples
/// ```ignore
/// let path = find_archived_unit(Path::new(".mana"), "12")?;
/// // Returns: .mana/archive/2026/01/12-unit-archive-system.md
/// ```
pub fn find_archived_unit(mana_dir: &Path, id: &str) -> Result<PathBuf> {
    // Validate ID to prevent path traversal attacks
    crate::util::validate_unit_id(id)?;

    let archive_dir = mana_dir.join("archive");

    // If archive directory doesn't exist, unit is not archived
    if !archive_dir.is_dir() {
        bail!(
            "Archived unit {} not found (archive directory does not exist)",
            id
        );
    }

    // Recursively search through year subdirectories
    for year_entry in std::fs::read_dir(&archive_dir).context("Failed to read archive directory")? {
        let year_path = year_entry?.path();
        if !year_path.is_dir() {
            continue;
        }

        // Search through month subdirectories
        for month_entry in std::fs::read_dir(&year_path).context("Failed to read year directory")? {
            let month_path = month_entry?.path();
            if !month_path.is_dir() {
                continue;
            }

            // Search through unit files in month directory
            for unit_entry in
                std::fs::read_dir(&month_path).context("Failed to read month directory")?
            {
                let unit_path = unit_entry?.path();
                if !unit_path.is_file() {
                    continue;
                }

                // Check if filename matches the pattern {id}-*.md
                if let Some(filename) = unit_path.file_name().and_then(|n| n.to_str()) {
                    if filename.starts_with(&format!("{}-", id)) && filename.ends_with(".md") {
                        return Ok(unit_path);
                    }
                }
            }
        }
    }

    Err(anyhow::anyhow!("Archived unit {} not found", id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_units_in_current_dir() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".mana")).unwrap();

        let result = find_mana_dir(dir.path()).unwrap();
        assert_eq!(result, dir.path().join(".mana"));
    }

    #[test]
    fn finds_units_in_parent_dir() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".mana")).unwrap();
        let child = dir.path().join("src");
        fs::create_dir(&child).unwrap();

        let result = find_mana_dir(&child).unwrap();
        assert_eq!(result, dir.path().join(".mana"));
    }

    #[cfg(unix)]
    #[test]
    fn finds_units_through_symlinked_start_path() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".mana")).unwrap();
        let real_child = dir.path().join("project");
        fs::create_dir(&real_child).unwrap();

        let symlink_root = tempfile::tempdir().unwrap();
        let link_path = symlink_root.path().join("linked-project");
        symlink(&real_child, &link_path).unwrap();

        let result = find_mana_dir(&link_path).unwrap();
        assert_eq!(
            result.canonicalize().unwrap(),
            dir.path().join(".mana").canonicalize().unwrap()
        );
    }

    #[test]
    fn finds_units_in_grandparent_dir() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".mana")).unwrap();
        let child = dir.path().join("src").join("deep");
        fs::create_dir_all(&child).unwrap();

        let result = find_mana_dir(&child).unwrap();
        assert_eq!(result, dir.path().join(".mana"));
    }

    #[test]
    fn returns_error_when_no_units_exists() {
        let dir = tempfile::tempdir().unwrap();
        let child = dir.path().join("some").join("nested").join("dir");
        fs::create_dir_all(&child).unwrap();

        let result = find_mana_dir(&child);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("No .mana/ directory found"),
            "Error message was: {}",
            err_msg
        );
    }

    #[test]
    fn prefers_closest_mana_dir() {
        let dir = tempfile::tempdir().unwrap();
        // Parent has .mana
        fs::create_dir(dir.path().join(".mana")).unwrap();
        // Child also has .mana
        let child = dir.path().join("subproject");
        fs::create_dir(&child).unwrap();
        fs::create_dir(child.join(".mana")).unwrap();

        let result = find_mana_dir(&child).unwrap();
        assert_eq!(result, child.join(".mana"));
    }

    #[test]
    fn find_outermost_prefers_highest_ancestor_mana_dir() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".mana")).unwrap();
        let child = dir.path().join("subproject");
        fs::create_dir(&child).unwrap();
        fs::create_dir(child.join(".mana")).unwrap();

        let result = find_outermost_mana_dir(&child).unwrap();
        assert_eq!(result, dir.path().join(".mana"));
    }

    // =====================================================================
    // Tests for find_unit_file()
    // =====================================================================

    #[test]
    fn find_unit_file_simple_id() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create a unit file with slug
        fs::write(mana_dir.join("1-my-task.md"), "test content").unwrap();

        let result = find_unit_file(&mana_dir, "1").unwrap();
        assert_eq!(result, mana_dir.join("1-my-task.md"));
    }

    #[test]
    fn find_unit_file_hierarchical_id() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create a unit file with hierarchical ID
        fs::write(mana_dir.join("11.1-refactor-parser.md"), "test content").unwrap();

        let result = find_unit_file(&mana_dir, "11.1").unwrap();
        assert_eq!(result, mana_dir.join("11.1-refactor-parser.md"));
    }

    #[test]
    fn find_unit_file_three_level_id() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create a unit file with three-level ID
        fs::write(mana_dir.join("3.2.1-deep-task.md"), "test content").unwrap();

        let result = find_unit_file(&mana_dir, "3.2.1").unwrap();
        assert_eq!(result, mana_dir.join("3.2.1-deep-task.md"));
    }

    #[test]
    fn find_unit_file_returns_first_match() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create multiple files that start with the same ID prefix
        // (shouldn't happen in practice, but test the behavior)
        fs::write(mana_dir.join("2-alpha.md"), "first").unwrap();
        fs::write(mana_dir.join("2-beta.md"), "second").unwrap();

        let result = find_unit_file(&mana_dir, "2").unwrap();
        // Should return one of the files (glob order is implementation-dependent)
        assert!(result.ends_with("2-alpha.md") || result.ends_with("2-beta.md"));
        assert!(result
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .ends_with(".md"));
    }

    #[test]
    fn find_unit_file_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Try to find a unit that doesn't exist
        let result = find_unit_file(&mana_dir, "999");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Unit 999 not found"));
    }

    #[test]
    fn find_unit_file_validates_id() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Try to find with invalid ID (path traversal attempt)
        let result = find_unit_file(&mana_dir, "../../../etc/passwd");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Invalid unit ID") || err_msg.contains("path traversal"));
    }

    #[test]
    fn find_unit_file_validates_empty_id() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Try to find with empty ID
        let result = find_unit_file(&mana_dir, "");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("cannot be empty") || err_msg.contains("invalid"));
    }

    #[test]
    fn find_unit_file_with_long_slug() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create a unit file with a long slug
        let long_slug = "implement-comprehensive-feature-with-full-test-coverage";
        let filename = format!("5-{}.md", long_slug);
        fs::write(mana_dir.join(&filename), "test content").unwrap();

        let result = find_unit_file(&mana_dir, "5").unwrap();
        assert!(result.to_str().unwrap().contains(long_slug));
    }

    #[test]
    fn find_unit_file_supports_legacy_yaml_files() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create a .yaml file (legacy format - should be found as fallback)
        fs::write(mana_dir.join("7.yaml"), "old format").unwrap();

        // Should find the legacy .yaml file
        let result = find_unit_file(&mana_dir, "7");
        assert!(result.is_ok());
        assert!(result.unwrap().ends_with("7.yaml"));
    }

    #[test]
    fn find_unit_file_prefers_md_over_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create both formats - .md should be preferred
        fs::write(mana_dir.join("7-my-task.md"), "new format").unwrap();
        fs::write(mana_dir.join("7.yaml"), "old format").unwrap();

        let result = find_unit_file(&mana_dir, "7");
        assert!(result.is_ok());
        assert!(result.unwrap().ends_with("7-my-task.md"));
    }

    #[test]
    fn find_unit_file_ignores_files_without_proper_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create a file that doesn't match the pattern
        fs::write(mana_dir.join("7-something-else.md"), "wrong file").unwrap();

        // Try to find "8" (which exists as "7-something-else.md")
        let result = find_unit_file(&mana_dir, "8");
        assert!(result.is_err());
    }

    #[test]
    fn find_unit_file_handles_numeric_id_prefix_matching() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create files: "2-task.md" and "20-task.md"
        fs::write(mana_dir.join("2-task.md"), "unit 2").unwrap();
        fs::write(mana_dir.join("20-task.md"), "unit 20").unwrap();

        // Looking for "2" should only match "2-task.md", not "20-task.md"
        let result = find_unit_file(&mana_dir, "2").unwrap();
        assert_eq!(result, mana_dir.join("2-task.md"));
    }

    #[test]
    fn find_unit_file_with_special_chars_in_slug() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create a unit file with hyphens and numbers in slug
        fs::write(mana_dir.join("6-v2-refactor-api.md"), "test").unwrap();

        let result = find_unit_file(&mana_dir, "6").unwrap();
        assert_eq!(result, mana_dir.join("6-v2-refactor-api.md"));
    }

    #[test]
    fn find_unit_file_rejects_special_chars_in_id() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Try IDs with special characters that should be rejected
        assert!(find_unit_file(&mana_dir, "task@home").is_err());
        assert!(find_unit_file(&mana_dir, "task#1").is_err());
        assert!(find_unit_file(&mana_dir, "task$money").is_err());
    }

    // =====================================================================
    // Tests for archive_path_for_unit()
    // =====================================================================

    #[test]
    fn archive_path_for_unit_basic() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");

        let date = chrono::NaiveDate::from_ymd_opt(2026, 1, 31).unwrap();
        let path = archive_path_for_unit(&mana_dir, "12", "unit-archive-system", "md", date);

        // Verify path structure
        assert_eq!(
            path,
            mana_dir.join("archive/2026/01/12-unit-archive-system.md")
        );
    }

    #[test]
    fn archive_path_for_unit_hierarchical_id() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");

        let date = chrono::NaiveDate::from_ymd_opt(2025, 12, 15).unwrap();
        let path = archive_path_for_unit(&mana_dir, "11.1", "refactor-parser", "md", date);

        assert_eq!(
            path,
            mana_dir.join("archive/2025/12/11.1-refactor-parser.md")
        );
    }

    #[test]
    fn archive_path_for_unit_single_digit_month() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");

        let date = chrono::NaiveDate::from_ymd_opt(2026, 3, 5).unwrap();
        let path = archive_path_for_unit(&mana_dir, "5", "task", "md", date);

        // Month should be zero-padded (03, not 3)
        assert_eq!(path, mana_dir.join("archive/2026/03/5-task.md"));
    }

    #[test]
    fn archive_path_for_unit_three_level_id() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");

        let date = chrono::NaiveDate::from_ymd_opt(2024, 8, 20).unwrap();
        let path = archive_path_for_unit(&mana_dir, "3.2.1", "deep-task", "md", date);

        assert_eq!(path, mana_dir.join("archive/2024/08/3.2.1-deep-task.md"));
    }

    #[test]
    fn archive_path_for_unit_long_slug() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");

        let date = chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let long_slug = "implement-comprehensive-feature-with-full-test-coverage";
        let path = archive_path_for_unit(&mana_dir, "42", long_slug, "md", date);

        assert!(path.to_str().unwrap().contains(long_slug));
        assert_eq!(
            path,
            mana_dir.join(
                "archive/2026/01/42-implement-comprehensive-feature-with-full-test-coverage.md"
            )
        );
    }

    #[test]
    fn archive_path_for_unit_yaml_extension() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");

        let date = chrono::NaiveDate::from_ymd_opt(2026, 1, 31).unwrap();
        let path = archive_path_for_unit(&mana_dir, "5", "yaml-task", "yaml", date);

        assert_eq!(path, mana_dir.join("archive/2026/01/5-yaml-task.yaml"));
    }

    // =====================================================================
    // Tests for find_archived_unit()
    // =====================================================================

    #[test]
    fn find_archived_unit_simple_id() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");
        let archive_dir = mana_dir.join("archive/2026/01");
        fs::create_dir_all(&archive_dir).unwrap();

        // Create an archived unit file
        fs::write(archive_dir.join("12-unit-archive.md"), "archived content").unwrap();

        let result = find_archived_unit(&mana_dir, "12").unwrap();
        assert_eq!(result, archive_dir.join("12-unit-archive.md"));
    }

    #[test]
    fn find_archived_unit_hierarchical_id() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");
        let archive_dir = mana_dir.join("archive/2025/12");
        fs::create_dir_all(&archive_dir).unwrap();

        // Create an archived unit file
        fs::write(
            archive_dir.join("11.1-refactor-parser.md"),
            "archived content",
        )
        .unwrap();

        let result = find_archived_unit(&mana_dir, "11.1").unwrap();
        assert_eq!(result, archive_dir.join("11.1-refactor-parser.md"));
    }

    #[test]
    fn find_archived_unit_multiple_years() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");

        // Create archive structure with multiple years
        fs::create_dir_all(mana_dir.join("archive/2024/06")).unwrap();
        fs::create_dir_all(mana_dir.join("archive/2025/12")).unwrap();
        fs::create_dir_all(mana_dir.join("archive/2026/01")).unwrap();

        // Create unit in 2024
        fs::write(
            mana_dir.join("archive/2024/06/5-old-task.md"),
            "old content",
        )
        .unwrap();

        // Create unit in 2026
        fs::write(
            mana_dir.join("archive/2026/01/12-new-task.md"),
            "new content",
        )
        .unwrap();

        // Should find the unit regardless of year
        let result = find_archived_unit(&mana_dir, "5").unwrap();
        assert!(result.to_str().unwrap().contains("2024/06"));

        let result = find_archived_unit(&mana_dir, "12").unwrap();
        assert!(result.to_str().unwrap().contains("2026/01"));
    }

    #[test]
    fn find_archived_unit_multiple_months() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");

        // Create archive structure with multiple months in same year
        fs::create_dir_all(mana_dir.join("archive/2026/01")).unwrap();
        fs::create_dir_all(mana_dir.join("archive/2026/02")).unwrap();
        fs::create_dir_all(mana_dir.join("archive/2026/03")).unwrap();

        // Create units in different months
        fs::write(
            mana_dir.join("archive/2026/01/10-january-task.md"),
            "january",
        )
        .unwrap();

        fs::write(mana_dir.join("archive/2026/03/10-march-task.md"), "march").unwrap();

        // Both should be found (returns first match)
        let result = find_archived_unit(&mana_dir, "10").unwrap();
        assert!(result.to_str().unwrap().contains("2026"));
        assert!(result
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("10-"));
    }

    #[test]
    fn find_archived_unit_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");
        let archive_dir = mana_dir.join("archive/2026/01");
        fs::create_dir_all(&archive_dir).unwrap();

        // Create a different unit
        fs::write(archive_dir.join("12-some-task.md"), "content").unwrap();

        // Try to find a unit that doesn't exist
        let result = find_archived_unit(&mana_dir, "999");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Archived unit 999 not found"));
    }

    #[test]
    fn find_archived_unit_no_archive_dir() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Archive directory doesn't exist
        let result = find_archived_unit(&mana_dir, "12");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Archived unit 12 not found"));
    }

    #[test]
    fn find_archived_unit_validates_id() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Try with invalid IDs (path traversal)
        let result = find_archived_unit(&mana_dir, "../../../etc/passwd");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid unit ID"));

        let result = find_archived_unit(&mana_dir, "");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be empty"));
    }

    #[test]
    fn find_archived_unit_three_level_id() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");
        let archive_dir = mana_dir.join("archive/2024/08");
        fs::create_dir_all(&archive_dir).unwrap();

        // Create an archived unit with three-level ID
        fs::write(archive_dir.join("3.2.1-deep-task.md"), "archived content").unwrap();

        let result = find_archived_unit(&mana_dir, "3.2.1").unwrap();
        assert_eq!(result, archive_dir.join("3.2.1-deep-task.md"));
    }

    #[test]
    fn find_archived_unit_ignores_non_matching_ids() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");
        let archive_dir = mana_dir.join("archive/2026/01");
        fs::create_dir_all(&archive_dir).unwrap();

        // Create units with similar IDs
        fs::write(archive_dir.join("1-first-task.md"), "unit 1").unwrap();
        fs::write(archive_dir.join("10-tenth-task.md"), "unit 10").unwrap();
        fs::write(archive_dir.join("100-hundredth-task.md"), "unit 100").unwrap();

        // Looking for "1" should only match "1-first-task.md", not "10-" or "100-"
        let result = find_archived_unit(&mana_dir, "1").unwrap();
        assert_eq!(result, archive_dir.join("1-first-task.md"));

        // Looking for "10" should only match "10-tenth-task.md"
        let result = find_archived_unit(&mana_dir, "10").unwrap();
        assert_eq!(result, archive_dir.join("10-tenth-task.md"));
    }

    #[test]
    fn find_archived_unit_with_long_slug() {
        let dir = tempfile::tempdir().unwrap();
        let mana_dir = dir.path().join(".mana");
        let archive_dir = mana_dir.join("archive/2026/01");
        fs::create_dir_all(&archive_dir).unwrap();

        let long_slug = "implement-comprehensive-feature-with-full-test-coverage";
        let filename = format!("42-{}.md", long_slug);
        fs::write(archive_dir.join(&filename), "archived").unwrap();

        let result = find_archived_unit(&mana_dir, "42").unwrap();
        assert!(result.to_str().unwrap().contains(long_slug));
    }
}
