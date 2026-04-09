//! mana-pool — Resource-aware dispatch engine for mana agents.
//!
//! The pool manages agent lifecycle: deciding what to spawn, enforcing resource
//! limits (concurrency, memory), tracking running agents, and reporting results.
//!
//! # Usage modes
//!
//! - **Embedded** (current): `mana run` creates a pool, dispatches units, waits
//!   for completion. Pool lives for one dispatch cycle.
//! - **Daemon** (future): pool runs persistently, accepts dispatch requests over
//!   a socket, coordinates across multiple projects.

mod dispatch;
mod memory;
mod types;

pub use dispatch::{execute_deferred_verify, run_dispatch};
pub use memory::{available_memory_mb, has_sufficient_memory};
pub use types::*;
