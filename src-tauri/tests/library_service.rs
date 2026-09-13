use std::fs;
use std::path::Path;

use personal_document_manager_lib::library::{LibraryService, LocationStatus};
use serde_json::Value;
use tempfile::tempdir;

#[test]
fn creates_portable_library_and_reopens_it_from_recent_state() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("My Library");
    let mut service = LibraryService::new(&state_dir).unwrap();

    let inspection = service.inspect_location(&library_dir).unwrap();
    assert_eq!(inspection.status, LocationStatus::Usable);
    assert!(!inspection.is_existing_library);

    let created = service.create_library(&library_dir).unwrap();
    assert_eq!(created.name, "My Library");
    assert_eq!(created.path, library_dir.to_string_lossy());

    assert!(library_dir.join(".pdm").join("library.json").is_file());
    assert!(library_dir.join(".pdm").join("library.sqlite3").is_file());
    assert!(library_dir.join("documents").is_dir());
    assert!(library_dir.join("trash").is_dir());
    assert!(library_dir.join("thumbnails").is_dir());

    let metadata: Value =
        serde_json::from_slice(&fs::read(library_dir.join(".pdm").join("library.json")).unwrap())
            .unwrap();
    assert_eq!(metadata["formatVersion"], 1);
    assert_eq!(metadata["libraryId"], created.id);

    drop(service);

    let mut restarted = LibraryService::new(&state_dir).unwrap();
    let bootstrapped = restarted.bootstrap().unwrap();
    assert_eq!(bootstrapped.current_library.unwrap().id, created.id);
    assert_eq!(bootstrapped.recent_libraries.len(), 1);
    assert!(bootstrapped.recent_libraries[0].is_available);
}

#[test]
fn does_not_fall_back_to_an_older_library_when_the_last_one_is_missing() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let first_dir = root.path().join("First");
    let missing_last_dir = root.path().join("Missing Last");
    let mut service = LibraryService::new(&state_dir).unwrap();

    service.create_library(&first_dir).unwrap();
    service.create_library(&missing_last_dir).unwrap();
    drop(service);
    fs::remove_dir_all(&missing_last_dir).unwrap();

    let mut restarted = LibraryService::new(&state_dir).unwrap();
    let bootstrapped = restarted.bootstrap().unwrap();
    assert!(bootstrapped.current_library.is_none());
    assert_eq!(bootstrapped.recent_libraries.len(), 2);
    assert!(!bootstrapped.recent_libraries[0].is_available);
}

#[test]
fn switches_between_recent_libraries_without_deleting_them() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let first_dir = root.path().join("First");
    let second_dir = root.path().join("Second");
    let mut service = LibraryService::new(&state_dir).unwrap();

    let first = service.create_library(&first_dir).unwrap();
    let second = service.create_library(&second_dir).unwrap();
    assert_eq!(service.current_library().unwrap().id, second.id);

    let recent = service.list_recent_libraries();
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].path, second.path);

    let opened = service.open_library(&first_dir).unwrap();
    assert_eq!(opened.id, first.id);
    assert_eq!(service.current_library().unwrap().id, first.id);

    let remaining = service.forget_recent_library(&second_dir).unwrap();
    assert_eq!(remaining.len(), 1);
    assert!(second_dir.join(".pdm").join("library.json").is_file());
    assert!(second_dir.join("documents").is_dir());
}

#[test]
fn rejects_unsafe_or_non_empty_locations_before_creation() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let non_empty = root.path().join("not-empty");
    fs::create_dir_all(&non_empty).unwrap();
    fs::write(non_empty.join("existing.txt"), "keep").unwrap();

    let service = LibraryService::new(&state_dir).unwrap();
    let blocked = service.inspect_location(&non_empty).unwrap();
    assert_eq!(blocked.status, LocationStatus::Blocked);
    assert!(blocked.reason.unwrap().contains("不为空"));

    let file_path = root.path().join("plain-file");
    fs::write(&file_path, "not a directory").unwrap();
    let file_result = service.inspect_location(&file_path).unwrap();
    assert_eq!(file_result.status, LocationStatus::Blocked);
}

#[test]
fn warns_when_the_candidate_is_inside_a_cloud_sync_directory() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let cloud_path = root.path().join("OneDrive").join("Documents");
    let service = LibraryService::new(&state_dir).unwrap();

    let inspection = service.inspect_location(&cloud_path).unwrap();
    assert_eq!(inspection.status, LocationStatus::Usable);
    let warning = inspection.cloud_sync_warning.unwrap();
    assert_eq!(warning.provider, "OneDrive");
    assert!(warning.message.contains("同步冲突"));
}

#[test]
fn existing_library_is_detected_as_an_open_location() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Portable Library");
    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();

    let inspection = service.inspect_location(&library_dir).unwrap();
    assert_eq!(inspection.status, LocationStatus::ExistingLibrary);
    assert!(inspection.is_existing_library);
    assert!(is_library(Path::new(&inspection.path)));
}

fn is_library(path: &Path) -> bool {
    path.join(".pdm").join("library.json").is_file()
        && path.join(".pdm").join("library.sqlite3").is_file()
}
