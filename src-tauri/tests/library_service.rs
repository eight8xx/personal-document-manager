use std::fs;
use std::path::Path;

use personal_document_manager_lib::library::{
    DocumentProcessingStatus, ImportDecision, ImportItemStatus, IndexStatus, LibraryService,
    LocationStatus,
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

fn is_library(path: &Path) -> bool {
    path.join(".pdm").join("library.json").is_file()
        && path.join(".pdm").join("library.sqlite3").is_file()
}
