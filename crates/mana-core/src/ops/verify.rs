use std::io::Read;
use std::path::Path;
use std::process::{Command as ShellCommand, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::discovery::find_unit_file;
use crate::unit::Unit;

/// Result of running a verify command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyResult {
    /// Whether the verify command passed (exit 0).
    pub passed: bool,
    /// The process exit code, if available.
    pub exit_code: Option<i32>,
    /// Combined stdout content.
    pub stdout: String,
    /// Combined stderr content.
    pub stderr: String,
    /// Whether the command was killed due to timeout.
    pub timed_out: bool,
    /// The verify command that was run.
    pub command: String,
    /// Timeout that was applied, if any.
    pub timeout_secs: Option<u64>,
}

/// Run the verify command for a unit without closing it.
///
/// Loads the unit, resolves the effective timeout, spawns the verify command,
/// and captures all output. Returns a structured `VerifyResult`.
///
/// If the unit has no verify command, returns `Ok(None)`.
pub fn run_verify(mana_dir: &Path, id: &str) -> Result<Option<VerifyResult>> {
    let unit_path = find_unit_file(mana_dir, id).map_err(|_| anyhow!("Unit not found: {}", id))?;
    let unit =
        Unit::from_file(&unit_path).with_context(|| format!("Failed to load unit: {}", id))?;

    let verify_cmd = match &unit.verify {
        Some(cmd) if !cmd.trim().is_empty() => cmd.clone(),
        _ => return Ok(None),
    };

    let config = Config::load_with_extends(mana_dir).ok();
    let timeout_secs =
        unit.effective_verify_timeout(config.as_ref().and_then(|c| c.verify_timeout));

    let project_root = mana_dir
        .parent()
        .ok_or_else(|| anyhow!("Cannot determine project root from units dir"))?;

    run_verify_command(&verify_cmd, project_root, timeout_secs).map(Some)
}

/// Execute a verify command in the given directory with an optional timeout.
///
/// This is the low-level execution function — no unit loading, no config resolution.
/// Useful when the caller already has the command and timeout.
pub fn run_verify_command(
    verify_cmd: &str,
    working_dir: &Path,
    timeout_secs: Option<u64>,
) -> Result<VerifyResult> {
    let mut child = ShellCommand::new("sh")
        .args(["-c", verify_cmd])
        .current_dir(working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to spawn verify command: {}", verify_cmd))?;

    // Drain output in background threads to prevent pipe deadlock.
    let stdout_thread = {
        let stdout = child.stdout.take().expect("stdout is piped");
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut reader = std::io::BufReader::new(stdout);
            let _ = reader.read_to_end(&mut buf);
            String::from_utf8_lossy(&buf).to_string()
        })
    };
    let stderr_thread = {
        let stderr = child.stderr.take().expect("stderr is piped");
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut reader = std::io::BufReader::new(stderr);
            let _ = reader.read_to_end(&mut buf);
            String::from_utf8_lossy(&buf).to_string()
        })
    };

    let timeout = timeout_secs.map(Duration::from_secs);
    let start = Instant::now();

    let (timed_out, exit_status) = loop {
        match child
            .try_wait()
            .with_context(|| "Failed to poll verify process")?
        {
            Some(status) => break (false, Some(status)),
            None => {
                if let Some(limit) = timeout {
                    if start.elapsed() >= limit {
                        let _ = child.kill();
                        let _ = child.wait();
                        break (true, None);
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    };

    let stdout = stdout_thread.join().unwrap_or_default();
    let stderr = stderr_thread.join().unwrap_or_default();

    let exit_code = exit_status.and_then(|s| s.code());
    let passed = !timed_out && exit_status.map(|s| s.success()).unwrap_or(false);

    Ok(VerifyResult {
        passed,
        exit_code,
        stdout,
        stderr,
        timed_out,
        command: verify_cmd.to_string(),
        timeout_secs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::ops::create::{self, tests::minimal_params};
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn setup() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let bd = dir.path().join(".mana");
        fs::create_dir(&bd).unwrap();
        Config {
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
        .save(&bd)
        .unwrap();
        (dir, bd)
    }

    #[test]
    fn verify_passing_command() {
        let (_dir, bd) = setup();
        let mut params = minimal_params("Task");
        params.verify =
            Some("grep -q 'project: test' .mana/config.yaml && printf hello".to_string());
        create::create(&bd, params).unwrap();

        let result = run_verify(&bd, "1").unwrap().unwrap();
        assert!(result.passed);
        assert_eq!(result.exit_code, Some(0));
        assert!(result.stdout.contains("hello"));
        assert!(!result.timed_out);
    }

    #[test]
    fn verify_failing_command() {
        let (_dir, bd) = setup();
        let mut params = minimal_params("Task");
        params.verify = Some("exit 1".to_string());
        create::create(&bd, params).unwrap();

        let result = run_verify(&bd, "1").unwrap().unwrap();
        assert!(!result.passed);
        assert_eq!(result.exit_code, Some(1));
        assert!(!result.timed_out);
    }

    #[test]
    fn verify_no_command_returns_none() {
        let (_dir, bd) = setup();
        create::create(&bd, minimal_params("Task")).unwrap();

        let result = run_verify(&bd, "1").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn verify_nonexistent_unit() {
        let (_dir, bd) = setup();
        assert!(run_verify(&bd, "99").is_err());
    }

    #[test]
    fn verify_captures_stderr() {
        let (_dir, bd) = setup();
        let mut params = minimal_params("Task");
        params.verify =
            Some("grep -q 'project: test' .mana/config.yaml && printf err >&2".to_string());
        create::create(&bd, params).unwrap();

        let result = run_verify(&bd, "1").unwrap().unwrap();
        assert!(result.passed);
        assert!(result.stderr.contains("err"));
    }

    #[test]
    fn run_verify_command_directly() {
        let dir = TempDir::new().unwrap();
        let result = run_verify_command("echo direct", dir.path(), None).unwrap();
        assert!(result.passed);
        assert!(result.stdout.contains("direct"));
    }

    #[test]
    fn run_verify_command_timeout() {
        let dir = TempDir::new().unwrap();
        let result = run_verify_command("sleep 10", dir.path(), Some(1)).unwrap();
        assert!(!result.passed);
        assert!(result.timed_out);
    }
}
