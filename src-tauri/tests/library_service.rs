use std::fs;
use std::path::Path;

use personal_document_manager_lib::library::{
    DocumentProcessingStatus, IndexStatus, LibraryService, LocationStatus,
};
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

#[test]
fn imports_a_single_file_and_preserves_the_source() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("source").join("项目说明.md");
    let source_bytes = "第一版导入闭环\n".as_bytes();
    fs::create_dir_all(source_path.parent().unwrap()).unwrap();
    fs::write(&source_path, source_bytes).unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();

    let imported = service.import_document(&source_path).unwrap();

    assert_eq!(fs::read(&source_path).unwrap(), source_bytes);
    assert_eq!(source_path.file_name().unwrap(), "项目说明.md");
    assert_eq!(imported.title, "项目说明");
    assert_eq!(imported.file_name, "项目说明.md");
    assert_eq!(imported.file_type, "Markdown");
    assert_eq!(imported.file_size, source_bytes.len() as i64);
    assert_eq!(
        imported.content_hash,
        Some("f0e90aeef1ad3ef3666f7cf73a7938d958078cddb1dcfc6eed65a394d9940e18".to_string())
    );
    assert_eq!(imported.collection_id, "inbox");
    assert_eq!(imported.processing_status, DocumentProcessingStatus::Ready);
    assert_eq!(imported.index_status, IndexStatus::Pending);
    assert!(imported.source_path.ends_with("项目说明.md"));
    assert_eq!(
        imported.source_identifier,
        imported.source_path.to_ascii_lowercase()
    );
    assert_eq!(imported.last_imported_at, imported.imported_at);

    let library_copy = library_dir
        .join("documents")
        .join(&imported.id)
        .join(&imported.file_name);
    assert!(library_copy.is_file());
    assert_eq!(fs::read(library_copy).unwrap(), source_bytes);
    assert_eq!(service.list_documents().unwrap(), vec![imported]);
}

#[test]
fn rejects_an_unsupported_file_before_copying_it() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("installer.exe");
    fs::write(&source_path, b"not a document").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();

    let error = service.import_document(&source_path).unwrap_err();
    assert!(error.to_string().contains("不支持"));
    assert_eq!(
        fs::read_dir(library_dir.join("documents")).unwrap().count(),
        0
    );
    assert!(source_path.is_file());
}

#[test]
fn imports_every_supported_file_type() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();

    let supported = [
        ("report.pdf", "PDF"),
        ("report.docx", "DOCX"),
        ("notes.txt", "TXT"),
        ("readme.md", "Markdown"),
        ("scan.jpg", "JPG"),
        ("photo.png", "PNG"),
    ];

    for (file_name, expected_type) in supported {
        let source_path = root.path().join(file_name);
        fs::write(&source_path, file_name.as_bytes()).unwrap();
        let imported = service.import_document(&source_path).unwrap();
        assert_eq!(imported.file_type, expected_type);
        assert_eq!(
            service.import_document(&source_path).unwrap().file_type,
            expected_type
        );
    }

    assert_eq!(service.list_documents().unwrap().len(), supported.len() * 2);
}

fn is_library(path: &Path) -> bool {
    path.join(".pdm").join("library.json").is_file()
        && path.join(".pdm").join("library.sqlite3").is_file()
}
