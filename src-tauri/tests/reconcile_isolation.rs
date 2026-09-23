//! M4 回归：中断恢复对「单个文件被占用」的逐文档隔离（Windows，真实文件句柄）。
//!
//! 修复前：`documents/<id>/` 下任何一个删不掉的残留都会让 `reconcile` 返回 Err，
//! 于是 `open_library` 失败——**用户打不开自己的资料库**。这里用 `share_mode(0)`
//! （不共享任何访问）制造真实的 Windows 共享冲突，断言：
//! 残留删不掉只是那一份文档的现场保留，其余文档与整个资料库照常可用；
//! 而「记录副本缺失且候选无法采用」这类真正的恢复失败仍然要让用户看见。
#![cfg(windows)]

use std::fs;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use personal_document_manager_lib::library::{
    DocumentPreview, DocumentProcessingStatus, DocumentSummary, LibraryService,
};
use tempfile::tempdir;

fn lock_exclusive(path: &Path) -> fs::File {
    // dwShareMode = 0：其它任何打开（包括删除）都会被共享冲突拒绝。
    fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .unwrap()
}

fn copy_path(library: &Path, document: &DocumentSummary) -> PathBuf {
    library
        .join("documents")
        .join(&document.id)
        .join(&document.file_name)
}

fn document_by_id(service: &LibraryService, document_id: &str) -> DocumentSummary {
    service
        .list_documents()
        .unwrap()
        .into_iter()
        .find(|document| document.id == document_id)
        .expect("document is listed")
}

fn text_preview(service: &LibraryService, document_id: &str) -> String {
    match service.get_document_preview(document_id, None).unwrap() {
        DocumentPreview::Text { text } => text,
        other => panic!("unexpected preview: {other:?}"),
    }
}

/// 建一个含两份文档的资料库，返回（状态目录、资料库目录、两份文档）。
fn library_with_two_documents(root: &Path) -> (PathBuf, PathBuf, DocumentSummary, DocumentSummary) {
    let state_dir = root.join("app-state");
    let library_dir = root.join("Library");
    let first_source = root.join("first.txt");
    let second_source = root.join("second.txt");
    fs::write(&first_source, "第一份正文 searchable-first").unwrap();
    fs::write(&second_source, "第二份正文 searchable-second").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let first = service.import_document(&first_source).unwrap();
    let second = service.import_document(&second_source).unwrap();
    drop(service);
    (state_dir, library_dir, first, second)
}

/// 锁住残留：打开**成功**、两份文档都照常可用、残留保留到现场；释放句柄后再打开即被清理。
#[test]
fn a_locked_leftover_no_longer_blocks_opening_the_library() {
    let root = tempdir().unwrap();
    let (state_dir, library_dir, first, second) = library_with_two_documents(root.path());
    let directory = copy_path(&library_dir, &first).parent().unwrap().to_path_buf();
    let leftover = directory.join(".interrupted-copy.previous");
    fs::write(&leftover, "残留的旧副本").unwrap();

    let guard = lock_exclusive(&leftover);

    // 关键断言：资料库能打开（修复前这里是 Err(Io(Os { code: 32, .. }))）。
    let mut service = LibraryService::new(&state_dir).unwrap();
    service
        .open_library(&library_dir)
        .expect("单个残留被占用不应阻止打开资料库");
    assert_eq!(service.list_documents().unwrap().len(), 2);
    // 被占用残留的那一份文档必须保持可用（不能被误标成失败）。
    let first_after = document_by_id(&service, &first.id);
    assert_eq!(first_after.processing_status, DocumentProcessingStatus::Ready);
    assert!(first_after.error_message.is_none(), "{first_after:?}");
    assert_eq!(
        text_preview(&service, &first.id),
        "第一份正文 searchable-first"
    );
    assert_eq!(
        text_preview(&service, &second.id),
        "第二份正文 searchable-second"
    );
    // 现场保留：删不掉就先留着，下次打开再试。
    assert!(leftover.is_file(), "删不掉的残留应保留现场");
    assert!(copy_path(&library_dir, &first).is_file());
    drop(service);

    // 释放句柄后再打开：残留被清理（自愈）。
    drop(guard);
    let mut service = LibraryService::new(&state_dir).unwrap();
    service.open_library(&library_dir).unwrap();
    assert!(!leftover.exists(), "释放占用后残留应被清理");
    assert_eq!(service.list_documents().unwrap().len(), 2);
}

/// 对照：锁住**文档副本本身**不阻止打开（副本只读不删），另一份文档照常可用。
///
/// 被锁住的那一份预览会返回 `Failure`（读不了文件是诚实的），但**打开资料库**与
/// 其余文档都不受影响——这正是 M4 与「副本被占用」的区别。
#[test]
fn a_locked_document_copy_still_opens_and_stays_readable() {
    let root = tempdir().unwrap();
    let (state_dir, library_dir, first, second) = library_with_two_documents(root.path());
    let guard = lock_exclusive(&copy_path(&library_dir, &first));

    let mut service = LibraryService::new(&state_dir).unwrap();
    service
        .open_library(&library_dir)
        .expect("锁住副本本身不应阻止打开资料库");
    assert_eq!(service.list_documents().unwrap().len(), 2);
    assert_eq!(
        text_preview(&service, &second.id),
        "第二份正文 searchable-second"
    );
    match service.get_document_preview(&first.id, None).unwrap() {
        DocumentPreview::Failure { message, .. } => assert!(
            message.contains("os error 32") || message.contains("使用"),
            "被占用副本的预览要说明原因：{message}"
        ),
        other => panic!("锁住的副本不应读出内容：{other:?}"),
    }
    drop(guard);

    // 释放占用后同一份文档恢复可读。
    assert_eq!(
        text_preview(&service, &first.id),
        "第一份正文 searchable-first"
    );
}

/// 反向用例：记录副本被移走、候选又被锁住（无法核对内容）→ 打开成功，
/// 但该文档必须进入可见的恢复失败状态，另一份照常可用。
#[test]
fn an_unrecoverable_document_is_reported_while_the_library_still_opens() {
    let root = tempdir().unwrap();
    let (state_dir, library_dir, first, second) = library_with_two_documents(root.path());
    let copy = copy_path(&library_dir, &first);
    let directory = copy.parent().unwrap().to_path_buf();
    let leftover = directory.join(".interrupted-copy.previous");
    fs::write(&leftover, "第一份正文 searchable-first").unwrap();
    // 记录的副本被移走：只剩一个候选，而候选内容核对不出来（被独占锁住读不了）。
    fs::remove_file(&copy).unwrap();
    let guard = lock_exclusive(&leftover);

    let mut service = LibraryService::new(&state_dir).unwrap();
    service
        .open_library(&library_dir)
        .expect("个别文档无法恢复不应阻止打开资料库");

    let failed = document_by_id(&service, &first.id);
    assert_eq!(failed.processing_status, DocumentProcessingStatus::Failed);
    assert_eq!(failed.error_stage.as_deref(), Some("recovery"));
    let message = failed.error_message.clone().unwrap_or_default();
    assert!(!message.trim().is_empty(), "恢复失败必须给出原因");
    // 现场保留：候选还在，用户可以自行处置。
    assert!(leftover.is_file());
    assert!(!copy.exists());

    // 另一份文档完全不受影响。
    let healthy = document_by_id(&service, &second.id);
    assert_eq!(healthy.processing_status, DocumentProcessingStatus::Ready);
    assert_eq!(
        text_preview(&service, &second.id),
        "第二份正文 searchable-second"
    );
    drop(service);
    drop(guard);
}
