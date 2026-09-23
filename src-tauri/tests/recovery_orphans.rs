//! L2 回归：中断在「副本已落盘、数据库未写入」之间留下的孤儿副本目录。
//!
//! 崩在 `rename(temp → documents/<id>/<name>)` 与写库之间时，资料库里会多出一个没有任何
//! 文档记录的副本目录。处理策略必须保守——**只搬不删**：
//! 陈旧的无记录目录被移进 `<资料库>/.recovered-orphans/`（内容原样保留 + 一份说明），
//! 而活动文档、回收站文档的副本，以及「太新、可能是另一个实例正在导入」的目录一律不动。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use personal_document_manager_lib::library::{
    DocumentPreview, DocumentSummary, LibraryService,
};
use tempfile::tempdir;

const ORPHAN_CONTENT: &str = "孤儿副本的正文，必须一个字节都不能少";

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

/// 建一个资料库：两份文档，其中一份在回收站里，另一份保持活动。
fn library_with_active_and_trashed(
    root: &Path,
) -> (PathBuf, PathBuf, DocumentSummary, DocumentSummary) {
    let state_dir = root.join("app-state");
    let library_dir = root.join("Library");
    let active_source = root.join("active.txt");
    let trashed_source = root.join("trashed.txt");
    fs::write(&active_source, "活动文档正文").unwrap();
    fs::write(&trashed_source, "回收站文档正文").unwrap();

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.create_library(&library_dir).unwrap();
    let active = service.import_document(&active_source).unwrap();
    let trashed = service.import_document(&trashed_source).unwrap();
    service.move_document_to_trash(&trashed.id).unwrap();
    assert_eq!(service.list_documents().unwrap().len(), 1);
    assert_eq!(service.list_trash_documents().unwrap().len(), 1);
    drop(service);
    (state_dir, library_dir, active, trashed)
}

/// 造一个「副本已落盘、数据库没有记录」的目录，并把目录内文件的 mtime 改成 `age` 之前。
fn orphan_copy_directory(library: &Path, label: &str, age: Duration) -> PathBuf {
    let directory = library.join("documents").join(label);
    fs::create_dir_all(&directory).unwrap();
    let copy = directory.join("孤儿副本.txt");
    fs::write(&copy, ORPHAN_CONTENT).unwrap();
    let modified = SystemTime::now() - age;
    fs::File::options()
        .write(true)
        .open(&copy)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    directory
}

#[test]
fn stale_orphan_copy_is_quarantined_without_losing_any_bytes() {
    let root = tempdir().unwrap();
    let (state_dir, library_dir, active, trashed) = library_with_active_and_trashed(root.path());
    let active_copy = copy_path(&library_dir, &active);
    let trashed_copy = copy_path(&library_dir, &trashed);
    let orphan = orphan_copy_directory(&library_dir, "11111111-2222-3333-4444-555555555555", Duration::from_secs(3600));

    let mut service = LibraryService::new(&state_dir).unwrap();
    service
        .open_library(&library_dir)
        .expect("孤儿副本不应阻止打开资料库");

    // 原目录已不在 documents/ 下，内容被原样搬到隔离区。
    assert!(!orphan.exists(), "孤儿目录应被移出 documents/");
    let quarantine_root = library_dir.join(".recovered-orphans");
    let moved = fs::read_dir(&quarantine_root)
        .expect("应创建隔离区")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.is_dir())
        .expect("隔离区里应有被搬走的目录");
    let moved_copy = moved.join("孤儿副本.txt");
    assert_eq!(
        fs::read(&moved_copy).unwrap(),
        ORPHAN_CONTENT.as_bytes(),
        "隔离必须保内容，不能丢字节"
    );
    let report = fs::read_to_string(moved.join("orphan-copy-report.txt")).unwrap();
    assert!(report.contains("孤儿副本"), "说明文件要写清这是什么：{report}");
    assert!(
        report.contains("11111111-2222-3333-4444-555555555555"),
        "说明文件要能定位到原始目录：{report}"
    );

    // 活动文档与回收站文档的副本一个都没被动，文档本身照常可用。
    assert_eq!(fs::read(&active_copy).unwrap(), "活动文档正文".as_bytes());
    assert_eq!(fs::read(&trashed_copy).unwrap(), "回收站文档正文".as_bytes());
    assert_eq!(service.list_documents().unwrap().len(), 1);
    assert_eq!(service.list_trash_documents().unwrap().len(), 1);
    assert_eq!(text_preview(&service, &active.id), "活动文档正文");
    drop(service);

    // 再次打开：隔离区不会被重复处理，也不会再生成新的副本目录。
    let mut reopened = LibraryService::new(&state_dir).unwrap();
    reopened.open_library(&library_dir).unwrap();
    assert_eq!(
        fs::read_dir(&quarantine_root).unwrap().count(),
        1,
        "隔离区内容应当稳定，不重复搬移"
    );
}

#[test]
fn fresh_orphan_copy_is_left_in_place_for_a_later_open() {
    let root = tempdir().unwrap();
    let (state_dir, library_dir, _active, _trashed) = library_with_active_and_trashed(root.path());
    // 刚刚出现的无记录副本：可能是另一个实例正在导入，不能搬走。
    let orphan = orphan_copy_directory(&library_dir, "99999999-8888-7777-6666-555555555555", Duration::from_secs(0));

    let mut service = LibraryService::new(&state_dir).unwrap();
    service.open_library(&library_dir).unwrap();

    assert!(orphan.is_dir(), "新鲜的孤儿目录应保持原样");
    assert_eq!(
        fs::read(orphan.join("孤儿副本.txt")).unwrap(),
        ORPHAN_CONTENT.as_bytes()
    );
    assert!(
        !library_dir.join(".recovered-orphans").exists(),
        "没有陈旧孤儿时不应创建隔离区"
    );
}
