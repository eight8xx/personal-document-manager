use std::fs;
use std::path::Path;

use personal_document_manager_lib::library::{
    DocumentMetadataUpdate, DocumentPreview, DocumentProcessingStatus, DocumentSearchFilters,
    DocumentSearchQuery, DocumentSearchResponse, DocumentThumbnail, ImportDecision,
    ImportItemStatus, IndexStatus, LibraryService, LocationStatus, SearchMatchKind,
};
use rusqlite::Connection;
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

    let supported: [(&str, &str, &[u8]); 6] = [
        ("report.pdf", "PDF", b"%PDF-1.4\nannual report"),
        ("report.docx", "DOCX", b"PK\x03\x04meeting notes"),
        ("notes.txt", "TXT", b"plain text"),
        ("readme.md", "Markdown", b"project readme"),
        ("scan.jpg", "JPG", b"\xff\xd8\xffscanned page"),
        ("photo.png", "PNG", b"\x89PNG\r\n\x1a\nproject photo"),
    ];

    for (file_name, expected_type, contents) in supported {
        let source_path = root.path().join(file_name);
        fs::write(&source_path, contents).unwrap();
        let batch = service
            .start_import(vec![source_path.to_string_lossy().into_owned()])
            .unwrap();
        let imported = batch.items[0].clone();
        assert_eq!(batch.imported_count, 1, "unexpected item: {imported:?}");
        assert_eq!(imported.status, ImportItemStatus::Imported);
        assert_eq!(imported.file_type.as_deref(), Some(expected_type));
    }

    assert_eq!(service.list_documents().unwrap().len(), supported.len());
}

#[test]
fn corrupt_documents_fail_validation_without_blocking_other_items() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let good_text = root.path().join("notes.txt");
    let corrupt_pdf = root.path().join("broken.pdf");
    let corrupt_image = root.path().join("broken.png");
    fs::write(&good_text, "可搜索的正文").unwrap();
    fs::write(&corrupt_pdf, "these bytes are not a pdf").unwrap();
    fs::write(&corrupt_image, "these bytes are not a png").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();

    let batch = service
        .start_import(vec![
            good_text.to_string_lossy().into_owned(),
            corrupt_pdf.to_string_lossy().into_owned(),
            corrupt_image.to_string_lossy().into_owned(),
        ])
        .unwrap();

    assert_eq!(batch.imported_count, 1);
    assert_eq!(batch.failed_count, 2);
    let failures = batch
        .items
        .iter()
        .filter(|item| item.status == ImportItemStatus::Failed)
        .collect::<Vec<_>>();
    assert_eq!(failures.len(), 2);
    assert!(failures
        .iter()
        .all(|item| item.error_stage.as_deref() == Some("validate")));
    assert!(failures.iter().all(|item| item
        .error_message
        .as_deref()
        .unwrap_or_default()
        .contains("不是有效")));
    assert!(failures.iter().all(|item| item.retryable));

    let documents = service.list_documents().unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].file_name, "notes.txt");
    assert_eq!(
        fs::read_dir(library_dir.join("documents")).unwrap().count(),
        1
    );
    assert!(corrupt_pdf.is_file());
    assert!(corrupt_image.is_file());
}

#[test]
fn batch_import_handles_success_ignored_and_failed_items_independently() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_dir = root.path().join("source");
    let scan_root = root.path().join("scan-source");
    let nested_dir = scan_root.join("nested");
    let explicit_source = source_dir.join("explicit.txt");
    let scanned_source = nested_dir.join("scanned.md");
    let unsupported_in_directory = nested_dir.join("ignored.exe");
    let explicit_unsupported = root.path().join("broken.pdf.exe");
    let missing_source = source_dir.join("missing.txt");

    fs::create_dir_all(&source_dir).unwrap();
    fs::create_dir_all(&nested_dir).unwrap();
    fs::write(&explicit_source, "explicit contents").unwrap();
    fs::write(&scanned_source, "scanned contents").unwrap();
    fs::write(&unsupported_in_directory, "unsupported").unwrap();
    fs::write(&explicit_unsupported, "unsupported").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let batch = service
        .start_import(vec![
            explicit_source.to_string_lossy().into_owned(),
            scan_root.to_string_lossy().into_owned(),
            explicit_unsupported.to_string_lossy().into_owned(),
            missing_source.to_string_lossy().into_owned(),
        ])
        .unwrap();

    assert_eq!(batch.imported_count, 2);
    assert_eq!(batch.ignored_count, 1);
    assert_eq!(batch.failed_count, 2);
    assert_eq!(batch.duplicate_count, 0);
    assert_eq!(batch.source_changed_count, 0);
    assert_eq!(batch.items.len(), 5);

    let ignored = batch
        .items
        .iter()
        .find(|item| item.status == ImportItemStatus::Ignored)
        .unwrap();
    assert_eq!(ignored.file_name, "ignored.exe");
    assert_eq!(ignored.file_type, None);
    assert!(!ignored.retryable);

    let failures = batch
        .items
        .iter()
        .filter(|item| item.status == ImportItemStatus::Failed)
        .collect::<Vec<_>>();
    assert_eq!(failures.len(), 2);
    assert!(failures.iter().all(|item| item.retryable));
    assert!(failures
        .iter()
        .all(|item| item.error_stage.as_deref() == Some("scan")));
    assert!(failures
        .iter()
        .all(|item| item.error_message.as_deref().is_some()));

    let documents = service.list_documents().unwrap();
    assert_eq!(documents.len(), 2);
    assert!(documents.iter().all(|document| {
        library_dir
            .join("documents")
            .join(&document.id)
            .join(&document.file_name)
            .is_file()
    }));
}

#[test]
fn duplicate_imports_wait_for_use_existing_import_anyway_or_cancel() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let first_source = root.path().join("first.txt");
    let second_source = root.path().join("second.txt");
    let contents = b"same contents";
    fs::write(&first_source, contents).unwrap();
    fs::write(&second_source, contents).unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();

    let first = service
        .start_import(vec![first_source.to_string_lossy().into_owned()])
        .unwrap()
        .items
        .remove(0);
    let existing_document_id = first.document_id.unwrap();

    let duplicate = service
        .start_import(vec![first_source.to_string_lossy().into_owned()])
        .unwrap()
        .items
        .remove(0);
    assert_eq!(duplicate.status, ImportItemStatus::Duplicate);
    assert_eq!(
        duplicate.duplicate_document_id.as_deref(),
        Some(existing_document_id.as_str())
    );

    let mismatch = service
        .resolve_import_item(&duplicate.item_id, ImportDecision::CreateNew)
        .unwrap_err();
    assert_eq!(mismatch.code(), "invalidImportDecision");

    let skipped = service
        .resolve_import_item(&duplicate.item_id, ImportDecision::UseExisting)
        .unwrap();
    assert_eq!(skipped.status, ImportItemStatus::Skipped);
    assert_eq!(
        skipped.document_id.as_deref(),
        Some(existing_document_id.as_str())
    );
    assert_eq!(service.list_documents().unwrap().len(), 1);

    let duplicate = service
        .start_import(vec![second_source.to_string_lossy().into_owned()])
        .unwrap()
        .items
        .remove(0);
    let imported = service
        .resolve_import_item(&duplicate.item_id, ImportDecision::ImportAnyway)
        .unwrap();
    assert_eq!(imported.status, ImportItemStatus::Imported);
    assert_ne!(
        imported.document_id.as_deref(),
        Some(existing_document_id.as_str())
    );
    assert_eq!(service.list_documents().unwrap().len(), 2);

    let duplicate = service
        .start_import(vec![second_source.to_string_lossy().into_owned()])
        .unwrap()
        .items
        .remove(0);
    let cancelled = service
        .resolve_import_item(&duplicate.item_id, ImportDecision::Cancel)
        .unwrap();
    assert_eq!(cancelled.status, ImportItemStatus::Skipped);
    assert_eq!(service.list_documents().unwrap().len(), 2);

    let error = service
        .resolve_import_item("missing-item", ImportDecision::Cancel)
        .unwrap_err();
    assert_eq!(error.code(), "importItemNotFound");
}

#[test]
fn changed_source_can_create_or_replace_without_retaining_the_old_library_copy() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("changing.txt");
    fs::write(&source_path, "first version").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    service
        .start_import(vec![source_path.to_string_lossy().into_owned()])
        .unwrap();

    fs::write(&source_path, "second version").unwrap();
    let changed = service
        .start_import(vec![source_path.to_string_lossy().into_owned()])
        .unwrap()
        .items
        .remove(0);
    assert_eq!(changed.status, ImportItemStatus::SourceChanged);
    let created = service
        .resolve_import_item(&changed.item_id, ImportDecision::CreateNew)
        .unwrap();
    assert_eq!(created.status, ImportItemStatus::Imported);
    assert_eq!(service.list_documents().unwrap().len(), 2);

    fs::write(&source_path, "third version, replacement contents").unwrap();
    let changed = service
        .start_import(vec![source_path.to_string_lossy().into_owned()])
        .unwrap()
        .items
        .remove(0);
    assert_eq!(changed.status, ImportItemStatus::SourceChanged);
    let target_document_id = changed.duplicate_document_id.clone().unwrap();
    let old_document = service
        .list_documents()
        .unwrap()
        .into_iter()
        .find(|document| document.id == target_document_id)
        .unwrap();
    let old_copy = library_dir
        .join("documents")
        .join(&old_document.id)
        .join(&old_document.file_name);
    assert!(old_copy.is_file());
    let old_contents = fs::read(&old_copy).unwrap();

    let replaced = service
        .resolve_import_item(&changed.item_id, ImportDecision::ReplaceExisting)
        .unwrap();
    assert_eq!(replaced.status, ImportItemStatus::Imported);
    assert_eq!(
        replaced.document_id.as_deref(),
        Some(target_document_id.as_str())
    );
    assert_ne!(
        fs::read(&old_copy).unwrap(),
        old_contents,
        "old library copy contents must not be retained"
    );
    let documents = service.list_documents().unwrap();
    assert_eq!(documents.len(), 2);
    let updated = documents
        .iter()
        .find(|document| document.id == target_document_id)
        .unwrap();
    let new_copy = library_dir
        .join("documents")
        .join(&updated.id)
        .join(&updated.file_name);
    assert!(new_copy.is_file());
    assert_eq!(
        fs::read(&new_copy).unwrap(),
        b"third version, replacement contents"
    );
}

#[test]
fn failed_import_items_can_be_retried_without_processing_successful_items() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("recovered.txt");

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let failed = service
        .start_import(vec![source_path.to_string_lossy().into_owned()])
        .unwrap()
        .items
        .remove(0);
    assert_eq!(failed.status, ImportItemStatus::Failed);
    assert!(failed.retryable);
    assert_eq!(failed.error_stage.as_deref(), Some("scan"));

    fs::write(&source_path, "recovered contents").unwrap();
    let retried = service.retry_import_item(&failed.item_id).unwrap();
    assert_eq!(retried.item_id, failed.item_id);
    assert_eq!(retried.status, ImportItemStatus::Imported);
    assert!(retried.document_id.is_some());
    assert_eq!(service.list_documents().unwrap().len(), 1);

    let error = service.retry_import_item(&failed.item_id).unwrap_err();
    assert_eq!(error.code(), "importItemNotFound");
}

#[test]
fn copy_failures_are_reported_and_retryable_without_reprocessing_successes() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("copy-retry.txt");
    fs::write(&source_path, "copy retry contents").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    fs::remove_dir(library_dir.join("documents")).unwrap();
    fs::write(library_dir.join("documents"), "blocked").unwrap();

    let failed = service
        .start_import(vec![source_path.to_string_lossy().into_owned()])
        .unwrap()
        .items
        .remove(0);
    assert_eq!(failed.status, ImportItemStatus::Failed);
    assert_eq!(failed.error_stage.as_deref(), Some("copy"));
    assert!(failed.retryable);
    assert!(service.list_documents().unwrap().is_empty());

    fs::remove_file(library_dir.join("documents")).unwrap();
    fs::create_dir(library_dir.join("documents")).unwrap();
    let retried = service.retry_import_item(&failed.item_id).unwrap();
    assert_eq!(retried.status, ImportItemStatus::Imported);
    assert_eq!(service.list_documents().unwrap().len(), 1);
}

#[test]
fn collections_support_nested_operations_and_reject_cycles() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();

    let initial = service.list_collections().unwrap();
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0].id, "inbox");
    assert!(initial[0].is_inbox);

    let parent = service.create_collection("Work".to_string(), None).unwrap();
    let child = service
        .create_collection("Projects".to_string(), Some(parent.id.clone()))
        .unwrap();
    let grandchild = service
        .create_collection("2026".to_string(), Some(child.id.clone()))
        .unwrap();

    let renamed = service
        .rename_collection(&child.id, "Active Projects".to_string())
        .unwrap();
    assert_eq!(renamed.name, "Active Projects");

    let moved = service.move_collection(&grandchild.id, None).unwrap();
    assert_eq!(moved.parent_id, None);
    let moved_back = service
        .move_collection(&grandchild.id, Some(parent.id.clone()))
        .unwrap();
    assert_eq!(moved_back.parent_id.as_deref(), Some(parent.id.as_str()));

    let cycle = service
        .move_collection(&parent.id, Some(grandchild.id))
        .unwrap_err();
    assert_eq!(cycle.code(), "collectionCycle");

    let rename_error = service
        .rename_collection("inbox", "Inbox".to_string())
        .unwrap_err();
    assert_eq!(rename_error.code(), "protectedCollection");
    let move_error = service.move_collection("inbox", None).unwrap_err();
    assert_eq!(move_error.code(), "protectedCollection");
    let delete_error = service.delete_collection("inbox").unwrap_err();
    assert_eq!(delete_error.code(), "protectedCollection");
}

#[test]
fn deleting_a_collection_reparents_children_and_moves_direct_documents_to_inbox() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("document.txt");
    fs::write(&source_path, "document contents").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let parent = service
        .create_collection("Parent".to_string(), None)
        .unwrap();
    let child = service
        .create_collection("Child".to_string(), Some(parent.id.clone()))
        .unwrap();
    let grandchild = service
        .create_collection("Grandchild".to_string(), Some(child.id.clone()))
        .unwrap();
    let imported = service
        .start_import(vec![source_path.to_string_lossy().into_owned()])
        .unwrap()
        .items
        .remove(0);
    let document_id = imported.document_id.unwrap();

    let moved = service
        .move_document_to_collection(&document_id, &child.id)
        .unwrap();
    assert_eq!(moved.collection_id, child.id);
    let library_copy = library_dir
        .join("documents")
        .join(&moved.id)
        .join(&moved.file_name);
    assert!(library_copy.is_file());

    let deleted = service.delete_collection(&child.id).unwrap();
    assert_eq!(deleted.collection_id, child.id);
    assert_eq!(deleted.target_collection_id, "inbox");
    assert_eq!(deleted.moved_document_count, 1);
    assert!(library_copy.is_file());

    let collections = service.list_collections().unwrap();
    assert!(!collections
        .iter()
        .any(|collection| collection.id == child.id));
    let reparented = collections
        .iter()
        .find(|collection| collection.id == grandchild.id)
        .unwrap();
    assert_eq!(reparented.parent_id.as_deref(), Some(parent.id.as_str()));
    let inbox = collections
        .iter()
        .find(|collection| collection.id == "inbox")
        .unwrap();
    assert_eq!(inbox.document_count, 1);
    let document = service
        .list_documents()
        .unwrap()
        .into_iter()
        .find(|document| document.id == document_id)
        .unwrap();
    assert_eq!(document.collection_id, "inbox");

    let missing_target = service
        .move_document_to_collection(&document_id, "missing-collection")
        .unwrap_err();
    assert_eq!(missing_target.code(), "collectionNotFound");
}

#[test]
fn manages_tag_lifecycle_and_multiple_document_tags_without_deleting_documents() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("project.md");
    fs::write(&source_path, "project metadata").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let imported = service.import_document(&source_path).unwrap();
    let original_file_name = imported.file_name.clone();
    let original_source_path = imported.source_path.clone();
    let original_source_identifier = imported.source_identifier.clone();

    let work = service.create_tag("工作".to_string()).unwrap();
    let important = service.create_tag("重要".to_string()).unwrap();
    assert_eq!(work.document_count, 0);
    assert!(service.create_tag("工作".to_string()).is_err());
    assert!(service.create_tag("   ".to_string()).is_err());

    let updated = service
        .update_document_metadata(
            &imported.id,
            DocumentMetadataUpdate {
                title: "项目规划".to_string(),
                description: Some("第一版规划".to_string()),
                document_date: Some("2024-03-18".to_string()),
                collection_id: "inbox".to_string(),
                tag_ids: vec![work.id.clone(), important.id.clone()],
            },
        )
        .unwrap();
    assert_eq!(updated.title, "项目规划");
    assert_eq!(updated.description.as_deref(), Some("第一版规划"));
    assert_eq!(updated.document_date.as_deref(), Some("2024-03-18"));
    assert_eq!(updated.file_name, original_file_name);
    assert_eq!(updated.source_path, original_source_path);
    assert_eq!(updated.source_identifier, original_source_identifier);
    assert_eq!(
        updated
            .tags
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>(),
        vec!["工作", "重要"]
    );

    let renamed = service.rename_tag(&work.id, "项目".to_string()).unwrap();
    assert_eq!(renamed.name, "项目");
    assert_eq!(renamed.document_count, 1);
    let documents = service.list_documents().unwrap();
    assert_eq!(documents.len(), 1);
    assert!(documents[0].tags.iter().any(|tag| tag.name == "项目"));

    let without_important = service
        .remove_tag_from_document(&imported.id, &important.id)
        .unwrap();
    assert_eq!(without_important.tags.len(), 1);
    let with_important_again = service
        .add_tag_to_document(&imported.id, &important.id)
        .unwrap();
    assert_eq!(with_important_again.tags.len(), 2);

    service.delete_tag(&important.id).unwrap();
    let remaining_tags = service.list_tags().unwrap();
    assert_eq!(remaining_tags.len(), 1);
    assert_eq!(remaining_tags[0].name, "项目");
    assert_eq!(remaining_tags[0].document_count, 1);

    let remaining_document = service.list_documents().unwrap().remove(0);
    assert_eq!(remaining_document.title, "项目规划");
    assert_eq!(remaining_document.tags.len(), 1);
    assert_eq!(remaining_document.tags[0].name, "项目");
    let library_copy = library_dir
        .join("documents")
        .join(&remaining_document.id)
        .join(&remaining_document.file_name);
    assert!(library_copy.is_file());
}

#[test]
fn updates_all_document_metadata_and_allows_an_empty_document_date() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("report.txt");
    fs::write(&source_path, "report contents").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let imported = service.import_document(&source_path).unwrap();
    let tag = service.create_tag("报告".to_string()).unwrap();

    let dated = service
        .update_document_metadata(
            &imported.id,
            DocumentMetadataUpdate {
                title: "年度报告".to_string(),
                description: Some("2025 年总结".to_string()),
                document_date: Some("2019-12-31".to_string()),
                collection_id: "inbox".to_string(),
                tag_ids: vec![tag.id.clone()],
            },
        )
        .unwrap();
    assert_eq!(dated.document_date.as_deref(), Some("2019-12-31"));
    assert_ne!(
        dated.document_date.as_deref(),
        dated.imported_at.get(..10),
        "文档日期必须独立于导入时间"
    );

    let undated = service
        .update_document_metadata(
            &imported.id,
            DocumentMetadataUpdate {
                title: "年度报告（未定稿）".to_string(),
                description: None,
                document_date: None,
                collection_id: "inbox".to_string(),
                tag_ids: vec![tag.id],
            },
        )
        .unwrap();
    assert_eq!(undated.title, "年度报告（未定稿）");
    assert_eq!(undated.description, None);
    assert_eq!(undated.document_date, None);
    assert_eq!(undated.file_name, imported.file_name);
    assert_eq!(undated.source_path, imported.source_path);
    assert_eq!(undated.tags.len(), 1);

    let invalid_date = service
        .update_document_metadata(
            &imported.id,
            DocumentMetadataUpdate {
                title: "报告".to_string(),
                description: None,
                document_date: Some("2024-13-40".to_string()),
                collection_id: "inbox".to_string(),
                tag_ids: Vec::new(),
            },
        )
        .unwrap_err();
    assert_eq!(invalid_date.code(), "invalidDocumentMetadata");

    let empty_title = service
        .update_document_metadata(
            &imported.id,
            DocumentMetadataUpdate {
                title: "  ".to_string(),
                description: None,
                document_date: None,
                collection_id: "inbox".to_string(),
                tag_ids: Vec::new(),
            },
        )
        .unwrap_err();
    assert_eq!(empty_title.code(), "invalidDocumentMetadata");
}

#[test]
fn previews_real_library_text_docx_and_pdf_contents() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let text_path = root.path().join("notes.txt");
    let markdown_path = root.path().join("readme.md");
    let docx_path = root.path().join("meeting.docx");
    let pdf_path = root.path().join("report.pdf");
    fs::write(&text_path, "纯文本预览内容").unwrap();
    fs::write(&markdown_path, "# Markdown\n\n只读文本内容").unwrap();
    let docx_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>会议</w:t></w:r><w:r><w:t>记录</w:t></w:r></w:p>
    <w:p><w:r><w:t>第二段</w:t><w:br/><w:t>续行</w:t></w:r></w:p>
  </w:body>
</w:document>"#;
    fs::write(
        &docx_path,
        stored_zip(&[("word/document.xml", docx_xml.as_bytes())]),
    )
    .unwrap();
    fs::write(
        &pdf_path,
        b"%PDF-1.4\n1 0 obj << /Type /Pages /Count 2 >> endobj\n%%EOF",
    )
    .unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let text = service.import_document(&text_path).unwrap();
    let markdown = service.import_document(&markdown_path).unwrap();
    let docx = service.import_document(&docx_path).unwrap();
    let pdf = service.import_document(&pdf_path).unwrap();

    assert_eq!(
        service.get_document_preview(&text.id).unwrap(),
        DocumentPreview::Text {
            text: "纯文本预览内容".to_string()
        }
    );
    assert_eq!(
        service.get_document_preview(&markdown.id).unwrap(),
        DocumentPreview::Text {
            text: "# Markdown\n\n只读文本内容".to_string()
        }
    );
    let DocumentPreview::Docx { text, notice } = service.get_document_preview(&docx.id).unwrap()
    else {
        panic!("DOCX should return extracted text");
    };
    assert_eq!(text, "会议记录\n第二段\n续行");
    assert!(notice.contains("不是完整版式预览"));

    let DocumentPreview::Pdf {
        data_url,
        page_count,
    } = service.get_document_preview(&pdf.id).unwrap()
    else {
        panic!("PDF should return an inline document preview");
    };
    assert!(data_url.starts_with("data:application/pdf;base64,"));
    assert_eq!(page_count, Some(2));
}

#[test]
fn generates_thumbnails_and_reports_missing_library_copies_without_changing_state() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let image_path = root.path().join("pixel.png");
    let pdf_path = root.path().join("report.pdf");
    fs::write(
        &image_path,
        [
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H',
            b'D', b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89,
        ],
    )
    .unwrap();
    fs::write(&pdf_path, b"%PDF-1.4\n%%EOF").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let image = service.import_document(&image_path).unwrap();
    let pdf = service.import_document(&pdf_path).unwrap();

    let DocumentThumbnail::Image { data_url } = service.get_document_thumbnail(&image.id).unwrap()
    else {
        panic!("image thumbnail should be available");
    };
    assert!(data_url.starts_with("data:image/png;base64,"));
    assert!(matches!(
        service.get_document_thumbnail(&pdf.id).unwrap(),
        DocumentThumbnail::Pdf { .. }
    ));

    let documents_before = service.list_documents().unwrap();
    let image_copy = library_dir
        .join("documents")
        .join(&image.id)
        .join(&image.file_name);
    fs::remove_file(&image_copy).unwrap();

    assert!(matches!(
        service.get_document_thumbnail(&image.id).unwrap(),
        DocumentThumbnail::Fallback { .. }
    ));
    let preview_error = service.get_document_preview(&image.id).unwrap_err();
    assert_eq!(preview_error.code(), "documentFileMissing");
    assert!(preview_error.to_string().contains("资料库副本不存在"));
    let open_error = service.open_document(&image.id).unwrap_err();
    assert_eq!(open_error.code(), "documentFileMissing");
    assert_eq!(service.list_documents().unwrap(), documents_before);
}

#[test]
fn searches_chinese_english_and_mixed_content_with_snippets() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let chinese_path = root.path().join("中文资料.txt");
    let english_path = root.path().join("annual-report.md");
    let mixed_path = root.path().join("mixed.pdf");
    let docx_path = root.path().join("meeting.docx");
    let image_path = root.path().join("扫描图.png");

    fs::write(&chinese_path, "项目计划\n这是中文正文，用于检索完整内容。").unwrap();
    fs::write(
        &english_path,
        "Annual report\nRevenue increased across every region.",
    )
    .unwrap();
    let mixed_pdf =
        "%PDF-1.4\n1 0 obj << /Length 64 >>\nstream\nBT (Project Alpha 计划) Tj ET\nendstream\n%%EOF";
    fs::write(&mixed_path, mixed_pdf.as_bytes()).unwrap();
    let docx_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body><w:p><w:r><w:t>会议纪要</w:t></w:r></w:p></w:body>
</w:document>"#;
    fs::write(
        &docx_path,
        stored_zip(&[("word/document.xml", docx_xml.as_bytes())]),
    )
    .unwrap();
    fs::write(&image_path, png_prefix()).unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let chinese = service.import_document(&chinese_path).unwrap();
    let english = service.import_document(&english_path).unwrap();
    let mixed = service.import_document(&mixed_path).unwrap();
    let docx = service.import_document(&docx_path).unwrap();
    let image = service.import_document(&image_path).unwrap();

    let indexed = service.index_pending_documents().unwrap();
    assert_eq!(indexed.processed, 5);
    assert_eq!(indexed.searchable, 5);
    assert_eq!(indexed.failed, 0);

    let chinese_results = search(&service, "中文正文", DocumentSearchFilters::default());
    assert_eq!(chinese_results.len(), 1);
    assert_eq!(chinese_results[0].document.id, chinese.id);
    assert_eq!(chinese_results[0].match_kind, SearchMatchKind::Content);
    assert!(chinese_results[0]
        .snippet
        .as_deref()
        .unwrap_or_default()
        .contains("中文正文"));

    let english_results = search(
        &service,
        "revenue increased",
        DocumentSearchFilters::default(),
    );
    assert_eq!(english_results.len(), 1);
    assert_eq!(english_results[0].document.id, english.id);
    assert_eq!(english_results[0].match_kind, SearchMatchKind::Content);

    let mixed_results = search(&service, "Alpha 计划", DocumentSearchFilters::default());
    assert_eq!(mixed_results.len(), 1);
    assert_eq!(mixed_results[0].document.id, mixed.id);
    assert!(mixed_results[0]
        .snippet
        .as_deref()
        .unwrap_or_default()
        .contains("Alpha"));

    let docx_results = search(&service, "会议纪要", DocumentSearchFilters::default());
    assert_eq!(docx_results.len(), 1);
    assert_eq!(docx_results[0].document.id, docx.id);

    let image_results = search(&service, "扫描图", DocumentSearchFilters::default());
    assert_eq!(image_results.len(), 1);
    assert_eq!(image_results[0].document.id, image.id);
    assert_eq!(image_results[0].match_kind, SearchMatchKind::Metadata);
}

#[test]
fn supports_short_metadata_queries_and_combined_filters() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let matching_path = root.path().join("ai-plan.txt");
    let other_path = root.path().join("budget.md");
    fs::write(&matching_path, "正文不包含查询短词。").unwrap();
    fs::write(&other_path, "another document").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let matching = service.import_document(&matching_path).unwrap();
    let other = service.import_document(&other_path).unwrap();
    service.index_pending_documents().unwrap();

    let collection = service.create_collection("工作".to_string(), None).unwrap();
    let tag = service.create_tag("重要".to_string()).unwrap();
    service
        .update_document_metadata(
            &matching.id,
            DocumentMetadataUpdate {
                title: "AI 计划".to_string(),
                description: Some("季度路线图".to_string()),
                document_date: Some("2025-03-15".to_string()),
                collection_id: collection.id.clone(),
                tag_ids: vec![tag.id.clone()],
            },
        )
        .unwrap();
    service
        .update_document_metadata(
            &other.id,
            DocumentMetadataUpdate {
                title: "预算".to_string(),
                description: None,
                document_date: Some("2024-01-01".to_string()),
                collection_id: "inbox".to_string(),
                tag_ids: Vec::new(),
            },
        )
        .unwrap();

    let short_results = search(&service, "AI", DocumentSearchFilters::default());
    assert_eq!(short_results.len(), 1);
    assert_eq!(short_results[0].document.id, matching.id);
    assert_eq!(short_results[0].match_kind, SearchMatchKind::Metadata);

    let combined = search(
        &service,
        "",
        DocumentSearchFilters {
            collection_id: Some(collection.id.clone()),
            tag_id: Some(tag.id),
            file_type: Some("TXT".to_string()),
            document_date_from: Some("2025-01-01".to_string()),
            document_date_to: Some("2025-12-31".to_string()),
        },
    );
    assert_eq!(combined.len(), 1);
    assert_eq!(combined[0].document.id, matching.id);

    let cleared = search(&service, "", DocumentSearchFilters::default());
    assert_eq!(cleared.len(), 2);
}

#[test]
fn updates_metadata_search_results_and_marks_failed_index_retryable() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("recover.txt");
    fs::write(&source_path, "初始正文").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let imported = service.import_document(&source_path).unwrap();
    let library_copy = library_dir
        .join("documents")
        .join(&imported.id)
        .join(&imported.file_name);
    fs::remove_file(&library_copy).unwrap();

    let failed_run = service.index_pending_documents().unwrap();
    assert_eq!(failed_run.failed, 1);
    let failed = service
        .list_documents()
        .unwrap()
        .into_iter()
        .find(|document| document.id == imported.id)
        .unwrap();
    assert_eq!(failed.index_status, IndexStatus::Failed);
    assert!(failed
        .error_message
        .as_deref()
        .unwrap_or_default()
        .contains("不存在"));

    let still_failed = service.retry_document_index(&imported.id).unwrap();
    assert_eq!(still_failed.index_status, IndexStatus::Failed);
    let metadata_results = search(&service, "recover", DocumentSearchFilters::default());
    assert_eq!(metadata_results.len(), 1, "失败文档仍应可按元数据浏览");

    fs::write(&library_copy, "恢复后的正文内容").unwrap();
    let recovered = service.retry_document_index(&imported.id).unwrap();
    assert_eq!(recovered.index_status, IndexStatus::Searchable);
    let recovered_results = search(&service, "恢复后的正文", DocumentSearchFilters::default());
    assert_eq!(recovered_results.len(), 1);
    assert_eq!(recovered_results[0].document.id, imported.id);

    service
        .update_document_metadata(
            &imported.id,
            DocumentMetadataUpdate {
                title: "旧标题资料".to_string(),
                description: Some("旧说明文本".to_string()),
                document_date: Some("2025-01-10".to_string()),
                collection_id: "inbox".to_string(),
                tag_ids: Vec::new(),
            },
        )
        .unwrap();
    service
        .update_document_metadata(
            &imported.id,
            DocumentMetadataUpdate {
                title: "新标题资料".to_string(),
                description: Some("更新后的说明文本".to_string()),
                document_date: Some("2026-01-10".to_string()),
                collection_id: "inbox".to_string(),
                tag_ids: Vec::new(),
            },
        )
        .unwrap();
    assert_eq!(
        search(&service, "新标题", DocumentSearchFilters::default()).len(),
        1
    );
    assert!(search(&service, "旧说明文本", DocumentSearchFilters::default()).is_empty());
}

#[test]
fn soft_deletes_restores_and_keeps_trash_across_restarts() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("plan.txt");
    let source_bytes = "项目计划正文，可被搜索。".as_bytes();
    fs::write(&source_path, source_bytes).unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let collection = service.create_collection("项目".to_string(), None).unwrap();
    let tag = service.create_tag("重要".to_string()).unwrap();
    let imported = service.import_document(&source_path).unwrap();
    service.index_pending_documents().unwrap();
    service
        .update_document_metadata(
            &imported.id,
            DocumentMetadataUpdate {
                title: "项目计划".to_string(),
                description: None,
                document_date: None,
                collection_id: collection.id.clone(),
                tag_ids: vec![tag.id.clone()],
            },
        )
        .unwrap();
    let library_copy = library_dir
        .join("documents")
        .join(&imported.id)
        .join(&imported.file_name);
    assert!(library_copy.is_file());
    assert_eq!(
        search(&service, "项目计划正文", DocumentSearchFilters::default()).len(),
        1
    );

    service.move_document_to_trash(&imported.id).unwrap();

    assert!(service.list_documents().unwrap().is_empty());
    assert!(search(&service, "项目计划正文", DocumentSearchFilters::default()).is_empty());
    assert_eq!(fs::read(&source_path).unwrap(), source_bytes);
    assert!(library_copy.is_file());
    let collections = service.list_collections().unwrap();
    let project = collections
        .iter()
        .find(|item| item.id == collection.id)
        .unwrap();
    assert_eq!(project.document_count, 0);
    assert_eq!(service.list_tags().unwrap()[0].document_count, 0);

    let trash = service.list_trash_documents().unwrap();
    assert_eq!(trash.len(), 1);
    assert_eq!(trash[0].document.id, imported.id);
    assert_eq!(
        trash[0].original_collection_id.as_deref(),
        Some(collection.id.as_str())
    );
    assert_eq!(trash[0].original_collection_name.as_deref(), Some("项目"));
    assert_eq!(trash[0].document.tags.len(), 1);

    drop(service);
    let mut restarted = LibraryService::new(&state_dir).unwrap();
    restarted.bootstrap().unwrap();
    assert_eq!(restarted.list_trash_documents().unwrap().len(), 1);

    let restored = restarted.restore_document(&imported.id).unwrap();
    assert_eq!(restored.collection_id, collection.id);
    assert!(restarted.list_trash_documents().unwrap().is_empty());
    assert_eq!(
        search(&restarted, "项目计划正文", DocumentSearchFilters::default()).len(),
        1
    );
    assert!(library_copy.is_file());
}

#[test]
fn restores_to_inbox_when_the_original_collection_was_deleted() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("archive.txt");
    fs::write(&source_path, "待归档正文").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let collection = service
        .create_collection("临时集合".to_string(), None)
        .unwrap();
    let imported = service.import_document(&source_path).unwrap();
    service
        .move_document_to_collection(&imported.id, &collection.id)
        .unwrap();
    service.move_document_to_trash(&imported.id).unwrap();
    service.delete_collection(&collection.id).unwrap();

    let trash = service.list_trash_documents().unwrap();
    assert_eq!(trash.len(), 1);
    assert_eq!(
        trash[0].original_collection_id.as_deref(),
        Some(collection.id.as_str())
    );
    assert_eq!(trash[0].original_collection_name, None);

    let restored = service.restore_document(&imported.id).unwrap();
    assert_eq!(restored.collection_id, "inbox");
    let inbox = service
        .list_collections()
        .unwrap()
        .into_iter()
        .find(|item| item.id == "inbox")
        .unwrap();
    assert_eq!(inbox.document_count, 1);
}

#[test]
fn permanently_deletes_records_index_tags_and_library_copy_but_not_source() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("report.txt");
    let source_bytes = "永久删除前必须索引到的正文。".as_bytes();
    fs::write(&source_path, source_bytes).unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let tag = service.create_tag("报告".to_string()).unwrap();
    let imported = service.import_document(&source_path).unwrap();
    service
        .update_document_metadata(
            &imported.id,
            DocumentMetadataUpdate {
                title: "年度报告".to_string(),
                description: Some("删除测试".to_string()),
                document_date: None,
                collection_id: "inbox".to_string(),
                tag_ids: vec![tag.id.clone()],
            },
        )
        .unwrap();
    service.index_pending_documents().unwrap();
    let library_copy = library_dir
        .join("documents")
        .join(&imported.id)
        .join(&imported.file_name);
    assert!(library_copy.is_file());

    service.move_document_to_trash(&imported.id).unwrap();
    service.permanently_delete_document(&imported.id).unwrap();

    assert_eq!(fs::read(&source_path).unwrap(), source_bytes);
    assert!(!library_copy.exists());
    assert!(!library_copy.parent().unwrap().exists());
    assert!(service.list_documents().unwrap().is_empty());
    assert!(service.list_trash_documents().unwrap().is_empty());
    assert!(search(
        &service,
        "永久删除前必须索引到的正文",
        DocumentSearchFilters::default()
    )
    .is_empty());
    assert_eq!(service.list_tags().unwrap()[0].document_count, 0);

    let connection = Connection::open(library_dir.join(".pdm").join("library.sqlite3")).unwrap();
    for table in ["documents", "document_tags", "document_search", "sources"] {
        let count: i64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "{table} should be empty");
    }
}

#[test]
fn empties_trash_and_removes_every_library_copy() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let first_source = root.path().join("first.txt");
    let second_source = root.path().join("second.md");
    fs::write(&first_source, "first").unwrap();
    fs::write(&second_source, "second").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let first = service.import_document(&first_source).unwrap();
    let second = service.import_document(&second_source).unwrap();
    let first_copy = library_dir
        .join("documents")
        .join(&first.id)
        .join(&first.file_name);
    let second_copy = library_dir
        .join("documents")
        .join(&second.id)
        .join(&second.file_name);

    service.move_document_to_trash(&first.id).unwrap();
    service.move_document_to_trash(&second.id).unwrap();
    let result = service.empty_trash().unwrap();

    assert_eq!(result.deleted_count, 2);
    assert!(service.list_trash_documents().unwrap().is_empty());
    assert!(!first_copy.exists());
    assert!(!second_copy.exists());
    assert!(first_source.is_file());
    assert!(second_source.is_file());
}

fn search(
    service: &LibraryService,
    query: &str,
    filters: DocumentSearchFilters,
) -> Vec<personal_document_manager_lib::library::DocumentSearchResult> {
    let DocumentSearchResponse { results } = service
        .search_documents(DocumentSearchQuery {
            query: query.to_string(),
            filters,
        })
        .unwrap();
    results
}

fn png_prefix() -> &'static [u8] {
    &[
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D',
        b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89,
    ]
}

fn stored_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut archive = Vec::new();
    let mut central_entries = Vec::new();

    for (name, contents) in entries {
        let local_offset = archive.len() as u32;
        push_u32(&mut archive, 0x0403_4b50);
        push_u16(&mut archive, 20);
        push_u16(&mut archive, 0);
        push_u16(&mut archive, 0);
        push_u16(&mut archive, 0);
        push_u16(&mut archive, 0);
        push_u32(&mut archive, 0);
        push_u32(&mut archive, contents.len() as u32);
        push_u32(&mut archive, contents.len() as u32);
        push_u16(&mut archive, name.len() as u16);
        push_u16(&mut archive, 0);
        archive.extend_from_slice(name.as_bytes());
        archive.extend_from_slice(contents);

        let mut central = Vec::new();
        push_u32(&mut central, 0x0201_4b50);
        push_u16(&mut central, 20);
        push_u16(&mut central, 20);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u32(&mut central, 0);
        push_u32(&mut central, contents.len() as u32);
        push_u32(&mut central, contents.len() as u32);
        push_u16(&mut central, name.len() as u16);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u32(&mut central, 0);
        push_u32(&mut central, local_offset);
        central.extend_from_slice(name.as_bytes());
        central_entries.push(central);
    }

    let central_offset = archive.len() as u32;
    for entry in &central_entries {
        archive.extend_from_slice(entry);
    }
    let central_size = archive.len() as u32 - central_offset;

    push_u32(&mut archive, 0x0605_4b50);
    push_u16(&mut archive, 0);
    push_u16(&mut archive, 0);
    push_u16(&mut archive, entries.len() as u16);
    push_u16(&mut archive, entries.len() as u16);
    push_u32(&mut archive, central_size);
    push_u32(&mut archive, central_offset);
    push_u16(&mut archive, 0);
    archive
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn is_library(path: &Path) -> bool {
    path.join(".pdm").join("library.json").is_file()
        && path.join(".pdm").join("library.sqlite3").is_file()
}
