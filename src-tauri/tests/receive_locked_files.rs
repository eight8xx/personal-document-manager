//! Windows 共享冲突（来源文件被其它进程占用）的真实样本 —— 工作单 11 的「文件占用」验收。
//!
//! 用真实的 Windows 文件句柄构造占用：以 `share_mode(0)`（不共享任何访问）打开源文件，
//! 其它打开请求都会拿到 `ERROR_SHARING_VIOLATION`（os error 32），这正是 QQ/微信接收目录里
//! 「文件还在被写入/被杀软扫描」的最常见形态。整个文件只在 Windows 上编译运行。
#![cfg(windows)]

use std::fs;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use personal_document_manager_lib::library::{
    DocumentPreview, ImportItemStatus, LibraryService, ReceiveSourceInput, ReceiveSourceKind,
    ReceiveSourceScanResult,
};
use tempfile::tempdir;

/// `CreateFileW` 的 `FILE_SHARE_READ`：允许别人读，但拒绝写/删除。
const FILE_SHARE_READ: u32 = 0x0000_0001;

fn setup(root: &Path) -> (LibraryService, PathBuf) {
    let mut service = LibraryService::new(root.join("app-state")).unwrap();
    let library_dir = root.join("Library");
    service.create_library(&library_dir).unwrap();
    (service, library_dir)
}

/// 独占打开：任何其它句柄（包括只读打开）都会被共享冲突拒绝。
fn lock_exclusive(path: &Path) -> fs::File {
    fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .unwrap()
}

/// 只共享读：其它进程仍可只读打开，只有写/删除会被拒绝。
fn lock_read_shared(path: &Path) -> fs::File {
    fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
        .unwrap()
}

/// 一条来源的一次补扫/导入：逐文件分步执行，与命令层、监视线程同一套原语。
fn import_paths(
    service: &mut LibraryService,
    source_id: &str,
    paths: Vec<String>,
) -> ReceiveSourceScanResult {
    let first = service
        .begin_receive_import_batch(source_id, paths)
        .unwrap();
    let batch_id = first.batch_id.clone();
    while service.peek_import_progress(&batch_id).unwrap().is_some() {
        service.import_batch_step(&batch_id).unwrap();
    }
    service.finish_receive_import_batch(&batch_id).unwrap()
}

/// 用户主动扫描（与命令层一致：立即重试失败项）。
fn scan_and_import(service: &mut LibraryService, retry_failed: bool) -> Vec<ReceiveSourceScanResult> {
    let sources = service.list_receive_sources().unwrap();
    let mut results = Vec::new();
    for source in sources {
        if !source.enabled || source.path.is_none() || source.last_scanned_at.is_none() {
            continue;
        }
        let pending = service
            .receive_pending_files(&source.id, retry_failed)
            .unwrap();
        if pending.is_empty() {
            continue;
        }
        results.push(import_paths(service, &source.id, pending));
    }
    results
}

fn confirm_source(service: &mut LibraryService, dir: &Path) -> String {
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
    let source_id = sources[0].id.clone();
    // 标记为已完成首次确认（本次不导入既有文件，随后按显式路径导入）。
    import_paths(service, &source_id, Vec::new());
    source_id
}

/// 统计资料库副本目录：返回（文档目录数、残留的临时/备份文件数）。
fn copy_layout(library_dir: &Path) -> (usize, usize) {
    let documents = library_dir.join("documents");
    let mut directories = 0;
    let mut leftovers = 0;
    let Ok(entries) = fs::read_dir(&documents) else {
        return (0, 0);
    };
    for entry in entries.filter_map(Result::ok) {
        if !entry.path().is_dir() {
            continue;
        }
        directories += 1;
        for inner in fs::read_dir(entry.path()).unwrap().filter_map(Result::ok) {
            let name = inner.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                leftovers += 1;
            }
        }
    }
    (directories, leftovers)
}

#[test]
fn locked_source_file_fails_alone_then_succeeds_after_release() {
    let root = tempdir().unwrap();
    let (mut service, library_dir) = setup(root.path());
    let receive_dir = root.path().join("wechat");
    fs::create_dir_all(&receive_dir).unwrap();
    let locked = receive_dir.join("被占用.txt");
    let healthy = receive_dir.join("正常.txt");
    fs::write(&locked, "被占用的正文").unwrap();
    fs::write(&healthy, "正常正文").unwrap();
    let source_id = confirm_source(&mut service, &receive_dir);

    // 持有独占句柄：源文件此时被「其它进程」占用。
    let guard = lock_exclusive(&locked);
    let result = import_paths(
        &mut service,
        &source_id,
        vec![
            locked.to_string_lossy().into_owned(),
            healthy.to_string_lossy().into_owned(),
        ],
    );

    // 单项失败、整批不失败，另一项照常导入。
    assert_eq!(result.failed_count, 1, "被占用的文件应单项失败：{result:?}");
    assert_eq!(result.imported_count, 1, "同批其他文件应继续导入：{result:?}");
    let documents = service.list_documents().unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].file_name, "正常.txt");

    // 可理解的原因：日志里带 Windows 共享冲突的原始错误码。
    let log = service.list_receive_import_log(10).unwrap();
    let failed = log
        .iter()
        .find(|entry| entry.file_name == "被占用.txt")
        .expect("被占用的文件应留下失败日志");
    assert_eq!(failed.status, ImportItemStatus::Failed);
    assert!(failed.document_id.is_none(), "失败项不应产生文档记录");
    let message = failed.error_message.clone().unwrap_or_default();
    assert!(
        message.contains("os error 32") || message.contains("占用") || message.contains("使用"),
        "失败原因应说明文件被占用：{message}"
    );

    // 不产生半成品副本：只有正常文件那一个目录，且没有临时/备份残留。
    let (directories, leftovers) = copy_layout(&library_dir);
    assert_eq!(directories, 1, "不应为被占用文件创建副本目录");
    assert_eq!(leftovers, 0, "不应残留 .importing/.previous 临时文件");
    // 句柄还在测试自己手里，读不了内容，只能核对大小没变。
    assert_eq!(fs::metadata(&locked).unwrap().len(), "被占用的正文".len() as u64);

    // 周期补扫在冷却时间内不会反复重试失败项。
    assert!(
        scan_and_import(&mut service, false).is_empty(),
        "失败项在冷却时间内不应被周期补扫重试"
    );

    // 释放句柄后，用户主动扫描即可重试成功。
    drop(guard);
    let results = scan_and_import(&mut service, true);
    assert_eq!(results.len(), 1, "释放后主动扫描应重试该项：{results:?}");
    assert_eq!(results[0].imported_count, 1);
    assert_eq!(results[0].failed_count, 0);
    let documents = service.list_documents().unwrap();
    assert_eq!(documents.len(), 2);
    let locked_document = documents
        .iter()
        .find(|document| document.file_name == "被占用.txt")
        .expect("重试后应生成文档");
    assert_eq!(
        service.get_document_preview(&locked_document.id, None).unwrap(),
        DocumentPreview::Text {
            text: "被占用的正文".to_string()
        }
    );
    let (directories, leftovers) = copy_layout(&library_dir);
    assert_eq!((directories, leftovers), (2, 0));
    assert_eq!(fs::read(&locked).unwrap(), "被占用的正文".as_bytes());
}

/// 负面对照：只共享读的句柄不会挡住读取，导入正常成功。
///
/// 这条说明「用 `FILE_SHARE_READ` 打开」并不能复现共享冲突——只有独占句柄才会，
/// 所以上面那个用例必须用 `share_mode(0)` 才真实。
#[test]
fn read_shared_handle_does_not_block_import() {
    let root = tempdir().unwrap();
    let (mut service, _library_dir) = setup(root.path());
    let receive_dir = root.path().join("qq");
    fs::create_dir_all(&receive_dir).unwrap();
    let path = receive_dir.join("允许读.txt");
    fs::write(&path, "允许读的正文").unwrap();
    let source_id = confirm_source(&mut service, &receive_dir);

    let guard = lock_read_shared(&path);
    let result = import_paths(
        &mut service,
        &source_id,
        vec![path.to_string_lossy().into_owned()],
    );
    assert_eq!(result.imported_count, 1, "只共享读不应影响导入：{result:?}");
    assert_eq!(result.failed_count, 0);
    drop(guard);
    assert_eq!(service.list_documents().unwrap().len(), 1);
}
