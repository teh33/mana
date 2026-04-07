//! Fast unit index cache.
//!
//! The index (`index.yaml`) is a compact summary of all active units, built
//! by scanning every unit file in the `.mana/` directory. It lets the CLI
//! answer list/filter/graph queries without parsing every individual unit file.
//!
//! The index is rebuilt automatically whenever unit files are newer than the
//! cached `index.yaml`. It is saved atomically to prevent corruption.
//!
//! ## Usage
//!
//! ```rust,no_run
//! use mana_core::index::Index;
//! use std::path::Path;
//!
//! let mana_dir = Path::new("/project/.mana");
//!
//! // Load from disk, rebuilding if stale
//! let index = Index::load_or_rebuild(mana_dir).unwrap();
//! println!("{} units", index.units.len());
//!
//! // Force a full rebuild from unit files
//! let index = Index::build(mana_dir).unwrap();
//! index.save(mana_dir).unwrap();
//! ```

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::unit::{Status, Unit, UnitKind};
use crate::util::{atomic_write, natural_cmp};

// ---------------------------------------------------------------------------
// IndexEntry
// ---------------------------------------------------------------------------

/// Default for `created_at` when deserializing old index files that lack the field.
fn default_created_at() -> DateTime<Utc> {
    DateTime::UNIX_EPOCH
}

/// A lightweight summary of a single unit, stored in the index cache.
///
/// `IndexEntry` contains only the fields needed for list/filter/graph
/// operations. For the full unit with description, notes, and history,
/// load the unit file directly via [`crate::unit::Unit::from_file`] or
/// [`crate::api::get_unit`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexEntry {
    pub id: String,
    pub title: String,
    pub status: Status,
    pub priority: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    pub updated_at: DateTime<Utc>,
    /// Artifacts this unit produces (for smart dependency inference)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub produces: Vec<String>,
    /// Artifacts this unit requires (for smart dependency inference)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    /// Whether this unit has a verify command (SPECs have verify, GOALs don't)
    #[serde(default)]
    pub has_verify: bool,
    /// The actual verify command string (so agents don't need bn show per-unit)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<String>,
    #[serde(default = "default_created_at")]
    pub created_at: DateTime<Utc>,
    /// Agent or user currently holding a claim on this unit (e.g., "spro:12345" for agent with PID)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
    /// Number of verify attempts so far
    #[serde(default)]
    pub attempts: u32,
    /// File paths this unit touches (for scope-based blocking)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// Explicit schema kind.
    pub kind: UnitKind,
    /// Whether this unit is a feature (product-level goal, human-only close)
    #[serde(default)]
    pub feature: bool,
    /// Whether this unit has unresolved decisions
    #[serde(default)]
    pub has_decisions: bool,
}

impl From<&Unit> for IndexEntry {
    fn from(unit: &Unit) -> Self {
        Self {
            id: unit.id.clone(),
            title: unit.title.clone(),
            status: unit.status,
            priority: unit.priority,
            parent: unit.parent.clone(),
            dependencies: unit.dependencies.clone(),
            labels: unit.labels.clone(),
            assignee: unit.assignee.clone(),
            updated_at: unit.updated_at,
            produces: unit.produces.clone(),
            requires: unit.requires.clone(),
            has_verify: unit.verify.is_some(),
            verify: unit.verify.clone(),
            created_at: unit.created_at,
            claimed_by: unit.claimed_by.clone(),
            attempts: unit.attempts,
            paths: unit.paths.clone(),
            kind: unit.kind,
            feature: unit.feature,
            has_decisions: !unit.decisions.is_empty(),
        }
    }
}

// ---------------------------------------------------------------------------
// Index
// ---------------------------------------------------------------------------

/// The in-memory and on-disk unit index.
///
/// Holds a flat list of [`IndexEntry`] values for all active (non-archived)
/// units in the project. Archived units are stored separately in
/// `.mana/archive/` and are not included here.
///
/// Obtain an index via [`Index::load_or_rebuild`] (lazy, cached) or
/// [`Index::build`] (always scans unit files from disk).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Index {
    /// All active units, sorted by ID in natural order.
    pub units: Vec<IndexEntry>,
}

// Files to exclude when scanning for unit YAMLs.
const EXCLUDED_FILES: &[&str] = &["config.yaml", "index.yaml", "unit.yaml", "archive.yaml"];

/// Check if a filename represents a unit file (not a config/index/template file).
fn is_unit_filename(filename: &str) -> bool {
    if EXCLUDED_FILES.contains(&filename) {
        return false;
    }
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|e| e.to_str());
    match ext {
        Some("md") => filename.contains('-'), // New format: {id}-{slug}.md
        Some("yaml") => true,                 // Legacy format: {id}.yaml
        _ => false,
    }
}

/// Count unit files by format in the units directory.
/// Returns (md_count, yaml_count) tuple.
pub fn count_unit_formats(mana_dir: &Path) -> Result<(usize, usize)> {
    let mut md_count = 0;
    let mut yaml_count = 0;

    let dir_entries = fs::read_dir(mana_dir)
        .with_context(|| format!("Failed to read directory: {}", mana_dir.display()))?;

    for entry in dir_entries {
        let entry = entry?;
        let path = entry.path();

        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        if !is_unit_filename(filename) {
            continue;
        }

        let ext = path.extension().and_then(|e| e.to_str());
        match ext {
            Some("md") => md_count += 1,
            Some("yaml") => yaml_count += 1,
            _ => {}
        }
    }

    Ok((md_count, yaml_count))
}

impl Index {
    /// Build the index by reading all unit files from the units directory.
    /// Supports both new format ({id}-{slug}.md) and legacy format ({id}.yaml).
    /// Excludes config.yaml, index.yaml, and unit.yaml.
    /// Sorts entries by ID using natural ordering.
    /// Returns an error if duplicate unit IDs are detected.
    pub fn build(mana_dir: &Path) -> Result<Self> {
        let mut entries = Vec::new();
        // Track which files define each ID to detect duplicates
        let mut id_to_files: HashMap<String, Vec<String>> = HashMap::new();

        let dir_entries = fs::read_dir(mana_dir)
            .with_context(|| format!("Failed to read directory: {}", mana_dir.display()))?;

        for entry in dir_entries {
            let entry = entry?;
            let path = entry.path();

            let filename = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();

            if !is_unit_filename(filename) {
                continue;
            }

            let unit = Unit::from_file(&path)
                .with_context(|| format!("Failed to parse unit: {}", path.display()))?;

            // Track this ID's file for duplicate detection
            id_to_files
                .entry(unit.id.clone())
                .or_default()
                .push(filename.to_string());

            entries.push(IndexEntry::from(&unit));
        }

        // Check for duplicate IDs
        let duplicates: Vec<_> = id_to_files
            .iter()
            .filter(|(_, files)| files.len() > 1)
            .collect();

        if !duplicates.is_empty() {
            let mut msg = String::from("Duplicate unit IDs detected:\n");
            for (id, files) in duplicates {
                msg.push_str(&format!("  ID '{}' defined in: {}\n", id, files.join(", ")));
            }
            return Err(anyhow!(msg));
        }

        entries.sort_by(|a, b| natural_cmp(&a.id, &b.id));

        Ok(Index { units: entries })
    }

    /// Check whether the cached index is stale.
    /// Returns true if the index file is missing or if any unit file (.md or .yaml)
    /// in the units directory has been modified after the index was last written.
    pub fn is_stale(mana_dir: &Path) -> Result<bool> {
        let index_path = mana_dir.join("index.yaml");

        // If index doesn't exist, it's stale
        if !index_path.exists() {
            return Ok(true);
        }

        let index_mtime = fs::metadata(&index_path)
            .with_context(|| "Failed to read index.yaml metadata")?
            .modified()
            .with_context(|| "Failed to get index.yaml mtime")?;

        let dir_entries = fs::read_dir(mana_dir)
            .with_context(|| format!("Failed to read directory: {}", mana_dir.display()))?;

        for entry in dir_entries {
            let entry = entry?;
            let path = entry.path();

            let filename = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();

            if !is_unit_filename(filename) {
                continue;
            }

            let file_mtime = fs::metadata(&path)
                .with_context(|| format!("Failed to read metadata: {}", path.display()))?
                .modified()
                .with_context(|| format!("Failed to get mtime: {}", path.display()))?;

            if file_mtime > index_mtime {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Load the cached index or rebuild it if stale.
    /// This is the main entry point for read-heavy commands.
    pub fn load_or_rebuild(mana_dir: &Path) -> Result<Self> {
        if Self::is_stale(mana_dir)? {
            let index = Self::build(mana_dir)?;
            index.save(mana_dir)?;
            Ok(index)
        } else {
            Self::load(mana_dir)
        }
    }

    /// Load the index from the cached index.yaml file.
    pub fn load(mana_dir: &Path) -> Result<Self> {
        let index_path = mana_dir.join("index.yaml");
        let contents = fs::read_to_string(&index_path)
            .with_context(|| format!("Failed to read {}", index_path.display()))?;
        let index: Index =
            serde_yml::from_str(&contents).with_context(|| "Failed to parse index.yaml")?;
        Ok(index)
    }

    /// Save the index to .mana/index.yaml.
    pub fn save(&self, mana_dir: &Path) -> Result<()> {
        let index_path = mana_dir.join("index.yaml");
        let yaml = serde_yml::to_string(self).with_context(|| "Failed to serialize index")?;
        atomic_write(&index_path, &yaml)
            .with_context(|| format!("Failed to write {}", index_path.display()))?;
        Ok(())
    }

    /// Collect all archived units from .mana/archive/ directory.
    /// Walks through year/month subdirectories and loads all unit files.
    /// Returns IndexEntry items for archived units.
    pub fn collect_archived(mana_dir: &Path) -> Result<Vec<IndexEntry>> {
        let mut entries = Vec::new();
        let archive_dir = mana_dir.join("archive");

        if !archive_dir.is_dir() {
            return Ok(entries);
        }

        // Walk through archive directory recursively
        Self::walk_archive_dir(&archive_dir, &mut entries)?;

        Ok(entries)
    }

    /// Recursively walk archive directory and collect unit entries.
    /// Uses catch_unwind to survive YAML parser panics from corrupt files.
    fn walk_archive_dir(dir: &Path, entries: &mut Vec<IndexEntry>) -> Result<()> {
        use crate::unit::Unit;

        if !dir.is_dir() {
            return Ok(());
        }

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                Self::walk_archive_dir(&path, entries)?;
            } else if path.is_file() {
                if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                    if is_unit_filename(filename) {
                        let path_clone = path.clone();
                        let result = std::panic::catch_unwind(|| Unit::from_file(&path_clone));
                        match result {
                            Ok(Ok(unit)) => entries.push(IndexEntry::from(&unit)),
                            Ok(Err(_)) => {} // normal parse error, skip silently
                            Err(_) => {
                                eprintln!(
                                    "warning: skipping corrupt archive file (parser panic): {}",
                                    path.display()
                                );
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ArchiveIndex — cached index of archived units
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArchiveIndex {
    pub units: Vec<IndexEntry>,
}

impl ArchiveIndex {
    /// Build the archive index by walking `.mana/archive/` recursively.
    /// Reuses `Index::collect_archived` to find all archived unit files,
    /// then sorts entries by ID using natural ordering.
    pub fn build(mana_dir: &Path) -> Result<Self> {
        let mut entries = Index::collect_archived(mana_dir)?;
        entries.sort_by(|a, b| natural_cmp(&a.id, &b.id));
        Ok(ArchiveIndex { units: entries })
    }

    /// Load the archive index from `.mana/archive.yaml`.
    pub fn load(mana_dir: &Path) -> Result<Self> {
        let path = mana_dir.join("archive.yaml");
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let index: ArchiveIndex =
            serde_yml::from_str(&contents).with_context(|| "Failed to parse archive.yaml")?;
        Ok(index)
    }

    /// Save the archive index to `.mana/archive.yaml`.
    pub fn save(&self, mana_dir: &Path) -> Result<()> {
        let path = mana_dir.join("archive.yaml");
        let yaml =
            serde_yml::to_string(self).with_context(|| "Failed to serialize archive index")?;
        atomic_write(&path, &yaml)
            .with_context(|| format!("Failed to write {}", path.display()))?;
        Ok(())
    }

    /// Load cached archive index or rebuild if stale.
    pub fn load_or_rebuild(mana_dir: &Path) -> Result<Self> {
        let archive_yaml = mana_dir.join("archive.yaml");
        if Self::is_stale(mana_dir)? {
            let index = Self::build(mana_dir)?;
            // Only save if there are entries or the file already exists
            // (avoids creating archive.yaml when there's no archive dir)
            if !index.units.is_empty() || archive_yaml.exists() {
                index.save(mana_dir)?;
            }
            Ok(index)
        } else if archive_yaml.exists() {
            Self::load(mana_dir)
        } else {
            // No archive dir and no archive.yaml — return empty
            Ok(ArchiveIndex { units: Vec::new() })
        }
    }

    /// Check whether the cached archive index is stale.
    /// Returns true if archive.yaml is missing (and archive dir exists),
    /// or if any file in the archive tree has been modified after archive.yaml.
    pub fn is_stale(mana_dir: &Path) -> Result<bool> {
        let archive_yaml = mana_dir.join("archive.yaml");
        let archive_dir = mana_dir.join("archive");

        if !archive_yaml.exists() {
            // If the archive dir doesn't exist either, nothing to index
            return Ok(archive_dir.is_dir());
        }

        if !archive_dir.is_dir() {
            return Ok(false);
        }

        let index_mtime = fs::metadata(&archive_yaml)
            .with_context(|| "Failed to read archive.yaml metadata")?
            .modified()
            .with_context(|| "Failed to get archive.yaml mtime")?;

        Self::any_file_newer(&archive_dir, index_mtime)
    }

    /// Check if any file in the given directory tree is newer than the reference time.
    fn any_file_newer(dir: &Path, reference: std::time::SystemTime) -> Result<bool> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                if Self::any_file_newer(&path, reference)? {
                    return Ok(true);
                }
            } else if path.is_file() {
                let mtime = fs::metadata(&path)?.modified()?;
                if mtime > reference {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Append an entry, deduplicating by ID (replaces any existing entry with the same ID).
    pub fn append(&mut self, entry: IndexEntry) {
        self.units.retain(|e| e.id != entry.id);
        self.units.push(entry);
        self.units.sort_by(|a, b| natural_cmp(&a.id, &b.id));
    }

    /// Remove an entry by ID.
    pub fn remove(&mut self, id: &str) {
        self.units.retain(|e| e.id != id);
    }
}

// ---------------------------------------------------------------------------
// LockedIndex — exclusive access during read-modify-write
// ---------------------------------------------------------------------------

/// Default timeout for acquiring the index lock.
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// Exclusive handle to the index, backed by an advisory flock on `.mana/index.lock`.
///
/// Prevents concurrent read-modify-write races when multiple agents run in parallel.
/// The lock is held from acquisition until `save_and_release` is called or the
/// `LockedIndex` is dropped.
///
/// ```no_run
/// # use anyhow::Result;
/// # use std::path::Path;
/// # fn example(mana_dir: &Path) -> Result<()> {
/// use mana_core::index::LockedIndex;
/// let mut locked = LockedIndex::acquire(mana_dir)?;
/// locked.index.units[0].title = "Updated".to_string();
/// locked.save_and_release()?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct LockedIndex {
    pub index: Index,
    lock_file: fs::File,
    mana_dir: std::path::PathBuf,
}

impl LockedIndex {
    /// Acquire an exclusive lock on the index, then load or rebuild it.
    /// Uses the default 5-second timeout.
    pub fn acquire(mana_dir: &Path) -> Result<Self> {
        Self::acquire_with_timeout(mana_dir, LOCK_TIMEOUT)
    }

    /// Acquire an exclusive lock with a custom timeout.
    pub fn acquire_with_timeout(mana_dir: &Path, timeout: Duration) -> Result<Self> {
        let lock_path = mana_dir.join("index.lock");
        let lock_file = fs::File::create(&lock_path)
            .with_context(|| format!("Failed to create lock file: {}", lock_path.display()))?;

        Self::flock_with_timeout(&lock_file, timeout)?;

        let index = Index::load_or_rebuild(mana_dir)?;

        Ok(Self {
            index,
            lock_file,
            mana_dir: mana_dir.to_path_buf(),
        })
    }

    /// Save the modified index and release the lock.
    pub fn save_and_release(self) -> Result<()> {
        self.index.save(&self.mana_dir)?;
        // self drops here, releasing the flock via Drop
        Ok(())
    }

    /// Poll for an exclusive flock with timeout.
    fn flock_with_timeout(file: &fs::File, timeout: Duration) -> Result<()> {
        let start = Instant::now();
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(()),
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if start.elapsed() >= timeout {
                        return Err(anyhow!(
                            "Timed out after {}s waiting for .mana/index.lock — \
                             another mana process may be running. \
                             If no other process is active, delete .mana/index.lock and retry.",
                            timeout.as_secs()
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    return Err(anyhow!("Failed to acquire index lock: {}", e));
                }
            }
        }
    }
}

impl Drop for LockedIndex {
    fn drop(&mut self) {
        // Use fs2's unlock explicitly (std's File::unlock stabilized in 1.89, above our MSRV)
        let _ = fs2::FileExt::unlock(&self.lock_file);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;
    use std::fs;
    use std::thread;
    use std::time::Duration;
    use tempfile::TempDir;

    /// Helper: create a .mana directory with some unit YAML files.
    fn setup_mana_dir() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create a few units
        let unit1 = Unit::new("1", "First task");
        let unit2 = Unit::new("2", "Second task");
        let unit10 = Unit::new("10", "Tenth task");
        let mut unit3_1 = Unit::new("3.1", "Subtask");
        unit3_1.parent = Some("3".to_string());
        unit3_1.labels = vec!["backend".to_string()];
        unit3_1.dependencies = vec!["1".to_string()];

        unit1.to_file(mana_dir.join("1.yaml")).unwrap();
        unit2.to_file(mana_dir.join("2.yaml")).unwrap();
        unit10.to_file(mana_dir.join("10.yaml")).unwrap();
        unit3_1.to_file(mana_dir.join("3.1.yaml")).unwrap();

        // Create files that should be excluded
        fs::write(mana_dir.join("config.yaml"), "project: test\nnext_id: 11\n").unwrap();

        (dir, mana_dir)
    }

    // -- natural_cmp tests --

    #[test]
    fn natural_sort_basic() {
        assert_eq!(natural_cmp("1", "2"), Ordering::Less);
        assert_eq!(natural_cmp("2", "1"), Ordering::Greater);
        assert_eq!(natural_cmp("1", "1"), Ordering::Equal);
    }

    #[test]
    fn natural_sort_numeric_not_lexicographic() {
        // Lexicographic: "10" < "2", but natural: "10" > "2"
        assert_eq!(natural_cmp("2", "10"), Ordering::Less);
        assert_eq!(natural_cmp("10", "2"), Ordering::Greater);
    }

    #[test]
    fn natural_sort_dotted_ids() {
        assert_eq!(natural_cmp("3", "3.1"), Ordering::Less);
        assert_eq!(natural_cmp("3.1", "3.2"), Ordering::Less);
        assert_eq!(natural_cmp("3.2", "10"), Ordering::Less);
    }

    #[test]
    fn natural_sort_full_sequence() {
        let mut ids = vec!["10", "3.2", "1", "3", "3.1", "2"];
        ids.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(ids, vec!["1", "2", "3", "3.1", "3.2", "10"]);
    }

    // -- build tests --

    #[test]
    fn build_reads_all_units_and_excludes_config() {
        let (_dir, mana_dir) = setup_mana_dir();
        let index = Index::build(&mana_dir).unwrap();

        // Should have 4 units: 1, 2, 3.1, 10
        assert_eq!(index.units.len(), 4);

        // Should be naturally sorted
        let ids: Vec<&str> = index.units.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["1", "2", "3.1", "10"]);
    }

    #[test]
    fn build_extracts_fields_correctly() {
        let (_dir, mana_dir) = setup_mana_dir();
        let index = Index::build(&mana_dir).unwrap();

        let entry = index.units.iter().find(|e| e.id == "3.1").unwrap();
        assert_eq!(entry.title, "Subtask");
        assert_eq!(entry.status, Status::Open);
        assert_eq!(entry.priority, 2);
        assert_eq!(entry.parent, Some("3".to_string()));
        assert_eq!(entry.dependencies, vec!["1".to_string()]);
        assert_eq!(entry.labels, vec!["backend".to_string()]);
    }

    #[test]
    fn index_entry_preserves_kind() {
        let mut unit = Unit::new("1", "Epic unit");
        unit.kind = crate::unit::UnitKind::Epic;

        let entry = IndexEntry::from(&unit);
        assert_eq!(entry.kind, crate::unit::UnitKind::Epic);
    }

    #[test]
    fn build_excludes_index_and_unit_yaml() {
        let (_dir, mana_dir) = setup_mana_dir();

        // Create index.yaml and unit.yaml — these should be excluded
        fs::write(mana_dir.join("index.yaml"), "units: []\n").unwrap();
        fs::write(
            mana_dir.join("unit.yaml"),
            "id: template\ntitle: Template\n",
        )
        .unwrap();

        let index = Index::build(&mana_dir).unwrap();
        assert_eq!(index.units.len(), 4);
        assert!(!index.units.iter().any(|e| e.id == "template"));
    }

    #[test]
    fn build_detects_duplicate_ids() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create two units with the same ID in different files
        let unit_a = Unit::new("99", "Unit A");
        let unit_b = Unit::new("99", "Unit B");

        unit_a.to_file(mana_dir.join("99-a.md")).unwrap();
        unit_b.to_file(mana_dir.join("99-b.md")).unwrap();

        let result = Index::build(&mana_dir);
        assert!(result.is_err());

        let err = result.unwrap_err().to_string();
        assert!(err.contains("Duplicate unit IDs detected"));
        assert!(err.contains("99"));
        assert!(err.contains("99-a.md"));
        assert!(err.contains("99-b.md"));
    }

    #[test]
    fn build_detects_multiple_duplicate_ids() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create duplicates for two different IDs
        Unit::new("1", "First A")
            .to_file(mana_dir.join("1-a.md"))
            .unwrap();
        Unit::new("1", "First B")
            .to_file(mana_dir.join("1-b.md"))
            .unwrap();
        Unit::new("2", "Second A")
            .to_file(mana_dir.join("2-a.md"))
            .unwrap();
        Unit::new("2", "Second B")
            .to_file(mana_dir.join("2-b.md"))
            .unwrap();

        let result = Index::build(&mana_dir);
        assert!(result.is_err());

        let err = result.unwrap_err().to_string();
        assert!(err.contains("ID '1'"));
        assert!(err.contains("ID '2'"));
    }

    // -- is_stale tests --

    #[test]
    fn is_stale_when_index_missing() {
        let (_dir, mana_dir) = setup_mana_dir();
        assert!(Index::is_stale(&mana_dir).unwrap());
    }

    #[test]
    fn is_stale_when_yaml_newer_than_index() {
        let (_dir, mana_dir) = setup_mana_dir();

        // Build and save the index first
        let index = Index::build(&mana_dir).unwrap();
        index.save(&mana_dir).unwrap();

        // Wait a moment to ensure distinct mtimes
        thread::sleep(Duration::from_millis(50));

        // Modify a unit file — this makes it newer than the index
        let unit = Unit::new("1", "Modified first task");
        unit.to_file(mana_dir.join("1.yaml")).unwrap();

        assert!(Index::is_stale(&mana_dir).unwrap());
    }

    #[test]
    fn not_stale_when_index_is_fresh() {
        let (_dir, mana_dir) = setup_mana_dir();

        // Build and save
        let index = Index::build(&mana_dir).unwrap();
        index.save(&mana_dir).unwrap();

        // The index was just written, so it should not be stale
        // (index.yaml mtime >= all other yaml mtimes)
        assert!(!Index::is_stale(&mana_dir).unwrap());
    }

    // -- load_or_rebuild tests --

    #[test]
    fn load_or_rebuild_builds_when_no_index() {
        let (_dir, mana_dir) = setup_mana_dir();

        let index = Index::load_or_rebuild(&mana_dir).unwrap();
        assert_eq!(index.units.len(), 4);

        // Should have created index.yaml
        assert!(mana_dir.join("index.yaml").exists());
    }

    #[test]
    fn load_or_rebuild_loads_when_fresh() {
        let (_dir, mana_dir) = setup_mana_dir();

        // Build + save
        let original = Index::build(&mana_dir).unwrap();
        original.save(&mana_dir).unwrap();

        // load_or_rebuild should load without rebuilding
        let loaded = Index::load_or_rebuild(&mana_dir).unwrap();
        assert_eq!(original, loaded);
    }

    // -- save / load round-trip --

    #[test]
    fn save_and_load_round_trip() {
        let (_dir, mana_dir) = setup_mana_dir();

        let index = Index::build(&mana_dir).unwrap();
        index.save(&mana_dir).unwrap();

        let loaded = Index::load(&mana_dir).unwrap();
        assert_eq!(index, loaded);
    }

    // -- empty directory --

    #[test]
    fn build_empty_directory() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        let index = Index::build(&mana_dir).unwrap();
        assert!(index.units.is_empty());
    }

    // -- LockedIndex tests --

    #[test]
    fn locked_index_acquire_and_save() {
        let (_dir, mana_dir) = setup_mana_dir();

        let mut locked = LockedIndex::acquire(&mana_dir).unwrap();
        assert_eq!(locked.index.units.len(), 4);

        // Modify a title
        locked.index.units[0].title = "Modified".to_string();
        locked.save_and_release().unwrap();

        // Verify the change persisted
        let index = Index::load(&mana_dir).unwrap();
        assert_eq!(index.units[0].title, "Modified");
    }

    #[test]
    fn locked_index_blocks_concurrent_access() {
        let (_dir, mana_dir) = setup_mana_dir();

        // First lock
        let _locked = LockedIndex::acquire(&mana_dir).unwrap();

        // Second lock should fail with timeout
        let result = LockedIndex::acquire_with_timeout(&mana_dir, Duration::from_millis(200));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Timed out"),
            "Expected timeout error, got: {}",
            err
        );
    }

    #[test]
    fn locked_index_released_on_drop() {
        let (_dir, mana_dir) = setup_mana_dir();

        {
            let _locked = LockedIndex::acquire(&mana_dir).unwrap();
            // lock held in this scope
        }
        // lock released on drop

        // Should be able to acquire again
        let _locked = LockedIndex::acquire(&mana_dir).unwrap();
    }

    #[test]
    fn locked_index_creates_lock_file() {
        let (_dir, mana_dir) = setup_mana_dir();

        let _locked = LockedIndex::acquire(&mana_dir).unwrap();
        assert!(mana_dir.join("index.lock").exists());
    }

    // -- is_stale ignores non-yaml files --

    #[test]
    fn is_stale_ignores_non_yaml() {
        let (_dir, mana_dir) = setup_mana_dir();

        let index = Index::build(&mana_dir).unwrap();
        index.save(&mana_dir).unwrap();

        // Create a non-yaml file after the index
        thread::sleep(Duration::from_millis(50));
        fs::write(mana_dir.join("notes.txt"), "some notes").unwrap();

        // Should NOT be stale — non-yaml files don't count
        assert!(!Index::is_stale(&mana_dir).unwrap());
    }
}

#[cfg(test)]
mod archive_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn collect_archived_finds_units() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create archive structure
        let archive_dir = mana_dir.join("archive").join("2026").join("02");
        fs::create_dir_all(&archive_dir).unwrap();

        // Create an archived unit
        let mut unit = crate::unit::Unit::new("1", "Archived task");
        unit.status = crate::unit::Status::Closed;
        unit.to_file(archive_dir.join("1-archived-task.md"))
            .unwrap();

        let archived = Index::collect_archived(&mana_dir).unwrap();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].id, "1");
        assert_eq!(archived[0].status, crate::unit::Status::Closed);
    }

    #[test]
    fn collect_archived_empty_when_no_archive() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        let archived = Index::collect_archived(&mana_dir).unwrap();
        assert!(archived.is_empty());
    }
}

#[cfg(test)]
mod format_count_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn count_unit_formats_only_yaml() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create only yaml files
        let unit1 = crate::unit::Unit::new("1", "Task 1");
        let unit2 = crate::unit::Unit::new("2", "Task 2");
        unit1.to_file(mana_dir.join("1.yaml")).unwrap();
        unit2.to_file(mana_dir.join("2.yaml")).unwrap();

        let (md_count, yaml_count) = count_unit_formats(&mana_dir).unwrap();
        assert_eq!(md_count, 0);
        assert_eq!(yaml_count, 2);
    }

    #[test]
    fn count_unit_formats_only_md() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create only md files
        let unit1 = crate::unit::Unit::new("1", "Task 1");
        let unit2 = crate::unit::Unit::new("2", "Task 2");
        unit1.to_file(mana_dir.join("1-task-1.md")).unwrap();
        unit2.to_file(mana_dir.join("2-task-2.md")).unwrap();

        let (md_count, yaml_count) = count_unit_formats(&mana_dir).unwrap();
        assert_eq!(md_count, 2);
        assert_eq!(yaml_count, 0);
    }

    #[test]
    fn count_unit_formats_mixed() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create mixed formats
        let unit1 = crate::unit::Unit::new("1", "Task 1");
        let unit2 = crate::unit::Unit::new("2", "Task 2");
        let unit3 = crate::unit::Unit::new("3", "Task 3");
        unit1.to_file(mana_dir.join("1.yaml")).unwrap();
        unit2.to_file(mana_dir.join("2-task-2.md")).unwrap();
        unit3.to_file(mana_dir.join("3-task-3.md")).unwrap();

        let (md_count, yaml_count) = count_unit_formats(&mana_dir).unwrap();
        assert_eq!(md_count, 2);
        assert_eq!(yaml_count, 1);
    }

    #[test]
    fn count_unit_formats_excludes_config_files() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create excluded yaml files (config.yaml, index.yaml)
        fs::write(mana_dir.join("config.yaml"), "project: test").unwrap();
        fs::write(mana_dir.join("index.yaml"), "units: []").unwrap();

        // Create one actual unit
        let unit1 = crate::unit::Unit::new("1", "Task 1");
        unit1.to_file(mana_dir.join("1-task-1.md")).unwrap();

        let (md_count, yaml_count) = count_unit_formats(&mana_dir).unwrap();
        assert_eq!(md_count, 1);
        assert_eq!(yaml_count, 0); // config.yaml and index.yaml are excluded
    }

    #[test]
    fn count_unit_formats_empty_dir() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        let (md_count, yaml_count) = count_unit_formats(&mana_dir).unwrap();
        assert_eq!(md_count, 0);
        assert_eq!(yaml_count, 0);
    }
}

#[cfg(test)]
mod archive_index_tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_mana_dir_with_archive() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        let archive_dir = mana_dir.join("archive").join("2026").join("03");
        fs::create_dir_all(&archive_dir).unwrap();

        let mut unit1 = crate::unit::Unit::new("5", "Archived task five");
        unit1.status = crate::unit::Status::Closed;
        unit1.is_archived = true;
        unit1
            .to_file(archive_dir.join("5-archived-task-five.md"))
            .unwrap();

        let mut unit2 = crate::unit::Unit::new("3", "Archived task three");
        unit2.status = crate::unit::Status::Closed;
        unit2.is_archived = true;
        unit2
            .to_file(archive_dir.join("3-archived-task-three.md"))
            .unwrap();

        (dir, mana_dir)
    }

    #[test]
    fn archive_index_build_from_archive_dir() {
        let (_dir, mana_dir) = setup_mana_dir_with_archive();
        let archive = ArchiveIndex::build(&mana_dir).unwrap();

        assert_eq!(archive.units.len(), 2);
        // Should be sorted by natural ordering: "3" before "5"
        assert_eq!(archive.units[0].id, "3");
        assert_eq!(archive.units[1].id, "5");
        assert_eq!(archive.units[0].status, crate::unit::Status::Closed);
        assert_eq!(archive.units[1].status, crate::unit::Status::Closed);
    }

    #[test]
    fn archive_index_build_empty_when_no_archive_dir() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        let archive = ArchiveIndex::build(&mana_dir).unwrap();
        assert!(archive.units.is_empty());
    }

    #[test]
    fn archive_index_save_load_roundtrip() {
        let (_dir, mana_dir) = setup_mana_dir_with_archive();
        let original = ArchiveIndex::build(&mana_dir).unwrap();
        original.save(&mana_dir).unwrap();

        let loaded = ArchiveIndex::load(&mana_dir).unwrap();
        assert_eq!(original, loaded);
    }

    #[test]
    fn archive_index_append_deduplicates() {
        let (_dir, mana_dir) = setup_mana_dir_with_archive();
        let mut archive = ArchiveIndex::build(&mana_dir).unwrap();
        assert_eq!(archive.units.len(), 2);

        // Append a new entry
        let mut new_unit = crate::unit::Unit::new("7", "New archived");
        new_unit.status = crate::unit::Status::Closed;
        archive.append(IndexEntry::from(&new_unit));
        assert_eq!(archive.units.len(), 3);

        // Append again with same ID — should replace, not duplicate
        let mut updated_unit = crate::unit::Unit::new("7", "Updated title");
        updated_unit.status = crate::unit::Status::Closed;
        archive.append(IndexEntry::from(&updated_unit));
        assert_eq!(archive.units.len(), 3);

        let entry = archive.units.iter().find(|e| e.id == "7").unwrap();
        assert_eq!(entry.title, "Updated title");
    }

    #[test]
    fn archive_index_remove() {
        let (_dir, mana_dir) = setup_mana_dir_with_archive();
        let mut archive = ArchiveIndex::build(&mana_dir).unwrap();
        assert_eq!(archive.units.len(), 2);

        archive.remove("3");
        assert_eq!(archive.units.len(), 1);
        assert_eq!(archive.units[0].id, "5");

        // Removing non-existent ID is a no-op
        archive.remove("999");
        assert_eq!(archive.units.len(), 1);
    }

    #[test]
    fn archive_index_is_stale_when_no_archive_yaml() {
        let (_dir, mana_dir) = setup_mana_dir_with_archive();
        // Archive dir exists but archive.yaml doesn't
        assert!(ArchiveIndex::is_stale(&mana_dir).unwrap());
    }

    #[test]
    fn archive_index_not_stale_when_no_archive_dir() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();
        // Neither archive dir nor archive.yaml exist
        assert!(!ArchiveIndex::is_stale(&mana_dir).unwrap());
    }

    #[test]
    fn archive_index_not_stale_after_build_and_save() {
        let (_dir, mana_dir) = setup_mana_dir_with_archive();
        let archive = ArchiveIndex::build(&mana_dir).unwrap();
        archive.save(&mana_dir).unwrap();
        assert!(!ArchiveIndex::is_stale(&mana_dir).unwrap());
    }

    #[test]
    fn archive_index_stale_when_file_newer() {
        let (_dir, mana_dir) = setup_mana_dir_with_archive();
        let archive = ArchiveIndex::build(&mana_dir).unwrap();
        archive.save(&mana_dir).unwrap();

        // Wait and add a new file to the archive
        std::thread::sleep(std::time::Duration::from_millis(50));
        let archive_dir = mana_dir.join("archive").join("2026").join("03");
        let mut new_unit = crate::unit::Unit::new("9", "Newer");
        new_unit.status = crate::unit::Status::Closed;
        new_unit.is_archived = true;
        new_unit.to_file(archive_dir.join("9-newer.md")).unwrap();

        assert!(ArchiveIndex::is_stale(&mana_dir).unwrap());
    }

    #[test]
    fn archive_index_load_or_rebuild_builds_when_stale() {
        let (_dir, mana_dir) = setup_mana_dir_with_archive();
        let archive = ArchiveIndex::load_or_rebuild(&mana_dir).unwrap();
        assert_eq!(archive.units.len(), 2);
        // Should have created archive.yaml
        assert!(mana_dir.join("archive.yaml").exists());
    }

    #[test]
    fn archive_index_load_or_rebuild_returns_empty_when_no_archive() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        let archive = ArchiveIndex::load_or_rebuild(&mana_dir).unwrap();
        assert!(archive.units.is_empty());
        // Should NOT create archive.yaml when there's nothing to index
        assert!(!mana_dir.join("archive.yaml").exists());
    }
}
