//! # mana-core
//!
//! Core library for the mana work coordination system.
//!
//! `mana-core` provides the full data model, I/O layer, and orchestration logic
//! for managing units (atomic work items) in a `.mana/` project directory.
//! It is the single source of truth consumed by the mana CLI, GUI tooling,
//! MCP servers, and any other integration point.
//!
//! ## Crate layout
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`api`] | High-level public API — the recommended entry point |
//! | `unit` | Core `Unit` data model and serialization |
//! | [`config`] | Project and global configuration |
//! | [`index`] | Fast unit cache (`index.yaml`) |
//! | [`graph`] | Dependency graph utilities |
//! | [`ops`] | Low-level operations (create, update, close, claim, …) |
//! | [`error`] | Typed error and result types |
//! | [`discovery`] | `.mana/` directory and unit file discovery |
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use mana_core::api::{find_mana_dir, load_index, get_unit, list_units};
//! use mana_core::ops::list::ListParams;
//! use std::path::Path;
//!
//! // 1. Locate the .mana/ directory (walks up from the current path)
//! let mana_dir = find_mana_dir(Path::new(".")).expect("not inside a mana project");
//!
//! // 2. Load the cached index (rebuilds from unit files if stale)
//! let index = load_index(&mana_dir).expect("failed to load index");
//! println!("{} units in project", index.units.len());
//!
//! // 3. Read a specific unit by ID
//! let unit = get_unit(&mana_dir, "1").expect("unit not found");
//! println!("{}: {} ({:?})", unit.id, unit.title, unit.status);
//!
//! // 4. List all open units
//! let open = list_units(&mana_dir, &ListParams::default()).expect("list failed");
//! for entry in open {
//!     println!("  [{}] {}", entry.id, entry.title);
//! }
//! ```
//!
//! ## Design principles
//!
//! - **`&Path` as entry point** — every function takes `mana_dir: &Path`.
//!   No global state, no singletons.
//! - **No stdout/stderr side effects** — all output is returned as structured data.
//! - **Serializable** — all public types derive `Serialize`/`Deserialize` for
//!   JSON-RPC, MCP, and Tauri IPC.
//! - **Thread-safe** — no interior mutability or shared mutable globals.

pub mod agent_presets;
pub mod api;
pub mod blocking;
pub mod config;
pub mod ctx_assembler;
pub mod discovery;
pub mod error;
pub mod failure;
pub mod graph;
pub mod history;
pub mod hooks;
pub mod index;
pub mod locks;
pub mod ops;
pub mod prompt;
pub mod relevance;
pub mod sqlite;
pub mod unit;
pub mod util;
pub mod verify_lint;
pub mod worktree;
pub mod yaml;
