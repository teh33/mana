//! Review state persistence in `.mana/`.
//!
//! Reviews are stored as YAML files in `.mana/reviews/`.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::types::Review;
use mana_core::yaml;

/// Directory within `.mana/` where reviews are stored.
const REVIEWS_DIR: &str = "reviews";

/// Get the reviews directory path.
fn reviews_dir(mana_dir: &Path) -> PathBuf {
    mana_dir.join(REVIEWS_DIR)
}

/// Get the file path for a specific unit's review.
fn review_path(mana_dir: &Path, unit_id: &str) -> PathBuf {
    let safe_id = unit_id.replace('.', "-");
    reviews_dir(mana_dir).join(format!("{safe_id}.yaml"))
}

/// Save a review to `.mana/reviews/`.
pub fn save(mana_dir: &Path, review: &Review) -> Result<()> {
    let dir = reviews_dir(mana_dir);
    fs::create_dir_all(&dir).context("failed to create reviews directory")?;

    let path = review_path(mana_dir, &review.unit_id);
    let yaml = serde_yml::to_string(review).context("failed to serialize review")?;
    fs::write(&path, yaml).context("failed to write review file")?;

    Ok(())
}

/// Load the review for a specific unit, if one exists.
pub fn load(mana_dir: &Path, unit_id: &str) -> Result<Option<Review>> {
    let path = review_path(mana_dir, unit_id);

    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&path).context("failed to read review file")?;
    let review: Review = yaml::from_str(&content).context("failed to parse review")?;

    Ok(Some(review))
}

/// Load all reviews in the `.mana/reviews/` directory.
pub fn load_all(mana_dir: &Path) -> Result<Vec<Review>> {
    let dir = reviews_dir(mana_dir);

    if !dir.exists() {
        return Ok(vec![]);
    }

    let mut reviews = Vec::new();

    for entry in fs::read_dir(&dir).context("failed to read reviews directory")? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().is_some_and(|ext| ext == "yaml") {
            let content = fs::read_to_string(&path)?;
            if let Ok(review) = yaml::from_str::<Review>(&content) {
                reviews.push(review);
            }
        }
    }

    Ok(reviews)
}

/// Check if a unit has been reviewed.
pub fn has_review(mana_dir: &Path, unit_id: &str) -> bool {
    review_path(mana_dir, unit_id).exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;
    use chrono::Utc;
    use tempfile::TempDir;

    fn make_review(unit_id: &str) -> Review {
        Review {
            unit_id: unit_id.into(),
            attempt: 1,
            decision: ReviewDecision::Approved,
            summary: Some("Looks good".into()),
            annotations: vec![],
            reviewed_at: Utc::now(),
            reviewer: "human".into(),
        }
    }

    #[test]
    fn save_and_load_review() {
        let tmp = TempDir::new().unwrap();
        let mana_dir = tmp.path().join(".mana");
        fs::create_dir_all(&mana_dir).unwrap();

        let review = make_review("1.3");
        save(&mana_dir, &review).unwrap();

        let loaded = load(&mana_dir, "1.3").unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.unit_id, "1.3");
        assert_eq!(loaded.decision, ReviewDecision::Approved);
    }

    #[test]
    fn load_nonexistent_returns_none() {
        let tmp = TempDir::new().unwrap();
        let mana_dir = tmp.path().join(".mana");
        fs::create_dir_all(&mana_dir).unwrap();

        let loaded = load(&mana_dir, "999").unwrap();
        assert!(loaded.is_none());
    }

    #[test]
    fn load_all_reviews() {
        let tmp = TempDir::new().unwrap();
        let mana_dir = tmp.path().join(".mana");
        fs::create_dir_all(&mana_dir).unwrap();

        save(&mana_dir, &make_review("1.1")).unwrap();
        save(&mana_dir, &make_review("1.2")).unwrap();
        save(&mana_dir, &make_review("1.3")).unwrap();

        let all = load_all(&mana_dir).unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn has_review_check() {
        let tmp = TempDir::new().unwrap();
        let mana_dir = tmp.path().join(".mana");
        fs::create_dir_all(&mana_dir).unwrap();

        assert!(!has_review(&mana_dir, "1.3"));
        save(&mana_dir, &make_review("1.3")).unwrap();
        assert!(has_review(&mana_dir, "1.3"));
    }
}
