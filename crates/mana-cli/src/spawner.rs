//! Compatibility process spawning, tracking, and log capture for legacy `mana run` flows.
//!
//! Provides [`Spawner`] which manages the lifecycle of agent processes for
//! legacy/compatibility execution paths: building commands from config templates,
//! redirecting output to log files, tracking running processes, and handling
//! unit claim/release lifecycle.
//!
//! This module is migration scaffolding, not the intended long-term execution
//! center. The preferred primary path is `imp run <unit-id>`, with `mana run`
//! and template spawning retained here as compatibility behavior during the
//! transition.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Instant;

use anyhow::{anyhow, Context, Result};

use crate::commands::agents::{save_agents, AgentEntry};
use crate::commands::logs;
use crate::config::{resolve_identity, Config};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// What action an agent should perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentAction {
    /// Unit fits within token budget — implement directly.
    Implement,
    /// Unit exceeds token budget — needs planning/decomposition.
    Plan,
}

impl std::fmt::Display for AgentAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentAction::Implement => write!(f, "implement"),
            AgentAction::Plan => write!(f, "plan"),
        }
    }
}

/// A running agent process tracked by the compatibility spawner.
pub struct AgentProcess {
    pub unit_id: String,
    pub unit_title: String,
    pub action: AgentAction,
    pub pid: u32,
    pub started_at: Instant,
    pub log_path: PathBuf,
    child: Child,
}

/// Result of a completed agent process.
#[derive(Debug)]
pub struct CompletedAgent {
    pub unit_id: String,
    pub unit_title: String,
    pub action: AgentAction,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub duration: std::time::Duration,
    pub log_path: PathBuf,
}

/// Agent-agnostic process spawner for legacy/compatibility flows.
///
/// Manages compatibility lifecycle scaffolding: claim → spawn → track →
/// complete/release. Keep this working during migration, but do not treat it as
/// the intended long-term runtime center.
pub struct Spawner {
    running: HashMap<String, AgentProcess>,
}

// ---------------------------------------------------------------------------
// Template helpers
// ---------------------------------------------------------------------------

/// Replace `{id}` and `{model}` placeholders in a command template.
///
/// If `model` is `Some`, replaces `{model}` with the value.
/// If `model` is `None`, `{model}` is left as-is (backward compatible).
#[must_use]
pub fn substitute_template(template: &str, unit_id: &str) -> String {
    template.replace("{id}", unit_id)
}

/// Replace `{id}` and `{model}` placeholders in a command template.
///
/// Model substitution follows precedence: unit-level override > config-level > no substitution.
#[must_use]
pub fn substitute_template_with_model(
    template: &str,
    unit_id: &str,
    model: Option<&str>,
) -> String {
    let result = template.replace("{id}", unit_id);
    match model {
        Some(m) => result.replace("{model}", m),
        None => result,
    }
}

/// Build the log file path for a unit spawn.
///
/// Format: `{log_dir}/{safe_id}-{timestamp}.log`
/// Dots in unit IDs are replaced with underscores for filesystem safety.
pub fn build_log_path(unit_id: &str) -> Result<PathBuf> {
    let dir = logs::log_dir()?;
    let safe_id = unit_id.replace('.', "_");
    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    Ok(dir.join(format!("{}-{}.log", safe_id, timestamp)))
}

// ---------------------------------------------------------------------------
// Spawner implementation
// ---------------------------------------------------------------------------

impl Spawner {
    /// Create an empty spawner with no running agents.
    #[must_use]
    pub fn new() -> Self {
        Self {
            running: HashMap::new(),
        }
    }

    /// Spawn an agent for a unit through the legacy/compatibility path.
    ///
    /// 1. Selects the command template from config (`run` or `plan`)
    /// 2. Substitutes `{id}` with the unit ID
    /// 3. Claims the unit via `mana claim`
    /// 4. Opens a log file for stdout/stderr capture
    /// 5. Spawns the process via `sh -c <cmd>`
    /// 6. Registers the process in the agents persistence file
    ///
    /// This remains useful migration scaffolding while the repo still supports
    /// `mana run` and shell-template dispatch, but it is not the intended
    /// long-term execution center.
    pub fn spawn(
        &mut self,
        unit_id: &str,
        unit_title: &str,
        action: AgentAction,
        config: &Config,
        mana_dir: Option<&std::path::Path>,
    ) -> Result<()> {
        if self.running.contains_key(unit_id) {
            return Err(anyhow!("Unit {} already has a running agent", unit_id));
        }

        let (template, model) = match action {
            AgentAction::Implement => (
                config
                    .run
                    .as_deref()
                    .ok_or_else(|| anyhow!("No run template configured"))?,
                config.run_model.as_deref(),
            ),
            AgentAction::Plan => (
                config
                    .plan
                    .as_deref()
                    .ok_or_else(|| anyhow!("No plan template configured"))?,
                config.plan_model.as_deref(),
            ),
        };

        let cmd = substitute_template_with_model(template, unit_id, model);
        let log_path = build_log_path(unit_id)?;

        // Build agent identity: user/agent-N (namespaced under the user who spawned)
        let agent_identity = build_agent_identity(mana_dir);

        // Claim the unit before spawning with agent identity
        claim_unit(unit_id, agent_identity.as_deref())?;

        // Open log file for output capture
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("Failed to open log file: {}", log_path.display()))?;
        let log_stderr = log_file
            .try_clone()
            .context("Failed to clone log file handle")?;

        // Set IMP_MODE so legacy headless compatibility flows get the right tool restrictions.
        let imp_mode = match action {
            AgentAction::Implement => "worker",
            AgentAction::Plan => "planner",
        };

        // Spawn the process
        let child = match Command::new("sh")
            .args(["-c", &cmd])
            .env("IMP_MODE", imp_mode)
            .stdout(log_file)
            .stderr(log_stderr)
            .spawn()
        {
            Ok(child) => child,
            Err(e) => {
                // Release claim on spawn failure
                let _ = release_unit(unit_id);
                return Err(anyhow!("Failed to spawn agent for {}: {}", unit_id, e));
            }
        };

        let pid = child.id();

        // Register in agents persistence file
        let _ = register_agent(unit_id, unit_title, action, pid, &log_path);

        self.running.insert(
            unit_id.to_string(),
            AgentProcess {
                unit_id: unit_id.to_string(),
                unit_title: unit_title.to_string(),
                action,
                pid,
                started_at: Instant::now(),
                log_path,
                child,
            },
        );

        Ok(())
    }

    /// Non-blocking check for completed agents.
    ///
    /// Calls `try_wait()` on each running process. Completed agents are
    /// removed from the running map and returned. On failure, the unit
    /// claim is released.
    pub fn check_completed(&mut self) -> Vec<CompletedAgent> {
        let mut completed = Vec::new();
        let mut finished_ids = Vec::new();

        for (id, proc) in self.running.iter_mut() {
            match proc.child.try_wait() {
                Ok(Some(status)) => {
                    let success = status.success();
                    let exit_code = status.code();

                    if !success {
                        let _ = release_unit(id);
                    }

                    // Update agents persistence
                    let _ = finish_agent(id, exit_code);

                    completed.push(CompletedAgent {
                        unit_id: id.clone(),
                        unit_title: proc.unit_title.clone(),
                        action: proc.action,
                        success,
                        exit_code,
                        duration: proc.started_at.elapsed(),
                        log_path: proc.log_path.clone(),
                    });
                    finished_ids.push(id.clone());
                }
                Ok(None) => {} // Still running
                Err(e) => {
                    eprintln!("Error checking agent for {}: {}", id, e);
                    let _ = release_unit(id);
                    let _ = finish_agent(id, Some(-1));
                    completed.push(CompletedAgent {
                        unit_id: id.clone(),
                        unit_title: proc.unit_title.clone(),
                        action: proc.action,
                        success: false,
                        exit_code: Some(-1),
                        duration: proc.started_at.elapsed(),
                        log_path: proc.log_path.clone(),
                    });
                    finished_ids.push(id.clone());
                }
            }
        }

        for id in finished_ids {
            self.running.remove(&id);
        }

        completed
    }

    /// Number of currently running agents.
    #[must_use]
    pub fn running_count(&self) -> usize {
        self.running.len()
    }

    /// Whether a new agent can be spawned given the concurrency limit.
    #[must_use]
    pub fn can_spawn(&self, max_concurrent: u32) -> bool {
        (self.running.len() as u32) < max_concurrent
    }

    /// Immutable view of all running agent processes.
    #[must_use]
    pub fn list_running(&self) -> Vec<&AgentProcess> {
        self.running.values().collect()
    }

    /// Kill all running agent processes and release their claims.
    pub fn kill_all(&mut self) {
        for (id, proc) in self.running.iter_mut() {
            let _ = proc.child.kill();
            let _ = proc.child.wait(); // Reap the zombie
            let _ = release_unit(id);
            let _ = finish_agent(id, Some(-9));
        }
        self.running.clear();
    }

    /// Gracefully shutdown all running agent processes.
    ///
    /// Sends SIGTERM first, waits up to `grace_period` for processes to exit,
    /// then falls back to SIGKILL for any remaining. Releases claims on all
    /// affected units.
    pub fn shutdown_all(&mut self, grace_period: std::time::Duration) {
        if self.running.is_empty() {
            return;
        }

        // Send SIGTERM to all children
        for proc in self.running.values() {
            unsafe {
                libc::kill(proc.pid as i32, libc::SIGTERM);
            }
        }

        // Wait for graceful shutdown
        let deadline = Instant::now() + grace_period;
        loop {
            let mut finished_ids = Vec::new();
            for (id, proc) in self.running.iter_mut() {
                if let Ok(Some(_)) = proc.child.try_wait() {
                    finished_ids.push(id.clone());
                }
            }
            for id in &finished_ids {
                let _ = release_unit(id);
                let _ = finish_agent(id, Some(-15));
                self.running.remove(id);
            }
            if self.running.is_empty() || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        // SIGKILL any remaining processes
        self.kill_all();
    }
}

impl Default for Spawner {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Unit lifecycle helpers (shell out to `bn`)
// ---------------------------------------------------------------------------

/// Build an agent identity string: `user/agent-PID` or just `agent-PID`.
fn build_agent_identity(mana_dir: Option<&std::path::Path>) -> Option<String> {
    let pid = std::process::id();
    let user = mana_dir.and_then(resolve_identity);
    match user {
        Some(u) => Some(format!("{}/agent-{}", u, pid)),
        None => Some(format!("agent-{}", pid)),
    }
}

/// Claim a unit by running `mana claim {id}`.
fn claim_unit(unit_id: &str, by: Option<&str>) -> Result<()> {
    let mut args = vec!["claim", unit_id, "--force"];
    let by_owned;
    if let Some(identity) = by {
        args.push("--by");
        by_owned = identity.to_string();
        args.push(&by_owned);
    }
    let status = Command::new("mana")
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .with_context(|| format!("Failed to run mana claim {}", unit_id))?;

    if !status.success() {
        return Err(anyhow!(
            "mana claim {} failed with exit code {}",
            unit_id,
            status.code().unwrap_or(-1)
        ));
    }
    Ok(())
}

/// Release a unit claim by running `mana claim {id} --release`.
fn release_unit(unit_id: &str) -> Result<()> {
    let status = Command::new("mana")
        .args(["claim", unit_id, "--release"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .with_context(|| format!("Failed to run mana claim {} --release", unit_id))?;

    if !status.success() {
        return Err(anyhow!(
            "mana claim {} --release failed with exit code {}",
            unit_id,
            status.code().unwrap_or(-1)
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Agents persistence helpers
// ---------------------------------------------------------------------------

/// Register a newly spawned agent in the agents.json persistence file.
fn register_agent(
    unit_id: &str,
    unit_title: &str,
    action: AgentAction,
    pid: u32,
    log_path: &std::path::Path,
) -> Result<()> {
    let mut agents = crate::commands::agents::load_agents().unwrap_or_default();
    agents.insert(
        unit_id.to_string(),
        AgentEntry {
            pid,
            title: unit_title.to_string(),
            action: action.to_string(),
            started_at: chrono::Utc::now().timestamp(),
            log_path: Some(log_path.display().to_string()),
            finished_at: None,
            exit_code: None,
        },
    );
    save_agents(&agents)
}

/// Mark an agent as finished in the agents.json persistence file.
fn finish_agent(unit_id: &str, exit_code: Option<i32>) -> Result<()> {
    let mut agents = crate::commands::agents::load_agents().unwrap_or_default();
    if let Some(entry) = agents.get_mut(unit_id) {
        entry.finished_at = Some(chrono::Utc::now().timestamp());
        entry.exit_code = exit_code;
        save_agents(&agents)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Re-exports from commands::logs for convenience
// ---------------------------------------------------------------------------

/// Return the log directory path, creating it if needed.
///
/// Logs are stored at `~/.local/share/units/logs/`.
pub fn log_dir() -> Result<PathBuf> {
    logs::log_dir()
}

/// Find the most recent log file for a unit.
pub fn find_latest_log(unit_id: &str) -> Result<Option<PathBuf>> {
    logs::find_latest_log(unit_id)
}

/// Find all log files for a unit, sorted oldest to newest.
pub fn find_all_logs(unit_id: &str) -> Result<Vec<PathBuf>> {
    logs::find_all_logs(unit_id)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn spawner_starts_empty() {
        let spawner = Spawner::new();
        assert_eq!(spawner.running_count(), 0);
        assert!(spawner.list_running().is_empty());
    }

    #[test]
    fn can_spawn_respects_max_concurrent() {
        let spawner = Spawner::new();
        assert!(spawner.can_spawn(4));
        assert!(spawner.can_spawn(1));
        // Zero means no slots available
        assert!(!spawner.can_spawn(0));
    }

    #[test]
    fn can_spawn_false_when_full() {
        let mut spawner = Spawner::new();

        // Manually insert a fake process to simulate a running agent.
        // We spawn `sleep 60` so it stays alive during the test.
        let log_path = std::env::temp_dir().join("test-spawner-full.log");
        let log_file = File::create(&log_path).unwrap();
        let log_stderr = log_file.try_clone().unwrap();
        let child = Command::new("sleep")
            .arg("60")
            .stdout(log_file)
            .stderr(log_stderr)
            .spawn()
            .unwrap();

        spawner.running.insert(
            "1".to_string(),
            AgentProcess {
                unit_id: "1".to_string(),
                unit_title: "Test".to_string(),
                action: AgentAction::Implement,
                pid: child.id(),
                started_at: Instant::now(),
                log_path: log_path.clone(),
                child,
            },
        );

        assert!(!spawner.can_spawn(1));
        assert!(spawner.can_spawn(2));

        // Clean up
        spawner.kill_all();
        let _ = std::fs::remove_file(&log_path);
    }

    #[test]
    fn log_dir_creates_directory() {
        let dir = log_dir().unwrap();
        assert!(dir.exists());
        assert!(dir.is_dir());
    }

    #[test]
    fn template_substitution_replaces_id() {
        assert_eq!(
            substitute_template("deli spawn {id}", "5.1"),
            "deli spawn 5.1"
        );
        assert_eq!(
            substitute_template(
                "claude -p 'implement unit {id} and run mana close {id}'",
                "42"
            ),
            "claude -p 'implement unit 42 and run mana close 42'"
        );
    }

    #[test]
    fn template_substitution_no_placeholder() {
        assert_eq!(substitute_template("echo hello", "5.1"), "echo hello");
    }

    #[test]
    fn template_substitution_multiple_placeholders() {
        assert_eq!(substitute_template("{id}-{id}-{id}", "3"), "3-3-3");
    }

    #[test]
    fn template_with_model_substitution() {
        assert_eq!(
            substitute_template_with_model(
                "claude --model {model} -p 'implement {id}'",
                "5",
                Some("sonnet")
            ),
            "claude --model sonnet -p 'implement 5'"
        );
    }

    #[test]
    fn template_with_model_none_leaves_placeholder() {
        assert_eq!(
            substitute_template_with_model("claude --model {model} -p 'implement {id}'", "5", None),
            "claude --model {model} -p 'implement 5'"
        );
    }

    #[test]
    fn template_with_model_no_model_placeholder() {
        // If template doesn't use {model}, model config is ignored (backward compatible)
        assert_eq!(
            substitute_template_with_model("echo {id}", "5", Some("opus")),
            "echo 5"
        );
    }

    #[test]
    fn find_latest_log_returns_none_for_unknown() {
        let result = find_latest_log("nonexistent_spawner_test_99999").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn find_all_logs_empty_for_unknown() {
        let result = find_all_logs("nonexistent_spawner_test_99999").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn build_log_path_uses_safe_id() {
        let path = build_log_path("5.1").unwrap();
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(filename.starts_with("5_1-"), "Got: {}", filename);
        assert!(filename.ends_with(".log"), "Got: {}", filename);
    }

    #[test]
    fn build_log_path_simple_id() {
        let path = build_log_path("42").unwrap();
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(filename.starts_with("42-"), "Got: {}", filename);
        assert!(filename.ends_with(".log"), "Got: {}", filename);
    }

    #[test]
    fn check_completed_on_empty_spawner() {
        let mut spawner = Spawner::new();
        let completed = spawner.check_completed();
        assert!(completed.is_empty());
    }

    #[test]
    fn check_completed_detects_finished_process() {
        let mut spawner = Spawner::new();

        // Spawn a process that exits immediately
        let log_path = std::env::temp_dir().join("test-spawner-finished.log");
        let log_file = File::create(&log_path).unwrap();
        let log_stderr = log_file.try_clone().unwrap();
        let child = Command::new("true")
            .stdout(log_file)
            .stderr(log_stderr)
            .spawn()
            .unwrap();

        spawner.running.insert(
            "test-1".to_string(),
            AgentProcess {
                unit_id: "test-1".to_string(),
                unit_title: "Instant task".to_string(),
                action: AgentAction::Implement,
                pid: child.id(),
                started_at: Instant::now(),
                log_path: log_path.clone(),
                child,
            },
        );

        // Give it a moment to exit
        std::thread::sleep(std::time::Duration::from_millis(100));

        let completed = spawner.check_completed();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].unit_id, "test-1");
        assert!(completed[0].success);
        assert_eq!(completed[0].exit_code, Some(0));
        assert_eq!(spawner.running_count(), 0);

        let _ = std::fs::remove_file(&log_path);
    }

    #[test]
    fn check_completed_detects_failed_process() {
        let mut spawner = Spawner::new();

        let log_path = std::env::temp_dir().join("test-spawner-failed.log");
        let log_file = File::create(&log_path).unwrap();
        let log_stderr = log_file.try_clone().unwrap();
        let child = Command::new("false")
            .stdout(log_file)
            .stderr(log_stderr)
            .spawn()
            .unwrap();

        spawner.running.insert(
            "test-2".to_string(),
            AgentProcess {
                unit_id: "test-2".to_string(),
                unit_title: "Failing task".to_string(),
                action: AgentAction::Plan,
                pid: child.id(),
                started_at: Instant::now(),
                log_path: log_path.clone(),
                child,
            },
        );

        std::thread::sleep(std::time::Duration::from_millis(100));

        let completed = spawner.check_completed();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].unit_id, "test-2");
        assert!(!completed[0].success);
        assert_eq!(completed[0].exit_code, Some(1));

        let _ = std::fs::remove_file(&log_path);
    }

    #[test]
    fn kill_all_clears_running() {
        let mut spawner = Spawner::new();

        let log_path = std::env::temp_dir().join("test-spawner-killall.log");
        let log_file = File::create(&log_path).unwrap();
        let log_stderr = log_file.try_clone().unwrap();
        let child = Command::new("sleep")
            .arg("60")
            .stdout(log_file)
            .stderr(log_stderr)
            .spawn()
            .unwrap();

        spawner.running.insert(
            "test-3".to_string(),
            AgentProcess {
                unit_id: "test-3".to_string(),
                unit_title: "Long task".to_string(),
                action: AgentAction::Implement,
                pid: child.id(),
                started_at: Instant::now(),
                log_path: log_path.clone(),
                child,
            },
        );

        assert_eq!(spawner.running_count(), 1);
        spawner.kill_all();
        assert_eq!(spawner.running_count(), 0);

        let _ = std::fs::remove_file(&log_path);
    }

    #[test]
    fn spawn_errors_without_run_template() {
        let mut spawner = Spawner::new();
        let config = Config {
            project: "test".to_string(),
            next_id: 1,
            auto_close_parent: true,
            run: None,
            plan: None,
            max_loops: 10,
            max_concurrent: 4,
            poll_interval: 30,
            extends: vec![],
            rules_file: None,
            file_locking: false,
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
            worktree: false,
        };

        let result = spawner.spawn("1", "Test", AgentAction::Implement, &config, None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("No run template"), "Got: {}", msg);
        assert!(!msg.contains("primary execution center"));
    }

    #[test]
    fn spawn_errors_without_plan_template() {
        let mut spawner = Spawner::new();
        let config = Config {
            project: "test".to_string(),
            next_id: 1,
            auto_close_parent: true,
            run: Some("echo {id}".to_string()),
            plan: None,
            max_loops: 10,
            max_concurrent: 4,
            poll_interval: 30,
            extends: vec![],
            rules_file: None,
            file_locking: false,
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
            worktree: false,
        };

        let result = spawner.spawn("1", "Test", AgentAction::Plan, &config, None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("No plan template"), "Got: {}", msg);
    }

    #[test]
    fn default_creates_empty_spawner() {
        let spawner = Spawner::default();
        assert_eq!(spawner.running_count(), 0);
    }

    #[test]
    fn agent_action_display() {
        assert_eq!(AgentAction::Implement.to_string(), "implement");
        assert_eq!(AgentAction::Plan.to_string(), "plan");
    }
}
