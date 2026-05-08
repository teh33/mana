//! Utility functions for unit ID parsing and status conversion.

use crate::unit::Status;
use anyhow::{Context, Result};
use std::path::Path;
use std::str::FromStr;

/// Validate a unit ID to prevent path traversal attacks.
///
/// Valid IDs match the pattern: ^[a-zA-Z0-9._-]+$
/// This prevents directory escape attacks like "../../../etc/passwd".
///
/// # Examples
/// - "1" ✓ (valid)
/// - "3.2.1" ✓ (valid)
/// - "my-task" ✓ (valid)
/// - "task_v1.0" ✓ (valid)
/// - "../etc/passwd" ✗ (invalid)
/// - "task/../escape" ✗ (invalid)
pub fn validate_unit_id(id: &str) -> Result<()> {
    if id.is_empty() {
        return Err(anyhow::anyhow!("Unit ID cannot be empty"));
    }

    if id.len() > 255 {
        return Err(anyhow::anyhow!("Unit ID too long (max 255 characters)"));
    }

    // Check that ID only contains safe characters: alphanumeric, dots, underscores, hyphens
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        return Err(anyhow::anyhow!(
            "Invalid unit ID '{}': must contain only alphanumeric characters, dots, underscores, and hyphens",
            id
        ));
    }

    // Ensure no path traversal sequences
    if id.contains("..") {
        return Err(anyhow::anyhow!(
            "Invalid unit ID '{}': cannot contain '..' (path traversal protection)",
            id
        ));
    }

    Ok(())
}

/// A segment of a dot-separated ID, either numeric or alphanumeric.
/// Numeric segments sort before alpha segments when compared.
#[derive(Debug, Clone, PartialEq, Eq)]
enum IdSegment {
    Num(u64),
    Alpha(String),
}

impl PartialOrd for IdSegment {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for IdSegment {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (IdSegment::Num(a), IdSegment::Num(b)) => a.cmp(b),
            (IdSegment::Alpha(a), IdSegment::Alpha(b)) => a.cmp(b),
            // Numeric segments sort before alpha segments
            (IdSegment::Num(_), IdSegment::Alpha(_)) => std::cmp::Ordering::Less,
            (IdSegment::Alpha(_), IdSegment::Num(_)) => std::cmp::Ordering::Greater,
        }
    }
}

/// Compare two unit IDs using natural ordering.
/// Parses IDs as dot-separated segments and compares them.
/// Numeric segments are compared numerically, alpha segments lexicographically.
/// Numeric segments sort before alpha segments.
///
/// # Examples
/// - "1" < "2" (numeric comparison)
/// - "1" < "10" (numeric comparison, not string comparison)
/// - "3.1" < "3.2" (multi-level comparison)
/// - "abc" < "def" (alpha comparison)
/// - "1" < "abc" (numeric sorts before alpha)
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let sa = parse_id_segments(a);
    let sb = parse_id_segments(b);
    sa.cmp(&sb)
}

/// Parse a dot-separated ID into segments.
///
/// Each segment is parsed as numeric (u64) if possible, otherwise kept as a string.
/// Used for natural ID comparison.
///
/// # Examples
/// - "1" → [Num(1)]
/// - "3.1" → [Num(3), Num(1)]
/// - "my-task" → [Alpha("my-task")]
/// - "1.abc.2" → [Num(1), Alpha("abc"), Num(2)]
fn parse_id_segments(id: &str) -> Vec<IdSegment> {
    id.split('.')
        .map(|seg| match seg.parse::<u64>() {
            Ok(n) => IdSegment::Num(n),
            Err(_) => IdSegment::Alpha(seg.to_string()),
        })
        .collect()
}

/// Convert a status string to a Status enum, or None if invalid.
///
/// Valid inputs: "open", "in_progress", "closed"
pub fn parse_status(s: &str) -> Option<Status> {
    match s {
        "open" => Some(Status::Open),
        "in_progress" => Some(Status::InProgress),
        "closed" => Some(Status::Closed),
        _ => None,
    }
}

/// Implement FromStr for Status to support standard parsing.
impl FromStr for Status {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_status(s).ok_or_else(|| format!("Invalid status: {}", s))
    }
}

/// Convert a unit title into a URL-safe kebab-case slug for use in filenames.
///
/// Algorithm:
/// 1. Trim whitespace
/// 2. Lowercase all characters
/// 3. Replace spaces with hyphens
/// 4. Remove non-alphanumeric characters except hyphens
/// 5. Collapse consecutive hyphens into single hyphen
/// 6. Remove leading/trailing hyphens
/// 7. Truncate to 50 characters
/// 8. Return "unnamed" if empty
///
/// # Examples
/// - "My Task" → "my-task"
/// - "Build API v2.0" → "build-api-v20"
/// - "Foo   Bar" → "foo-bar"
/// - "Implement `mana show` to render Markdown" → "implement-mana-show-to-render-markdown"
/// - "Update Unit parser to read .md + YAML frontmatter" → "update-unit-parser-to-read-md-yaml-frontmatter"
/// - "My-Task!!!" → "my-task"
/// - "   Spaces   " → "spaces"
/// - "" (empty) → "unnamed"
/// - "a" (single char) → "a"
pub fn title_to_slug(title: &str) -> String {
    // Step 1: Trim whitespace
    let trimmed = title.trim();

    // Step 2: Lowercase all characters
    let lowercased = trimmed.to_lowercase();

    // Step 3 & 4: Replace spaces with hyphens and remove non-alphanumeric (except hyphens)
    let mut slug = String::new();
    for c in lowercased.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if c.is_whitespace() || c == '-' {
            slug.push('-');
        }
        // Skip all other characters (special chars, punctuation, etc.)
    }

    // Step 5: Collapse consecutive hyphens into single hyphen
    let slug = slug.chars().fold(String::new(), |mut acc, c| {
        if c == '-' && acc.ends_with('-') {
            acc
        } else {
            acc.push(c);
            acc
        }
    });

    // Step 6: Remove leading/trailing hyphens
    let slug = slug.trim_matches('-').to_string();

    // Step 7: Truncate to 50 characters and re-trim hyphens
    let slug = if slug.len() > 50 {
        slug.chars()
            .take(50)
            .collect::<String>()
            .trim_end_matches('-')
            .to_string()
    } else {
        slug
    };

    // Step 8: Return "unnamed" if empty
    if slug.is_empty() {
        "unnamed".to_string()
    } else {
        slug
    }
}

/// Normalize a title for similarity comparison.
///
/// Lowercases, strips punctuation, and splits into a set of words.
/// Common short words (stop words) are removed to focus on meaningful content.
fn normalize_title_words(title: &str) -> Vec<String> {
    let stop_words: &[&str] = &[
        "a", "an", "the", "to", "in", "on", "of", "for", "and", "or", "is", "it", "by", "at", "be",
        "do", "up", "as", "so", "if", "no", "not", "but", "all", "can", "had", "has", "was", "are",
        "its", "may", "our", "out", "own", "too", "use", "via", "way", "yet", "with", "from",
        "that", "this", "into", "when", "will", "been", "have", "each", "make", "than", "them",
        "then", "some",
    ];

    let lowered = title.to_lowercase();
    lowered
        .split(|c: char| !c.is_ascii_alphanumeric())
        .map(|w| w.trim())
        .filter(|w| !w.is_empty() && w.len() > 1 && !stop_words.contains(w))
        .map(|w| w.to_string())
        .collect()
}

/// Compute word-overlap similarity between two titles.
///
/// Returns a value between 0.0 (no overlap) and 1.0 (identical words).
/// Uses Jaccard-like similarity: |intersection| / |smaller set|.
/// Dividing by the smaller set means "Fix auth" matches "Fix authentication timeout handling"
/// at a high score even though one title has more words.
pub fn title_similarity(a: &str, b: &str) -> f64 {
    let words_a = normalize_title_words(a);
    let words_b = normalize_title_words(b);

    if words_a.is_empty() || words_b.is_empty() {
        return 0.0;
    }

    let intersection = words_a.iter().filter(|w| words_b.contains(w)).count();
    let min_len = words_a.len().min(words_b.len());

    intersection as f64 / min_len as f64
}

/// A similar unit found during duplicate detection.
#[derive(Debug, Clone)]
pub struct SimilarUnit {
    pub id: String,
    pub title: String,
    pub score: f64,
}

/// Find open/in-progress units with titles similar to the given title.
///
/// Returns units whose title similarity exceeds the threshold (default 0.7).
/// Only checks units with status Open or InProgress.
pub fn find_similar_titles(
    index: &crate::index::Index,
    new_title: &str,
    threshold: f64,
) -> Vec<SimilarUnit> {
    let mut matches = Vec::new();

    for entry in &index.units {
        if entry.status != Status::Open && entry.status != Status::InProgress {
            continue;
        }

        let score = title_similarity(new_title, &entry.title);
        if score >= threshold {
            matches.push(SimilarUnit {
                id: entry.id.clone(),
                title: entry.title.clone(),
                score,
            });
        }
    }

    // Sort by score descending
    matches.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    matches
}

/// Default similarity threshold for duplicate detection (70% word overlap).
pub const DEFAULT_SIMILARITY_THRESHOLD: f64 = 0.7;

/// Write contents to a file atomically using write-to-temp + rename.
///
/// Writes to a temporary file in the same directory as `path`, then renames
/// it to the target. `rename()` is atomic on POSIX when source and destination
/// are on the same filesystem (guaranteed here since we use the same directory).
/// The temp file is cleaned up on error.
pub fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    let tmp_path = path.with_extension(format!("tmp.{}", std::process::id()));

    // Write to temp file; clean up on failure
    if let Err(e) = std::fs::write(&tmp_path, contents) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e)
            .with_context(|| format!("Failed to write temp file: {}", tmp_path.display()));
    }

    // Atomic rename; clean up temp on failure
    if let Err(e) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e).with_context(|| {
            format!(
                "Failed to rename {} -> {}",
                tmp_path.display(),
                path.display()
            )
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- title_to_slug tests ----------

    #[test]
    fn title_to_slug_simple_case() {
        assert_eq!(title_to_slug("My Task"), "my-task");
    }

    #[test]
    fn title_to_slug_with_numbers_and_dots() {
        assert_eq!(title_to_slug("Build API v2.0"), "build-api-v20");
    }

    #[test]
    fn title_to_slug_multiple_spaces() {
        assert_eq!(title_to_slug("Foo   Bar"), "foo-bar");
    }

    #[test]
    fn title_to_slug_with_backticks() {
        assert_eq!(
            title_to_slug("Implement `mana show` to render Markdown"),
            "implement-mana-show-to-render-markdown"
        );
    }

    #[test]
    fn title_to_slug_with_special_chars() {
        assert_eq!(
            title_to_slug("Update Unit parser to read .md + YAML frontmatter"),
            "update-unit-parser-to-read-md-yaml-frontmatter"
        );
    }

    #[test]
    fn title_to_slug_with_exclamation() {
        assert_eq!(title_to_slug("My-Task!!!"), "my-task");
    }

    #[test]
    fn title_to_slug_leading_trailing_spaces() {
        assert_eq!(title_to_slug("   Spaces   "), "spaces");
    }

    #[test]
    fn title_to_slug_empty_string() {
        assert_eq!(title_to_slug(""), "unnamed");
    }

    #[test]
    fn title_to_slug_single_character() {
        assert_eq!(title_to_slug("a"), "a");
        assert_eq!(title_to_slug("Z"), "z");
    }

    #[test]
    fn title_to_slug_only_spaces() {
        assert_eq!(title_to_slug("   "), "unnamed");
    }

    #[test]
    fn title_to_slug_only_special_chars() {
        assert_eq!(title_to_slug("!!!@@@###"), "unnamed");
    }

    #[test]
    fn title_to_slug_truncate_50_chars() {
        let long_title = "a".repeat(60);
        let result = title_to_slug(&long_title);
        assert_eq!(result, "a".repeat(50));
        assert_eq!(result.len(), 50);
    }

    #[test]
    fn title_to_slug_truncate_with_hyphens() {
        let title = "word ".repeat(20); // Creates long string with hyphens after truncation
        let result = title_to_slug(&title);
        assert!(result.len() <= 50);
    }

    #[test]
    fn title_to_slug_mixed_case() {
        assert_eq!(
            title_to_slug("ThIs Is A MiXeD CaSe TiTle"),
            "this-is-a-mixed-case-title"
        );
    }

    #[test]
    fn title_to_slug_numbers_preserved() {
        assert_eq!(
            title_to_slug("Task 123 Version 4.5.6"),
            "task-123-version-456"
        );
    }

    #[test]
    fn title_to_slug_consecutive_hyphens() {
        assert_eq!(title_to_slug("foo---bar"), "foo-bar");
        assert_eq!(title_to_slug("foo - - bar"), "foo-bar");
    }

    #[test]
    fn title_to_slug_unicode_removed() {
        // Unicode characters are not ASCII alphanumeric, so they get removed
        assert_eq!(title_to_slug("café"), "caf");
        assert_eq!(title_to_slug("naïve"), "nave");
    }

    #[test]
    fn title_to_slug_all_whitespace_types() {
        assert_eq!(title_to_slug("foo\tbar\nbaz"), "foo-bar-baz");
    }

    #[test]
    fn title_to_slug_exactly_50_chars() {
        let title = "a".repeat(50);
        assert_eq!(title_to_slug(&title), title);
    }

    // ---------- natural_cmp tests ----------

    #[test]
    fn natural_cmp_single_digit() {
        assert_eq!(natural_cmp("1", "2"), std::cmp::Ordering::Less);
        assert_eq!(natural_cmp("2", "1"), std::cmp::Ordering::Greater);
        assert_eq!(natural_cmp("1", "1"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn natural_cmp_multi_digit() {
        assert_eq!(natural_cmp("1", "10"), std::cmp::Ordering::Less);
        assert_eq!(natural_cmp("10", "1"), std::cmp::Ordering::Greater);
        assert_eq!(natural_cmp("10", "10"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn natural_cmp_multi_level() {
        assert_eq!(natural_cmp("3.1", "3.2"), std::cmp::Ordering::Less);
        assert_eq!(natural_cmp("3.2", "3.1"), std::cmp::Ordering::Greater);
        assert_eq!(natural_cmp("3.1", "3.1"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn natural_cmp_three_level() {
        assert_eq!(natural_cmp("3.2.1", "3.2.2"), std::cmp::Ordering::Less);
        assert_eq!(natural_cmp("3.2.2", "3.2.1"), std::cmp::Ordering::Greater);
        assert_eq!(natural_cmp("3.2.1", "3.2.1"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn natural_cmp_different_prefix() {
        assert_eq!(natural_cmp("2.1", "3.1"), std::cmp::Ordering::Less);
        assert_eq!(natural_cmp("10.5", "9.99"), std::cmp::Ordering::Greater);
    }

    // ---------- parse_id_segments tests ----------

    #[test]
    fn parse_id_segments_single() {
        assert_eq!(parse_id_segments("1"), vec![IdSegment::Num(1)]);
        assert_eq!(parse_id_segments("42"), vec![IdSegment::Num(42)]);
    }

    #[test]
    fn parse_id_segments_multi_level() {
        assert_eq!(
            parse_id_segments("1.2"),
            vec![IdSegment::Num(1), IdSegment::Num(2)]
        );
        assert_eq!(
            parse_id_segments("3.2.1"),
            vec![IdSegment::Num(3), IdSegment::Num(2), IdSegment::Num(1)]
        );
    }

    #[test]
    fn parse_id_segments_leading_zeros() {
        // Leading zeros are parsed as decimal, not octal
        assert_eq!(parse_id_segments("01"), vec![IdSegment::Num(1)]);
        assert_eq!(
            parse_id_segments("03.02"),
            vec![IdSegment::Num(3), IdSegment::Num(2)]
        );
    }

    #[test]
    fn parse_id_segments_alpha() {
        assert_eq!(
            parse_id_segments("abc"),
            vec![IdSegment::Alpha("abc".to_string())]
        );
        assert_eq!(
            parse_id_segments("1.abc.2"),
            vec![
                IdSegment::Num(1),
                IdSegment::Alpha("abc".to_string()),
                IdSegment::Num(2)
            ]
        );
    }

    #[test]
    fn natural_cmp_alpha_ids() {
        // Alpha IDs should not all compare equal
        assert_eq!(natural_cmp("abc", "def"), std::cmp::Ordering::Less);
        assert_eq!(natural_cmp("def", "abc"), std::cmp::Ordering::Greater);
        assert_eq!(natural_cmp("abc", "abc"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn natural_cmp_numeric_before_alpha() {
        assert_eq!(natural_cmp("1", "abc"), std::cmp::Ordering::Less);
        assert_eq!(natural_cmp("abc", "1"), std::cmp::Ordering::Greater);
    }

    #[test]
    fn natural_cmp_mixed_segments() {
        // "1.abc.2" vs "1.abc.3" — third segment differs
        assert_eq!(natural_cmp("1.abc.2", "1.abc.3"), std::cmp::Ordering::Less);
        // "1.abc" vs "1.def" — second segment differs
        assert_eq!(natural_cmp("1.abc", "1.def"), std::cmp::Ordering::Less);
    }

    // ---------- parse_status tests ----------

    #[test]
    fn parse_status_valid_open() {
        assert_eq!(parse_status("open"), Some(Status::Open));
    }

    #[test]
    fn parse_status_valid_in_progress() {
        assert_eq!(parse_status("in_progress"), Some(Status::InProgress));
    }

    #[test]
    fn parse_status_valid_closed() {
        assert_eq!(parse_status("closed"), Some(Status::Closed));
    }

    #[test]
    fn parse_status_invalid() {
        assert_eq!(parse_status("invalid"), None);
        assert_eq!(parse_status(""), None);
        assert_eq!(parse_status("OPEN"), None);
        assert_eq!(parse_status("Closed"), None);
    }

    #[test]
    fn parse_status_whitespace() {
        assert_eq!(parse_status("open "), None);
        assert_eq!(parse_status(" open"), None);
    }

    // ---------- Status::FromStr tests ----------

    #[test]
    fn status_from_str_open() {
        assert_eq!("open".parse::<Status>(), Ok(Status::Open));
    }

    #[test]
    fn status_from_str_in_progress() {
        assert_eq!("in_progress".parse::<Status>(), Ok(Status::InProgress));
    }

    #[test]
    fn status_from_str_closed() {
        assert_eq!("closed".parse::<Status>(), Ok(Status::Closed));
    }

    #[test]
    fn status_from_str_invalid() {
        assert!("invalid".parse::<Status>().is_err());
        assert!("".parse::<Status>().is_err());
    }

    // ---------- validate_unit_id tests ----------

    #[test]
    fn validate_unit_id_simple_numeric() {
        assert!(validate_unit_id("1").is_ok());
        assert!(validate_unit_id("42").is_ok());
        assert!(validate_unit_id("999").is_ok());
    }

    #[test]
    fn validate_unit_id_dotted() {
        assert!(validate_unit_id("3.1").is_ok());
        assert!(validate_unit_id("3.2.1").is_ok());
        assert!(validate_unit_id("1.2.3.4.5").is_ok());
    }

    #[test]
    fn validate_unit_id_with_underscores() {
        assert!(validate_unit_id("task_1").is_ok());
        assert!(validate_unit_id("my_task_v1").is_ok());
    }

    #[test]
    fn validate_unit_id_with_hyphens() {
        assert!(validate_unit_id("my-task").is_ok());
        assert!(validate_unit_id("task-v1-0").is_ok());
    }

    #[test]
    fn validate_unit_id_alphanumeric() {
        assert!(validate_unit_id("abc123def").is_ok());
        assert!(validate_unit_id("Task1").is_ok());
    }

    #[test]
    fn validate_unit_id_empty_fails() {
        assert!(validate_unit_id("").is_err());
    }

    #[test]
    fn validate_unit_id_path_traversal_fails() {
        assert!(validate_unit_id("../etc/passwd").is_err());
        assert!(validate_unit_id("..").is_err());
        assert!(validate_unit_id("foo/../bar").is_err());
        assert!(validate_unit_id("task..escape").is_err());
    }

    #[test]
    fn validate_unit_id_absolute_path_fails() {
        assert!(validate_unit_id("/etc/passwd").is_err());
    }

    #[test]
    fn validate_unit_id_spaces_fail() {
        assert!(validate_unit_id("my task").is_err());
        assert!(validate_unit_id(" 1").is_err());
        assert!(validate_unit_id("1 ").is_err());
    }

    #[test]
    fn validate_unit_id_special_chars_fail() {
        assert!(validate_unit_id("task@home").is_err());
        assert!(validate_unit_id("task#1").is_err());
        assert!(validate_unit_id("task$money").is_err());
        assert!(validate_unit_id("task%complete").is_err());
        assert!(validate_unit_id("task&friend").is_err());
        assert!(validate_unit_id("task*star").is_err());
        assert!(validate_unit_id("task(paren").is_err());
        assert!(validate_unit_id("task)close").is_err());
        assert!(validate_unit_id("task+plus").is_err());
        assert!(validate_unit_id("task=equals").is_err());
        assert!(validate_unit_id("task[bracket").is_err());
        assert!(validate_unit_id("task]close").is_err());
        assert!(validate_unit_id("task{brace").is_err());
        assert!(validate_unit_id("task}close").is_err());
        assert!(validate_unit_id("task|pipe").is_err());
        assert!(validate_unit_id("task;semicolon").is_err());
        assert!(validate_unit_id("task:colon").is_err());
        assert!(validate_unit_id("task\"quote").is_err());
        assert!(validate_unit_id("task'apostrophe").is_err());
        assert!(validate_unit_id("task<less").is_err());
        assert!(validate_unit_id("task>greater").is_err());
        assert!(validate_unit_id("task,comma").is_err());
        assert!(validate_unit_id("task?question").is_err());
    }

    #[test]
    fn validate_unit_id_too_long() {
        let long_id = "a".repeat(256);
        assert!(validate_unit_id(&long_id).is_err());

        let max_id = "a".repeat(255);
        assert!(validate_unit_id(&max_id).is_ok());
    }

    // ---------- atomic_write tests ----------

    #[test]
    fn test_atomic_write_creates_file_with_correct_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.yaml");

        atomic_write(&path, "hello: world\n").unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "hello: world\n");
    }

    #[test]
    fn test_atomic_write_overwrites_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.yaml");

        std::fs::write(&path, "old content").unwrap();
        atomic_write(&path, "new content").unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "new content");
    }

    #[test]
    fn test_atomic_write_no_temp_file_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.yaml");

        atomic_write(&path, "data").unwrap();

        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1, "only the target file should exist");
        assert_eq!(entries[0].file_name().to_str().unwrap(), "test.yaml");
    }

    // ---------- title_similarity tests ----------

    #[test]
    fn similarity_identical_titles() {
        assert!(
            (title_similarity("Fix auth timeout", "Fix auth timeout") - 1.0).abs() < f64::EPSILON
        );
    }

    #[test]
    fn similarity_close_titles() {
        // "Fix auth timeout" vs "Fix authentication timeout handling"
        // Normalized: ["fix", "auth", "timeout"] vs ["fix", "authentication", "timeout", "handling"]
        // "auth" != "authentication" so intersection = {"fix", "timeout"} = 2
        // min_len = 3 → 2/3 ≈ 0.67
        let score = title_similarity("Fix auth timeout", "Fix authentication timeout handling");
        assert!(score > 0.5, "Expected > 0.5, got {}", score);
    }

    #[test]
    fn similarity_very_different_titles() {
        let score = title_similarity("Fix auth timeout", "Add database migration");
        assert!(score < 0.3, "Expected < 0.3, got {}", score);
    }

    #[test]
    fn similarity_empty_title() {
        assert!((title_similarity("", "Something")).abs() < f64::EPSILON);
        assert!((title_similarity("Something", "")).abs() < f64::EPSILON);
    }

    #[test]
    fn similarity_case_insensitive() {
        let score = title_similarity("Fix Auth Timeout", "fix auth timeout");
        assert!((score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn similarity_ignores_stop_words() {
        // "Add a new feature" normalized: ["add", "new", "feature"]
        // "Add the new feature" normalized: ["add", "new", "feature"]
        let score = title_similarity("Add a new feature", "Add the new feature");
        assert!((score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn similarity_strips_punctuation() {
        let score = title_similarity("Fix: auth timeout!", "Fix auth timeout");
        assert!((score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn similarity_subset_match_scores_high() {
        // "Fix auth" vs "Fix auth timeout" → intersection = {fix, auth} = 2, min_len = 2 → 1.0
        let score = title_similarity("Fix auth", "Fix auth timeout");
        assert!((score - 1.0).abs() < f64::EPSILON);
    }

    // ---------- find_similar_titles tests ----------

    #[test]
    fn find_similar_returns_matches_above_threshold() {
        use crate::index::{Index, IndexEntry};
        use chrono::Utc;

        let index = Index {
            units: vec![
                IndexEntry {
                    id: "1".to_string(),
                    title: "Fix auth timeout".to_string(),
                    handle: None,
                    status: Status::Open,
                    priority: 2,
                    parent: None,
                    dependencies: vec![],
                    labels: vec![],
                    assignee: None,
                    updated_at: Utc::now(),
                    produces: vec![],
                    requires: vec![],
                    has_verify: false,
                    verify: None,
                    created_at: Utc::now(),
                    claimed_by: None,
                    attempts: 0,
                    paths: vec![],
                    kind: crate::unit::UnitType::Task,
                    feature: false,
                    has_decisions: false,
                },
                IndexEntry {
                    id: "2".to_string(),
                    title: "Add database migration".to_string(),
                    handle: None,
                    status: Status::Open,
                    priority: 2,
                    parent: None,
                    dependencies: vec![],
                    labels: vec![],
                    assignee: None,
                    updated_at: Utc::now(),
                    produces: vec![],
                    requires: vec![],
                    has_verify: false,
                    verify: None,
                    created_at: Utc::now(),
                    claimed_by: None,
                    attempts: 0,
                    paths: vec![],
                    kind: crate::unit::UnitType::Task,
                    feature: false,
                    has_decisions: false,
                },
            ],
        };

        let matches = find_similar_titles(&index, "Fix auth timeout handling", 0.7);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, "1");
    }

    #[test]
    fn find_similar_skips_closed_units() {
        use crate::index::{Index, IndexEntry};
        use chrono::Utc;

        let index = Index {
            units: vec![IndexEntry {
                handle: None,
                id: "1".to_string(),
                title: "Fix auth timeout".to_string(),
                status: Status::Closed,
                priority: 2,
                parent: None,
                dependencies: vec![],
                labels: vec![],
                assignee: None,
                updated_at: Utc::now(),
                produces: vec![],
                requires: vec![],
                has_verify: false,
                verify: None,
                created_at: Utc::now(),
                claimed_by: None,
                attempts: 0,
                paths: vec![],
                kind: crate::unit::UnitType::Task,
                feature: false,
                has_decisions: false,
            }],
        };

        let matches = find_similar_titles(&index, "Fix auth timeout", 0.7);
        assert!(matches.is_empty());
    }

    #[test]
    fn find_similar_returns_empty_when_no_match() {
        use crate::index::{Index, IndexEntry};
        use chrono::Utc;

        let index = Index {
            units: vec![IndexEntry {
                handle: None,
                id: "1".to_string(),
                title: "Fix auth timeout".to_string(),
                status: Status::Open,
                priority: 2,
                parent: None,
                dependencies: vec![],
                labels: vec![],
                assignee: None,
                updated_at: Utc::now(),
                produces: vec![],
                requires: vec![],
                has_verify: false,
                verify: None,
                created_at: Utc::now(),
                claimed_by: None,
                attempts: 0,
                paths: vec![],
                kind: crate::unit::UnitType::Task,
                feature: false,
                has_decisions: false,
            }],
        };

        let matches = find_similar_titles(&index, "Add database migration", 0.7);
        assert!(matches.is_empty());
    }
}
