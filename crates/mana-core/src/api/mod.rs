//! # mana-core Public API
//!
//! Programmatic access to all mana unit operations. Use this module when embedding
//! mana in another application — a GUI, MCP server, orchestration daemon, or custom
//! tooling.
//!
//! The API is organized into layers:
//!
//! - **Types** — Core data structures re-exported from internal modules
//! - **Discovery** — Find `.mana/` directories and unit files
//! - **Query** — Read-only operations (list, get, tree, status, graph)
//! - **Mutations** — Write operations (create, update, close, delete)
//! - **Orchestration** — Agent dispatch, context assembly, and verification
//! - **Facts** — Verified project knowledge with TTL
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use mana_core::api::*;
//! use std::path::Path;
//!
//! // Find the .mana/ directory
//! let mana_dir = find_mana_dir(Path::new(".")).unwrap();
//!
//! // Load the index (cached, rebuilds if stale)
//! let index = load_index(&mana_dir).unwrap();
//!
//! // Get a specific unit
//! let unit = get_unit(&mana_dir, "1").unwrap();
//! println!("{}: {}", unit.id, unit.title);
//! ```
//!
//! ## Design Principles
//!
//! - **No I/O side effects** — Library functions never print to stdout/stderr.
//!   All output is returned as structured data.
//! - **Structured params and results** — Each mutation takes a `Params` struct
//!   and returns a typed result. No raw argument passing.
//! - **`&Path` as entry point** — Every function takes `mana_dir: &Path`.
//!   No global state, no singletons, no `Arc` required.
//! - **Serializable** — All types derive `Serialize`/`Deserialize` for easy
//!   IPC (Tauri, JSON-RPC, MCP).
//! - **Thread-safe** — No interior mutability, no shared global state.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;

use crate::error::{ManaError, ManaResult};

// ---------------------------------------------------------------------------
// Re-exported core types
// ---------------------------------------------------------------------------

/// Core unit type representing a single work item.
pub use crate::unit::{
    AttemptOutcome, AttemptRecord, AutonomyObservation, AutonomyProvenance, OnCloseAction,
    OnFailAction, RunRecord, RunResult, Status, Unit, UnitType, VisibilityState,
};

/// Index types for working with the unit cache.
pub use crate::index::{Index, IndexEntry};

/// Project configuration.
pub use crate::config::Config;

/// Typed error and result types.
pub use crate::error::{self, ManaError as Error};

// ---------------------------------------------------------------------------
// Discovery re-exports
// ---------------------------------------------------------------------------

/// Find the `.mana/` directory by walking up from `path`.
///
/// Searches the given path and all parent directories until a `.mana/`
/// directory is found.
///
/// # Errors
/// - Returns an error if no `.mana/` directory is found in the hierarchy
/// - [`ManaError::IoError`] — filesystem failure
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::find_mana_dir;
/// use std::path::Path;
///
/// let mana_dir = find_mana_dir(Path::new("/some/project/subdir")).unwrap();
/// ```
pub use crate::discovery::find_mana_dir;

/// Find the file path for a unit by ID.
///
/// Searches the `.mana/` directory for an active (non-archived) unit with
/// the given ID.
///
/// # Errors
/// - [`ManaError::UnitNotFound`] — no unit file for the given ID
/// - [`ManaError::InvalidId`] — ID is empty or contains invalid characters
/// - [`ManaError::IoError`] — filesystem failure
pub use crate::discovery::find_unit_file;

/// Find the file path for an archived unit by ID.
///
/// Searches the `.mana/archive/` tree for a unit that was previously closed.
///
/// # Errors
/// - [`ManaError::UnitNotFound`] — unit ID not found in archive
/// - [`ManaError::InvalidId`] — ID is empty or contains invalid characters
pub use crate::discovery::find_archived_unit;

// ---------------------------------------------------------------------------
// Graph types (new: not in ops modules)
// ---------------------------------------------------------------------------

/// A node in the unit hierarchy tree, used by [`get_tree`].
#[derive(Debug, Clone)]
pub struct SiblingComparison {
    /// Unit ID.
    pub id: String,
    /// Unit title.
    pub title: String,
    /// Unit status.
    pub status: Status,
    /// Number of recorded attempts.
    pub attempts: usize,
    /// Recent lifecycle outcome, when known.
    pub recent_outcome: Option<String>,
}

/// A node in the unit hierarchy tree, used by [`get_tree`].
#[derive(Debug, Clone)]
pub struct TreeNode {
    /// Unit ID.
    pub id: String,
    /// Unit title.
    pub title: String,
    /// Unit status.
    pub status: Status,
    /// Priority (0 = P0/highest, 4 = P4/lowest).
    pub priority: u8,
    /// Whether the unit has a verify command.
    pub has_verify: bool,
    /// Explicit unit type.
    pub kind: UnitType,
    /// Child nodes (units whose `parent` field is this unit's ID).
    pub children: Vec<TreeNode>,
}

/// A full dependency graph representation.
///
/// The graph is a directed acyclic graph where each edge `a -> b`
/// means "unit `a` depends on unit `b`".
#[derive(Debug, Clone)]
pub struct DependencyGraph {
    /// All nodes in the graph, keyed by unit ID.
    pub nodes: HashMap<String, GraphNode>,
    /// Adjacency list: unit ID → list of dependency IDs.
    pub edges: HashMap<String, Vec<String>>,
}

/// A node in the dependency graph.
#[derive(Debug, Clone)]
pub struct GraphNode {
    /// Unit ID.
    pub id: String,
    /// Unit title.
    pub title: String,
    /// Unit status.
    pub status: Status,
}

// Re-export orchestration and ops types
pub use crate::ops::{
    claim, close, context, create, delete, dep, fact, fact_sheet, list, plan, show, update, verify,
};

pub use crate::ops::context::summarize_child_units as compare_sibling_jobs;
pub use crate::ops::context::AgentContext;
pub use crate::ops::fact::{FactParams, FactResult, VerifyFactsResult};
pub use crate::ops::fact_sheet::{
    FactSheetCheckEntry, FactSheetCheckResult, FactSheetDiagnostic, FactSheetDiagnosticSeverity,
    FactSheetFact, FactSheetParseResult, FactSheetStatus,
};
pub use crate::ops::memory_context::{
    memory_context, MemoryContext, RecentWork, RelevantFact, WorkingUnit,
};
pub use crate::ops::run::{
    BlockedUnit, ReadyQueue, ReadyUnit, RunPlan, RunRetryContext, RunScopeWarning, RunTarget,
    RunWave,
};
pub use crate::ops::stats::StatsResult;
pub use crate::ops::status::StatusSummary;
pub use crate::ops::verify::VerifyResult;

// ---------------------------------------------------------------------------
// Query functions
// ---------------------------------------------------------------------------

/// Load a unit by ID.
///
/// Finds the unit file in the `.mana/` directory and deserializes it.
/// Works for active (non-archived) units only. For archived units, use
/// [`get_archived_unit`].
///
/// # Errors
/// - [`ManaError::UnitNotFound`] — no unit file for the given ID
/// - [`ManaError::InvalidId`] — ID is empty or contains invalid characters
/// - [`ManaError::ParseError`] — file cannot be deserialized
/// - [`ManaError::IoError`] — filesystem failure
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::get_unit;
/// use std::path::Path;
///
/// let mana_dir = Path::new("/project/.mana");
/// let unit = get_unit(mana_dir, "42").unwrap();
/// println!("{}: {}", unit.id, unit.title);
/// ```
pub fn get_unit(mana_dir: &Path, id: &str) -> ManaResult<Unit> {
    let path = find_unit_file(mana_dir, id).map_err(|e| {
        let msg = e.to_string();
        if msg.contains("Invalid unit ID") || msg.contains("cannot be empty") {
            ManaError::InvalidId {
                id: id.to_string(),
                reason: msg,
            }
        } else {
            ManaError::UnitNotFound { id: id.to_string() }
        }
    })?;
    Unit::from_file(&path).map_err(|e| ManaError::ParseError {
        path,
        reason: e.to_string(),
    })
}

/// Load a unit from the archive by ID.
///
/// Searches the `.mana/archive/` tree for a unit that was previously closed
/// and archived.
///
/// # Errors
/// - [`ManaError::UnitNotFound`] — unit ID not found in archive
/// - [`ManaError::InvalidId`] — ID is empty or contains invalid characters
/// - [`ManaError::ParseError`] — file cannot be deserialized
/// - [`ManaError::IoError`] — filesystem failure
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::get_archived_unit;
/// use std::path::Path;
///
/// let mana_dir = Path::new("/project/.mana");
/// let unit = get_archived_unit(mana_dir, "42").unwrap();
/// println!("Closed at: {:?}", unit.closed_at);
/// ```
pub fn get_archived_unit(mana_dir: &Path, id: &str) -> ManaResult<Unit> {
    let path = find_archived_unit(mana_dir, id).map_err(|e| {
        let msg = e.to_string();
        if msg.contains("Invalid unit ID") || msg.contains("cannot be empty") {
            ManaError::InvalidId {
                id: id.to_string(),
                reason: msg,
            }
        } else {
            ManaError::UnitNotFound { id: id.to_string() }
        }
    })?;
    Unit::from_file(&path).map_err(|e| ManaError::ParseError {
        path,
        reason: e.to_string(),
    })
}

/// Load the index, rebuilding from unit files if stale.
///
/// The index is a YAML cache that's faster than reading every unit file.
/// It is automatically rebuilt when unit files are newer than the cached index.
///
/// # Errors
/// - [`ManaError::IndexError`] — index cannot be built, loaded, or saved
/// - [`ManaError::IoError`] — filesystem failure
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::load_index;
/// use std::path::Path;
///
/// let index = load_index(Path::new("/project/.mana")).unwrap();
/// println!("{} units", index.units.len());
/// ```
pub fn load_index(mana_dir: &Path) -> ManaResult<Index> {
    Index::load_or_rebuild(mana_dir).map_err(|e| ManaError::IndexError(e.to_string()))
}

/// List units with optional filters.
///
/// Returns index entries (lightweight unit summaries) for all units matching
/// the given filters. By default, closed units are excluded.
///
/// # Errors
/// - [`ManaError::IndexError`] — index cannot be loaded
/// - [`ManaError::IoError`] — filesystem failure
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::list_units;
/// use mana_core::ops::list::ListParams;
/// use std::path::Path;
///
/// let mana_dir = Path::new("/project/.mana");
///
/// // List all open units
/// let units = list_units(mana_dir, &ListParams::default()).unwrap();
///
/// // List units assigned to alice
/// let alice_units = list_units(mana_dir, &ListParams {
///     assignee: Some("alice".to_string()),
///     ..Default::default()
/// }).unwrap();
/// ```
pub fn list_units(mana_dir: &Path, params: &list::ListParams) -> Result<Vec<IndexEntry>> {
    crate::ops::list::list(mana_dir, params)
}

/// Build a unit hierarchy tree rooted at the given unit ID.
///
/// Returns a [`TreeNode`] with all descendants nested recursively. Only units
/// in the active index are included (archived units are excluded).
///
/// # Errors
/// - [`ManaError::UnitNotFound`] — no unit with the given ID in the active index
/// - [`ManaError::IndexError`] — index cannot be loaded
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::get_tree;
/// use std::path::Path;
///
/// let tree = get_tree(Path::new("/project/.mana"), "1").unwrap();
/// println!("{}: {} children", tree.id, tree.children.len());
/// ```
pub fn get_tree(mana_dir: &Path, root_id: &str) -> Result<TreeNode> {
    let index = Index::load_or_rebuild(mana_dir)?;
    build_tree_node(root_id, &index)
}

fn build_tree_node(id: &str, index: &Index) -> Result<TreeNode> {
    let entry = index
        .units
        .iter()
        .find(|e| e.id == id)
        .ok_or_else(|| anyhow::anyhow!("Unit {} not found", id))?;

    let children: Vec<TreeNode> = index
        .units
        .iter()
        .filter(|e| e.parent.as_deref() == Some(id))
        .map(|child| build_tree_node(&child.id, index))
        .collect::<Result<Vec<_>>>()?;

    Ok(TreeNode {
        id: entry.id.clone(),
        title: entry.title.clone(),
        status: entry.status,
        priority: entry.priority,
        has_verify: entry.has_verify,
        kind: entry.kind,
        children,
    })
}

fn has_open_children(entry: &IndexEntry, index: &Index) -> bool {
    index
        .units
        .iter()
        .any(|e| e.parent.as_deref() == Some(entry.id.as_str()) && e.status != Status::Closed)
}

/// Get a categorized project status summary.
///
/// Returns units grouped into: epics, features, in-progress (claimed), ready to run,
/// goals (no verify command), and blocked (dependencies not met).
///
/// # Errors
/// - [`ManaError::IndexError`] — index cannot be loaded
/// - [`ManaError::IoError`] — filesystem failure
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::get_status;
/// use std::path::Path;
///
/// let summary = get_status(Path::new("/project/.mana")).unwrap();
/// println!("Ready: {}, Blocked: {}", summary.ready.len(), summary.blocked.len());
/// ```
pub fn get_status(mana_dir: &Path) -> Result<StatusSummary> {
    crate::ops::status::status(mana_dir)
}

/// Get aggregate project statistics.
///
/// Returns counts by status, priority distribution, completion percentage,
/// and cost/token metrics from unit history (if available).
///
/// # Errors
/// - [`ManaError::IndexError`] — index cannot be loaded
/// - [`ManaError::IoError`] — filesystem failure
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::get_stats;
/// use std::path::Path;
///
/// let stats = get_stats(Path::new("/project/.mana")).unwrap();
/// println!("Completion: {:.1}%", stats.completion_pct);
/// println!("Total: {}, Open: {}, Closed: {}", stats.total, stats.open, stats.closed);
/// ```
pub fn get_stats(mana_dir: &Path) -> Result<StatsResult> {
    crate::ops::stats::stats(mana_dir)
}

// ---------------------------------------------------------------------------
// Graph functions
// ---------------------------------------------------------------------------

/// Return units with all dependencies satisfied (ready to dispatch).
///
/// A unit is "ready" if it is an open dispatchable task and all of its
/// explicit dependency IDs are closed in the active index or archived.
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::{load_index, ready_units};
/// use std::path::Path;
///
/// let mana_dir = Path::new("/project/.mana");
/// let index = load_index(mana_dir).unwrap();
/// let ready = ready_units(&index);
/// for entry in ready {
///     println!("Ready: {} {}", entry.id, entry.title);
/// }
/// ```
pub fn ready_units(index: &Index) -> Vec<&IndexEntry> {
    let closed_ids: std::collections::HashSet<&str> = index
        .units
        .iter()
        .filter(|e| e.status == Status::Closed)
        .map(|e| e.id.as_str())
        .collect();

    index
        .units
        .iter()
        .filter(|e| {
            e.status == Status::Open
                && e.kind == crate::unit::UnitType::Task
                && e.has_verify
                && e.dependencies
                    .iter()
                    .all(|dep| closed_ids.contains(dep.as_str()))
                && !has_open_children(e, index)
        })
        .collect()
}

/// Build a dependency graph from the active index.
///
/// Returns a [`DependencyGraph`] with all units as nodes and explicit
/// dependency relationships as directed edges (`a -> b` = `a` depends on `b`).
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::{load_index, dependency_graph};
/// use std::path::Path;
///
/// let mana_dir = Path::new("/project/.mana");
/// let index = load_index(mana_dir).unwrap();
/// let graph = dependency_graph(&index);
/// println!("{} nodes, {} with deps", graph.nodes.len(),
///     graph.edges.values().filter(|deps| !deps.is_empty()).count());
/// ```
pub fn dependency_graph(index: &Index) -> DependencyGraph {
    let nodes: HashMap<String, GraphNode> = index
        .units
        .iter()
        .map(|e| {
            (
                e.id.clone(),
                GraphNode {
                    id: e.id.clone(),
                    title: e.title.clone(),
                    status: e.status,
                },
            )
        })
        .collect();

    let edges: HashMap<String, Vec<String>> = index
        .units
        .iter()
        .map(|e| (e.id.clone(), e.dependencies.clone()))
        .collect();

    DependencyGraph { nodes, edges }
}

/// Topologically sort all units by dependency order.
///
/// Returns a list of unit IDs where each unit appears after all its
/// dependencies. Units with no dependencies appear first.
///
/// # Errors
/// - Returns an error if a cycle is detected in the dependency graph.
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::{load_index, topological_sort};
/// use std::path::Path;
///
/// let mana_dir = Path::new("/project/.mana");
/// let index = load_index(mana_dir).unwrap();
/// let order = topological_sort(&index).unwrap();
/// println!("Execution order: {:?}", order);
/// ```
pub fn topological_sort(index: &Index) -> Result<Vec<String>> {
    use std::collections::{HashSet, VecDeque};

    // Build in-degree map and adjacency list
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new();

    for entry in &index.units {
        in_degree.entry(entry.id.clone()).or_insert(0);
        for dep_id in &entry.dependencies {
            in_degree.entry(entry.id.clone()).and_modify(|d| *d += 1);
            dependents
                .entry(dep_id.clone())
                .or_default()
                .push(entry.id.clone());
        }
    }

    // Kahn's algorithm
    let mut queue: VecDeque<String> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(id, _)| id.clone())
        .collect();

    let mut result: Vec<String> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();

    while let Some(id) = queue.pop_front() {
        if visited.contains(&id) {
            continue;
        }
        visited.insert(id.clone());
        result.push(id.clone());

        if let Some(deps_on_me) = dependents.get(&id) {
            for dependent in deps_on_me {
                if let Some(deg) = in_degree.get_mut(dependent) {
                    *deg = deg.saturating_sub(1);
                    if *deg == 0 && !visited.contains(dependent) {
                        queue.push_back(dependent.clone());
                    }
                }
            }
        }
    }

    if result.len() != index.units.len() {
        return Err(anyhow::anyhow!(
            "Cycle detected in dependency graph: {} of {} units could be ordered",
            result.len(),
            index.units.len()
        ));
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Additional graph utilities (re-exported from graph module)
// ---------------------------------------------------------------------------

/// Build a text dependency tree rooted at a unit ID.
///
/// Returns a box-drawing string showing which units depend on the given unit.
///
/// # Errors
/// - Returns an error if the unit ID is not found in the index.
pub use crate::graph::build_dependency_tree;

/// Build a project-wide dependency graph as a text tree.
///
/// Shows all units with no parents as roots, with their dependents branching below.
///
/// # Errors
/// - Returns an error only on unexpected failures.
pub use crate::graph::build_full_graph;

/// Count total verify attempts across all descendants of a unit.
///
/// Includes the unit itself and archived descendants. Used by the circuit
/// breaker to detect runaway retry loops across a subtree.
///
/// # Errors
/// - Returns an error on I/O failures reading the index.
pub use crate::graph::count_subtree_attempts;

/// Find all dependency cycles in the graph.
///
/// Returns a list of cycle paths (each path is a list of unit IDs forming a cycle).
/// An empty list means the graph is acyclic.
///
/// # Errors
/// - Returns an error only on unexpected graph traversal failures.
pub use crate::graph::find_all_cycles;

// Also re-export validate_priority for callers who need to validate
pub use crate::unit::validate_priority;

/// Detect whether adding an edge from `from_id` to `to_id` would create a cycle.
///
/// Returns `true` if the proposed edge would introduce a cycle. Use this
/// before calling [`add_dep`] to pre-validate the addition.
///
/// # Errors
/// - Returns an error only on unexpected graph traversal failures.
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::{load_index, detect_cycle};
/// use std::path::Path;
///
/// let mana_dir = Path::new("/project/.mana");
/// let index = load_index(mana_dir).unwrap();
/// if detect_cycle(&index, "3", "1").unwrap() {
///     eprintln!("Cannot add that dependency — would create a cycle");
/// }
/// ```
pub fn detect_cycle(index: &Index, from_id: &str, to_id: &str) -> Result<bool> {
    crate::graph::detect_cycle(index, from_id, to_id)
}

// ---------------------------------------------------------------------------
// Mutation functions
// ---------------------------------------------------------------------------

/// Create a new unit.
///
/// Assigns the next sequential ID (or child ID if `params.parent` is set),
/// writes the unit file, and rebuilds the index.
///
/// # Errors
/// - [`anyhow::Error`] — validation failure, I/O error, or hook rejection
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::create_unit;
/// use mana_core::ops::create::CreateParams;
/// use std::path::Path;
///
/// let result = create_unit(Path::new("/project/.mana"), CreateParams {
///     title: "Fix the login bug".to_string(),
///     verify: Some("cargo test --test login".to_string()),
///     ..Default::default()
/// }).unwrap();
/// println!("Created unit {}", result.unit.id);
/// ```
pub fn create_unit(mana_dir: &Path, params: create::CreateParams) -> Result<create::CreateResult> {
    create::create(mana_dir, params)
}

/// Update a unit's fields.
///
/// Only fields set to `Some(...)` are updated. Notes are appended with
/// a timestamp separator rather than replaced.
///
/// # Errors
/// - [`anyhow::Error`] — unit not found, validation failure, or hook rejection
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::update_unit;
/// use mana_core::ops::update::UpdateParams;
/// use std::path::Path;
///
/// let result = update_unit(Path::new("/project/.mana"), "1", UpdateParams {
///     notes: Some("Discovered the root cause: off-by-one in pagination".to_string()),
///     ..Default::default()
/// }).unwrap();
/// ```
pub fn update_unit(
    mana_dir: &Path,
    id: &str,
    params: update::UpdateParams,
) -> Result<update::UpdateResult> {
    update::update(mana_dir, id, params)
}

/// Move a unit under a new parent, or detach it to the root.
pub fn reparent_unit(
    mana_dir: &Path,
    id: &str,
    params: crate::ops::reparent::ReparentParams,
) -> Result<crate::ops::reparent::ReparentResult> {
    crate::ops::reparent::reparent(mana_dir, id, params)
}

/// Close a unit — run verify, archive, and cascade to parents.
///
/// The full close lifecycle:
/// 1. Pre-close hook (if configured)
/// 2. Run verify command (unless `opts.force` is true)
/// 3. Worktree merge (if in worktree mode)
/// 4. Feature gate (feature units require manual confirmation)
/// 5. Mark closed and archive
/// 6. Post-close hook and on_close actions
/// 7. Auto-close parents whose children are all done
///
/// Returns a [`close::CloseOutcome`] that describes what happened — the unit
/// may have been closed, verify may have failed, or the close may have been
/// blocked by a hook or feature gate.
///
/// # Errors
/// - [`anyhow::Error`] — unit not found or unexpected I/O failure
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::close_unit;
/// use mana_core::ops::close::{CloseOpts, CloseOutcome};
/// use std::path::Path;
///
/// let outcome = close_unit(Path::new("/project/.mana"), "1", CloseOpts {
///     reason: Some("Implemented and tested".to_string()),
///     force: false,
///     defer_verify: false,
/// }).unwrap();
///
/// match outcome {
///     CloseOutcome::Closed(r) => println!("Closed! Auto-closed parents: {:?}", r.auto_closed_parents),
///     CloseOutcome::VerifyFailed(r) => eprintln!("Verify failed: {}", r.output),
///     _ => {}
/// }
/// ```
pub fn close_unit(
    mana_dir: &Path,
    id: &str,
    opts: close::CloseOpts,
) -> Result<close::CloseOutcome> {
    close::close(mana_dir, id, opts)
}

/// Mark a unit as explicitly failed without closing it.
///
/// Releases the claim, finalizes the current attempt as `Failed`, appends a
/// structured failure summary to notes, and returns the unit to `Open` status
/// for retry.
///
/// # Errors
/// - [`anyhow::Error`] — unit not found or I/O failure
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::fail_unit;
/// use std::path::Path;
///
/// let unit = fail_unit(Path::new("/project/.mana"), "1",
///     Some("Blocked by missing auth token".to_string())).unwrap();
/// assert_eq!(unit.status, mana_core::api::Status::Open);
/// ```
pub fn fail_unit(mana_dir: &Path, id: &str, reason: Option<String>) -> Result<Unit> {
    close::close_failed(mana_dir, id, reason)
}

/// Delete a unit and remove all references to it from other units' dependencies.
///
/// # Errors
/// - [`anyhow::Error`] — unit not found or I/O failure
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::delete_unit;
/// use std::path::Path;
///
/// let r = delete_unit(Path::new("/project/.mana"), "1").unwrap();
/// println!("Deleted: {}", r.title);
/// ```
pub fn delete_unit(mana_dir: &Path, id: &str) -> Result<delete::DeleteResult> {
    delete::delete(mana_dir, id)
}

/// Reopen a closed unit.
///
/// Sets status back to `Open`, clears `closed_at` and `close_reason`,
/// and rebuilds the index.
///
/// # Errors
/// - [`anyhow::Error`] — unit not found or I/O failure
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::reopen_unit;
/// use std::path::Path;
///
/// let r = reopen_unit(Path::new("/project/.mana"), "1").unwrap();
/// println!("Reopened: {}", r.unit.id);
/// ```
pub fn reopen_unit(mana_dir: &Path, id: &str) -> Result<crate::ops::reopen::ReopenResult> {
    crate::ops::reopen::reopen(mana_dir, id)
}

/// Claim a unit for work.
///
/// Sets status to `InProgress`, records who claimed it and when, and starts
/// a new attempt in the attempt log.
///
/// If `params.force` is false and the unit has a verify command with
/// `fail_first: true`, the verify command is run first. If it already passes,
/// the claim is rejected (nothing to do). This enforces fail-first/TDD semantics.
/// Any claimed unit with a verify command also records a checkpoint SHA so
/// later diff/review/close flows can compare against the claim baseline.
///
/// # Errors
/// - [`anyhow::Error`] — unit not found, not open, or verify pre-check failed
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::claim_unit;
/// use mana_core::ops::claim::ClaimParams;
/// use std::path::Path;
///
/// let r = claim_unit(Path::new("/project/.mana"), "1", ClaimParams {
///     by: Some("agent-42".to_string()),
///     force: true,
/// }).unwrap();
/// println!("Claimed by: {}", r.claimer);
/// ```
pub fn claim_unit(
    mana_dir: &Path,
    id: &str,
    params: claim::ClaimParams,
) -> Result<claim::ClaimResult> {
    claim::claim(mana_dir, id, params)
}

/// Release a claim on a unit.
///
/// Clears `claimed_by`/`claimed_at`, sets status back to `Open`, and marks
/// the current attempt as `Abandoned`.
///
/// # Errors
/// - [`anyhow::Error`] — unit not found or I/O failure
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::release_unit;
/// use std::path::Path;
///
/// let r = release_unit(Path::new("/project/.mana"), "1").unwrap();
/// assert_eq!(r.unit.status, mana_core::api::Status::Open);
/// ```
pub fn release_unit(mana_dir: &Path, id: &str) -> Result<claim::ReleaseResult> {
    claim::release(mana_dir, id)
}

/// Add a dependency: `from_id` depends on `dep_id`.
///
/// Validates both units exist, checks for self-dependency, detects cycles,
/// and persists the change.
///
/// # Errors
/// - [`anyhow::Error`] — unit not found, self-dependency, or cycle detected
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::add_dep;
/// use std::path::Path;
///
/// // Unit 3 now depends on unit 2
/// add_dep(Path::new("/project/.mana"), "3", "2").unwrap();
/// ```
pub fn add_dep(mana_dir: &Path, from_id: &str, dep_id: &str) -> Result<dep::DepAddResult> {
    dep::dep_add(mana_dir, from_id, dep_id)
}

/// Remove a dependency: `from_id` no longer depends on `dep_id`.
///
/// # Errors
/// - [`anyhow::Error`] — unit not found or dependency not present
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::remove_dep;
/// use std::path::Path;
///
/// remove_dep(Path::new("/project/.mana"), "3", "2").unwrap();
/// ```
pub fn remove_dep(mana_dir: &Path, from_id: &str, dep_id: &str) -> Result<dep::DepRemoveResult> {
    dep::dep_remove(mana_dir, from_id, dep_id)
}

// ---------------------------------------------------------------------------
// Orchestration functions
// ---------------------------------------------------------------------------

/// Compute which units are ready to dispatch.
///
/// Returns a [`ReadyQueue`] with units sorted by priority then critical-path
/// weight (units blocking the most downstream work come first).
///
/// Optionally filters to a specific unit ID or its ready children if
/// `filter_id` is a parent unit.
///
/// Set `simulate = true` to include all open units with verify commands,
/// even those whose dependencies are not yet met. This is the dry-run mode.
///
/// # Errors
/// - [`anyhow::Error`] — index or I/O failure
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::compute_ready_queue;
/// use std::path::Path;
///
/// let queue = compute_ready_queue(Path::new("/project/.mana"), None, false).unwrap();
/// for unit in &queue.units {
///     println!("Ready: {} (weight={})", unit.id, unit.critical_path_weight);
/// }
/// println!("Blocked: {}", queue.blocked.len());
/// ```
pub fn compute_ready_queue(
    mana_dir: &Path,
    filter_id: Option<&str>,
    simulate: bool,
) -> Result<ReadyQueue> {
    let target = filter_id
        .map(|id| RunTarget::Unit(id.to_string()))
        .unwrap_or(RunTarget::AllReady);
    crate::ops::run::compute_ready_queue(mana_dir, &target, simulate)
}

/// Compute a ready queue for a canonical run target.
pub fn compute_ready_queue_for_target(
    mana_dir: &Path,
    target: &RunTarget,
    simulate: bool,
) -> Result<ReadyQueue> {
    crate::ops::run::compute_ready_queue(mana_dir, target, simulate)
}

/// Assemble the full agent context for a unit.
///
/// Loads the unit, resolves dependency context (which sibling units produce
/// artifacts this unit requires), reads referenced files, and extracts
/// structural summaries. Returns a structured [`AgentContext`] ready for
/// rendering into any format (text prompt, JSON, IPC message).
///
/// # Errors
/// - [`anyhow::Error`] — unit not found or I/O failure
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::assemble_context;
/// use std::path::Path;
///
/// let ctx = assemble_context(Path::new("/project/.mana"), "1").unwrap();
/// println!("Rules: {:?}", ctx.rules.is_some());
/// println!("Files: {}", ctx.files.len());
/// println!("Dep providers: {}", ctx.dep_providers.len());
/// ```
pub fn assemble_context(mana_dir: &Path, id: &str) -> Result<AgentContext> {
    crate::ops::context::assemble_agent_context(mana_dir, id)
}

/// Record a verify attempt result on a unit.
///
/// Appends an [`AttemptRecord`] to the unit's `attempt_log` and persists.
/// Use this when an external orchestrator completes a verify cycle and wants
/// to record the outcome without going through the full close lifecycle.
///
/// # Errors
/// - [`anyhow::Error`] — unit not found or I/O failure
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::{record_attempt, AttemptRecord, AttemptOutcome};
/// use std::path::Path;
/// use chrono::Utc;
///
/// let now = Utc::now();
/// let attempt = AttemptRecord {
///     num: 1,
///     outcome: AttemptOutcome::Success,
///     notes: Some("Passed on first attempt".to_string()),
///     agent: Some("imp-agent".to_string()),
///     started_at: Some(now),
///     finished_at: Some(now),
///     autonomy_observation: None,
/// };
/// record_attempt(Path::new("/project/.mana"), "1", attempt).unwrap();
/// ```
pub fn record_attempt(mana_dir: &Path, id: &str, attempt: AttemptRecord) -> Result<Unit> {
    use crate::discovery::find_unit_file;
    use crate::index::Index;

    let unit_path =
        find_unit_file(mana_dir, id).map_err(|_| anyhow::anyhow!("Unit not found: {}", id))?;
    let mut unit = Unit::from_file(&unit_path)
        .map_err(|e| anyhow::anyhow!("Failed to load unit {}: {}", id, e))?;

    unit.attempt_log.push(attempt);
    unit.updated_at = chrono::Utc::now();

    unit.to_file(&unit_path)
        .map_err(|e| anyhow::anyhow!("Failed to save unit {}: {}", id, e))?;

    let index = Index::build(mana_dir)?;
    index.save(mana_dir)?;

    Ok(unit)
}

/// Run the verify command for a unit without closing it.
///
/// Loads the unit, resolves the effective timeout (unit override → config default),
/// spawns the verify command in a subprocess, and captures all output.
///
/// Returns `Ok(None)` if the unit has no verify command.
///
/// # Errors
/// - [`anyhow::Error`] — unit not found, spawn failure, or I/O error
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::run_verify;
/// use std::path::Path;
///
/// match run_verify(Path::new("/project/.mana"), "1").unwrap() {
///     Some(result) => {
///         if result.passed {
///             println!("Verify passed (exit {:?})", result.exit_code);
///         } else {
///             eprintln!("Verify failed:\n{}", result.stderr);
///         }
///     }
///     None => println!("No verify command"),
/// }
/// ```
pub fn run_verify(mana_dir: &Path, id: &str) -> Result<Option<VerifyResult>> {
    crate::ops::verify::run_verify(mana_dir, id)
}

// ---------------------------------------------------------------------------
// Facts functions
// ---------------------------------------------------------------------------

/// Create a verified fact — a unit that encodes checked project knowledge.
///
/// Facts differ from regular units in that they:
/// - Have `unit_type = "fact"` and the `"fact"` label
/// - Require a verify command (the verification is the point)
/// - Have a TTL (default 30 days) after which they are considered stale
/// - Can reference source file paths for relevance scoring
///
/// # Errors
/// - [`anyhow::Error`] — empty verify command, validation failure, or I/O error
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::create_fact;
/// use mana_core::ops::fact::FactParams;
/// use std::path::Path;
///
/// let r = create_fact(Path::new("/project/.mana"), FactParams {
///     title: "Auth uses RS256 JWT signing".to_string(),
///     verify: "grep -q 'RS256' src/auth.rs".to_string(),
///     description: Some("JWT tokens are signed with RS256 (not HS256)".to_string()),
///     paths: Some("src/auth.rs".to_string()),
///     ttl_days: Some(90),
///     pass_ok: true,
/// }).unwrap();
/// println!("Created fact {} (stale after {:?})", r.unit_id, r.unit.stale_after);
/// ```
pub fn create_fact(mana_dir: &Path, params: fact::FactParams) -> Result<FactResult> {
    fact::create_fact(mana_dir, params)
}

/// Verify all facts and report staleness and failures.
///
/// Re-runs the verify command for every unit with `unit_type = "fact"`.
/// Stale facts (past their `stale_after` date) are reported without re-running.
/// Facts that require artifacts produced by failing/stale facts are flagged as
/// "suspect" (up to depth 3 in the dependency chain).
///
/// Facts whose verify passes have their `stale_after` deadline extended.
///
/// # Errors
/// - [`anyhow::Error`] — index or I/O failure
///
/// # Example
/// ```rust,no_run
/// use mana_core::api::verify_facts;
/// use std::path::Path;
///
/// let r = verify_facts(Path::new("/project/.mana")).unwrap();
/// println!("{}/{} facts verified", r.verified_count, r.total_facts);
/// if r.failing_count > 0 {
///     println!("{} facts failing!", r.failing_count);
/// }
/// ```
pub fn verify_facts(mana_dir: &Path) -> Result<VerifyFactsResult> {
    fact::verify_facts(mana_dir)
}

/// Check the root `facts.mana` fact sheet.
pub fn check_fact_sheet(mana_dir: &Path) -> Result<FactSheetCheckResult> {
    fact_sheet::check_facts_sheet(mana_dir)
}

// Legacy aliases removed — beans→mana rename complete.
