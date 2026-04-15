//! Structured task-packet and compatibility prompt builder.
//!
//! Constructs a multi-section task packet that gives runtimes/agents the context
//! they need to implement a unit successfully. Historically this module also
//! acted like a final agent-prompt authority; the rebuild is narrowing it toward
//! durable task/input preparation, with final runtime/system prompt assembly
//! living in imp.
//!
//! The current API still returns a prompt-shaped bundle because legacy
//! `mana run`/`mana context --agent-prompt` compatibility flows consume it.
//! Long-term, treat the material here as durable task-packet source content
//! rather than the final runtime prompt center.
//!
//! Sections (in order):
//! 1. Project Rules
//! 2. Parent Context
//! 3. Sibling Discoveries
//! 4. Unit Assignment
//! 5. Concurrent Modification Warning
//! 6. Referenced Files
//! 7. Acceptance Criteria
//! 8. Pre-flight Check
//! 9. Previous Attempts
//! 10. Approach
//! 11. Verify Gate
//! 12. Constraints
//! 13. Tool Strategy

use std::path::{Path, PathBuf};

use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

use crate::config::Config;
use crate::ctx_assembler::{extract_paths, read_file};
use crate::discovery::find_unit_file;
use crate::index::Index;
use crate::unit::{AttemptOutcome, Status, Unit};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Result of building durable task-packet material for compatibility flows.
pub struct PromptResult {
    /// Compatibility system-prompt content assembled from durable task/input sections.
    pub system_prompt: String,
    /// Compatibility user message for legacy consumers.
    pub user_message: String,
    /// Path to the unit file, for @file injection by the caller.
    pub file_ref: String,
}

/// Options for prompt construction.
pub struct PromptOptions {
    /// Path to the `.mana/` directory.
    pub mana_dir: PathBuf,
    /// Optional instructions to prepend to the user message.
    pub instructions: Option<String>,
    /// Units running concurrently that share files with this unit.
    pub concurrent_overlaps: Option<Vec<FileOverlap>>,
}

/// Describes a concurrent unit that overlaps on files.
pub struct FileOverlap {
    /// ID of the overlapping unit.
    pub unit_id: String,
    /// Title of the overlapping unit.
    pub title: String,
    /// File paths shared between the two units.
    pub shared_files: Vec<String>,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Max characters per parent body.
const PARENT_CHAR_CAP: usize = 2000;

/// Max total characters across all ancestors.
const TOTAL_ANCESTOR_CHAR_CAP: usize = 3000;

/// Max total characters from sibling discovery notes.
const DISCOVERY_CHAR_CAP: usize = 1500;

/// Max total characters of file content to embed in the prompt.
const FILE_CONTENT_CHAR_CAP: usize = 8000;

/// Pattern to detect discovery notes in unit notes.
static DISCOVERY_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)discover").expect("Invalid discovery regex"));

/// Keywords near a path that hint the file is a modify/create target.
static PRIORITY_KEYWORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(modify|create|add|edit|update|change|implement|write)\b")
        .expect("Invalid priority keywords regex")
});

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Build the full structured task-packet material for a unit.
///
/// Returns a [`PromptResult`] containing compatibility prompt-shaped fields
/// for current legacy consumers. The content is assembled from up to 13
/// sections that capture durable task/input material another runtime can use.
pub fn build_agent_prompt(unit: &Unit, options: &PromptOptions) -> Result<PromptResult> {
    let mana_dir = &options.mana_dir;
    let mut sections: Vec<String> = Vec::new();

    // 1. Project Rules
    if let Some(rules) = load_rules(mana_dir) {
        sections.push(format!("# Project Rules\n\n{}", rules));
    }

    // 2. Parent Context
    let parent_sections = collect_parent_context(unit, mana_dir);
    for section in parent_sections {
        sections.push(section);
    }

    // 3. Sibling Discoveries
    if let Some(discoveries) = collect_sibling_discoveries(unit, mana_dir) {
        sections.push(discoveries);
    }

    // 4. Unit Assignment
    sections.push(format!(
        "# Unit Assignment\n\nYou are implementing unit {}: {}",
        unit.id, unit.title
    ));

    // 5. Concurrent Modification Warning
    if let Some(ref overlaps) = options.concurrent_overlaps {
        if !overlaps.is_empty() {
            sections.push(format_concurrent_warning(overlaps));
        }
    }

    // 6. Referenced Files
    let project_dir = mana_dir.parent().unwrap_or(Path::new("."));
    let description = unit.description.as_deref().unwrap_or("");
    if let Some(file_context) = assemble_file_context(description, project_dir) {
        sections.push(file_context);
    }

    // 7. Acceptance Criteria
    if let Some(ref acceptance) = unit.acceptance {
        sections.push(format!(
            "# Acceptance Criteria (must ALL be true)\n\n{}",
            acceptance
        ));
    }

    // 8. Pre-flight Check
    if let Some(ref verify) = unit.verify {
        sections.push(format!(
            "# Pre-flight Check\n\n\
             Before implementing, run the verify command to confirm it currently FAILS:\n\
             ```\n{}\n```\n\
             If it errors for infrastructure reasons (missing deps, wrong path), fix that first.",
            verify
        ));
    }

    // 9. Previous Attempts
    if unit.attempts > 0 {
        sections.push(format_previous_attempts(unit));
    }

    // 10. Approach
    sections.push(format_approach(&unit.id));

    // 11. Verify Gate
    sections.push(format_verify_gate(unit));

    // 12. Constraints
    sections.push(format_constraints(&unit.id));

    // 13. Tool Strategy
    sections.push(format_tool_strategy());

    // Assemble system prompt
    let system_prompt = sections.join("\n\n---\n\n");

    // User message (legacy compatibility for current mana-side prompt consumers)
    let mut user_message = String::new();
    if let Some(ref instructions) = options.instructions {
        user_message.push_str(instructions);
        user_message.push_str("\n\n");
    }
    user_message.push_str(&format!(
        "implement this unit and hand completion back through the configured runtime/close path for unit {}",
        unit.id
    ));

    // File reference
    let file_ref = find_unit_file(mana_dir, &unit.id)
        .map(|p| format!("@{}", p.display()))
        .unwrap_or_default();

    Ok(PromptResult {
        system_prompt,
        user_message,
        file_ref,
    })
}

// ---------------------------------------------------------------------------
// Section builders
// ---------------------------------------------------------------------------

/// Load project rules from `.mana/RULES.md` (or configured path).
fn load_rules(mana_dir: &Path) -> Option<String> {
    let config = Config::load_with_extends(mana_dir).ok()?;
    let rules_path = config.rules_path(mana_dir);
    let content = std::fs::read_to_string(&rules_path).ok()?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(content)
}

/// Walk up the parent chain and collect context sections.
///
/// Returns sections in outermost-first order (grandparent before parent).
/// Each parent body is capped at [`PARENT_CHAR_CAP`]; total ancestor
/// context is capped at [`TOTAL_ANCESTOR_CHAR_CAP`].
fn collect_parent_context(unit: &Unit, mana_dir: &Path) -> Vec<String> {
    let Some(ref first_parent) = unit.parent else {
        return Vec::new();
    };

    let mut sections = Vec::new();
    let mut total_chars: usize = 0;
    let mut current_id = Some(first_parent.clone());

    while let Some(id) = current_id {
        if total_chars >= TOTAL_ANCESTOR_CHAR_CAP {
            break;
        }

        let parent = match load_unit(mana_dir, &id) {
            Some(b) => b,
            None => break,
        };

        let body = match parent.description {
            Some(ref d) if !d.trim().is_empty() => d.clone(),
            _ => break,
        };

        let remaining = TOTAL_ANCESTOR_CHAR_CAP - total_chars;
        let char_limit = PARENT_CHAR_CAP.min(remaining);
        let trimmed = truncate_text(&body, char_limit);

        sections.push(format!(
            "# Parent Context (unit {}: {})\n\n{}",
            parent.id, parent.title, trimmed
        ));

        total_chars += trimmed.len();
        current_id = parent.parent.clone();
    }

    // Reverse so grandparent appears before parent (outermost context first)
    sections.reverse();
    sections
}

/// Collect discovery notes from closed sibling units.
///
/// Reads siblings (children of the same parent) and extracts notes
/// containing "discover" from closed siblings. Caps total context
/// at [`DISCOVERY_CHAR_CAP`].
fn collect_sibling_discoveries(unit: &Unit, mana_dir: &Path) -> Option<String> {
    let parent_id = unit.parent.as_ref()?;

    let index = Index::load_or_rebuild(mana_dir).ok()?;

    // Find closed siblings (same parent, not self)
    let closed_siblings: Vec<_> = index
        .units
        .iter()
        .filter(|e| {
            e.id != unit.id && e.parent.as_deref() == Some(parent_id) && e.status == Status::Closed
        })
        .collect();

    if closed_siblings.is_empty() {
        return None;
    }

    let mut parts = Vec::new();
    let mut total_chars: usize = 0;

    for sibling in &closed_siblings {
        if total_chars >= DISCOVERY_CHAR_CAP {
            break;
        }

        let sibling_unit = match load_unit(mana_dir, &sibling.id) {
            Some(b) => b,
            None => continue,
        };

        let notes = match sibling_unit.notes {
            Some(ref n) if !n.trim().is_empty() => n.clone(),
            _ => continue,
        };

        if !DISCOVERY_PATTERN.is_match(&notes) {
            continue;
        }

        let remaining = DISCOVERY_CHAR_CAP - total_chars;
        let trimmed = truncate_text(&notes, remaining);

        parts.push(format!(
            "## From unit {} ({}):\n{}",
            sibling.id, sibling.title, trimmed
        ));
        total_chars += trimmed.len();
    }

    if parts.is_empty() {
        return None;
    }

    Some(format!(
        "# Discoveries from completed siblings\n\n{}",
        parts.join("\n\n")
    ))
}

/// Format the concurrent modification warning section.
fn format_concurrent_warning(overlaps: &[FileOverlap]) -> String {
    let mut lines = Vec::new();
    for overlap in overlaps {
        let files = overlap.shared_files.join(", ");
        lines.push(format!(
            "- Unit {} ({}) may also be modifying: {}",
            overlap.unit_id, overlap.title, files
        ));
    }

    format!(
        "# Concurrent Modification Warning\n\n\
         The following units are running in parallel and share files with your unit:\n\n\
         {}\n\n\
         Be careful with overwrites. Prefer surgical Edit operations over full Write.\n\
         If you must rewrite a file, read it immediately before writing to avoid clobbering concurrent changes.",
        lines.join("\n")
    )
}

/// Assemble referenced file contents from the unit description.
///
/// Extracts file paths from the description text, reads their contents
/// from the project directory, and assembles them into a markdown section.
/// Files near priority keywords (modify, create, etc.) are listed first.
/// Total content is capped at [`FILE_CONTENT_CHAR_CAP`].
fn assemble_file_context(description: &str, project_dir: &Path) -> Option<String> {
    let paths = extract_prioritized_paths(description);
    if paths.is_empty() {
        return None;
    }

    let canonical_base = project_dir.canonicalize().ok()?;
    let mut file_sections = Vec::new();
    let mut total_chars: usize = 0;

    for file_path in &paths {
        if total_chars >= FILE_CONTENT_CHAR_CAP {
            break;
        }

        let full_path = project_dir.join(file_path);
        let canonical = match full_path.canonicalize() {
            Ok(c) => c,
            Err(_) => continue, // file doesn't exist
        };

        // Stay within project directory
        if !canonical.starts_with(&canonical_base) {
            continue;
        }

        // Skip directories
        if canonical.is_dir() {
            continue;
        }

        let content = match read_file(&canonical) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let remaining = FILE_CONTENT_CHAR_CAP - total_chars;
        let content = if content.len() > remaining {
            let mut truncated = content[..remaining].to_string();
            truncated.push_str("\n\n[…truncated]");
            truncated
        } else {
            content
        };

        let lang = detect_language(file_path);
        file_sections.push(format!("## {}\n```{}\n{}\n```", file_path, lang, content));
        total_chars += content.len();
    }

    if file_sections.is_empty() {
        return None;
    }

    Some(format!(
        "# Referenced Files\n\n{}",
        file_sections.join("\n\n")
    ))
}

/// Format the previous attempts section.
fn format_previous_attempts(unit: &Unit) -> String {
    let mut section = format!("# Previous Attempts ({} so far)", unit.attempts);

    // Include unit notes
    if let Some(ref notes) = unit.notes {
        let trimmed = notes.trim();
        if !trimmed.is_empty() {
            section.push_str(&format!("\n\n{}", trimmed));
        }
    }

    // Include per-attempt notes from attempt_log
    for attempt in &unit.attempt_log {
        if let Some(ref notes) = attempt.notes {
            let trimmed = notes.trim();
            if !trimmed.is_empty() {
                let outcome = match attempt.outcome {
                    AttemptOutcome::Success => "success",
                    AttemptOutcome::Failed => "failed",
                    AttemptOutcome::Abandoned => "abandoned",
                };
                let agent_str = attempt
                    .agent
                    .as_deref()
                    .map(|a| format!(" ({})", a))
                    .unwrap_or_default();
                section.push_str(&format!(
                    "\n\nAttempt #{}{} [{}]: {}",
                    attempt.num, agent_str, outcome, trimmed
                ));
            }
        }
    }

    section.push_str(
        "\n\nIMPORTANT: Do NOT repeat the same approach. \
         The notes above explain what was tried.\n\
         Read them carefully before starting.",
    );

    section
}

/// Format the approach section with numbered workflow.
fn format_approach(unit_id: &str) -> String {
    format!(
        "# Approach\n\n\
         1. Read the unit description carefully — it IS your spec\n\
         2. Understand the acceptance criteria before writing code\n\
         3. Read referenced files to understand existing patterns\n\
         4. Implement changes file by file\n\
         5. Run the verify command to check your work\n\
         6. If verify passes, hand completion back through the configured runtime/close path for unit {id}\n\
         7. After completion, record what you learned for future workers:\n   \
            mana update {id} --note \"Discoveries: <brief notes about patterns, conventions, \
            or gotchas you found that might help sibling units>\"\n\
         8. If verify fails, fix and retry\n\
         9. If stuck after 3 attempts, run: mana update {id} --note \"Stuck: <explanation>\"",
        id = unit_id
    )
}

/// Format the verify gate section.
fn format_verify_gate(unit: &Unit) -> String {
    let batch_mode = std::env::var("MANA_BATCH_VERIFY").is_ok();

    if let Some(ref verify) = unit.verify {
        if batch_mode {
            format!(
                "# Verify Gate\n\n\
                 Your verify command is:\n\
                 ```\n{verify}\n```\n\
                 Batch verify mode: the orchestrator/runtime runs this command after you exit — \
                 you do not need to run it yourself.\n\
                 Use scoped checks (e.g. `cargo check -p <crate>`) for fast feedback during work.\n\
                 Signal completion through the configured runtime/close path for unit {id}",
                verify = verify,
                id = unit.id
            )
        } else {
            format!(
                "# Verify Gate\n\n\
                 Your verify command is:\n\
                 ```\n{}\n```\n\
                 This MUST exit 0 for the unit to close. Test it before declaring done.",
                verify
            )
        }
    } else {
        format!(
            "# Verify Gate\n\n\
             No verify command is set for this unit.\n\
             When all acceptance criteria are met, hand completion back through the configured runtime/close path for unit {}",
            unit.id
        )
    }
}

/// Format the constraints section.
fn format_constraints(unit_id: &str) -> String {
    format!(
        "# Constraints\n\n\
         - Only modify files mentioned in the description unless clearly necessary\n\
         - Don't add dependencies without justification\n\
         - Preserve existing tests\n\
         - Run the project's test/build commands before handing completion back\n\
         - When complete, hand completion back through the configured runtime/close path for unit {}",
        unit_id
    )
}

/// Format the tool strategy section.
fn format_tool_strategy() -> String {
    "# Tool Strategy\n\n\
     - Use probe_search for semantic code search, rg for exact text matching\n\
     - Read files before editing — never edit blind\n\
     - Use Edit for surgical changes, Write for new files\n\
     - Use Bash to run tests and verify commands"
        .to_string()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Truncate text to a character limit, appending an ellipsis if trimmed.
fn truncate_text(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut result = text[..limit].to_string();
    result.push_str("\n\n[…truncated]");
    result
}

/// Extract file paths from description text, prioritized by action keywords.
///
/// Paths on lines containing words like "modify", "create", "add" come first,
/// followed by other referenced paths.
fn extract_prioritized_paths(description: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut prioritized = Vec::new();
    let mut normal = Vec::new();

    for line in description.lines() {
        let line_paths = extract_paths(line);
        let is_priority = PRIORITY_KEYWORDS.is_match(line);

        for p in line_paths {
            if seen.insert(p.clone()) {
                if is_priority {
                    prioritized.push(p);
                } else {
                    normal.push(p);
                }
            }
        }
    }

    prioritized.extend(normal);
    prioritized
}

/// Detect programming language from file extension for code fence tagging.
fn detect_language(path: &str) -> &'static str {
    match Path::new(path).extension().and_then(|e| e.to_str()) {
        Some("rs") => "rust",
        Some("ts") => "typescript",
        Some("tsx") => "typescript",
        Some("js") => "javascript",
        Some("jsx") => "javascript",
        Some("py") => "python",
        Some("md") => "markdown",
        Some("json") => "json",
        Some("toml") => "toml",
        Some("yaml") | Some("yml") => "yaml",
        Some("sh") => "bash",
        Some("go") => "go",
        Some("java") => "java",
        Some("css") => "css",
        Some("html") => "html",
        Some("sql") => "sql",
        Some("c") => "c",
        Some("cpp") => "cpp",
        Some("h") => "c",
        Some("hpp") => "cpp",
        Some("rb") => "ruby",
        Some("php") => "php",
        Some("swift") => "swift",
        Some("kt") => "kotlin",
        _ => "",
    }
}

/// Load a unit by ID, returning None on any error.
fn load_unit(mana_dir: &Path, id: &str) -> Option<Unit> {
    let path = find_unit_file(mana_dir, id).ok()?;
    Unit::from_file(&path).ok()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unit::{AttemptOutcome, AttemptRecord, Unit};
    use std::fs;
    use tempfile::TempDir;

    /// Create a test environment with .mana/ directory and minimal config.
    fn setup_test_env() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();
        fs::write(
            mana_dir.join("config.yaml"),
            "project: test\nnext_id: 100\n",
        )
        .unwrap();
        (dir, mana_dir)
    }

    /// Write a unit to the .mana/ directory with standard naming.
    fn write_test_unit(mana_dir: &Path, unit: &Unit) {
        let slug = crate::util::title_to_slug(&unit.title);
        let path = mana_dir.join(format!("{}-{}.md", unit.id, slug));
        unit.to_file(&path).unwrap();
    }

    // -- truncate_text --

    #[test]
    fn truncate_text_short() {
        assert_eq!(truncate_text("hello", 100), "hello");
    }

    #[test]
    fn truncate_text_at_limit() {
        assert_eq!(truncate_text("hello", 5), "hello");
    }

    #[test]
    fn truncate_text_over_limit() {
        let result = truncate_text("hello world", 5);
        assert!(result.starts_with("hello"));
        assert!(result.contains("[…truncated]"));
    }

    // -- detect_language --

    #[test]
    fn detect_language_known_extensions() {
        assert_eq!(detect_language("src/main.rs"), "rust");
        assert_eq!(detect_language("index.ts"), "typescript");
        assert_eq!(detect_language("app.tsx"), "typescript");
        assert_eq!(detect_language("script.py"), "python");
        assert_eq!(detect_language("config.json"), "json");
        assert_eq!(detect_language("Cargo.toml"), "toml");
        assert_eq!(detect_language("config.yaml"), "yaml");
        assert_eq!(detect_language("config.yml"), "yaml");
        assert_eq!(detect_language("deploy.sh"), "bash");
        assert_eq!(detect_language("main.go"), "go");
        assert_eq!(detect_language("Main.java"), "java");
        assert_eq!(detect_language("style.css"), "css");
        assert_eq!(detect_language("page.html"), "html");
        assert_eq!(detect_language("query.sql"), "sql");
    }

    #[test]
    fn detect_language_unknown_extension() {
        assert_eq!(detect_language("file.xyz"), "");
        assert_eq!(detect_language("Makefile"), "");
    }

    // -- extract_prioritized_paths --

    #[test]
    fn prioritized_paths_modify_first() {
        let desc = "Read src/lib.rs for context\nModify src/main.rs to add feature";
        let paths = extract_prioritized_paths(desc);
        assert_eq!(paths, vec!["src/main.rs", "src/lib.rs"]);
    }

    #[test]
    fn prioritized_paths_create_first() {
        let desc = "Check src/old.rs\nCreate src/new.rs with the new module";
        let paths = extract_prioritized_paths(desc);
        assert_eq!(paths, vec!["src/new.rs", "src/old.rs"]);
    }

    #[test]
    fn prioritized_paths_deduplicates() {
        let desc = "Modify src/main.rs\nAlso read src/main.rs for context";
        let paths = extract_prioritized_paths(desc);
        assert_eq!(paths, vec!["src/main.rs"]);
    }

    #[test]
    fn prioritized_paths_no_keywords() {
        let desc = "See src/foo.rs and src/bar.rs";
        let paths = extract_prioritized_paths(desc);
        assert_eq!(paths, vec!["src/foo.rs", "src/bar.rs"]);
    }

    #[test]
    fn prioritized_paths_empty() {
        let paths = extract_prioritized_paths("No files here");
        assert!(paths.is_empty());
    }

    // -- load_rules --

    #[test]
    fn load_rules_returns_none_when_missing() {
        let (_dir, mana_dir) = setup_test_env();
        let result = load_rules(&mana_dir);
        assert!(result.is_none());
    }

    #[test]
    fn load_rules_returns_none_when_empty() {
        let (_dir, mana_dir) = setup_test_env();
        fs::write(mana_dir.join("RULES.md"), "   \n  ").unwrap();
        let result = load_rules(&mana_dir);
        assert!(result.is_none());
    }

    #[test]
    fn load_rules_returns_content() {
        let (_dir, mana_dir) = setup_test_env();
        fs::write(mana_dir.join("RULES.md"), "# Rules\nNo unwrap.\n").unwrap();
        let result = load_rules(&mana_dir);
        assert!(result.is_some());
        assert!(result.unwrap().contains("No unwrap."));
    }

    // -- collect_parent_context --

    #[test]
    fn parent_context_no_parent() {
        let (_dir, mana_dir) = setup_test_env();
        let unit = Unit::new("1", "No parent");
        let sections = collect_parent_context(&unit, &mana_dir);
        assert!(sections.is_empty());
    }

    #[test]
    fn parent_context_single_parent() {
        let (_dir, mana_dir) = setup_test_env();

        // Create parent unit
        let mut parent = Unit::new("1", "Parent Task");
        parent.description = Some("This is the parent goal.".to_string());
        write_test_unit(&mana_dir, &parent);

        // Create child referencing parent
        let mut child = Unit::new("1.1", "Child Task");
        child.parent = Some("1".to_string());
        write_test_unit(&mana_dir, &child);

        let sections = collect_parent_context(&child, &mana_dir);
        assert_eq!(sections.len(), 1);
        assert!(sections[0].contains("Parent Context"));
        assert!(sections[0].contains("unit 1: Parent Task"));
        assert!(sections[0].contains("parent goal"));
    }

    #[test]
    fn parent_context_grandparent_appears_first() {
        let (_dir, mana_dir) = setup_test_env();

        // Grandparent
        let mut grandparent = Unit::new("1", "Grandparent");
        grandparent.description = Some("Grand context.".to_string());
        write_test_unit(&mana_dir, &grandparent);

        // Parent
        let mut parent = Unit::new("1.1", "Parent");
        parent.parent = Some("1".to_string());
        parent.description = Some("Parent context.".to_string());
        write_test_unit(&mana_dir, &parent);

        // Child
        let mut child = Unit::new("1.1.1", "Child");
        child.parent = Some("1.1".to_string());

        let sections = collect_parent_context(&child, &mana_dir);
        assert_eq!(sections.len(), 2);
        // Grandparent should appear first (reversed order)
        assert!(sections[0].contains("Grandparent"));
        assert!(sections[1].contains("Parent"));
    }

    #[test]
    fn parent_context_caps_total_chars() {
        let (_dir, mana_dir) = setup_test_env();

        // Create a parent with a very long description
        let mut parent = Unit::new("1", "Verbose Parent");
        parent.description = Some("x".repeat(5000));
        write_test_unit(&mana_dir, &parent);

        let mut child = Unit::new("1.1", "Child");
        child.parent = Some("1".to_string());

        let sections = collect_parent_context(&child, &mana_dir);
        assert_eq!(sections.len(), 1);
        // Body should be truncated
        assert!(sections[0].contains("[…truncated]"));
        // Total chars should respect PARENT_CHAR_CAP
        let body_start = sections[0].find("\n\n").unwrap() + 2;
        let body = &sections[0][body_start..];
        // Truncated body should be roughly PARENT_CHAR_CAP + truncation marker
        assert!(body.len() < PARENT_CHAR_CAP + 50);
    }

    // -- collect_sibling_discoveries --

    #[test]
    fn sibling_discoveries_no_parent() {
        let (_dir, mana_dir) = setup_test_env();
        let unit = Unit::new("1", "No parent");
        let result = collect_sibling_discoveries(&unit, &mana_dir);
        assert!(result.is_none());
    }

    #[test]
    fn sibling_discoveries_finds_closed_with_discover() {
        let (_dir, mana_dir) = setup_test_env();

        // Create parent
        let parent = Unit::new("1", "Parent");
        write_test_unit(&mana_dir, &parent);

        // Create closed sibling with discovery notes
        let mut sibling = Unit::new("1.1", "Sibling A");
        sibling.parent = Some("1".to_string());
        sibling.status = Status::Closed;
        sibling.notes = Some("Discoveries: the API uses snake_case".to_string());
        write_test_unit(&mana_dir, &sibling);

        // The unit under test
        let mut unit = Unit::new("1.2", "Current Unit");
        unit.parent = Some("1".to_string());
        write_test_unit(&mana_dir, &unit);

        // Need to rebuild index
        let _ = Index::build(&mana_dir).unwrap().save(&mana_dir);

        let result = collect_sibling_discoveries(&unit, &mana_dir);
        assert!(result.is_some());
        let text = result.unwrap();
        assert!(text.contains("Discoveries from completed siblings"));
        assert!(text.contains("snake_case"));
    }

    #[test]
    fn sibling_discoveries_skips_non_discover_notes() {
        let (_dir, mana_dir) = setup_test_env();

        let parent = Unit::new("1", "Parent");
        write_test_unit(&mana_dir, &parent);

        // Closed sibling without "discover" in notes
        let mut sibling = Unit::new("1.1", "Sibling");
        sibling.parent = Some("1".to_string());
        sibling.status = Status::Closed;
        sibling.notes = Some("Just regular notes about the task".to_string());
        write_test_unit(&mana_dir, &sibling);

        let mut unit = Unit::new("1.2", "Current");
        unit.parent = Some("1".to_string());
        write_test_unit(&mana_dir, &unit);

        let _ = Index::build(&mana_dir).unwrap().save(&mana_dir);

        let result = collect_sibling_discoveries(&unit, &mana_dir);
        assert!(result.is_none());
    }

    #[test]
    fn sibling_discoveries_skips_open_siblings() {
        let (_dir, mana_dir) = setup_test_env();

        let parent = Unit::new("1", "Parent");
        write_test_unit(&mana_dir, &parent);

        // Open sibling with discovery notes — should be skipped
        let mut sibling = Unit::new("1.1", "Open Sibling");
        sibling.parent = Some("1".to_string());
        sibling.status = Status::Open;
        sibling.notes = Some("Discoveries: something useful".to_string());
        write_test_unit(&mana_dir, &sibling);

        let mut unit = Unit::new("1.2", "Current");
        unit.parent = Some("1".to_string());
        write_test_unit(&mana_dir, &unit);

        let _ = Index::build(&mana_dir).unwrap().save(&mana_dir);

        let result = collect_sibling_discoveries(&unit, &mana_dir);
        assert!(result.is_none());
    }

    // -- format_concurrent_warning --

    #[test]
    fn concurrent_warning_single_overlap() {
        let overlaps = vec![FileOverlap {
            unit_id: "5".to_string(),
            title: "Other Task".to_string(),
            shared_files: vec!["src/main.rs".to_string()],
        }];
        let result = format_concurrent_warning(&overlaps);
        assert!(result.contains("Concurrent Modification Warning"));
        assert!(result.contains("Unit 5 (Other Task)"));
        assert!(result.contains("src/main.rs"));
    }

    #[test]
    fn concurrent_warning_multiple_overlaps() {
        let overlaps = vec![
            FileOverlap {
                unit_id: "5".to_string(),
                title: "Task A".to_string(),
                shared_files: vec!["src/a.rs".to_string(), "src/b.rs".to_string()],
            },
            FileOverlap {
                unit_id: "6".to_string(),
                title: "Task B".to_string(),
                shared_files: vec!["src/c.rs".to_string()],
            },
        ];
        let result = format_concurrent_warning(&overlaps);
        assert!(result.contains("Unit 5"));
        assert!(result.contains("Unit 6"));
        assert!(result.contains("src/a.rs, src/b.rs"));
    }

    // -- assemble_file_context --

    #[test]
    fn file_context_reads_existing_files() {
        let dir = TempDir::new().unwrap();
        let project_dir = dir.path();

        // Create a source file
        let src = project_dir.join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("main.rs"), "fn main() {}").unwrap();

        let desc = "Modify src/main.rs to add feature";
        let result = assemble_file_context(desc, project_dir);
        assert!(result.is_some());
        let text = result.unwrap();
        assert!(text.contains("# Referenced Files"));
        assert!(text.contains("## src/main.rs"));
        assert!(text.contains("```rust"));
        assert!(text.contains("fn main() {}"));
    }

    #[test]
    fn file_context_skips_missing_files() {
        let dir = TempDir::new().unwrap();
        let desc = "Read src/nonexistent.rs";
        let result = assemble_file_context(desc, dir.path());
        assert!(result.is_none());
    }

    #[test]
    fn file_context_caps_total_chars() {
        let dir = TempDir::new().unwrap();
        let project_dir = dir.path();
        let src = project_dir.join("src");
        fs::create_dir(&src).unwrap();

        // Create a large file
        fs::write(src.join("big.rs"), "x".repeat(20000)).unwrap();

        let desc = "Read src/big.rs";
        let result = assemble_file_context(desc, project_dir);
        assert!(result.is_some());
        let text = result.unwrap();
        assert!(text.contains("[…truncated]"));
        // Total content should be around FILE_CONTENT_CHAR_CAP
        assert!(text.len() < FILE_CONTENT_CHAR_CAP + 500);
    }

    #[test]
    fn file_context_no_paths() {
        let dir = TempDir::new().unwrap();
        let result = assemble_file_context("No file paths here", dir.path());
        assert!(result.is_none());
    }

    // -- format_previous_attempts --

    #[test]
    fn previous_attempts_with_notes() {
        let mut unit = Unit::new("1", "Test");
        unit.attempts = 2;
        unit.notes = Some("Tried approach X, it broke Y.".to_string());
        unit.attempt_log = vec![AttemptRecord {
            num: 1,
            outcome: AttemptOutcome::Failed,
            notes: Some("First try failed due to Z".to_string()),
            agent: Some("agent-1".to_string()),
            started_at: None,
            finished_at: None,
            autonomy_observation: None,
        }];

        let result = format_previous_attempts(&unit);
        assert!(result.contains("Previous Attempts (2 so far)"));
        assert!(result.contains("Tried approach X"));
        assert!(result.contains("Attempt #1 (agent-1) [failed]"));
        assert!(result.contains("First try failed"));
        assert!(result.contains("Do NOT repeat"));
    }

    #[test]
    fn previous_attempts_no_notes() {
        let mut unit = Unit::new("1", "Test");
        unit.attempts = 1;

        let result = format_previous_attempts(&unit);
        assert!(result.contains("Previous Attempts (1 so far)"));
        assert!(result.contains("Do NOT repeat"));
    }

    // -- format_approach --

    #[test]
    fn approach_contains_unit_id() {
        let result = format_approach("42");
        assert!(result.contains("configured runtime/close path for unit 42"));
        assert!(result.contains("mana update 42"));
    }

    // -- format_verify_gate --

    #[test]
    fn verify_gate_with_command() {
        let mut unit = Unit::new("1", "Test");
        unit.verify = Some("cargo test unit::check".to_string());
        let result = format_verify_gate(&unit);
        assert!(result.contains("cargo test"));
        assert!(result.contains("MUST exit 0"));
    }

    #[test]
    fn verify_gate_without_command() {
        let unit = Unit::new("1", "Test");
        let result = format_verify_gate(&unit);
        assert!(result.contains("No verify command"));
        assert!(result.contains("configured runtime/close path for unit 1"));
    }

    // -- format_constraints --

    #[test]
    fn constraints_contains_unit_id() {
        let result = format_constraints("7");
        assert!(result.contains("configured runtime/close path for unit 7"));
        assert!(result.contains("Don't add dependencies"));
    }

    // -- format_tool_strategy --

    #[test]
    fn tool_strategy_mentions_key_tools() {
        let result = format_tool_strategy();
        assert!(result.contains("probe_search"));
        assert!(result.contains("rg"));
        assert!(result.contains("Edit"));
        assert!(result.contains("Write"));
    }

    // -- build_agent_prompt integration --

    #[test]
    fn build_prompt_minimal_unit() {
        let (_dir, mana_dir) = setup_test_env();

        let mut unit = Unit::new("1", "Simple Task");
        unit.description = Some("Just do the thing.".to_string());
        unit.verify = Some("cargo test unit::check".to_string());
        write_test_unit(&mana_dir, &unit);

        let options = PromptOptions {
            mana_dir: mana_dir.clone(),
            instructions: None,
            concurrent_overlaps: None,
        };

        let result = build_agent_prompt(&unit, &options).unwrap();

        // System prompt should contain key sections
        assert!(result.system_prompt.contains("Unit Assignment"));
        assert!(result.system_prompt.contains("unit 1: Simple Task"));
        assert!(result.system_prompt.contains("Pre-flight Check"));
        assert!(result.system_prompt.contains("cargo test"));
        assert!(result.system_prompt.contains("Verify Gate"));
        assert!(result.system_prompt.contains("Approach"));
        assert!(result.system_prompt.contains("Constraints"));
        assert!(result.system_prompt.contains("Tool Strategy"));

        // Sections should be separated by ---
        assert!(result.system_prompt.contains("---"));

        // User message should contain close instruction
        assert!(result
            .user_message
            .contains("configured runtime/close path for unit 1"));

        // File ref should point to the unit file
        assert!(result.file_ref.contains("1-simple-task.md"));
    }

    #[test]
    fn build_prompt_with_instructions() {
        let (_dir, mana_dir) = setup_test_env();

        let unit = Unit::new("1", "Task");
        write_test_unit(&mana_dir, &unit);

        let options = PromptOptions {
            mana_dir: mana_dir.clone(),
            instructions: Some("Focus on performance".to_string()),
            concurrent_overlaps: None,
        };

        let result = build_agent_prompt(&unit, &options).unwrap();
        assert!(result.user_message.starts_with("Focus on performance"));
        assert!(result
            .user_message
            .contains("configured runtime/close path for unit 1"));
    }

    #[test]
    fn build_prompt_with_rules() {
        let (_dir, mana_dir) = setup_test_env();
        fs::write(mana_dir.join("RULES.md"), "# Style\nUse snake_case.\n").unwrap();

        let unit = Unit::new("1", "Task");
        write_test_unit(&mana_dir, &unit);

        let options = PromptOptions {
            mana_dir: mana_dir.clone(),
            instructions: None,
            concurrent_overlaps: None,
        };

        let result = build_agent_prompt(&unit, &options).unwrap();
        assert!(result.system_prompt.contains("Project Rules"));
        assert!(result.system_prompt.contains("snake_case"));
    }

    #[test]
    fn build_prompt_with_acceptance_criteria() {
        let (_dir, mana_dir) = setup_test_env();

        let mut unit = Unit::new("1", "Task");
        unit.acceptance = Some("All tests pass\nNo warnings".to_string());
        write_test_unit(&mana_dir, &unit);

        let options = PromptOptions {
            mana_dir: mana_dir.clone(),
            instructions: None,
            concurrent_overlaps: None,
        };

        let result = build_agent_prompt(&unit, &options).unwrap();
        assert!(result.system_prompt.contains("Acceptance Criteria"));
        assert!(result.system_prompt.contains("All tests pass"));
        assert!(result.system_prompt.contains("No warnings"));
    }

    #[test]
    fn build_prompt_with_concurrent_overlaps() {
        let (_dir, mana_dir) = setup_test_env();

        let unit = Unit::new("1", "Task");
        write_test_unit(&mana_dir, &unit);

        let options = PromptOptions {
            mana_dir: mana_dir.clone(),
            instructions: None,
            concurrent_overlaps: Some(vec![FileOverlap {
                unit_id: "2".to_string(),
                title: "Other".to_string(),
                shared_files: vec!["src/shared.rs".to_string()],
            }]),
        };

        let result = build_agent_prompt(&unit, &options).unwrap();
        assert!(result
            .system_prompt
            .contains("Concurrent Modification Warning"));
        assert!(result.system_prompt.contains("Unit 2 (Other)"));
    }

    #[test]
    fn build_prompt_with_previous_attempts() {
        let (_dir, mana_dir) = setup_test_env();

        let mut unit = Unit::new("1", "Retry Task");
        unit.attempts = 2;
        unit.notes = Some("Tried X, failed due to Y.".to_string());
        write_test_unit(&mana_dir, &unit);

        let options = PromptOptions {
            mana_dir: mana_dir.clone(),
            instructions: None,
            concurrent_overlaps: None,
        };

        let result = build_agent_prompt(&unit, &options).unwrap();
        assert!(result.system_prompt.contains("Previous Attempts"));
        assert!(result.system_prompt.contains("Tried X"));
        assert!(result.system_prompt.contains("Do NOT repeat"));
    }

    #[test]
    fn build_prompt_no_verify() {
        let (_dir, mana_dir) = setup_test_env();

        let unit = Unit::new("1", "No Verify");
        write_test_unit(&mana_dir, &unit);

        let options = PromptOptions {
            mana_dir: mana_dir.clone(),
            instructions: None,
            concurrent_overlaps: None,
        };

        let result = build_agent_prompt(&unit, &options).unwrap();
        // Should not have pre-flight check
        assert!(!result.system_prompt.contains("Pre-flight Check"));
        // Verify gate should say no command
        assert!(result.system_prompt.contains("No verify command"));
    }

    #[test]
    fn build_prompt_with_file_references() {
        let (dir, mana_dir) = setup_test_env();
        let project_dir = dir.path();

        // Create source files
        let src = project_dir.join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("lib.rs"), "pub mod utils;").unwrap();
        fs::write(src.join("utils.rs"), "pub fn helper() {}").unwrap();

        let mut unit = Unit::new("1", "Task");
        unit.description =
            Some("Modify src/lib.rs to export new module\nRead src/utils.rs".to_string());
        write_test_unit(&mana_dir, &unit);

        let options = PromptOptions {
            mana_dir: mana_dir.clone(),
            instructions: None,
            concurrent_overlaps: None,
        };

        let result = build_agent_prompt(&unit, &options).unwrap();
        assert!(result.system_prompt.contains("Referenced Files"));
        assert!(result.system_prompt.contains("src/lib.rs"));
        assert!(result.system_prompt.contains("pub mod utils;"));
    }

    #[test]
    fn build_prompt_section_order() {
        let (dir, mana_dir) = setup_test_env();
        let project_dir = dir.path();

        // Write rules
        fs::write(mana_dir.join("RULES.md"), "# Rules\nBe nice.").unwrap();

        // Create parent
        let mut parent = Unit::new("1", "Parent");
        parent.description = Some("Parent goal.".to_string());
        write_test_unit(&mana_dir, &parent);

        // Create source file
        let src = project_dir.join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("main.rs"), "fn main() {}").unwrap();

        // Create child unit with all features
        let mut unit = Unit::new("1.1", "Child Task");
        unit.parent = Some("1".to_string());
        unit.description = Some("Modify src/main.rs".to_string());
        unit.acceptance = Some("Tests pass".to_string());
        unit.verify = Some("cargo test unit::check".to_string());
        unit.attempts = 1;
        unit.notes = Some("Tried something".to_string());
        write_test_unit(&mana_dir, &unit);

        let _ = Index::build(&mana_dir).unwrap().save(&mana_dir);

        let options = PromptOptions {
            mana_dir: mana_dir.clone(),
            instructions: None,
            concurrent_overlaps: None,
        };

        let result = build_agent_prompt(&unit, &options).unwrap();
        let prompt = &result.system_prompt;

        // Verify section ordering by finding positions
        let rules_pos = prompt.find("# Project Rules").unwrap();
        let parent_pos = prompt.find("# Parent Context").unwrap();
        let assignment_pos = prompt.find("# Unit Assignment").unwrap();
        let files_pos = prompt.find("# Referenced Files").unwrap();
        let acceptance_pos = prompt.find("# Acceptance Criteria").unwrap();
        let preflight_pos = prompt.find("# Pre-flight Check").unwrap();
        let attempts_pos = prompt.find("# Previous Attempts").unwrap();
        let approach_pos = prompt.find("# Approach").unwrap();
        let verify_pos = prompt.find("# Verify Gate").unwrap();
        let constraints_pos = prompt.find("# Constraints").unwrap();
        let tools_pos = prompt.find("# Tool Strategy").unwrap();

        assert!(rules_pos < parent_pos, "Rules before Parent");
        assert!(parent_pos < assignment_pos, "Parent before Assignment");
        assert!(assignment_pos < files_pos, "Assignment before Files");
        assert!(files_pos < acceptance_pos, "Files before Acceptance");
        assert!(
            acceptance_pos < preflight_pos,
            "Acceptance before Preflight"
        );
        assert!(preflight_pos < attempts_pos, "Preflight before Attempts");
        assert!(attempts_pos < approach_pos, "Attempts before Approach");
        assert!(approach_pos < verify_pos, "Approach before Verify");
        assert!(verify_pos < constraints_pos, "Verify before Constraints");
        assert!(constraints_pos < tools_pos, "Constraints before Tools");
    }
}
