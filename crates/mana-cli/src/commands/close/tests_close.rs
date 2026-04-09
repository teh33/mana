use super::*;
use crate::unit::OnCloseAction;
use crate::unit::OnFailAction;
use crate::util::title_to_slug;
use tempfile::{Builder, TempDir};

fn new_close_temp_dir(prefix: &str) -> TempDir {
    Builder::new().prefix(prefix).tempdir().unwrap()
}

fn setup_test_mana_dir() -> (TempDir, std::path::PathBuf) {
    let dir = new_close_temp_dir("mana-close-test-");
    let project_root = fs::canonicalize(dir.path()).unwrap();
    let mana_dir = project_root.join(".mana");
    fs::create_dir(&mana_dir).unwrap();
    (dir, mana_dir)
}

#[test]
fn test_close_single_unit() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let unit = Unit::new("1", "Task");
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert_eq!(updated.status, Status::Closed);
    assert!(updated.closed_at.is_some());
    assert!(updated.close_reason.is_none());
    assert!(updated.is_archived);
}

#[test]
fn test_close_with_reason() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let unit = Unit::new("1", "Task");
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(
        &mana_dir,
        vec!["1".to_string()],
        Some("Fixed".to_string()),
        false,
        false,
    )
    .unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert_eq!(updated.status, Status::Closed);
    assert_eq!(updated.close_reason, Some("Fixed".to_string()));
    assert!(updated.is_archived);
}

#[test]
fn test_close_multiple_units() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let unit1 = Unit::new("1", "Task 1");
    let unit2 = Unit::new("2", "Task 2");
    let unit3 = Unit::new("3", "Task 3");
    let slug1 = title_to_slug(&unit1.title);
    let slug2 = title_to_slug(&unit2.title);
    let slug3 = title_to_slug(&unit3.title);
    unit1
        .to_file(mana_dir.join(format!("1-{}.md", slug1)))
        .unwrap();
    unit2
        .to_file(mana_dir.join(format!("2-{}.md", slug2)))
        .unwrap();
    unit3
        .to_file(mana_dir.join(format!("3-{}.md", slug3)))
        .unwrap();

    cmd_close(
        &mana_dir,
        vec!["1".to_string(), "2".to_string(), "3".to_string()],
        None,
        false,
        false,
    )
    .unwrap();

    for id in &["1", "2", "3"] {
        let archived = crate::discovery::find_archived_unit(&mana_dir, id).unwrap();
        let unit = Unit::from_file(&archived).unwrap();
        assert_eq!(unit.status, Status::Closed);
        assert!(unit.closed_at.is_some());
        assert!(unit.is_archived);
    }
}

#[test]
fn test_close_nonexistent_unit() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let result = cmd_close(&mana_dir, vec!["99".to_string()], None, false, false);
    assert!(result.is_err());
}

#[test]
fn test_close_no_ids() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let result = cmd_close(&mana_dir, vec![], None, false, false);
    assert!(result.is_err());
}

#[test]
fn test_close_rebuilds_index() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let unit1 = Unit::new("1", "Task 1");
    let unit2 = Unit::new("2", "Task 2");
    let slug1 = title_to_slug(&unit1.title);
    let slug2 = title_to_slug(&unit2.title);
    unit1
        .to_file(mana_dir.join(format!("1-{}.md", slug1)))
        .unwrap();
    unit2
        .to_file(mana_dir.join(format!("2-{}.md", slug2)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let index = Index::load(&mana_dir).unwrap();
    assert_eq!(index.units.len(), 1);
    let entry2 = index.units.iter().find(|e| e.id == "2").unwrap();
    assert_eq!(entry2.status, Status::Open);

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let unit1_archived = Unit::from_file(&archived).unwrap();
    assert_eq!(unit1_archived.status, Status::Closed);
}

#[test]
fn test_close_sets_updated_at() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let unit = Unit::new("1", "Task");
    let original_updated_at = unit.updated_at;
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    std::thread::sleep(std::time::Duration::from_millis(10));

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert!(updated.updated_at > original_updated_at);
}

#[test]
fn test_close_with_passing_verify() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with verify");
    unit.verify = Some("true".to_string());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert_eq!(updated.status, Status::Closed);
    assert!(updated.closed_at.is_some());
    assert!(updated.is_archived);
}

#[test]
fn test_close_with_failing_verify_increments_attempts() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with failing verify");
    unit.verify = Some("false".to_string());
    unit.attempts = 0;
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert_eq!(updated.status, Status::Open);
    assert_eq!(updated.attempts, 1);
    assert!(updated.closed_at.is_none());
}

#[test]
fn test_close_with_failing_verify_multiple_attempts() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with failing verify");
    unit.verify = Some("false".to_string());
    unit.attempts = 0;
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    // First attempt
    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();
    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert_eq!(updated.attempts, 1);
    assert_eq!(updated.status, Status::Open);

    // Second attempt
    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();
    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert_eq!(updated.attempts, 2);
    assert_eq!(updated.status, Status::Open);

    // Third attempt
    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();
    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert_eq!(updated.attempts, 3);
    assert_eq!(updated.status, Status::Open);

    // Fourth attempt
    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();
    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert_eq!(updated.attempts, 4);
    assert_eq!(updated.status, Status::Open);
}

#[test]
fn test_close_failure_appends_to_notes() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with failing verify");
    unit.verify = Some("echo 'test error output' && exit 1".to_string());
    unit.notes = Some("Original notes".to_string());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    let notes = updated.notes.unwrap();

    assert!(notes.contains("Original notes"));
    assert!(notes.contains("## Attempt 1"));
    assert!(notes.contains("Exit code: 1"));
    assert!(notes.contains("test error output"));
}

#[test]
fn test_close_failure_creates_notes_if_none() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with no notes");
    unit.verify = Some("echo 'failure' && exit 1".to_string());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    let notes = updated.notes.unwrap();

    assert!(notes.contains("## Attempt 1"));
    assert!(notes.contains("failure"));
}

#[test]
fn test_close_without_verify_still_works() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let unit = Unit::new("1", "Task without verify");
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert_eq!(updated.status, Status::Closed);
    assert!(updated.closed_at.is_some());
    assert!(updated.is_archived);
}

#[test]
fn test_close_with_force_skips_verify() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with failing verify");
    unit.verify = Some("false".to_string());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, true, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert_eq!(updated.status, Status::Closed);
    assert!(updated.is_archived);
    assert_eq!(updated.attempts, 0);
}

#[test]
fn test_close_with_empty_verify_still_closes() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with empty verify");
    unit.verify = Some("".to_string());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert_eq!(updated.status, Status::Closed);
    assert!(updated.is_archived);
    assert_eq!(updated.attempts, 0);
}

#[test]
fn test_close_with_whitespace_verify_still_closes() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with whitespace verify");
    unit.verify = Some("   ".to_string());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert_eq!(updated.status, Status::Closed);
    assert!(updated.is_archived);
}

#[test]
fn test_close_with_shell_operators_work() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with shell operators");
    unit.verify = Some("true && true".to_string());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert_eq!(updated.status, Status::Closed);
    assert!(updated.is_archived);
}

#[test]
fn test_close_with_pipe_propagates_exit_code() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with pipe");
    unit.verify = Some("true | false".to_string());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    let _ = cmd_close(&mana_dir, vec!["1".to_string()], None, false, false);

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert_eq!(updated.status, Status::Open);
    assert_eq!(updated.attempts, 1);
}

// =====================================================================
// Pre-Close Hook Tests
// =====================================================================

#[test]
fn test_close_with_passing_pre_close_hook() {
    let (dir, mana_dir) = setup_test_mana_dir();
    let project_root = dir.path();
    let hooks_dir = mana_dir.join("hooks");
    fs::create_dir_all(&hooks_dir).unwrap();

    crate::hooks::create_trust(project_root).unwrap();

    let hook_path = hooks_dir.join("pre-close");
    fs::write(&hook_path, "#!/bin/bash\nexit 0").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let unit = Unit::new("1", "Task with passing hook");
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert_eq!(updated.status, Status::Closed);
    assert!(updated.is_archived);
}

#[test]
fn test_close_with_failing_pre_close_hook_blocks_close() {
    let (dir, mana_dir) = setup_test_mana_dir();
    let project_root = dir.path();
    let hooks_dir = mana_dir.join("hooks");
    fs::create_dir_all(&hooks_dir).unwrap();

    crate::hooks::create_trust(project_root).unwrap();

    let hook_path = hooks_dir.join("pre-close");
    fs::write(&hook_path, "#!/bin/bash\nexit 1").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let unit = Unit::new("1", "Task with failing hook");
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let not_archived = crate::discovery::find_unit_file(&mana_dir, "1");
    assert!(not_archived.is_ok());
    let updated = Unit::from_file(not_archived.unwrap()).unwrap();
    assert_eq!(updated.status, Status::Open);
    assert!(!updated.is_archived);
}

#[test]
fn test_close_batch_with_mixed_hook_results() {
    let (dir, mana_dir) = setup_test_mana_dir();
    let project_root = dir.path();
    let hooks_dir = mana_dir.join("hooks");
    fs::create_dir_all(&hooks_dir).unwrap();

    crate::hooks::create_trust(project_root).unwrap();

    let hook_path = hooks_dir.join("pre-close");
    fs::write(&hook_path, "#!/bin/bash\nexit 0").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let unit1 = Unit::new("1", "Task 1 - will close");
    let unit2 = Unit::new("2", "Task 2 - will close");
    let unit3 = Unit::new("3", "Task 3 - will close");
    let slug1 = title_to_slug(&unit1.title);
    let slug2 = title_to_slug(&unit2.title);
    let slug3 = title_to_slug(&unit3.title);
    unit1
        .to_file(mana_dir.join(format!("1-{}.md", slug1)))
        .unwrap();
    unit2
        .to_file(mana_dir.join(format!("2-{}.md", slug2)))
        .unwrap();
    unit3
        .to_file(mana_dir.join(format!("3-{}.md", slug3)))
        .unwrap();

    cmd_close(
        &mana_dir,
        vec!["1".to_string(), "2".to_string(), "3".to_string()],
        None,
        false,
        false,
    )
    .unwrap();

    for id in &["1", "2", "3"] {
        let archived = crate::discovery::find_archived_unit(&mana_dir, id).unwrap();
        let unit = Unit::from_file(&archived).unwrap();
        assert_eq!(unit.status, Status::Closed);
        assert!(unit.is_archived);
    }
}

#[test]
fn test_close_with_untrusted_hooks_silently_skips() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let hooks_dir = mana_dir.join("hooks");
    fs::create_dir_all(&hooks_dir).unwrap();

    let hook_path = hooks_dir.join("pre-close");
    fs::write(&hook_path, "#!/bin/bash\nexit 1").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let unit = Unit::new("1", "Task with untrusted hook");
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert_eq!(updated.status, Status::Closed);
    assert!(updated.is_archived);
}

#[test]
fn test_close_with_missing_hook_silently_succeeds() {
    let (dir, mana_dir) = setup_test_mana_dir();
    let project_root = dir.path();

    crate::hooks::create_trust(project_root).unwrap();

    let unit = Unit::new("1", "Task with missing hook");
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert_eq!(updated.status, Status::Closed);
    assert!(updated.is_archived);
}

#[test]
fn test_close_passes_reason_to_pre_close_hook() {
    let (dir, mana_dir) = setup_test_mana_dir();
    let project_root = dir.path();
    let hooks_dir = mana_dir.join("hooks");
    fs::create_dir_all(&hooks_dir).unwrap();

    crate::hooks::create_trust(project_root).unwrap();

    let hook_path = hooks_dir.join("pre-close");
    fs::write(&hook_path, "#!/bin/bash\nexit 0").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let unit = Unit::new("1", "Task with reason");
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(
        &mana_dir,
        vec!["1".to_string()],
        Some("Completed".to_string()),
        false,
        false,
    )
    .unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert_eq!(updated.status, Status::Closed);
    assert_eq!(updated.close_reason, Some("Completed".to_string()));
}

#[test]
fn test_close_batch_partial_rejection_by_hook() {
    let (dir, mana_dir) = setup_test_mana_dir();
    let project_root = dir.path();
    let hooks_dir = mana_dir.join("hooks");
    fs::create_dir_all(&hooks_dir).unwrap();

    crate::hooks::create_trust(project_root).unwrap();

    let hook_path = hooks_dir.join("pre-close");
    fs::write(
        &hook_path,
        "#!/bin/bash\ntimeout 5 dd bs=1M 2>/dev/null | grep -q '\"id\":\"2\"' && exit 1 || exit 0",
    )
    .unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let unit1 = Unit::new("1", "Task 1");
    let unit2 = Unit::new("2", "Task 2 - will be rejected");
    let unit3 = Unit::new("3", "Task 3");
    let slug1 = title_to_slug(&unit1.title);
    let slug2 = title_to_slug(&unit2.title);
    let slug3 = title_to_slug(&unit3.title);
    unit1
        .to_file(mana_dir.join(format!("1-{}.md", slug1)))
        .unwrap();
    unit2
        .to_file(mana_dir.join(format!("2-{}.md", slug2)))
        .unwrap();
    unit3
        .to_file(mana_dir.join(format!("3-{}.md", slug3)))
        .unwrap();

    cmd_close(
        &mana_dir,
        vec!["1".to_string(), "2".to_string(), "3".to_string()],
        None,
        false,
        false,
    )
    .unwrap();

    let archived1 = crate::discovery::find_archived_unit(&mana_dir, "1");
    assert!(archived1.is_ok());
    let unit1_result = Unit::from_file(archived1.unwrap()).unwrap();
    assert_eq!(unit1_result.status, Status::Closed);

    let open2 = crate::discovery::find_unit_file(&mana_dir, "2");
    assert!(open2.is_ok());
    let unit2_result = Unit::from_file(open2.unwrap()).unwrap();
    assert_eq!(unit2_result.status, Status::Open);

    let archived3 = crate::discovery::find_archived_unit(&mana_dir, "3");
    assert!(archived3.is_ok());
    let unit3_result = Unit::from_file(archived3.unwrap()).unwrap();
    assert_eq!(unit3_result.status, Status::Closed);
}

// =====================================================================
// Post-Close Hook Tests
// =====================================================================

#[test]
fn test_post_close_hook_fires_after_successful_close() {
    let (dir, mana_dir) = setup_test_mana_dir();
    let project_root = dir.path();
    let hooks_dir = mana_dir.join("hooks");
    fs::create_dir_all(&hooks_dir).unwrap();

    crate::hooks::create_trust(project_root).unwrap();

    let marker = project_root.join("post-close-fired");
    let hook_path = hooks_dir.join("post-close");
    fs::write(
        &hook_path,
        format!("#!/bin/bash\ntouch {}\nexit 0", marker.display()),
    )
    .unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let unit = Unit::new("1", "Task with post-close hook");
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    assert!(marker.exists(), "post-close hook should have fired");

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert_eq!(updated.status, Status::Closed);
    assert!(updated.is_archived);
}

#[test]
fn test_post_close_hook_failure_does_not_prevent_close() {
    let (dir, mana_dir) = setup_test_mana_dir();
    let project_root = dir.path();
    let hooks_dir = mana_dir.join("hooks");
    fs::create_dir_all(&hooks_dir).unwrap();

    crate::hooks::create_trust(project_root).unwrap();

    let hook_path = hooks_dir.join("post-close");
    fs::write(&hook_path, "#!/bin/bash\nexit 1").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let unit = Unit::new("1", "Task with failing post-close hook");
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert_eq!(updated.status, Status::Closed);
    assert!(updated.is_archived);
}

// =====================================================================
// Auto-Close Parent Tests
// =====================================================================

fn setup_test_mana_dir_with_config() -> (TempDir, std::path::PathBuf) {
    let dir = new_close_temp_dir("mana-close-config-");
    let project_root = fs::canonicalize(dir.path()).unwrap();
    let mana_dir = project_root.join(".mana");
    fs::create_dir(&mana_dir).unwrap();

    let config = crate::config::Config {
        project: "test".to_string(),
        next_id: 100,
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
    };
    config.save(&mana_dir).unwrap();

    (dir, mana_dir)
}

#[test]
fn test_auto_close_parent_when_all_children_closed() {
    let (_dir, mana_dir) = setup_test_mana_dir_with_config();

    let parent = Unit::new("1", "Parent Task");
    let parent_slug = title_to_slug(&parent.title);
    parent
        .to_file(mana_dir.join(format!("1-{}.md", parent_slug)))
        .unwrap();

    let mut child1 = Unit::new("1.1", "Child 1");
    child1.parent = Some("1".to_string());
    let child1_slug = title_to_slug(&child1.title);
    child1
        .to_file(mana_dir.join(format!("1.1-{}.md", child1_slug)))
        .unwrap();

    let mut child2 = Unit::new("1.2", "Child 2");
    child2.parent = Some("1".to_string());
    let child2_slug = title_to_slug(&child2.title);
    child2
        .to_file(mana_dir.join(format!("1.2-{}.md", child2_slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1.1".to_string()], None, false, false).unwrap();

    let parent_still_open = crate::discovery::find_unit_file(&mana_dir, "1");
    assert!(parent_still_open.is_ok());
    let parent_unit = Unit::from_file(parent_still_open.unwrap()).unwrap();
    assert_eq!(parent_unit.status, Status::Open);

    cmd_close(&mana_dir, vec!["1.2".to_string()], None, false, false).unwrap();

    let parent_archived = crate::discovery::find_archived_unit(&mana_dir, "1");
    assert!(parent_archived.is_ok(), "Parent should be auto-archived");
    let parent_result = Unit::from_file(parent_archived.unwrap()).unwrap();
    assert_eq!(parent_result.status, Status::Closed);
    assert!(parent_result
        .close_reason
        .as_ref()
        .unwrap()
        .contains("Auto-closed"));
}

#[test]
fn test_no_auto_close_when_children_still_open() {
    let (_dir, mana_dir) = setup_test_mana_dir_with_config();

    let parent = Unit::new("1", "Parent Task");
    let parent_slug = title_to_slug(&parent.title);
    parent
        .to_file(mana_dir.join(format!("1-{}.md", parent_slug)))
        .unwrap();

    let mut child1 = Unit::new("1.1", "Child 1");
    child1.parent = Some("1".to_string());
    let child1_slug = title_to_slug(&child1.title);
    child1
        .to_file(mana_dir.join(format!("1.1-{}.md", child1_slug)))
        .unwrap();

    let mut child2 = Unit::new("1.2", "Child 2");
    child2.parent = Some("1".to_string());
    let child2_slug = title_to_slug(&child2.title);
    child2
        .to_file(mana_dir.join(format!("1.2-{}.md", child2_slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1.1".to_string()], None, false, false).unwrap();

    let parent_still_open = crate::discovery::find_unit_file(&mana_dir, "1");
    assert!(parent_still_open.is_ok());
    let parent_unit = Unit::from_file(parent_still_open.unwrap()).unwrap();
    assert_eq!(parent_unit.status, Status::Open);
}

#[test]
fn test_auto_close_disabled_via_config() {
    let dir = new_close_temp_dir("mana-close-auto-close-disabled-");
    let project_root = fs::canonicalize(dir.path()).unwrap();
    let mana_dir = project_root.join(".mana");
    fs::create_dir(&mana_dir).unwrap();

    let config = crate::config::Config {
        project: "test".to_string(),
        next_id: 100,
        auto_close_parent: false,
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
    };
    config.save(&mana_dir).unwrap();

    let parent = Unit::new("1", "Parent Task");
    let parent_slug = title_to_slug(&parent.title);
    parent
        .to_file(mana_dir.join(format!("1-{}.md", parent_slug)))
        .unwrap();

    let mut child = Unit::new("1.1", "Only Child");
    child.parent = Some("1".to_string());
    let child_slug = title_to_slug(&child.title);
    child
        .to_file(mana_dir.join(format!("1.1-{}.md", child_slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1.1".to_string()], None, false, false).unwrap();

    let parent_still_open = crate::discovery::find_unit_file(&mana_dir, "1");
    assert!(parent_still_open.is_ok());
    let parent_unit = Unit::from_file(parent_still_open.unwrap()).unwrap();
    assert_eq!(parent_unit.status, Status::Open);
}

#[test]
fn test_auto_close_recursive_grandparent() {
    let (_dir, mana_dir) = setup_test_mana_dir_with_config();

    let grandparent = Unit::new("1", "Grandparent");
    let gp_slug = title_to_slug(&grandparent.title);
    grandparent
        .to_file(mana_dir.join(format!("1-{}.md", gp_slug)))
        .unwrap();

    let mut parent = Unit::new("1.1", "Parent");
    parent.parent = Some("1".to_string());
    let p_slug = title_to_slug(&parent.title);
    parent
        .to_file(mana_dir.join(format!("1.1-{}.md", p_slug)))
        .unwrap();

    let mut grandchild = Unit::new("1.1.1", "Grandchild");
    grandchild.parent = Some("1.1".to_string());
    let gc_slug = title_to_slug(&grandchild.title);
    grandchild
        .to_file(mana_dir.join(format!("1.1.1-{}.md", gc_slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1.1.1".to_string()], None, false, false).unwrap();

    let gc_archived = crate::discovery::find_archived_unit(&mana_dir, "1.1.1");
    assert!(gc_archived.is_ok(), "Grandchild should be archived");

    let p_archived = crate::discovery::find_archived_unit(&mana_dir, "1.1");
    assert!(p_archived.is_ok(), "Parent should be auto-archived");

    let gp_archived = crate::discovery::find_archived_unit(&mana_dir, "1");
    assert!(gp_archived.is_ok(), "Grandparent should be auto-archived");

    let p_unit = Unit::from_file(p_archived.unwrap()).unwrap();
    assert!(p_unit
        .close_reason
        .as_ref()
        .unwrap()
        .contains("Auto-closed"));

    let gp_unit = Unit::from_file(gp_archived.unwrap()).unwrap();
    assert!(gp_unit
        .close_reason
        .as_ref()
        .unwrap()
        .contains("Auto-closed"));
}

#[test]
fn test_auto_close_with_no_parent() {
    let (_dir, mana_dir) = setup_test_mana_dir_with_config();

    let unit = Unit::new("1", "Standalone Task");
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1");
    assert!(archived.is_ok());
    let unit_result = Unit::from_file(archived.unwrap()).unwrap();
    assert_eq!(unit_result.status, Status::Closed);
}

#[test]
fn test_all_children_closed_checks_archived_units() {
    let (_dir, mana_dir) = setup_test_mana_dir_with_config();

    let parent = Unit::new("1", "Parent Task");
    let parent_slug = title_to_slug(&parent.title);
    parent
        .to_file(mana_dir.join(format!("1-{}.md", parent_slug)))
        .unwrap();

    let mut child1 = Unit::new("1.1", "Child 1");
    child1.parent = Some("1".to_string());
    let child1_slug = title_to_slug(&child1.title);
    child1
        .to_file(mana_dir.join(format!("1.1-{}.md", child1_slug)))
        .unwrap();

    let mut child2 = Unit::new("1.2", "Child 2");
    child2.parent = Some("1".to_string());
    let child2_slug = title_to_slug(&child2.title);
    child2
        .to_file(mana_dir.join(format!("1.2-{}.md", child2_slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1.1".to_string()], None, false, false).unwrap();

    let child1_archived = crate::discovery::find_archived_unit(&mana_dir, "1.1");
    assert!(child1_archived.is_ok(), "Child 1 should be archived");

    cmd_close(&mana_dir, vec!["1.2".to_string()], None, false, false).unwrap();

    let parent_archived = crate::discovery::find_archived_unit(&mana_dir, "1");
    assert!(
        parent_archived.is_ok(),
        "Parent should be auto-archived when all children (including archived) are closed"
    );
}

// =====================================================================
// Feature Unit Close Tests
// =====================================================================

#[test]
fn test_feature_unit_not_closed_in_non_tty() {
    let (_dir, mana_dir) = setup_test_mana_dir();

    let mut unit = Unit::new("1", "Task management");
    unit.feature = true;
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let still_open = crate::discovery::find_unit_file(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&still_open).unwrap();
    assert_eq!(updated.status, Status::Open);
}

#[test]
fn test_feature_unit_force_still_blocked_in_non_tty() {
    let (_dir, mana_dir) = setup_test_mana_dir();

    let mut unit = Unit::new("1", "Release v2");
    unit.feature = true;
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, true, false).unwrap();

    let still_open = crate::discovery::find_unit_file(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&still_open).unwrap();
    assert_eq!(updated.status, Status::Open);
}

#[test]
fn test_feature_parent_not_auto_closed() {
    let (_dir, mana_dir) = setup_test_mana_dir_with_config();

    let mut parent = Unit::new("1", "Feature parent");
    parent.feature = true;
    let parent_slug = title_to_slug(&parent.title);
    parent
        .to_file(mana_dir.join(format!("1-{}.md", parent_slug)))
        .unwrap();

    let mut child1 = Unit::new("1.1", "Child 1");
    child1.parent = Some("1".to_string());
    let child1_slug = title_to_slug(&child1.title);
    child1
        .to_file(mana_dir.join(format!("1.1-{}.md", child1_slug)))
        .unwrap();

    let mut child2 = Unit::new("1.2", "Child 2");
    child2.parent = Some("1".to_string());
    let child2_slug = title_to_slug(&child2.title);
    child2
        .to_file(mana_dir.join(format!("1.2-{}.md", child2_slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1.1".to_string()], None, false, false).unwrap();
    cmd_close(&mana_dir, vec!["1.2".to_string()], None, false, false).unwrap();

    let parent_still_open = crate::discovery::find_unit_file(&mana_dir, "1");
    assert!(
        parent_still_open.is_ok(),
        "Feature parent should NOT be auto-closed"
    );
    let parent_unit = Unit::from_file(parent_still_open.unwrap()).unwrap();
    assert_eq!(parent_unit.status, Status::Open);
}

#[test]
fn test_non_feature_parent_still_auto_closes() {
    let (_dir, mana_dir) = setup_test_mana_dir_with_config();

    let parent = Unit::new("1", "Regular parent");
    let parent_slug = title_to_slug(&parent.title);
    parent
        .to_file(mana_dir.join(format!("1-{}.md", parent_slug)))
        .unwrap();

    let mut child = Unit::new("1.1", "Only child");
    child.parent = Some("1".to_string());
    let child_slug = title_to_slug(&child.title);
    child
        .to_file(mana_dir.join(format!("1.1-{}.md", child_slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1.1".to_string()], None, false, false).unwrap();

    let parent_archived = crate::discovery::find_archived_unit(&mana_dir, "1");
    assert!(
        parent_archived.is_ok(),
        "Regular parent should be auto-archived"
    );
    let parent_unit = Unit::from_file(parent_archived.unwrap()).unwrap();
    assert_eq!(parent_unit.status, Status::Closed);
}

#[test]
fn test_feature_grandparent_blocks_recursive_auto_close() {
    let (_dir, mana_dir) = setup_test_mana_dir_with_config();

    let mut grandparent = Unit::new("1", "Feature goal");
    grandparent.feature = true;
    let gp_slug = title_to_slug(&grandparent.title);
    grandparent
        .to_file(mana_dir.join(format!("1-{}.md", gp_slug)))
        .unwrap();

    let mut parent = Unit::new("1.1", "Regular parent");
    parent.parent = Some("1".to_string());
    let p_slug = title_to_slug(&parent.title);
    parent
        .to_file(mana_dir.join(format!("1.1-{}.md", p_slug)))
        .unwrap();

    let mut child = Unit::new("1.1.1", "Leaf child");
    child.parent = Some("1.1".to_string());
    let c_slug = title_to_slug(&child.title);
    child
        .to_file(mana_dir.join(format!("1.1.1-{}.md", c_slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1.1.1".to_string()], None, false, false).unwrap();

    assert!(crate::discovery::find_archived_unit(&mana_dir, "1.1.1").is_ok());

    assert!(
        crate::discovery::find_archived_unit(&mana_dir, "1.1").is_ok(),
        "Regular parent should be auto-archived"
    );

    let gp_still_open = crate::discovery::find_unit_file(&mana_dir, "1");
    assert!(
        gp_still_open.is_ok(),
        "Feature grandparent should NOT be auto-closed"
    );
    let gp_unit = Unit::from_file(gp_still_open.unwrap()).unwrap();
    assert_eq!(gp_unit.status, Status::Open);
}

// =====================================================================
// Truncation Helper Tests
// =====================================================================

#[test]
fn test_truncate_to_char_boundary_ascii() {
    let s = "hello world";
    assert_eq!(truncate_to_char_boundary(s, 5), 5);
    assert_eq!(&s[..truncate_to_char_boundary(s, 5)], "hello");
}

#[test]
fn test_truncate_to_char_boundary_multibyte() {
    let s = "😀😁😂";
    assert_eq!(s.len(), 12);

    assert_eq!(truncate_to_char_boundary(s, 5), 4);
    assert_eq!(&s[..truncate_to_char_boundary(s, 5)], "😀");

    assert_eq!(truncate_to_char_boundary(s, 8), 8);
    assert_eq!(&s[..truncate_to_char_boundary(s, 8)], "😀😁");
}

#[test]
fn test_truncate_to_char_boundary_beyond_len() {
    let s = "short";
    assert_eq!(truncate_to_char_boundary(s, 100), 5);
}

#[test]
fn test_truncate_to_char_boundary_zero() {
    let s = "hello";
    assert_eq!(truncate_to_char_boundary(s, 0), 0);
}

// =====================================================================
// on_close Action Tests
// =====================================================================

#[test]
fn on_close_run_action_executes_command() {
    let (dir, mana_dir) = setup_test_mana_dir();
    let project_root = dir.path();
    crate::hooks::create_trust(project_root).unwrap();
    let marker = project_root.join("on_close_ran");

    let mut unit = Unit::new("1", "Task with on_close run");
    unit.on_close = vec![OnCloseAction::Run {
        command: format!("touch {}", marker.display()),
    }];
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    assert!(marker.exists(), "on_close run command should have executed");
}

#[test]
fn on_close_notify_action_prints_message() {
    let (_dir, mana_dir) = setup_test_mana_dir();

    let mut unit = Unit::new("1", "Task with on_close notify");
    unit.on_close = vec![OnCloseAction::Notify {
        message: "All done!".to_string(),
    }];
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert_eq!(updated.status, Status::Closed);
}

#[test]
fn on_close_run_failure_does_not_prevent_close() {
    let (dir, mana_dir) = setup_test_mana_dir();
    let project_root = dir.path();
    crate::hooks::create_trust(project_root).unwrap();

    let mut unit = Unit::new("1", "Task with failing on_close");
    unit.on_close = vec![OnCloseAction::Run {
        command: "false".to_string(),
    }];
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert_eq!(updated.status, Status::Closed);
    assert!(updated.is_archived);
}

#[test]
fn on_close_multiple_actions_all_run() {
    let (dir, mana_dir) = setup_test_mana_dir();
    let project_root = dir.path();
    crate::hooks::create_trust(project_root).unwrap();
    let marker1 = project_root.join("on_close_1");
    let marker2 = project_root.join("on_close_2");

    let mut unit = Unit::new("1", "Task with multiple on_close");
    unit.on_close = vec![
        OnCloseAction::Run {
            command: format!("touch {}", marker1.display()),
        },
        OnCloseAction::Notify {
            message: "Between actions".to_string(),
        },
        OnCloseAction::Run {
            command: format!("touch {}", marker2.display()),
        },
    ];
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    assert!(marker1.exists(), "First on_close run should have executed");
    assert!(marker2.exists(), "Second on_close run should have executed");
}

#[test]
fn on_close_run_skipped_without_trust() {
    let (dir, mana_dir) = setup_test_mana_dir();
    let project_root = dir.path();
    let marker = project_root.join("on_close_should_not_exist");

    let mut unit = Unit::new("1", "Task with untrusted on_close");
    unit.on_close = vec![OnCloseAction::Run {
        command: format!("touch {}", marker.display()),
    }];
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    assert!(
        !marker.exists(),
        "on_close run should be skipped without trust"
    );

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert_eq!(updated.status, Status::Closed);
    assert!(updated.is_archived);
}

#[test]
fn on_close_runs_in_project_root() {
    let (dir, mana_dir) = setup_test_mana_dir();
    let project_root = dir.path();
    crate::hooks::create_trust(project_root).unwrap();

    let mut unit = Unit::new("1", "Task with pwd check");
    let pwd_file = project_root.join("on_close_pwd");
    unit.on_close = vec![OnCloseAction::Run {
        command: format!("pwd > {}", pwd_file.display()),
    }];
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let pwd_output = fs::read_to_string(&pwd_file).unwrap();
    let expected = std::fs::canonicalize(project_root).unwrap();
    let actual = std::fs::canonicalize(pwd_output.trim()).unwrap();
    assert_eq!(actual, expected);
}

// =====================================================================
// History Recording Tests
// =====================================================================

#[test]
fn history_failure_creates_run_record() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with failing verify");
    unit.verify = Some("echo 'some error' && exit 1".to_string());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert_eq!(updated.history.len(), 1);
    let record = &updated.history[0];
    assert_eq!(record.result, RunResult::Fail);
    assert_eq!(record.attempt, 1);
    assert_eq!(record.exit_code, Some(1));
    assert!(record.output_snippet.is_some());
    assert!(record
        .output_snippet
        .as_ref()
        .unwrap()
        .contains("some error"));
}

#[test]
fn history_success_creates_run_record() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with passing verify");
    unit.verify = Some("true".to_string());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert_eq!(updated.history.len(), 1);
    let record = &updated.history[0];
    assert_eq!(record.result, RunResult::Pass);
    assert_eq!(record.attempt, 1);
    let snippet = record.output_snippet.as_deref().unwrap_or("");
    assert!(snippet.contains("verify passed"));
    assert!(snippet.contains("changed"));
}

#[test]
fn history_has_correct_duration() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with timed verify");
    unit.verify = Some("sleep 0.1 && true".to_string());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert_eq!(updated.history.len(), 1);
    let record = &updated.history[0];
    assert!(record.finished_at.is_some());
    assert!(record.duration_secs.is_some());
    let dur = record.duration_secs.unwrap();
    assert!(dur >= 0.05, "Duration should be >= 0.05s, got {}", dur);
    assert!(record.finished_at.unwrap() >= record.started_at);
}

#[test]
fn history_records_exit_code() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with exit code 42");
    unit.verify = Some("exit 42".to_string());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert_eq!(updated.history.len(), 1);
    assert_eq!(updated.history[0].exit_code, Some(42));
    assert_eq!(updated.history[0].result, RunResult::Fail);
}

#[test]
fn history_multiple_attempts_accumulate() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with multiple failures");
    unit.verify = Some("false".to_string());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();
    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();
    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert_eq!(updated.history.len(), 3);
    assert_eq!(updated.history[0].attempt, 1);
    assert_eq!(updated.history[1].attempt, 2);
    assert_eq!(updated.history[2].attempt, 3);
    for record in &updated.history {
        assert_eq!(record.result, RunResult::Fail);
    }
}

#[test]
fn history_agent_from_env_var() {
    std::env::set_var("MANA_AGENT", "test-agent-42");

    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with agent env");
    unit.verify = Some("true".to_string());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    std::env::remove_var("MANA_AGENT");

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert_eq!(updated.history.len(), 1);
    assert_eq!(updated.history[0].agent, Some("test-agent-42".to_string()));
}

#[test]
fn history_no_record_without_verify() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let unit = Unit::new("1", "Task without verify");
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert!(
        updated.history.is_empty(),
        "No history when no verify command"
    );
}

#[test]
fn history_no_record_when_force_skip() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task force closed");
    unit.verify = Some("false".to_string());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, true, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert!(
        updated.history.is_empty(),
        "No history when verify skipped with --force"
    );
}

#[test]
fn history_failure_then_success_accumulates() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task that eventually passes");
    unit.verify = Some("false".to_string());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let mut updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    updated.verify = Some("true".to_string());
    updated
        .to_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap())
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let final_unit = Unit::from_file(&archived).unwrap();
    assert_eq!(final_unit.history.len(), 2);
    assert_eq!(final_unit.history[0].result, RunResult::Fail);
    assert_eq!(final_unit.history[0].attempt, 1);
    assert_eq!(final_unit.history[1].result, RunResult::Pass);
    assert_eq!(final_unit.history[1].attempt, 2);
}

// =====================================================================
// on_fail Action Tests
// =====================================================================

#[test]
fn on_fail_retry_releases_claim_when_under_max() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with retry on_fail");
    unit.verify = Some("false".to_string());
    unit.on_fail = Some(OnFailAction::Retry {
        max: Some(5),
        delay_secs: None,
    });
    unit.claimed_by = Some("agent-1".to_string());
    unit.claimed_at = Some(Utc::now());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert_eq!(updated.status, Status::Open);
    assert_eq!(updated.attempts, 1);
    assert!(updated.claimed_by.is_none());
    assert!(updated.claimed_at.is_none());
}

#[test]
fn on_fail_retry_keeps_claim_when_at_max() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task exhausted retries");
    unit.verify = Some("false".to_string());
    unit.on_fail = Some(OnFailAction::Retry {
        max: Some(2),
        delay_secs: None,
    });
    unit.attempts = 1;
    unit.claimed_by = Some("agent-1".to_string());
    unit.claimed_at = Some(Utc::now());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert_eq!(updated.attempts, 2);
    assert_eq!(updated.claimed_by, Some("agent-1".to_string()));
    assert!(updated.claimed_at.is_some());
}

#[test]
fn on_fail_retry_max_defaults_to_max_attempts() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with default max");
    unit.verify = Some("false".to_string());
    unit.max_attempts = 3;
    unit.on_fail = Some(OnFailAction::Retry {
        max: None,
        delay_secs: None,
    });
    unit.claimed_by = Some("agent-1".to_string());
    unit.claimed_at = Some(Utc::now());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    // First attempt (1 < 3) — should release
    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();
    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert_eq!(updated.attempts, 1);
    assert!(updated.claimed_by.is_none());

    // Re-claim and fail again (2 < 3) — should release
    let mut unit2 =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    unit2.claimed_by = Some("agent-2".to_string());
    unit2.claimed_at = Some(Utc::now());
    unit2
        .to_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap())
        .unwrap();
    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();
    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert_eq!(updated.attempts, 2);
    assert!(updated.claimed_by.is_none());

    // Re-claim and fail again (3 >= 3) — should NOT release
    let mut unit3 =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    unit3.claimed_by = Some("agent-3".to_string());
    unit3.claimed_at = Some(Utc::now());
    unit3
        .to_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap())
        .unwrap();
    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();
    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert_eq!(updated.attempts, 3);
    assert_eq!(updated.claimed_by, Some("agent-3".to_string()));
}

#[test]
fn on_fail_retry_with_delay_releases_claim() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with delay");
    unit.verify = Some("false".to_string());
    unit.on_fail = Some(OnFailAction::Retry {
        max: Some(3),
        delay_secs: Some(30),
    });
    unit.claimed_by = Some("agent-1".to_string());
    unit.claimed_at = Some(Utc::now());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert_eq!(updated.attempts, 1);
    assert!(updated.claimed_by.is_none());
    assert!(updated.claimed_at.is_none());
}

#[test]
fn on_fail_escalate_updates_priority() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task to escalate");
    unit.verify = Some("false".to_string());
    unit.priority = 2;
    unit.on_fail = Some(OnFailAction::Escalate {
        priority: Some(0),
        message: None,
    });
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert_eq!(updated.priority, 0);
    assert!(updated.labels.contains(&"escalated".to_string()));
}

#[test]
fn on_fail_escalate_appends_message_to_notes() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with escalation message");
    unit.verify = Some("false".to_string());
    unit.notes = Some("Existing notes".to_string());
    unit.on_fail = Some(OnFailAction::Escalate {
        priority: None,
        message: Some("Needs human review".to_string()),
    });
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    let notes = updated.notes.unwrap();
    assert!(notes.contains("Existing notes"));
    assert!(notes.contains("## Escalated"));
    assert!(notes.contains("Needs human review"));
    assert!(updated.labels.contains(&"escalated".to_string()));
}

#[test]
fn on_fail_escalate_adds_label() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task to label");
    unit.verify = Some("false".to_string());
    unit.on_fail = Some(OnFailAction::Escalate {
        priority: None,
        message: None,
    });
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert!(updated.labels.contains(&"escalated".to_string()));
}

#[test]
fn on_fail_escalate_no_duplicate_label() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task already escalated");
    unit.verify = Some("false".to_string());
    unit.labels = vec!["escalated".to_string()];
    unit.on_fail = Some(OnFailAction::Escalate {
        priority: None,
        message: None,
    });
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    let count = updated
        .labels
        .iter()
        .filter(|l| l.as_str() == "escalated")
        .count();
    assert_eq!(count, 1, "Should not duplicate 'escalated' label");
}

#[test]
fn on_fail_none_existing_behavior_unchanged() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with no on_fail");
    unit.verify = Some("false".to_string());
    unit.claimed_by = Some("agent-1".to_string());
    unit.claimed_at = Some(Utc::now());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert_eq!(updated.status, Status::Open);
    assert_eq!(updated.attempts, 1);
    assert_eq!(updated.claimed_by, Some("agent-1".to_string()));
    assert!(updated.labels.is_empty());
}

// =====================================================================
// Output Capture Tests
// =====================================================================

#[test]
fn output_capture_json_stdout_stored_as_outputs() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with JSON output");
    unit.verify = Some(r#"echo '{"passed":42,"failed":0}'"#.to_string());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert_eq!(updated.status, Status::Closed);
    let outputs = updated.outputs.expect("outputs should be set");
    assert_eq!(outputs["passed"], 42);
    assert_eq!(outputs["failed"], 0);
}

#[test]
fn output_capture_non_json_stdout_stored_as_text() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with plain text output");
    unit.verify = Some("echo 'hello world'".to_string());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    let outputs = updated.outputs.expect("outputs should be set");
    assert_eq!(outputs["text"], "hello world");
}

#[test]
fn output_capture_empty_stdout_no_outputs() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with no stdout");
    unit.verify = Some("true".to_string());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert!(
        updated.outputs.is_none(),
        "empty stdout should not set outputs"
    );
}

#[test]
fn output_capture_large_stdout_truncated() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with large output");
    unit.verify = Some("python3 -c \"print('x' * 70000)\"".to_string());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    let outputs = updated
        .outputs
        .expect("outputs should be set for large output");
    assert_eq!(outputs["truncated"], true);
    assert!(outputs["original_bytes"].as_u64().unwrap() > 64 * 1024);
    let text = outputs["text"].as_str().unwrap();
    assert!(text.len() <= 64 * 1024);
}

#[test]
fn output_capture_stderr_not_captured_as_outputs() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with stderr only");
    unit.verify = Some("echo 'error info' >&2".to_string());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    assert!(
        updated.outputs.is_none(),
        "stderr-only output should not set outputs"
    );
}

#[test]
fn output_capture_failure_unchanged() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task that fails with output");
    unit.verify = Some(r#"echo '{"result":"data"}' && exit 1"#.to_string());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert_eq!(updated.status, Status::Open);
    assert!(
        updated.outputs.is_none(),
        "failed verify should not capture outputs"
    );
}

#[test]
fn output_capture_json_array() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with JSON array output");
    unit.verify = Some(r#"echo '["a","b","c"]'"#.to_string());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    let outputs = updated.outputs.expect("outputs should be set");
    let arr = outputs.as_array().unwrap();
    assert_eq!(arr.len(), 3);
    assert_eq!(arr[0], "a");
}

#[test]
fn output_capture_mixed_stdout_stderr() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task with mixed output");
    unit.verify = Some(r#"echo '{"key":"value"}' && echo 'debug log' >&2"#.to_string());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
    let updated = Unit::from_file(&archived).unwrap();
    let outputs = updated.outputs.expect("outputs should capture stdout only");
    assert_eq!(outputs["key"], "value");
    assert!(
        outputs.get("text").is_none()
            || !outputs["text"].as_str().unwrap_or("").contains("debug log")
    );
}

// =====================================================================
// Circuit Breaker (max_loops) Tests
// =====================================================================

fn setup_mana_dir_with_max_loops(max_loops: u32) -> (TempDir, std::path::PathBuf) {
    let dir = new_close_temp_dir("mana-close-max-loops-");
    let project_root = fs::canonicalize(dir.path()).unwrap();
    let mana_dir = project_root.join(".mana");
    fs::create_dir(&mana_dir).unwrap();

    let config = crate::config::Config {
        project: "test".to_string(),
        next_id: 100,
        auto_close_parent: true,
        run: None,
        plan: None,
        max_loops,
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
    };
    config.save(&mana_dir).unwrap();

    (dir, mana_dir)
}

#[test]
fn max_loops_circuit_breaker_triggers_at_limit() {
    let (_dir, mana_dir) = setup_mana_dir_with_max_loops(3);

    let parent = Unit::new("1", "Parent");
    let parent_slug = title_to_slug(&parent.title);
    parent
        .to_file(mana_dir.join(format!("1-{}.md", parent_slug)))
        .unwrap();

    let mut child1 = Unit::new("1.1", "Child with attempts");
    child1.parent = Some("1".to_string());
    child1.verify = Some("false".to_string());
    child1.attempts = 2;
    let child1_slug = title_to_slug(&child1.title);
    child1
        .to_file(mana_dir.join(format!("1.1-{}.md", child1_slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1.1".to_string()], None, false, false).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1.1").unwrap()).unwrap();
    assert_eq!(updated.status, Status::Open);
    assert_eq!(updated.attempts, 3);
    assert!(
        updated.labels.contains(&"circuit-breaker".to_string()),
        "Circuit breaker label should be added"
    );
    assert_eq!(updated.priority, 0, "Priority should be escalated to P0");
}

#[test]
fn max_loops_circuit_breaker_does_not_trigger_below_limit() {
    let (_dir, mana_dir) = setup_mana_dir_with_max_loops(5);

    let parent = Unit::new("1", "Parent");
    let parent_slug = title_to_slug(&parent.title);
    parent
        .to_file(mana_dir.join(format!("1-{}.md", parent_slug)))
        .unwrap();

    let mut child = Unit::new("1.1", "Child");
    child.parent = Some("1".to_string());
    child.verify = Some("false".to_string());
    child.attempts = 1;
    let child_slug = title_to_slug(&child.title);
    child
        .to_file(mana_dir.join(format!("1.1-{}.md", child_slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1.1".to_string()], None, false, false).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1.1").unwrap()).unwrap();
    assert_eq!(updated.attempts, 2);
    assert!(
        !updated.labels.contains(&"circuit-breaker".to_string()),
        "Circuit breaker should NOT trigger below limit"
    );
    assert_ne!(updated.priority, 0, "Priority should not change");
}

#[test]
fn max_loops_zero_disables_circuit_breaker() {
    let (_dir, mana_dir) = setup_mana_dir_with_max_loops(0);

    let mut unit = Unit::new("1", "Unlimited retries");
    unit.verify = Some("false".to_string());
    unit.attempts = 100;
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert_eq!(updated.attempts, 101);
    assert!(
        !updated.labels.contains(&"circuit-breaker".to_string()),
        "Circuit breaker should not trigger when max_loops=0"
    );
}

#[test]
fn max_loops_per_unit_overrides_config() {
    let (_dir, mana_dir) = setup_mana_dir_with_max_loops(100);

    let mut parent = Unit::new("1", "Parent with low max_loops");
    parent.max_loops = Some(3);
    let parent_slug = title_to_slug(&parent.title);
    parent
        .to_file(mana_dir.join(format!("1-{}.md", parent_slug)))
        .unwrap();

    let mut child = Unit::new("1.1", "Child");
    child.parent = Some("1".to_string());
    child.verify = Some("false".to_string());
    child.attempts = 2;
    let child_slug = title_to_slug(&child.title);
    child
        .to_file(mana_dir.join(format!("1.1-{}.md", child_slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1.1".to_string()], None, false, false).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1.1").unwrap()).unwrap();
    assert!(
        updated.labels.contains(&"circuit-breaker".to_string()),
        "Per-unit max_loops should override config"
    );
    assert_eq!(updated.priority, 0);
}

#[test]
fn max_loops_circuit_breaker_skips_on_fail_retry() {
    let (_dir, mana_dir) = setup_mana_dir_with_max_loops(2);

    let mut unit = Unit::new("1", "Unit with retry that should be blocked");
    unit.verify = Some("false".to_string());
    unit.attempts = 1;
    unit.on_fail = Some(OnFailAction::Retry {
        max: Some(10),
        delay_secs: None,
    });
    unit.claimed_by = Some("agent-1".to_string());
    unit.claimed_at = Some(Utc::now());
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert!(updated.labels.contains(&"circuit-breaker".to_string()));
    assert_eq!(updated.priority, 0);
    assert_eq!(
        updated.claimed_by,
        Some("agent-1".to_string()),
        "on_fail retry should not release claim when circuit breaker trips"
    );
}

#[test]
fn max_loops_counts_across_siblings() {
    let (_dir, mana_dir) = setup_mana_dir_with_max_loops(5);

    let parent = Unit::new("1", "Parent");
    let parent_slug = title_to_slug(&parent.title);
    parent
        .to_file(mana_dir.join(format!("1-{}.md", parent_slug)))
        .unwrap();

    let mut sibling = Unit::new("1.1", "Sibling");
    sibling.parent = Some("1".to_string());
    sibling.attempts = 2;
    let sib_slug = title_to_slug(&sibling.title);
    sibling
        .to_file(mana_dir.join(format!("1.1-{}.md", sib_slug)))
        .unwrap();

    let mut child = Unit::new("1.2", "Child");
    child.parent = Some("1".to_string());
    child.verify = Some("false".to_string());
    child.attempts = 2;
    let child_slug = title_to_slug(&child.title);
    child
        .to_file(mana_dir.join(format!("1.2-{}.md", child_slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1.2".to_string()], None, false, false).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1.2").unwrap()).unwrap();
    assert!(
        updated.labels.contains(&"circuit-breaker".to_string()),
        "Circuit breaker should count sibling attempts"
    );
    assert_eq!(updated.priority, 0);
}

#[test]
fn max_loops_standalone_unit_uses_own_max_loops() {
    let (_dir, mana_dir) = setup_mana_dir_with_max_loops(100);

    let mut unit = Unit::new("1", "Standalone");
    unit.verify = Some("false".to_string());
    unit.max_loops = Some(2);
    unit.attempts = 1;
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert!(updated.labels.contains(&"circuit-breaker".to_string()));
    assert_eq!(updated.priority, 0);
}

#[test]
fn max_loops_no_config_defaults_to_10() {
    let (_dir, mana_dir) = setup_test_mana_dir();

    let mut unit = Unit::new("1", "No config");
    unit.verify = Some("false".to_string());
    unit.attempts = 9;
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert!(
        updated.labels.contains(&"circuit-breaker".to_string()),
        "Should use default max_loops=10"
    );
}

#[test]
fn max_loops_no_duplicate_label() {
    let (_dir, mana_dir) = setup_mana_dir_with_max_loops(1);

    let mut unit = Unit::new("1", "Already has label");
    unit.verify = Some("false".to_string());
    unit.labels = vec!["circuit-breaker".to_string()];
    unit.attempts = 0;
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    let count = updated
        .labels
        .iter()
        .filter(|l| l.as_str() == "circuit-breaker")
        .count();
    assert_eq!(count, 1, "Should not duplicate 'circuit-breaker' label");
}

// =====================================================================
// Close Failed Tests
// =====================================================================

#[test]
fn test_close_failed_marks_attempt_as_failed() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task");
    unit.status = Status::InProgress;
    unit.claimed_by = Some("agent-1".to_string());
    unit.attempt_log.push(crate::unit::AttemptRecord {
        num: 1,
        outcome: crate::unit::AttemptOutcome::Abandoned,
        notes: None,
        agent: Some("agent-1".to_string()),
        started_at: Some(Utc::now()),
        finished_at: None,
    });
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close_failed(
        &mana_dir,
        vec!["1".to_string()],
        Some("blocked by upstream".to_string()),
    )
    .unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert_eq!(updated.status, Status::Open);
    assert!(updated.claimed_by.is_none());
    assert_eq!(updated.attempt_log.len(), 1);
    assert_eq!(
        updated.attempt_log[0].outcome,
        crate::unit::AttemptOutcome::Failed
    );
    assert!(updated.attempt_log[0].finished_at.is_some());
    assert_eq!(
        updated.attempt_log[0].notes.as_deref(),
        Some("blocked by upstream")
    );
}

#[test]
fn test_close_failed_appends_to_notes() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task");
    unit.status = Status::InProgress;
    unit.attempt_log.push(crate::unit::AttemptRecord {
        num: 1,
        outcome: crate::unit::AttemptOutcome::Abandoned,
        notes: None,
        agent: None,
        started_at: Some(Utc::now()),
        finished_at: None,
    });
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close_failed(
        &mana_dir,
        vec!["1".to_string()],
        Some("JWT incompatible".to_string()),
    )
    .unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert!(updated.notes.is_some());
    assert!(updated.notes.unwrap().contains("JWT incompatible"));
}

#[test]
fn test_close_failed_without_reason() {
    let (_dir, mana_dir) = setup_test_mana_dir();
    let mut unit = Unit::new("1", "Task");
    unit.status = Status::InProgress;
    unit.attempt_log.push(crate::unit::AttemptRecord {
        num: 1,
        outcome: crate::unit::AttemptOutcome::Abandoned,
        notes: None,
        agent: None,
        started_at: Some(Utc::now()),
        finished_at: None,
    });
    let slug = title_to_slug(&unit.title);
    unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
        .unwrap();

    cmd_close_failed(&mana_dir, vec!["1".to_string()], None).unwrap();

    let updated =
        Unit::from_file(crate::discovery::find_unit_file(&mana_dir, "1").unwrap()).unwrap();
    assert_eq!(updated.status, Status::Open);
    assert_eq!(
        updated.attempt_log[0].outcome,
        crate::unit::AttemptOutcome::Failed
    );
}

// =====================================================================
// Worktree Merge Integration Tests
// =====================================================================

mod worktree_merge_tests {
    use super::*;
    use std::path::PathBuf;

    fn run_git(dir: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap_or_else(|e| unreachable!("git {:?} failed to execute: {}", args, e));
        assert!(
            output.status.success(),
            "git {:?} in {} failed (exit {:?}):\nstdout: {}\nstderr: {}",
            args,
            dir.display(),
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    /// Set up a git repo with a worktree. Returns (TempDir, main_dir, worktree_mana_dir).
    ///
    /// Each test gets a fully isolated temp directory — no shared state, no CWD mutation.
    /// The `detect_worktree` and `commit_worktree_changes` functions now accept explicit
    /// path arguments, so tests no longer need `std::env::set_current_dir`.
    fn setup_git_worktree() -> (TempDir, PathBuf, PathBuf) {
        let dir = new_close_temp_dir("mana-close-worktree-");
        let base = std::fs::canonicalize(dir.path()).unwrap();
        let main_dir = base.join("main");
        let worktree_dir = base.join("worktree");
        fs::create_dir(&main_dir).unwrap();

        run_git(&main_dir, &["init"]);
        run_git(&main_dir, &["config", "user.email", "test@test.com"]);
        run_git(&main_dir, &["config", "user.name", "Test"]);
        run_git(&main_dir, &["checkout", "-b", "main"]);

        fs::write(main_dir.join("initial.txt"), "initial content").unwrap();
        run_git(&main_dir, &["add", "-A"]);
        run_git(&main_dir, &["commit", "-m", "Initial commit"]);

        let mana_dir = main_dir.join(".mana");
        fs::create_dir(&mana_dir).unwrap();
        fs::write(mana_dir.join(".gitkeep"), "").unwrap();
        run_git(&main_dir, &["add", "-A"]);
        run_git(&main_dir, &["commit", "-m", "Add .mana directory"]);

        run_git(
            &main_dir,
            &[
                "worktree",
                "add",
                worktree_dir.to_str().unwrap(),
                "-b",
                "feature",
            ],
        );

        let worktree_mana_dir = worktree_dir.join(".mana");

        (dir, main_dir, worktree_mana_dir)
    }

    #[test]
    fn test_close_in_worktree_commits_and_merges() {
        let (_dir, main_dir, worktree_mana_dir) = setup_git_worktree();
        let worktree_dir = worktree_mana_dir.parent().unwrap();

        let unit = Unit::new("1", "Worktree Task");
        let slug = title_to_slug(&unit.title);
        unit.to_file(worktree_mana_dir.join(format!("1-{}.md", slug)))
            .unwrap();

        fs::write(worktree_dir.join("feature.txt"), "feature content").unwrap();

        // No set_current_dir needed — detect_worktree now takes an explicit path
        cmd_close(
            &worktree_mana_dir,
            vec!["1".to_string()],
            None,
            false,
            false,
        )
        .unwrap();

        assert!(
            main_dir.join("feature.txt").exists(),
            "feature.txt should be merged to main"
        );
        let content = fs::read_to_string(main_dir.join("feature.txt")).unwrap();
        assert_eq!(content, "feature content");
    }

    #[test]
    fn test_close_with_merge_conflict_aborts() {
        let (_dir, main_dir, worktree_mana_dir) = setup_git_worktree();
        let worktree_dir = worktree_mana_dir.parent().unwrap();

        fs::write(main_dir.join("initial.txt"), "main version").unwrap();
        run_git(&main_dir, &["add", "-A"]);
        run_git(&main_dir, &["commit", "-m", "Diverge on main"]);

        fs::write(worktree_dir.join("initial.txt"), "feature version").unwrap();

        let unit = Unit::new("1", "Conflict Task");
        let slug = title_to_slug(&unit.title);
        unit.to_file(worktree_mana_dir.join(format!("1-{}.md", slug)))
            .unwrap();

        // No set_current_dir needed
        cmd_close(
            &worktree_mana_dir,
            vec!["1".to_string()],
            None,
            false,
            false,
        )
        .unwrap();

        let unit_file = crate::discovery::find_unit_file(&worktree_mana_dir, "1").unwrap();
        let updated = Unit::from_file(&unit_file).unwrap();
        assert_eq!(
            updated.status,
            Status::Open,
            "Unit should remain open when merge conflicts"
        );
    }

    #[test]
    fn test_close_in_main_worktree_skips_merge() {
        let dir = new_close_temp_dir("mana-close-main-worktree-");
        let base = std::fs::canonicalize(dir.path()).unwrap();
        let repo_dir = base.join("repo");
        fs::create_dir(&repo_dir).unwrap();

        run_git(&repo_dir, &["init"]);
        run_git(&repo_dir, &["config", "user.email", "test@test.com"]);
        run_git(&repo_dir, &["config", "user.name", "Test"]);
        run_git(&repo_dir, &["checkout", "-b", "main"]);

        fs::write(repo_dir.join("file.txt"), "content").unwrap();
        run_git(&repo_dir, &["add", "-A"]);
        run_git(&repo_dir, &["commit", "-m", "Initial commit"]);

        let mana_dir = repo_dir.join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        let unit = Unit::new("1", "Main Worktree Task");
        let slug = title_to_slug(&unit.title);
        unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
            .unwrap();

        // No set_current_dir needed
        cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

        let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
        let updated = Unit::from_file(&archived).unwrap();
        assert_eq!(updated.status, Status::Closed);
        assert!(updated.is_archived);
    }

    #[test]
    fn test_close_outside_git_repo_works() {
        let dir = new_close_temp_dir("mana-close-no-git-");
        let base = std::fs::canonicalize(dir.path()).unwrap();
        let mana_dir = base.join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        let unit = Unit::new("1", "No Git Task");
        let slug = title_to_slug(&unit.title);
        unit.to_file(mana_dir.join(format!("1-{}.md", slug)))
            .unwrap();

        // No set_current_dir needed
        cmd_close(&mana_dir, vec!["1".to_string()], None, false, false).unwrap();

        let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
        let updated = Unit::from_file(&archived).unwrap();
        assert_eq!(updated.status, Status::Closed);
        assert!(updated.is_archived);
    }
}
