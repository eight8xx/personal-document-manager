use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use personal_document_manager_lib::library::{
    BatchDocumentOperation, BatchDocumentOperationRequest, DocumentMetadataUpdate, DocumentPreview,
    DocumentProcessingStatus, DocumentSearchFilters, DocumentSearchQuery, DocumentSearchResponse,
    DocumentSummary, IndexStatus, LibraryService, SearchMatchKind,
};
use serde::Serialize;
use tempfile::tempdir;

const DOCUMENT_COUNT: usize = 10_000;
const EXTERNAL_CHANGE_COUNT: usize = 120;
const ORGANIZED_DOCUMENT_COUNT: usize = 50;
const TRASH_DOCUMENT_COUNT: usize = 100;
const RESTORE_DOCUMENT_COUNT: usize = 50;
const CHINESE_MARKER: &str = "中文验收词";
const ENGLISH_MARKER: &str = "acceptance needle";
const EXTERNAL_MARKER: &str = "外部增量验收词";
const SHORT_MARKER: &str = "AX";
const DOCUMENT_DATE: &str = "2026-02-01";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AcceptanceMetrics {
    document_count: usize,
    formats: Vec<&'static str>,
    generation_ms: u64,
    import_ms: u64,
    index_ms: u64,
    restart_ms: u64,
    list_documents_ms: u64,
    chinese_search_ms: u64,
    english_search_ms: u64,
    short_search_ms: u64,
    combined_filter_ms: u64,
    external_change_scan_ms: u64,
    external_change_index_ms: u64,
    trash_transition_ms: u64,
    source_verification_ms: u64,
    progress_events: usize,
    imported_count: i64,
    indexed_count: i64,
    chinese_match_count: usize,
    english_match_count: usize,
    short_match_count: usize,
    combined_match_count: usize,
    external_searchable_count: usize,
    external_failed_count: i64,
    active_document_count: usize,
    restored_count: usize,
    permanently_deleted_count: usize,
    source_file_count: usize,
    source_files_unchanged: bool,
}

struct Fixture {
    path: PathBuf,
    original_bytes: Vec<u8>,
    has_chinese_marker: bool,
    has_english_marker: bool,
    has_extractable_text: bool,
}

#[test]
#[ignore = "10k acceptance harness; run scripts/run-10k-acceptance.ps1"]
fn ten_thousand_document_library_acceptance() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("acceptance-library");
    let source_dir = root.path().join("synthetic-sources");
    fs::create_dir_all(&source_dir).unwrap();

    let generation_start = Instant::now();
    let fixtures = generate_fixtures(&source_dir);
    let generation_ms = elapsed_ms(generation_start);
    assert_eq!(fixtures.len(), DOCUMENT_COUNT);
    println!("10K_ACCEPTANCE_GENERATION_MS={generation_ms}");

    let mut service = LibraryService::new(&state_dir).unwrap();
    let library = service.create_library(&library_dir).unwrap();
    let source_paths = vec![source_dir.to_string_lossy().into_owned()];
    let mut progress_events = 0_usize;
    let mut last_completed = 0_usize;
    let mut progress_finished = false;

    let import_start = Instant::now();
    let import_batch = service
        .start_import_with_progress(source_paths, |progress| {
            progress_events += 1;
            assert!(progress.completed >= last_completed);
            assert!(progress.completed <= progress.total);
            last_completed = progress.completed;
            if progress.finished {
                progress_finished = true;
            }
        })
        .unwrap();
    let import_ms = elapsed_ms(import_start);
    println!("10K_ACCEPTANCE_IMPORT_MS={import_ms}");
    assert_eq!(import_batch.imported_count, DOCUMENT_COUNT as i64);
    assert_eq!(import_batch.failed_count, 0);
    assert_eq!(import_batch.items.len(), DOCUMENT_COUNT);
    assert!(progress_finished);
    assert_eq!(last_completed, DOCUMENT_COUNT);

    let index_start = Instant::now();
    let index_result = service.index_pending_documents().unwrap();
    let index_ms = elapsed_ms(index_start);
    println!("10K_ACCEPTANCE_INDEX_MS={index_ms}");
    assert_eq!(index_result.processed, DOCUMENT_COUNT as i64);
    assert_eq!(index_result.searchable, DOCUMENT_COUNT as i64);
    assert_eq!(index_result.failed, 0);

    let list_start = Instant::now();
    let indexed_documents = service.list_documents().unwrap();
    let list_documents_ms = elapsed_ms(list_start);
    println!("10K_ACCEPTANCE_LIST_DOCUMENTS_MS={list_documents_ms}");
    assert_eq!(indexed_documents.len(), DOCUMENT_COUNT);
    assert!(indexed_documents.iter().all(|document| {
        document.processing_status == DocumentProcessingStatus::Ready
            && document.index_status == IndexStatus::Searchable
    }));

    let restart_start = Instant::now();
    drop(service);
    let mut service = LibraryService::new(&state_dir).unwrap();
    let bootstrap = service.bootstrap().unwrap();
    let restart_ms = elapsed_ms(restart_start);
    println!("10K_ACCEPTANCE_RESTART_MS={restart_ms}");
    assert_eq!(bootstrap.current_library.as_ref().unwrap().id, library.id);
    let restarted_documents = service.list_documents().unwrap();
    assert_eq!(restarted_documents.len(), DOCUMENT_COUNT);
    assert!(restarted_documents.iter().all(|document| {
        document.processing_status == DocumentProcessingStatus::Ready
            && document.index_status == IndexStatus::Searchable
    }));
    assert!(service.list_trash_documents().unwrap().is_empty());

    let organized_ids = choose_organized_document_ids(&restarted_documents);
    assert_eq!(organized_ids.len(), ORGANIZED_DOCUMENT_COUNT);
    let collection = service
        .create_collection("验收归档".to_string(), None)
        .unwrap();
    let tag = service.create_tag("重点验收".to_string()).unwrap();
    service
        .batch_organize_documents(
            BatchDocumentOperationRequest {
                job_id: "acceptance-move".to_string(),
                document_ids: organized_ids.clone(),
                operation: BatchDocumentOperation::MoveToCollection {
                    collection_id: collection.id.clone(),
                },
            },
            || false,
        )
        .unwrap();
    service
        .batch_organize_documents(
            BatchDocumentOperationRequest {
                job_id: "acceptance-tag".to_string(),
                document_ids: organized_ids.clone(),
                operation: BatchDocumentOperation::AddTag {
                    tag_id: tag.id.clone(),
                },
            },
            || false,
        )
        .unwrap();
    for document_id in &organized_ids {
        let document = restarted_documents
            .iter()
            .find(|document| &document.id == document_id)
            .unwrap();
        service
            .update_document_metadata(
                document_id,
                DocumentMetadataUpdate {
                    title: format!("{SHORT_MARKER} {}", document.title),
                    description: Some("10k acceptance metadata".to_string()),
                    document_date: Some(DOCUMENT_DATE.to_string()),
                    collection_id: collection.id.clone(),
                    tag_ids: vec![tag.id.clone()],
                },
            )
            .unwrap();
    }

    let (chinese_matches, chinese_search_ms) =
        timed_search(&service, CHINESE_MARKER, DocumentSearchFilters::default());
    let expected_chinese_count = fixtures
        .iter()
        .filter(|fixture| fixture.has_chinese_marker && fixture.has_extractable_text)
        .count();
    assert_eq!(chinese_matches.len(), expected_chinese_count);
    assert!(chinese_matches.iter().all(|result| {
        result.match_kind == SearchMatchKind::Content
            && result
                .snippet
                .as_deref()
                .is_some_and(|snippet| snippet.contains(CHINESE_MARKER))
    }));

    let (english_matches, english_search_ms) =
        timed_search(&service, ENGLISH_MARKER, DocumentSearchFilters::default());
    let expected_english_count = fixtures
        .iter()
        .filter(|fixture| fixture.has_english_marker && fixture.has_extractable_text)
        .count();
    assert_eq!(english_matches.len(), expected_english_count);
    assert!(english_matches.iter().all(|result| {
        result.match_kind == SearchMatchKind::Content
            && result
                .snippet
                .as_deref()
                .is_some_and(|snippet| snippet.contains(ENGLISH_MARKER))
    }));

    let (short_matches, short_search_ms) =
        timed_search(&service, SHORT_MARKER, DocumentSearchFilters::default());
    assert_eq!(short_matches.len(), ORGANIZED_DOCUMENT_COUNT);
    assert!(short_matches
        .iter()
        .all(|result| result.match_kind == SearchMatchKind::Metadata));

    let (combined_matches, combined_filter_ms) = timed_search(
        &service,
        "",
        DocumentSearchFilters {
            collection_id: Some(collection.id.clone()),
            tag_id: Some(tag.id.clone()),
            file_type: Some("TXT".to_string()),
            document_date_from: Some("2026-01-01".to_string()),
            document_date_to: Some("2026-12-31".to_string()),
        },
    );
    assert_eq!(combined_matches.len(), ORGANIZED_DOCUMENT_COUNT);
    assert!(combined_matches
        .iter()
        .all(|result| result.document.collection_id == collection.id));

    let preview_document = restarted_documents
        .iter()
        .find(|document| document.file_type == "TXT")
        .unwrap();
    let DocumentPreview::Text { text } = service
        .get_document_preview(&preview_document.id, None)
        .unwrap()
    else {
        panic!("TXT preview should return text");
    };
    assert!(text.contains("Acceptance document"));

    let (external_changed_ids, corrupt_pdf_id) =
        external_change_candidates(&restarted_documents, EXTERNAL_CHANGE_COUNT);
    assert_eq!(external_changed_ids.len(), EXTERNAL_CHANGE_COUNT);
    for (index, document_id) in external_changed_ids.iter().enumerate() {
        let document = restarted_documents
            .iter()
            .find(|document| &document.id == document_id)
            .unwrap();
        let copy_path = library_copy_path(&library_dir, document);
        let body = format!("{EXTERNAL_MARKER} {index:03}\n更新后的外部正文");
        fs::write(copy_path, body).unwrap();
    }
    let corrupt_pdf = restarted_documents
        .iter()
        .find(|document| document.id == corrupt_pdf_id)
        .unwrap();
    fs::write(
        library_copy_path(&library_dir, corrupt_pdf),
        b"this replacement is no longer a valid PDF",
    )
    .unwrap();

    drop(service);
    let restart_start = Instant::now();
    let mut service = LibraryService::new(&state_dir).unwrap();
    service.bootstrap().unwrap();
    let external_change_scan_ms = elapsed_ms(restart_start);
    println!("10K_ACCEPTANCE_EXTERNAL_SCAN_MS={external_change_scan_ms}");
    let pending_after_scan = service
        .list_documents()
        .unwrap()
        .into_iter()
        .filter(|document| document.index_status == IndexStatus::Pending)
        .count();
    assert_eq!(pending_after_scan, EXTERNAL_CHANGE_COUNT + 1);

    let external_index_start = Instant::now();
    let external_index = service.index_pending_documents().unwrap();
    let external_change_index_ms = elapsed_ms(external_index_start);
    println!("10K_ACCEPTANCE_EXTERNAL_INDEX_MS={external_change_index_ms}");
    assert_eq!(external_index.processed, (EXTERNAL_CHANGE_COUNT + 1) as i64);
    assert_eq!(external_index.searchable, EXTERNAL_CHANGE_COUNT as i64);
    assert_eq!(external_index.failed, 1);
    let (external_matches, _) =
        timed_search(&service, EXTERNAL_MARKER, DocumentSearchFilters::default());
    assert_eq!(external_matches.len(), EXTERNAL_CHANGE_COUNT);

    let mut final_documents = service.list_documents().unwrap();
    let trash_start = Instant::now();
    let trash_ids = final_documents
        .iter()
        .take(TRASH_DOCUMENT_COUNT)
        .map(|document| document.id.clone())
        .collect::<Vec<_>>();
    let trash_documents = trash_ids
        .iter()
        .map(|document_id| {
            final_documents
                .iter()
                .find(|document| &document.id == document_id)
                .unwrap()
                .clone()
        })
        .collect::<Vec<_>>();
    let trash_result = service
        .batch_organize_documents(
            BatchDocumentOperationRequest {
                job_id: "acceptance-trash".to_string(),
                document_ids: trash_ids.clone(),
                operation: BatchDocumentOperation::MoveToTrash,
            },
            || false,
        )
        .unwrap();
    assert_eq!(trash_result.succeeded_count, TRASH_DOCUMENT_COUNT);
    assert_eq!(
        service.list_trash_documents().unwrap().len(),
        TRASH_DOCUMENT_COUNT
    );
    assert_eq!(
        service.list_documents().unwrap().len(),
        DOCUMENT_COUNT - TRASH_DOCUMENT_COUNT
    );

    for document_id in trash_ids.iter().take(RESTORE_DOCUMENT_COUNT) {
        service.restore_document(document_id).unwrap();
    }
    for document in trash_documents.iter().skip(RESTORE_DOCUMENT_COUNT) {
        assert!(library_copy_path(&library_dir, document).is_file());
        service.permanently_delete_document(&document.id).unwrap();
        assert!(!library_copy_path(&library_dir, document).exists());
    }
    let trash_transition_ms = elapsed_ms(trash_start);
    assert!(service.list_trash_documents().unwrap().is_empty());
    final_documents = service.list_documents().unwrap();
    assert_eq!(
        final_documents.len(),
        DOCUMENT_COUNT - TRASH_DOCUMENT_COUNT + RESTORE_DOCUMENT_COUNT
    );
    let restored_count = service
        .list_documents()
        .unwrap()
        .iter()
        .filter(|document| {
            trash_ids
                .iter()
                .take(RESTORE_DOCUMENT_COUNT)
                .any(|id| id == &document.id)
        })
        .count();
    assert_eq!(restored_count, RESTORE_DOCUMENT_COUNT);
    let active_document_ids = final_documents
        .iter()
        .map(|document| document.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let expected_tag_document_count = organized_ids
        .iter()
        .filter(|document_id| active_document_ids.contains(document_id.as_str()))
        .count() as i64;
    assert_eq!(
        service
            .list_tags()
            .unwrap()
            .iter()
            .find(|item| item.id == tag.id)
            .unwrap()
            .document_count,
        expected_tag_document_count
    );

    let source_verification_start = Instant::now();
    let source_files_unchanged = fixtures.iter().all(|fixture| {
        fs::read(&fixture.path)
            .map(|bytes| bytes == fixture.original_bytes)
            .unwrap_or(false)
    });
    let source_verification_ms = elapsed_ms(source_verification_start);
    println!("10K_ACCEPTANCE_SOURCE_VERIFY_MS={source_verification_ms}");
    assert!(source_files_unchanged);

    service
        .forget_recent_library(Path::new(&library.path))
        .unwrap();
    assert!(library_dir.join(".pdm").join("library.json").is_file());
    assert!(library_dir.join(".pdm").join("library.sqlite3").is_file());
    assert!(library_dir.join("documents").is_dir());
    assert!(library_dir.join("trash").is_dir());
    assert!(library_dir.join("thumbnails").is_dir());
    drop(service);
    assert!(library_dir.join(".pdm").join("library.json").is_file());
    assert!(library_dir.join(".pdm").join("library.sqlite3").is_file());

    let metrics = AcceptanceMetrics {
        document_count: DOCUMENT_COUNT,
        formats: vec!["PDF", "DOCX", "TXT", "Markdown", "JPG", "PNG"],
        generation_ms,
        import_ms,
        index_ms,
        restart_ms,
        list_documents_ms,
        chinese_search_ms,
        english_search_ms,
        short_search_ms,
        combined_filter_ms,
        external_change_scan_ms,
        external_change_index_ms,
        trash_transition_ms,
        source_verification_ms,
        progress_events,
        imported_count: import_batch.imported_count,
        indexed_count: index_result.searchable,
        chinese_match_count: chinese_matches.len(),
        english_match_count: english_matches.len(),
        short_match_count: short_matches.len(),
        combined_match_count: combined_matches.len(),
        external_searchable_count: external_matches.len(),
        external_failed_count: external_index.failed,
        active_document_count: final_documents.len(),
        restored_count,
        permanently_deleted_count: TRASH_DOCUMENT_COUNT - RESTORE_DOCUMENT_COUNT,
        source_file_count: fixtures.len(),
        source_files_unchanged,
    };
    println!(
        "10K_ACCEPTANCE_METRICS={}",
        serde_json::to_string(&metrics).unwrap()
    );
}

fn generate_fixtures(source_dir: &Path) -> Vec<Fixture> {
    let mut fixtures = Vec::with_capacity(DOCUMENT_COUNT);
    for index in 0..DOCUMENT_COUNT {
        let extension = match index % 6 {
            0 => "pdf",
            1 => "docx",
            2 => "txt",
            3 => "md",
            4 => "jpg",
            _ => "png",
        };
        let has_chinese_marker = index % 5 == 0;
        let has_english_marker = index % 7 == 0;
        let body = synthetic_body(index, has_chinese_marker, has_english_marker);
        let bytes = render_document(extension, &body, index);
        let has_extractable_text = matches!(extension, "pdf" | "docx" | "txt" | "md");
        let path = source_dir.join(format!("acceptance-{index:05}.{extension}"));
        fs::write(&path, &bytes).unwrap();
        fixtures.push(Fixture {
            path,
            original_bytes: bytes,
            has_chinese_marker,
            has_english_marker,
            has_extractable_text,
        });
    }
    fixtures
}

fn synthetic_body(index: usize, has_chinese_marker: bool, has_english_marker: bool) -> String {
    let mut lines = vec![
        format!("Acceptance document {index:05}"),
        format!("Deterministic body for document {index:05}."),
    ];
    if has_chinese_marker {
        lines.push(format!("{CHINESE_MARKER} 中文正文 {index:05}"));
    }
    if has_english_marker {
        lines.push(format!("{ENGLISH_MARKER} for document {index:05}"));
    }
    lines.push(format!("Atlas 计划 {index:05}"));
    lines.join("\n")
}

fn render_document(extension: &str, body: &str, index: usize) -> Vec<u8> {
    match extension {
        "pdf" => format!(
            "%PDF-1.4\n1 0 obj << /Type /Pages /Count 1 >> endobj\n2 0 obj << /Length {} >>\nstream\nBT ({}) Tj ET\nendstream\n%%EOF",
            body.len(),
            escape_pdf_text(body)
        )
        .into_bytes(),
        "docx" => {
            let xml = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body><w:p><w:r><w:t>{}</w:t></w:r></w:p></w:body>
</w:document>"#,
                escape_xml_text(body)
            );
            stored_zip(&[("word/document.xml", xml.as_bytes())])
        }
        "txt" | "md" => body.as_bytes().to_vec(),
        "jpg" => {
            let mut bytes = vec![0xFF, 0xD8, 0xFF];
            bytes.extend_from_slice(format!("synthetic-jpeg-{index:05}").as_bytes());
            bytes
        }
        "png" => {
            let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
            bytes.extend_from_slice(format!("synthetic-png-{index:05}").as_bytes());
            bytes
        }
        _ => unreachable!(),
    }
}

fn choose_organized_document_ids(documents: &[DocumentSummary]) -> Vec<String> {
    documents
        .iter()
        .filter(|document| document.file_type == "TXT")
        .take(ORGANIZED_DOCUMENT_COUNT)
        .map(|document| document.id.clone())
        .collect()
}

fn external_change_candidates(
    documents: &[DocumentSummary],
    change_count: usize,
) -> (Vec<String>, String) {
    let changed_ids = documents
        .iter()
        .filter(|document| document.file_type == "TXT")
        .take(change_count)
        .map(|document| document.id.clone())
        .collect::<Vec<_>>();
    let corrupt_pdf_id = documents
        .iter()
        .find(|document| document.file_type == "PDF")
        .unwrap()
        .id
        .clone();
    assert_eq!(changed_ids.len(), change_count);
    (changed_ids, corrupt_pdf_id)
}

fn library_copy_path(library_dir: &Path, document: &DocumentSummary) -> PathBuf {
    library_dir
        .join("documents")
        .join(&document.id)
        .join(&document.file_name)
}

fn timed_search(
    service: &LibraryService,
    query: &str,
    filters: DocumentSearchFilters,
) -> (
    Vec<personal_document_manager_lib::library::DocumentSearchResult>,
    u64,
) {
    let start = Instant::now();
    let DocumentSearchResponse { results } = service
        .search_documents(DocumentSearchQuery {
            query: query.to_string(),
            filters,
        })
        .unwrap();
    (results, elapsed_ms(start))
}

fn elapsed_ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn escape_pdf_text(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

fn escape_xml_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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
