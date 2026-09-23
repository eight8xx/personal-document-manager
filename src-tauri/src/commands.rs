use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, MutexGuard,
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::library::{
    BatchDocumentOperationRequest, BatchDocumentOperationResult, BootstrapState,
    ClassificationPreviewRequest, ClassificationPreviewResponse, ClassificationRule,
    ClassificationRuleOperation, CollectionDeleteResult, CollectionSummary, DocumentFormatCapability,
    DocumentIndexChangedEvent, DocumentIndexPhase, DocumentMetadataUpdate, DocumentPreview,
    DocumentSearchQuery, DocumentSearchResponse, DocumentSummary, DocumentThumbnail,
    EmptyTrashResult, ExternalChangeMonitor, ImportBatch, ImportDecision, ImportItemResult,
    ImportProgress, ImportSource, IndexRunResult, IndexStatus, LibraryError, LibraryResult,
    LibraryService, LibrarySummary, ReceiveDirectoryListing, ReceiveDirectoryOperation,
    ReceiveImportLogEntry, ReceiveSource, ReceiveSourceCandidates, ReceiveSourceInput,
    ReceiveSourceKind, ReceiveSourceScanResult, RecentLibrary, TablePreviewRequest, TableSheet,
    TagSummary, TrashDocumentSummary,
};

pub struct AppState {
    service: Arc<Mutex<LibraryService>>,
    monitor: Mutex<Option<ExternalChangeMonitor>>,
    receive_monitor: Mutex<Option<ReceiveDirectoryMonitor>>,
    batch_cancellations: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl AppState {
    pub fn new(service: LibraryService) -> Self {
        Self {
            service: Arc::new(Mutex::new(service)),
            monitor: Mutex::new(None),
            receive_monitor: Mutex::new(None),
            batch_cancellations: Mutex::new(HashMap::new()),
        }
    }

    fn service(&self) -> LibraryResult<MutexGuard<'_, LibraryService>> {
        self.service.lock().map_err(|_| LibraryError::StateLock)
    }

    fn with_library<T>(
        &self,
        expected: &LibrarySummary,
        operation: impl FnOnce(&mut LibraryService) -> LibraryResult<T>,
    ) -> Result<T, CommandError> {
        let mut service = self.service()?;
        service.ensure_current_library(expected)?;
        operation(&mut service).map_err(CommandError::from)
    }

    pub(crate) fn service_handle(&self) -> Arc<Mutex<LibraryService>> {
        Arc::clone(&self.service)
    }

    pub(crate) fn set_external_change_monitor(
        &self,
        monitor: ExternalChangeMonitor,
    ) -> LibraryResult<()> {
        let mut current = self.monitor.lock().map_err(|_| LibraryError::StateLock)?;
        *current = Some(monitor);
        Ok(())
    }

    pub(crate) fn set_receive_directory_monitor(
        &self,
        monitor: ReceiveDirectoryMonitor,
    ) -> LibraryResult<()> {
        let mut current = self
            .receive_monitor
            .lock()
            .map_err(|_| LibraryError::StateLock)?;
        *current = Some(monitor);
        Ok(())
    }

    fn begin_batch(&self, job_id: &str) -> LibraryResult<Arc<AtomicBool>> {
        let mut cancellations = self
            .batch_cancellations
            .lock()
            .map_err(|_| LibraryError::StateLock)?;
        let cancellation = Arc::new(AtomicBool::new(false));
        cancellations.insert(job_id.to_string(), Arc::clone(&cancellation));
        Ok(cancellation)
    }

    fn cancel_batch(&self, job_id: &str) -> LibraryResult<bool> {
        let cancellations = self
            .batch_cancellations
            .lock()
            .map_err(|_| LibraryError::StateLock)?;
        let Some(cancellation) = cancellations.get(job_id) else {
            return Ok(false);
        };
        cancellation.store(true, Ordering::Relaxed);
        Ok(true)
    }

    fn finish_batch(&self, job_id: &str) -> LibraryResult<()> {
        self.batch_cancellations
            .lock()
            .map_err(|_| LibraryError::StateLock)?
            .remove(job_id);
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiveImportCompletedEvent {
    pub library: LibrarySummary,
    pub results: Vec<ReceiveSourceScanResult>,
}

/// 接收目录的实时导入监视。
///
/// 与外部变更监视一致：只在本进程内周期性补扫当前资料库已启用的来源，
/// 关闭应用（Drop）后线程立刻退出，不留下常驻进程；扫描过程**每个文件单独加锁**，
/// 已有文档的浏览、搜索与预览不会被整轮扫描挡住。
pub struct ReceiveDirectoryMonitor {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl ReceiveDirectoryMonitor {
    pub fn start<F>(
        service: Arc<Mutex<LibraryService>>,
        poll_interval: std::time::Duration,
        on_scan: F,
    ) -> std::io::Result<Self>
    where
        F: Fn(LibrarySummary, Vec<ReceiveSourceScanResult>) + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("pdm-receive-directory-monitor".to_string())
            .spawn(move || {
                while !thread_stop.load(Ordering::Relaxed) {
                    std::thread::park_timeout(poll_interval);
                    if thread_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let library = match service.lock() {
                        Ok(service) => service.current_library().cloned(),
                        Err(_) => break,
                    };
                    let Some(library) = library else {
                        continue;
                    };
                    match run_receive_scan(&service, &library, false) {
                        Ok(results) if !results.is_empty() => on_scan(library, results),
                        Ok(_) => {}
                        Err(_) => continue,
                    }
                }
            })?;
        Ok(Self {
            stop,
            handle: Some(handle),
        })
    }
}

impl Drop for ReceiveDirectoryMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            handle.thread().unpark();
            let _ = handle.join();
        }
    }
}

/// 按文件分步导入一批接收文件：每一步单独加锁，出错时丢弃批次状态。
pub(crate) fn drive_receive_import(
    service: &Arc<Mutex<LibraryService>>,
    library: &LibrarySummary,
    source_id: &str,
    paths: Vec<String>,
) -> LibraryResult<ReceiveSourceScanResult> {
    let first = {
        let mut service = service.lock().map_err(|_| LibraryError::StateLock)?;
        service.ensure_current_library(library)?;
        service.begin_receive_import_batch(source_id, paths)?
    };
    let batch_id = first.batch_id.clone();

    let outcome = (|| -> LibraryResult<()> {
        loop {
            let before = {
                let service = service.lock().map_err(|_| LibraryError::StateLock)?;
                service.ensure_current_library(library)?;
                service.peek_import_progress(&batch_id)?
            };
            if before.is_none() {
                break;
            }
            let mut service = service.lock().map_err(|_| LibraryError::StateLock)?;
            service.ensure_current_library(library)?;
            service.import_batch_step(&batch_id)?;
        }
        Ok(())
    })();

    if let Err(error) = outcome {
        if let Ok(mut service) = service.lock() {
            let _ = service.abort_import_batch(&batch_id);
        }
        return Err(error);
    }

    let mut service = service.lock().map_err(|_| LibraryError::StateLock)?;
    service.ensure_current_library(library)?;
    service.finish_receive_import_batch(&batch_id)
}

/// 补扫当前资料库已启用的接收来源；每一步都重新核对资料库身份。
///
/// `retry_failed` 为真（用户主动点「扫描」）时立即重试失败项；周期补扫传假，失败项走冷却时间。
fn run_receive_scan(
    service: &Arc<Mutex<LibraryService>>,
    library: &LibrarySummary,
    retry_failed: bool,
) -> LibraryResult<Vec<ReceiveSourceScanResult>> {
    let sources = {
        let service = service.lock().map_err(|_| LibraryError::StateLock)?;
        service.ensure_current_library(library)?;
        service.list_receive_sources()?
    };

    let mut results = Vec::new();
    for source in sources {
        if !source.enabled || source.path.is_none() {
            continue;
        }
        // 首次启用或刚重新定位的来源等待用户在清单里选择；补扫不会自动导入既有文件。
        if source.last_scanned_at.is_none() {
            continue;
        }
        let pending = {
            let service = service.lock().map_err(|_| LibraryError::StateLock)?;
            service.ensure_current_library(library)?;
            service.receive_pending_files(&source.id, retry_failed)?
        };
        if pending.is_empty() {
            continue;
        }
        results.push(drive_receive_import(service, library, &source.id, pending)?);
    }
    Ok(results)
}

impl Drop for AppState {
    fn drop(&mut self) {
        if let Ok(monitor) = self.monitor.get_mut() {
            monitor.take();
        }
        if let Ok(monitor) = self.receive_monitor.get_mut() {
            monitor.take();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: String,
    pub message: String,
}

impl From<LibraryError> for CommandError {
    fn from(error: LibraryError) -> Self {
        Self {
            code: error.code().to_string(),
            message: error.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryChangedEvent {
    pub action: String,
    pub library: LibrarySummary,
}

#[tauri::command]
pub fn bootstrap(state: State<'_, AppState>) -> Result<BootstrapState, CommandError> {
    bootstrap_contract(&state)
}

fn bootstrap_contract(state: &AppState) -> Result<BootstrapState, CommandError> {
    state.service()?.bootstrap().map_err(CommandError::from)
}

#[tauri::command]
pub fn inspect_library_location(
    path: String,
    state: State<'_, AppState>,
) -> Result<crate::library::LibraryLocationInspection, CommandError> {
    inspect_library_location_contract(&state, path)
}

fn inspect_library_location_contract(
    state: &AppState,
    path: String,
) -> Result<crate::library::LibraryLocationInspection, CommandError> {
    state
        .service()?
        .inspect_location(path)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn create_library(
    path: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<LibrarySummary, CommandError> {
    let library = create_library_contract(&state, path)?;
    emit_library_changed(&app, "created", library.clone());
    Ok(library)
}

fn create_library_contract(state: &AppState, path: String) -> Result<LibrarySummary, CommandError> {
    state
        .service()?
        .create_library(path)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn open_library(
    path: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<LibrarySummary, CommandError> {
    let library = open_library_contract(&state, path)?;
    emit_library_changed(&app, "opened", library.clone());
    Ok(library)
}

fn open_library_contract(state: &AppState, path: String) -> Result<LibrarySummary, CommandError> {
    state
        .service()?
        .open_library(path)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn import_document(
    library: LibrarySummary,
    path: String,
    state: State<'_, AppState>,
) -> Result<DocumentSummary, CommandError> {
    import_document_contract(&state, &library, path)
}

fn import_document_contract(
    state: &AppState,
    library: &LibrarySummary,
    path: String,
) -> Result<DocumentSummary, CommandError> {
    state.with_library(library, |service| service.import_document(path))
}

#[tauri::command]
pub async fn start_import(
    library: LibrarySummary,
    paths: Vec<String>,
    target_collection_id: Option<String>,
    source: Option<ImportSource>,
    apply_classification: Option<bool>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<ImportBatch, CommandError> {
    spawn_import_task(
        &state,
        library,
        paths,
        target_collection_id,
        source.unwrap_or(ImportSource::FilePicker),
        apply_classification.unwrap_or(true),
        move |progress| {
            let _ = app.emit("import-progress", progress);
        },
    )
    .await
    .map_err(|error| CommandError {
        code: "importTask".to_string(),
        message: format!("导入任务无法完成：{error}"),
    })?
}

#[allow(clippy::too_many_arguments)]
fn spawn_import_task<F>(
    state: &AppState,
    library: LibrarySummary,
    paths: Vec<String>,
    target_collection_id: Option<String>,
    source: ImportSource,
    apply_classification: bool,
    mut on_progress: F,
) -> tauri::async_runtime::JoinHandle<Result<ImportBatch, CommandError>>
where
    F: FnMut(ImportProgress) + Send + 'static,
{
    let service = state.service_handle();
    tauri::async_runtime::spawn_blocking(move || {
        // 整批导入不再长时间独占服务锁：开始阶段只做扫描与批次登记，
        // 之后每一项单独加锁处理，列表、搜索和预览可以在两项之间插进来。
        let outcome: Result<ImportBatch, LibraryError> = (|| {
            let first = {
                let mut service = service.lock().map_err(|_| LibraryError::StateLock)?;
                service.ensure_current_library(&library)?;
                service.begin_import_batch_with_classification(
                    paths,
                    target_collection_id,
                    source,
                    apply_classification,
                )?
            };
            let batch_id = first.batch_id.clone();
            on_progress(first);

            let batch_result = (|| -> Result<ImportBatch, LibraryError> {
                loop {
                    // 处理前事件与处理本身分两次加锁，中间是读取命令的插入点。
                    let before = {
                        let service = service.lock().map_err(|_| LibraryError::StateLock)?;
                        service.ensure_current_library(&library)?;
                        service.peek_import_progress(&batch_id)?
                    };
                    let Some(before) = before else {
                        break;
                    };
                    on_progress(before);

                    let after = {
                        let mut service = service.lock().map_err(|_| LibraryError::StateLock)?;
                        service.ensure_current_library(&library)?;
                        service.import_batch_step(&batch_id)?
                    };
                    on_progress(after);
                }

                let (finished, batch) = {
                    let mut service = service.lock().map_err(|_| LibraryError::StateLock)?;
                    service.ensure_current_library(&library)?;
                    let finished = service.import_batch_finished_progress(&batch_id)?;
                    let batch = service.finish_import_batch(&batch_id)?;
                    (finished, batch)
                };
                on_progress(finished);
                Ok(batch)
            })();

            if batch_result.is_err() {
                // 出错或资料库已切换：丢弃批次状态，不动已经提交的文档与副本。
                if let Ok(mut service) = service.lock() {
                    let _ = service.abort_import_batch(&batch_id);
                }
            }
            batch_result
        })();

        outcome.map_err(CommandError::from)
    })
}

#[cfg(test)]
fn start_import_contract<F>(
    state: &AppState,
    paths: Vec<String>,
    on_progress: F,
) -> Result<ImportBatch, CommandError>
where
    F: FnMut(ImportProgress),
{
    start_import_to_collection_contract(state, paths, None, on_progress)
}

#[cfg(test)]
fn start_import_to_collection_contract<F>(
    state: &AppState,
    paths: Vec<String>,
    target_collection_id: Option<String>,
    on_progress: F,
) -> Result<ImportBatch, CommandError>
where
    F: FnMut(ImportProgress),
{
    let mut service = state.service()?;
    service
        .start_import_to_collection_with_progress(
            paths,
            target_collection_id,
            ImportSource::FilePicker,
            on_progress,
        )
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn resolve_import_item(
    library: LibrarySummary,
    item_id: String,
    decision: ImportDecision,
    state: State<'_, AppState>,
) -> Result<ImportItemResult, CommandError> {
    resolve_import_item_contract(&state, &library, item_id, decision)
}

fn resolve_import_item_contract(
    state: &AppState,
    library: &LibrarySummary,
    item_id: String,
    decision: ImportDecision,
) -> Result<ImportItemResult, CommandError> {
    state.with_library(library, |service| {
        service.resolve_import_item(&item_id, decision)
    })
}

#[tauri::command]
pub fn retry_import_item(
    library: LibrarySummary,
    item_id: String,
    state: State<'_, AppState>,
) -> Result<ImportItemResult, CommandError> {
    retry_import_item_contract(&state, &library, item_id)
}

fn retry_import_item_contract(
    state: &AppState,
    library: &LibrarySummary,
    item_id: String,
) -> Result<ImportItemResult, CommandError> {
    state.with_library(library, |service| service.retry_import_item(&item_id))
}

#[tauri::command]
pub fn list_collections(
    state: State<'_, AppState>,
) -> Result<Vec<CollectionSummary>, CommandError> {
    list_collections_contract(&state)
}

fn list_collections_contract(state: &AppState) -> Result<Vec<CollectionSummary>, CommandError> {
    state
        .service()?
        .list_collections()
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn create_collection(
    library: LibrarySummary,
    name: String,
    parent_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<CollectionSummary, CommandError> {
    create_collection_contract(&state, &library, name, parent_id)
}

fn create_collection_contract(
    state: &AppState,
    library: &LibrarySummary,
    name: String,
    parent_id: Option<String>,
) -> Result<CollectionSummary, CommandError> {
    state.with_library(library, |service| {
        service.create_collection(name, parent_id)
    })
}

#[tauri::command]
pub fn rename_collection(
    library: LibrarySummary,
    collection_id: String,
    name: String,
    state: State<'_, AppState>,
) -> Result<CollectionSummary, CommandError> {
    rename_collection_contract(&state, &library, collection_id, name)
}

fn rename_collection_contract(
    state: &AppState,
    library: &LibrarySummary,
    collection_id: String,
    name: String,
) -> Result<CollectionSummary, CommandError> {
    state.with_library(library, |service| {
        service.rename_collection(&collection_id, name)
    })
}

#[tauri::command]
pub fn move_collection(
    library: LibrarySummary,
    collection_id: String,
    parent_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<CollectionSummary, CommandError> {
    move_collection_contract(&state, &library, collection_id, parent_id)
}

fn move_collection_contract(
    state: &AppState,
    library: &LibrarySummary,
    collection_id: String,
    parent_id: Option<String>,
) -> Result<CollectionSummary, CommandError> {
    state.with_library(library, |service| {
        service.move_collection(&collection_id, parent_id)
    })
}

#[tauri::command]
pub fn delete_collection(
    library: LibrarySummary,
    collection_id: String,
    state: State<'_, AppState>,
) -> Result<CollectionDeleteResult, CommandError> {
    delete_collection_contract(&state, &library, collection_id)
}

fn delete_collection_contract(
    state: &AppState,
    library: &LibrarySummary,
    collection_id: String,
) -> Result<CollectionDeleteResult, CommandError> {
    state.with_library(library, |service| service.delete_collection(&collection_id))
}

#[tauri::command]
pub fn move_document_to_collection(
    library: LibrarySummary,
    document_id: String,
    collection_id: String,
    state: State<'_, AppState>,
) -> Result<DocumentSummary, CommandError> {
    move_document_to_collection_contract(&state, &library, document_id, collection_id)
}

fn move_document_to_collection_contract(
    state: &AppState,
    library: &LibrarySummary,
    document_id: String,
    collection_id: String,
) -> Result<DocumentSummary, CommandError> {
    state.with_library(library, |service| {
        service.move_document_to_collection(&document_id, &collection_id)
    })
}

#[tauri::command]
pub fn move_document_to_trash(
    library: LibrarySummary,
    document_id: String,
    state: State<'_, AppState>,
) -> Result<(), CommandError> {
    move_document_to_trash_contract(&state, &library, document_id)
}

fn move_document_to_trash_contract(
    state: &AppState,
    library: &LibrarySummary,
    document_id: String,
) -> Result<(), CommandError> {
    state.with_library(library, |service| {
        service.move_document_to_trash(&document_id)
    })
}

#[tauri::command]
pub fn list_trash_documents(
    state: State<'_, AppState>,
) -> Result<Vec<TrashDocumentSummary>, CommandError> {
    list_trash_documents_contract(&state)
}

fn list_trash_documents_contract(
    state: &AppState,
) -> Result<Vec<TrashDocumentSummary>, CommandError> {
    state
        .service()?
        .list_trash_documents()
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn restore_document(
    library: LibrarySummary,
    document_id: String,
    state: State<'_, AppState>,
) -> Result<DocumentSummary, CommandError> {
    restore_document_contract(&state, &library, document_id)
}

fn restore_document_contract(
    state: &AppState,
    library: &LibrarySummary,
    document_id: String,
) -> Result<DocumentSummary, CommandError> {
    state.with_library(library, |service| service.restore_document(&document_id))
}

#[tauri::command]
pub fn permanently_delete_document(
    library: LibrarySummary,
    document_id: String,
    state: State<'_, AppState>,
) -> Result<(), CommandError> {
    permanently_delete_document_contract(&state, &library, document_id)
}

fn permanently_delete_document_contract(
    state: &AppState,
    library: &LibrarySummary,
    document_id: String,
) -> Result<(), CommandError> {
    state.with_library(library, |service| {
        service.permanently_delete_document(&document_id)
    })
}

#[tauri::command]
pub fn empty_trash(
    library: LibrarySummary,
    state: State<'_, AppState>,
) -> Result<EmptyTrashResult, CommandError> {
    empty_trash_contract(&state, &library)
}

fn empty_trash_contract(
    state: &AppState,
    library: &LibrarySummary,
) -> Result<EmptyTrashResult, CommandError> {
    state.with_library(library, |service| service.empty_trash())
}

#[tauri::command]
pub fn list_tags(state: State<'_, AppState>) -> Result<Vec<TagSummary>, CommandError> {
    list_tags_contract(&state)
}

fn list_tags_contract(state: &AppState) -> Result<Vec<TagSummary>, CommandError> {
    state.service()?.list_tags().map_err(CommandError::from)
}

#[tauri::command]
pub fn create_tag(
    library: LibrarySummary,
    name: String,
    state: State<'_, AppState>,
) -> Result<TagSummary, CommandError> {
    create_tag_contract(&state, &library, name)
}

fn create_tag_contract(
    state: &AppState,
    library: &LibrarySummary,
    name: String,
) -> Result<TagSummary, CommandError> {
    state.with_library(library, |service| service.create_tag(name))
}

#[tauri::command]
pub fn rename_tag(
    library: LibrarySummary,
    tag_id: String,
    name: String,
    state: State<'_, AppState>,
) -> Result<TagSummary, CommandError> {
    rename_tag_contract(&state, &library, tag_id, name)
}

fn rename_tag_contract(
    state: &AppState,
    library: &LibrarySummary,
    tag_id: String,
    name: String,
) -> Result<TagSummary, CommandError> {
    state.with_library(library, |service| service.rename_tag(&tag_id, name))
}

#[tauri::command]
pub fn delete_tag(
    library: LibrarySummary,
    tag_id: String,
    state: State<'_, AppState>,
) -> Result<(), CommandError> {
    delete_tag_contract(&state, &library, tag_id)
}

fn delete_tag_contract(
    state: &AppState,
    library: &LibrarySummary,
    tag_id: String,
) -> Result<(), CommandError> {
    state.with_library(library, |service| service.delete_tag(&tag_id))
}

#[tauri::command]
pub fn add_tag_to_document(
    library: LibrarySummary,
    document_id: String,
    tag_id: String,
    state: State<'_, AppState>,
) -> Result<DocumentSummary, CommandError> {
    add_tag_to_document_contract(&state, &library, document_id, tag_id)
}

fn add_tag_to_document_contract(
    state: &AppState,
    library: &LibrarySummary,
    document_id: String,
    tag_id: String,
) -> Result<DocumentSummary, CommandError> {
    state.with_library(library, |service| {
        service.add_tag_to_document(&document_id, &tag_id)
    })
}

#[tauri::command]
pub fn remove_tag_from_document(
    library: LibrarySummary,
    document_id: String,
    tag_id: String,
    state: State<'_, AppState>,
) -> Result<DocumentSummary, CommandError> {
    remove_tag_from_document_contract(&state, &library, document_id, tag_id)
}

fn remove_tag_from_document_contract(
    state: &AppState,
    library: &LibrarySummary,
    document_id: String,
    tag_id: String,
) -> Result<DocumentSummary, CommandError> {
    state.with_library(library, |service| {
        service.remove_tag_from_document(&document_id, &tag_id)
    })
}

#[tauri::command]
pub fn update_document_metadata(
    library: LibrarySummary,
    document_id: String,
    update: DocumentMetadataUpdate,
    state: State<'_, AppState>,
) -> Result<DocumentSummary, CommandError> {
    update_document_metadata_contract(&state, &library, document_id, update)
}

fn update_document_metadata_contract(
    state: &AppState,
    library: &LibrarySummary,
    document_id: String,
    update: DocumentMetadataUpdate,
) -> Result<DocumentSummary, CommandError> {
    state.with_library(library, |service| {
        service.update_document_metadata(&document_id, update)
    })
}

#[tauri::command]
pub async fn batch_organize_documents(
    library: LibrarySummary,
    request: BatchDocumentOperationRequest,
    state: State<'_, AppState>,
) -> Result<BatchDocumentOperationResult, CommandError> {
    let job_id = request.job_id.clone();
    let cancellation = state.begin_batch(&job_id).map_err(CommandError::from)?;
    let task = spawn_batch_organize_task(&state, library, request, cancellation).await;
    state.finish_batch(&job_id).map_err(CommandError::from)?;
    task.map_err(|error| CommandError {
        code: "batchTask".to_string(),
        message: format!("批量操作无法完成：{error}"),
    })?
}

fn spawn_batch_organize_task(
    state: &AppState,
    library: LibrarySummary,
    request: BatchDocumentOperationRequest,
    cancellation: Arc<AtomicBool>,
) -> tauri::async_runtime::JoinHandle<Result<BatchDocumentOperationResult, CommandError>> {
    let service = state.service_handle();
    tauri::async_runtime::spawn_blocking(move || {
        let mut service = service.lock().map_err(|_| LibraryError::StateLock)?;
        service.ensure_current_library(&library)?;
        service
            .batch_organize_documents(request, || cancellation.load(Ordering::Relaxed))
            .map_err(CommandError::from)
    })
}

#[tauri::command]
pub fn cancel_batch_document_operation(
    job_id: String,
    state: State<'_, AppState>,
) -> Result<bool, CommandError> {
    state.cancel_batch(&job_id).map_err(CommandError::from)
}

#[tauri::command]
pub async fn list_documents(
    state: State<'_, AppState>,
) -> Result<Vec<DocumentSummary>, CommandError> {
    // 同步命令跑在主线程上：等待服务锁会连带冻住界面，因此与搜索一样走后台线程。
    let service = state.service_handle();
    tauri::async_runtime::spawn_blocking(move || {
        service
            .lock()
            .map_err(|_| LibraryError::StateLock)?
            .list_documents()
            .map_err(CommandError::from)
    })
    .await
    .map_err(|error| CommandError {
        code: "listTask".to_string(),
        message: format!("读取文档列表无法完成：{error}"),
    })?
}

#[cfg(test)]
fn list_documents_contract(state: &AppState) -> Result<Vec<DocumentSummary>, CommandError> {
    state
        .service()?
        .list_documents()
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn search_documents(
    request: DocumentSearchQuery,
    state: State<'_, AppState>,
) -> Result<DocumentSearchResponse, CommandError> {
    let service = state.service_handle();
    tauri::async_runtime::spawn_blocking(move || {
        service
            .lock()
            .map_err(|_| LibraryError::StateLock)?
            .search_documents(request)
            .map_err(CommandError::from)
    })
    .await
    .map_err(|error| CommandError {
        code: "searchTask".to_string(),
        message: format!("搜索任务无法完成：{error}"),
    })?
}

#[tauri::command]
pub async fn index_pending_documents(
    library: LibrarySummary,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<IndexRunResult, CommandError> {
    let service = state.service_handle();
    tauri::async_runtime::spawn_blocking(move || {
        let mut result = IndexRunResult::default();
        loop {
            let document = index_next_pending_for_library(&service, &library)?;
            let Some(document) = document else {
                break;
            };
            result.processed += 1;
            match document.index_status {
                IndexStatus::Searchable => result.searchable += 1,
                IndexStatus::Failed => result.failed += 1,
                IndexStatus::Pending => {}
            }
            if result.processed % 100 == 0 {
                let _ = app.emit(
                    "document-index-changed",
                    DocumentIndexChangedEvent {
                        library: library.clone(),
                        phase: DocumentIndexPhase::Processing,
                        document_ids: Vec::new(),
                        result: Some(result.clone()),
                    },
                );
            }
        }
        let _ = app.emit(
            "document-index-changed",
            DocumentIndexChangedEvent {
                library,
                phase: DocumentIndexPhase::Completed,
                document_ids: Vec::new(),
                result: Some(result.clone()),
            },
        );
        Ok(result)
    })
    .await
    .map_err(|error| CommandError {
        code: "indexTask".to_string(),
        message: format!("索引任务无法完成：{error}"),
    })?
}

fn index_next_pending_for_library(
    service: &Arc<Mutex<LibraryService>>,
    library: &LibrarySummary,
) -> Result<Option<DocumentSummary>, CommandError> {
    let mut current = service.lock().map_err(|_| LibraryError::StateLock)?;
    current.ensure_current_library(library)?;
    current
        .index_next_pending_document()
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn pending_index_count(state: State<'_, AppState>) -> Result<i64, CommandError> {
    pending_index_count_contract(&state)
}

fn pending_index_count_contract(state: &AppState) -> Result<i64, CommandError> {
    state
        .service()?
        .pending_index_count()
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn retry_document_index(
    library: LibrarySummary,
    document_id: String,
    state: State<'_, AppState>,
) -> Result<DocumentSummary, CommandError> {
    let service = state.service_handle();
    tauri::async_runtime::spawn_blocking(move || {
        let mut service = service.lock().map_err(|_| LibraryError::StateLock)?;
        service.ensure_current_library(&library)?;
        service
            .retry_document_index(&document_id)
            .map_err(CommandError::from)
    })
    .await
    .map_err(|error| CommandError {
        code: "indexTask".to_string(),
        message: format!("索引任务无法完成：{error}"),
    })?
}

#[tauri::command]
pub async fn get_document_preview(
    library: LibrarySummary,
    document_id: String,
    page: Option<u32>,
    state: State<'_, AppState>,
) -> Result<DocumentPreview, CommandError> {
    // 预览会解析整份文档，同样不能占住主线程等服务锁。
    let service = state.service_handle();
    tauri::async_runtime::spawn_blocking(move || {
        let service = service.lock().map_err(|_| LibraryError::StateLock)?;
        service.ensure_current_library(&library)?;
        service
            .get_document_preview(&document_id, page)
            .map_err(CommandError::from)
    })
    .await
    .map_err(|error| CommandError {
        code: "previewTask".to_string(),
        message: format!("预览任务无法完成：{error}"),
    })?
}

#[cfg(test)]
fn get_document_preview_contract(
    state: &AppState,
    library: &LibrarySummary,
    document_id: String,
    page: Option<u32>,
) -> Result<DocumentPreview, CommandError> {
    state.with_library(library, |service| {
        service.get_document_preview(&document_id, page)
    })
}

#[tauri::command]
pub fn list_document_sheets(
    library: LibrarySummary,
    document_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<TableSheet>, CommandError> {
    list_document_sheets_contract(&state, &library, document_id)
}

fn list_document_sheets_contract(
    state: &AppState,
    library: &LibrarySummary,
    document_id: String,
) -> Result<Vec<TableSheet>, CommandError> {
    state.with_library(library, |service| {
        service.list_document_sheets(&document_id)
    })
}

#[tauri::command]
pub fn get_table_preview(
    library: LibrarySummary,
    document_id: String,
    sheet_index: usize,
    start_row: usize,
    row_count: usize,
    column_count: usize,
    state: State<'_, AppState>,
) -> Result<DocumentPreview, CommandError> {
    get_table_preview_contract(
        &state,
        &library,
        document_id,
        TablePreviewRequest {
            sheet_index,
            start_row,
            row_count,
            column_count,
        },
    )
}

fn get_table_preview_contract(
    state: &AppState,
    library: &LibrarySummary,
    document_id: String,
    request: TablePreviewRequest,
) -> Result<DocumentPreview, CommandError> {
    state.with_library(library, |service| {
        service.get_table_preview(&document_id, request)
    })
}

#[tauri::command]
pub async fn get_document_thumbnail(
    library: LibrarySummary,
    document_id: String,
    state: State<'_, AppState>,
) -> Result<DocumentThumbnail, CommandError> {
    let service = state.service_handle();
    tauri::async_runtime::spawn_blocking(move || {
        let service = service.lock().map_err(|_| LibraryError::StateLock)?;
        service.ensure_current_library(&library)?;
        service
            .get_document_thumbnail(&document_id)
            .map_err(CommandError::from)
    })
    .await
    .map_err(|error| CommandError {
        code: "thumbnailTask".to_string(),
        message: format!("缩略图任务无法完成：{error}"),
    })?
}

#[tauri::command]
pub async fn save_document_thumbnail(
    library: LibrarySummary,
    document_id: String,
    content_hash: String,
    thumbnail_data_url: String,
    state: State<'_, AppState>,
) -> Result<DocumentThumbnail, CommandError> {
    let service = state.service_handle();
    tauri::async_runtime::spawn_blocking(move || {
        let service = service.lock().map_err(|_| LibraryError::StateLock)?;
        service.ensure_current_library(&library)?;
        service
            .save_document_thumbnail(&document_id, &content_hash, &thumbnail_data_url)
            .map_err(CommandError::from)
    })
    .await
    .map_err(|error| CommandError {
        code: "thumbnailTask".to_string(),
        message: format!("缩略图保存任务无法完成：{error}"),
    })?
}

#[tauri::command]
pub fn open_document(
    library: LibrarySummary,
    document_id: String,
    state: State<'_, AppState>,
) -> Result<(), CommandError> {
    open_document_contract(&state, &library, document_id)
}

fn open_document_contract(
    state: &AppState,
    library: &LibrarySummary,
    document_id: String,
) -> Result<(), CommandError> {
    state.with_library(library, |service| service.open_document(&document_id))
}

#[tauri::command]
pub fn open_external_url(url: String, state: State<'_, AppState>) -> Result<(), CommandError> {
    state
        .service()?
        .open_external_url(&url)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_document_format_capabilities() -> Result<Vec<DocumentFormatCapability>, CommandError> {
    Ok(crate::library::document_format_capabilities().to_vec())
}

// ---- 分类规则（工作单 10） ----

#[tauri::command]
pub fn list_classification_rules(
    library: LibrarySummary,
    state: State<'_, AppState>,
) -> Result<Vec<ClassificationRule>, CommandError> {
    list_classification_rules_contract(&state, &library)
}

fn list_classification_rules_contract(
    state: &AppState,
    library: &LibrarySummary,
) -> Result<Vec<ClassificationRule>, CommandError> {
    state.with_library(library, |service| service.list_classification_rules())
}

#[tauri::command]
pub fn apply_classification_rule_operation(
    library: LibrarySummary,
    operation: ClassificationRuleOperation,
    state: State<'_, AppState>,
) -> Result<Vec<ClassificationRule>, CommandError> {
    apply_classification_rule_operation_contract(&state, &library, operation)
}

fn apply_classification_rule_operation_contract(
    state: &AppState,
    library: &LibrarySummary,
    operation: ClassificationRuleOperation,
) -> Result<Vec<ClassificationRule>, CommandError> {
    state.with_library(library, |service| {
        service.apply_classification_rule_operation(operation)
    })
}

#[tauri::command]
pub fn preview_classification(
    library: LibrarySummary,
    request: ClassificationPreviewRequest,
    state: State<'_, AppState>,
) -> Result<ClassificationPreviewResponse, CommandError> {
    preview_classification_contract(&state, &library, request)
}

fn preview_classification_contract(
    state: &AppState,
    library: &LibrarySummary,
    request: ClassificationPreviewRequest,
) -> Result<ClassificationPreviewResponse, CommandError> {
    state.with_library(library, |service| service.preview_classification(request))
}

// ---- 接收目录（工作单 11/12/13） ----

#[tauri::command]
pub fn list_receive_sources(
    library: LibrarySummary,
    state: State<'_, AppState>,
) -> Result<Vec<ReceiveSource>, CommandError> {
    list_receive_sources_contract(&state, &library)
}

fn list_receive_sources_contract(
    state: &AppState,
    library: &LibrarySummary,
) -> Result<Vec<ReceiveSource>, CommandError> {
    state.with_library(library, |service| service.list_receive_sources())
}

#[tauri::command]
pub fn list_receive_source_candidates(
    library: LibrarySummary,
    kind: ReceiveSourceKind,
    state: State<'_, AppState>,
) -> Result<ReceiveSourceCandidates, CommandError> {
    list_receive_source_candidates_contract(&state, &library, kind)
}

fn list_receive_source_candidates_contract(
    state: &AppState,
    library: &LibrarySummary,
    kind: ReceiveSourceKind,
) -> Result<ReceiveSourceCandidates, CommandError> {
    state.with_library(library, |service| {
        service.list_receive_source_candidates(kind)
    })
}

#[tauri::command]
pub fn upsert_receive_source(
    library: LibrarySummary,
    source_id: Option<String>,
    input: ReceiveSourceInput,
    state: State<'_, AppState>,
) -> Result<Vec<ReceiveSource>, CommandError> {
    upsert_receive_source_contract(&state, &library, source_id, input)
}

fn upsert_receive_source_contract(
    state: &AppState,
    library: &LibrarySummary,
    source_id: Option<String>,
    input: ReceiveSourceInput,
) -> Result<Vec<ReceiveSource>, CommandError> {
    state.with_library(library, |service| {
        service.upsert_receive_source(source_id.as_deref(), input)
    })
}

#[tauri::command]
pub fn remove_receive_source(
    library: LibrarySummary,
    source_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<ReceiveSource>, CommandError> {
    remove_receive_source_contract(&state, &library, source_id)
}

fn remove_receive_source_contract(
    state: &AppState,
    library: &LibrarySummary,
    source_id: String,
) -> Result<Vec<ReceiveSource>, CommandError> {
    state.with_library(library, |service| service.remove_receive_source(&source_id))
}

#[tauri::command]
pub fn list_receive_directory_files(
    library: LibrarySummary,
    source_id: String,
    state: State<'_, AppState>,
) -> Result<ReceiveDirectoryListing, CommandError> {
    list_receive_directory_files_contract(&state, &library, source_id)
}

fn list_receive_directory_files_contract(
    state: &AppState,
    library: &LibrarySummary,
    source_id: String,
) -> Result<ReceiveDirectoryListing, CommandError> {
    state.with_library(library, |service| {
        service.list_receive_directory_files(&source_id)
    })
}

#[tauri::command]
pub fn skip_receive_directory_files(
    library: LibrarySummary,
    operation: ReceiveDirectoryOperation,
    state: State<'_, AppState>,
) -> Result<Vec<ReceiveSource>, CommandError> {
    skip_receive_directory_files_contract(&state, &library, operation)
}

fn skip_receive_directory_files_contract(
    state: &AppState,
    library: &LibrarySummary,
    operation: ReceiveDirectoryOperation,
) -> Result<Vec<ReceiveSource>, CommandError> {
    state.with_library(library, |service| {
        service.skip_receive_directory_files(operation)
    })
}

/// 导入用户在首次清单里勾选的文件；每个文件单独加锁，读取命令不会被整轮挡住。
#[tauri::command]
pub async fn apply_receive_directory_selection(
    library: LibrarySummary,
    operation: ReceiveDirectoryOperation,
    state: State<'_, AppState>,
) -> Result<ReceiveSourceScanResult, CommandError> {
    let service = state.service_handle();
    tauri::async_runtime::spawn_blocking(move || {
        drive_receive_import(
            &service,
            &library,
            &operation.source_id,
            operation.paths,
        )
        .map_err(CommandError::from)
    })
    .await
    .map_err(|error| CommandError {
        code: "receiveImportTask".to_string(),
        message: format!("接收目录导入无法完成：{error}"),
    })?
}

/// 补扫当前资料库已启用的接收来源；切换资料库后不会继续写入旧资料库。
#[tauri::command]
pub async fn scan_receive_sources(
    library: LibrarySummary,
    state: State<'_, AppState>,
) -> Result<Vec<ReceiveSourceScanResult>, CommandError> {
    let service = state.service_handle();
    tauri::async_runtime::spawn_blocking(move || {
        // 用户主动触发：可以立即重试此前失败的项。
        run_receive_scan(&service, &library, true).map_err(CommandError::from)
    })
    .await
    .map_err(|error| CommandError {
        code: "receiveScanTask".to_string(),
        message: format!("接收目录扫描无法完成：{error}"),
    })?
}

#[tauri::command]
pub fn list_receive_import_log(
    library: LibrarySummary,
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> Result<Vec<ReceiveImportLogEntry>, CommandError> {
    list_receive_import_log_contract(&state, &library, limit.unwrap_or(100))
}

fn list_receive_import_log_contract(
    state: &AppState,
    library: &LibrarySummary,
    limit: usize,
) -> Result<Vec<ReceiveImportLogEntry>, CommandError> {
    state.with_library(library, |service| service.list_receive_import_log(limit))
}

#[tauri::command]
pub fn list_recent_libraries(
    state: State<'_, AppState>,
) -> Result<Vec<RecentLibrary>, CommandError> {
    list_recent_libraries_contract(&state)
}

fn list_recent_libraries_contract(state: &AppState) -> Result<Vec<RecentLibrary>, CommandError> {
    Ok(state.service()?.list_recent_libraries())
}

#[tauri::command]
pub fn forget_recent_library(
    path: String,
    state: State<'_, AppState>,
) -> Result<Vec<RecentLibrary>, CommandError> {
    forget_recent_library_contract(&state, path)
}

fn forget_recent_library_contract(
    state: &AppState,
    path: String,
) -> Result<Vec<RecentLibrary>, CommandError> {
    state
        .service()?
        .forget_recent_library(path)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn open_library_directory(path: String) -> Result<(), CommandError> {
    open_library_directory_contract(path)
}

fn open_library_directory_contract(path: String) -> Result<(), CommandError> {
    crate::library::open_directory(path).map_err(CommandError::from)
}

fn emit_library_changed(app: &AppHandle, action: &str, library: LibrarySummary) {
    let event = LibraryChangedEvent {
        action: action.to_string(),
        library,
    };
    let _ = app.emit("library-changed", event);
}

#[cfg(test)]
mod tests {
    use super::{
        add_tag_to_document_contract, apply_classification_rule_operation_contract,
        bootstrap_contract, create_collection_contract, create_library_contract,
        create_tag_contract, delete_collection_contract, delete_tag_contract,
        drive_receive_import, empty_trash_contract, get_document_preview_contract,
        index_next_pending_for_library, inspect_library_location_contract,
        list_classification_rules_contract, list_collections_contract,
        list_document_format_capabilities, list_documents_contract,
        list_receive_directory_files_contract, list_receive_import_log_contract,
        list_receive_source_candidates_contract, list_receive_sources_contract, list_tags_contract,
        list_trash_documents_contract, move_collection_contract,
        move_document_to_collection_contract, move_document_to_trash_contract,
        open_library_contract, pending_index_count_contract, permanently_delete_document_contract,
        preview_classification_contract, remove_receive_source_contract,
        remove_tag_from_document_contract, rename_collection_contract, rename_tag_contract,
        resolve_import_item_contract, restore_document_contract, retry_import_item_contract,
        skip_receive_directory_files_contract, spawn_batch_organize_task, spawn_import_task,
        start_import_contract, update_document_metadata_contract, upsert_receive_source_contract,
        AppState, CommandError, LibraryChangedEvent, ReceiveDirectoryMonitor,
    };
    use crate::library::{
        BatchDocumentOperation, BatchDocumentOperationRequest, ClassificationPreviewRequest,
        ClassificationRuleInput, ClassificationRuleOperation, ClassificationRuleUpdate,
        DocumentMetadataUpdate, DocumentPreview, DocumentSearchFilters, DocumentSearchQuery,
        DocumentThumbnail, ImportDecision, ImportItemStatus, ImportSource, IndexStatus,
        LibraryService, LibrarySummary, LocationStatus, ReceiveDirectoryOperation,
        ReceiveSourceInput, ReceiveSourceKind,
    };
    use std::path::Path;
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    fn copy_directory(source: &Path, destination: &Path) {
        std::fs::create_dir_all(destination).unwrap();
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let target = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_directory(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    #[test]
    fn create_and_bootstrap_commands_use_the_serialized_contract() {
        let root = tempdir().unwrap();
        let state = AppState::new(LibraryService::new(root.path().join("app-state")).unwrap());
        let library_path = root.path().join("Library");
        let inspection =
            inspect_library_location_contract(&state, library_path.to_string_lossy().into_owned())
                .unwrap();
        assert_eq!(inspection.status, LocationStatus::Usable);

        let created =
            create_library_contract(&state, library_path.to_string_lossy().into_owned()).unwrap();
        let bootstrap = bootstrap_contract(&state).unwrap();
        assert_eq!(bootstrap.current_library.unwrap().id, created.id);
        assert_eq!(bootstrap.recent_libraries.len(), 1);
    }

    #[test]
    fn library_response_uses_camel_case() {
        let event = LibraryChangedEvent {
            action: "created".to_string(),
            library: LibrarySummary {
                id: "lib-1".to_string(),
                name: "资料库".to_string(),
                path: r"C:\Docs\Library".to_string(),
                created_at: "2026-09-13T00:00:00Z".to_string(),
            },
        };

        let value = serde_json::to_value(event).unwrap();
        assert_eq!(value["library"]["createdAt"], "2026-09-13T00:00:00Z");
        assert_eq!(value["action"], "created");
    }

    #[test]
    fn preview_and_thumbnail_responses_use_camel_case() {
        let preview = serde_json::to_value(DocumentPreview::Pdf {
            data_url: "data:image/png;base64,AA==".to_string(),
            page_count: Some(3),
            page: 1,
        })
        .unwrap();
        assert_eq!(preview["kind"], "pdf");
        assert!(preview.get("dataUrl").is_some());
        assert!(preview.get("pageCount").is_some());
        assert!(preview.get("page").is_some());
        assert!(preview.get("data_url").is_none());

        let pptx = serde_json::to_value(DocumentPreview::Pptx {
            data_url: "data:application/vnd.openxmlformats-officedocument.presentationml.presentation;base64,AA==".to_string(),
            text: "幻灯片正文".to_string(),
            notice: "只读".to_string(),
            degraded_features: vec!["复杂图表".to_string()],
        })
        .unwrap();
        assert_eq!(pptx["kind"], "pptx");
        assert!(pptx.get("dataUrl").is_some());
        assert!(pptx.get("degradedFeatures").is_some());
        assert!(pptx.get("data_url").is_none());

        let thumbnail = serde_json::to_value(DocumentThumbnail::Fallback {
            reason: "使用类型图标。".to_string(),
        })
        .unwrap();
        assert_eq!(thumbnail["kind"], "fallback");
        assert!(thumbnail.get("reason").is_some());
    }

    #[test]
    fn format_capability_command_matches_the_shared_contract() {
        let capabilities = list_document_format_capabilities().unwrap();
        let value = serde_json::to_value(capabilities).unwrap();
        assert_eq!(value.as_array().unwrap().len(), 9);
        assert_eq!(value[0]["id"], "pdf");
        assert_eq!(value[0]["displayType"], "PDF");
        assert_eq!(value[0]["security"]["scripts"], "blocked");
        assert_eq!(value[0]["security"]["remoteResources"], "blocked");
        assert_eq!(value[0]["security"]["sourceMutation"], "blocked");
        assert_eq!(value[6]["id"], "pptx");
        assert_eq!(value[6]["importEnabled"], true);
        assert_eq!(value[6]["validation"], "pptxPackage");
        assert_eq!(value[6]["preview"], "pptxPages");
        assert_eq!(value[6]["thumbnail"], "pptxFirstPage");
        assert_eq!(value[6]["textExtraction"], "pptxText");

        // 表格格式由同一张能力表驱动，命令层只透传，不包含业务规则。
        assert_eq!(value[7]["id"], "csv");
        assert_eq!(value[7]["importEnabled"], true);
        assert_eq!(value[7]["validation"], "csvText");
        assert_eq!(value[7]["preview"], "tablePaged");
        assert_eq!(value[7]["textExtraction"], "tableText");
        assert_eq!(value[7]["searchable"], true);
        assert_eq!(value[8]["id"], "xlsx");
        assert_eq!(value[8]["importEnabled"], true);
        assert_eq!(value[8]["validation"], "xlsxPackage");
        assert_eq!(value[8]["preview"], "tablePaged");
        assert_eq!(value[8]["textExtraction"], "tableText");
        assert_eq!(value[8]["security"]["macros"], "blocked");
        assert_eq!(value[8]["security"]["remoteResources"], "blocked");
    }

    #[test]
    fn batch_operation_contract_uses_camel_case() {
        let request: BatchDocumentOperationRequest = serde_json::from_value(serde_json::json!({
            "jobId": "batch-1",
            "documentIds": ["document-1", "document-2"],
            "operation": {
                "kind": "moveToCollection",
                "collectionId": "projects"
            }
        }))
        .unwrap();
        assert_eq!(request.job_id, "batch-1");
        assert_eq!(request.document_ids.len(), 2);
        assert_eq!(
            request.operation,
            BatchDocumentOperation::MoveToCollection {
                collection_id: "projects".to_string()
            }
        );

        let root = tempdir().unwrap();
        let state = AppState::new(LibraryService::new(root.path().join("app-state")).unwrap());
        let library = create_library_contract(
            &state,
            root.path().join("Library").to_string_lossy().into_owned(),
        )
        .unwrap();
        let collection =
            create_collection_contract(&state, &library, "项目".to_string(), None).unwrap();
        let source_path = root.path().join("document.txt");
        std::fs::write(&source_path, "document").unwrap();
        let document_id = start_import_contract(
            &state,
            vec![source_path.to_string_lossy().into_owned()],
            |_| {},
        )
        .unwrap()
        .items
        .remove(0)
        .document_id
        .unwrap();

        let result = state
            .service()
            .unwrap()
            .batch_organize_documents(
                BatchDocumentOperationRequest {
                    job_id: request.job_id,
                    document_ids: vec![document_id.clone()],
                    operation: BatchDocumentOperation::MoveToCollection {
                        collection_id: collection.id,
                    },
                },
                || false,
            )
            .unwrap();
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value["jobId"], "batch-1");
        assert_eq!(value["succeededCount"], 1);
        assert_eq!(value["results"][0]["documentId"], document_id);
        assert_eq!(value["results"][0]["status"], "succeeded");
        assert!(value["results"][0].get("errorMessage").is_some());
    }

    #[test]
    fn search_contract_uses_camel_case_and_returns_match_details() {
        let request: DocumentSearchQuery = serde_json::from_value(serde_json::json!({
            "query": "正文匹配",
            "filters": {
                "collectionId": null,
                "tagId": null,
                "fileType": "TXT",
                "documentDateFrom": null,
                "documentDateTo": null
            }
        }))
        .unwrap();
        assert_eq!(request.filters.file_type.as_deref(), Some("TXT"));

        let root = tempdir().unwrap();
        let state = AppState::new(LibraryService::new(root.path().join("app-state")).unwrap());
        create_library_contract(
            &state,
            root.path().join("Library").to_string_lossy().into_owned(),
        )
        .unwrap();
        let source_path = root.path().join("document.txt");
        std::fs::write(&source_path, "正文匹配内容").unwrap();
        let document_id = start_import_contract(
            &state,
            vec![source_path.to_string_lossy().into_owned()],
            |_| {},
        )
        .unwrap()
        .items
        .remove(0)
        .document_id
        .unwrap();
        state.service().unwrap().index_pending_documents().unwrap();

        let response = state
            .service()
            .unwrap()
            .search_documents(DocumentSearchQuery {
                filters: DocumentSearchFilters {
                    file_type: Some("TXT".to_string()),
                    ..DocumentSearchFilters::default()
                },
                ..request
            })
            .unwrap();
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["results"][0]["document"]["id"], document_id);
        assert_eq!(value["results"][0]["matchKind"], "content");
        assert!(value["results"][0]["snippet"]
            .as_str()
            .unwrap()
            .contains("正文匹配"));
    }

    #[test]
    fn command_errors_serialize_as_structured_events() {
        let root = tempdir().unwrap();
        let non_empty = root.path().join("not-empty");
        std::fs::create_dir_all(&non_empty).unwrap();
        std::fs::write(non_empty.join("keep.txt"), "keep").unwrap();
        let state = AppState::new(LibraryService::new(root.path().join("app-state")).unwrap());

        let error: CommandError =
            create_library_contract(&state, non_empty.to_string_lossy().into_owned()).unwrap_err();
        let value = serde_json::to_value(error).unwrap();
        assert_eq!(value["code"], "invalidLocation");
        assert!(value["message"].as_str().unwrap().contains("目录不为空"));
    }

    #[test]
    fn import_commands_use_camel_case_contract_and_emit_progress() {
        let root = tempdir().unwrap();
        let state = AppState::new(LibraryService::new(root.path().join("app-state")).unwrap());
        let library_path = root.path().join("Library");
        let library =
            create_library_contract(&state, library_path.to_string_lossy().into_owned()).unwrap();
        let source_path = root.path().join("source.txt");
        std::fs::write(&source_path, "contract contents").unwrap();

        let mut progress = Vec::new();
        let batch = start_import_contract(
            &state,
            vec![source_path.to_string_lossy().into_owned()],
            |event| progress.push(event),
        )
        .unwrap();
        assert_eq!(progress.first().unwrap().completed, 0);
        assert!(!progress.first().unwrap().finished);
        assert!(progress.last().unwrap().finished);
        assert_eq!(progress.last().unwrap().completed, 1);
        assert_eq!(pending_index_count_contract(&state).unwrap(), 1);

        let value = serde_json::to_value(&batch).unwrap();
        assert!(value.get("batchId").is_some());
        assert!(value.get("items").is_some());
        assert!(value.get("importedCount").is_some());
        assert!(value.get("duplicateCount").is_some());
        assert!(value.get("sourceChangedCount").is_some());
        assert!(value.get("failedCount").is_some());
        assert!(value.get("ignoredCount").is_some());
        assert!(value.get("imported_count").is_none());
        assert_eq!(value["items"][0]["status"], "imported");
        assert!(value["items"][0].get("itemId").is_some());
        assert!(value["items"][0].get("sourcePath").is_some());
        assert!(value["items"][0].get("fileName").is_some());
        assert!(value["items"][0].get("fileType").is_some());
        assert!(value["items"][0].get("documentId").is_some());
        assert!(value["items"][0].get("duplicateDocumentId").is_some());
        assert!(value["items"][0].get("errorStage").is_some());
        assert!(value["items"][0].get("errorMessage").is_some());

        let duplicate = start_import_contract(
            &state,
            vec![source_path.to_string_lossy().into_owned()],
            |_| {},
        )
        .unwrap()
        .items
        .remove(0);
        let resolved = resolve_import_item_contract(
            &state,
            &library,
            duplicate.item_id,
            ImportDecision::UseExisting,
        )
        .unwrap();
        assert_eq!(resolved.status, ImportItemStatus::Skipped);
        assert_eq!(
            serde_json::to_value(ImportDecision::UseExisting).unwrap(),
            "useExisting"
        );

        let missing_path = root.path().join("retry.txt");
        let failed = start_import_contract(
            &state,
            vec![missing_path.to_string_lossy().into_owned()],
            |_| {},
        )
        .unwrap()
        .items
        .remove(0);
        std::fs::write(&missing_path, "retried").unwrap();
        let retried = retry_import_item_contract(&state, &library, failed.item_id).unwrap();
        let value = serde_json::to_value(retried).unwrap();
        assert_eq!(value["status"], "imported");
        assert!(value.get("retryable").is_some());
    }

    #[test]
    fn start_import_task_returns_while_the_service_is_locked() {
        let root = tempdir().unwrap();
        let state = AppState::new(LibraryService::new(root.path().join("app-state")).unwrap());
        let library = create_library_contract(
            &state,
            root.path().join("Library").to_string_lossy().into_owned(),
        )
        .unwrap();
        let source_path = root.path().join("source.txt");
        std::fs::write(&source_path, "async import contents").unwrap();

        let service_guard = state.service().unwrap();
        let progress = Arc::new(Mutex::new(Vec::new()));
        let task = spawn_import_task(
            &state,
            library,
            vec![source_path.to_string_lossy().into_owned()],
            None,
            ImportSource::FilePicker,
            true,
            {
                let progress = Arc::clone(&progress);
                move |event| progress.lock().unwrap().push(event)
            },
        );
        assert!(!task.inner().is_finished());

        drop(service_guard);
        let batch = tauri::async_runtime::block_on(task)
            .expect("import task should join")
            .expect("import should succeed");
        assert_eq!(batch.imported_count, 1);
        let progress = progress.lock().unwrap();
        assert_eq!(progress.first().unwrap().completed, 0);
        assert!(progress.last().unwrap().finished);
    }

    /// 一批导入尚未结束时，列表、搜索和预览必须能在导入结束前完成。
    ///
    /// 与导入进度回调握手：回调在服务锁之外执行，读取方一定能插进来；旧实现
    /// 整批持锁时读取方会被挡到批结束，`finished_during_read` 随即为真而失败。
    #[test]
    fn reads_return_while_a_batch_import_is_still_running() {
        #[derive(Default)]
        struct ReadGate {
            first_item_done: bool,
            reader_done: bool,
            batch_finished: bool,
        }

        let root = tempdir().unwrap();
        let state = AppState::new(LibraryService::new(root.path().join("app-state")).unwrap());
        let library = create_library_contract(
            &state,
            root.path().join("Library").to_string_lossy().into_owned(),
        )
        .unwrap();

        let existing_path = root.path().join("existing.txt");
        std::fs::write(&existing_path, "导入前就存在的正文").unwrap();
        let existing = state.service().unwrap().import_document(&existing_path).unwrap();
        state
            .service()
            .unwrap()
            .index_pending_documents()
            .unwrap();

        let mut paths = Vec::new();
        for index in 0..6 {
            let path = root.path().join(format!("batch-{index}.txt"));
            std::fs::write(&path, format!("批次正文 {index}")).unwrap();
            paths.push(path.to_string_lossy().into_owned());
        }

        let gate = Arc::new((Mutex::new(ReadGate::default()), Condvar::new()));
        let task = spawn_import_task(
            &state,
            library.clone(),
            paths,
            None,
            ImportSource::FilePicker,
            true,
            {
                let gate = Arc::clone(&gate);
                move |progress| {
                    let (lock, cvar) = &*gate;
                    let mut status = lock.lock().unwrap();
                    if progress.finished {
                        status.batch_finished = true;
                        cvar.notify_all();
                        return;
                    }
                    if progress.item.is_none() || progress.completed != 1 || status.first_item_done {
                        return;
                    }
                    status.first_item_done = true;
                    cvar.notify_all();
                    // 等读取方在批次尚未结束时读完；超时说明读取被导入挡住了。
                    let deadline = Instant::now() + Duration::from_secs(5);
                    while !status.reader_done && Instant::now() < deadline {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        let (next, _) = cvar.wait_timeout(status, remaining).unwrap();
                        status = next;
                    }
                }
            },
        );

        let (lock, cvar) = &*gate;
        let mut status = lock.lock().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !status.first_item_done && Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let (next, _) = cvar.wait_timeout(status, remaining).unwrap();
            status = next;
        }
        let signalled = status.first_item_done;
        drop(status);
        assert!(signalled, "等待第一项导入完成超时");

        // 读取方：批仍在进行时发起列表、搜索与预览。
        let started = Instant::now();
        let documents = list_documents_contract(&state).unwrap();
        let matches = state
            .service()
            .unwrap()
            .search_documents(DocumentSearchQuery {
                query: "导入前就存在的正文".to_string(),
                filters: DocumentSearchFilters::default(),
            })
            .unwrap();
        let preview =
            get_document_preview_contract(&state, &library, existing.id.clone(), None).unwrap();
        let read_elapsed = started.elapsed();

        let (lock, cvar) = &*gate;
        let mut status = lock.lock().unwrap();
        let finished_during_read = status.batch_finished;
        status.reader_done = true;
        cvar.notify_all();
        drop(status);

        let batch = tauri::async_runtime::block_on(task)
            .expect("import task should join")
            .expect("import should succeed");

        assert!(
            !finished_during_read,
            "读取必须在整批导入结束之前完成，而不是排队到最后"
        );
        assert!(
            read_elapsed < Duration::from_secs(2),
            "读取不应排队等整批导入：{read_elapsed:?}"
        );
        assert!(documents.iter().any(|document| document.id == existing.id));
        assert_eq!(matches.results.len(), 1);
        assert_eq!(
            preview,
            DocumentPreview::Text {
                text: "导入前就存在的正文".to_string()
            }
        );
        assert_eq!(batch.imported_count, 6);
        assert_eq!(batch.items.len(), 6);
        assert_eq!(state.service().unwrap().list_documents().unwrap().len(), 7);
    }

    /// 导入进行到一半时切换资料库：该批以 invalidLibrary 结束，新库零写入，
    /// 已提交的项留在原库，当前资料库身份与命令一致。
    #[test]
    fn switching_libraries_mid_batch_stops_the_import_without_touching_the_new_library() {
        #[derive(Default)]
        struct SwitchGate {
            first_item_done: bool,
            switched: bool,
        }

        let root = tempdir().unwrap();
        let state = AppState::new(LibraryService::new(root.path().join("app-state")).unwrap());
        let first_path = root.path().join("First");
        let second_path = root.path().join("Second");
        create_library_contract(&state, first_path.to_string_lossy().into_owned()).unwrap();
        let second =
            create_library_contract(&state, second_path.to_string_lossy().into_owned()).unwrap();
        let first =
            open_library_contract(&state, first_path.to_string_lossy().into_owned()).unwrap();
        assert_ne!(first.id, second.id);

        let mut paths = Vec::new();
        for index in 0..3 {
            let path = root.path().join(format!("mid-batch-{index}.txt"));
            std::fs::write(&path, format!("中途切库正文 {index}")).unwrap();
            paths.push(path.to_string_lossy().into_owned());
        }

        let gate = Arc::new((Mutex::new(SwitchGate::default()), Condvar::new()));
        let task = spawn_import_task(
            &state,
            first.clone(),
            paths,
            None,
            ImportSource::FilePicker,
            true,
            {
                let gate = Arc::clone(&gate);
                move |progress| {
                    let (lock, cvar) = &*gate;
                    let mut status = lock.lock().unwrap();
                    if progress.item.is_none() || progress.completed != 1 || status.first_item_done {
                        return;
                    }
                    status.first_item_done = true;
                    cvar.notify_all();
                    // 锁外的回调里等待测试线程完成切库；超时说明切库被导入挡住了。
                    let deadline = Instant::now() + Duration::from_secs(5);
                    while !status.switched && Instant::now() < deadline {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        let (next, _) = cvar.wait_timeout(status, remaining).unwrap();
                        status = next;
                    }
                }
            },
        );

        let (lock, cvar) = &*gate;
        let mut status = lock.lock().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !status.first_item_done && Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let (next, _) = cvar.wait_timeout(status, remaining).unwrap();
            status = next;
        }
        let signalled = status.first_item_done;
        drop(status);
        assert!(signalled, "等待第一项导入完成超时");

        let switched = open_library_contract(&state, second_path.to_string_lossy().into_owned());
        assert!(switched.is_ok(), "导入期间切库应当立即成功：{switched:?}");

        let (lock, cvar) = &*gate;
        let mut status = lock.lock().unwrap();
        status.switched = true;
        cvar.notify_all();
        drop(status);

        let error = tauri::async_runtime::block_on(task)
            .expect("import task should join")
            .expect_err("导入进行中切库后该批必须以 invalidLibrary 结束");
        assert_eq!(error.code, "invalidLibrary");

        // 命令与界面始终指向同一个资料库：当前就是切换后的那个。
        let current = state
            .service()
            .unwrap()
            .current_library()
            .cloned()
            .expect("切换后应当有当前资料库");
        assert_eq!(current.id, second.id);
        assert!(state.service().unwrap().list_documents().unwrap().is_empty());
        assert_eq!(
            std::fs::read_dir(second_path.join("documents"))
                .unwrap()
                .count(),
            0,
            "新库不应写入任何副本"
        );

        // 原库保留已经提交的第一项。
        open_library_contract(&state, first_path.to_string_lossy().into_owned()).unwrap();
        let documents = state.service().unwrap().list_documents().unwrap();
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].file_name, "mid-batch-0.txt");
    }

    #[test]
    fn queued_import_does_not_write_to_a_library_opened_after_the_request() {
        let root = tempdir().unwrap();
        let state = AppState::new(LibraryService::new(root.path().join("app-state")).unwrap());
        let first_path = root.path().join("First");
        let second_path = root.path().join("Second");
        create_library_contract(&state, first_path.to_string_lossy().into_owned()).unwrap();
        create_library_contract(&state, second_path.to_string_lossy().into_owned()).unwrap();
        let first =
            open_library_contract(&state, first_path.to_string_lossy().into_owned()).unwrap();
        let source = root.path().join("queued.txt");
        std::fs::write(&source, "belongs to First").unwrap();

        let mut guard = state.service().unwrap();
        let task = spawn_import_task(
            &state,
            first,
            vec![source.to_string_lossy().into_owned()],
            None,
            ImportSource::FilePicker,
            true,
            |_| {},
        );
        guard.open_library(&second_path).unwrap();
        drop(guard);

        let error = tauri::async_runtime::block_on(task).unwrap().unwrap_err();
        assert_eq!(error.code, "invalidLibrary");
        assert!(state
            .service()
            .unwrap()
            .list_documents()
            .unwrap()
            .is_empty());
        assert_eq!(
            std::fs::read_dir(second_path.join("documents"))
                .unwrap()
                .count(),
            0
        );
        assert_eq!(std::fs::read(&source).unwrap(), b"belongs to First");
    }

    #[test]
    fn queued_batch_does_not_trash_a_document_in_a_copied_library() {
        let root = tempdir().unwrap();
        let state_dir = root.path().join("app-state");
        let first_path = root.path().join("First");
        let second_path = root.path().join("Second Copy");
        let source = root.path().join("document.txt");
        std::fs::write(&source, "keep both copies").unwrap();
        let mut service = LibraryService::new(&state_dir).unwrap();
        let first = service.create_library(&first_path).unwrap();
        let document = service.import_document(&source).unwrap();
        drop(service);
        copy_directory(&first_path, &second_path);

        let mut service = LibraryService::new(&state_dir).unwrap();
        service.bootstrap().unwrap();
        let state = AppState::new(service);
        let request = BatchDocumentOperationRequest {
            job_id: "queued-batch".to_string(),
            document_ids: vec![document.id.clone()],
            operation: BatchDocumentOperation::MoveToTrash,
        };
        let cancellation = state.begin_batch(&request.job_id).unwrap();
        let mut guard = state.service().unwrap();
        let task = spawn_batch_organize_task(&state, first.clone(), request, cancellation);
        assert_eq!(guard.open_library(&second_path).unwrap().id, first.id);
        drop(guard);

        let error = tauri::async_runtime::block_on(task).unwrap().unwrap_err();
        assert_eq!(error.code, "invalidLibrary");
        assert_eq!(
            state.service().unwrap().list_documents().unwrap(),
            vec![document.clone()]
        );
        assert!(second_path
            .join("documents")
            .join(&document.id)
            .join(&document.file_name)
            .is_file());
        assert_eq!(std::fs::read(&source).unwrap(), b"keep both copies");
        state.finish_batch("queued-batch").unwrap();
    }

    #[test]
    fn queued_indexing_does_not_process_a_copied_library() {
        let root = tempdir().unwrap();
        let state_dir = root.path().join("app-state");
        let first_path = root.path().join("First");
        let second_path = root.path().join("Second Copy");
        let source = root.path().join("document.txt");
        std::fs::write(&source, "index only First").unwrap();
        let mut service = LibraryService::new(&state_dir).unwrap();
        let first = service.create_library(&first_path).unwrap();
        let document = service.import_document(&source).unwrap();
        drop(service);
        copy_directory(&first_path, &second_path);

        let mut service = LibraryService::new(&state_dir).unwrap();
        service.bootstrap().unwrap();
        let state = AppState::new(service);
        let mut guard = state.service().unwrap();
        let handle = state.service_handle();
        let task = tauri::async_runtime::spawn_blocking(move || {
            index_next_pending_for_library(&handle, &first)
        });
        guard.open_library(&second_path).unwrap();
        drop(guard);

        let error = tauri::async_runtime::block_on(task).unwrap().unwrap_err();
        assert_eq!(error.code, "invalidLibrary");
        let documents = state.service().unwrap().list_documents().unwrap();
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].id, document.id);
        assert_eq!(documents[0].index_status, IndexStatus::Pending);
        assert_eq!(std::fs::read(&source).unwrap(), b"index only First");
    }

    #[test]
    fn stale_single_delete_keeps_the_copied_library_document() {
        let root = tempdir().unwrap();
        let state_dir = root.path().join("app-state");
        let first_path = root.path().join("First");
        let second_path = root.path().join("Second Copy");
        let source = root.path().join("document.txt");
        std::fs::write(&source, "keep B document").unwrap();
        let mut service = LibraryService::new(&state_dir).unwrap();
        let first = service.create_library(&first_path).unwrap();
        let document = service.import_document(&source).unwrap();
        drop(service);
        copy_directory(&first_path, &second_path);

        let mut service = LibraryService::new(&state_dir).unwrap();
        service.bootstrap().unwrap();
        let state = AppState::new(service);
        assert_eq!(
            state
                .service()
                .unwrap()
                .open_library(&second_path)
                .unwrap()
                .id,
            first.id
        );

        let error =
            move_document_to_trash_contract(&state, &first, document.id.clone()).unwrap_err();
        assert_eq!(error.code, "invalidLibrary");
        assert_eq!(
            state.service().unwrap().list_documents().unwrap(),
            vec![document.clone()]
        );
        assert!(second_path
            .join("documents")
            .join(&document.id)
            .join(&document.file_name)
            .is_file());

        state.service().unwrap().open_library(&first_path).unwrap();
        move_document_to_trash_contract(&state, &first, document.id.clone()).unwrap();
        assert!(state
            .service()
            .unwrap()
            .list_documents()
            .unwrap()
            .is_empty());
        state.service().unwrap().open_library(&second_path).unwrap();
        assert_eq!(
            state.service().unwrap().list_documents().unwrap(),
            vec![document]
        );
        assert_eq!(std::fs::read(&source).unwrap(), b"keep B document");
    }

    #[test]
    fn stale_restore_and_permanent_delete_keep_the_copied_library_trash() {
        let root = tempdir().unwrap();
        let state_dir = root.path().join("app-state");
        let first_path = root.path().join("First");
        let second_path = root.path().join("Second Copy");
        let source = root.path().join("document.txt");
        std::fs::write(&source, "keep B trash").unwrap();
        let mut service = LibraryService::new(&state_dir).unwrap();
        let first = service.create_library(&first_path).unwrap();
        let document = service.import_document(&source).unwrap();
        service.move_document_to_trash(&document.id).unwrap();
        drop(service);
        copy_directory(&first_path, &second_path);

        let mut service = LibraryService::new(&state_dir).unwrap();
        service.bootstrap().unwrap();
        let state = AppState::new(service);
        state.service().unwrap().open_library(&second_path).unwrap();

        for error in [
            restore_document_contract(&state, &first, document.id.clone()).unwrap_err(),
            permanently_delete_document_contract(&state, &first, document.id.clone()).unwrap_err(),
        ] {
            assert_eq!(error.code, "invalidLibrary");
        }
        assert_eq!(
            state
                .service()
                .unwrap()
                .list_trash_documents()
                .unwrap()
                .len(),
            1
        );
        assert!(second_path
            .join("documents")
            .join(&document.id)
            .join(&document.file_name)
            .is_file());

        state.service().unwrap().open_library(&first_path).unwrap();
        restore_document_contract(&state, &first, document.id.clone()).unwrap();
        move_document_to_trash_contract(&state, &first, document.id.clone()).unwrap();
        permanently_delete_document_contract(&state, &first, document.id.clone()).unwrap();
        assert!(state
            .service()
            .unwrap()
            .list_trash_documents()
            .unwrap()
            .is_empty());
        state.service().unwrap().open_library(&second_path).unwrap();
        assert_eq!(
            state
                .service()
                .unwrap()
                .list_trash_documents()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(std::fs::read(&source).unwrap(), b"keep B trash");
    }

    #[test]
    fn stale_metadata_update_does_not_change_the_copied_library() {
        let root = tempdir().unwrap();
        let state_dir = root.path().join("app-state");
        let first_path = root.path().join("First");
        let second_path = root.path().join("Second Copy");
        let source = root.path().join("document.txt");
        std::fs::write(&source, "keep B metadata").unwrap();
        let mut service = LibraryService::new(&state_dir).unwrap();
        let first = service.create_library(&first_path).unwrap();
        let document = service.import_document(&source).unwrap();
        drop(service);
        copy_directory(&first_path, &second_path);

        let mut service = LibraryService::new(&state_dir).unwrap();
        service.bootstrap().unwrap();
        let state = AppState::new(service);
        state.service().unwrap().open_library(&second_path).unwrap();
        let update = DocumentMetadataUpdate {
            title: "已修改".to_string(),
            description: None,
            document_date: None,
            collection_id: "inbox".to_string(),
            tag_ids: Vec::new(),
        };

        let error =
            update_document_metadata_contract(&state, &first, document.id.clone(), update.clone())
                .unwrap_err();
        assert_eq!(error.code, "invalidLibrary");
        assert_eq!(
            state.service().unwrap().list_documents().unwrap(),
            vec![document.clone()]
        );

        state.service().unwrap().open_library(&first_path).unwrap();
        let updated =
            update_document_metadata_contract(&state, &first, document.id.clone(), update).unwrap();
        assert_eq!(updated.title, "已修改");
        state.service().unwrap().open_library(&second_path).unwrap();
        assert_eq!(
            state.service().unwrap().list_documents().unwrap(),
            vec![document]
        );
        assert_eq!(std::fs::read(&source).unwrap(), b"keep B metadata");
    }

    #[test]
    fn collection_commands_use_camel_case_contract() {
        let root = tempdir().unwrap();
        let state = AppState::new(LibraryService::new(root.path().join("app-state")).unwrap());
        let library_path = root.path().join("Library");
        let library =
            create_library_contract(&state, library_path.to_string_lossy().into_owned()).unwrap();

        let collections = list_collections_contract(&state).unwrap();
        assert_eq!(collections[0].id, "inbox");
        let parent =
            create_collection_contract(&state, &library, "Parent".to_string(), None).unwrap();
        let child = create_collection_contract(
            &state,
            &library,
            "Child".to_string(),
            Some(parent.id.clone()),
        )
        .unwrap();
        let renamed =
            rename_collection_contract(&state, &library, child.id.clone(), "Renamed".to_string())
                .unwrap();
        let moved = move_collection_contract(&state, &library, renamed.id.clone(), None).unwrap();

        let source_path = root.path().join("document.txt");
        std::fs::write(&source_path, "document").unwrap();
        let imported = start_import_contract(
            &state,
            vec![source_path.to_string_lossy().into_owned()],
            |_| {},
        )
        .unwrap()
        .items
        .remove(0);
        let document = move_document_to_collection_contract(
            &state,
            &library,
            imported.document_id.unwrap(),
            moved.id.clone(),
        )
        .unwrap();
        let document_value = serde_json::to_value(document).unwrap();
        assert_eq!(document_value["collectionId"], moved.id);

        let collection_value = serde_json::to_value(&moved).unwrap();
        assert!(collection_value.get("parentId").is_some());
        assert!(collection_value.get("isInbox").is_some());
        assert!(collection_value.get("documentCount").is_some());

        let deleted = delete_collection_contract(&state, &library, moved.id).unwrap();
        let delete_value = serde_json::to_value(deleted).unwrap();
        assert!(delete_value.get("collectionId").is_some());
        assert!(delete_value.get("targetCollectionId").is_some());
        assert!(delete_value.get("movedDocumentCount").is_some());
    }

    #[test]
    fn tag_and_metadata_commands_use_camel_case_contract() {
        let root = tempdir().unwrap();
        let state = AppState::new(LibraryService::new(root.path().join("app-state")).unwrap());
        let library_path = root.path().join("Library");
        let library =
            create_library_contract(&state, library_path.to_string_lossy().into_owned()).unwrap();
        let source_path = root.path().join("document.txt");
        std::fs::write(&source_path, "document").unwrap();
        let document_id = start_import_contract(
            &state,
            vec![source_path.to_string_lossy().into_owned()],
            |_| {},
        )
        .unwrap()
        .items
        .remove(0)
        .document_id
        .unwrap();

        let work = create_tag_contract(&state, &library, "工作".to_string()).unwrap();
        let important = create_tag_contract(&state, &library, "重要".to_string()).unwrap();
        let renamed =
            rename_tag_contract(&state, &library, work.id.clone(), "项目".to_string()).unwrap();
        assert_eq!(renamed.name, "项目");
        assert_eq!(list_tags_contract(&state).unwrap().len(), 2);

        let updated = update_document_metadata_contract(
            &state,
            &library,
            document_id.clone(),
            DocumentMetadataUpdate {
                title: "项目文档".to_string(),
                description: Some("说明".to_string()),
                document_date: Some("2025-02-03".to_string()),
                collection_id: "inbox".to_string(),
                tag_ids: vec![renamed.id.clone(), important.id.clone()],
            },
        )
        .unwrap();
        let value = serde_json::to_value(updated).unwrap();
        assert_eq!(value["documentDate"], "2025-02-03");
        assert_eq!(value["tags"].as_array().unwrap().len(), 2);
        assert!(value.get("sourcePath").is_some());

        let removed = remove_tag_from_document_contract(
            &state,
            &library,
            document_id.clone(),
            important.id.clone(),
        )
        .unwrap();
        assert_eq!(removed.tags.len(), 1);
        let added =
            add_tag_to_document_contract(&state, &library, document_id, important.id.clone())
                .unwrap();
        assert_eq!(added.tags.len(), 2);
        delete_tag_contract(&state, &library, important.id).unwrap();
        assert_eq!(list_tags_contract(&state).unwrap().len(), 1);
    }

    #[test]
    fn trash_commands_use_camel_case_contract_and_restore_to_the_original_collection() {
        let root = tempdir().unwrap();
        let state = AppState::new(LibraryService::new(root.path().join("app-state")).unwrap());
        let library_path = root.path().join("Library");
        let library =
            create_library_contract(&state, library_path.to_string_lossy().into_owned()).unwrap();
        let collection =
            create_collection_contract(&state, &library, "归档".to_string(), None).unwrap();
        let source_path = root.path().join("document.txt");
        std::fs::write(&source_path, "document").unwrap();
        let document_id = start_import_contract(
            &state,
            vec![source_path.to_string_lossy().into_owned()],
            |_| {},
        )
        .unwrap()
        .items
        .remove(0)
        .document_id
        .unwrap();
        move_document_to_collection_contract(
            &state,
            &library,
            document_id.clone(),
            collection.id.clone(),
        )
        .unwrap();

        move_document_to_trash_contract(&state, &library, document_id.clone()).unwrap();
        let trash = list_trash_documents_contract(&state).unwrap();
        assert_eq!(trash.len(), 1);
        let value = serde_json::to_value(&trash[0]).unwrap();
        assert_eq!(value["originalCollectionId"], collection.id);
        assert_eq!(value["originalCollectionName"], "归档");
        assert!(value.get("deletedAt").is_some());

        let restored = restore_document_contract(&state, &library, document_id.clone()).unwrap();
        assert_eq!(restored.collection_id, collection.id);

        move_document_to_trash_contract(&state, &library, document_id.clone()).unwrap();
        permanently_delete_document_contract(&state, &library, document_id).unwrap();
        assert!(list_trash_documents_contract(&state).unwrap().is_empty());

        let empty = empty_trash_contract(&state, &library).unwrap();
        let value = serde_json::to_value(empty).unwrap();
        assert_eq!(value["deletedCount"], 0);
        assert_eq!(value["failedCount"], 0);
        assert!(value["items"].as_array().unwrap().is_empty());
    }

    /// 分类规则与接收目录的命令层契约：命令可调用、字段名与冻结契约一致。
    #[test]
    fn classification_and_receive_commands_use_the_frozen_contract() {
        let root = tempdir().unwrap();
        let state = AppState::new(LibraryService::new(root.path().join("app-state")).unwrap());
        let library_path = root.path().join("Library");
        let library =
            create_library_contract(&state, library_path.to_string_lossy().into_owned()).unwrap();
        let collection = create_collection_contract(
            &state,
            &library,
            "归档".to_string(),
            None,
        )
        .unwrap();
        let tag = create_tag_contract(&state, &library, "重要".to_string()).unwrap();

        // 五种规则操作都能走通，并按位置返回排序后的列表。
        let created = apply_classification_rule_operation_contract(
            &state,
            &library,
            ClassificationRuleOperation::Create {
                rule: ClassificationRuleInput {
                    name: "报告".to_string(),
                    enabled: true,
                    file_name_pattern: "报告".to_string(),
                    file_type: Some("TXT".to_string()),
                    source_directory: None,
                    collection_id: Some(collection.id.clone()),
                    tag_ids: vec![tag.id.clone()],
                },
            },
        )
        .unwrap();
        assert_eq!(created.len(), 1);
        let rule_id = created[0].id.clone();
        let value = serde_json::to_value(&created[0]).unwrap();
        for key in [
            "id",
            "name",
            "enabled",
            "position",
            "fileNamePattern",
            "fileType",
            "sourceDirectory",
            "collectionId",
            "tagIds",
        ] {
            assert!(value.get(key).is_some(), "分类规则缺少字段 {key}");
        }
        assert!(value.get("file_name_pattern").is_none());

        let operation_json = serde_json::to_value(ClassificationRuleOperation::SetEnabled {
            rule_id: rule_id.clone(),
            enabled: false,
        })
        .unwrap();
        assert_eq!(operation_json["kind"], "setEnabled");
        assert_eq!(operation_json["ruleId"], rule_id);
        assert_eq!(operation_json["enabled"], false);

        apply_classification_rule_operation_contract(
            &state,
            &library,
            ClassificationRuleOperation::Reorder {
                ordered_rule_ids: vec![rule_id.clone()],
            },
        )
        .unwrap();
        apply_classification_rule_operation_contract(
            &state,
            &library,
            ClassificationRuleOperation::SetEnabled {
                rule_id: rule_id.clone(),
                enabled: true,
            },
        )
        .unwrap();
        apply_classification_rule_operation_contract(
            &state,
            &library,
            ClassificationRuleOperation::Update {
                rule: ClassificationRuleUpdate {
                    id: rule_id.clone(),
                    input: ClassificationRuleInput {
                        name: "报告（改）".to_string(),
                        enabled: true,
                        file_name_pattern: "报告".to_string(),
                        file_type: None,
                        source_directory: None,
                        collection_id: Some(collection.id.clone()),
                        tag_ids: Vec::new(),
                    },
                },
            },
        )
        .unwrap();
        let source_path = root.path().join("季度报告.txt");
        std::fs::write(&source_path, "报告正文").unwrap();
        let preview = preview_classification_contract(
            &state,
            &library,
            ClassificationPreviewRequest {
                paths: vec![source_path.to_string_lossy().into_owned()],
                target_collection_id: None,
            },
        )
        .unwrap();
        let value = serde_json::to_value(&preview).unwrap();
        assert_eq!(value["items"][0]["collectionId"], collection.id);
        assert_eq!(value["items"][0]["matchedRuleIds"][0], rule_id);
        assert_eq!(value["items"][0]["fileType"], "TXT");
        assert!(value["items"][0].get("sourcePath").is_some());
        assert!(value["items"][0].get("fileName").is_some());

        apply_classification_rule_operation_contract(
            &state,
            &library,
            ClassificationRuleOperation::Delete {
                rule_id: rule_id.clone(),
            },
        )
        .unwrap();
        assert!(list_classification_rules_contract(&state, &library).unwrap().is_empty());

        // 接收来源：确认目录 → 首次清单 → 跳过未选 → 导入所选。
        let receive_dir = root.path().join("wechat");
        std::fs::create_dir_all(&receive_dir).unwrap();
        let kept = receive_dir.join("保留.txt");
        let skipped = receive_dir.join("未选.txt");
        std::fs::write(&kept, "保留正文").unwrap();
        std::fs::write(&skipped, "未选正文").unwrap();

        let sources = upsert_receive_source_contract(
            &state,
            &library,
            None,
            ReceiveSourceInput {
                kind: ReceiveSourceKind::Wechat,
                display_name: "微信".to_string(),
                path: receive_dir.to_string_lossy().into_owned(),
                enabled: true,
            },
        )
        .unwrap();
        assert_eq!(sources.len(), 1);
        let source_id = sources[0].id.clone();
        let value = serde_json::to_value(&sources[0]).unwrap();
        for key in [
            "id",
            "kind",
            "displayName",
            "path",
            "enabled",
            "status",
            "pendingCount",
            "lastScannedAt",
        ] {
            assert!(value.get(key).is_some(), "接收来源缺少字段 {key}");
        }
        assert_eq!(value["kind"], "wechat");
        assert_eq!(value["status"], "ready");
        assert!(value["lastScannedAt"].is_null());

        let candidates = list_receive_source_candidates_contract(
            &state,
            &library,
            ReceiveSourceKind::Other,
        )
        .unwrap();
        let value = serde_json::to_value(&candidates).unwrap();
        assert_eq!(value["kind"], "other");
        assert!(value["candidates"].as_array().unwrap().is_empty());

        let listing =
            list_receive_directory_files_contract(&state, &library, source_id.clone()).unwrap();
        let value = serde_json::to_value(&listing).unwrap();
        assert_eq!(value["sourceId"], source_id);
        assert_eq!(value["items"].as_array().unwrap().len(), 2);
        for key in ["path", "fileName", "fileType", "fileSize", "previouslySkipped"] {
            assert!(
                value["items"][0].get(key).is_some(),
                "清单项缺少字段 {key}"
            );
        }

        skip_receive_directory_files_contract(
            &state,
            &library,
            ReceiveDirectoryOperation {
                source_id: source_id.clone(),
                paths: vec![skipped.to_string_lossy().into_owned()],
            },
        )
        .unwrap();
        let listing =
            list_receive_directory_files_contract(&state, &library, source_id.clone()).unwrap();
        let value = serde_json::to_value(&listing).unwrap();
        let skipped_item = value["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["path"].as_str().unwrap().ends_with("未选.txt"))
            .unwrap();
        assert_eq!(skipped_item["previouslySkipped"], true);

        let result = drive_receive_import(
            &state.service_handle(),
            &library,
            &source_id,
            vec![kept.to_string_lossy().into_owned()],
        )
        .unwrap();
        let value = serde_json::to_value(&result).unwrap();
        for key in [
            "sourceId",
            "scannedCount",
            "importedCount",
            "skippedCount",
            "pendingCount",
            "failedCount",
        ] {
            assert!(value.get(key).is_some(), "扫描结果缺少字段 {key}");
        }
        assert_eq!(value["importedCount"], 1);

        let log = list_receive_import_log_contract(&state, &library, 10).unwrap();
        assert_eq!(log.len(), 1);
        let value = serde_json::to_value(&log[0]).unwrap();
        for key in [
            "sourceId",
            "sourcePath",
            "fileName",
            "status",
            "documentId",
            "collectionId",
            "tagIds",
            "matchedRuleIds",
            "errorMessage",
            "createdAt",
        ] {
            assert!(value.get(key).is_some(), "导入日志缺少字段 {key}");
        }
        assert_eq!(value["status"], "imported");

        remove_receive_source_contract(&state, &library, source_id).unwrap();
        assert!(list_receive_sources_contract(&state, &library).unwrap().is_empty());
    }

    /// 接收目录监视线程在 Drop 后立刻退出，不留下常驻进程。
    #[test]
    fn receive_directory_monitor_stops_when_dropped() {
        let root = tempdir().unwrap();
        let state = AppState::new(LibraryService::new(root.path().join("app-state")).unwrap());
        let library_path = root.path().join("Library");
        create_library_contract(&state, library_path.to_string_lossy().into_owned()).unwrap();

        let monitor = ReceiveDirectoryMonitor::start(
            state.service_handle(),
            Duration::from_millis(20),
            |_, _| {},
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(60));
        let started = Instant::now();
        drop(monitor);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "监视线程应在 Drop 后立刻退出：{:?}",
            started.elapsed()
        );
    }

    /// 应用运行期间，已确认接收目录里的新文件会被监视自动导入。
    #[test]
    fn receive_directory_monitor_imports_new_files_while_running() {
        let root = tempdir().unwrap();
        let state = AppState::new(LibraryService::new(root.path().join("app-state")).unwrap());
        let library_path = root.path().join("Library");
        let library =
            create_library_contract(&state, library_path.to_string_lossy().into_owned()).unwrap();
        let receive_dir = root.path().join("wechat");
        std::fs::create_dir_all(&receive_dir).unwrap();

        // 先确认目录并完成首次清单（空目录：没有任何未选项）。
        let sources = upsert_receive_source_contract(
            &state,
            &library,
            None,
            ReceiveSourceInput {
                kind: ReceiveSourceKind::Wechat,
                display_name: "微信".to_string(),
                path: receive_dir.to_string_lossy().into_owned(),
                enabled: true,
            },
        )
        .unwrap();
        let source_id = sources[0].id.clone();
        let pending = {
            let mut service = state.service().unwrap();
            service.ensure_current_library(&library).unwrap();
            service.receive_pending_files(&source_id, true).unwrap()
        };
        assert!(pending.is_empty());
        let first = {
            let mut service = state.service().unwrap();
            service.ensure_current_library(&library).unwrap();
            service
                .begin_receive_import_batch(&source_id, Vec::new())
                .unwrap()
        };
        {
            let mut service = state.service().unwrap();
            service
                .finish_receive_import_batch(&first.batch_id)
                .unwrap();
        }

        let monitor = ReceiveDirectoryMonitor::start(
            state.service_handle(),
            Duration::from_millis(30),
            |_, _| {},
        )
        .unwrap();

        // 应用打开期间新保存的文件应在下一次补扫被自动导入。
        std::fs::write(receive_dir.join("新消息.txt"), "新消息正文").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut imported = false;
        while Instant::now() < deadline {
            let documents = list_documents_contract(&state).unwrap();
            if documents.iter().any(|document| document.file_name == "新消息.txt") {
                imported = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(monitor);
        assert!(imported, "运行期间的新文件应被监视自动导入");
        assert_eq!(
            std::fs::read(receive_dir.join("新消息.txt")).unwrap(),
            "新消息正文".as_bytes(),
            "来源文件不应被修改"
        );
    }
}
