use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;

use crate::discovery::find_unit_file;
use crate::index::Index;
use crate::unit::Unit;

/// Result of an adopt operation.
pub struct AdoptResult {
    /// Map of old_id -> new_id for adopted units.
    pub id_map: HashMap<String, String>,
}

/// Find the next available child number for a parent.
fn next_child_number(mana_dir: &Path, parent_id: &str) -> Result<u32> {
    let mut max_child: u32 = 0;

    let dir_entries = fs::read_dir(mana_dir)
        .with_context(|| format!("Failed to read directory: {}", mana_dir.display()))?;

    for entry in dir_entries {
        let entry = entry?;
        let path = entry.path();
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        if let Some(name_without_ext) = filename.strip_suffix(".md") {
            if let Some(name_without_parent) = name_without_ext.strip_prefix(parent_id) {
                if let Some(after_dot) = name_without_parent.strip_prefix('.') {
                    let num_part = after_dot.split('-').next().unwrap_or_default();
                    if let Ok(child_num) = num_part.parse::<u32>() {
                        if child_num > max_child {
                            max_child = child_num;
                        }
                    }
                }
            }
        }

        if let Some(name_without_ext) = filename.strip_suffix(".yaml") {
            if let Some(name_without_parent) = name_without_ext.strip_prefix(parent_id) {
                if let Some(after_dot) = name_without_parent.strip_prefix('.') {
                    if let Ok(child_num) = after_dot.parse::<u32>() {
                        if child_num > max_child {
                            max_child = child_num;
                        }
                    }
                }
            }
        }
    }

    Ok(max_child + 1)
}

/// Update dependency references in all units based on the ID mapping.
fn update_all_dependencies(mana_dir: &Path, id_map: &HashMap<String, String>) -> Result<()> {
    let dir_entries = fs::read_dir(mana_dir)
        .with_context(|| format!("Failed to read directory: {}", mana_dir.display()))?;

    for entry in dir_entries {
        let entry = entry?;
        let path = entry.path();
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        let is_unit_file = (filename.ends_with(".md") && filename.contains('-'))
            || (filename.ends_with(".yaml")
                && filename != "config.yaml"
                && filename != "index.yaml"
                && filename != "unit.yaml");

        if !is_unit_file {
            continue;
        }

        let mut unit = match Unit::from_file(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };

        let mut modified = false;
        let mut new_deps = Vec::new();

        for dep in &unit.dependencies {
            if let Some(new_id) = id_map.get(dep) {
                new_deps.push(new_id.clone());
                modified = true;
            } else {
                new_deps.push(dep.clone());
            }
        }

        if let Some(ref parent) = unit.parent {
            if let Some(new_parent) = id_map.get(parent) {
                unit.parent = Some(new_parent.clone());
                modified = true;
            }
        }

        if modified {
            unit.dependencies = new_deps;
            unit.updated_at = Utc::now();
            unit.to_file(&path)
                .with_context(|| format!("Failed to update unit {}", path.display()))?;
        }
    }

    Ok(())
}

/// Adopt existing units as children of a parent unit.
///
/// For each child ID, assigns a new sequential child ID under the parent,
/// renames the file, updates all dependency references, and rebuilds the index.
///
/// Returns a map of old_id -> new_id.
pub fn adopt(mana_dir: &Path, parent_id: &str, child_ids: &[String]) -> Result<AdoptResult> {
    let parent_path = find_unit_file(mana_dir, parent_id)
        .with_context(|| format!("Parent unit '{}' not found", parent_id))?;
    let _parent_unit = Unit::from_file(&parent_path)
        .with_context(|| format!("Failed to load parent unit '{}'", parent_id))?;

    let mut id_map: HashMap<String, String> = HashMap::new();
    for (next_num, old_id) in (next_child_number(mana_dir, parent_id)?..).zip(child_ids.iter()) {
        let old_path = find_unit_file(mana_dir, old_id)
            .with_context(|| format!("Child unit '{}' not found", old_id))?;
        let mut unit = Unit::from_file(&old_path)
            .with_context(|| format!("Failed to load child unit '{}'", old_id))?;

        let new_id = format!("{}.{}", parent_id, next_num);

        unit.id = new_id.clone();
        unit.parent = Some(parent_id.to_string());
        unit.updated_at = Utc::now();

        let slug = unit.slug.clone().unwrap_or_else(|| "unnamed".to_string());
        let new_filename = format!("{}-{}.md", new_id, slug);
        let new_path = mana_dir.join(&new_filename);

        unit.to_file(&new_path)
            .with_context(|| format!("Failed to write unit to {}", new_path.display()))?;

        if old_path != new_path {
            fs::remove_file(&old_path).with_context(|| {
                format!("Failed to remove old unit file {}", old_path.display())
            })?;
        }

        id_map.insert(old_id.clone(), new_id);
    }

    if !id_map.is_empty() {
        update_all_dependencies(mana_dir, &id_map)?;
    }

    let index = Index::build(mana_dir)?;
    index.save(mana_dir)?;

    Ok(AdoptResult { id_map })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use tempfile::TempDir;

    fn setup_mana_dir() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        Config {
            project: "test".to_string(),
            next_id: 10,
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

    #[test]
    fn adopt_single_unit() {
        let (_dir, mana_dir) = setup_mana_dir();

        let mut parent = Unit::new("1", "Parent task");
        parent.slug = Some("parent-task".to_string());
        parent.acceptance = Some("Children complete".to_string());
        parent.to_file(mana_dir.join("1-parent-task.md")).unwrap();

        let mut child = Unit::new("2", "Child task");
        child.slug = Some("child-task".to_string());
        child.verify = Some("cargo test unit::check".to_string());
        child.to_file(mana_dir.join("2-child-task.md")).unwrap();

        let result = adopt(&mana_dir, "1", &["2".to_string()]).unwrap();

        assert_eq!(result.id_map.get("2"), Some(&"1.1".to_string()));
        assert!(!mana_dir.join("2-child-task.md").exists());
        assert!(mana_dir.join("1.1-child-task.md").exists());
    }

    #[test]
    fn adopt_fails_for_missing_parent() {
        let (_dir, mana_dir) = setup_mana_dir();

        let mut child = Unit::new("2", "Child");
        child.slug = Some("child".to_string());
        child.to_file(mana_dir.join("2-child.md")).unwrap();

        let result = adopt(&mana_dir, "99", &["2".to_string()]);
        assert!(result.is_err());
    }

    #[test]
    fn adopt_updates_dependencies() {
        let (_dir, mana_dir) = setup_mana_dir();

        let mut parent = Unit::new("1", "Parent");
        parent.slug = Some("parent".to_string());
        parent.acceptance = Some("Done".to_string());
        parent.to_file(mana_dir.join("1-parent.md")).unwrap();

        let mut to_adopt = Unit::new("2", "To adopt");
        to_adopt.slug = Some("to-adopt".to_string());
        to_adopt.verify = Some("true".to_string());
        to_adopt.to_file(mana_dir.join("2-to-adopt.md")).unwrap();

        let mut dependent = Unit::new("3", "Dependent");
        dependent.slug = Some("dependent".to_string());
        dependent.verify = Some("true".to_string());
        dependent.dependencies = vec!["2".to_string()];
        dependent.to_file(mana_dir.join("3-dependent.md")).unwrap();

        adopt(&mana_dir, "1", &["2".to_string()]).unwrap();

        let dependent_updated = Unit::from_file(mana_dir.join("3-dependent.md")).unwrap();
        assert_eq!(dependent_updated.dependencies, vec!["1.1".to_string()]);
    }
}
