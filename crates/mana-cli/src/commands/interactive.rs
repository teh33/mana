use std::path::Path;

use anyhow::Result;
use dialoguer::theme::ColorfulTheme;
use dialoguer::{Confirm, Editor, FuzzySelect, Input, Select};

use crate::commands::create::CreateArgs;
use crate::index::Index;
use crate::project::suggest_verify_command;
use crate::unit::Status;

/// Pre-filled values from CLI flags that were already provided.
/// Any `Some` field skips the corresponding prompt.
#[derive(Default)]
pub struct Prefill {
    pub title: Option<String>,
    pub description: Option<String>,
    pub acceptance: Option<String>,
    pub notes: Option<String>,
    pub design: Option<String>,
    pub verify: Option<String>,
    pub parent: Option<String>,
    pub priority: Option<u8>,
    pub labels: Option<String>,
    pub assignee: Option<String>,
    pub deps: Option<String>,
    pub produces: Option<String>,
    pub requires: Option<String>,
    pub pass_ok: Option<bool>,
}

/// Run the interactive unit creation wizard.
///
/// Prompts the user step-by-step for unit fields. Any field already
/// provided in `prefill` is skipped (shown as pre-accepted).
///
/// Flow:
/// 1. Title (required)
/// 2. Parent (fuzzy-search from existing units, or none)
/// 3. Verify command (with smart default from project type)
/// 4. Acceptance criteria
/// 5. Priority (P0-P4, default P2)
/// 6. Description (open $EDITOR)
/// 7. Produces / Requires (for dependency tracking)
/// 8. Labels
/// 9. Summary + confirm
///
/// Returns a fully populated `CreateArgs`.
pub fn interactive_create(mana_dir: &Path, prefill: Prefill) -> Result<CreateArgs> {
    let theme = ColorfulTheme::default();
    let project_dir = mana_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine project root"))?;

    println!("Creating a new unit\n");

    // ── 1. Title (required) ──────────────────────────────────────────
    let title = if let Some(t) = prefill.title {
        println!("  Title: {}", t);
        t
    } else {
        Input::with_theme(&theme)
            .with_prompt("Title")
            .interact_text()?
    };

    // ── 2. Parent (fuzzy-select from existing open units) ────────────
    let parent = if let Some(p) = prefill.parent {
        println!("  Parent: {}", p);
        Some(p)
    } else {
        select_parent(mana_dir, &theme)?
    };

    // ── 3. Verify command ────────────────────────────────────────────
    let verify = if let Some(v) = prefill.verify {
        println!("  Verify: {}", v);
        Some(v)
    } else {
        let suggested = suggest_verify_command(project_dir);
        let mut input = Input::<String>::with_theme(&theme)
            .with_prompt("Verify command (empty to skip)")
            .allow_empty(true);
        if let Some(s) = suggested {
            input = input.default(s.to_string()).show_default(true);
        }
        let v: String = input.interact_text()?;
        if v.is_empty() {
            None
        } else {
            Some(v)
        }
    };

    // ── 4. Acceptance criteria ───────────────────────────────────────
    let acceptance = if let Some(a) = prefill.acceptance {
        println!("  Acceptance: {}", a);
        Some(a)
    } else {
        let a: String = Input::with_theme(&theme)
            .with_prompt("Acceptance criteria (empty to skip)")
            .allow_empty(true)
            .interact_text()?;
        if a.is_empty() {
            None
        } else {
            Some(a)
        }
    };

    // ── 5. Priority ──────────────────────────────────────────────────
    let priority = if let Some(p) = prefill.priority {
        println!("  Priority: P{}", p);
        p
    } else {
        let items = &[
            "P0 (critical)",
            "P1 (high)",
            "P2 (normal)",
            "P3 (low)",
            "P4 (backlog)",
        ];
        let idx = Select::with_theme(&theme)
            .with_prompt("Priority")
            .items(items)
            .default(2)
            .interact()?;
        idx as u8
    };

    // ── 6. Description ($EDITOR) ─────────────────────────────────────
    let description = if let Some(d) = prefill.description {
        println!("  Description: (provided)");
        Some(d)
    } else {
        let wants = Confirm::with_theme(&theme)
            .with_prompt("Open editor for description?")
            .default(false)
            .interact()?;

        if wants {
            let template = build_description_template(mana_dir, parent.as_deref(), &title);
            Editor::new().edit(&template)?
        } else {
            None
        }
    };

    // ── 7. Produces / Requires ───────────────────────────────────────
    let produces = if let Some(p) = prefill.produces {
        println!("  Produces: {}", p);
        Some(p)
    } else {
        let p: String = Input::with_theme(&theme)
            .with_prompt("Produces (comma-separated, empty to skip)")
            .allow_empty(true)
            .interact_text()?;
        if p.is_empty() {
            None
        } else {
            Some(p)
        }
    };

    let requires = if let Some(r) = prefill.requires {
        println!("  Requires: {}", r);
        Some(r)
    } else {
        let r: String = Input::with_theme(&theme)
            .with_prompt("Requires (comma-separated, empty to skip)")
            .allow_empty(true)
            .interact_text()?;
        if r.is_empty() {
            None
        } else {
            Some(r)
        }
    };

    // ── 8. Labels ────────────────────────────────────────────────────
    let labels = if let Some(l) = prefill.labels {
        println!("  Labels: {}", l);
        Some(l)
    } else {
        let wants = Confirm::with_theme(&theme)
            .with_prompt("Add labels?")
            .default(false)
            .interact()?;
        if wants {
            let l: String = Input::with_theme(&theme)
                .with_prompt("Labels (comma-separated)")
                .interact_text()?;
            if l.is_empty() {
                None
            } else {
                Some(l)
            }
        } else {
            None
        }
    };

    // ── 9. Summary + confirm ─────────────────────────────────────────
    println!();
    println!("─── Unit Summary ───────────────────────");
    println!("  Title:      {}", title);
    if let Some(ref p) = parent {
        println!("  Parent:     {}", p);
    }
    if let Some(ref v) = verify {
        println!("  Verify:     {}", v);
    }
    if let Some(ref a) = acceptance {
        println!("  Acceptance: {}", truncate(a, 60));
    }
    println!("  Priority:   P{}", priority);
    if description.is_some() {
        println!("  Description: (provided)");
    }
    if let Some(ref p) = produces {
        println!("  Produces:   {}", p);
    }
    if let Some(ref r) = requires {
        println!("  Requires:   {}", r);
    }
    if let Some(ref l) = labels {
        println!("  Labels:     {}", l);
    }
    println!("────────────────────────────────────────");

    let confirmed = Confirm::with_theme(&theme)
        .with_prompt("Create this unit?")
        .default(true)
        .interact()?;

    if !confirmed {
        anyhow::bail!("Cancelled");
    }

    // For interactive human usage, default to pass_ok=true.
    // Fail-first is an agent workflow concept — humans creating units
    // interactively usually want to just create the unit.
    let pass_ok = prefill.pass_ok.unwrap_or(true);

    Ok(CreateArgs {
        title,
        description,
        acceptance,
        notes: prefill.notes,
        design: prefill.design,
        verify,
        priority: Some(priority),
        labels,
        assignee: prefill.assignee,
        deps: prefill.deps,
        parent,
        produces,
        requires,
        paths: None,
        on_fail: None,
        pass_ok,
        claim: false,
        by: None,
        verify_timeout: None,
        feature: false,
        epic: false,
        decisions: Vec::new(),
        force: false,
    })
}

/// Build a description template for $EDITOR.
/// If a parent is selected, embed its title and any existing description context.
fn build_description_template(mana_dir: &Path, parent_id: Option<&str>, title: &str) -> String {
    let mut template = format!("# {}\n\n", title);

    // If parent exists, pull context from it
    if let Some(pid) = parent_id {
        if let Ok(parent_unit) = load_unit_by_id(mana_dir, pid) {
            template.push_str(&format!(
                "<!-- Parent: {} — {} -->\n\n",
                pid, parent_unit.title
            ));
            if let Some(ref desc) = parent_unit.description {
                // Extract file references from parent for hints
                let files: Vec<&str> = desc
                    .lines()
                    .filter(|l| {
                        l.starts_with("- ")
                            && (l.contains('/')
                                || l.contains(".rs")
                                || l.contains(".ts")
                                || l.contains(".py"))
                    })
                    .collect();
                if !files.is_empty() {
                    template.push_str("## Files (from parent)\n");
                    for f in files {
                        template.push_str(&format!("{}\n", f));
                    }
                    template.push('\n');
                }
            }
        }
    }

    template.push_str("## Task\n\n\n");
    template.push_str("## Files\n");
    template.push_str("- \n\n");
    template.push_str("## Context\n\n\n");
    template.push_str("## Acceptance\n");
    template.push_str("- [ ] \n");

    template
}

/// Load a unit by ID (scans units dir for matching file).
fn load_unit_by_id(mana_dir: &Path, id: &str) -> Result<crate::unit::Unit> {
    use std::fs;
    let prefix = format!("{}-", id);
    let exact_yaml = format!("{}.yaml", id);

    for entry in fs::read_dir(mana_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&prefix) && name.ends_with(".md") {
            return crate::unit::Unit::from_file(entry.path());
        }
        if *name == exact_yaml {
            return crate::unit::Unit::from_file(entry.path());
        }
    }
    anyhow::bail!("Unit {} not found", id)
}

/// Present a fuzzy-searchable selection list of open units to pick as parent.
/// Returns `None` if the user picks "(none)" or there are no units.
fn select_parent(mana_dir: &Path, theme: &ColorfulTheme) -> Result<Option<String>> {
    let index = match Index::load(mana_dir) {
        Ok(idx) => idx,
        Err(_) => return Ok(None),
    };

    // Only show open/in-progress units as potential parents
    let candidates: Vec<_> = index
        .units
        .iter()
        .filter(|b| b.status == Status::Open || b.status == Status::InProgress)
        .collect();

    if candidates.is_empty() {
        return Ok(None);
    }

    // Build display items: "(none)" + each unit
    let mut items: Vec<String> = vec!["(none — top-level unit)".to_string()];
    for b in &candidates {
        items.push(format!("{} — {}", b.id, b.title));
    }

    let selection = FuzzySelect::with_theme(theme)
        .with_prompt("Parent (type to filter)")
        .items(&items)
        .default(0)
        .interact()?;

    if selection == 0 {
        Ok(None)
    } else {
        Ok(Some(candidates[selection - 1].id.clone()))
    }
}

/// Truncate a string for display, adding ellipsis if needed.
fn truncate(s: &str, max: usize) -> String {
    // Take first line only
    let line = s.lines().next().unwrap_or(s);
    if line.len() <= max {
        line.to_string()
    } else {
        format!("{}…", &line[..max])
    }
}
