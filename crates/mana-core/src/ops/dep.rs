use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use chrono::Utc;

use serde::{Deserialize, Serialize};

use crate::discovery::find_unit_file;
use crate::graph::detect_cycle;
use crate::index::{Index, IndexEntry};
use crate::unit::Unit;

/// Result of adding a dependency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepAddResult {
    pub from_id: String,
    pub to_id: String,
}

/// Result of removing a dependency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepRemoveResult {
    pub from_id: String,
    pub to_id: String,
}

/// A dependency relationship for display.
pub struct DepEntry {
    pub id: String,
    pub title: String,
    pub found: bool,
}

/// Result of listing dependencies for a unit.
pub struct DepListResult {
    pub id: String,
    pub dependencies: Vec<DepEntry>,
    pub dependents: Vec<DepEntry>,
}

/// Add a dependency: `from_id` depends on `depends_on_id`.
///
/// Validates both units exist, checks for self-dependency,
/// detects cycles, and persists the change.
pub fn dep_add(mana_dir: &Path, from_id: &str, depends_on_id: &str) -> Result<DepAddResult> {
    let unit_path =
        find_unit_file(mana_dir, from_id).map_err(|_| anyhow!("Unit {} not found", from_id))?;

    find_unit_file(mana_dir, depends_on_id)
        .map_err(|_| anyhow!("Unit {} not found", depends_on_id))?;

    if from_id == depends_on_id {
        return Err(anyhow!(
            "Cannot add self-dependency: {} cannot depend on itself",
            from_id
        ));
    }

    let index = Index::load_or_rebuild(mana_dir)?;

    if detect_cycle(&index, from_id, depends_on_id)? {
        return Err(anyhow!(
            "Dependency cycle detected: adding {} -> {} would create a cycle. Edge not added.",
            from_id,
            depends_on_id
        ));
    }

    let mut unit =
        Unit::from_file(&unit_path).with_context(|| format!("Failed to load unit: {}", from_id))?;

    if unit.dependencies.contains(&depends_on_id.to_string()) {
        return Err(anyhow!(
            "Unit {} already depends on {}",
            from_id,
            depends_on_id
        ));
    }

    unit.dependencies.push(depends_on_id.to_string());
    unit.updated_at = Utc::now();

    unit.to_file(&unit_path)
        .with_context(|| format!("Failed to save unit: {}", from_id))?;

    let index = Index::build(mana_dir).with_context(|| "Failed to rebuild index")?;
    index
        .save(mana_dir)
        .with_context(|| "Failed to save index")?;

    Ok(DepAddResult {
        from_id: from_id.to_string(),
        to_id: depends_on_id.to_string(),
    })
}

/// Remove a dependency: `from_id` no longer depends on `depends_on_id`.
pub fn dep_remove(mana_dir: &Path, from_id: &str, depends_on_id: &str) -> Result<DepRemoveResult> {
    let unit_path =
        find_unit_file(mana_dir, from_id).map_err(|_| anyhow!("Unit {} not found", from_id))?;

    let mut unit =
        Unit::from_file(&unit_path).with_context(|| format!("Failed to load unit: {}", from_id))?;

    let original_len = unit.dependencies.len();
    unit.dependencies.retain(|d| d != depends_on_id);

    if unit.dependencies.len() == original_len {
        return Err(anyhow!(
            "Unit {} does not depend on {}",
            from_id,
            depends_on_id
        ));
    }

    unit.updated_at = Utc::now();
    unit.to_file(&unit_path)
        .with_context(|| format!("Failed to save unit: {}", from_id))?;

    let index = Index::build(mana_dir).with_context(|| "Failed to rebuild index")?;
    index
        .save(mana_dir)
        .with_context(|| "Failed to save index")?;

    Ok(DepRemoveResult {
        from_id: from_id.to_string(),
        to_id: depends_on_id.to_string(),
    })
}

/// List dependencies and dependents for a unit.
pub fn dep_list(mana_dir: &Path, id: &str) -> Result<DepListResult> {
    let index = Index::load_or_rebuild(mana_dir)?;

    let entry = index
        .units
        .iter()
        .find(|e| e.id == id)
        .ok_or_else(|| anyhow!("Unit {} not found", id))?;

    let id_map: HashMap<String, &IndexEntry> =
        index.units.iter().map(|e| (e.id.clone(), e)).collect();

    let dependencies: Vec<DepEntry> = entry
        .dependencies
        .iter()
        .map(|dep_id| {
            if let Some(dep_entry) = id_map.get(dep_id) {
                DepEntry {
                    id: dep_entry.id.clone(),
                    title: dep_entry.title.clone(),
                    found: true,
                }
            } else {
                DepEntry {
                    id: dep_id.clone(),
                    title: String::new(),
                    found: false,
                }
            }
        })
        .collect();

    let dependents: Vec<DepEntry> = index
        .units
        .iter()
        .filter(|e| e.dependencies.contains(&id.to_string()))
        .map(|e| DepEntry {
            id: e.id.clone(),
            title: e.title.clone(),
            found: true,
        })
        .collect();

    Ok(DepListResult {
        id: id.to_string(),
        dependencies,
        dependents,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::title_to_slug;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_mana_dir() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();
        (dir, mana_dir)
    }

    fn create_unit(mana_dir: &Path, unit: &Unit) {
        let slug = title_to_slug(&unit.title);
        let filename = format!("{}-{}.md", unit.id, slug);
        unit.to_file(mana_dir.join(filename)).unwrap();
    }

    #[test]
    fn test_dep_add_simple() {
        let (_dir, mana_dir) = setup_test_mana_dir();
        let unit1 = Unit::new("1", "Task 1");
        let unit2 = Unit::new("2", "Task 2");
        create_unit(&mana_dir, &unit1);
        create_unit(&mana_dir, &unit2);

        let result = dep_add(&mana_dir, "1", "2").unwrap();
        assert_eq!(result.from_id, "1");
        assert_eq!(result.to_id, "2");

        let updated = Unit::from_file(mana_dir.join("1-task-1.md")).unwrap();
        assert_eq!(updated.dependencies, vec!["2".to_string()]);
    }

    #[test]
    fn test_dep_add_self_dependency_rejected() {
        let (_dir, mana_dir) = setup_test_mana_dir();
        let unit1 = Unit::new("1", "Task 1");
        create_unit(&mana_dir, &unit1);

        let result = dep_add(&mana_dir, "1", "1");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("self-dependency"));
    }

    #[test]
    fn test_dep_add_nonexistent_unit() {
        let (_dir, mana_dir) = setup_test_mana_dir();
        let unit1 = Unit::new("1", "Task 1");
        create_unit(&mana_dir, &unit1);

        let result = dep_add(&mana_dir, "1", "999");
        assert!(result.is_err());
    }

    #[test]
    fn test_dep_add_cycle_detection() {
        let (_dir, mana_dir) = setup_test_mana_dir();
        let mut unit1 = Unit::new("1", "Task 1");
        let unit2 = Unit::new("2", "Task 2");
        unit1.dependencies = vec!["2".to_string()];
        create_unit(&mana_dir, &unit1);
        create_unit(&mana_dir, &unit2);

        Index::build(&mana_dir).unwrap().save(&mana_dir).unwrap();

        let result = dep_add(&mana_dir, "2", "1");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cycle"));
    }

    #[test]
    fn test_dep_remove() {
        let (_dir, mana_dir) = setup_test_mana_dir();
        let mut unit1 = Unit::new("1", "Task 1");
        let unit2 = Unit::new("2", "Task 2");
        unit1.dependencies = vec!["2".to_string()];
        create_unit(&mana_dir, &unit1);
        create_unit(&mana_dir, &unit2);

        let result = dep_remove(&mana_dir, "1", "2").unwrap();
        assert_eq!(result.from_id, "1");
        assert_eq!(result.to_id, "2");

        let updated = Unit::from_file(mana_dir.join("1-task-1.md")).unwrap();
        assert_eq!(updated.dependencies, Vec::<String>::new());
    }

    #[test]
    fn test_dep_remove_not_found() {
        let (_dir, mana_dir) = setup_test_mana_dir();
        let unit1 = Unit::new("1", "Task 1");
        create_unit(&mana_dir, &unit1);

        let result = dep_remove(&mana_dir, "1", "2");
        assert!(result.is_err());
    }

    #[test]
    fn test_dep_list_with_dependencies() {
        let (_dir, mana_dir) = setup_test_mana_dir();
        let mut unit1 = Unit::new("1", "Task 1");
        let unit2 = Unit::new("2", "Task 2");
        let mut unit3 = Unit::new("3", "Task 3");
        unit1.dependencies = vec!["2".to_string()];
        unit3.dependencies = vec!["1".to_string()];
        create_unit(&mana_dir, &unit1);
        create_unit(&mana_dir, &unit2);
        create_unit(&mana_dir, &unit3);

        let result = dep_list(&mana_dir, "1").unwrap();
        assert_eq!(result.dependencies.len(), 1);
        assert_eq!(result.dependencies[0].id, "2");
        assert_eq!(result.dependents.len(), 1);
        assert_eq!(result.dependents[0].id, "3");
    }

    #[test]
    fn test_dep_add_duplicate_rejected() {
        let (_dir, mana_dir) = setup_test_mana_dir();
        let mut unit1 = Unit::new("1", "Task 1");
        let unit2 = Unit::new("2", "Task 2");
        unit1.dependencies = vec!["2".to_string()];
        create_unit(&mana_dir, &unit1);
        create_unit(&mana_dir, &unit2);

        let result = dep_add(&mana_dir, "1", "2");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already depends"));
    }
}
