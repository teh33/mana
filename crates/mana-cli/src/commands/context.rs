use std::path::Path;

use anyhow::{Context, Result};

use crate::ctx_assembler::assemble_context;
use crate::discovery::find_unit_file;
use crate::prompt::{build_agent_prompt, FileOverlap, PromptOptions};
use mana_core::ops::context::{assemble_agent_context, merge_paths, AgentContext, DepProvider};
use mana_core::unit::Unit;

// ─── Formatting helpers (CLI-only) ──────────────────────────────────────────

/// Format rules content with delimiters for agent context injection.
fn format_rules_section(rules: &str) -> String {
    format!(
        "═══ PROJECT RULES ═══════════════════════════════════════════\n\
         {}\n\
         ═════════════════════════════════════════════════════════════\n\n",
        rules.trim_end()
    )
}

/// Format attempt notes with delimiters.
fn format_attempt_notes_section(notes: &str) -> String {
    format!(
        "═══ Previous Attempts ════════════════════════════════════════\n\
         {}\n\
         ══════════════════════════════════════════════════════════════\n\n",
        notes.trim_end()
    )
}

/// Format the unit's core spec as the first section of the context output.
fn format_unit_spec_section(unit: &Unit) -> String {
    let mut s = String::new();
    s.push_str("═══ UNIT ════════════════════════════════════════════════════\n");
    s.push_str(&format!("ID: {}\n", unit.id));
    s.push_str(&format!("Title: {}\n", unit.title));
    s.push_str(&format!("Priority: P{}\n", unit.priority));
    s.push_str(&format!("Status: {}\n", unit.status));

    if let Some(ref verify) = unit.verify {
        s.push_str(&format!("Verify: {}\n", verify));
    }

    if !unit.produces.is_empty() {
        s.push_str(&format!("Produces: {}\n", unit.produces.join(", ")));
    }
    if !unit.requires.is_empty() {
        s.push_str(&format!("Requires: {}\n", unit.requires.join(", ")));
    }
    if !unit.dependencies.is_empty() {
        s.push_str(&format!("Dependencies: {}\n", unit.dependencies.join(", ")));
    }
    if let Some(ref parent) = unit.parent {
        s.push_str(&format!("Parent: {}\n", parent));
    }

    if !unit.decisions.is_empty() {
        s.push_str(&format!(
            "\n⚠ UNRESOLVED DECISIONS ({}):\n",
            unit.decisions.len()
        ));
        for (i, decision) in unit.decisions.iter().enumerate() {
            s.push_str(&format!("  {}: {}\n", i, decision));
        }
    }

    if let Some(ref desc) = unit.description {
        s.push_str(&format!("\n## Description\n{}\n", desc));
    }
    if let Some(ref acceptance) = unit.acceptance {
        s.push_str(&format!("\n## Acceptance Criteria\n{}\n", acceptance));
    }

    s.push_str("═════════════════════════════════════════════════════════════\n\n");
    s
}

/// Format dependency providers into a section for the context output.
fn format_dependency_section(providers: &[DepProvider]) -> Option<String> {
    if providers.is_empty() {
        return None;
    }

    let mut s = String::new();
    s.push_str("═══ DEPENDENCY CONTEXT ══════════════════════════════════════\n");

    for p in providers {
        s.push_str(&format!(
            "Unit {} ({}) produces `{}` [{}]\n",
            p.unit_id, p.unit_title, p.artifact, p.status
        ));
        if let Some(ref desc) = p.description {
            let preview: String = desc.chars().take(500).collect();
            s.push_str(&format!("{}\n", preview));
            if desc.len() > 500 {
                s.push_str("...\n");
            }
        }
        s.push('\n');
    }

    s.push_str("═════════════════════════════════════════════════════════════\n\n");
    Some(s)
}

/// Format multiple file structures into a single section.
fn format_structure_block(structures: &[(&str, String)]) -> Option<String> {
    if structures.is_empty() {
        return None;
    }

    let mut body = String::new();
    for (path, structure) in structures {
        body.push_str(&format!("### {}\n```\n{}\n```\n\n", path, structure));
    }

    Some(format!(
        "═══ File Structure ═══════════════════════════════════════════\n\
         {}\
         ══════════════════════════════════════════════════════════════\n\n",
        body
    ))
}

/// Format child job summaries into a compact context section.
fn format_child_summaries_section(
    children: &[mana_core::ops::context::ChildSummary],
) -> Option<String> {
    if children.is_empty() {
        return None;
    }

    let mut s = String::new();
    s.push_str("═══ Child Job Summaries ═════════════════════════════════════\n");
    for child in children {
        let mut line = format!("{} [{}] attempts={}", child.id, child.status, child.attempts);
        if let Some(outcome) = &child.recent_outcome {
            line.push_str(&format!(" recent={}", outcome));
        }
        line.push_str(&format!(": {}\n", child.title));
        s.push_str(&line);
        if let Some(summary) = &child.summary {
            s.push_str(&format!("  summary: {}\n", summary));
        }
        if let Some(follow_up) = &child.follow_up {
            s.push_str(&format!("  follow-up: {}\n", follow_up));
        }
    }
    s.push_str("═════════════════════════════════════════════════════════════\n\n");
    Some(s)
}

// ─── Command ─────────────────────────────────────────────────────────────────

/// Assemble complete agent context for a unit — the single source of truth.
pub fn cmd_context(
    mana_dir: &Path,
    id: &str,
    json: bool,
    structure_only: bool,
    agent_prompt: bool,
    instructions: Option<String>,
    overlaps_json: Option<String>,
) -> Result<()> {
    // --agent-prompt: output the full structured prompt that an agent sees during mana run
    if agent_prompt {
        let unit_path =
            find_unit_file(mana_dir, id).context(format!("Could not find unit with ID: {}", id))?;
        let unit = Unit::from_file(&unit_path).context(format!(
            "Failed to parse unit from: {}",
            unit_path.display()
        ))?;

        // Parse --overlaps JSON into FileOverlap structs
        let concurrent_overlaps = match overlaps_json {
            Some(ref s) => {
                let raw: Vec<serde_json::Value> =
                    serde_json::from_str(s).context("Failed to parse --overlaps JSON")?;
                let overlaps: Vec<FileOverlap> = raw
                    .into_iter()
                    .map(|v| FileOverlap {
                        unit_id: v["unit_id"].as_str().unwrap_or("").to_string(),
                        title: v["title"].as_str().unwrap_or("").to_string(),
                        shared_files: v["shared_files"]
                            .as_array()
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|f| f.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                    .collect();
                Some(overlaps)
            }
            None => None,
        };

        let options = PromptOptions {
            mana_dir: mana_dir.to_path_buf(),
            instructions,
            concurrent_overlaps,
        };
        let result = build_agent_prompt(&unit, &options)?;

        if json {
            let obj = serde_json::json!({
                "system_prompt": result.system_prompt,
                "user_message": result.user_message,
                "file_ref": result.file_ref,
            });
            println!("{}", serde_json::to_string(&obj)?);
        } else {
            println!("{}", result.system_prompt);
        }
        return Ok(());
    }

    // Delegate data assembly to core
    let ctx = assemble_agent_context(mana_dir, id)?;

    let project_dir = mana_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Invalid .mana/ path: {}", mana_dir.display()))?;

    if json {
        output_json(&ctx, structure_only)?;
    } else {
        output_text(&ctx, project_dir, structure_only)?;
    }

    Ok(())
}

fn output_json(ctx: &AgentContext, structure_only: bool) -> Result<()> {
    let files: Vec<serde_json::Value> = ctx
        .files
        .iter()
        .map(|entry| {
            let exists = entry.content.is_some();
            let mut file_obj = serde_json::json!({
                "path": entry.path,
                "exists": exists,
            });
            if !structure_only {
                file_obj["content"] = serde_json::Value::String(
                    entry
                        .content
                        .as_deref()
                        .unwrap_or("(not found)")
                        .to_string(),
                );
            }
            if let Some(ref s) = entry.structure {
                file_obj["structure"] = serde_json::Value::String(s.clone());
            }
            file_obj
        })
        .collect();

    let dep_json: Vec<serde_json::Value> = ctx
        .dep_providers
        .iter()
        .map(|p| {
            serde_json::json!({
                "artifact": p.artifact,
                "unit_id": p.unit_id,
                "title": p.unit_title,
                "status": p.status,
                "description": p.description,
            })
        })
        .collect();

    let unit = &ctx.unit;
    let mut obj = serde_json::json!({
        "id": unit.id,
        "title": unit.title,
        "priority": unit.priority,
        "status": format!("{}", unit.status),
        "verify": unit.verify,
        "description": unit.description,
        "acceptance": unit.acceptance,
        "produces": unit.produces,
        "requires": unit.requires,
        "dependencies": unit.dependencies,
        "parent": unit.parent,
        "files": files,
        "dependency_context": dep_json,
        "child_summaries": ctx.child_summaries,
    });
    if let Some(ref rules_content) = ctx.rules {
        obj["rules"] = serde_json::Value::String(rules_content.clone());
    }
    if let Some(ref notes) = ctx.attempt_notes {
        obj["attempt_notes"] = serde_json::Value::String(notes.clone());
    }
    println!("{}", serde_json::to_string_pretty(&obj)?);

    Ok(())
}

fn output_text(ctx: &AgentContext, project_dir: &Path, structure_only: bool) -> Result<()> {
    let mut output = String::new();

    // 1. Unit spec
    output.push_str(&format_unit_spec_section(&ctx.unit));

    // 2. Previous attempts
    if let Some(ref notes) = ctx.attempt_notes {
        output.push_str(&format_attempt_notes_section(notes));
    }

    // 3. Project rules
    if let Some(ref rules_content) = ctx.rules {
        output.push_str(&format_rules_section(rules_content));
    }

    // 4. Dependency context
    if let Some(dep_section) = format_dependency_section(&ctx.dep_providers) {
        output.push_str(&dep_section);
    }

    // 5. Child summaries
    if let Some(child_section) = format_child_summaries_section(&ctx.child_summaries) {
        output.push_str(&child_section);
    }

    // 6. Structural summaries
    let structure_pairs: Vec<(&str, String)> = ctx
        .files
        .iter()
        .filter_map(|e| e.structure.as_ref().map(|s| (e.path.as_str(), s.clone())))
        .collect();

    if let Some(structure_block) = format_structure_block(&structure_pairs) {
        output.push_str(&structure_block);
    }

    // 6. Full file contents (unless --structure-only)
    if !structure_only {
        let file_paths: Vec<String> = merge_paths(&ctx.unit);
        if !file_paths.is_empty() {
            let context =
                assemble_context(file_paths, project_dir).context("Failed to assemble context")?;
            output.push_str(&context);
        }
    }

    print!("{}", output);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mana_core::ops::context::format_attempt_notes as core_format_attempt_notes;
    use mana_core::ops::context::load_rules;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_env() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();
        (dir, mana_dir)
    }

    #[test]
    fn context_with_no_paths_in_description() {
        let (_dir, mana_dir) = setup_test_env();

        let mut unit = crate::unit::Unit::new("1", "Test unit");
        unit.description = Some("A description with no file paths".to_string());
        let slug = crate::util::title_to_slug(&unit.title);
        let unit_path = mana_dir.join(format!("1-{}.md", slug));
        unit.to_file(&unit_path).unwrap();

        let result = cmd_context(&mana_dir, "1", false, false, false, None, None);
        assert!(result.is_ok());
    }

    #[test]
    fn context_with_paths_in_description() {
        let (dir, mana_dir) = setup_test_env();
        let project_dir = dir.path();

        let src_dir = project_dir.join("src");
        fs::create_dir(&src_dir).unwrap();
        fs::write(src_dir.join("foo.rs"), "fn main() {}").unwrap();

        let mut unit = crate::unit::Unit::new("1", "Test unit");
        unit.description = Some("Check src/foo.rs for implementation".to_string());
        let slug = crate::util::title_to_slug(&unit.title);
        let unit_path = mana_dir.join(format!("1-{}.md", slug));
        unit.to_file(&unit_path).unwrap();

        let result = cmd_context(&mana_dir, "1", false, false, false, None, None);
        assert!(result.is_ok());
    }

    #[test]
    fn context_unit_not_found() {
        let (_dir, mana_dir) = setup_test_env();

        let result = cmd_context(&mana_dir, "999", false, false, false, None, None);
        assert!(result.is_err());
    }

    #[test]
    fn load_rules_returns_none_when_file_missing() {
        let (_dir, mana_dir) = setup_test_env();
        fs::write(mana_dir.join("config.yaml"), "project: test\nnext_id: 1\n").unwrap();

        let result = load_rules(&mana_dir);
        assert!(result.is_none());
    }

    #[test]
    fn load_rules_returns_none_when_file_empty() {
        let (_dir, mana_dir) = setup_test_env();
        fs::write(mana_dir.join("config.yaml"), "project: test\nnext_id: 1\n").unwrap();
        fs::write(mana_dir.join("RULES.md"), "   \n\n  ").unwrap();

        let result = load_rules(&mana_dir);
        assert!(result.is_none());
    }

    #[test]
    fn load_rules_returns_content_when_present() {
        let (_dir, mana_dir) = setup_test_env();
        fs::write(mana_dir.join("config.yaml"), "project: test\nnext_id: 1\n").unwrap();
        fs::write(mana_dir.join("RULES.md"), "# My Rules\nNo unwrap.\n").unwrap();

        let result = load_rules(&mana_dir);
        assert!(result.is_some());
        assert!(result.unwrap().contains("No unwrap."));
    }

    #[test]
    fn load_rules_uses_custom_rules_file_path() {
        let (_dir, mana_dir) = setup_test_env();
        fs::write(
            mana_dir.join("config.yaml"),
            "project: test\nnext_id: 1\nrules_file: custom-rules.md\n",
        )
        .unwrap();
        fs::write(mana_dir.join("custom-rules.md"), "Custom rules here").unwrap();

        let result = load_rules(&mana_dir);
        assert!(result.is_some());
        assert!(result.unwrap().contains("Custom rules here"));
    }

    #[test]
    fn format_rules_section_wraps_with_delimiters() {
        let output = format_rules_section("# Rules\nBe nice.\n");
        assert!(output.starts_with("═══ PROJECT RULES"));
        assert!(output.contains("# Rules\nBe nice."));
        assert!(
            output.ends_with("═════════════════════════════════════════════════════════════\n\n")
        );
    }

    // --- attempt notes tests (delegated to core) ---

    fn make_unit_with_attempts() -> crate::unit::Unit {
        use crate::unit::{AttemptOutcome, AttemptRecord};
        let mut unit = crate::unit::Unit::new("1", "Test unit");
        unit.attempt_log = vec![
            AttemptRecord {
                num: 1,
                outcome: AttemptOutcome::Abandoned,
                notes: Some("Tried X, hit bug Y".to_string()),
                agent: Some("pi-agent".to_string()),
                started_at: None,
                finished_at: None,
            },
            AttemptRecord {
                num: 2,
                outcome: AttemptOutcome::Failed,
                notes: Some("Fixed Y, now Z fails".to_string()),
                agent: None,
                started_at: None,
                finished_at: None,
            },
        ];
        unit
    }

    #[test]
    fn format_child_summaries_section_renders_parent_rollup() {
        let section = format_child_summaries_section(&[mana_core::ops::context::ChildSummary {
            id: "1.1".to_string(),
            title: "Child".to_string(),
            status: "open".to_string(),
            attempts: 2,
            recent_outcome: Some("failed".to_string()),
            summary: Some("Found a parser edge case".to_string()),
            follow_up: Some("1 unresolved decision(s)".to_string()),
        }])
        .unwrap();

        assert!(section.contains("Child Job Summaries"));
        assert!(section.contains("1.1 [open] attempts=2 recent=failed: Child"));
        assert!(section.contains("summary: Found a parser edge case"));
        assert!(section.contains("follow-up: 1 unresolved decision(s)"));
    }

    #[test]
    fn format_attempt_notes_returns_none_when_no_notes() {
        let unit = crate::unit::Unit::new("1", "Empty unit");
        let result = core_format_attempt_notes(&unit);
        assert!(result.is_none());
    }

    #[test]
    fn format_attempt_notes_returns_none_when_attempts_have_no_notes() {
        use crate::unit::{AttemptOutcome, AttemptRecord};
        let mut unit = crate::unit::Unit::new("1", "Empty unit");
        unit.attempt_log = vec![AttemptRecord {
            num: 1,
            outcome: AttemptOutcome::Abandoned,
            notes: None,
            agent: None,
            started_at: None,
            finished_at: None,
        }];
        let result = core_format_attempt_notes(&unit);
        assert!(result.is_none());
    }

    #[test]
    fn format_attempt_notes_includes_attempt_log_notes() {
        let unit = make_unit_with_attempts();
        let result = core_format_attempt_notes(&unit).expect("should produce output");
        assert!(result.contains("Attempt #1"), "should include attempt 1");
        assert!(result.contains("pi-agent"), "should include agent name");
        assert!(result.contains("abandoned"), "should include outcome");
        assert!(
            result.contains("Tried X, hit bug Y"),
            "should include notes text"
        );
        assert!(result.contains("Attempt #2"), "should include attempt 2");
        assert!(
            result.contains("Fixed Y, now Z fails"),
            "should include attempt 2 notes"
        );
    }

    #[test]
    fn format_attempt_notes_includes_unit_notes() {
        let mut unit = crate::unit::Unit::new("1", "Test unit");
        unit.notes = Some("Watch out for edge cases".to_string());
        let result = core_format_attempt_notes(&unit).expect("should produce output");
        assert!(result.contains("Watch out for edge cases"));
        assert!(result.contains("Unit notes:"));
    }

    #[test]
    fn format_attempt_notes_skips_empty_notes_strings() {
        use crate::unit::{AttemptOutcome, AttemptRecord};
        let mut unit = crate::unit::Unit::new("1", "Test unit");
        unit.notes = Some("   ".to_string());
        unit.attempt_log = vec![AttemptRecord {
            num: 1,
            outcome: AttemptOutcome::Abandoned,
            notes: Some("  ".to_string()),
            agent: None,
            started_at: None,
            finished_at: None,
        }];
        let result = core_format_attempt_notes(&unit);
        assert!(
            result.is_none(),
            "whitespace-only notes should produce no output"
        );
    }

    #[test]
    fn context_includes_attempt_notes_in_text_output() {
        let (dir, mana_dir) = setup_test_env();
        let project_dir = dir.path();

        let src_dir = project_dir.join("src");
        fs::create_dir(&src_dir).unwrap();
        fs::write(src_dir.join("foo.rs"), "fn main() {}").unwrap();

        let mut unit = make_unit_with_attempts();
        unit.id = "1".to_string();
        unit.description = Some("Check src/foo.rs for implementation".to_string());
        let slug = crate::util::title_to_slug(&unit.title);
        let unit_path = mana_dir.join(format!("1-{}.md", slug));
        unit.to_file(&unit_path).unwrap();

        let result = cmd_context(&mana_dir, "1", false, false, false, None, None);
        assert!(result.is_ok());
    }

    #[test]
    fn context_includes_attempt_notes_in_json_output() {
        let (dir, mana_dir) = setup_test_env();
        let project_dir = dir.path();

        let src_dir = project_dir.join("src");
        fs::create_dir(&src_dir).unwrap();
        fs::write(src_dir.join("foo.rs"), "fn main() {}").unwrap();

        let mut unit = make_unit_with_attempts();
        unit.id = "1".to_string();
        unit.description = Some("Check src/foo.rs for implementation".to_string());
        let slug = crate::util::title_to_slug(&unit.title);
        let unit_path = mana_dir.join(format!("1-{}.md", slug));
        unit.to_file(&unit_path).unwrap();

        let result = cmd_context(&mana_dir, "1", true, false, false, None, None);
        assert!(result.is_ok());
    }
}
