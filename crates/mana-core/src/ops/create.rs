use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

use crate::config::Config;
use crate::hooks::{execute_hook, HookEvent};
use crate::index::{Index, LockedIndex};
use crate::unit::{validate_priority, OnFailAction, Unit, UnitKind};
use crate::util::title_to_slug;
use crate::verify_lint::{lint_verify, VerifyLintLevel};

/// Parameters for creating a new unit.
#[derive(Default)]
pub struct CreateParams {
    pub title: String,
    pub description: Option<String>,
    pub acceptance: Option<String>,
    pub notes: Option<String>,
    pub design: Option<String>,
    pub verify: Option<String>,
    pub priority: Option<u8>,
    pub labels: Vec<String>,
    pub assignee: Option<String>,
    pub dependencies: Vec<String>,
    pub parent: Option<String>,
    pub produces: Vec<String>,
    pub requires: Vec<String>,
    pub paths: Vec<String>,
    pub on_fail: Option<OnFailAction>,
    pub fail_first: bool,
    pub feature: bool,
    pub kind: Option<UnitKind>,
    pub verify_timeout: Option<u64>,
    pub decisions: Vec<String>,
    /// Skip verify lint errors (allow anti-pattern verify commands)
    pub force: bool,
}

/// Result of creating a unit.
#[derive(serde::Serialize)]
pub struct CreateResult {
    pub unit: Unit,
    pub path: PathBuf,
}

/// Create a new unit and persist it to disk.
pub fn create(mana_dir: &Path, params: CreateParams) -> Result<CreateResult> {
    if let Some(priority) = params.priority {
        validate_priority(priority)?;
    }

    // Lint the verify command for anti-patterns
    if let Some(ref verify_cmd) = params.verify {
        let findings = lint_verify(verify_cmd);
        if !findings.is_empty() {
            let has_errors = findings.iter().any(|f| f.level == VerifyLintLevel::Error);

            // Print warnings to stderr regardless
            for finding in &findings {
                let prefix = match finding.level {
                    VerifyLintLevel::Error => "error",
                    VerifyLintLevel::Warning => "warning",
                };
                eprintln!("[verify-lint {}] {}", prefix, finding.message);
            }

            // Block on errors unless force is set
            if has_errors && !params.force {
                return Err(anyhow!(
                    "Verify command has lint errors. Use --force to override."
                ));
            }
        }
    }

    let mut config = Config::load(mana_dir)?;

    let unit_id = if let Some(ref parent_id) = params.parent {
        assign_child_id(mana_dir, parent_id)?
    } else {
        let id = config.increment_id();
        config.save(mana_dir)?;
        id.to_string()
    };

    let slug = title_to_slug(&params.title);
    let mut unit = Unit::new(&unit_id, &params.title);
    unit.slug = Some(slug.clone());
    unit.description = params.description;
    unit.acceptance = params.acceptance;
    unit.notes = params.notes;
    unit.design = params.design;
    unit.verify = params.verify;
    unit.fail_first = params.fail_first;
    unit.feature = params.feature;
    if let Some(kind) = params.kind {
        unit.kind = kind;
    }
    unit.verify_timeout = params.verify_timeout;
    unit.on_fail = params.on_fail;
    if let Some(priority) = params.priority {
        unit.priority = priority;
    }
    unit.assignee = params.assignee;
    unit.parent = params.parent;
    unit.labels = params.labels;
    unit.dependencies = params.dependencies;
    unit.produces = params.produces;
    unit.requires = params.requires;
    unit.paths = params.paths;
    unit.decisions = params.decisions;

    let project_dir = mana_dir
        .parent()
        .ok_or_else(|| anyhow!("Failed to determine project directory"))?;

    let pre_passed = execute_hook(HookEvent::PreCreate, &unit, project_dir, None)
        .context("Pre-create hook execution failed")?;
    if !pre_passed {
        return Err(anyhow!("Pre-create hook rejected unit creation"));
    }

    let unit_path = mana_dir.join(format!("{}-{}.md", unit_id, slug));
    unit.to_file(&unit_path)?;
    let mut locked = LockedIndex::acquire(mana_dir)?;
    locked.index = Index::build(mana_dir)?;
    locked.save_and_release()?;

    if let Err(e) = execute_hook(HookEvent::PostCreate, &unit, project_dir, None) {
        eprintln!("Warning: post-create hook failed: {}", e);
    }

    Ok(CreateResult {
        unit,
        path: unit_path,
    })
}

/// Assign a child ID for a parent unit.
pub fn assign_child_id(mana_dir: &Path, parent_id: &str) -> Result<String> {
    let mut max_child: u32 = 0;
    let dir_entries = fs::read_dir(mana_dir)
        .with_context(|| format!("Failed to read directory: {}", mana_dir.display()))?;
    for entry in dir_entries {
        let entry = entry?;
        let filename = entry
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if let Some(name) = filename.strip_suffix(".md") {
            if let Some(rest) = name.strip_prefix(parent_id) {
                if let Some(after_dot) = rest.strip_prefix('.') {
                    if let Ok(n) = after_dot
                        .split('-')
                        .next()
                        .unwrap_or_default()
                        .parse::<u32>()
                    {
                        max_child = max_child.max(n);
                    }
                }
            }
        }
        if let Some(name) = filename.strip_suffix(".yaml") {
            if let Some(rest) = name.strip_prefix(parent_id) {
                if let Some(after_dot) = rest.strip_prefix('.') {
                    if let Ok(n) = after_dot.parse::<u32>() {
                        max_child = max_child.max(n);
                    }
                }
            }
        }
    }
    Ok(format!("{}.{}", parent_id, max_child + 1))
}

/// Parse an on-fail string into an OnFailAction.
pub fn parse_on_fail(s: &str) -> Result<OnFailAction> {
    let (action, arg) = match s.split_once(':') {
        Some((a, b)) => (a, Some(b)),
        None => (s, None),
    };
    match action {
        "retry" => {
            let max = arg
                .map(|a| a.parse::<u32>())
                .transpose()
                .map_err(|_| anyhow!("Invalid retry max: \'{}\'", arg.unwrap_or("")))?;
            Ok(OnFailAction::Retry {
                max,
                delay_secs: None,
            })
        }
        "escalate" => {
            let priority = match arg {
                Some(a) => {
                    let stripped = a
                        .strip_prefix('P')
                        .or_else(|| a.strip_prefix('p'))
                        .unwrap_or(a);
                    let p = stripped
                        .parse::<u8>()
                        .map_err(|_| anyhow!("Invalid priority: \'{}\'", a))?;
                    validate_priority(p)?;
                    Some(p)
                }
                None => None,
            };
            Ok(OnFailAction::Escalate {
                priority,
                message: None,
            })
        }
        _ => Err(anyhow!("Unknown on-fail action: \'{}\'", action)),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_mana_dir() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();
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
        .save(&mana_dir)
        .unwrap();
        (dir, mana_dir)
    }

    pub fn minimal_params(title: &str) -> CreateParams {
        CreateParams {
            title: title.to_string(),
            description: None,
            acceptance: None,
            notes: None,
            design: None,
            verify: None,
            priority: None,
            labels: vec![],
            assignee: None,
            dependencies: vec![],
            parent: None,
            produces: vec![],
            requires: vec![],
            paths: vec![],
            on_fail: None,
            fail_first: false,
            feature: false,
            kind: None,
            verify_timeout: None,
            decisions: vec![],
            force: false,
        }
    }

    #[test]
    fn create_minimal() {
        let (_dir, bd) = setup_mana_dir();
        let r = create(&bd, minimal_params("First")).unwrap();
        assert_eq!(r.unit.id, "1");
        assert!(r.path.exists());
    }

    #[test]
    fn create_increments() {
        let (_dir, bd) = setup_mana_dir();
        assert_eq!(create(&bd, minimal_params("A")).unwrap().unit.id, "1");
        assert_eq!(create(&bd, minimal_params("B")).unwrap().unit.id, "2");
    }

    #[test]
    fn create_child() {
        let (_dir, bd) = setup_mana_dir();
        create(&bd, minimal_params("Parent")).unwrap();
        let mut p = minimal_params("Child");
        p.parent = Some("1".into());
        assert_eq!(create(&bd, p).unwrap().unit.id, "1.1");
    }

    #[test]
    fn create_rebuilds_index() {
        let (_dir, bd) = setup_mana_dir();
        create(&bd, minimal_params("Indexed")).unwrap();
        let index = Index::load(&bd).unwrap();
        assert_eq!(index.units[0].title, "Indexed");
    }
}
