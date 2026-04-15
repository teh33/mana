use std::fs;
use std::path::Path;

use anyhow::{anyhow, Result};
use serde_yml::Value;

use mana_core::yaml;

use crate::config::{Config, GlobalConfig, DEFAULT_COMMIT_TEMPLATE};

const CONFIG_KEY_HELP: &str = "Available keys: project, next_id, auto_close_parent, run, plan, research, run_model, plan_model, review_model, research_model, max_loops, max_concurrent, poll_interval, rules_file, file_locking, worktree, auto_commit, batch_verify, verify_timeout, commit_template, on_close, on_fail, memory_reserve_mb, user, user.email";
const MODEL_KEY_HELP: &str = "Model keys: run_model = legacy mana run compatibility, plan_model = mana plan, review_model = AI review, research_model = project research/planning";
const INSPECT_KEYS: &[&str] = &[
    "run",
    "run_model",
    "plan",
    "plan_model",
    "review_model",
    "research_model",
    "max_loops",
    "max_concurrent",
    "poll_interval",
    "file_locking",
    "worktree",
    "auto_commit",
    "batch_verify",
    "verify_timeout",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigValueSource {
    Project,
    Global,
    Default,
}

impl ConfigValueSource {
    fn label(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Global => "global",
            Self::Default => "default",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DoctorFinding {
    pub(crate) summary: String,
    pub(crate) details: String,
}

/// Get the effective configuration value by key.
pub fn cmd_config_get(mana_dir: &Path, key: &str) -> Result<()> {
    let config = Config::load_with_extends(mana_dir)?;
    println!("{}", resolved_config_value(&config, key)?);
    Ok(())
}

/// Get the raw project-local configuration value by key.
pub fn cmd_config_get_project(mana_dir: &Path, key: &str) -> Result<()> {
    validate_config_key(key)?;
    let value = load_raw_yaml(&mana_dir.join("config.yaml"))
        .and_then(|raw| yaml_lookup_string(&raw, key))
        .unwrap_or_default();
    println!("{}", value);
    Ok(())
}

/// Get the raw global configuration value by key.
pub fn cmd_config_get_global(_mana_dir: &Path, key: &str) -> Result<()> {
    validate_global_config_key(key)?;
    let value = GlobalConfig::path()
        .ok()
        .and_then(|path| load_raw_yaml(&path))
        .and_then(|raw| yaml_lookup_string(&raw, key))
        .unwrap_or_default();
    println!("{}", value);
    Ok(())
}

/// Inspect configuration values and where they come from.
pub fn cmd_config_inspect(mana_dir: &Path, key: Option<&str>) -> Result<()> {
    let effective = Config::load_with_extends(mana_dir)?;
    let local_raw = load_raw_yaml(&mana_dir.join("config.yaml"));
    let global_path = GlobalConfig::path().ok();
    let global_raw = global_path.as_ref().and_then(|path| load_raw_yaml(path));

    if let Some(key) = key {
        print_inspected_key(key, &effective, local_raw.as_ref(), global_raw.as_ref())?;
        return Ok(());
    }

    println!("Effective config:\n");
    for key in INSPECT_KEYS {
        let value = resolved_config_value(&effective, key)?;
        let source = config_value_source(key, local_raw.as_ref(), global_raw.as_ref());
        println!(
            "  {:<16} {:<40} [{}]",
            key,
            display_inline(&value),
            source.label()
        );
    }

    println!();
    println!("Use `mana config inspect <key>` for detailed local/global/effective values.");
    Ok(())
}

/// Detect stale or misleading project-local config.
pub fn cmd_config_doctor(mana_dir: &Path) -> Result<()> {
    let findings = collect_doctor_findings(mana_dir)?;
    if findings.is_empty() {
        println!("Config looks clean — no stale project-local overrides detected.");
        return Ok(());
    }

    println!("Stale or risky config findings:\n");
    for finding in findings {
        println!("- {}", finding.summary);
        println!("  {}", finding.details);
    }
    Ok(())
}

/// Set a configuration value by key in the project-local config.
pub fn cmd_config_set(mana_dir: &Path, key: &str, value: &str) -> Result<()> {
    cmd_config_set_project(mana_dir, key, value)
}

/// Set a configuration value by key in the project-local config.
pub fn cmd_config_set_project(mana_dir: &Path, key: &str, value: &str) -> Result<()> {
    let mut config = Config::load(mana_dir)?;
    apply_project_config_value(&mut config, key, value)?;
    config.save(mana_dir)?;
    println!("Set project {} = {}", key, value);
    if let Some(scope) = model_key_scope(key) {
        println!("Applies to: {}", scope);
    }
    Ok(())
}

/// Set a configuration value by key in the global config.
pub fn cmd_config_set_global(_mana_dir: &Path, key: &str, value: &str) -> Result<()> {
    let mut config = GlobalConfig::load()?;
    apply_global_config_value(&mut config, key, value)?;
    config.save()?;
    println!("Set global {} = {}", key, value);
    if let Some(scope) = model_key_scope(key) {
        println!("Applies to: {}", scope);
    }
    Ok(())
}

fn validate_config_key(key: &str) -> Result<()> {
    resolved_key_kind(key).map(|_| ())
}

fn validate_global_config_key(key: &str) -> Result<()> {
    match resolved_key_kind(key)? {
        ConfigKeyKind::ProjectOnly => Err(anyhow!(
            "`{}` is project-only and cannot be stored in ~/.config/mana/config.yaml",
            key
        )),
        ConfigKeyKind::Shared => Ok(()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigKeyKind {
    ProjectOnly,
    Shared,
}

fn resolved_key_kind(key: &str) -> Result<ConfigKeyKind> {
    match key {
        "project" | "next_id" => Ok(ConfigKeyKind::ProjectOnly),
        "auto_close_parent" | "run" | "plan" | "research" | "run_model" | "plan_model"
        | "review_model" | "research_model" | "max_loops" | "max_concurrent" | "poll_interval"
        | "rules_file" | "file_locking" | "worktree" | "auto_commit" | "batch_verify"
        | "verify_timeout" | "commit_template" | "on_close" | "on_fail" | "memory_reserve_mb"
        | "user" | "user.email" => Ok(ConfigKeyKind::Shared),
        _ => Err(anyhow!(
            "Unknown config key: {}\n{}\n{}",
            key,
            CONFIG_KEY_HELP,
            MODEL_KEY_HELP
        )),
    }
}

fn is_unset_value(value: &str) -> bool {
    value.is_empty() || value == "none" || value == "unset"
}

fn apply_project_config_value(config: &mut Config, key: &str, value: &str) -> Result<()> {
    match key {
        "project" => config.project = value.to_string(),
        "next_id" => {
            config.next_id = value
                .parse()
                .map_err(|_| anyhow!("Invalid value for next_id: {}", value))?;
        }
        "auto_close_parent" => {
            config.auto_close_parent = value.parse().map_err(|_| {
                anyhow!(
                    "Invalid value for auto_close_parent: {} (expected true/false)",
                    value
                )
            })?;
        }
        "run" => config.run = (!is_unset_value(value)).then(|| value.to_string()),
        "plan" => config.plan = (!is_unset_value(value)).then(|| value.to_string()),
        "research" => config.research = (!is_unset_value(value)).then(|| value.to_string()),
        "run_model" => config.run_model = (!is_unset_value(value)).then(|| value.to_string()),
        "plan_model" => config.plan_model = (!is_unset_value(value)).then(|| value.to_string()),
        "review_model" => config.review_model = (!is_unset_value(value)).then(|| value.to_string()),
        "research_model" => {
            config.research_model = (!is_unset_value(value)).then(|| value.to_string())
        }
        "max_loops" => {
            config.max_loops = value.parse().map_err(|_| {
                anyhow!(
                    "Invalid value for max_loops: {} (expected non-negative integer)",
                    value
                )
            })?;
        }
        "max_concurrent" => {
            config.max_concurrent = value.parse().map_err(|_| {
                anyhow!(
                    "Invalid value for max_concurrent: {} (expected positive integer)",
                    value
                )
            })?;
        }
        "poll_interval" => {
            config.poll_interval = value.parse().map_err(|_| {
                anyhow!(
                    "Invalid value for poll_interval: {} (expected positive integer)",
                    value
                )
            })?;
        }
        "rules_file" => config.rules_file = (!is_unset_value(value)).then(|| value.to_string()),
        "file_locking" => {
            config.file_locking = value.parse().map_err(|_| {
                anyhow!(
                    "Invalid value for file_locking: {} (expected true/false)",
                    value
                )
            })?;
        }
        "worktree" => {
            config.worktree = value.parse().map_err(|_| {
                anyhow!(
                    "Invalid value for worktree: {} (expected true/false)",
                    value
                )
            })?;
        }
        "auto_commit" => {
            config.auto_commit = value.parse().map_err(|_| {
                anyhow!(
                    "Invalid value for auto_commit: {} (expected true/false)",
                    value
                )
            })?;
        }
        "batch_verify" => {
            config.batch_verify = value.parse().map_err(|_| {
                anyhow!(
                    "Invalid value for batch_verify: {} (expected true/false)",
                    value
                )
            })?;
        }
        "verify_timeout" => {
            config.verify_timeout = if is_unset_value(value) {
                None
            } else {
                Some(value.parse().map_err(|_| {
                    anyhow!(
                        "Invalid value for verify_timeout: {} (expected integer seconds)",
                        value
                    )
                })?)
            };
        }
        "commit_template" => {
            config.commit_template = (!is_unset_value(value)).then(|| value.to_string())
        }
        "on_close" => config.on_close = (!is_unset_value(value)).then(|| value.to_string()),
        "on_fail" => config.on_fail = (!is_unset_value(value)).then(|| value.to_string()),
        "memory_reserve_mb" => {
            config.memory_reserve_mb = value.parse().map_err(|_| {
                anyhow!(
                    "Invalid value for memory_reserve_mb: {} (expected non-negative integer in MB)",
                    value
                )
            })?;
        }
        "user" => config.user = (!is_unset_value(value)).then(|| value.to_string()),
        "user.email" => config.user_email = (!is_unset_value(value)).then(|| value.to_string()),
        _ => return validate_config_key(key),
    }

    Ok(())
}

fn apply_global_config_value(config: &mut GlobalConfig, key: &str, value: &str) -> Result<()> {
    validate_global_config_key(key)?;
    match key {
        "auto_close_parent" => {
            config.auto_close_parent = if is_unset_value(value) {
                None
            } else {
                Some(value.parse().map_err(|_| {
                    anyhow!(
                        "Invalid value for auto_close_parent: {} (expected true/false)",
                        value
                    )
                })?)
            };
        }
        "run" => config.run = (!is_unset_value(value)).then(|| value.to_string()),
        "plan" => config.plan = (!is_unset_value(value)).then(|| value.to_string()),
        "research" => config.research = (!is_unset_value(value)).then(|| value.to_string()),
        "run_model" => config.run_model = (!is_unset_value(value)).then(|| value.to_string()),
        "plan_model" => config.plan_model = (!is_unset_value(value)).then(|| value.to_string()),
        "review_model" => config.review_model = (!is_unset_value(value)).then(|| value.to_string()),
        "research_model" => {
            config.research_model = (!is_unset_value(value)).then(|| value.to_string())
        }
        "max_loops" => {
            config.max_loops = if is_unset_value(value) {
                None
            } else {
                Some(value.parse().map_err(|_| {
                    anyhow!(
                        "Invalid value for max_loops: {} (expected non-negative integer)",
                        value
                    )
                })?)
            };
        }
        "max_concurrent" => {
            config.max_concurrent = if is_unset_value(value) {
                None
            } else {
                Some(value.parse().map_err(|_| {
                    anyhow!(
                        "Invalid value for max_concurrent: {} (expected positive integer)",
                        value
                    )
                })?)
            };
        }
        "poll_interval" => {
            config.poll_interval = if is_unset_value(value) {
                None
            } else {
                Some(value.parse().map_err(|_| {
                    anyhow!(
                        "Invalid value for poll_interval: {} (expected positive integer)",
                        value
                    )
                })?)
            };
        }
        "rules_file" => config.rules_file = (!is_unset_value(value)).then(|| value.to_string()),
        "file_locking" => {
            config.file_locking = if is_unset_value(value) {
                None
            } else {
                Some(value.parse().map_err(|_| {
                    anyhow!(
                        "Invalid value for file_locking: {} (expected true/false)",
                        value
                    )
                })?)
            };
        }
        "worktree" => {
            config.worktree = if is_unset_value(value) {
                None
            } else {
                Some(value.parse().map_err(|_| {
                    anyhow!(
                        "Invalid value for worktree: {} (expected true/false)",
                        value
                    )
                })?)
            };
        }
        "auto_commit" => {
            config.auto_commit = if is_unset_value(value) {
                None
            } else {
                Some(value.parse().map_err(|_| {
                    anyhow!(
                        "Invalid value for auto_commit: {} (expected true/false)",
                        value
                    )
                })?)
            };
        }
        "batch_verify" => {
            config.batch_verify = if is_unset_value(value) {
                None
            } else {
                Some(value.parse().map_err(|_| {
                    anyhow!(
                        "Invalid value for batch_verify: {} (expected true/false)",
                        value
                    )
                })?)
            };
        }
        "verify_timeout" => {
            config.verify_timeout = if is_unset_value(value) {
                None
            } else {
                Some(value.parse().map_err(|_| {
                    anyhow!(
                        "Invalid value for verify_timeout: {} (expected integer seconds)",
                        value
                    )
                })?)
            };
        }
        "commit_template" => {
            config.commit_template = (!is_unset_value(value)).then(|| value.to_string())
        }
        "on_close" => config.on_close = (!is_unset_value(value)).then(|| value.to_string()),
        "on_fail" => config.on_fail = (!is_unset_value(value)).then(|| value.to_string()),
        "memory_reserve_mb" => {
            config.memory_reserve_mb = if is_unset_value(value) {
                None
            } else {
                Some(value.parse().map_err(|_| {
                    anyhow!(
                        "Invalid value for memory_reserve_mb: {} (expected non-negative integer in MB)",
                        value
                    )
                })?)
            };
        }
        "user" => config.user = (!is_unset_value(value)).then(|| value.to_string()),
        "user.email" => config.user_email = (!is_unset_value(value)).then(|| value.to_string()),
        _ => unreachable!("validated global config key before applying"),
    }
    Ok(())
}

fn print_inspected_key(
    key: &str,
    effective: &Config,
    local_raw: Option<&Value>,
    global_raw: Option<&Value>,
) -> Result<()> {
    let effective_value = resolved_config_value(effective, key)?;
    let local_value = local_raw.and_then(|raw| yaml_lookup_string(raw, key));
    let global_value = global_raw.and_then(|raw| yaml_lookup_string(raw, key));
    let source = config_value_source(key, local_raw, global_raw);

    println!("Key: {}", key);
    println!("Effective: {}", display_multiline(&effective_value));
    println!("Source: {}", source.label());
    println!(
        "Project: {}",
        local_value
            .as_deref()
            .map(display_multiline)
            .unwrap_or_else(|| "(unset)".to_string())
    );
    println!(
        "Global: {}",
        global_value
            .as_deref()
            .map(display_multiline)
            .unwrap_or_else(|| "(unset)".to_string())
    );

    Ok(())
}

pub(crate) fn collect_doctor_findings(mana_dir: &Path) -> Result<Vec<DoctorFinding>> {
    let local_path = mana_dir.join("config.yaml");
    if !local_path.exists() {
        return Ok(Vec::new());
    }

    let effective = Config::load_with_extends(mana_dir)?;
    let local_raw = load_raw_yaml(&local_path);
    let global_path = GlobalConfig::path().ok();
    let global_raw = global_path.as_ref().and_then(|path| load_raw_yaml(path));

    let mut findings = Vec::new();

    for key in INSPECT_KEYS {
        let local_value = local_raw
            .as_ref()
            .and_then(|raw| yaml_lookup_string(raw, key));
        let global_value = global_raw
            .as_ref()
            .and_then(|raw| yaml_lookup_string(raw, key));
        if let (Some(local_value), Some(global_value)) = (local_value, global_value) {
            if local_value == global_value {
                findings.push(DoctorFinding {
                    summary: format!("`{}` is redundantly set in project config", key),
                    details: format!(
                        "Project and global config both set `{}` to `{}`. Remove the project-local line to inherit the global default cleanly.",
                        key,
                        display_inline(&local_value)
                    ),
                });
            }
        }
    }

    if let Some(run) = effective.run.as_deref() {
        if is_legacy_run_template(run) {
            findings.push(DoctorFinding {
                summary: "Run template looks legacy or path-specific".to_string(),
                details: format!(
                    "Effective run template is `{}`. Prefer `imp --model {{model}} run {{id}}` so model overrides work and the config is machine-independent.",
                    run
                ),
            });
        }

        if effective.run_model.is_some() && !run.contains("{model}") {
            findings.push(DoctorFinding {
                summary: "run_model is configured but the run template ignores it".to_string(),
                details: format!(
                    "Effective run template is `{}` and does not include `{{model}}`, so `run_model` will be ignored. Use `imp --model {{model}} run {{id}}` or remove the custom run template.",
                    run
                ),
            });
        }
    }

    if let Some(plan) = effective.plan.as_deref() {
        if effective.plan_model.is_some() && !plan.contains("{model}") {
            findings.push(DoctorFinding {
                summary: "plan_model is configured but the plan template ignores it".to_string(),
                details: format!(
                    "Effective plan template is `{}` and does not include `{{model}}`, so `plan_model` will be ignored.",
                    plan
                ),
            });
        }
    }

    if local_raw.is_none() {
        findings.push(DoctorFinding {
            summary: "Project has no local config file".to_string(),
            details: "That is fine if you want to inherit everything globally; mana will fall back to ~/.config/mana/config.yaml and built-in defaults.".to_string(),
        });
    }

    Ok(findings)
}

fn resolved_config_value(config: &Config, key: &str) -> Result<String> {
    let value = match key {
        "project" => config.project.clone(),
        "next_id" => config.next_id.to_string(),
        "auto_close_parent" => config.auto_close_parent.to_string(),
        "run" => config.run.clone().unwrap_or_default(),
        "plan" => config.plan.clone().unwrap_or_default(),
        "research" => config.research.clone().unwrap_or_default(),
        "run_model" => config.run_model.clone().unwrap_or_default(),
        "plan_model" => config.plan_model.clone().unwrap_or_default(),
        "review_model" => config.review_model.clone().unwrap_or_default(),
        "research_model" => config.research_model.clone().unwrap_or_default(),
        "max_loops" => config.max_loops.to_string(),
        "max_concurrent" => config.max_concurrent.to_string(),
        "poll_interval" => config.poll_interval.to_string(),
        "rules_file" => config
            .rules_file
            .clone()
            .unwrap_or_else(|| "RULES.md".to_string()),
        "file_locking" => config.file_locking.to_string(),
        "worktree" => config.worktree.to_string(),
        "auto_commit" => config.auto_commit.to_string(),
        "batch_verify" => config.batch_verify.to_string(),
        "verify_timeout" => config
            .verify_timeout
            .map(|timeout| timeout.to_string())
            .unwrap_or_default(),
        "commit_template" => config
            .commit_template
            .clone()
            .unwrap_or_else(|| DEFAULT_COMMIT_TEMPLATE.to_string()),
        "on_close" => config.on_close.clone().unwrap_or_default(),
        "on_fail" => config.on_fail.clone().unwrap_or_default(),
        "memory_reserve_mb" => config.memory_reserve_mb.to_string(),
        "user" => config.user.clone().unwrap_or_default(),
        "user.email" => config.user_email.clone().unwrap_or_default(),
        _ => {
            return Err(anyhow!(
                "Unknown config key: {}\n{}\n{}",
                key,
                CONFIG_KEY_HELP,
                MODEL_KEY_HELP
            ))
        }
    };

    Ok(value)
}

fn config_value_source(
    key: &str,
    local_raw: Option<&Value>,
    global_raw: Option<&Value>,
) -> ConfigValueSource {
    if local_raw
        .and_then(|raw| yaml_lookup_string(raw, key))
        .is_some()
    {
        ConfigValueSource::Project
    } else if global_raw
        .and_then(|raw| yaml_lookup_string(raw, key))
        .is_some()
    {
        ConfigValueSource::Global
    } else {
        ConfigValueSource::Default
    }
}

fn load_raw_yaml(path: &Path) -> Option<Value> {
    let contents = fs::read_to_string(path).ok()?;
    yaml::from_str(&contents).ok()
}

fn yaml_lookup_string(raw: &Value, key: &str) -> Option<String> {
    let yaml_key = match key {
        "user.email" => "user_email",
        other => other,
    };
    let map = raw.as_mapping()?;
    let value = map.get(Value::String(yaml_key.to_string()))?;
    Some(yaml_scalar_to_string(value))
}

fn yaml_scalar_to_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(v) => v.to_string(),
        Value::Number(v) => v.to_string(),
        Value::String(v) => v.clone(),
        _ => serde_yml::to_string(value)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

fn display_inline(value: &str) -> String {
    if value.is_empty() {
        "(unset)".to_string()
    } else {
        value.replace('\n', " ")
    }
}

fn display_multiline(value: &str) -> String {
    if value.is_empty() {
        "(unset)".to_string()
    } else {
        value.to_string()
    }
}

fn is_legacy_run_template(run: &str) -> bool {
    run.contains("pi -p") || run.contains("../target/debug/imp")
}

fn model_key_scope(key: &str) -> Option<&'static str> {
    match key {
        "run_model" => Some("legacy mana run compatibility"),
        "plan_model" => Some("mana plan"),
        "review_model" => Some("AI review flows"),
        "research_model" => Some("project research/planning"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup_test_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.yaml"),
            "project: test\nnext_id: 1\nauto_close_parent: true\n",
        )
        .unwrap();
        dir
    }

    #[test]
    fn load_raw_yaml_returns_none_for_invalid_yaml_instead_of_panicking() {
        let dir = setup_test_dir();
        fs::write(dir.path().join("config.yaml"), "title: [unterminated").unwrap();
        assert!(load_raw_yaml(&dir.path().join("config.yaml")).is_none());
    }

    #[test]
    fn get_unknown_key_returns_error() {
        let dir = setup_test_dir();
        let result = cmd_config_get(dir.path(), "unknown_key");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unknown config key"));
    }

    #[test]
    fn set_unknown_key_returns_error() {
        let dir = setup_test_dir();
        let result = cmd_config_set(dir.path(), "unknown_key", "value");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unknown config key"));
    }

    #[test]
    fn get_run_returns_empty_when_unset() {
        let dir = setup_test_dir();
        let result = cmd_config_get(dir.path(), "run");
        assert!(result.is_ok());
    }

    #[test]
    fn set_run_stores_command_template() {
        let dir = setup_test_dir();
        cmd_config_set(dir.path(), "run", "claude -p 'implement unit {id}'").unwrap();

        let config = Config::load(dir.path()).unwrap();
        assert_eq!(
            config.run,
            Some("claude -p 'implement unit {id}'".to_string())
        );
    }

    #[test]
    fn set_run_to_none_clears_it() {
        let dir = setup_test_dir();
        cmd_config_set(dir.path(), "run", "some command").unwrap();
        cmd_config_set(dir.path(), "run", "none").unwrap();

        let config = Config::load(dir.path()).unwrap();
        assert_eq!(config.run, None);
    }

    #[test]
    fn set_run_to_empty_clears_it() {
        let dir = setup_test_dir();
        cmd_config_set(dir.path(), "run", "some command").unwrap();
        cmd_config_set(dir.path(), "run", "").unwrap();

        let config = Config::load(dir.path()).unwrap();
        assert_eq!(config.run, None);
    }

    #[test]
    fn set_auto_commit_persists_bool() {
        let dir = setup_test_dir();
        cmd_config_set(dir.path(), "auto_commit", "true").unwrap();

        let config = Config::load(dir.path()).unwrap();
        assert!(config.auto_commit);
    }

    #[test]
    fn yaml_lookup_supports_user_email_alias() {
        let raw: Value = serde_yml::from_str("user_email: test@example.com\n").unwrap();
        assert_eq!(
            yaml_lookup_string(&raw, "user.email"),
            Some("test@example.com".to_string())
        );
    }

    #[test]
    fn config_value_source_prefers_project_then_global() {
        let local: Value = serde_yml::from_str("run_model: gpt-5.4\n").unwrap();
        let global: Value = serde_yml::from_str("run_model: sonnet\n").unwrap();
        assert_eq!(
            config_value_source("run_model", Some(&local), Some(&global)),
            ConfigValueSource::Project
        );
        assert_eq!(
            config_value_source("plan_model", Some(&local), Some(&global)),
            ConfigValueSource::Default
        );
    }

    #[test]
    fn legacy_run_template_detection_flags_old_templates() {
        assert!(is_legacy_run_template("pi -p 'do thing'"));
        assert!(is_legacy_run_template("../target/debug/imp run {id}"));
        assert!(!is_legacy_run_template("imp --model {model} run {id}"));
    }

    #[test]
    fn apply_global_config_value_sets_and_clears_model() {
        let mut config = GlobalConfig::default();
        apply_global_config_value(&mut config, "run_model", "gpt-5.4").unwrap();
        assert_eq!(config.run_model, Some("gpt-5.4".to_string()));

        apply_global_config_value(&mut config, "run_model", "unset").unwrap();
        assert_eq!(config.run_model, None);
    }

    #[test]
    fn global_config_rejects_project_only_keys() {
        let err = validate_global_config_key("next_id")
            .unwrap_err()
            .to_string();
        assert!(err.contains("project-only"));
    }
}
