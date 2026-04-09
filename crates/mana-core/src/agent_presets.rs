//! Agent presets and detection for known coding-agent CLIs.
//!
//! Provides built-in presets (pi, claude, aider) with compatibility run/plan
//! templates, and runtime detection of which agents are available on PATH.
//! During migration, bias these examples toward `imp run {id}` as the preferred
//! execution framing instead of mana-centered close language.

use std::process::Command;

/// A known agent preset with command templates.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentPreset {
    /// Agent name (e.g. "pi", "claude", "aider").
    pub name: &'static str,
    /// Template for running/implementing a unit. Contains `{id}` placeholder.
    /// Bias this toward `imp run {id}` or equivalent compatibility phrasing.
    pub run_template: &'static str,
    /// Template for planning/splitting a unit. Contains `{id}` placeholder.
    pub plan_template: &'static str,
    /// Command to check the agent version (e.g. `pi --version`).
    pub version_cmd: &'static str,
}

/// An agent detected on the current system.
#[derive(Debug, Clone, PartialEq)]
pub struct DetectedAgent {
    /// Agent name.
    pub name: String,
    /// Absolute path to the binary (from `which`).
    pub path: String,
    /// Version string, if obtainable.
    pub version: Option<String>,
}

// ---------------------------------------------------------------------------
// Built-in presets
// ---------------------------------------------------------------------------

const PRESETS: &[AgentPreset] = &[
    AgentPreset {
        name: "pi",
        run_template: "pi @.mana/{id}-*.md \"implement; hand completion back through the configured runtime/close path for unit {id}\"",
        plan_template: "pi @.mana/{id}-*.md \"plan into children with mana create --parent {id}\"",
        version_cmd: "pi --version",
    },
    AgentPreset {
        name: "claude",
        run_template: "imp run {id}",
        plan_template: "claude -p \"unit {id} is too large, split with mana create --parent {id}\"",
        version_cmd: "claude --version",
    },
    AgentPreset {
        name: "aider",
        run_template: "imp run {id}",
        plan_template: "aider --message \"plan unit {id} into children with mana create\"",
        version_cmd: "aider --version",
    },
];

/// Return all built-in agent presets.
#[must_use]
pub fn all_presets() -> &'static [AgentPreset] {
    PRESETS
}

/// Look up a preset by name (case-insensitive).
#[must_use]
pub fn get_preset(name: &str) -> Option<&'static AgentPreset> {
    let lower = name.to_ascii_lowercase();
    PRESETS.iter().find(|p| p.name == lower)
}

/// Scan PATH for known agent CLIs and return those that are available.
///
/// For each preset, runs `which <name>` to find the binary and then
/// attempts `<name> --version` to capture the version string.
pub fn detect_agents() -> Vec<DetectedAgent> {
    PRESETS
        .iter()
        .filter_map(|preset| {
            let path = which_binary(preset.name)?;
            let version = probe_version(preset.version_cmd);
            Some(DetectedAgent {
                name: preset.name.to_string(),
                path,
                version,
            })
        })
        .collect()
}

impl AgentPreset {
    /// Expand the run template, replacing `{id}` with the given unit id.
    #[must_use]
    pub fn run_cmd(&self, id: &str) -> String {
        self.run_template.replace("{id}", id)
    }

    /// Expand the plan template, replacing `{id}` with the given unit id.
    #[must_use]
    pub fn plan_cmd(&self, id: &str) -> String {
        self.plan_template.replace("{id}", id)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Use `which` to resolve a binary name to an absolute path.
fn which_binary(name: &str) -> Option<String> {
    let output = Command::new("which").arg(name).output().ok()?;
    if output.status.success() {
        let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    } else {
        None
    }
}

/// Run a version command and capture its first line of output.
fn probe_version(version_cmd: &str) -> Option<String> {
    let parts: Vec<&str> = version_cmd.split_whitespace().collect();
    let (bin, args) = parts.split_first()?;
    let output = Command::new(bin).args(args).output().ok()?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let first_line = stdout.lines().next()?.trim().to_string();
        if first_line.is_empty() {
            None
        } else {
            Some(first_line)
        }
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_presets_returns_at_least_three() {
        let presets = all_presets();
        assert!(
            presets.len() >= 3,
            "expected at least 3 presets, got {}",
            presets.len()
        );
    }

    #[test]
    fn get_preset_pi_returns_correct_templates() {
        let preset = get_preset("pi").expect("pi preset should exist");
        assert_eq!(preset.name, "pi");
        assert!(preset.run_template.contains("{id}"));
        assert!(preset.plan_template.contains("{id}"));
        assert_eq!(preset.version_cmd, "pi --version");
    }

    #[test]
    fn get_preset_claude() {
        let preset = get_preset("claude").expect("claude preset should exist");
        assert_eq!(preset.name, "claude");
        assert!(preset.run_template.contains("{id}"));
        assert!(preset.plan_template.contains("{id}"));
    }

    #[test]
    fn get_preset_aider() {
        let preset = get_preset("aider").expect("aider preset should exist");
        assert_eq!(preset.name, "aider");
        assert!(preset.run_template.contains("{id}"));
        assert!(preset.plan_template.contains("{id}"));
    }

    #[test]
    fn get_preset_nonexistent_returns_none() {
        assert!(get_preset("nonexistent").is_none());
    }

    #[test]
    fn get_preset_is_case_insensitive() {
        assert!(get_preset("Pi").is_some());
        assert!(get_preset("CLAUDE").is_some());
    }

    #[test]
    fn all_templates_contain_id_placeholder() {
        for preset in all_presets() {
            assert!(
                preset.run_template.contains("{id}"),
                "{} run_template missing {{id}}",
                preset.name
            );
            assert!(
                preset.plan_template.contains("{id}"),
                "{} plan_template missing {{id}}",
                preset.name
            );
        }
    }

    #[test]
    fn run_cmd_expands_id() {
        let preset = get_preset("pi").unwrap();
        let cmd = preset.run_cmd("42");
        assert!(cmd.contains("42"));
        assert!(!cmd.contains("{id}"));
    }

    #[test]
    fn plan_cmd_expands_id() {
        let preset = get_preset("claude").unwrap();
        let cmd = preset.plan_cmd("7.1");
        assert!(cmd.contains("7.1"));
        assert!(!cmd.contains("{id}"));
    }

    #[test]
    fn detect_agents_returns_vec() {
        // Smoke test — just ensure it doesn't panic. Actual results
        // depend on what's installed on the machine.
        let agents = detect_agents();
        for agent in &agents {
            assert!(!agent.name.is_empty());
            assert!(!agent.path.is_empty());
        }
    }
}
