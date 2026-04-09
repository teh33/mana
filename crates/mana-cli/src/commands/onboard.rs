use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const MARKER: &str = "# [mana-onboard]";

/// A detected agent and what file triggered the detection.
struct DetectedAgent {
    name: &'static str,
    #[allow(dead_code)]
    trigger_path: PathBuf,
}

/// Result of one onboarding action.
struct OnboardAction {
    file: PathBuf,
    description: String,
    skipped: bool,
}

/// Detect which coding agents are configured in `project_root`.
fn detect_agents(project_root: &Path) -> Vec<DetectedAgent> {
    let mut agents = Vec::new();

    // Claude Code
    let claude_settings = project_root.join(".claude/settings.json");
    let claude_md = project_root.join("CLAUDE.md");
    if claude_settings.exists() || claude_md.exists() {
        let trigger = if claude_settings.exists() {
            claude_settings
        } else {
            claude_md
        };
        agents.push(DetectedAgent {
            name: "Claude Code",
            trigger_path: trigger,
        });
    }

    // pi
    if project_root.join(".pi").exists() {
        agents.push(DetectedAgent {
            name: "pi",
            trigger_path: project_root.join(".pi"),
        });
    }

    // Cursor
    let cursor_rules = project_root.join(".cursor/rules");
    let cursor_rules_file = project_root.join(".cursorrules");
    if cursor_rules.exists() || cursor_rules_file.exists() {
        let trigger = if cursor_rules.exists() {
            cursor_rules
        } else {
            cursor_rules_file
        };
        agents.push(DetectedAgent {
            name: "Cursor",
            trigger_path: trigger,
        });
    }

    // AGENTS.md (generic — Codex, Gemini CLI, etc.)
    let agents_md = project_root.join("AGENTS.md");
    if agents_md.exists() {
        // Avoid double-counting if Claude Code was already detected via CLAUDE.md
        let already = agents.iter().any(|a| a.name == "Claude Code");
        if !already {
            agents.push(DetectedAgent {
                name: "AGENTS.md",
                trigger_path: agents_md,
            });
        } else {
            // Still want to configure AGENTS.md — add it as separate entry
            agents.push(DetectedAgent {
                name: "AGENTS.md",
                trigger_path: agents_md,
            });
        }
    }

    // Cline / OpenCode / Aider — lightweight detection
    if project_root.join(".cline").exists() || project_root.join("cline_docs").exists() {
        let trigger = if project_root.join(".cline").exists() {
            project_root.join(".cline")
        } else {
            project_root.join("cline_docs")
        };
        agents.push(DetectedAgent {
            name: "Cline",
            trigger_path: trigger,
        });
    }

    if project_root.join("opencode.yaml").exists() || project_root.join(".opencode").exists() {
        let trigger = if project_root.join("opencode.yaml").exists() {
            project_root.join("opencode.yaml")
        } else {
            project_root.join(".opencode")
        };
        agents.push(DetectedAgent {
            name: "OpenCode",
            trigger_path: trigger,
        });
    }

    if project_root.join(".aider.conf.yml").exists() {
        agents.push(DetectedAgent {
            name: "Aider",
            trigger_path: project_root.join(".aider.conf.yml"),
        });
    }

    agents
}

/// Return true if `content` already contains our marker.
fn already_onboarded(content: &str) -> bool {
    content.contains(MARKER)
}

/// Append text to a file (or create it), preceded by a blank line.
fn append_to_file(path: &Path, text: &str) -> Result<()> {
    let existing = if path.exists() {
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
    } else {
        String::new()
    };

    let separator = if existing.is_empty() || existing.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };

    let mut content = existing;
    content.push_str(separator);
    content.push_str(text);
    if !content.ends_with('\n') {
        content.push('\n');
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating directory {}", parent.display()))?;
    }

    fs::write(path, content).with_context(|| format!("writing {}", path.display()))
}

/// Write a new file (creates parent dirs). Returns false if already exists with marker.
fn write_new_file(path: &Path, content: &str) -> Result<bool> {
    if path.exists() {
        let existing =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        if already_onboarded(&existing) {
            return Ok(false); // skip
        }
        // File exists but no marker — append
        append_to_file(path, content)?;
        return Ok(true);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating directory {}", parent.display()))?;
    }

    let mut full = content.to_string();
    if !full.ends_with('\n') {
        full.push('\n');
    }
    fs::write(path, &full).with_context(|| format!("writing {}", path.display()))?;
    Ok(true)
}

// ── Per-agent onboarding logic ─────────────────────────────────────────────

fn onboard_claude_code(project_root: &Path, actions: &mut Vec<OnboardAction>) -> Result<()> {
    // 1. CLAUDE.md — append workflow section
    let claude_md = project_root.join("CLAUDE.md");
    let claude_content = if claude_md.exists() {
        fs::read_to_string(&claude_md).unwrap_or_default()
    } else {
        String::new()
    };

    if already_onboarded(&claude_content) {
        actions.push(OnboardAction {
            file: claude_md,
            description: "already configured".into(),
            skipped: true,
        });
    } else {
        let snippet = format!(
            r#"{MARKER}

## mana — task coordination

This project uses `mana` for task tracking.

- `mana status` — see what's in flight
- `mana context <id>` — load full context for a unit
- `mana close <id>` — close a unit (runs verify gate first)
- When dispatched via a runtime path (`imp run <id>` preferred, `mana run` legacy compatibility), read `mana show <id>` for full instructions.
"#
        );
        let created = !claude_md.exists();
        append_to_file(&claude_md, &snippet)?;
        actions.push(OnboardAction {
            file: claude_md,
            description: if created {
                "created with mana workflow".into()
            } else {
                "appended mana workflow".into()
            },
            skipped: false,
        });
    }

    // 2. .claude/settings.json — add SessionStart hook for `mana context`
    let settings_path = project_root.join(".claude/settings.json");
    if settings_path.exists() {
        let raw = fs::read_to_string(&settings_path)
            .with_context(|| format!("reading {}", settings_path.display()))?;

        if raw.contains("mana-onboard") {
            actions.push(OnboardAction {
                file: settings_path,
                description: "hooks already present".into(),
                skipped: true,
            });
        } else {
            // Parse and inject hooks via string manipulation to preserve formatting
            // We look for existing "hooks" key or inject one before the closing brace
            let hook_entry = r#""mana-onboard-session": {
        "type": "command",
        "command": "mana status 2>/dev/null || true"
      }"#;

            let updated = if raw.contains("\"hooks\"") {
                // Find first hook array/object and append — best-effort
                raw.clone()
            } else {
                // Inject a minimal hooks block before the final closing brace
                let trimmed = raw.trim_end_matches(|c: char| c.is_whitespace() || c == '}');
                let needs_comma = !trimmed.trim_end().ends_with('{');
                format!(
                    "{}{}\n  \"hooks\": {{\n    {}\n  }}\n}}",
                    trimmed,
                    if needs_comma { "," } else { "" },
                    hook_entry
                )
            };

            if updated != raw {
                fs::write(&settings_path, updated)
                    .with_context(|| format!("writing {}", settings_path.display()))?;
                actions.push(OnboardAction {
                    file: settings_path,
                    description: "added mana status hook".into(),
                    skipped: false,
                });
            } else {
                actions.push(OnboardAction {
                    file: settings_path,
                    description: "hooks block already exists — skipped".into(),
                    skipped: true,
                });
            }
        }
    }

    Ok(())
}

fn onboard_pi(project_root: &Path, actions: &mut Vec<OnboardAction>) -> Result<()> {
    let skill_path = project_root.join(".pi/agent/skills/mana/SKILL.md");
    let content = format!(
        r#"{MARKER}
# mana — task coordination

This project uses `mana` for task tracking and agent/runtime coordination.

## Key commands

```
mana status                   # See claimed, ready, and blocked units
mana show <id>                # Full unit details (title, description, verify)
mana context <id>             # Context dump for an agent working on a unit
mana close <id>               # Close unit after verify gate passes
mana update <id> --note "..."  # Log progress or failures
```

## Working on a unit

When dispatched via a runtime path (`imp run <id>` preferred, `mana run` legacy compatibility):
1. Run `mana show <id>` to read the full unit spec
2. Run `mana context <id>` to load referenced files
3. Implement what the unit describes
4. Run `mana close <id>` — this runs the verify gate first
"#
    );

    let existed = skill_path.exists();
    let written = write_new_file(&skill_path, &content)?;
    actions.push(OnboardAction {
        file: skill_path,
        description: if !written {
            "already configured".into()
        } else if existed {
            "appended mana skill".into()
        } else {
            "created mana skill".into()
        },
        skipped: !written,
    });

    Ok(())
}

fn onboard_cursor(project_root: &Path, actions: &mut Vec<OnboardAction>) -> Result<()> {
    // Prefer .cursor/rules/ directory if it exists, otherwise use .cursorrules
    let rules_dir = project_root.join(".cursor/rules");
    let target = if rules_dir.exists() {
        rules_dir.join("mana.mdc")
    } else {
        project_root.join(".cursorrules")
    };

    let content = format!(
        r#"{MARKER}

## mana task coordination

This project uses `mana` for task tracking.

- Run `mana status` to see what's in flight before starting new work
- Run `mana show <id>` to read a unit's full instructions
- Run `mana context <id>` to load referenced files for a unit
- Run `mana close <id>` when done (runs verify gate automatically)
- Log progress with `mana update <id> --note "..."`
"#
    );

    let existing = if target.exists() {
        fs::read_to_string(&target).unwrap_or_default()
    } else {
        String::new()
    };

    if already_onboarded(&existing) {
        actions.push(OnboardAction {
            file: target,
            description: "already configured".into(),
            skipped: true,
        });
    } else {
        let created = !target.exists();
        append_to_file(&target, &content)?;
        actions.push(OnboardAction {
            file: target,
            description: if created {
                "created with mana workflow".into()
            } else {
                "appended mana workflow".into()
            },
            skipped: false,
        });
    }

    Ok(())
}

fn onboard_agents_md(project_root: &Path, actions: &mut Vec<OnboardAction>) -> Result<()> {
    let agents_md = project_root.join("AGENTS.md");
    let existing = if agents_md.exists() {
        fs::read_to_string(&agents_md).unwrap_or_default()
    } else {
        String::new()
    };

    if already_onboarded(&existing) {
        actions.push(OnboardAction {
            file: agents_md,
            description: "already configured".into(),
            skipped: true,
        });
        return Ok(());
    }

    let content = format!(
        r#"{MARKER}

## mana — task coordination

This project uses `mana` for task tracking and agent/runtime coordination.

Key commands:
- `mana status` — see claimed, ready, and blocked units
- `mana show <id>` — read a unit's full instructions
- `mana context <id>` — load referenced files for a unit
- `mana close <id>` — close unit after verify gate passes
- `mana update <id> --note "..."` — log progress or failures
"#
    );

    let created = !agents_md.exists();
    append_to_file(&agents_md, &content)?;
    actions.push(OnboardAction {
        file: agents_md,
        description: if created {
            "created with mana workflow".into()
        } else {
            "appended mana workflow".into()
        },
        skipped: false,
    });

    Ok(())
}

fn onboard_aider(project_root: &Path, actions: &mut Vec<OnboardAction>) -> Result<()> {
    let conf = project_root.join(".aider.conf.yml");
    let existing = fs::read_to_string(&conf).unwrap_or_default();

    if already_onboarded(&existing) {
        actions.push(OnboardAction {
            file: conf,
            description: "already configured".into(),
            skipped: true,
        });
        return Ok(());
    }

    let snippet = format!(
        r#"
{MARKER}
# mana task coordination: run `mana status` before starting, `mana close <id>` when done.
"#
    );
    append_to_file(&conf, &snippet)?;
    actions.push(OnboardAction {
        file: conf,
        description: "appended mana conventions".into(),
        skipped: false,
    });

    Ok(())
}

fn onboard_cline(project_root: &Path, actions: &mut Vec<OnboardAction>) -> Result<()> {
    let doc_path = if project_root.join(".cline").exists() {
        project_root.join(".cline/mana.md")
    } else {
        project_root.join("cline_docs/mana.md")
    };

    let content = format!(
        r#"{MARKER}
# mana — task coordination

This project uses `mana` for task tracking.

- `mana status` — see what's in flight
- `mana show <id>` — read a unit's full instructions
- `mana context <id>` — load referenced files
- `mana close <id>` — close unit after verify gate passes
"#
    );

    let written = write_new_file(&doc_path, &content)?;
    actions.push(OnboardAction {
        file: doc_path,
        description: if written {
            "created mana rules".into()
        } else {
            "already configured".into()
        },
        skipped: !written,
    });

    Ok(())
}

fn onboard_opencode(project_root: &Path, actions: &mut Vec<OnboardAction>) -> Result<()> {
    let rules_path = if project_root.join(".opencode").exists() {
        project_root.join(".opencode/mana.md")
    } else {
        // Append a comment to opencode.yaml
        let yaml = project_root.join("opencode.yaml");
        let existing = fs::read_to_string(&yaml).unwrap_or_default();
        if already_onboarded(&existing) {
            return Ok(());
        }
        let snippet = format!(
            "\n{MARKER}\n# mana: run `mana status` to see tasks, `mana close <id>` when done.\n"
        );
        append_to_file(&yaml, &snippet)?;
        return Ok(());
    };

    let content = format!(
        r#"{MARKER}
# mana — task coordination

- `mana status` — see what's in flight
- `mana show <id>` — unit instructions
- `mana close <id>` — close after verify passes
"#
    );

    let written = write_new_file(&rules_path, &content)?;
    actions.push(OnboardAction {
        file: rules_path,
        description: if written {
            "created mana rules".into()
        } else {
            "already configured".into()
        },
        skipped: !written,
    });

    Ok(())
}

/// No agents detected — offer to create AGENTS.md.
fn onboard_fallback(project_root: &Path, actions: &mut Vec<OnboardAction>) -> Result<()> {
    let agents_md = project_root.join("AGENTS.md");
    if agents_md.exists() {
        return Ok(()); // already handled
    }

    let content = format!(
        r#"{MARKER}
# Agent Instructions

This project uses `mana` for task tracking and agent/runtime coordination.

## mana workflow

- `mana status` — see claimed, ready, and blocked units
- `mana show <id>` — read a unit's full instructions
- `mana context <id>` — load referenced files for a unit
- `mana close <id>` — close unit after verify gate passes
- `mana update <id> --note "..."` — log progress or failures

## Working on a unit

1. `mana show <id>` — read the full spec
2. `mana context <id>` — load referenced files
3. Implement exactly what the unit describes
4. `mana close <id>` — verify gate runs automatically
"#
    );

    fs::write(&agents_md, &content).with_context(|| format!("writing {}", agents_md.display()))?;
    actions.push(OnboardAction {
        file: agents_md,
        description: "created AGENTS.md from scratch".into(),
        skipped: false,
    });

    Ok(())
}

/// Main entry point for `mana onboard`.
///
/// Scans the project root for known coding-agent config files and writes
/// the appropriate mana integration instructions into each one.
/// Uses marker comments to remain idempotent across multiple runs.
pub fn cmd_onboard(project_root: &Path) -> Result<()> {
    let agents = detect_agents(project_root);

    if agents.is_empty() {
        eprintln!("No coding agents detected. Creating AGENTS.md for generic agent support.");
        let mut actions = Vec::new();
        onboard_fallback(project_root, &mut actions)?;
        print_summary(&[], &actions);
        return Ok(());
    }

    let agent_names: Vec<&str> = agents.iter().map(|a| a.name).collect();
    let unique_names: Vec<&str> = {
        let mut seen = std::collections::HashSet::new();
        agent_names
            .iter()
            .filter(|&&n| seen.insert(n))
            .copied()
            .collect()
    };
    eprintln!("Detected: {}", unique_names.join(", "));

    let mut actions: Vec<OnboardAction> = Vec::new();
    let mut configured: Vec<&str> = Vec::new();

    for agent in &agents {
        match agent.name {
            "Claude Code" => {
                onboard_claude_code(project_root, &mut actions)?;
                configured.push("Claude Code");
            }
            "pi" => {
                onboard_pi(project_root, &mut actions)?;
                configured.push("pi");
            }
            "Cursor" => {
                onboard_cursor(project_root, &mut actions)?;
                configured.push("Cursor");
            }
            "AGENTS.md" => {
                onboard_agents_md(project_root, &mut actions)?;
                configured.push("AGENTS.md");
            }
            "Cline" => {
                onboard_cline(project_root, &mut actions)?;
                configured.push("Cline");
            }
            "OpenCode" => {
                onboard_opencode(project_root, &mut actions)?;
                configured.push("OpenCode");
            }
            "Aider" => {
                onboard_aider(project_root, &mut actions)?;
                configured.push("Aider");
            }
            _ => {}
        }
    }

    print_summary(&configured, &actions);
    Ok(())
}

fn print_summary(configured: &[&str], actions: &[OnboardAction]) {
    for action in actions {
        let check = if action.skipped { "–" } else { "✓" };
        eprintln!(
            "  {} {} — {}",
            check,
            action.file.display(),
            action.description
        );
    }

    let new_count = actions.iter().filter(|a| !a.skipped).count();
    let skipped_count = actions.iter().filter(|a| a.skipped).count();

    if configured.is_empty() {
        eprintln!("Done.");
    } else if new_count == 0 {
        eprintln!("Done. Already configured ({} skipped).", skipped_count);
    } else {
        eprintln!(
            "Done. {} agent{} configured{}.",
            new_count,
            if new_count == 1 { "" } else { "s" },
            if skipped_count > 0 {
                format!(" ({} already done)", skipped_count)
            } else {
                String::new()
            }
        );
    }
}
