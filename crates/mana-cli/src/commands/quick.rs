use std::path::Path;
use std::process::Command as ShellCommand;

use anyhow::{anyhow, Context, Result};
use chrono::Utc;

use crate::commands::create::{assign_child_id, lint_verify_command};
use crate::config::Config;
use crate::hooks::{execute_hook, HookEvent};
use crate::index::Index;
use crate::project::suggest_verify_command;
use crate::unit::{validate_priority, OnFailAction, Status, Unit};
use crate::util::{find_similar_titles, title_to_slug, DEFAULT_SIMILARITY_THRESHOLD};

fn git_head_sha(working_dir: &Path) -> Option<String> {
    ShellCommand::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(working_dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Arguments for quick-create command.
pub struct QuickArgs {
    pub title: String,
    pub handle: Option<String>,
    pub description: Option<String>,
    pub acceptance: Option<String>,
    pub notes: Option<String>,
    pub verify: Option<String>,
    pub priority: Option<u8>,
    pub by: Option<String>,
    pub produces: Option<String>,
    pub requires: Option<String>,
    /// Parent unit ID (creates child unit under parent)
    pub parent: Option<String>,
    /// Action on verify failure
    pub on_fail: Option<OnFailAction>,
    /// Skip fail-first check (allow verify to already pass)
    pub pass_ok: bool,
    /// Timeout in seconds for the verify command (kills process on expiry).
    pub verify_timeout: Option<u64>,
    /// Skip duplicate title check
    pub force: bool,
}

/// Quick-create: create a unit and immediately claim it.
///
/// This is a convenience command that combines `mana create` + `mana claim`
/// in a single operation. Useful for agents starting immediate work.
pub fn cmd_quick(mana_dir: &Path, args: QuickArgs) -> Result<()> {
    // Validate priority if provided
    if let Some(priority) = args.priority {
        validate_priority(priority)?;
    }

    // Require at least acceptance or verify criteria
    if args.acceptance.is_none() && args.verify.is_none() {
        anyhow::bail!(
            "Unit must have validation criteria: provide --acceptance or --verify (or both)"
        );
    }

    lint_verify_command(args.verify.as_deref(), args.force)?;

    // Fail-first check (default): verify command must FAIL before unit can be created
    // This prevents "cheating tests" like `assert True` that always pass
    // Use --pass-ok / -p to skip this check
    if !args.pass_ok {
        if let Some(verify_cmd) = args.verify.as_ref() {
            let project_root = mana_dir
                .parent()
                .ok_or_else(|| anyhow!("Cannot determine project root"))?;

            println!("Running verify (must fail): {}", verify_cmd);

            let status = ShellCommand::new("sh")
                .args(["-c", verify_cmd])
                .current_dir(project_root)
                .status()
                .with_context(|| format!("Failed to execute verify command: {}", verify_cmd))?;

            if status.success() {
                anyhow::bail!(
                    "Cannot create unit: verify command already passes!\n\n\
                     The test must FAIL on current code to prove it tests something real.\n\
                     Either:\n\
                     - The test doesn't actually test the new behavior\n\
                     - The feature is already implemented\n\
                     - The test is a no-op (assert True)\n\n\
                     Use --pass-ok / -p to skip this check."
                );
            }

            println!("✓ Verify failed as expected - test is real");
        }
    }

    // Duplicate title check (skip with --force)
    if !args.force {
        if let Ok(index) = Index::load_or_rebuild(mana_dir) {
            let similar = find_similar_titles(&index, &args.title, DEFAULT_SIMILARITY_THRESHOLD);
            if !similar.is_empty() {
                let mut msg = String::from("Similar unit(s) already exist:\n");
                for s in &similar {
                    msg.push_str(&format!(
                        "  [{}] {} (similarity: {:.0}%)\n",
                        s.id,
                        s.title,
                        s.score * 100.0
                    ));
                }
                msg.push_str("\nUse --force to create anyway.");
                anyhow::bail!(msg);
            }
        }
    }

    // Load config and assign ID (child ID from parent, or next global ID)
    let unit_id = if let Some(ref parent_id) = args.parent {
        assign_child_id(mana_dir, parent_id)?
    } else {
        let mut config = Config::load(mana_dir)?;
        let id = config.increment_id().to_string();
        config.save(mana_dir)?;
        id
    };

    // Generate slug from title
    let slug = title_to_slug(&args.title);

    // Track if verify was provided for suggestion later
    let has_verify = args.verify.is_some();

    // Create the unit with InProgress status (already claimed)
    let now = Utc::now();
    let mut unit = Unit::new(&unit_id, &args.title);
    unit.slug = Some(slug.clone());
    unit.handle = args.handle;
    unit.ensure_handle();
    unit.status = Status::InProgress;
    unit.claimed_by = args.by.clone();
    unit.claimed_at = Some(now);

    if let Some(desc) = args.description {
        unit.description = Some(desc);
    }
    if let Some(acceptance) = args.acceptance {
        unit.acceptance = Some(acceptance);
    }
    if let Some(notes) = args.notes {
        unit.notes = Some(notes);
    }
    let has_fail_first = !args.pass_ok && args.verify.is_some();
    if let Some(verify) = args.verify {
        unit.verify = Some(verify);
    }
    if has_fail_first {
        unit.fail_first = true;
    }
    if let Some(priority) = args.priority {
        unit.priority = priority;
    }
    if let Some(parent) = args.parent {
        unit.parent = Some(parent);
    }

    // Parse produces
    if let Some(produces_str) = args.produces {
        unit.produces = produces_str
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();
    }

    // Parse requires
    if let Some(requires_str) = args.requires {
        unit.requires = requires_str
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();
    }

    // Set on_fail action
    if let Some(on_fail) = args.on_fail {
        unit.on_fail = Some(on_fail);
    }

    // Set verify_timeout if provided
    if let Some(timeout) = args.verify_timeout {
        unit.verify_timeout = Some(timeout);
    }

    // Get the project directory (parent of mana_dir which is .mana)
    let project_dir = mana_dir
        .parent()
        .ok_or_else(|| anyhow!("Failed to determine project directory"))?;

    if has_verify {
        unit.checkpoint = git_head_sha(project_dir);
    }

    // Call pre-create hook (blocking - abort if it fails)
    let pre_passed = execute_hook(HookEvent::PreCreate, &unit, project_dir, None)
        .context("Pre-create hook execution failed")?;

    if !pre_passed {
        return Err(anyhow!("Pre-create hook rejected unit creation"));
    }

    // Write the unit file with naming convention: {id}-{slug}.md
    let unit_path = mana_dir.join(format!("{}-{}.md", unit_id, slug));
    unit.to_file(&unit_path)?;

    // Update the index by rebuilding from disk
    let index = Index::build(mana_dir)?;
    index.save(mana_dir)?;

    let claimer = args.by.as_deref().unwrap_or("anonymous");
    println!(
        "Created and claimed unit {}: {} (by {})",
        unit_id, args.title, claimer
    );

    // Suggest verify command if none was provided
    if !has_verify {
        if let Some(suggested) = suggest_verify_command(project_dir) {
            println!(
                "Tip: Consider adding a verify command: --verify \"{}\"",
                suggested
            );
        }
    }

    // Call post-create hook (non-blocking - log warning if it fails)
    if let Err(e) = execute_hook(HookEvent::PostCreate, &unit, project_dir, None) {
        eprintln!("Warning: post-create hook failed: {}", e);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    fn setup_mana_dir_with_config() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

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
        };
        config.save(&mana_dir).unwrap();

        (dir, mana_dir)
    }

    fn setup_git_mana_dir_with_config() -> (TempDir, std::path::PathBuf) {
        let (dir, mana_dir) = setup_mana_dir_with_config();
        let project_root = dir.path();

        let init = Command::new("git")
            .args(["init"])
            .current_dir(project_root)
            .output()
            .unwrap();
        assert!(init.status.success());

        let commit = Command::new("git")
            .args(["commit", "-m", "init", "--allow-empty"])
            .current_dir(project_root)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap();
        assert!(commit.status.success());

        (dir, mana_dir)
    }

    #[test]
    fn quick_creates_and_claims_unit() {
        let (_dir, mana_dir) = setup_mana_dir_with_config();

        let args = QuickArgs {
            title: "Quick task".to_string(),
            handle: None,
            description: None,
            acceptance: Some("Done".to_string()),
            notes: None,
            verify: None,
            priority: None,
            by: Some("agent-1".to_string()),
            produces: None,
            requires: None,
            parent: None,
            on_fail: None,
            pass_ok: true,
            verify_timeout: None,
            force: false,
        };

        cmd_quick(&mana_dir, args).unwrap();

        // Check the unit file exists
        let unit_path = mana_dir.join("1-quick-task.md");
        assert!(unit_path.exists());

        // Verify content
        let unit = Unit::from_file(&unit_path).unwrap();
        assert_eq!(unit.id, "1");
        assert_eq!(unit.title, "Quick task");
        assert_eq!(unit.status, Status::InProgress);
        assert_eq!(unit.claimed_by, Some("agent-1".to_string()));
        assert!(unit.claimed_at.is_some());
    }

    #[test]
    fn quick_works_without_by() {
        let (_dir, mana_dir) = setup_mana_dir_with_config();

        let args = QuickArgs {
            title: "Anonymous task".to_string(),
            handle: None,
            description: None,
            acceptance: None,
            notes: None,
            verify: Some("cargo test unit::check".to_string()),
            priority: None,
            by: None,
            produces: None,
            requires: None,
            parent: None,
            on_fail: None,
            pass_ok: true,
            verify_timeout: None,
            force: false,
        };

        cmd_quick(&mana_dir, args).unwrap();

        let unit_path = mana_dir.join("1-anonymous-task.md");
        let unit = Unit::from_file(&unit_path).unwrap();
        assert_eq!(unit.status, Status::InProgress);
        assert_eq!(unit.claimed_by, None);
        assert!(unit.claimed_at.is_some());
    }

    #[test]
    fn quick_sets_checkpoint_when_verify_present() {
        let (_dir, mana_dir) = setup_git_mana_dir_with_config();

        let args = QuickArgs {
            title: "Checkpointed task".to_string(),
            handle: None,
            description: None,
            acceptance: None,
            notes: None,
            verify: Some("grep -q 'project: test' .mana/config.yaml".to_string()),
            priority: None,
            by: None,
            produces: None,
            requires: None,
            parent: None,
            on_fail: None,
            pass_ok: true,
            verify_timeout: None,
            force: false,
        };

        cmd_quick(&mana_dir, args).unwrap();

        let unit_path = mana_dir.join("1-checkpointed-task.md");
        let unit = Unit::from_file(&unit_path).unwrap();
        assert!(unit.checkpoint.is_some());
    }

    #[test]
    fn quick_rejects_missing_validation_criteria() {
        let (_dir, mana_dir) = setup_mana_dir_with_config();

        let args = QuickArgs {
            title: "No criteria".to_string(),
            handle: None,
            description: None,
            acceptance: None,
            notes: None,
            verify: None,
            priority: None,
            by: None,
            produces: None,
            requires: None,
            parent: None,
            on_fail: None,
            pass_ok: true,
            verify_timeout: None,
            force: false,
        };

        let result = cmd_quick(&mana_dir, args);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("validation criteria"));
    }

    #[test]
    fn quick_increments_id() {
        let (_dir, mana_dir) = setup_mana_dir_with_config();

        // Create first unit
        let args1 = QuickArgs {
            title: "First".to_string(),
            handle: None,
            description: None,
            acceptance: Some("Done".to_string()),
            notes: None,
            verify: None,
            priority: None,
            by: None,
            produces: None,
            requires: None,
            parent: None,
            on_fail: None,
            pass_ok: true,
            verify_timeout: None,
            force: false,
        };
        cmd_quick(&mana_dir, args1).unwrap();

        // Create second unit
        let args2 = QuickArgs {
            title: "Second".to_string(),
            handle: None,
            description: None,
            acceptance: None,
            notes: None,
            verify: Some("false".to_string()),
            priority: None,
            by: None,
            produces: None,
            requires: None,
            parent: None,
            on_fail: None,
            pass_ok: true,
            verify_timeout: None,
            force: false,
        };
        cmd_quick(&mana_dir, args2).unwrap();

        // Verify both exist with correct IDs
        let unit1 = Unit::from_file(mana_dir.join("1-first.md")).unwrap();
        let unit2 = Unit::from_file(mana_dir.join("2-second.md")).unwrap();
        assert_eq!(unit1.id, "1");
        assert_eq!(unit2.id, "2");
    }

    #[test]
    fn quick_updates_index() {
        let (_dir, mana_dir) = setup_mana_dir_with_config();

        let args = QuickArgs {
            title: "Indexed unit".to_string(),
            handle: None,
            description: None,
            acceptance: Some("Indexed correctly".to_string()),
            notes: None,
            verify: None,
            priority: None,
            by: Some("tester".to_string()),
            produces: None,
            requires: None,
            parent: None,
            on_fail: None,
            pass_ok: true,
            verify_timeout: None,
            force: false,
        };

        cmd_quick(&mana_dir, args).unwrap();

        // Load and check index
        let index = Index::load(&mana_dir).unwrap();
        assert_eq!(index.units.len(), 1);
        assert_eq!(index.units[0].id, "1");
        assert_eq!(index.units[0].title, "Indexed unit");
        assert_eq!(index.units[0].status, Status::InProgress);
    }

    #[test]
    fn quick_with_all_fields() {
        let (_dir, mana_dir) = setup_mana_dir_with_config();

        let args = QuickArgs {
            title: "Full unit".to_string(),
            handle: None,
            description: Some("A description".to_string()),
            acceptance: Some("All tests pass".to_string()),
            notes: Some("Some notes".to_string()),
            verify: Some("cargo test unit::check".to_string()),
            priority: Some(1),
            by: Some("agent-x".to_string()),
            produces: Some("FooStruct,bar_function".to_string()),
            requires: Some("BazTrait".to_string()),
            parent: None,
            on_fail: None,
            pass_ok: true,
            verify_timeout: None,
            force: false,
        };

        cmd_quick(&mana_dir, args).unwrap();

        let unit = Unit::from_file(mana_dir.join("1-full-unit.md")).unwrap();
        assert_eq!(unit.title, "Full unit");
        assert_eq!(unit.description, Some("A description".to_string()));
        assert_eq!(unit.acceptance, Some("All tests pass".to_string()));
        assert_eq!(unit.notes, Some("Some notes".to_string()));
        assert_eq!(unit.verify, Some("cargo test unit::check".to_string()));
        assert_eq!(unit.priority, 1);
        assert_eq!(unit.status, Status::InProgress);
        assert_eq!(unit.claimed_by, Some("agent-x".to_string()));
    }

    #[test]
    fn default_rejects_passing_verify() {
        let (_dir, mana_dir) = setup_mana_dir_with_config();

        let args = QuickArgs {
            title: "Cheating test".to_string(),
            handle: None,
            description: None,
            acceptance: None,
            notes: None,
            verify: Some("grep -q 'project: test' .mana/config.yaml".to_string()),
            priority: None,
            by: None,
            produces: None,
            requires: None,
            parent: None,
            on_fail: None,
            pass_ok: false, // default: fail-first enforced
            verify_timeout: None,
            force: false,
        };

        let result = cmd_quick(&mana_dir, args);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("verify command already passes"));
    }

    #[test]
    fn default_accepts_failing_verify() {
        let (_dir, mana_dir) = setup_mana_dir_with_config();

        let args = QuickArgs {
            title: "Real test".to_string(),
            handle: None,
            description: None,
            acceptance: None,
            notes: None,
            verify: Some("false".to_string()), // always fails
            priority: None,
            by: None,
            produces: None,
            requires: None,
            parent: None,
            on_fail: None,
            pass_ok: false, // default: fail-first enforced
            verify_timeout: None,
            force: false,
        };

        let result = cmd_quick(&mana_dir, args);
        assert!(result.is_ok());

        // Unit should be created
        let unit_path = mana_dir.join("1-real-test.md");
        assert!(unit_path.exists());

        // Should have fail_first set in the unit
        let unit = Unit::from_file(&unit_path).unwrap();
        assert!(unit.fail_first);
    }

    #[test]
    fn pass_ok_skips_fail_first_check() {
        let (_dir, mana_dir) = setup_mana_dir_with_config();

        let args = QuickArgs {
            title: "Passing verify ok".to_string(),
            handle: None,
            description: None,
            acceptance: None,
            notes: None,
            verify: Some("grep -q 'project: test' .mana/config.yaml".to_string()),
            priority: None,
            by: None,
            produces: None,
            requires: None,
            parent: None,
            on_fail: None,
            pass_ok: true,
            verify_timeout: None,
            force: false,
        };

        let result = cmd_quick(&mana_dir, args);
        assert!(result.is_ok());

        // Unit should be created
        let unit_path = mana_dir.join("1-passing-verify-ok.md");
        assert!(unit_path.exists());

        // Should NOT have fail_first set
        let unit = Unit::from_file(&unit_path).unwrap();
        assert!(!unit.fail_first);
    }

    #[test]
    fn no_verify_skips_fail_first_check() {
        let (_dir, mana_dir) = setup_mana_dir_with_config();

        let args = QuickArgs {
            title: "No verify".to_string(),
            handle: None,
            description: None,
            acceptance: Some("Done".to_string()),
            notes: None,
            verify: None, // no verify command — fail-first not applicable
            priority: None,
            by: None,
            produces: None,
            requires: None,
            parent: None,
            on_fail: None,
            pass_ok: false,
            verify_timeout: None,
            force: false,
        };

        let result = cmd_quick(&mana_dir, args);
        assert!(result.is_ok());

        // Should NOT have fail_first set (no verify)
        let unit_path = mana_dir.join("1-no-verify.md");
        let unit = Unit::from_file(&unit_path).unwrap();
        assert!(!unit.fail_first);
    }

    mod lint {
        use super::*;

        #[test]
        fn quick_verify_lint_rejects_errors_without_force() {
            let (_dir, mana_dir) = setup_mana_dir_with_config();

            let args = QuickArgs {
                title: "Quick lint error".to_string(),
                handle: None,
                description: None,
                acceptance: None,
                notes: None,
                verify: Some("true".to_string()),
                priority: None,
                by: None,
                produces: None,
                requires: None,
                parent: None,
                on_fail: None,
                pass_ok: true,
                verify_timeout: None,
                force: false,
            };

            let result = cmd_quick(&mana_dir, args);
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("lint error"));
        }

        #[test]
        fn quick_verify_lint_allows_errors_with_force() {
            let (_dir, mana_dir) = setup_mana_dir_with_config();

            let args = QuickArgs {
                title: "Forced quick lint error".to_string(),
                handle: None,
                description: None,
                acceptance: None,
                notes: None,
                verify: Some("echo done".to_string()),
                priority: None,
                by: None,
                produces: None,
                requires: None,
                parent: None,
                on_fail: None,
                pass_ok: true,
                verify_timeout: None,
                force: true,
            };

            let result = cmd_quick(&mana_dir, args);
            assert!(result.is_ok());
        }
    }
}
