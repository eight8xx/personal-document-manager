use std::fs;
use std::path::{Path, PathBuf};

use personal_document_manager_lib::library::{
    DocumentMetadataUpdate, DocumentPreview, DocumentProcessingStatus, DocumentSearchFilters,
    DocumentSearchQuery, DocumentSummary, ImportDecision, ImportItemStatus, IndexStatus,
    LibraryService,
};
use tempfile::tempdir;

fn backup_path(directory: &Path, label: &str) -> PathBuf {
    directory.join(format!(".{label}.previous"))
}

fn copy_path(library: &Path, document: &DocumentSummary) -> PathBuf {
    library
        .join("documents")
        .join(&document.id)
        .join(&document.file_name)
}

fn text_preview(service: &LibraryService, document_id: &str) -> String {
    match service.get_document_preview(document_id, None).unwrap() {
        DocumentPreview::Text { text } => text,
        other => panic!("unexpected preview: {other:?}"),
    }
}

fn search_count(service: &LibraryService, query: &str) -> usize {
    service
        .search_documents(DocumentSearchQuery {
            query: query.to_string(),
            filters: DocumentSearchFilters::default(),
        })
        .unwrap()
        .results
        .len()
}

fn document_by_id(service: &LibraryService, document_id: &str) -> DocumentSummary {
    service
        .list_documents()
        .unwrap()
        .into_iter()
        .find(|document| document.id == document_id)
        .expect("document is listed")
}

/// 文档目录里的替换残留：以 `.` 开头且以 `.previous` 或 `.importing` 结尾的文件。
fn artifact_paths(directory: &Path) -> Vec<PathBuf> {
    let mut paths = fs::read_dir(directory)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    let name = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    name.starts_with('.')
                        && (name.ends_with(".previous") || name.ends_with(".importing"))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    paths.sort();
    paths
}

/// 中断点一：旧副本已改名为备份，新副本尚未落位，数据库事务未提交。
#[test]
fn reopening_restores_a_staged_old_copy_before_the_new_copy_lands() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("changing.txt");
    fs::write(&source_path, "first searchable version").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let document = service.import_document(&source_path).unwrap();
    service.index_pending_documents().unwrap();
    drop(service);

    let copy = copy_path(&library_dir, &document);
    let directory = copy.parent().unwrap().to_path_buf();
    let backup = backup_path(&directory, "staged-old-copy");
    let abandoned_temporary = directory.join(".abandoned-new-copy.importing");
    fs::rename(&copy, &backup).unwrap();
    fs::write(&abandoned_temporary, "second searchable version").unwrap();
    fs::write(&source_path, "second searchable version").unwrap();

    for attempt in 0..2 {
        let mut reopened = LibraryService::new(&state_dir).unwrap();
        reopened.open_library(&library_dir).unwrap();
        let documents = reopened.list_documents().unwrap();
        assert_eq!(documents.len(), 1, "attempt {attempt}");
        assert_eq!(copy, copy_path(&library_dir, &documents[0]));
        assert_eq!(
            text_preview(&reopened, &document.id),
            "first searchable version"
        );
        assert_eq!(search_count(&reopened, "first searchable"), 1);
        assert!(copy.is_file());
        assert!(!backup.exists());
        assert!(
            !abandoned_temporary.exists(),
            "中断遗留的临时副本应当被清理"
        );
    }
    assert_eq!(
        fs::read(&source_path).unwrap(),
        b"second searchable version",
        "恢复不能修改源文件"
    );

    let mut retried = LibraryService::new(&state_dir).unwrap();
    retried.open_library(&library_dir).unwrap();
    let changed = retried
        .start_import(vec![source_path.to_string_lossy().into_owned()])
        .unwrap()
        .items
        .remove(0);
    assert_eq!(changed.status, ImportItemStatus::SourceChanged);
    let replaced = retried
        .resolve_import_item(&changed.item_id, ImportDecision::ReplaceExisting)
        .unwrap();
    assert_eq!(replaced.status, ImportItemStatus::Imported);
    retried.index_pending_documents().unwrap();
    assert_eq!(
        text_preview(&retried, &document.id),
        "second searchable version"
    );
    assert_eq!(search_count(&retried, "second searchable"), 1);
    assert_eq!(retried.list_documents().unwrap().len(), 1);
}

/// 中断点二：新副本已落位到原路径，但数据库事务未提交。
#[test]
fn reopening_restores_the_recorded_version_when_an_uncommitted_copy_landed() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("changing.txt");
    fs::write(&source_path, "first searchable version").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let document = service.import_document(&source_path).unwrap();
    service.index_pending_documents().unwrap();
    drop(service);

    let copy = copy_path(&library_dir, &document);
    let backup = backup_path(copy.parent().unwrap(), "uncommitted-new-copy");
    fs::write(&copy, "second searchable version").unwrap();
    fs::write(&backup, "first searchable version").unwrap();

    let mut reopened = LibraryService::new(&state_dir).unwrap();
    reopened.open_library(&library_dir).unwrap();
    assert_eq!(fs::read(&copy).unwrap(), b"first searchable version");
    assert_eq!(
        text_preview(&reopened, &document.id),
        "first searchable version"
    );
    assert_eq!(search_count(&reopened, "first searchable"), 1);
    assert_eq!(search_count(&reopened, "second searchable"), 0);
    assert!(!backup.exists());
    assert_eq!(reopened.list_documents().unwrap().len(), 1);

    let listed = document_by_id(&reopened, &document.id);
    assert_eq!(listed.processing_status, DocumentProcessingStatus::Ready);
    assert_eq!(listed.index_status, IndexStatus::Searchable);
}

/// 中断点三：替换已提交，进程在删除备份前退出。
#[test]
fn reopening_keeps_the_committed_copy_and_removes_leftover_backups() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("changing.txt");
    fs::write(&source_path, "first searchable version").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let collection = service.create_collection("项目".to_string(), None).unwrap();
    let tag = service.create_tag("重要".to_string()).unwrap();
    let document = service.import_document(&source_path).unwrap();
    service.index_pending_documents().unwrap();
    service
        .update_document_metadata(
            &document.id,
            DocumentMetadataUpdate {
                title: "变更记录".to_string(),
                description: Some("替换前的描述".to_string()),
                document_date: None,
                collection_id: collection.id.clone(),
                tag_ids: vec![tag.id.clone()],
            },
        )
        .unwrap();

    fs::write(&source_path, "second searchable version").unwrap();
    let changed = service
        .start_import(vec![source_path.to_string_lossy().into_owned()])
        .unwrap()
        .items
        .remove(0);
    assert_eq!(changed.status, ImportItemStatus::SourceChanged);
    let replaced = service
        .resolve_import_item(&changed.item_id, ImportDecision::ReplaceExisting)
        .unwrap();
    assert_eq!(replaced.status, ImportItemStatus::Imported);
    service.index_pending_documents().unwrap();

    let copy = copy_path(&library_dir, &document);
    let backup = backup_path(copy.parent().unwrap(), "leftover-old-copy");
    fs::write(&backup, "first searchable version").unwrap();
    drop(service);

    let mut reopened = LibraryService::new(&state_dir).unwrap();
    reopened.open_library(&library_dir).unwrap();
    assert_eq!(fs::read(&copy).unwrap(), b"second searchable version");
    assert_eq!(
        text_preview(&reopened, &document.id),
        "second searchable version"
    );
    assert_eq!(search_count(&reopened, "second searchable"), 1);
    assert!(!backup.exists());
    assert_eq!(fs::read(&source_path).unwrap(), b"second searchable version");

    let listed = document_by_id(&reopened, &document.id);
    assert_eq!(listed.title, "变更记录");
    assert_eq!(listed.description.as_deref(), Some("替换前的描述"));
    assert_eq!(listed.collection_id, collection.id);
    assert_eq!(listed.tags.len(), 1);
    assert_eq!(listed.tags[0].id, tag.id);
    assert_eq!(listed.processing_status, DocumentProcessingStatus::Ready);
    assert_eq!(listed.index_status, IndexStatus::Searchable);
}

/// 无法自动恢复时报告原因、保留诊断状态，且不影响同一资料库的其他文档。
#[test]
fn unrecoverable_replacement_reports_the_document_and_keeps_the_rest_usable() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let broken_source = root.path().join("broken.txt");
    let healthy_source = root.path().join("healthy.txt");
    fs::write(&broken_source, "first searchable version").unwrap();
    fs::write(&healthy_source, "healthy searchable contents").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let broken = service.import_document(&broken_source).unwrap();
    let healthy = service.import_document(&healthy_source).unwrap();
    service.index_pending_documents().unwrap();
    drop(service);

    let broken_copy = copy_path(&library_dir, &broken);
    let backup = backup_path(broken_copy.parent().unwrap(), "unmatched-backup");
    fs::remove_file(&broken_copy).unwrap();
    fs::write(&backup, "neither recorded version").unwrap();

    let mut reopened = LibraryService::new(&state_dir).unwrap();
    reopened.open_library(&library_dir).unwrap();
    assert_eq!(reopened.list_documents().unwrap().len(), 2);

    let reported = document_by_id(&reopened, &broken.id);
    assert_eq!(reported.processing_status, DocumentProcessingStatus::Failed);
    assert_eq!(reported.index_status, IndexStatus::Failed);
    assert_eq!(reported.error_stage.as_deref(), Some("recovery"));
    assert!(reported
        .error_message
        .as_deref()
        .unwrap_or_default()
        .contains("替换"));
    assert!(backup.is_file(), "无法恢复时保留可诊断的备份文件");
    assert!(!broken_copy.exists());

    let usable = document_by_id(&reopened, &healthy.id);
    assert_eq!(usable.processing_status, DocumentProcessingStatus::Ready);
    assert_eq!(
        text_preview(&reopened, &healthy.id),
        "healthy searchable contents"
    );
    assert_eq!(search_count(&reopened, "healthy searchable"), 1);
    drop(reopened);

    let mut reopened_again = LibraryService::new(&state_dir).unwrap();
    reopened_again.open_library(&library_dir).unwrap();
    let repeated = document_by_id(&reopened_again, &broken.id);
    assert_eq!(repeated.processing_status, DocumentProcessingStatus::Failed);
    assert_eq!(repeated.index_status, IndexStatus::Failed);
    assert!(backup.is_file());
    assert_eq!(search_count(&reopened_again, "healthy searchable"), 1);
}

/// 中断点零：新内容只写进了临时副本，旧副本尚未暂存，数据库未提交。
/// 记录的内容仍然可读，未落位的临时副本和更早遗留的备份都应被清理。
#[test]
fn interrupted_before_staging_keeps_the_recorded_copy_and_drops_the_temporary() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("changing.txt");
    fs::write(&source_path, "first searchable version").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let document = service.import_document(&source_path).unwrap();
    service.index_pending_documents().unwrap();
    drop(service);

    let copy = copy_path(&library_dir, &document);
    let directory = copy.parent().unwrap().to_path_buf();
    let abandoned_temporary = directory.join(".abandoned-new-copy.importing");
    let stale_backup = backup_path(&directory, "stale-previous-attempt");
    fs::write(&abandoned_temporary, "second searchable version").unwrap();
    fs::write(&stale_backup, "stale leftover from an older attempt").unwrap();
    fs::write(&source_path, "second searchable version").unwrap();

    for attempt in 0..2 {
        let mut reopened = LibraryService::new(&state_dir).unwrap();
        reopened.open_library(&library_dir).unwrap();
        assert_eq!(fs::read(&copy).unwrap(), b"first searchable version");
        assert_eq!(
            text_preview(&reopened, &document.id),
            "first searchable version"
        );
        assert_eq!(search_count(&reopened, "first searchable"), 1);
        assert_eq!(search_count(&reopened, "second searchable"), 0);
        assert!(
            artifact_paths(&directory).is_empty(),
            "第 {attempt} 次重开后不应留下中断残留：{:?}",
            artifact_paths(&directory)
        );
        assert_eq!(reopened.list_documents().unwrap().len(), 1);
    }
    assert_eq!(fs::read(&source_path).unwrap(), b"second searchable version");
}

/// 多个残留同时存在时只采用与记录一致的那一份，其余清理，且不会二次替换。
#[test]
fn staged_backup_is_preferred_over_unrelated_leftovers() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("changing.txt");
    fs::write(&source_path, "first searchable version").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let collection = service.create_collection("资料".to_string(), None).unwrap();
    let tag = service.create_tag("待办".to_string()).unwrap();
    let document = service.import_document(&source_path).unwrap();
    service.index_pending_documents().unwrap();
    service
        .update_document_metadata(
            &document.id,
            DocumentMetadataUpdate {
                title: "替换目标".to_string(),
                description: Some("保留描述".to_string()),
                document_date: None,
                collection_id: collection.id.clone(),
                tag_ids: vec![tag.id.clone()],
            },
        )
        .unwrap();
    drop(service);

    let copy = copy_path(&library_dir, &document);
    let directory = copy.parent().unwrap().to_path_buf();
    let recorded_backup = backup_path(&directory, "recorded-old-copy");
    let other_backup = backup_path(&directory, "unrelated-old-copy");
    let partial_temporary = directory.join(".half-written.importing");
    fs::rename(&copy, &recorded_backup).unwrap();
    fs::write(&other_backup, "unrelated leftover").unwrap();
    fs::write(&partial_temporary, "partial").unwrap();
    fs::write(&source_path, "second searchable version").unwrap();

    let mut reopened = LibraryService::new(&state_dir).unwrap();
    reopened.open_library(&library_dir).unwrap();
    assert_eq!(fs::read(&copy).unwrap(), b"first searchable version");
    assert_eq!(
        text_preview(&reopened, &document.id),
        "first searchable version"
    );
    assert_eq!(search_count(&reopened, "first searchable"), 1);
    assert!(artifact_paths(&directory).is_empty());
    assert_eq!(reopened.list_documents().unwrap().len(), 1);

    let listed = document_by_id(&reopened, &document.id);
    assert_eq!(listed.title, "替换目标");
    assert_eq!(listed.description.as_deref(), Some("保留描述"));
    assert_eq!(listed.collection_id, collection.id);
    assert_eq!(listed.tags.len(), 1);
    assert_eq!(listed.tags[0].id, tag.id);
    assert_eq!(listed.processing_status, DocumentProcessingStatus::Ready);
    assert_eq!(listed.index_status, IndexStatus::Searchable);
}

/// 没有数据库记录的中断导入（新文档在提交前退出）只清理残留，不产生活动文档。
#[test]
fn orphan_temporary_copy_is_removed_without_creating_documents() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("healthy.txt");
    fs::write(&source_path, "healthy searchable contents").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let healthy = service.import_document(&source_path).unwrap();
    service.index_pending_documents().unwrap();
    drop(service);

    let orphan_directory = library_dir.join("documents").join("orphan-document-id");
    fs::create_dir_all(&orphan_directory).unwrap();
    let orphan_temporary = orphan_directory.join(".never-committed.importing");
    fs::write(&orphan_temporary, "never committed contents").unwrap();

    for attempt in 0..2 {
        let mut reopened = LibraryService::new(&state_dir).unwrap();
        reopened.open_library(&library_dir).unwrap();
        assert_eq!(reopened.list_documents().unwrap().len(), 1, "attempt {attempt}");
        assert!(
            !orphan_temporary.exists(),
            "第 {attempt} 次重开应清理无记录的临时副本"
        );
        assert!(
            !orphan_directory.exists(),
            "第 {attempt} 次重开应清理无记录的空目录"
        );
        assert_eq!(
            text_preview(&reopened, &healthy.id),
            "healthy searchable contents"
        );
        assert_eq!(search_count(&reopened, "healthy searchable"), 1);
    }
}

/// 正常替换完成后不留下任何残留；重复重开不会再次替换或改变可读内容。
#[test]
fn successful_replacement_leaves_no_artifacts_across_repeated_reopen() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let source_path = root.path().join("changing.txt");
    fs::write(&source_path, "first searchable version").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let document = service.import_document(&source_path).unwrap();
    service.index_pending_documents().unwrap();

    fs::write(&source_path, "second searchable version").unwrap();
    let changed = service
        .start_import(vec![source_path.to_string_lossy().into_owned()])
        .unwrap()
        .items
        .remove(0);
    assert_eq!(changed.status, ImportItemStatus::SourceChanged);
    let replaced = service
        .resolve_import_item(&changed.item_id, ImportDecision::ReplaceExisting)
        .unwrap();
    assert_eq!(replaced.status, ImportItemStatus::Imported);
    service.index_pending_documents().unwrap();

    let copy = copy_path(&library_dir, &document);
    let directory = copy.parent().unwrap().to_path_buf();
    assert!(
        artifact_paths(&directory).is_empty(),
        "正常替换后不应保留残留：{:?}",
        artifact_paths(&directory)
    );
    assert_eq!(fs::read(&copy).unwrap(), b"second searchable version");
    drop(service);

    for attempt in 0..3 {
        let mut reopened = LibraryService::new(&state_dir).unwrap();
        reopened.open_library(&library_dir).unwrap();
        assert_eq!(fs::read(&copy).unwrap(), b"second searchable version");
        assert_eq!(
            text_preview(&reopened, &document.id),
            "second searchable version",
            "attempt {attempt}"
        );
        assert_eq!(search_count(&reopened, "second searchable"), 1);
        assert_eq!(search_count(&reopened, "first searchable"), 0);
        assert!(artifact_paths(&directory).is_empty());
        assert_eq!(reopened.list_documents().unwrap().len(), 1);
        assert_eq!(fs::read(&source_path).unwrap(), b"second searchable version");
    }
}

/// 同一批次里既有成功的导入也有待决的替换：替换中断后只恢复该文档，
/// 其余同批文档保持可读可搜索，重复重开也不会产生额外活动文档。
#[test]
fn interrupted_replacement_keeps_other_batch_items_usable() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let changing_path = root.path().join("changing.txt");
    let stable_path = root.path().join("stable.txt");
    let added_path = root.path().join("added.txt");
    fs::write(&changing_path, "first searchable version").unwrap();
    fs::write(&stable_path, "stable searchable contents").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let changing = service.import_document(&changing_path).unwrap();
    let stable = service.import_document(&stable_path).unwrap();
    service.index_pending_documents().unwrap();

    fs::write(&changing_path, "second searchable version").unwrap();
    fs::write(&added_path, "added searchable contents").unwrap();
    let batch = service
        .start_import(vec![
            changing_path.to_string_lossy().into_owned(),
            added_path.to_string_lossy().into_owned(),
            stable_path.to_string_lossy().into_owned(),
        ])
        .unwrap();
    let mut items = batch.items.clone();
    let changed_index = items
        .iter()
        .position(|item| item.status == ImportItemStatus::SourceChanged)
        .expect("批次里应有来源变化项");
    let changed = items.remove(changed_index);
    assert_eq!(changed.duplicate_document_id.as_deref(), Some(changing.id.as_str()));
    assert_eq!(
        items
            .iter()
            .filter(|item| item.status == ImportItemStatus::Imported)
            .count(),
        1,
        "同一批次里另一个新文件应已成功导入"
    );
    let added_document_id = items
        .iter()
        .find(|item| item.status == ImportItemStatus::Imported)
        .and_then(|item| item.document_id.clone())
        .expect("成功导入项应带文档标识");
    let replaced = service
        .resolve_import_item(&changed.item_id, ImportDecision::ReplaceExisting)
        .unwrap();
    assert_eq!(replaced.status, ImportItemStatus::Imported);
    service.index_pending_documents().unwrap();
    assert_eq!(service.list_documents().unwrap().len(), 3);
    drop(service);

    // 模拟替换在旧副本暂存后中断：新副本未落位，数据库保持已提交内容。
    let changing_copy = copy_path(&library_dir, &changing);
    let changing_directory = changing_copy.parent().unwrap().to_path_buf();
    fs::rename(&changing_copy, backup_path(&changing_directory, "staged")).unwrap();
    fs::write(
        changing_directory.join(".abandoned-new-copy.importing"),
        "third searchable version",
    )
    .unwrap();

    for attempt in 0..2 {
        let mut reopened = LibraryService::new(&state_dir).unwrap();
        reopened.open_library(&library_dir).unwrap();
        assert_eq!(reopened.list_documents().unwrap().len(), 3, "attempt {attempt}");
        assert_eq!(
            text_preview(&reopened, &changing.id),
            "second searchable version",
            "attempt {attempt}"
        );
        assert_eq!(search_count(&reopened, "second searchable"), 1);
        assert_eq!(
            text_preview(&reopened, &stable.id),
            "stable searchable contents"
        );
        assert_eq!(search_count(&reopened, "stable searchable"), 1);
        assert_eq!(
            text_preview(&reopened, &added_document_id),
            "added searchable contents"
        );
        assert_eq!(search_count(&reopened, "added searchable"), 1);
        assert!(artifact_paths(&changing_directory).is_empty());
        let added = document_by_id(&reopened, &added_document_id);
        assert_eq!(added.processing_status, DocumentProcessingStatus::Ready);
        assert_eq!(added.index_status, IndexStatus::Searchable);
    }
}

/// 无法恢复时即使记录路径上仍留着内容不一致的副本，打开资料库后也必须保留恢复失败的原因，
/// 不能被随后的启动哈希扫描改写成“待索引”，否则用户看不到诊断信息。
#[test]
fn unrecoverable_replacement_keeps_its_report_when_the_copy_still_exists() {
    let root = tempdir().unwrap();
    let state_dir = root.path().join("app-state");
    let library_dir = root.path().join("Library");
    let broken_source = root.path().join("broken.txt");
    let healthy_source = root.path().join("healthy.txt");
    fs::write(&broken_source, "first searchable version").unwrap();
    fs::write(&healthy_source, "healthy searchable contents").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let broken = service.import_document(&broken_source).unwrap();
    let healthy = service.import_document(&healthy_source).unwrap();
    service.index_pending_documents().unwrap();
    drop(service);

    let broken_copy = copy_path(&library_dir, &broken);
    let directory = broken_copy.parent().unwrap().to_path_buf();
    let backup = backup_path(&directory, "unmatched-backup");
    // 未提交的新副本落在记录路径上，备份又不是记录内容：无法判断哪一份才是记录版本。
    fs::write(&broken_copy, "uncommitted third version").unwrap();
    fs::write(&backup, "neither recorded version").unwrap();

    for attempt in 0..2 {
        let mut reopened = LibraryService::new(&state_dir).unwrap();
        reopened.open_library(&library_dir).unwrap();
        assert_eq!(reopened.list_documents().unwrap().len(), 2, "attempt {attempt}");

        let reported = document_by_id(&reopened, &broken.id);
        assert_eq!(
            reported.processing_status,
            DocumentProcessingStatus::Failed,
            "attempt {attempt}"
        );
        assert_eq!(reported.index_status, IndexStatus::Failed, "attempt {attempt}");
        assert_eq!(reported.error_stage.as_deref(), Some("recovery"));
        assert!(reported
            .error_message
            .as_deref()
            .unwrap_or_default()
            .contains("替换"));
        assert!(backup.is_file(), "无法恢复时保留可诊断的备份文件");
        assert_eq!(fs::read(&broken_copy).unwrap(), b"uncommitted third version");

        let usable = document_by_id(&reopened, &healthy.id);
        assert_eq!(usable.processing_status, DocumentProcessingStatus::Ready);
        assert_eq!(
            text_preview(&reopened, &healthy.id),
            "healthy searchable contents"
        );
        assert_eq!(search_count(&reopened, "healthy searchable"), 1);
    }
}
