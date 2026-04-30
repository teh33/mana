//! Git worktree detection and merge utilities.
//!
//! This module provides functions to detect if the current directory is within
//! a git worktree, and to merge changes back to the main branch.

use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::process::Command;

/// Result of a merge operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeResult {
    /// Merge completed successfully
    Success,
    /// Merge had conflicts that need resolution
    Conflict { files: Vec<String> },
    /// Nothing to commit (no changes)
    NothingToCommit,
}

/// Information about a git worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeInfo {
    /// Path to the main worktree
    pub main_path: PathBuf,
    /// Current worktree path
    pub worktree_path: PathBuf,
    /// Branch name of current worktree
    pub branch: String,
}

/// Parsed worktree entry from `git worktree list --porcelain` output.
#[derive(Debug)]
struct WorktreeEntry {
    path: PathBuf,
    branch: Option<String>,
}

/// Parse the output of `git worktree list --porcelain`.
///
/// Format:
/// ```text
/// worktree /path/to/worktree
/// HEAD abc123
/// branch refs/heads/main
///
/// worktree /path/to/another
/// HEAD def456
/// branch refs/heads/feature
/// ```
fn parse_worktree_list(output: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;

    for line in output.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            // Save previous entry if exists
            if let Some(path) = current_path.take() {
                entries.push(WorktreeEntry {
                    path,
                    branch: current_branch.take(),
                });
            }
            current_path = Some(PathBuf::from(path));
            current_branch = None;
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            // Extract branch name from refs/heads/...
            current_branch = Some(
                branch_ref
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch_ref)
                    .to_string(),
            );
        }
        // Ignore HEAD and other lines
    }

    // Don't forget the last entry
    if let Some(path) = current_path {
        entries.push(WorktreeEntry {
            path,
            branch: current_branch,
        });
    }

    entries
}

/// Detect if the given directory is within a git worktree.
///
/// Uses the provided `cwd` path to determine which worktree (if any)
/// the directory belongs to. This avoids relying on process-global
/// `std::env::current_dir()` which is unsafe in multi-threaded tests.
///
/// Returns:
/// - `Ok(None)` if not in a git repo or in the main worktree
/// - `Ok(Some(WorktreeInfo))` if in a secondary worktree
/// - `Err` if there's an error running git commands
pub fn detect_worktree(cwd: &std::path::Path) -> Result<Option<WorktreeInfo>> {
    // Run git worktree list --porcelain from the given directory
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(cwd)
        .output();

    let output = match output {
        Ok(o) => o,
        Err(_) => return Ok(None), // git not available
    };

    if !output.status.success() {
        // Not in a git repo or git error
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let entries = parse_worktree_list(&stdout);

    if entries.is_empty() {
        return Ok(None);
    }

    // First entry is always the main worktree
    let main_entry = &entries[0];
    let main_path = &main_entry.path;

    // Find which worktree we're in by checking if cwd starts with any worktree path
    // We need to find the most specific match (longest path)
    let mut current_entry: Option<&WorktreeEntry> = None;
    for entry in &entries {
        if cwd.starts_with(&entry.path) {
            match current_entry {
                None => current_entry = Some(entry),
                Some(prev) if entry.path.as_os_str().len() > prev.path.as_os_str().len() => {
                    current_entry = Some(entry)
                }
                _ => {}
            }
        }
    }

    let current_entry = match current_entry {
        Some(e) => e,
        None => return Ok(None), // Not in any known worktree
    };

    // If we're in the main worktree, return None
    if current_entry.path == *main_path {
        return Ok(None);
    }

    // We're in a secondary worktree
    Ok(Some(WorktreeInfo {
        main_path: main_path.clone(),
        worktree_path: current_entry.path.clone(),
        branch: current_entry.branch.clone().unwrap_or_default(),
    }))
}

/// Commit the specified paths in the worktree directory.
///
/// Runs `git add -A -- <paths>` followed by `git commit -m <message>` in the
/// given directory. Paths must be relative to the repository root. `-A` is scoped
/// by the explicit pathspecs so deletions for target paths are recorded without
/// staging unrelated worktree changes.
pub fn commit_worktree_paths(
    cwd: &std::path::Path,
    message: &str,
    paths: &[String],
) -> Result<bool> {
    if paths.is_empty() {
        return Ok(false);
    }

    let add_output = Command::new("git")
        .arg("add")
        .arg("-A")
        .arg("--")
        .args(paths)
        .current_dir(cwd)
        .output()?;

    if !add_output.status.success() {
        return Err(anyhow!(
            "git add failed: {}",
            String::from_utf8_lossy(&add_output.stderr)
        ));
    }

    commit_staged_changes(cwd, message)
}

/// Commit all changes in the specified worktree directory.
///
/// Runs `git add -A` followed by `git commit -m <message>` in the given directory.
/// Uses an explicit `cwd` to avoid relying on the process-global current directory.
///
/// Returns:
/// - `Ok(true)` if a commit was made
/// - `Ok(false)` if there was nothing to commit
/// - `Err` if git commands fail
pub fn commit_worktree_changes(cwd: &std::path::Path, message: &str) -> Result<bool> {
    // Stage all changes
    let add_output = Command::new("git")
        .args(["add", "-A"])
        .current_dir(cwd)
        .output()?;

    if !add_output.status.success() {
        return Err(anyhow!(
            "git add failed: {}",
            String::from_utf8_lossy(&add_output.stderr)
        ));
    }

    commit_staged_changes(cwd, message)
}

fn commit_staged_changes(cwd: &std::path::Path, message: &str) -> Result<bool> {
    // Commit changes
    let commit_output = Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(cwd)
        .output()?;

    if commit_output.status.success() {
        return Ok(true);
    }

    // Check if it failed because there was nothing to commit
    let stderr = String::from_utf8_lossy(&commit_output.stderr);
    let stdout = String::from_utf8_lossy(&commit_output.stdout);
    if stderr.contains("nothing to commit")
        || stdout.contains("nothing to commit")
        || stderr.contains("no changes added")
        || stdout.contains("no changes added")
    {
        return Ok(false);
    }

    Err(anyhow!("git commit failed: {}", stderr))
}

/// Merge the worktree branch to main.
///
/// Performs a no-fast-forward merge from the worktree's branch to the main branch.
/// If there are conflicts, aborts the merge and returns the conflicting files.
///
/// # Arguments
/// * `info` - Information about the worktree
/// * `unit_id` - Unit ID to include in the commit message
///
/// Returns:
/// - `Ok(MergeResult::Success)` if merge completed
/// - `Ok(MergeResult::Conflict { files })` if there were conflicts
/// - `Ok(MergeResult::NothingToCommit)` if branch is already merged
/// - `Err` if git commands fail unexpectedly
pub fn merge_to_main(info: &WorktreeInfo, unit_id: &str) -> Result<MergeResult> {
    let main_path = &info.main_path;
    let branch = &info.branch;

    if branch.is_empty() {
        return Err(anyhow!("Worktree has no branch (detached HEAD?)"));
    }

    // Perform the merge from the main worktree
    let merge_message = format!("Merge branch '{}' (unit {})", branch, unit_id);
    let merge_output = Command::new("git")
        .args(["-C", main_path.to_str().unwrap_or(".")])
        .args(["merge", branch, "--no-ff", "-m", &merge_message])
        .output()?;

    if merge_output.status.success() {
        return Ok(MergeResult::Success);
    }

    let stderr = String::from_utf8_lossy(&merge_output.stderr);
    let stdout = String::from_utf8_lossy(&merge_output.stdout);

    // Check if already up-to-date
    if stdout.contains("Already up to date") || stderr.contains("Already up to date") {
        return Ok(MergeResult::NothingToCommit);
    }

    // Check for conflicts
    if stdout.contains("CONFLICT") || stderr.contains("CONFLICT") {
        // Get list of conflicting files
        let conflicts = parse_conflict_files(&stdout, &stderr);

        // Abort the merge
        let _ = Command::new("git")
            .args(["-C", main_path.to_str().unwrap_or(".")])
            .args(["merge", "--abort"])
            .output();

        return Ok(MergeResult::Conflict { files: conflicts });
    }

    Err(anyhow!("git merge failed: {}", stderr))
}

/// Parse conflicting files from merge output.
fn parse_conflict_files(stdout: &str, stderr: &str) -> Vec<String> {
    let combined = format!("{}\n{}", stdout, stderr);
    let mut files = Vec::new();

    for line in combined.lines() {
        // Match lines like "CONFLICT (content): Merge conflict in <file>"
        if let Some(idx) = line.find("Merge conflict in ") {
            let file = line[idx + "Merge conflict in ".len()..].trim();
            files.push(file.to_string());
        }
        // Match lines like "CONFLICT (add/add): Merge conflict in <file>"
        // or "CONFLICT (modify/delete): <file> deleted in ..."
        else if line.starts_with("CONFLICT") {
            // Try to extract filename from various CONFLICT formats
            if let Some(colon_idx) = line.find("):") {
                let rest = &line[colon_idx + 2..].trim();
                // Get first word which might be the filename
                if let Some(word) = rest.split_whitespace().next() {
                    if !word.is_empty() && word != "Merge" && !files.contains(&word.to_string()) {
                        files.push(word.to_string());
                    }
                }
            }
        }
    }

    files
}

/// Clean up a worktree and its branch.
///
/// Removes the worktree directory and deletes the associated branch.
///
/// # Arguments
/// * `info` - Information about the worktree to clean up
pub fn cleanup_worktree(info: &WorktreeInfo) -> Result<()> {
    let main_path = &info.main_path;
    let worktree_path = &info.worktree_path;
    let branch = &info.branch;

    // Remove the worktree
    let remove_output = Command::new("git")
        .args(["-C", main_path.to_str().unwrap_or(".")])
        .args(["worktree", "remove", worktree_path.to_str().unwrap_or(".")])
        .output()?;

    if !remove_output.status.success() {
        // Try force remove if normal remove fails
        let force_output = Command::new("git")
            .args(["-C", main_path.to_str().unwrap_or(".")])
            .args([
                "worktree",
                "remove",
                "--force",
                worktree_path.to_str().unwrap_or("."),
            ])
            .output()?;

        if !force_output.status.success() {
            return Err(anyhow!(
                "Failed to remove worktree: {}",
                String::from_utf8_lossy(&force_output.stderr)
            ));
        }
    }

    // Delete the branch (only if we have a branch name)
    if !branch.is_empty() {
        let delete_output = Command::new("git")
            .args(["-C", main_path.to_str().unwrap_or(".")])
            .args(["branch", "-d", branch])
            .output()?;

        if !delete_output.status.success() {
            // Try force delete if normal delete fails (branch not fully merged)
            let force_delete = Command::new("git")
                .args(["-C", main_path.to_str().unwrap_or(".")])
                .args(["branch", "-D", branch])
                .output()?;

            if !force_delete.status.success() {
                return Err(anyhow!(
                    "Failed to delete branch '{}': {}",
                    branch,
                    String::from_utf8_lossy(&force_delete.stderr)
                ));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_worktree_list_single() {
        let output = "worktree /home/user/project\nHEAD abc123\nbranch refs/heads/main\n";
        let entries = parse_worktree_list(output);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, PathBuf::from("/home/user/project"));
        assert_eq!(entries[0].branch, Some("main".to_string()));
    }

    #[test]
    fn test_parse_worktree_list_multiple() {
        let output = r#"worktree /home/user/project
HEAD abc123
branch refs/heads/main

worktree /home/user/project-feature
HEAD def456
branch refs/heads/feature-x
"#;
        let entries = parse_worktree_list(output);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, PathBuf::from("/home/user/project"));
        assert_eq!(entries[0].branch, Some("main".to_string()));
        assert_eq!(entries[1].path, PathBuf::from("/home/user/project-feature"));
        assert_eq!(entries[1].branch, Some("feature-x".to_string()));
    }

    #[test]
    fn test_parse_worktree_list_detached_head() {
        let output = "worktree /home/user/project\nHEAD abc123\ndetached\n";
        let entries = parse_worktree_list(output);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, PathBuf::from("/home/user/project"));
        assert_eq!(entries[0].branch, None);
    }

    #[test]
    fn detect_worktree_runs_without_panic() {
        // This test just ensures the function doesn't panic
        // The actual result depends on the environment
        let cwd = std::env::current_dir().unwrap();
        let result = detect_worktree(&cwd);
        assert!(result.is_ok());
    }

    // Merge-related tests
    mod merge {
        use super::*;

        #[test]
        fn test_merge_result_variants() {
            // Test that MergeResult variants can be constructed and compared
            let success = MergeResult::Success;
            let conflict = MergeResult::Conflict {
                files: vec!["file1.txt".to_string(), "file2.txt".to_string()],
            };
            let nothing = MergeResult::NothingToCommit;

            assert_eq!(success, MergeResult::Success);
            assert_eq!(nothing, MergeResult::NothingToCommit);

            if let MergeResult::Conflict { files } = conflict {
                assert_eq!(files.len(), 2);
                assert!(files.contains(&"file1.txt".to_string()));
            } else {
                unreachable!("Expected Conflict variant");
            }
        }

        #[test]
        fn test_parse_conflict_files_content_conflict() {
            let stdout =
                "Auto-merging src/lib.rs\nCONFLICT (content): Merge conflict in src/lib.rs\n";
            let stderr = "";
            let files = parse_conflict_files(stdout, stderr);
            assert_eq!(files, vec!["src/lib.rs"]);
        }

        #[test]
        fn test_parse_conflict_files_multiple() {
            let stdout = r#"Auto-merging file1.txt
CONFLICT (content): Merge conflict in file1.txt
Auto-merging file2.txt
CONFLICT (content): Merge conflict in file2.txt
"#;
            let files = parse_conflict_files(stdout, "");
            assert_eq!(files.len(), 2);
            assert!(files.contains(&"file1.txt".to_string()));
            assert!(files.contains(&"file2.txt".to_string()));
        }

        #[test]
        fn test_parse_conflict_files_empty() {
            let files = parse_conflict_files("", "");
            assert!(files.is_empty());
        }

        #[test]
        fn test_parse_conflict_files_no_conflicts() {
            let stdout = "Already up to date.\n";
            let files = parse_conflict_files(stdout, "");
            assert!(files.is_empty());
        }

        #[test]
        fn test_worktree_info_for_merge() {
            // Test that WorktreeInfo can be used with merge functions
            let info = WorktreeInfo {
                main_path: PathBuf::from("/home/user/project"),
                worktree_path: PathBuf::from("/home/user/project-feature"),
                branch: "feature-branch".to_string(),
            };

            assert_eq!(info.branch, "feature-branch");
            assert_eq!(info.main_path, PathBuf::from("/home/user/project"));
            assert_eq!(
                info.worktree_path,
                PathBuf::from("/home/user/project-feature")
            );
        }

        #[test]
        fn test_merge_to_main_requires_branch() {
            // Test that merge_to_main fails with empty branch
            let info = WorktreeInfo {
                main_path: PathBuf::from("/tmp/nonexistent"),
                worktree_path: PathBuf::from("/tmp/nonexistent-wt"),
                branch: String::new(), // Empty branch
            };

            let result = merge_to_main(&info, "test-unit");
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.to_string().contains("no branch"));
        }

        #[test]
        fn test_commit_worktree_changes_type_signature() {
            // This test verifies the function signature and return type
            // by calling it - it will likely fail (not in git repo) but
            // shouldn't panic
            let cwd = std::env::current_dir().unwrap();
            let result = commit_worktree_changes(&cwd, "test message");
            // Result should be Ok or Err, not panic
            let _ = result;
        }

        #[test]
        fn test_cleanup_worktree_type_signature() {
            // This test verifies the function signature works correctly
            let info = WorktreeInfo {
                main_path: PathBuf::from("/tmp/nonexistent-main"),
                worktree_path: PathBuf::from("/tmp/nonexistent-wt"),
                branch: "test-branch".to_string(),
            };

            // This will fail because the paths don't exist, but shouldn't panic
            let result = cleanup_worktree(&info);
            assert!(result.is_err()); // Expected to fail with nonexistent paths
        }
    }
}
