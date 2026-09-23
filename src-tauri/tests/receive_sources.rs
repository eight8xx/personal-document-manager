//! 分类规则与接收目录的端到端集成测试（工作单 10/11/12/13）。
//!
//! 全部使用真实临时资料库与真实目录，不 mock 文件系统或数据库。

use std::fs;
use std::path::{Path, PathBuf};

use personal_document_manager_lib::library::{
    ClassificationPreviewRequest, ClassificationRuleInput, ClassificationRuleOperation, ImportSource,
    LibraryService, ReceiveSourceInput, ReceiveSourceKind, ReceiveSourceScanResult,
    ReceiveSourceStatus,
};
use tempfile::tempdir;

fn setup(root: &Path) -> (LibraryService, PathBuf) {
    let mut service = LibraryService::new(root.join("app-state")).unwrap();
    let library_dir = root.join("Library");
    service.create_library(&library_dir).unwrap();
    (service, library_dir)
}

fn inbox_id(service: &LibraryService) -> String {
    service
        .list_collections()
        .unwrap()
        .into_iter()
        .find(|collection| collection.is_inbox)
        .expect("资料库应当有收件箱")
        .id
}

fn rule(name: &str, file_name_pattern: &str, collection_id: Option<String>, tag_ids: Vec<String>) -> ClassificationRuleInput {
    ClassificationRuleInput {
        name: name.to_string(),
        enabled: true,
        file_name_pattern: file_name_pattern.to_string(),
        file_type: None,
        source_directory: None,
        collection_id,
        tag_ids,
    }
}

/// 模拟监视线程/命令层的补扫驱动：逐文件分步导入。
fn scan_and_import(service: &mut LibraryService) -> Vec<ReceiveSourceScanResult> {
    let sources = service.list_receive_sources().unwrap();
    let mut results = Vec::new();
    for source in sources {
        if !source.enabled || source.path.is_none() || source.last_scanned_at.is_none() {
            continue;
        }
        let pending = service.receive_pending_files(&source.id).unwrap();
        if pending.is_empty() {
            continue;
        }
        let first = service
            .begin_receive_import_batch(&source.id, pending)
            .unwrap();
        let batch_id = first.batch_id.clone();
        while service.peek_import_progress(&batch_id).unwrap().is_some() {
            service.import_batch_step(&batch_id).unwrap();
        }
        results.push(service.finish_receive_import_batch(&batch_id).unwrap());
    }
    results
}

/// 首次启用来源：确认目录 → 列出清单 → 跳过未选项 → 导入所选项。
fn enable_source(
    service: &mut LibraryService,
    kind: ReceiveSourceKind,
    dir: &Path,
    selected: &[&Path],
    all: &[&Path],
) -> String {
    // 同一个目录重复配置时复用已有来源，避免唯一索引冲突。
    let dir_text = dir.to_string_lossy().into_owned();
    let existing = service
        .list_receive_sources()
        .unwrap()
        .into_iter()
        .find(|source| {
            source.kind == kind && source.path.as_deref() == Some(dir_text.as_str())
        })
        .map(|source| source.id);
    let sources = service
        .upsert_receive_source(
            existing.as_deref(),
            ReceiveSourceInput {
                kind,
                display_name: format!("{kind:?} 接收"),
                path: dir_text,
                enabled: true,
            },
        )
        .unwrap();
    let source = sources
        .into_iter()
        .find(|source| source.kind == kind)
        .expect("来源应当已保存");

    let skipped = all
        .iter()
        .filter(|path| !selected.contains(path))
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    if !skipped.is_empty() {
        service
            .skip_receive_directory_files(personal_document_manager_lib::library::ReceiveDirectoryOperation {
                source_id: source.id.clone(),
                paths: skipped,
            })
            .unwrap();
    }
    if !selected.is_empty() {
        let first = service
            .begin_receive_import_batch(
                &source.id,
                selected
                    .iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect(),
            )
            .unwrap();
        let batch_id = first.batch_id.clone();
        while service.peek_import_progress(&batch_id).unwrap().is_some() {
            service.import_batch_step(&batch_id).unwrap();
        }
        service.finish_receive_import_batch(&batch_id).unwrap();
    } else {
        // 全部跳过也要把来源标记为已完成首次确认。
        service
            .skip_receive_directory_files(personal_document_manager_lib::library::ReceiveDirectoryOperation {
                source_id: source.id.clone(),
                paths: Vec::new(),
            })
            .unwrap();
    }
    source.id
}

fn receive_dir(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(name);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn file_type_of(path: &Path) -> Option<&'static str> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .filter(|extension| extension == "txt")
        .map(|_| "TXT")
}

fn supported_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| file_type_of(path).is_some())
        .collect::<Vec<_>>();
    files.sort();
    files
}

// ---------------- 工作单 10：分类规则 ----------------

#[test]
fn classification_rules_are_scoped_to_their_library() {
    let root = tempdir().unwrap();
    let mut service = LibraryService::new(root.path().join("app-state")).unwrap();
    let first = root.path().join("First");
    let second = root.path().join("Second");
    service.create_library(&first).unwrap();
    service.create_library(&second).unwrap();

    service.open_library(&first).unwrap();
    let collection = service.create_collection("归档".to_string(), None).unwrap();
    service
        .apply_classification_rule_operation(ClassificationRuleOperation::Create {
            rule: rule("A 库规则", "报告", Some(collection.id.clone()), Vec::new()),
        })
        .unwrap();
    assert_eq!(service.list_classification_rules().unwrap().len(), 1);

    service.open_library(&second).unwrap();
    assert!(
        service.list_classification_rules().unwrap().is_empty(),
        "另一个资料库不应看到本库规则"
    );
    service.open_library(&first).unwrap();
    assert_eq!(service.list_classification_rules().unwrap().len(), 1);
}

#[test]
fn first_rule_with_collection_wins_and_tags_are_merged() {
    let root = tempdir().unwrap();
    let (mut service, library_dir) = setup(root.path());
    let first_collection = service.create_collection("首选集合".to_string(), None).unwrap();
    let second_collection = service.create_collection("次选集合".to_string(), None).unwrap();
    let important = service.create_tag("重要".to_string()).unwrap();
    let todo = service.create_tag("待办".to_string()).unwrap();

    service
        .apply_classification_rule_operation(ClassificationRuleOperation::Create {
            rule: rule(
                "第一条",
                "报告",
                Some(first_collection.id.clone()),
                vec![important.id.clone(), todo.id.clone(), important.id.clone()],
            ),
        })
        .unwrap();
    service
        .apply_classification_rule_operation(ClassificationRuleOperation::Create {
            rule: rule(
                "第二条",
                "报告",
                Some(second_collection.id.clone()),
                vec![todo.id.clone()],
            ),
        })
        .unwrap();

    let source = root.path().join("季度报告.txt");
    fs::write(&source, "季度报告正文").unwrap();
    service
        .start_import(vec![source.to_string_lossy().into_owned()])
        .unwrap();

    let documents = service.list_documents().unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(
        documents[0].collection_id, first_collection.id,
        "按顺序取第一条指定集合的命中规则"
    );
    let tag_ids = documents[0]
        .tags
        .iter()
        .map(|tag| tag.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(tag_ids.len(), 2, "重复标签只添加一次：{tag_ids:?}");
    assert!(tag_ids.contains(&important.id));
    assert!(tag_ids.contains(&todo.id));
    assert!(library_dir.join("documents").is_dir());
}

#[test]
fn unmatched_documents_go_to_inbox_and_explicit_target_keeps_rule_tags() {
    let root = tempdir().unwrap();
    let (mut service, _library_dir) = setup(root.path());
    let archive = service.create_collection("归档".to_string(), None).unwrap();
    let explicit = service.create_collection("指定".to_string(), None).unwrap();
    let tag = service.create_tag("重要".to_string()).unwrap();
    service
        .apply_classification_rule_operation(ClassificationRuleOperation::Create {
            rule: rule("报告", "报告", Some(archive.id.clone()), vec![tag.id.clone()]),
        })
        .unwrap();

    let unmatched = root.path().join("随手记.txt");
    fs::write(&unmatched, "随手正文").unwrap();
    service
        .start_import(vec![unmatched.to_string_lossy().into_owned()])
        .unwrap();
    let documents = service.list_documents().unwrap();
    assert_eq!(documents[0].collection_id, inbox_id(&service));
    assert!(documents[0].tags.is_empty());

    // 显式目标集合优先于规则集合，但命中规则的标签仍然生效。
    let matched = root.path().join("月度报告.txt");
    fs::write(&matched, "报告正文").unwrap();
    service
        .start_import_to_collection(
            vec![matched.to_string_lossy().into_owned()],
            Some(explicit.id.clone()),
            ImportSource::FilePicker,
        )
        .unwrap();
    let document = service
        .list_documents()
        .unwrap()
        .into_iter()
        .find(|document| document.file_name == "月度报告.txt")
        .unwrap();
    assert_eq!(document.collection_id, explicit.id);
    assert_eq!(document.tags.len(), 1);
    assert_eq!(document.tags[0].id, tag.id);
}

#[test]
fn preview_and_import_share_the_same_classification() {
    let root = tempdir().unwrap();
    let (mut service, _library_dir) = setup(root.path());
    let collection = service.create_collection("归档".to_string(), None).unwrap();
    let tag = service.create_tag("重要".to_string()).unwrap();
    service
        .apply_classification_rule_operation(ClassificationRuleOperation::Create {
            rule: rule("报告", "报告", Some(collection.id.clone()), vec![tag.id.clone()]),
        })
        .unwrap();

    let source = root.path().join("年度报告.txt");
    fs::write(&source, "年度报告正文").unwrap();
    let preview = service
        .preview_classification(ClassificationPreviewRequest {
            paths: vec![source.to_string_lossy().into_owned()],
            target_collection_id: None,
        })
        .unwrap();
    assert_eq!(preview.items.len(), 1);
    assert_eq!(preview.items[0].collection_id, collection.id);
    assert_eq!(preview.items[0].tag_ids, vec![tag.id.clone()]);
    assert_eq!(preview.items[0].file_type.as_deref(), Some("TXT"));

    service
        .start_import_to_collection(
            vec![source.to_string_lossy().into_owned()],
            None,
            ImportSource::FilePicker,
        )
        .unwrap();
    let document = service.list_documents().unwrap().remove(0);
    assert_eq!(document.collection_id, preview.items[0].collection_id);
    assert_eq!(
        document.tags.iter().map(|tag| tag.id.clone()).collect::<Vec<_>>(),
        preview.items[0].tag_ids
    );

    // 选择不应用时保留原有导入行为。
    let plain = root.path().join("另一份报告.txt");
    fs::write(&plain, "另一份正文").unwrap();
    let first = service
        .begin_import_batch_with_classification(
            vec![plain.to_string_lossy().into_owned()],
            None,
            ImportSource::FilePicker,
            false,
        )
        .unwrap();
    let batch_id = first.batch_id.clone();
    while service.peek_import_progress(&batch_id).unwrap().is_some() {
        service.import_batch_step(&batch_id).unwrap();
    }
    service.finish_import_batch(&batch_id).unwrap();
    let kept = service
        .list_documents()
        .unwrap()
        .into_iter()
        .find(|document| document.file_name == "另一份报告.txt")
        .unwrap();
    assert_eq!(kept.collection_id, inbox_id(&service));
    assert!(kept.tags.is_empty());
}

#[test]
fn replacing_a_library_copy_keeps_collection_and_tags() {
    let root = tempdir().unwrap();
    let (mut service, _library_dir) = setup(root.path());
    let collection = service.create_collection("归档".to_string(), None).unwrap();
    let tag = service.create_tag("重要".to_string()).unwrap();
    service
        .apply_classification_rule_operation(ClassificationRuleOperation::Create {
            rule: rule("报告", "报告", Some(collection.id.clone()), vec![tag.id.clone()]),
        })
        .unwrap();

    let source = root.path().join("报告.txt");
    fs::write(&source, "第一版正文").unwrap();
    service
        .start_import(vec![source.to_string_lossy().into_owned()])
        .unwrap();
    let document = service.list_documents().unwrap().remove(0);

    service
        .update_document_metadata(
            &document.id,
            personal_document_manager_lib::library::DocumentMetadataUpdate {
                title: "手工标题".to_string(),
                description: Some("手工描述".to_string()),
                document_date: None,
                collection_id: collection.id.clone(),
                tag_ids: vec![tag.id.clone()],
            },
        )
        .unwrap();

    fs::write(&source, "第二版正文").unwrap();
    let changed = service
        .start_import(vec![source.to_string_lossy().into_owned()])
        .unwrap()
        .items
        .remove(0);
    assert_eq!(
        changed.status,
        personal_document_manager_lib::library::ImportItemStatus::SourceChanged
    );
    service
        .resolve_import_item(
            &changed.item_id,
            personal_document_manager_lib::library::ImportDecision::ReplaceExisting,
        )
        .unwrap();

    let replaced = service
        .list_documents()
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.id == document.id)
        .unwrap();
    assert_eq!(replaced.title, "手工标题");
    assert_eq!(replaced.collection_id, collection.id);
    assert_eq!(replaced.tags.len(), 1);
    assert_eq!(replaced.tags[0].id, tag.id);
}

#[test]
fn rule_operations_reject_unknown_collections_and_tags() {
    let root = tempdir().unwrap();
    let (mut service, _library_dir) = setup(root.path());
    let error = service
        .apply_classification_rule_operation(ClassificationRuleOperation::Create {
            rule: rule("坏规则", "报告", Some("missing-collection".to_string()), Vec::new()),
        })
        .unwrap_err();
    assert!(error.to_string().contains("集合"));

    let error = service
        .apply_classification_rule_operation(ClassificationRuleOperation::Create {
            rule: rule("坏标签", "报告", None, vec!["missing-tag".to_string()]),
        })
        .unwrap_err();
    assert!(error.to_string().contains("标签"));

    let error = service
        .apply_classification_rule_operation(ClassificationRuleOperation::Create {
            rule: rule("空规则", "", None, Vec::new()),
        })
        .unwrap_err();
    assert!(error.to_string().contains("匹配条件"));
    assert!(service.list_classification_rules().unwrap().is_empty());
}

// ---------------- 工作单 11/12：接收目录 ----------------

#[test]
fn receive_sources_are_scoped_to_their_library() {
    let root = tempdir().unwrap();
    let mut service = LibraryService::new(root.path().join("app-state")).unwrap();
    let first = root.path().join("First");
    let second = root.path().join("Second");
    service.create_library(&first).unwrap();
    service.create_library(&second).unwrap();
    let first_dir = receive_dir(root.path(), "qq-first");

    service.open_library(&first).unwrap();
    service
        .upsert_receive_source(
            None,
            ReceiveSourceInput {
                kind: ReceiveSourceKind::Qq,
                display_name: "QQ".to_string(),
                path: first_dir.to_string_lossy().into_owned(),
                enabled: true,
            },
        )
        .unwrap();
    assert_eq!(service.list_receive_sources().unwrap().len(), 1);

    service.open_library(&second).unwrap();
    assert!(service.list_receive_sources().unwrap().is_empty());

    // 目录失效与恢复都以当前资料库为准。
    fs::remove_dir(&first_dir).unwrap();
    service.open_library(&first).unwrap();
    let sources = service.list_receive_sources().unwrap();
    assert_eq!(sources[0].status, ReceiveSourceStatus::Missing);
    assert!(sources[0]
        .status_message
        .as_deref()
        .unwrap_or_default()
        .contains("重新确认"));
    fs::create_dir_all(&first_dir).unwrap();
    let sources = service.list_receive_sources().unwrap();
    assert_eq!(sources[0].status, ReceiveSourceStatus::Ready);
}

#[test]
fn library_directory_cannot_be_a_receive_source() {
    let root = tempdir().unwrap();
    let (mut service, library_dir) = setup(root.path());
    let error = service
        .upsert_receive_source(
            None,
            ReceiveSourceInput {
                kind: ReceiveSourceKind::Other,
                display_name: "自身".to_string(),
                path: library_dir.to_string_lossy().into_owned(),
                enabled: true,
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("资料库自身"));

    let inside = library_dir.join("documents");
    let error = service
        .upsert_receive_source(
            None,
            ReceiveSourceInput {
                kind: ReceiveSourceKind::Other,
                display_name: "内部".to_string(),
                path: inside.to_string_lossy().into_owned(),
                enabled: true,
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("资料库自身"));
}

#[test]
fn unselected_existing_files_are_not_imported_and_can_be_selected_later() {
    let root = tempdir().unwrap();
    let (mut service, _library_dir) = setup(root.path());
    let dir = receive_dir(root.path(), "wechat");
    let keep = dir.join("保留.txt");
    let skipped = dir.join("未选.txt");
    fs::write(&keep, "保留正文").unwrap();
    fs::write(&skipped, "未选正文").unwrap();

    let listing = {
        let sources = service
            .upsert_receive_source(
                None,
                ReceiveSourceInput {
                    kind: ReceiveSourceKind::Wechat,
                    display_name: "微信".to_string(),
                    path: dir.to_string_lossy().into_owned(),
                    enabled: true,
                },
            )
            .unwrap();
        service
            .list_receive_directory_files(&sources[0].id)
            .unwrap()
    };
    assert_eq!(listing.items.len(), 2);
    assert!(listing.items.iter().all(|item| !item.previously_skipped));

    let all = supported_files(&dir);
    let all_refs = all.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    enable_source(
        &mut service,
        ReceiveSourceKind::Wechat,
        &dir,
        &[keep.as_path()],
        &all_refs,
    );

    // 未选文件不会在补扫时自动导入；已选文件已入库。
    let results = scan_and_import(&mut service);
    assert!(results.is_empty(), "补扫不应再有待导入文件：{results:?}");
    let documents = service.list_documents().unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].file_name, "保留.txt");

    // 重新打开清单：未选文件仍可补选。
    let source_id = service.list_receive_sources().unwrap()[0].id.clone();
    let listing = service.list_receive_directory_files(&source_id).unwrap();
    let previous = listing
        .items
        .iter()
        .find(|item| item.path.ends_with("未选.txt"))
        .unwrap();
    assert!(previous.previously_skipped);
    let first = service
        .begin_receive_import_batch(
            &source_id,
            vec![skipped.to_string_lossy().into_owned()],
        )
        .unwrap();
    let batch_id = first.batch_id.clone();
    service.import_batch_step(&batch_id).unwrap();
    service.finish_receive_import_batch(&batch_id).unwrap();
    assert_eq!(service.list_documents().unwrap().len(), 2);
}

#[test]
fn startup_scan_imports_new_files_and_repeated_scans_do_not_duplicate() {
    let root = tempdir().unwrap();
    let (mut service, _library_dir) = setup(root.path());
    let dir = receive_dir(root.path(), "qq");
    let source_file = dir.join("第一份.txt");
    fs::write(&source_file, "第一份正文").unwrap();
    enable_source(
        &mut service,
        ReceiveSourceKind::Qq,
        &dir,
        &[source_file.as_path()],
        &[source_file.as_path()],
    );
    assert_eq!(service.list_documents().unwrap().len(), 1);

    // 关闭应用期间新增文件：重新打开资料库后补扫导入。
    drop(service);
    let mut service = LibraryService::new(root.path().join("app-state")).unwrap();
    service.open_library(&root.path().join("Library")).unwrap();
    let second = dir.join("第二份.txt");
    fs::write(&second, "第二份正文").unwrap();
    let results = scan_and_import(&mut service);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].imported_count, 1);
    assert_eq!(service.list_documents().unwrap().len(), 2);

    // 重复补扫不会产生重复文档，也不会再次写日志。
    let results = scan_and_import(&mut service);
    assert!(results.is_empty(), "重复补扫不应再有待处理项：{results:?}");
    assert_eq!(service.list_documents().unwrap().len(), 2);
    let log = service.list_receive_import_log(100).unwrap();
    assert_eq!(log.len(), 2, "每个源文件只应有一条終态日志：{log:?}");
}

#[test]
fn source_change_keeps_a_pending_entry_while_duplicates_are_skipped() {
    let root = tempdir().unwrap();
    let (mut service, _library_dir) = setup(root.path());
    let dir = receive_dir(root.path(), "wechat");
    let source_file = dir.join("变化.txt");
    fs::write(&source_file, "第一版正文").unwrap();
    let source_id = enable_source(
        &mut service,
        ReceiveSourceKind::Wechat,
        &dir,
        &[source_file.as_path()],
        &[source_file.as_path()],
    );

    // 同一内容重复出现（未变化）时不产生新文档、不重复导入。
    fs::write(&source_file, "第一版正文").unwrap();
    let results = scan_and_import(&mut service);
    assert!(results.is_empty());
    assert_eq!(service.list_documents().unwrap().len(), 1);

    // 同一来源内容变化：保留待决项，不静默覆盖资料库副本。
    fs::write(&source_file, "第二版正文").unwrap();
    let results = scan_and_import(&mut service);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].pending_count, 1);
    assert_eq!(results[0].imported_count, 0);
    assert_eq!(service.list_documents().unwrap().len(), 1);

    let log = service.list_receive_import_log(10).unwrap();
    assert!(
        log.iter()
            .any(|entry| entry.status
                == personal_document_manager_lib::library::ImportItemStatus::SourceChanged
            && entry.source_path.ends_with("变化.txt")),
        "内容变化应留下待决日志：{log:?}"
    );
    let sources = service.list_receive_sources().unwrap();
    assert_eq!(
        sources.iter().find(|source| source.id == source_id).unwrap().pending_count,
        1
    );

    // 待决项不会在重复补扫里反复导入。
    let results = scan_and_import(&mut service);
    assert!(results.is_empty());
}

#[test]
fn overlapping_sources_do_not_create_duplicate_documents() {
    let root = tempdir().unwrap();
    let (mut service, _library_dir) = setup(root.path());
    let qq_dir = receive_dir(root.path(), "qq");
    let wechat_dir = receive_dir(root.path(), "wechat");
    let qq_file = qq_dir.join("同一份.txt");
    let wechat_file = wechat_dir.join("副本.txt");
    fs::write(&qq_file, "重叠内容").unwrap();
    fs::write(&wechat_file, "重叠内容").unwrap();

    enable_source(
        &mut service,
        ReceiveSourceKind::Qq,
        &qq_dir,
        &[qq_file.as_path()],
        &[qq_file.as_path()],
    );
    let results = scan_and_import(&mut service);
    assert!(results.is_empty());

    // 微信目录里的同内容文件按现有重复语义跳过，不产生第二份文档。
    let sources = service
        .upsert_receive_source(
            None,
            ReceiveSourceInput {
                kind: ReceiveSourceKind::Wechat,
                display_name: "微信".to_string(),
                path: wechat_dir.to_string_lossy().into_owned(),
                enabled: true,
            },
        )
        .unwrap();
    let wechat_source = sources
        .iter()
        .find(|source| source.kind == ReceiveSourceKind::Wechat)
        .unwrap()
        .id
        .clone();
    let first = service
        .begin_receive_import_batch(
            &wechat_source,
            vec![wechat_file.to_string_lossy().into_owned()],
        )
        .unwrap();
    let batch_id = first.batch_id.clone();
    service.import_batch_step(&batch_id).unwrap();
    let result = service.finish_receive_import_batch(&batch_id).unwrap();
    assert_eq!(result.imported_count, 0);
    assert_eq!(result.skipped_count, 1);
    assert_eq!(service.list_documents().unwrap().len(), 1);
    assert_eq!(fs::read(&wechat_file).unwrap(), "重叠内容".as_bytes());
}

#[test]
fn temporary_and_unsupported_files_are_ignored_and_renamed_files_are_imported() {
    let root = tempdir().unwrap();
    let (mut service, _library_dir) = setup(root.path());
    let dir = receive_dir(root.path(), "qq");
    fs::write(dir.join("下载中.txt.part"), "半成品").unwrap();
    fs::write(dir.join(".隐藏.txt"), "隐藏文件").unwrap();
    fs::write(dir.join("聊天记录.db"), "聊天数据库").unwrap();
    fs::write(dir.join("不支持的.xyz"), "不支持").unwrap();
    let renamed = dir.join("完成后改名.txt");
    fs::write(dir.join("未完成.txt.tmp"), "写入中").unwrap();

    let source_id = enable_source(&mut service, ReceiveSourceKind::Qq, &dir, &[], &[]);
    let listing = service.list_receive_directory_files(&source_id).unwrap();
    assert!(
        listing
            .items
            .iter()
            .all(|item| !item.path.contains(".part") && !item.path.contains(".db")),
        "临时文件与聊天数据库不应出现在清单里：{:?}",
        listing.items
    );

    // 临时名改成正式名后才导入。
    fs::rename(dir.join("未完成.txt.tmp"), &renamed).unwrap();
    let results = scan_and_import(&mut service);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].imported_count, 1);
    let documents = service.list_documents().unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].file_name, "完成后改名.txt");
}

#[test]
fn receive_import_applies_rules_and_logs_the_result() {
    let root = tempdir().unwrap();
    let (mut service, _library_dir) = setup(root.path());
    let collection = service.create_collection("接收归档".to_string(), None).unwrap();
    let tag = service.create_tag("自动分类".to_string()).unwrap();
    service
        .apply_classification_rule_operation(ClassificationRuleOperation::Create {
            rule: rule("报告", "报告", Some(collection.id.clone()), vec![tag.id.clone()]),
        })
        .unwrap();

    let dir = receive_dir(root.path(), "wechat");
    let report = dir.join("新报告.txt");
    let other = dir.join("随手记.txt");
    fs::write(&report, "报告正文").unwrap();
    fs::write(&other, "随手正文").unwrap();

    let source_id = {
        let sources = service
            .upsert_receive_source(
                None,
                ReceiveSourceInput {
                    kind: ReceiveSourceKind::Wechat,
                    display_name: "微信".to_string(),
                    path: dir.to_string_lossy().into_owned(),
                    enabled: true,
                },
            )
            .unwrap();
        let id = sources[0].id.clone();
        // 首次确认前补扫不会自动导入既有文件。
        assert!(
            scan_and_import(&mut service).is_empty(),
            "首次确认前不应自动导入既有文件"
        );
        // 首次清单：全部导入。
        let listing = service.list_receive_directory_files(&id).unwrap();
        let all = listing
            .items
            .iter()
            .map(|item| item.path.clone())
            .collect::<Vec<_>>();
        let first = service.begin_receive_import_batch(&id, all).unwrap();
        let batch_id = first.batch_id.clone();
        while service.peek_import_progress(&batch_id).unwrap().is_some() {
            service.import_batch_step(&batch_id).unwrap();
        }
        let result = service.finish_receive_import_batch(&batch_id).unwrap();
        assert_eq!(result.imported_count, 2);
        id
    };

    let documents = service.list_documents().unwrap();
    let report_document = documents
        .iter()
        .find(|document| document.file_name == "新报告.txt")
        .unwrap();
    assert_eq!(report_document.collection_id, collection.id);
    assert_eq!(report_document.tags.len(), 1);
    assert_eq!(report_document.tags[0].id, tag.id);
    let other_document = documents
        .iter()
        .find(|document| document.file_name == "随手记.txt")
        .unwrap();
    assert_eq!(other_document.collection_id, inbox_id(&service));
    assert!(other_document.tags.is_empty());

    let log = service.list_receive_import_log(10).unwrap();
    assert_eq!(log.len(), 2);
    let report_log = log
        .iter()
        .find(|entry| entry.file_name == "新报告.txt")
        .unwrap();
    assert_eq!(report_log.source_id, source_id);
    assert_eq!(report_log.collection_id.as_deref(), Some(collection.id.as_str()));
    assert_eq!(report_log.tag_ids, vec![tag.id.clone()]);
    assert_eq!(report_log.matched_rule_ids.len(), 1);
    assert!(report_log.document_id.is_some());
}

#[test]
fn failed_items_do_not_block_other_receive_files() {
    let root = tempdir().unwrap();
    let (mut service, _library_dir) = setup(root.path());
    let dir = receive_dir(root.path(), "qq");
    let healthy = dir.join("可用.txt");
    let broken = dir.join("损坏.txt");
    fs::write(&healthy, "可用正文").unwrap();
    fs::write(&broken, "损坏正文").unwrap();

    let sources = service
        .upsert_receive_source(
            None,
            ReceiveSourceInput {
                kind: ReceiveSourceKind::Qq,
                display_name: "QQ".to_string(),
                path: dir.to_string_lossy().into_owned(),
                enabled: true,
            },
        )
        .unwrap();
    let source_id = sources[0].id.clone();
    // 一个不存在的路径（模拟文件在清单与导入之间被移走）不应阻断其他文件。
    let first = service
        .begin_receive_import_batch(
            &source_id,
            vec![
                dir.join("已消失.txt").to_string_lossy().into_owned(),
                healthy.to_string_lossy().into_owned(),
            ],
        )
        .unwrap();
    let batch_id = first.batch_id.clone();
    while service.peek_import_progress(&batch_id).unwrap().is_some() {
        service.import_batch_step(&batch_id).unwrap();
    }
    let result = service.finish_receive_import_batch(&batch_id).unwrap();
    assert_eq!(result.failed_count, 1);
    assert_eq!(result.imported_count, 1);
    let documents = service.list_documents().unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].file_name, "可用.txt");
    assert!(fs::read(&healthy).unwrap() == "可用正文".as_bytes());
}

#[test]
fn receive_imports_do_not_modify_or_remove_source_files() {
    let root = tempdir().unwrap();
    let (mut service, _library_dir) = setup(root.path());
    let dir = receive_dir(root.path(), "wechat");
    let source_file = dir.join("原件.txt");
    fs::write(&source_file, "原件正文").unwrap();
    let source_id = enable_source(
        &mut service,
        ReceiveSourceKind::Wechat,
        &dir,
        &[source_file.as_path()],
        &[source_file.as_path()],
    );
    assert_eq!(fs::read(&source_file).unwrap(), "原件正文".as_bytes());
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);

    // 源文件后续消失不影响资料库副本。
    fs::remove_file(&source_file).unwrap();
    let documents = service.list_documents().unwrap();
    assert_eq!(documents.len(), 1);
    let preview = service
        .get_document_preview(&documents[0].id, None)
        .unwrap();
    assert!(matches!(
        preview,
        personal_document_manager_lib::library::DocumentPreview::Text { .. }
    ));
    let _ = source_id;
}

#[test]
fn candidate_directories_are_optional_hints_only() {
    let root = tempdir().unwrap();
    let (service, _library_dir) = setup(root.path());
    let candidates = service
        .list_receive_source_candidates(ReceiveSourceKind::Other)
        .unwrap();
    assert!(candidates.candidates.is_empty(), "其他来源没有候选目录");
    assert_eq!(candidates.kind, ReceiveSourceKind::Other);

    let candidates = service
        .list_receive_source_candidates(ReceiveSourceKind::Wechat)
        .unwrap();
    assert_eq!(candidates.kind, ReceiveSourceKind::Wechat);
    for candidate in &candidates.candidates {
        assert!(!candidate.evidence.is_empty(), "候选目录必须带识别依据");
        assert!(Path::new(&candidate.path).is_dir());
    }
}

#[test]
fn rules_can_match_by_source_directory() {
    let root = tempdir().unwrap();
    let (mut service, _library_dir) = setup(root.path());
    let qq_dir = receive_dir(root.path(), "qq");
    let elsewhere = receive_dir(root.path(), "elsewhere");
    let collection = service.create_collection("来自接收目录".to_string(), None).unwrap();

    let mut input = rule("接收来源", "", Some(collection.id.clone()), Vec::new());
    input.source_directory = Some(qq_dir.to_string_lossy().into_owned());
    service
        .apply_classification_rule_operation(ClassificationRuleOperation::Create { rule: input })
        .unwrap();

    let inside = qq_dir.join("目录内.txt");
    let outside = elsewhere.join("目录外.txt");
    fs::write(&inside, "目录内正文").unwrap();
    fs::write(&outside, "目录外正文").unwrap();

    let preview = service
        .preview_classification(ClassificationPreviewRequest {
            paths: vec![
                inside.to_string_lossy().into_owned(),
                outside.to_string_lossy().into_owned(),
            ],
            target_collection_id: None,
        })
        .unwrap();
    assert_eq!(preview.items[0].collection_id, collection.id);
    assert_eq!(preview.items.len(), 2);
    assert_eq!(preview.items[1].collection_id, inbox_id(&service));
    assert!(preview.items[1].matched_rule_ids.is_empty());

    // 前缀相近但不属于该目录的路径不应被误判。
    let sibling = PathBuf::from(format!("{}-副本", qq_dir.to_string_lossy()));
    fs::create_dir_all(&sibling).unwrap();
    let sibling_file = sibling.join("同前缀.txt");
    fs::write(&sibling_file, "同前缀正文").unwrap();
    let preview = service
        .preview_classification(ClassificationPreviewRequest {
            paths: vec![sibling_file.to_string_lossy().into_owned()],
            target_collection_id: None,
        })
        .unwrap();
    assert_eq!(preview.items[0].collection_id, inbox_id(&service));
}

