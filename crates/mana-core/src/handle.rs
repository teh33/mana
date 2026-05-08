use std::collections::HashSet;

const STOP_WORDS: &[&str] = &[
    "a",
    "an",
    "and",
    "are",
    "as",
    "at",
    "be",
    "by",
    "for",
    "from",
    "has",
    "have",
    "in",
    "into",
    "is",
    "it",
    "its",
    "of",
    "on",
    "or",
    "over",
    "than",
    "that",
    "the",
    "this",
    "to",
    "under",
    "with",
    "without",
    "via",
    "after",
    "before",
    "beyond",
    "across",
    "while",
    "when",
    "where",
    "why",
    "how",
    "then",
    "now",
    "current",
    "existing",
    "old",
    "new",
    "next",
    "previous",
    "future",
    "first",
    "second",
    "third",
    "v1",
    "v2",
    "v3",
    "phase",
    "slice",
    "task",
    "unit",
    "epic",
    "goal",
    "feature",
    "implement",
    "implementation",
    "add",
    "adds",
    "added",
    "make",
    "create",
    "define",
    "plan",
    "fix",
    "support",
    "wire",
    "use",
    "using",
    "based",
    "native",
    "canonical",
    "durable",
    "explicit",
    "specific",
    "clean",
    "project",
    "mana",
    "imp",
    "agent",
    "agents",
    "workflow",
    "work",
    "system",
];

/// Generate a short project-scoped human handle from a unit title.
///
/// Handles are navigation aliases, not canonical identity. The generator keeps
/// the first three meaningful title words after removing common stop words and
/// noisy planning vocabulary, falling back to title words when a short/generic
/// title has fewer than three meaningful words.
pub fn generate_handle(title: &str) -> Option<String> {
    let tokens = tokenize_title(title);
    if tokens.is_empty() {
        return None;
    }

    let stop_words: HashSet<&str> = STOP_WORDS.iter().copied().collect();
    let meaningful: Vec<String> = tokens
        .iter()
        .filter(|word| !stop_words.contains(word.as_str()))
        .filter(|word| !word.chars().all(|c| c.is_ascii_digit()))
        .cloned()
        .collect();

    let mut selected = Vec::new();
    for word in meaningful.iter().chain(tokens.iter()) {
        if !selected.contains(word) && !word.chars().all(|c| c.is_ascii_digit()) {
            selected.push(word.clone());
        }
        if selected.len() == 3 {
            break;
        }
    }

    if selected.is_empty() {
        None
    } else {
        Some(selected.join(" "))
    }
}

/// Normalize a handle or user-provided handle query for exact matching.
pub fn normalize_handle(value: &str) -> String {
    tokenize_title(value).join(" ")
}

fn tokenize_title(title: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();

    for ch in title.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            current.push(ch);
        } else if !current.is_empty() {
            if keep_word(&current) {
                words.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
        }
    }

    if !current.is_empty() && keep_word(&current) {
        words.push(current);
    }

    words
}

fn keep_word(word: &str) -> bool {
    word.len() > 1 || word.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_generation_keeps_three_meaningful_words() {
        assert_eq!(
            generate_handle("Implement SQLite-derived index for mana agent context assembly")
                .as_deref(),
            Some("sqlite derived index")
        );
    }

    #[test]
    fn handle_generation_falls_back_for_short_titles() {
        assert_eq!(
            generate_handle("Onboarding improvements").as_deref(),
            Some("onboarding improvements")
        );
    }

    #[test]
    fn handle_generation_ignores_punctuation_and_numbers_when_possible() {
        assert_eq!(
            generate_handle("Vibecheck: Improve crates/uu/src/cmd/doctor.rs").as_deref(),
            Some("vibecheck improve crates")
        );
    }

    #[test]
    fn normalize_handle_matches_generated_shape() {
        assert_eq!(
            normalize_handle("SQLite-Derived Index"),
            "sqlite derived index"
        );
    }
}
