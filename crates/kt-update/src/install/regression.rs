//! 复制刷盘、存储布局和恢复生命周期的回归；Windows 占用测试不触发 UAC。
use super::*;

pub(super) fn fixture() -> (tempfile::TempDir, PathBuf, Journal) {
    let directory = tempfile::tempdir().unwrap();
    let parent = fs::canonicalize(directory.path()).unwrap();
    let root = parent.join("updates");
    private_root(&root).unwrap();
    let executable = parent.join("kitonyterms.exe");
    fs::write(&executable, b"old").unwrap();
    let current = BuildInfo::current();
    let task = InstallTask {
        installation: Installation::from_executable(executable).unwrap(),
        current: current.clone(),
        next: current,
        source_sha256: download::sha256(&parent.join("kitonyterms.exe")).unwrap(),
        package: Package {
            name: "kitonyterms-test.zip".into(),
            size: 3,
            sha256: "0".repeat(64),
            kind: PackageKind::Zip,
        },
        parent_pid: 1,
        elevate: false,
    };
    let staging = tempfile::Builder::new()
        .prefix(".kt-update-stage-")
        .tempdir_in(&root)
        .unwrap()
        .keep();
    fs::write(staging.join("payload"), b"new").unwrap();
    let journal = Journal {
        task,
        staged_sha256: download::sha256(&staging.join("payload")).unwrap(),
        staging,
        stage: Stage::BackedUp,
        storage: TransactionStorage::UserDirectory,
    };
    copy_file_synced(
        &journal.task.installation.executable,
        &backup_path(&root, &journal.task.installation, journal.storage),
    )
    .unwrap();
    (directory, root, journal)
}

#[test]
fn copies_and_flushes_read_only_source_without_overwriting_existing_file() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    fs::write(&source, b"verified bytes").unwrap();
    let mut permissions = fs::metadata(&source).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&source, permissions).unwrap();
    copy_file_synced(&source, &target).unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"verified bytes");
    assert!(fs::metadata(&target).unwrap().permissions().readonly());
    let error = copy_file_synced(&source, &target).unwrap_err();
    assert!(format!("{error:#}").contains("Cannot create copy target"));
    assert!(error.downcast_ref::<std::io::Error>().is_some());
    assert_eq!(fs::read(&target).unwrap(), b"verified bytes");
    remove(&source).unwrap();
    remove(&target).unwrap();
}

#[test]
fn unchanged_target_cleans_cancelled_elevated_transaction_without_authorization() {
    let (_directory, root, mut journal) = fixture();
    journal.task.elevate = true;
    atomic_json(&root.join("journal.json"), &journal).unwrap();
    rollback(&root).unwrap();
    assert_eq!(
        fs::read(&journal.task.installation.executable).unwrap(),
        b"old"
    );
    assert!(!root.join("journal.json").exists());
    assert!(!root.join("previous-executable.exe").exists());
    assert!(!journal.staging.exists());
}

#[test]
fn user_directory_backup_restores_changed_target_and_removes_only_its_transaction() {
    let (_directory, root, journal) = fixture();
    fs::write(&journal.task.installation.executable, b"uncommitted new").unwrap();
    fs::write(root.join("preferences.json"), b"keep").unwrap();
    atomic_json(&root.join("journal.json"), &journal).unwrap();
    rollback(&root).unwrap();
    assert_eq!(
        fs::read(&journal.task.installation.executable).unwrap(),
        b"old"
    );
    assert_eq!(fs::read(root.join("preferences.json")).unwrap(), b"keep");
    assert!(!root.join("journal.json").exists());
    assert!(!root.join("previous-executable.exe").exists());
}

#[test]
fn absent_storage_field_uses_legacy_layout_and_bad_layout_is_rejected() {
    let (_directory, root, journal) = fixture();
    let mut value = serde_json::to_value(&journal).unwrap();
    value.as_object_mut().unwrap().remove("storage");
    let mut old: Journal = serde_json::from_value(value).unwrap();
    assert_eq!(old.storage, TransactionStorage::Legacy);
    assert!(validate_journal(&root, &old).is_err());
    old.staging = old
        .task
        .installation
        .target
        .parent()
        .unwrap()
        .join(".kt-update-stage-legacy");
    validate_journal(&root, &old).unwrap();
    old.task.installation.target = PathBuf::from("/");
    assert!(validate_journal(&root, &old).is_err());
}

#[test]
fn startup_retains_failure_marker_until_matching_committed_gui_confirmation() {
    let (_directory, root, mut journal) = fixture();
    let service =
        crate::UpdateService::open(root.clone(), journal.task.installation.clone()).unwrap();
    atomic_json(&root.join("elevation.json"), &true).unwrap();
    service.confirm_startup().unwrap();
    assert!(root.join("elevation.json").exists());
    journal.stage = Stage::Committed;
    atomic_json(&root.join("journal.json"), &journal).unwrap();
    service.confirm_startup().unwrap();
    assert!(!root.join("elevation.json").exists());
    assert!(!root.join("journal.json").exists());
    assert!(!root.join("previous-executable.exe").exists());
}

#[test]
fn replacement_error_preserves_the_original_os_error() {
    let error = anyhow::Error::new(ReplaceFailure {
        target: PathBuf::from("application.exe"),
        kind: ReplaceKind::AccessDenied,
        source: std::io::Error::from_raw_os_error(5),
    });
    assert!(format!("{error:#}").contains("application.exe"));
    let original = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<std::io::Error>())
        .unwrap();
    assert_eq!(original.raw_os_error(), Some(5));
}

#[cfg(windows)]
#[test]
fn sharing_violation_preserves_both_files_and_never_creates_an_aside() {
    use std::os::windows::fs::OpenOptionsExt;
    let (_directory, root, journal) = fixture();
    let target = &journal.task.installation.target;
    let source = journal.staging.join("payload");
    let locked = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(target)
        .unwrap();
    let error = replace_file(&source, target).unwrap_err();
    assert_eq!(
        error.downcast_ref::<ReplaceFailure>().unwrap().kind,
        ReplaceKind::InUse
    );
    assert!(!target_access_denied(&error, target));
    drop(locked);
    assert_eq!(fs::read(target).unwrap(), b"old");
    assert_eq!(fs::read(source).unwrap(), b"new");
    assert!(!fs::read_dir(root.parent().unwrap())
        .unwrap()
        .any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(REPLACE_ASIDE_MARKER)));
}

#[cfg(windows)]
#[test]
fn error_five_with_sharing_conflict_is_in_use_and_read_only_targets_do_not_elevate() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ};
    let (_directory, _root, journal) = fixture();
    let target = &journal.task.installation.target;
    let locked = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .open(target)
        .unwrap();
    let failure = ReplaceFailure::new(target, std::io::Error::from_raw_os_error(5));
    assert_eq!(failure.kind, ReplaceKind::InUse);
    assert_eq!(failure.source.raw_os_error(), Some(5));
    assert!(!target_access_denied(&failure.into(), target));
    drop(locked);
    let mut permissions = fs::metadata(target).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(target, permissions).unwrap();
    let failure = ReplaceFailure::new(target, std::io::Error::from_raw_os_error(5));
    assert_eq!(failure.kind, ReplaceKind::AccessDenied);
    assert!(!target_access_denied(&failure.into(), target));
    remove(target).unwrap();
}
