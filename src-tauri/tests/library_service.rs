use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use personal_document_manager_lib::library::{
    BatchDocumentItemStatus, BatchDocumentOperation, BatchDocumentOperationRequest,
    DocumentIndexPhase, DocumentMetadataUpdate, DocumentPreview, DocumentProcessingStatus,
    DocumentSearchFilters, DocumentSearchQuery, DocumentSearchResponse, DocumentThumbnail,
    EmptyTrashItemStatus, ExternalChangeMonitor, ImportDecision, ImportItemStatus, ImportSource,
    IndexStatus, LibraryService, LocationStatus, SearchMatchKind,
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
fn validates_pptx_packages_and_rejects_encrypted_or_legacy_presentations() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let valid_path = root.path().join("slides.pptx");
    let invalid_path = root.path().join("not-a-presentation.pptx");
    let encrypted_path = root.path().join("encrypted.pptx");
    let legacy_path = root.path().join("legacy.ppt");
    let macro_path = root.path().join("macro.pptm");
    fs::write(&valid_path, pptx_fixture()).unwrap();
    fs::write(
        &invalid_path,
        stored_zip(&[(
            "[Content_Types].xml",
            br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/></Types>"#,
        )]),
    )
    .unwrap();
    fs::write(
        &encrypted_path,
        [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0x00],
    )
    .unwrap();
    fs::write(&legacy_path, b"legacy presentation").unwrap();
    fs::write(&macro_path, b"macro presentation").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();

    let imported = service.import_document(&valid_path).unwrap();
    assert_eq!(imported.file_type, "PPTX");

    let failure = service.import_document(&invalid_path).unwrap_err();
    assert_eq!(failure.code(), "importFile");
    assert!(failure.to_string().contains("presentation"));

    let encrypted = service.import_document(&encrypted_path).unwrap_err();
    assert_eq!(encrypted.code(), "importFile");
    assert!(encrypted.to_string().contains("加密"));

    let batch = service
        .start_import(vec![
            legacy_path.to_string_lossy().into_owned(),
            macro_path.to_string_lossy().into_owned(),
        ])
        .unwrap();
    assert_eq!(batch.failed_count, 2);
    assert!(batch.items.iter().all(|item| {
        item.status == ImportItemStatus::Failed
            && item.file_type.is_none()
            && item
                .error_message
                .as_deref()
                .is_some_and(|message| message.contains("仅支持"))
    }));
    assert_eq!(service.list_documents().unwrap().len(), 1);

    let error = service
        .open_external_url("javascript:alert('blocked')")
        .unwrap_err();
    assert_eq!(error.code(), "invalidExternalUrl");
    assert!(error.to_string().contains("HTTP 或 HTTPS"));
}

#[test]
fn imports_every_supported_file_type() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();

    let supported: [(&str, &str, &[u8]); 7] = [
        ("report.pdf", "PDF", b"%PDF-1.4\nannual report"),
        ("report.docx", "DOCX", b"PK\x03\x04meeting notes"),
        ("notes.txt", "TXT", b"plain text"),
        ("readme.md", "Markdown", b"project readme"),
        ("scan.jpg", "JPG", b"\xff\xd8\xffscanned page"),
        ("photo.png", "PNG", b"\x89PNG\r\n\x1a\nproject photo"),
        ("slides.pptx", "PPTX", &[]),
    ];

    for (file_name, expected_type, contents) in supported {
        let source_path = root.path().join(file_name);
        if file_name == "slides.pptx" {
            fs::write(&source_path, pptx_fixture()).unwrap();
        } else {
            fs::write(&source_path, contents).unwrap();
        }
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
fn pptx_extracts_slide_table_group_and_chart_text_without_notes_or_comments() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("quarterly.pptx");
    let slide_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<p:sld
  xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
  xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
  xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <p:cSld>
    <p:spTree>
      <p:sp><p:txBody><a:p><a:r><a:t>幻灯片正文</a:t></a:r></a:p></p:txBody></p:sp>
      <p:graphicFrame><a:graphic><a:graphicData><a:tbl>
        <a:tr><a:tc><a:txBody><a:p><a:r><a:t>表格文本</a:t></a:r></a:p></a:txBody></a:tc></a:tr>
      </a:tbl></a:graphicData></a:graphic></p:graphicFrame>
      <p:graphicFrame r:id="rIdChart"/>
      <p:grpSp><p:sp><p:txBody><a:p><a:r><a:t>组合形状文本</a:t></a:r></a:p></p:txBody></p:sp></p:grpSp>
    </p:spTree>
  </p:cSld>
</p:sld>"#;
    let chart_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart">
  <c:chart><c:plotArea><c:barChart><c:ser><c:tx><c:strRef><c:strCache>
    <c:pt><c:v>季度图表标签</c:v></c:pt>
  </c:strCache></c:strRef></c:tx></c:ser></c:barChart></c:plotArea></c:chart>
</c:chartSpace>"#;
    let notes_xml = r#"<p:notes xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>备注中不应索引的文本</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:notes>"#;
    let comments_xml = r#"<p:cmLst xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cm><p:text>批注中不应索引的文本</p:text></p:cm></p:cmLst>"#;
    let slide_relationships = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rIdChart" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="../charts/chart1.xml"/>
</Relationships>"#;
    let orphan_slide = r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>孤儿幻灯片</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;
    let orphan_chart = r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart><c:plotArea><c:barChart><c:ser><c:tx><c:strRef><c:strCache><c:pt><c:v>孤儿图表</c:v></c:pt></c:strCache></c:strRef></c:tx></c:ser></c:barChart></c:plotArea></c:chart></c:chartSpace>"#;
    fs::write(
        &source_path,
        pptx_fixture_with_entries(
            slide_xml,
            &[
                ("ppt/charts/chart1.xml", chart_xml.as_bytes()),
                (
                    "ppt/slides/_rels/slide1.xml.rels",
                    slide_relationships.as_bytes(),
                ),
                ("ppt/slides/slide2.xml", orphan_slide.as_bytes()),
                ("ppt/charts/chart2.xml", orphan_chart.as_bytes()),
                ("ppt/notesSlides/notesSlide1.xml", notes_xml.as_bytes()),
                ("ppt/comments/comment1.xml", comments_xml.as_bytes()),
            ],
        ),
    )
    .unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let imported = service.import_document(&source_path).unwrap();

    let indexed = service.index_pending_documents().unwrap();
    assert_eq!(indexed.processed, 1);
    assert_eq!(indexed.searchable, 1);
    for expected in ["幻灯片正文", "表格文本", "组合形状文本", "季度图表标签"]
    {
        let results = search(&service, expected, DocumentSearchFilters::default());
        assert_eq!(results.len(), 1, "missing indexed PPTX text: {expected}");
        assert_eq!(results[0].document.id, imported.id);
        assert_eq!(results[0].match_kind, SearchMatchKind::Content);
    }
    assert!(search(
        &service,
        "备注中不应索引的文本",
        DocumentSearchFilters::default()
    )
    .is_empty());
    assert!(search(
        &service,
        "批注中不应索引的文本",
        DocumentSearchFilters::default()
    )
    .is_empty());
    assert!(search(&service, "孤儿幻灯片", DocumentSearchFilters::default()).is_empty());
    assert!(search(&service, "孤儿图表", DocumentSearchFilters::default()).is_empty());
}

#[test]
fn pptx_validation_rejects_a_slide_with_unclosed_relationship_graph() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("damaged-slide.pptx");
    fs::write(
        &source_path,
        pptx_fixture_with_slide(
            r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld></p:not-slide></p:sld>"#,
        ),
    )
    .unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let error = service.import_document(&source_path).unwrap_err();
    assert_eq!(error.code(), "importFile");
    assert!(error.to_string().contains("无法解析 PPTX 关系引用"));
    assert!(service.list_documents().unwrap().is_empty());
}

#[test]
fn pptx_thumbnail_accepts_only_real_images_and_invalidates_on_hash_change() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("slides.pptx");
    fs::write(&source_path, pptx_fixture()).unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let imported = service.import_document(&source_path).unwrap();
    let imported_hash = imported.content_hash.clone().unwrap();
    assert!(matches!(
        service.get_document_thumbnail(&imported.id).unwrap(),
        DocumentThumbnail::Fallback { .. }
    ));

    let invalid = service
        .save_document_thumbnail(
            &imported.id,
            &imported_hash,
            &format!(
                "data:image/png;base64,{}",
                BASE64.encode(png_fixture(32, 32))
            ),
        )
        .unwrap_err();
    assert_eq!(invalid.code(), "preview");
    assert!(invalid.to_string().contains("类型图标"));

    let thumbnail_data_url = format!(
        "data:image/png;base64,{}",
        BASE64.encode(png_fixture(320, 180))
    );
    let DocumentThumbnail::Pptx { data_url } = service
        .save_document_thumbnail(&imported.id, &imported_hash, &thumbnail_data_url)
        .unwrap()
    else {
        panic!("valid PPTX thumbnail should be cached as an image");
    };
    assert_eq!(data_url, thumbnail_data_url);
    assert_eq!(
        png_dimensions(&png_bytes_from_data_url(&data_url)),
        (320, 180)
    );
    let cache_file = only_thumbnail_for(&library_dir.join("thumbnails"), &imported.id);
    assert!(cache_file.is_file());

    let copy = library_dir
        .join("documents")
        .join(&imported.id)
        .join(&imported.file_name);
    fs::write(
        &copy,
        pptx_fixture_with_slide(
            r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>替换幻灯片</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#,
        ),
    )
    .unwrap();
    drop(service);

    let mut restarted = LibraryService::new(&state_dir).unwrap();
    restarted.bootstrap().unwrap();
    restarted.index_pending_documents().unwrap();
    let refreshed = restarted
        .list_documents()
        .unwrap()
        .into_iter()
        .find(|document| document.id == imported.id)
        .unwrap();
    assert_ne!(refreshed.content_hash, imported.content_hash);
    assert!(!cache_file.exists());
    assert!(matches!(
        restarted.get_document_thumbnail(&imported.id).unwrap(),
        DocumentThumbnail::Fallback { .. }
    ));

    let stale = restarted
        .save_document_thumbnail(&imported.id, &imported_hash, &thumbnail_data_url)
        .unwrap_err();
    assert_eq!(stale.code(), "staleThumbnail");
    assert!(stale.to_string().contains("内容版本已变化"));
    assert!(fs::read_dir(library_dir.join("thumbnails"))
        .map(|entries| entries.flatten().all(|entry| {
            !entry
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{}-", imported.id))
        }))
        .unwrap_or(true));
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
fn target_collection_imports_keep_partial_success_and_fall_back_when_deleted() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("target.txt");
    let missing_path = root.path().join("missing.txt");
    fs::write(&source_path, "target collection contents").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let target = service
        .create_collection("目标集合".to_string(), None)
        .unwrap();

    let batch = service
        .start_import_to_collection(
            vec![
                source_path.to_string_lossy().into_owned(),
                missing_path.to_string_lossy().into_owned(),
            ],
            Some(target.id.clone()),
            ImportSource::CollectionDrop,
        )
        .unwrap();
    assert_eq!(
        batch.target_collection_id.as_deref(),
        Some(target.id.as_str())
    );
    assert_eq!(batch.imported_count, 1);
    assert_eq!(batch.failed_count, 1);

    let imported = batch
        .items
        .iter()
        .find(|item| item.status == ImportItemStatus::Imported)
        .unwrap();
    assert_eq!(
        imported.target_collection_id.as_deref(),
        Some(target.id.as_str())
    );
    assert_eq!(imported.collection_id.as_deref(), Some(target.id.as_str()));
    assert!(imported.notice.is_none());
    let imported_document_id = imported.document_id.clone().unwrap();
    assert_eq!(
        service
            .list_documents()
            .unwrap()
            .into_iter()
            .find(|document| document.id == imported_document_id)
            .unwrap()
            .collection_id,
        target.id
    );

    let failed = batch
        .items
        .iter()
        .find(|item| item.status == ImportItemStatus::Failed)
        .unwrap();
    assert_eq!(
        failed.target_collection_id.as_deref(),
        Some(target.id.as_str())
    );

    let pending = service
        .start_import_to_collection(
            vec![source_path.to_string_lossy().into_owned()],
            Some(target.id.clone()),
            ImportSource::CollectionDrop,
        )
        .unwrap()
        .items
        .remove(0);
    assert_eq!(pending.status, ImportItemStatus::Duplicate);
    assert_eq!(
        pending.target_collection_id.as_deref(),
        Some(target.id.as_str())
    );

    service.delete_collection(&target.id).unwrap();
    let fallback = service
        .resolve_import_item(&pending.item_id, ImportDecision::ImportAnyway)
        .unwrap();
    assert_eq!(fallback.status, ImportItemStatus::Imported);
    assert_eq!(
        fallback.target_collection_id.as_deref(),
        Some(target.id.as_str())
    );
    assert_eq!(fallback.collection_id.as_deref(), Some("inbox"));
    assert!(fallback
        .notice
        .as_deref()
        .unwrap()
        .contains("目标集合已删除"));
    assert_eq!(
        service
            .list_documents()
            .unwrap()
            .into_iter()
            .find(|document| { document.id == fallback.document_id.as_deref().unwrap() })
            .unwrap()
            .collection_id,
        "inbox"
    );
}

#[test]
fn collection_drop_ignores_unsupported_files_without_creating_documents() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_dir = root.path().join("source");
    let direct_unsupported = root.path().join("direct.exe");
    let folder_unsupported = source_dir.join("unsupported.bin");
    let supported = source_dir.join("notes.txt");

    fs::create_dir_all(&source_dir).unwrap();
    fs::write(&direct_unsupported, "direct unsupported").unwrap();
    fs::write(&folder_unsupported, "folder unsupported").unwrap();
    fs::write(&supported, "supported contents").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let target = service
        .create_collection("目标集合".to_string(), None)
        .unwrap();

    let batch = service
        .start_import_to_collection(
            vec![
                direct_unsupported.to_string_lossy().into_owned(),
                source_dir.to_string_lossy().into_owned(),
            ],
            Some(target.id.clone()),
            ImportSource::CollectionDrop,
        )
        .unwrap();

    assert_eq!(batch.imported_count, 1);
    assert_eq!(batch.ignored_count, 2);
    assert_eq!(batch.failed_count, 0);
    assert_eq!(batch.items.len(), 3);

    let ignored = batch
        .items
        .iter()
        .filter(|item| item.status == ImportItemStatus::Ignored)
        .collect::<Vec<_>>();
    assert_eq!(ignored.len(), 2);
    assert!(ignored.iter().any(|item| item.file_name == "direct.exe"));
    assert!(ignored
        .iter()
        .any(|item| item.file_name == "unsupported.bin"));
    assert!(ignored.iter().all(|item| {
        item.document_id.is_none()
            && !item.retryable
            && item.target_collection_id.as_deref() == Some(target.id.as_str())
    }));

    let documents = service.list_documents().unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].file_name, "notes.txt");
    assert_eq!(documents[0].collection_id, target.id);
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
fn batch_organizes_every_document_and_refreshes_derived_counts() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let collection = service.create_collection("归档".to_string(), None).unwrap();
    let tag = service.create_tag("重要".to_string()).unwrap();

    let mut document_ids = Vec::new();
    for index in 1..=3 {
        let source_path = root.path().join(format!("document-{index}.txt"));
        fs::write(&source_path, format!("document {index}")).unwrap();
        document_ids.push(service.import_document(source_path).unwrap().id);
    }

    let moved = service
        .batch_organize_documents(
            BatchDocumentOperationRequest {
                job_id: "move-all".to_string(),
                document_ids: document_ids.clone(),
                operation: BatchDocumentOperation::MoveToCollection {
                    collection_id: collection.id.clone(),
                },
            },
            || false,
        )
        .unwrap();
    assert_eq!(moved.succeeded_count, 3);
    assert_eq!(moved.failed_count, 0);
    assert_eq!(moved.cancelled_count, 0);
    assert!(moved
        .results
        .iter()
        .all(|result| result.status == BatchDocumentItemStatus::Succeeded));
    assert_eq!(
        service
            .list_documents()
            .unwrap()
            .iter()
            .filter(|document| document.collection_id == collection.id)
            .count(),
        3
    );
    assert_eq!(
        service
            .list_collections()
            .unwrap()
            .into_iter()
            .find(|item| item.id == collection.id)
            .unwrap()
            .document_count,
        3
    );

    let tagged = service
        .batch_organize_documents(
            BatchDocumentOperationRequest {
                job_id: "tag-all".to_string(),
                document_ids: document_ids.clone(),
                operation: BatchDocumentOperation::AddTag {
                    tag_id: tag.id.clone(),
                },
            },
            || false,
        )
        .unwrap();
    assert_eq!(tagged.succeeded_count, 3);
    assert_eq!(service.list_tags().unwrap()[0].document_count, 3);

    let untagged = service
        .batch_organize_documents(
            BatchDocumentOperationRequest {
                job_id: "untag-all".to_string(),
                document_ids: document_ids.clone(),
                operation: BatchDocumentOperation::RemoveTag {
                    tag_id: tag.id.clone(),
                },
            },
            || false,
        )
        .unwrap();
    assert_eq!(untagged.succeeded_count, 3);
    assert_eq!(service.list_tags().unwrap()[0].document_count, 0);

    let trashed = service
        .batch_organize_documents(
            BatchDocumentOperationRequest {
                job_id: "trash-all".to_string(),
                document_ids,
                operation: BatchDocumentOperation::MoveToTrash,
            },
            || false,
        )
        .unwrap();
    assert_eq!(trashed.succeeded_count, 3);
    assert!(service.list_documents().unwrap().is_empty());
    assert_eq!(service.list_trash_documents().unwrap().len(), 3);
    assert_eq!(
        service
            .list_collections()
            .unwrap()
            .into_iter()
            .find(|item| item.id == collection.id)
            .unwrap()
            .document_count,
        0
    );
}

#[test]
fn batch_organize_keeps_successes_when_one_document_fails() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let collection = service.create_collection("项目".to_string(), None).unwrap();

    let mut document_ids = Vec::new();
    for index in 1..=2 {
        let source_path = root.path().join(format!("document-{index}.txt"));
        fs::write(&source_path, format!("document {index}")).unwrap();
        document_ids.push(service.import_document(source_path).unwrap().id);
    }
    document_ids.insert(1, "missing-document".to_string());

    let result = service
        .batch_organize_documents(
            BatchDocumentOperationRequest {
                job_id: "partial".to_string(),
                document_ids: document_ids.clone(),
                operation: BatchDocumentOperation::MoveToCollection {
                    collection_id: collection.id.clone(),
                },
            },
            || false,
        )
        .unwrap();

    assert_eq!(result.succeeded_count, 2);
    assert_eq!(result.failed_count, 1);
    assert_eq!(result.cancelled_count, 0);
    assert_eq!(result.results[1].document_id, "missing-document");
    assert_eq!(result.results[1].status, BatchDocumentItemStatus::Failed);
    assert_eq!(
        result.results[1].error_code.as_deref(),
        Some("documentNotFound")
    );
    assert!(result.results[1]
        .error_message
        .as_deref()
        .unwrap_or_default()
        .contains("missing-document"));
    assert_eq!(
        service
            .list_documents()
            .unwrap()
            .iter()
            .filter(|document| document.collection_id == collection.id)
            .count(),
        2
    );
}

#[test]
fn batch_organize_cancels_remaining_items_and_preserves_completed_work() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let collection = service.create_collection("归档".to_string(), None).unwrap();

    let mut document_ids = Vec::new();
    for index in 1..=3 {
        let source_path = root.path().join(format!("document-{index}.txt"));
        fs::write(&source_path, format!("document {index}")).unwrap();
        document_ids.push(service.import_document(source_path).unwrap().id);
    }

    let mut cancellation_checks = 0;
    let result = service
        .batch_organize_documents(
            BatchDocumentOperationRequest {
                job_id: "cancel-after-first".to_string(),
                document_ids: document_ids.clone(),
                operation: BatchDocumentOperation::MoveToCollection {
                    collection_id: collection.id.clone(),
                },
            },
            || {
                cancellation_checks += 1;
                cancellation_checks > 1
            },
        )
        .unwrap();

    assert_eq!(result.succeeded_count, 1);
    assert_eq!(result.cancelled_count, 2);
    assert_eq!(result.failed_count, 0);
    assert_eq!(result.results[0].status, BatchDocumentItemStatus::Succeeded);
    assert_eq!(result.results[1].status, BatchDocumentItemStatus::Cancelled);
    assert_eq!(result.results[2].status, BatchDocumentItemStatus::Cancelled);
    assert_eq!(
        service
            .list_documents()
            .unwrap()
            .into_iter()
            .filter(|document| document.collection_id == collection.id)
            .count(),
        1
    );
}

#[test]
fn batch_organize_failed_items_can_be_retried_after_their_blocker_is_removed() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let tag = service.create_tag("重要".to_string()).unwrap();

    let first_source = root.path().join("first.txt");
    let second_source = root.path().join("second.txt");
    fs::write(&first_source, "first").unwrap();
    fs::write(&second_source, "second").unwrap();
    let first = service.import_document(first_source).unwrap();
    let second = service.import_document(second_source).unwrap();
    service.move_document_to_trash(&second.id).unwrap();

    let initial = service
        .batch_organize_documents(
            BatchDocumentOperationRequest {
                job_id: "initial-retry".to_string(),
                document_ids: vec![first.id.clone(), second.id.clone()],
                operation: BatchDocumentOperation::AddTag {
                    tag_id: tag.id.clone(),
                },
            },
            || false,
        )
        .unwrap();
    assert_eq!(initial.succeeded_count, 1);
    assert_eq!(initial.failed_count, 1);
    let failed_id = initial.results[1].document_id.clone();

    service.restore_document(&failed_id).unwrap();
    let retried = service
        .batch_organize_documents(
            BatchDocumentOperationRequest {
                job_id: "retry-failed".to_string(),
                document_ids: vec![failed_id.clone()],
                operation: BatchDocumentOperation::AddTag {
                    tag_id: tag.id.clone(),
                },
            },
            || false,
        )
        .unwrap();

    assert_eq!(retried.succeeded_count, 1);
    assert_eq!(retried.failed_count, 0);
    assert_eq!(service.list_tags().unwrap()[0].document_count, 2);
    assert!(service
        .list_documents()
        .unwrap()
        .into_iter()
        .find(|document| document.id == failed_id)
        .unwrap()
        .tags
        .iter()
        .any(|candidate| candidate.id == tag.id));
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
    fs::write(&pdf_path, valid_pdf()).unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let text = service.import_document(&text_path).unwrap();
    let markdown = service.import_document(&markdown_path).unwrap();
    let docx = service.import_document(&docx_path).unwrap();
    let pdf = service.import_document(&pdf_path).unwrap();

    assert_eq!(
        service.get_document_preview(&text.id, None).unwrap(),
        DocumentPreview::Text {
            text: "纯文本预览内容".to_string()
        }
    );
    assert_eq!(
        service.get_document_preview(&markdown.id, None).unwrap(),
        DocumentPreview::Markdown {
            text: "# Markdown\n\n只读文本内容".to_string()
        }
    );
    let DocumentPreview::Docx {
        data_url,
        text,
        notice,
        degraded_features,
    } = service.get_document_preview(&docx.id, None).unwrap()
    else {
        panic!("DOCX should return a local layout preview");
    };
    assert!(data_url.starts_with(
        "data:application/vnd.openxmlformats-officedocument.wordprocessingml.document;base64,"
    ));
    assert_eq!(text, "会议记录\n第二段\n续行");
    assert!(notice.contains("本地只读近似呈现"));
    assert!(degraded_features.is_empty());

    let DocumentPreview::Pdf {
        data_url,
        page_count,
        page,
    } = service.get_document_preview(&pdf.id, None).unwrap()
    else {
        panic!("PDF should return an inline document preview");
    };
    assert!(data_url.starts_with("data:image/png;base64,"));
    assert_eq!(page_count, Some(1));
    assert_eq!(page, 1);
}

#[test]
fn docx_preview_reports_degraded_regions_without_failing_the_document() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let docx_path = root.path().join("complex.docx");
    let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document
  xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
  xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape">
  <w:body>
    <w:p><w:r><w:t>仍可预览的正文</w:t></w:r></w:p>
    <w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r></w:p>
    <w:p><w:r><wps:txbx><w:txbxContent><w:p><w:r><w:t>文本框</w:t></w:r></w:p></w:txbxContent></wps:txbx></w:r></w:p>
  </w:body>
</w:document>"#;
    let relationships = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rIdChart" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="charts/chart1.xml"/>
  <Relationship Id="rIdRemote" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="https://example.com/image.png" TargetMode="External"/>
</Relationships>"#;
    fs::write(
        &docx_path,
        stored_zip(&[
            ("word/document.xml", document_xml.as_bytes()),
            ("word/_rels/document.xml.rels", relationships.as_bytes()),
            (
                "word/charts/chart1.xml",
                br#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"/>"#,
            ),
        ]),
    )
    .unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let document = service.import_document(&docx_path).unwrap();

    let DocumentPreview::Docx {
        text,
        notice,
        degraded_features,
        ..
    } = service.get_document_preview(&document.id, None).unwrap()
    else {
        panic!("complex DOCX should still return a preview");
    };
    assert!(text.contains("仍可预览的正文"));
    assert!(notice.contains("部分复杂内容"));
    for expected in ["形状或文本框", "字段", "图表或 SmartArt", "远程资源"] {
        assert!(
            degraded_features.iter().any(|feature| feature == expected),
            "missing degraded feature: {expected}"
        );
    }
}

#[test]
fn office_preview_packages_are_sanitized_before_they_reach_renderers() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let docx_path = root.path().join("remote-image.docx");
    let pptx_path = root.path().join("remote-image.pptx");
    let docx_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document
  xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
  xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"
  xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
  <w:body><w:p><w:r><w:t>安全正文</w:t></w:r></w:p>
  <w:p><w:r><w:drawing><a:blip r:embed="rIdRemote"/></w:drawing></w:r></w:p></w:body>
</w:document>"#;
    let docx_relationships = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rIdRemote" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="https://evil.example/tracker.png" TargetMode="External"/>
</Relationships>"#;
    let docx_root_relationships = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rIdDocument" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;
    fs::write(
        &docx_path,
        stored_zip(&[
            ("word/document.xml", docx_xml.as_bytes()),
            ("_rels/.rels", docx_root_relationships.as_bytes()),
            (
                "word/_rels/document.xml.rels",
                docx_relationships.as_bytes(),
            ),
        ]),
    )
    .unwrap();

    let pptx_slide = r#"<p:sld
      xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
      xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
      xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
      <p:cSld><p:spTree><p:pic><p:blipFill><a:blip r:embed="rIdRemote"/></p:blipFill></p:pic></p:spTree></p:cSld>
    </p:sld>"#;
    let pptx_slide_relationships = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rIdRemote" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="https://evil.example/tracker.png" TargetMode="External"/>
</Relationships>"#;
    let pptx_root_relationships = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rIdPresentation" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
  <Relationship Id="rIdThumbnail" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail" Target="docProps/thumbnail.jpeg"/>
</Relationships>"#;
    fs::write(
        &pptx_path,
        pptx_fixture_with_entries(
            pptx_slide,
            &[
                ("_rels/.rels", pptx_root_relationships.as_bytes()),
                (
                    "ppt/slides/_rels/slide1.xml.rels",
                    pptx_slide_relationships.as_bytes(),
                ),
                ("docProps/thumbnail.jpeg", b"EMBEDDED-THUMBNAIL"),
            ],
        ),
    )
    .unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let docx = service.import_document(&docx_path).unwrap();
    let pptx = service.import_document(&pptx_path).unwrap();

    let DocumentPreview::Docx { data_url, .. } =
        service.get_document_preview(&docx.id, None).unwrap()
    else {
        panic!("DOCX preview should be available");
    };
    let docx_package = bytes_from_data_url(&data_url);
    assert!(!bytes_contain(
        &docx_package,
        b"https://evil.example/tracker.png"
    ));
    assert!(!bytes_contain(&docx_package, b"TargetMode=\"External\""));
    assert!(!bytes_contain(&docx_package, b"rIdRemote"));

    let DocumentPreview::Pptx { data_url, .. } =
        service.get_document_preview(&pptx.id, None).unwrap()
    else {
        panic!("PPTX preview should be available");
    };
    let pptx_package = bytes_from_data_url(&data_url);
    assert!(!bytes_contain(
        &pptx_package,
        b"https://evil.example/tracker.png"
    ));
    assert!(!bytes_contain(&pptx_package, b"TargetMode=\"External\""));
    assert!(!bytes_contain(&pptx_package, b"rIdRemote"));
    assert!(!bytes_contain(&pptx_package, b"EMBEDDED-THUMBNAIL"));
}

#[test]
fn generates_thumbnails_and_reports_missing_library_copies_without_changing_state() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let image_path = root.path().join("pixel.png");
    let corrupt_image_path = root.path().join("corrupt.png");
    let pdf_path = root.path().join("report.pdf");
    let source_image = png_fixture(800, 600);
    fs::write(&image_path, &source_image).unwrap();
    fs::write(
        &corrupt_image_path,
        [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x01],
    )
    .unwrap();
    fs::write(&pdf_path, valid_pdf()).unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let image = service.import_document(&image_path).unwrap();
    let corrupt_image = service.import_document(&corrupt_image_path).unwrap();
    let pdf = service.import_document(&pdf_path).unwrap();

    let DocumentThumbnail::Image {
        data_url: image_url,
    } = service.get_document_thumbnail(&image.id).unwrap()
    else {
        panic!("image thumbnail should be available");
    };
    assert!(image_url.starts_with("data:image/png;base64,"));
    let image_thumbnail = png_bytes_from_data_url(&image_url);
    assert_eq!(png_dimensions(&image_thumbnail), (320, 240));
    assert!(image_thumbnail.len() < source_image.len());

    let thumbnail_dir = library_dir.join("thumbnails");
    let image_cache_file = only_thumbnail_for(&thumbnail_dir, &image.id);
    let first_modified = fs::metadata(&image_cache_file).unwrap().modified().unwrap();
    let DocumentThumbnail::Image {
        data_url: cached_url,
    } = service.get_document_thumbnail(&image.id).unwrap()
    else {
        panic!("cached image thumbnail should be available");
    };
    assert_eq!(cached_url, image_url);
    assert_eq!(
        fs::metadata(&image_cache_file).unwrap().modified().unwrap(),
        first_modified
    );

    let DocumentThumbnail::Pdf { data_url: pdf_url } =
        service.get_document_thumbnail(&pdf.id).unwrap()
    else {
        panic!("PDF first-page thumbnail should be available");
    };
    assert!(pdf_url.starts_with("data:image/png;base64,"));
    assert!(!pdf_url.starts_with("data:application/pdf"));
    let pdf_thumbnail = png_bytes_from_data_url(&pdf_url);
    let (pdf_width, pdf_height) = png_dimensions(&pdf_thumbnail);
    assert!(pdf_width <= 320 && pdf_height <= 240);
    assert!(pdf_width > 0 && pdf_height > 0);
    assert!(only_thumbnail_for(&thumbnail_dir, &pdf.id).is_file());
    let DocumentThumbnail::Fallback { reason } =
        service.get_document_thumbnail(&corrupt_image.id).unwrap()
    else {
        panic!("corrupt image should fall back to a type icon");
    };
    assert!(reason.contains("无法生成缩略图"));

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
    let DocumentPreview::Failure { code, message } =
        service.get_document_preview(&image.id, None).unwrap()
    else {
        panic!("missing library copy should return a structured preview failure");
    };
    assert_eq!(code, "documentFileMissing");
    assert!(message.contains("资料库副本不存在"));
    let open_error = service.open_document(&image.id).unwrap_err();
    assert_eq!(open_error.code(), "documentFileMissing");
    assert_eq!(service.list_documents().unwrap(), documents_before);
}

#[test]
fn pdf_preview_rejects_active_content_and_invalidates_temporary_page_cache() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("report.pdf");
    fs::write(&source_path, valid_pdf()).unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    let library = service.create_library(&library_dir).unwrap();
    let imported = service.import_document(&source_path).unwrap();
    let first_hash = imported.content_hash.clone().unwrap();
    let DocumentPreview::Pdf {
        data_url,
        page_count,
        page,
    } = service.get_document_preview(&imported.id, None).unwrap()
    else {
        panic!("safe PDF should render a static page image");
    };
    assert!(data_url.starts_with("data:image/png;base64,"));
    assert_eq!(page_count, Some(1));
    assert_eq!(page, 1);

    let first_cache_directory = std::env::temp_dir()
        .join("personal-document-manager")
        .join("rendered-pages")
        .join(&library.id)
        .join(&imported.id)
        .join(&first_hash);
    let first_cache_page = first_cache_directory.join("page-1.png");
    assert!(first_cache_page.is_file());
    let first_modified = fs::metadata(&first_cache_page).unwrap().modified().unwrap();
    assert!(matches!(
        service.get_document_preview(&imported.id, None).unwrap(),
        DocumentPreview::Pdf { .. }
    ));
    assert_eq!(
        fs::metadata(&first_cache_page).unwrap().modified().unwrap(),
        first_modified
    );

    let copy = library_dir
        .join("documents")
        .join(&imported.id)
        .join(&imported.file_name);
    fs::write(&copy, valid_pdf_with_text("Replacement PDF page")).unwrap();
    drop(service);

    let mut restarted = LibraryService::new(&state_dir).unwrap();
    restarted.bootstrap().unwrap();
    restarted.index_pending_documents().unwrap();
    let refreshed = restarted
        .list_documents()
        .unwrap()
        .into_iter()
        .find(|document| document.id == imported.id)
        .unwrap();
    let refreshed_hash = refreshed.content_hash.unwrap();
    assert_ne!(refreshed_hash, first_hash);
    assert!(matches!(
        restarted.get_document_preview(&imported.id, None).unwrap(),
        DocumentPreview::Pdf { .. }
    ));
    assert!(!first_cache_directory.exists());
    assert!(std::env::temp_dir()
        .join("personal-document-manager")
        .join("rendered-pages")
        .join(&library.id)
        .join(&imported.id)
        .join(&refreshed_hash)
        .join("page-1.png")
        .is_file());

    let unsafe_path = root.path().join("unsafe.pdf");
    fs::write(&unsafe_path, pdf_with_open_action("JavaScript")).unwrap();
    let unsafe_document = restarted.import_document(&unsafe_path).unwrap();
    let DocumentPreview::Failure { code, message } = restarted
        .get_document_preview(&unsafe_document.id, None)
        .unwrap()
    else {
        panic!("active PDF content must not be rendered");
    };
    assert_eq!(code, "unsafePreview");
    assert!(message.contains("禁止"));
}

#[test]
fn pdf_preview_rejects_active_objects_inside_a_compressed_object_stream() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("compressed-active.pdf");
    fs::write(&source_path, compressed_pdf_with_nested_javascript()).unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let imported = service.import_document(&source_path).unwrap();
    let DocumentPreview::Failure { code, message } =
        service.get_document_preview(&imported.id, None).unwrap()
    else {
        panic!("compressed active PDF content must not be rendered");
    };
    assert_eq!(code, "unsafePreview");
    assert!(message.to_ascii_lowercase().contains("javascript"));
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

    let short_results = search(&service, "计划", DocumentSearchFilters::default());
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
fn monitor_coalesces_consecutive_saves_and_refreshes_content() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("mutable.txt");
    let source_bytes = "source stays unchanged".as_bytes();
    fs::write(&source_path, source_bytes).unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let imported = service.import_document(&source_path).unwrap();
    service.index_pending_documents().unwrap();
    let copy = library_dir
        .join("documents")
        .join(&imported.id)
        .join(&imported.file_name);
    let original_hash = imported.content_hash.clone().unwrap();

    let service = Arc::new(Mutex::new(service));
    let events = Arc::new(Mutex::new(Vec::new()));
    let callback_events = Arc::clone(&events);
    let monitor = ExternalChangeMonitor::start(
        Arc::clone(&service),
        Duration::from_millis(20),
        Duration::from_millis(120),
        move |event| callback_events.lock().unwrap().push(event),
    )
    .unwrap();

    fs::write(&copy, "first external save").unwrap();
    thread::sleep(Duration::from_millis(35));
    fs::write(&copy, "second external save is searchable").unwrap();

    assert!(
        wait_until(Duration::from_secs(3), || {
            let service = service.lock().unwrap();
            let document = service
                .list_documents()
                .unwrap()
                .into_iter()
                .find(|document| document.id == imported.id)
                .unwrap();
            document.index_status == IndexStatus::Searchable
                && document.content_hash.as_deref() != Some(original_hash.as_str())
                && !search(
                    &service,
                    "second external save is searchable",
                    DocumentSearchFilters::default(),
                )
                .is_empty()
        }),
        "monitor did not finish reindexing the externally modified document"
    );

    drop(monitor);

    let service = service.lock().unwrap();
    assert_eq!(fs::read(&source_path).unwrap(), source_bytes);
    assert!(search(
        &service,
        "source stays unchanged",
        DocumentSearchFilters::default()
    )
    .is_empty());
    let completed = events
        .lock()
        .unwrap()
        .iter()
        .filter(|event| event.phase == DocumentIndexPhase::Completed)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].result.as_ref().unwrap().processed, 1);
}

#[test]
fn dropping_monitor_releases_its_worker_promptly() {
    let root = tempdir().unwrap();
    let service = LibraryService::new(root.path().join("app-state")).unwrap();
    let service = Arc::new(Mutex::new(service));
    let monitor = ExternalChangeMonitor::start(
        service,
        Duration::from_secs(5),
        Duration::from_millis(50),
        |_| {},
    )
    .unwrap();

    let started = Instant::now();
    drop(monitor);

    assert!(
        started.elapsed() < Duration::from_secs(1),
        "monitor drop should wake and join the worker promptly"
    );
}

#[test]
fn startup_scan_finds_changes_made_while_the_application_was_closed() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("startup.txt");
    let source_bytes = "旧正文只应在第一次索引中出现".as_bytes();
    fs::write(&source_path, source_bytes).unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let imported = service.import_document(&source_path).unwrap();
    service.index_pending_documents().unwrap();
    let copy = library_dir
        .join("documents")
        .join(&imported.id)
        .join(&imported.file_name);
    let old_hash = imported.content_hash.unwrap();
    drop(service);

    let replacement = "关闭期间写入的新正文";
    fs::write(&copy, replacement).unwrap();

    let mut restarted = LibraryService::new(&state_dir).unwrap();
    restarted.bootstrap().unwrap();
    let processing = restarted
        .list_documents()
        .unwrap()
        .into_iter()
        .find(|document| document.id == imported.id)
        .unwrap();
    assert_eq!(
        processing.processing_status,
        DocumentProcessingStatus::Processing
    );
    assert_eq!(processing.index_status, IndexStatus::Pending);
    assert!(search(&restarted, "旧正文", DocumentSearchFilters::default()).is_empty());

    let result = restarted.index_pending_documents().unwrap();
    assert_eq!(result.processed, 1);
    assert_eq!(result.searchable, 1);
    let refreshed = restarted
        .list_documents()
        .unwrap()
        .into_iter()
        .find(|document| document.id == imported.id)
        .unwrap();
    assert_eq!(refreshed.file_size, replacement.len() as i64);
    assert_ne!(refreshed.content_hash.as_deref(), Some(old_hash.as_str()));
    assert_eq!(
        search(&restarted, replacement, DocumentSearchFilters::default()).len(),
        1
    );
    assert_eq!(fs::read(&source_path).unwrap(), source_bytes);
}

#[test]
fn startup_hash_scan_detects_same_size_timestamp_preserving_rewrite() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("rewrite.txt");
    let source_bytes = "source stays unchanged";
    let replacement = "source remains changed";
    assert_eq!(source_bytes.len(), replacement.len());
    fs::write(&source_path, source_bytes).unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let imported = service.import_document(&source_path).unwrap();
    service.index_pending_documents().unwrap();
    let copy = library_dir
        .join("documents")
        .join(&imported.id)
        .join(&imported.file_name);
    let original_modified = fs::metadata(&copy).unwrap().modified().unwrap();
    drop(service);

    fs::write(&copy, replacement).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&copy)
        .unwrap()
        .set_modified(original_modified)
        .unwrap();
    let metadata = fs::metadata(&copy).unwrap();
    assert_eq!(metadata.len(), replacement.len() as u64);
    assert_eq!(metadata.modified().unwrap(), original_modified);

    let mut restarted = LibraryService::new(&state_dir).unwrap();
    restarted.bootstrap().unwrap();
    let processing = restarted
        .list_documents()
        .unwrap()
        .into_iter()
        .find(|document| document.id == imported.id)
        .unwrap();
    assert_eq!(
        processing.processing_status,
        DocumentProcessingStatus::Processing
    );
    assert_eq!(processing.index_status, IndexStatus::Pending);

    let result = restarted.index_pending_documents().unwrap();
    assert_eq!(result.processed, 1);
    assert_eq!(result.searchable, 1);
    assert!(search(
        &restarted,
        "stays unchanged",
        DocumentSearchFilters::default()
    )
    .is_empty());
    assert_eq!(
        search(&restarted, replacement, DocumentSearchFilters::default()).len(),
        1
    );
    assert_eq!(fs::read(&source_path).unwrap(), source_bytes.as_bytes());
}

#[test]
fn deleted_library_copy_stays_visible_fails_explicitly_and_can_be_retried() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("recover.txt");
    let source_bytes = "删除前的可搜索正文".as_bytes();
    fs::write(&source_path, source_bytes).unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let imported = service.import_document(&source_path).unwrap();
    service.index_pending_documents().unwrap();
    let copy = library_dir
        .join("documents")
        .join(&imported.id)
        .join(&imported.file_name);
    drop(service);

    fs::remove_file(&copy).unwrap();
    let mut restarted = LibraryService::new(&state_dir).unwrap();
    restarted.bootstrap().unwrap();
    let result = restarted.index_pending_documents().unwrap();
    assert_eq!(result.failed, 1);
    let failed = restarted
        .list_documents()
        .unwrap()
        .into_iter()
        .find(|document| document.id == imported.id)
        .unwrap();
    assert_eq!(failed.processing_status, DocumentProcessingStatus::Failed);
    assert_eq!(failed.index_status, IndexStatus::Failed);
    assert!(failed
        .error_message
        .as_deref()
        .unwrap_or_default()
        .contains("不存在"));
    assert!(search(
        &restarted,
        "删除前的可搜索正文",
        DocumentSearchFilters::default()
    )
    .is_empty());

    let recovered_text = "重新放置后的恢复正文";
    fs::write(&copy, recovered_text).unwrap();
    let recovered = restarted.retry_document_index(&imported.id).unwrap();
    assert_eq!(recovered.processing_status, DocumentProcessingStatus::Ready);
    assert_eq!(recovered.index_status, IndexStatus::Searchable);
    assert_eq!(
        search(&restarted, recovered_text, DocumentSearchFilters::default()).len(),
        1
    );
    assert_eq!(fs::read(&source_path).unwrap(), source_bytes);
}

#[test]
fn corrupt_replacement_keeps_the_document_and_marks_processing_failed() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("report.pdf");
    fs::write(&source_path, b"%PDF-1.4\noriginal").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let imported = service.import_document(&source_path).unwrap();
    service.index_pending_documents().unwrap();
    let copy = library_dir
        .join("documents")
        .join(&imported.id)
        .join(&imported.file_name);
    drop(service);

    fs::write(&copy, b"this is no longer a PDF").unwrap();
    let mut restarted = LibraryService::new(&state_dir).unwrap();
    restarted.bootstrap().unwrap();
    let result = restarted.index_pending_documents().unwrap();
    assert_eq!(result.failed, 1);

    let failed = restarted
        .list_documents()
        .unwrap()
        .into_iter()
        .find(|document| document.id == imported.id)
        .unwrap();
    assert_eq!(failed.processing_status, DocumentProcessingStatus::Failed);
    assert_eq!(failed.index_status, IndexStatus::Failed);
    assert!(failed
        .error_message
        .as_deref()
        .unwrap_or_default()
        .contains("不是有效"));
}

#[test]
fn unsupported_external_replacement_keeps_the_document_and_fails_explicitly() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("replace-me.txt");
    fs::write(&source_path, "original searchable text").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let imported = service.import_document(&source_path).unwrap();
    service.index_pending_documents().unwrap();
    let copy = library_dir
        .join("documents")
        .join(&imported.id)
        .join(&imported.file_name);
    drop(service);

    fs::remove_file(&copy).unwrap();
    fs::write(copy.with_file_name("replacement.exe"), b"not supported").unwrap();

    let mut restarted = LibraryService::new(&state_dir).unwrap();
    restarted.bootstrap().unwrap();
    let result = restarted.index_pending_documents().unwrap();
    assert_eq!(result.failed, 1);

    let failed = restarted
        .list_documents()
        .unwrap()
        .into_iter()
        .find(|document| document.id == imported.id)
        .unwrap();
    assert_eq!(failed.processing_status, DocumentProcessingStatus::Failed);
    assert_eq!(failed.index_status, IndexStatus::Failed);
    assert!(failed
        .error_message
        .as_deref()
        .unwrap_or_default()
        .contains("不支持"));
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
fn database_failure_during_permanent_delete_restores_the_library_copy() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("rollback.txt");
    fs::write(&source_path, "rollback contents").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let imported = service.import_document(&source_path).unwrap();
    let library_copy = library_dir
        .join("documents")
        .join(&imported.id)
        .join(&imported.file_name);
    service.move_document_to_trash(&imported.id).unwrap();

    let connection = Connection::open(library_dir.join(".pdm").join("library.sqlite3")).unwrap();
    connection
        .execute_batch(&format!(
            "
            CREATE TRIGGER fail_document_delete
            BEFORE DELETE ON documents
            WHEN OLD.id = '{}'
            BEGIN
                SELECT RAISE(ABORT, 'forced delete failure');
            END;
            ",
            imported.id
        ))
        .unwrap();
    drop(connection);

    let error = service
        .permanently_delete_document(&imported.id)
        .unwrap_err();
    assert_eq!(error.code(), "database");
    assert!(library_copy.is_file());
    assert_eq!(
        service.list_trash_documents().unwrap()[0].document.id,
        imported.id
    );
    assert!(fs::read_dir(library_copy.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .all(|entry| !entry.file_name().to_string_lossy().contains(".tombstone")));
}

#[test]
fn opening_library_restores_or_removes_crash_left_tombstones() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("tombstone.txt");
    fs::write(&source_path, "tombstone contents").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let restored_document = service.import_document(&source_path).unwrap();
    service.index_pending_documents().unwrap();
    let restored_copy = library_dir
        .join("documents")
        .join(&restored_document.id)
        .join(&restored_document.file_name);
    let restored_tombstone = restored_copy
        .parent()
        .unwrap()
        .join(".pdm-delete-crash-restore.tombstone");
    fs::rename(&restored_copy, &restored_tombstone).unwrap();
    drop(service);

    let mut restarted = LibraryService::new(&state_dir).unwrap();
    restarted.bootstrap().unwrap();
    assert!(restored_copy.is_file());
    assert!(!restored_tombstone.exists());

    let deleted_document_id = restarted.list_documents().unwrap()[0].id.clone();
    let deleted_file_name = restarted.list_documents().unwrap()[0].file_name.clone();
    let deleted_copy = library_dir
        .join("documents")
        .join(&deleted_document_id)
        .join(&deleted_file_name);
    let deleted_tombstone = deleted_copy
        .parent()
        .unwrap()
        .join(".pdm-delete-crash-cleanup.tombstone");
    fs::rename(&deleted_copy, &deleted_tombstone).unwrap();
    drop(restarted);

    let connection = Connection::open(library_dir.join(".pdm").join("library.sqlite3")).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    connection
        .execute(
            "DELETE FROM document_search WHERE document_id = ?1",
            [&deleted_document_id],
        )
        .unwrap();
    connection
        .execute(
            "DELETE FROM documents WHERE id = ?1",
            [&deleted_document_id],
        )
        .unwrap();
    drop(connection);

    let mut cleaned = LibraryService::new(&state_dir).unwrap();
    cleaned.open_library(&library_dir).unwrap();
    assert!(!deleted_tombstone.exists());
    assert!(!deleted_copy.exists());
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
    assert_eq!(result.failed_count, 0);
    assert_eq!(result.items.len(), 2);
    assert!(service.list_trash_documents().unwrap().is_empty());
    assert!(!first_copy.exists());
    assert!(!second_copy.exists());
    assert!(first_source.is_file());
    assert!(second_source.is_file());
}

#[test]
fn empty_trash_reports_partial_failures_and_keeps_failed_documents_retryable() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let blocked_source = root.path().join("blocked.txt");
    let removable_source = root.path().join("removable.txt");
    fs::write(&blocked_source, "blocked").unwrap();
    fs::write(&removable_source, "removable").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let blocked = service.import_document(&blocked_source).unwrap();
    let removable = service.import_document(&removable_source).unwrap();
    let blocked_copy = library_dir
        .join("documents")
        .join(&blocked.id)
        .join(&blocked.file_name);
    let removable_copy = library_dir
        .join("documents")
        .join(&removable.id)
        .join(&removable.file_name);
    service.move_document_to_trash(&blocked.id).unwrap();
    service.move_document_to_trash(&removable.id).unwrap();

    let connection = Connection::open(library_dir.join(".pdm").join("library.sqlite3")).unwrap();
    connection
        .execute_batch(&format!(
            "
            CREATE TRIGGER fail_specific_document_delete
            BEFORE DELETE ON documents
            WHEN OLD.id = '{}'
            BEGIN
                SELECT RAISE(ABORT, 'forced per-document failure');
            END;
            ",
            blocked.id
        ))
        .unwrap();
    drop(connection);

    let result = service.empty_trash().unwrap();
    assert_eq!(result.deleted_count, 1);
    assert_eq!(result.failed_count, 1);
    assert_eq!(result.items.len(), 2);
    assert!(result.items.iter().any(|item| {
        item.document_id == blocked.id
            && item.status == EmptyTrashItemStatus::Failed
            && item.error_message.is_some()
    }));
    assert!(result.items.iter().any(|item| {
        item.document_id == removable.id && item.status == EmptyTrashItemStatus::Succeeded
    }));
    assert!(blocked_copy.is_file());
    assert!(!removable_copy.exists());
    let remaining = service.list_trash_documents().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].document.id, blocked.id);
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

fn pptx_fixture() -> Vec<u8> {
    pptx_fixture_with_slide(
        r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>演示文稿正文</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#,
    )
}

fn pptx_fixture_with_slide(slide_xml: &str) -> Vec<u8> {
    pptx_fixture_with_entries(slide_xml, &[])
}

fn pptx_fixture_with_entries(slide_xml: &str, extras: &[(&str, &[u8])]) -> Vec<u8> {
    let content_types = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
  <Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>
</Types>"#;
    let presentation = r#"<?xml version="1.0" encoding="UTF-8"?>
<p:presentation
  xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
  xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst>
</p:presentation>"#;
    let relationships = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/>
</Relationships>"#;

    let mut entries: Vec<(&str, &[u8])> = vec![
        ("[Content_Types].xml", content_types.as_bytes()),
        ("ppt/presentation.xml", presentation.as_bytes()),
        ("ppt/_rels/presentation.xml.rels", relationships.as_bytes()),
        ("ppt/slides/slide1.xml", slide_xml.as_bytes()),
    ];
    entries.extend_from_slice(extras);
    stored_zip(&entries)
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

fn png_fixture(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        let mut pixels = vec![0_u8; (width * height * 3) as usize];
        for y in 0..height {
            for x in 0..width {
                let offset = ((y * width + x) * 3) as usize;
                pixels[offset] = (x % 251) as u8;
                pixels[offset + 1] = (y % 241) as u8;
                pixels[offset + 2] = ((x + y) % 239) as u8;
            }
        }
        writer.write_image_data(&pixels).unwrap();
    }
    bytes
}

fn valid_pdf() -> Vec<u8> {
    valid_pdf_with_text("PDF page")
}

fn valid_pdf_with_text(text: &str) -> Vec<u8> {
    let content = format!("BT /F1 24 Tf 36 100 Td ({}) Tj ET", escape_pdf_text(text));
    let content = content.as_bytes();
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        format!(
            "<< /Length {} >>\nstream\n{}\nendstream",
            content.len(),
            String::from_utf8_lossy(content)
        ),
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0_usize];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref_offset = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", offsets.len()).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets.iter().skip(1) {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
            offsets.len()
        )
        .as_bytes(),
    );
    pdf
}

fn pdf_with_open_action(action: &str) -> Vec<u8> {
    let mut document = base_pdf_document();
    let mut action_dictionary = lopdf::Dictionary::new();
    action_dictionary.set("Type", "Action");
    action_dictionary.set("S", action);
    if action == "JavaScript" {
        action_dictionary.set("JS", lopdf::Object::string_literal("app.alert('blocked')"));
    }
    document
        .get_dictionary_mut((1, 0))
        .unwrap()
        .set("OpenAction", lopdf::Object::Dictionary(action_dictionary));
    save_compressed_pdf(document)
}

fn compressed_pdf_with_nested_javascript() -> Vec<u8> {
    let mut document = base_pdf_document();
    let mut nested = lopdf::Dictionary::new();
    nested.set("Name", lopdf::Object::Name(b"JavaScript".to_vec()));
    nested.set("JS", lopdf::Object::string_literal("app.alert('nested')"));
    let mut wrapper = lopdf::Dictionary::new();
    wrapper.set("Nested", lopdf::Object::Dictionary(nested));
    document
        .get_dictionary_mut((1, 0))
        .unwrap()
        .set("CustomData", lopdf::Object::Dictionary(wrapper));
    save_compressed_pdf(document)
}

fn base_pdf_document() -> lopdf::Document {
    let mut document = lopdf::Document::with_version("1.7");
    let mut catalog = lopdf::Dictionary::new();
    catalog.set("Type", "Catalog");
    catalog.set("Pages", lopdf::Object::Reference((2, 0)));

    let mut pages = lopdf::Dictionary::new();
    pages.set("Type", "Pages");
    pages.set("Kids", vec![lopdf::Object::Reference((3, 0))]);
    pages.set("Count", 1);

    let mut page = lopdf::Dictionary::new();
    page.set("Type", "Page");
    page.set("Parent", lopdf::Object::Reference((2, 0)));
    page.set("MediaBox", vec![0.into(), 0.into(), 200.into(), 200.into()]);

    document
        .objects
        .insert((1, 0), lopdf::Object::Dictionary(catalog));
    document
        .objects
        .insert((2, 0), lopdf::Object::Dictionary(pages));
    document
        .objects
        .insert((3, 0), lopdf::Object::Dictionary(page));
    document
        .trailer
        .set("Root", lopdf::Object::Reference((1, 0)));
    document.max_id = 3;
    document
}

fn save_compressed_pdf(mut document: lopdf::Document) -> Vec<u8> {
    let mut bytes = Vec::new();
    document
        .save_with_options(
            &mut bytes,
            lopdf::SaveOptions {
                use_object_streams: true,
                use_xref_streams: true,
                ..Default::default()
            },
        )
        .unwrap();
    bytes
}

fn escape_pdf_text(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

fn png_bytes_from_data_url(data_url: &str) -> Vec<u8> {
    bytes_from_data_url(data_url)
}

fn bytes_from_data_url(data_url: &str) -> Vec<u8> {
    let (_, encoded) = data_url.split_once(',').unwrap();
    BASE64.decode(encoded).unwrap()
}

fn bytes_contain(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn png_dimensions(bytes: &[u8]) -> (u32, u32) {
    assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]));
    (
        u32::from_be_bytes(bytes[16..20].try_into().unwrap()),
        u32::from_be_bytes(bytes[20..24].try_into().unwrap()),
    )
}

fn only_thumbnail_for(thumbnail_dir: &Path, document_id: &str) -> PathBuf {
    let prefix = format!("{document_id}-");
    let matches = fs::read_dir(thumbnail_dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".png"))
        })
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 1);
    matches[0].clone()
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn is_library(path: &Path) -> bool {
    path.join(".pdm").join("library.json").is_file()
        && path.join(".pdm").join("library.sqlite3").is_file()
}
