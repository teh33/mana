use std::fmt;

use crate::index::{ArchiveIndex, Index, IndexEntry};
use crate::unit::Status;

// ---------------------------------------------------------------------------
// Scope thresholds
// ---------------------------------------------------------------------------

/// Maximum number of `produces` artifacts before a unit is considered oversized.
pub const MAX_PRODUCES: usize = 3;

/// Maximum number of `paths` before a unit is considered oversized.
pub const MAX_PATHS: usize = 5;

// ---------------------------------------------------------------------------
// BlockReason
// ---------------------------------------------------------------------------

/// Why a unit cannot be dispatched right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockReason {
    /// One or more dependency units are not yet closed.
    WaitingOn(Vec<String>),
}

/// Soft scope warnings — displayed but don't block dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeWarning {
    /// Scope is large: `produces > MAX_PRODUCES` or `paths > MAX_PATHS`.
    Oversized,
}

impl fmt::Display for BlockReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlockReason::WaitingOn(ids) => {
                write!(f, "waiting on {}", ids.join(", "))
            }
        }
    }
}

impl fmt::Display for ScopeWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScopeWarning::Oversized => write!(f, "oversized"),
        }
    }
}

// ---------------------------------------------------------------------------
// Unified blocking check
// ---------------------------------------------------------------------------

/// Check whether `entry` is blocked, returning the reason if so.
///
/// Checks in priority order:
/// 1. **Explicit dependencies** — any dep that isn't closed (or doesn't exist).
/// 2. **Requires/produces** — sibling units that produce a required artifact
///    but aren't closed yet.
///
/// Note: This overload does not check archived units. If a dependency was closed
/// and archived, it will appear unsatisfied. Use [`check_blocked_with_archive`]
/// when archive awareness is needed (e.g., `mana run`).
pub fn check_blocked(entry: &IndexEntry, index: &Index) -> Option<BlockReason> {
    check_blocked_with_archive(entry, index, None)
}

/// Like [`check_blocked`], but also checks the archive index.
/// Archived units are treated as closed (satisfied).
pub fn check_blocked_with_archive(
    entry: &IndexEntry,
    index: &Index,
    archive: Option<&ArchiveIndex>,
) -> Option<BlockReason> {
    let mut waiting_on = Vec::new();

    // Explicit dependencies
    for dep_id in &entry.dependencies {
        match index.units.iter().find(|e| e.id == *dep_id) {
            Some(dep) if dep.status == Status::Closed => {}
            Some(_) => waiting_on.push(dep_id.clone()), // active but not closed
            None => {
                // Not in active index — check archive (archived = closed)
                let in_archive = archive
                    .map(|a| a.units.iter().any(|e| e.id == *dep_id))
                    .unwrap_or(false);
                if !in_archive {
                    waiting_on.push(dep_id.clone());
                }
            }
        }
    }

    // Smart dependencies: requires → sibling produces
    for required in &entry.requires {
        if let Some(producer) = index
            .units
            .iter()
            .find(|e| e.id != entry.id && e.parent == entry.parent && e.produces.contains(required))
        {
            if producer.status != Status::Closed && !waiting_on.contains(&producer.id) {
                waiting_on.push(producer.id.clone());
            }
        }
        // If no active producer found, check archive — archived producers are satisfied
    }

    if !waiting_on.is_empty() {
        return Some(BlockReason::WaitingOn(waiting_on));
    }

    None
}

/// Check for scope warnings (non-blocking).
///
/// Returns a warning if scope is large (`produces > MAX_PRODUCES` or `paths > MAX_PATHS`).
/// Units with no scope (no produces, no paths) are fine — not every unit needs explicit paths.
pub fn check_scope_warning(entry: &IndexEntry) -> Option<ScopeWarning> {
    if entry.produces.len() > MAX_PRODUCES || entry.paths.len() > MAX_PATHS {
        return Some(ScopeWarning::Oversized);
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_entry(id: &str) -> IndexEntry {
        IndexEntry {
            handle: None,
            id: id.to_string(),
            title: format!("Unit {}", id),
            status: Status::Open,
            priority: 2,
            parent: None,
            dependencies: vec![],
            labels: vec![],
            assignee: None,
            updated_at: Utc::now(),
            produces: vec![],
            requires: vec![],
            has_verify: true,
            verify: None,
            created_at: Utc::now(),
            claimed_by: None,
            attempts: 0,
            paths: vec![],
            kind: crate::unit::UnitType::Task,
            feature: false,
            has_decisions: false,
        }
    }

    fn make_index(entries: Vec<IndexEntry>) -> Index {
        Index { units: entries }
    }

    // -- WaitingOn: explicit deps --

    #[test]
    fn blocking_not_blocked_when_deps_closed() {
        let mut dep = make_entry("1");
        dep.status = Status::Closed;

        let mut entry = make_entry("2");
        entry.dependencies = vec!["1".into()];
        entry.produces = vec!["Foo".into()];
        entry.paths = vec!["src/foo.rs".into()];

        let index = make_index(vec![dep, entry.clone()]);
        assert_eq!(check_blocked(&entry, &index), None);
    }

    #[test]
    fn blocking_waiting_on_open_dep() {
        let dep = make_entry("1"); // open

        let mut entry = make_entry("2");
        entry.dependencies = vec!["1".into()];
        entry.produces = vec!["Foo".into()];
        entry.paths = vec!["src/foo.rs".into()];

        let index = make_index(vec![dep, entry.clone()]);
        assert_eq!(
            check_blocked(&entry, &index),
            Some(BlockReason::WaitingOn(vec!["1".into()]))
        );
    }

    #[test]
    fn blocking_waiting_on_missing_dep() {
        let mut entry = make_entry("2");
        entry.dependencies = vec!["999".into()]; // doesn't exist
        entry.produces = vec!["Foo".into()];
        entry.paths = vec!["src/foo.rs".into()];

        let index = make_index(vec![entry.clone()]);
        assert_eq!(
            check_blocked(&entry, &index),
            Some(BlockReason::WaitingOn(vec!["999".into()]))
        );
    }

    #[test]
    fn blocking_waiting_on_multiple_deps() {
        let dep_a = make_entry("1"); // open
        let dep_b = make_entry("3"); // open

        let mut entry = make_entry("2");
        entry.dependencies = vec!["1".into(), "3".into()];
        entry.produces = vec!["Foo".into()];
        entry.paths = vec!["src/foo.rs".into()];

        let index = make_index(vec![dep_a, entry.clone(), dep_b]);
        assert_eq!(
            check_blocked(&entry, &index),
            Some(BlockReason::WaitingOn(vec!["1".into(), "3".into()]))
        );
    }

    // -- WaitingOn: requires/produces --

    #[test]
    fn blocking_waiting_on_sibling_producer() {
        let mut producer = make_entry("5.1");
        producer.parent = Some("5".into());
        producer.produces = vec!["UserType".into()];

        let mut consumer = make_entry("5.2");
        consumer.parent = Some("5".into());
        consumer.requires = vec!["UserType".into()];
        consumer.produces = vec!["UserAPI".into()];
        consumer.paths = vec!["src/api.rs".into()];

        let index = make_index(vec![producer, consumer.clone()]);
        assert_eq!(
            check_blocked(&consumer, &index),
            Some(BlockReason::WaitingOn(vec!["5.1".into()]))
        );
    }

    #[test]
    fn blocking_not_blocked_when_producer_closed() {
        let mut producer = make_entry("5.1");
        producer.parent = Some("5".into());
        producer.produces = vec!["UserType".into()];
        producer.status = Status::Closed;

        let mut consumer = make_entry("5.2");
        consumer.parent = Some("5".into());
        consumer.requires = vec!["UserType".into()];
        consumer.produces = vec!["UserAPI".into()];
        consumer.paths = vec!["src/api.rs".into()];

        let index = make_index(vec![producer, consumer.clone()]);
        assert_eq!(check_blocked(&consumer, &index), None);
    }

    #[test]
    fn blocking_no_duplicate_when_dep_and_requires_overlap() {
        let mut producer = make_entry("5.1");
        producer.parent = Some("5".into());
        producer.produces = vec!["UserType".into()];

        let mut consumer = make_entry("5.2");
        consumer.parent = Some("5".into());
        consumer.dependencies = vec!["5.1".into()]; // explicit dep
        consumer.requires = vec!["UserType".into()]; // also requires from same unit
        consumer.produces = vec!["UserAPI".into()];
        consumer.paths = vec!["src/api.rs".into()];

        let index = make_index(vec![producer, consumer.clone()]);
        if let Some(BlockReason::WaitingOn(ids)) = check_blocked(&consumer, &index) {
            // 5.1 should appear only once even though it's both an explicit dep and a producer
            assert_eq!(ids, vec!["5.1".to_string()]);
        } else {
            panic!("Expected WaitingOn");
        }
    }

    // -- Scope warnings (non-blocking) --

    #[test]
    fn warning_oversized_too_many_produces() {
        let mut entry = make_entry("1");
        entry.produces = vec!["A".into(), "B".into(), "C".into(), "D".into()]; // 4 > MAX_PRODUCES
        entry.paths = vec!["src/a.rs".into()];

        // Not blocked — just a warning
        let index = make_index(vec![entry.clone()]);
        assert_eq!(check_blocked(&entry, &index), None);
        assert_eq!(check_scope_warning(&entry), Some(ScopeWarning::Oversized));
    }

    #[test]
    fn warning_oversized_too_many_paths() {
        let mut entry = make_entry("1");
        entry.produces = vec!["A".into()];
        entry.paths = vec![
            "a.rs".into(),
            "b.rs".into(),
            "c.rs".into(),
            "d.rs".into(),
            "e.rs".into(),
            "f.rs".into(),
        ]; // 6 > MAX_PATHS

        let index = make_index(vec![entry.clone()]);
        assert_eq!(check_blocked(&entry, &index), None);
        assert_eq!(check_scope_warning(&entry), Some(ScopeWarning::Oversized));
    }

    #[test]
    fn warning_not_oversized_at_threshold() {
        let mut entry = make_entry("1");
        entry.produces = vec!["A".into(), "B".into(), "C".into()]; // exactly MAX_PRODUCES
        entry.paths = vec![
            "a.rs".into(),
            "b.rs".into(),
            "c.rs".into(),
            "d.rs".into(),
            "e.rs".into(),
        ]; // exactly MAX_PATHS

        assert_eq!(check_scope_warning(&entry), None);
    }

    // -- Unscoped is NOT blocking --

    #[test]
    fn unscoped_unit_is_not_blocked() {
        let entry = make_entry("1"); // produces=[], paths=[]

        let index = make_index(vec![entry.clone()]);
        assert_eq!(check_blocked(&entry, &index), None);
    }

    #[test]
    fn not_blocked_with_produces_only() {
        let mut entry = make_entry("1");
        entry.produces = vec!["SomeType".into()];

        let index = make_index(vec![entry.clone()]);
        assert_eq!(check_blocked(&entry, &index), None);
    }

    #[test]
    fn not_blocked_with_paths_only() {
        let mut entry = make_entry("1");
        entry.paths = vec!["src/main.rs".into()];

        let index = make_index(vec![entry.clone()]);
        assert_eq!(check_blocked(&entry, &index), None);
    }

    // -- Display --

    #[test]
    fn blocking_display_waiting_on() {
        let reason = BlockReason::WaitingOn(vec!["3.1".into(), "3.2".into()]);
        assert_eq!(format!("{}", reason), "waiting on 3.1, 3.2");
    }

    #[test]
    fn warning_display_oversized() {
        assert_eq!(format!("{}", ScopeWarning::Oversized), "oversized");
    }

    // -- Priority: deps still checked --

    #[test]
    fn blocking_deps_still_block_oversized_units() {
        let dep = make_entry("1"); // open

        let mut entry = make_entry("2");
        entry.dependencies = vec!["1".into()];
        entry.produces = vec!["A".into(), "B".into(), "C".into(), "D".into()]; // oversized
        entry.paths = vec!["a.rs".into()];

        let index = make_index(vec![dep, entry.clone()]);
        assert!(matches!(
            check_blocked(&entry, &index),
            Some(BlockReason::WaitingOn(_))
        ));
    }

    #[test]
    fn blocking_deps_still_block_unscoped_units() {
        let dep = make_entry("1"); // open

        let mut entry = make_entry("2");
        entry.dependencies = vec!["1".into()];
        // produces=[], paths=[] → unscoped but deps block first

        let index = make_index(vec![dep, entry.clone()]);
        assert!(matches!(
            check_blocked(&entry, &index),
            Some(BlockReason::WaitingOn(_))
        ));
    }
}
