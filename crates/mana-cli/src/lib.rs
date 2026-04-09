// External workspace crates re-exported where helpful for command modules.
pub extern crate mana_review;

// Core modules — re-exported from mana-core
pub use crate::commands::run::{
    run_native, run_with_stream_capture, run_with_stream_capture_and_sink, NativeRunParams,
    RunArgs, RunSummary, RunTarget, RunUnitStatus, RunView,
};
pub use mana_core::agent_presets;
pub use mana_core::api;
pub use mana_core::blocking;
pub use mana_core::config;
pub use mana_core::ctx_assembler;
pub use mana_core::discovery;
pub use mana_core::failure;
pub use mana_core::graph;
pub use mana_core::history;
pub use mana_core::hooks;
pub use mana_core::index;
pub use mana_core::locks;
pub use mana_core::prompt;
pub use mana_core::relevance;
pub use mana_core::unit;
pub use mana_core::util;
pub use mana_core::worktree;

// CLI-only modules
pub(crate) mod cli;
pub mod commands;
pub mod mcp;
pub mod output;
pub(crate) mod pi_output;
pub(crate) mod project;
#[allow(dead_code)]
pub mod spawner;
pub mod stream;
pub(crate) mod timeout;
