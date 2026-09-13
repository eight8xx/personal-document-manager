use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chrono::{NaiveDate, SecondsFormat, Utc};
use flate2::read::{DeflateDecoder, ZlibDecoder};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::error::{LibraryError, LibraryResult};
use super::formats::{
    canonical_file_type, capability_for_file_type, capability_for_path,
    require_capability_for_file_type, unsupported_message, PreviewStrategy, TextExtractionStrategy,
    ThumbnailStrategy, ValidationStrategy,
};
use super::models::{
    BatchDocumentItemResult, BatchDocumentItemStatus, BatchDocumentOperation,
    BatchDocumentOperationRequest, BatchDocumentOperationResult, BootstrapState, CloudSyncWarning,
    CollectionDeleteResult, CollectionSummary, DocumentIndexChangedEvent, DocumentIndexPhase,
    DocumentMetadataUpdate, DocumentPreview, DocumentProcessingStatus, DocumentSearchFilters,
    DocumentSearchQuery, DocumentSearchResponse, DocumentSearchResult, DocumentSummary,
    DocumentThumbnail, EmptyTrashItemResult, EmptyTrashItemStatus, EmptyTrashResult, ImportBatch,
    ImportDecision, ImportItemResult, ImportItemStatus, ImportProgress, IndexRunResult,
    IndexStatus, LibraryLocationInspection, LibraryMetadata, LibrarySummary, LocationStatus,
    RecentLibrary, RecentLibraryRecord, SearchMatchKind, TagSummary, TrashDocumentSummary,
};
use super::{ooxml, thumbnail};

const FORMAT_VERSION: u32 = 1;
const INTERNAL_DIR: &str = ".pdm";
const METADATA_FILE: &str = "library.json";
const DATABASE_FILE: &str = "library.sqlite3";
const DOCUMENTS_DIR: &str = "documents";
const TRASH_DIR: &str = "trash";
const THUMBNAILS_DIR: &str = "thumbnails";
const PPTX_THUMBNAIL_CACHE_SCHEME: &str = "first-slide-v2";
const RECENT_FILE: &str = "recent_libraries.json";
const MAX_RECENT_LIBRARIES: usize = 10;
const DELETE_TOMBSTONE_PREFIX: &str = ".pdm-delete-";
const DELETE_TOMBSTONE_SUFFIX: &str = ".tombstone";

pub struct LibraryService {
    state_dir: PathBuf,
    current: Option<OpenLibrary>,
    recent: Vec<RecentLibraryRecord>,
    import_items: HashMap<String, ImportItemContext>,
}

struct OpenLibrary {
    summary: LibrarySummary,
    connection: Connection,
}

struct ImportItemContext {
    item: ImportItemResult,
    pending: Option<PendingImport>,
    target_collection_id: Option<String>,
}

struct PendingImport {
    path: PathBuf,
    source_path: String,
    file_name: String,
    file_type: String,
    file_size: i64,
    source_identifier: String,
    content_hash: String,
    existing_document_id: String,
    target_collection_id: Option<String>,
}

struct StoredDocumentFile {
    id: String,
    file_name: String,
    file_type: String,
    library_path: String,
    content_hash: Option<String>,
}

struct StoredDocumentIndex {
    id: String,
    file_name: String,
    file_type: String,
    file_size: i64,
    content_hash: Option<String>,
    file_modified_at: Option<i64>,
    library_path: String,
    processing_status: DocumentProcessingStatus,
    index_status: IndexStatus,
}

#[derive(Default)]
struct PptxExtraction {
    text: String,
    degraded_features: Vec<String>,
}

struct CachedImageThumbnail {
    bytes: Vec<u8>,
    media_type: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileFingerprint {
    available: bool,
    is_file: bool,
    size: i64,
    modified_at: Option<i64>,
}

#[derive(Default)]
struct ExternalChangeScan {
    changed_document_ids: Vec<String>,
    pending_count: i64,
}

pub struct ExternalChangeMonitor {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl ExternalChangeMonitor {
    pub fn start<F>(
        service: Arc<Mutex<LibraryService>>,
        poll_interval: Duration,
        quiet_period: Duration,
        on_event: F,
    ) -> io::Result<Self>
    where
        F: Fn(DocumentIndexChangedEvent) + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name("pdm-external-change-monitor".to_string())
            .spawn(move || {
                let mut previous_fingerprints = HashMap::new();
                let mut quiet_deadline = None;

                while !thread_stop.load(Ordering::Relaxed) {
                    thread::park_timeout(poll_interval);
                    if thread_stop.load(Ordering::Relaxed) {
                        break;
                    }

                    let scan = {
                        let mut service = match service.lock() {
                            Ok(service) => service,
                            Err(_) => break,
                        };
                        let scan = match service.scan_external_changes(false) {
                            Ok(scan) => scan,
                            Err(_) => continue,
                        };
                        let fingerprints = service.external_file_fingerprints();
                        (scan, fingerprints)
                    };
                    let (scan, fingerprints) = scan;

                    let fingerprints_changed =
                        !previous_fingerprints.is_empty() && fingerprints != previous_fingerprints;
                    previous_fingerprints = fingerprints;

                    if !scan.changed_document_ids.is_empty() || fingerprints_changed {
                        quiet_deadline = Some(Instant::now() + quiet_period);
                        if !scan.changed_document_ids.is_empty() {
                            on_event(DocumentIndexChangedEvent {
                                phase: DocumentIndexPhase::Processing,
                                document_ids: scan.changed_document_ids,
                                result: None,
                            });
                        }
                    }

                    if scan.pending_count == 0 {
                        quiet_deadline = None;
                        continue;
                    }
                    if quiet_deadline.is_none() {
                        quiet_deadline = Some(Instant::now() + quiet_period);
                    }
                    if quiet_deadline.is_some_and(|deadline| Instant::now() < deadline) {
                        continue;
                    }

                    let result = {
                        let mut service = match service.lock() {
                            Ok(service) => service,
                            Err(_) => break,
                        };
                        service.process_pending_external_changes()
                    };
                    let Ok(result) = result else {
                        quiet_deadline = None;
                        continue;
                    };
                    quiet_deadline = None;
                    on_event(DocumentIndexChangedEvent {
                        phase: DocumentIndexPhase::Completed,
                        document_ids: Vec::new(),
                        result: Some(result),
                    });
                }
            })?;

        Ok(Self {
            stop,
            handle: Some(handle),
        })
    }
}

impl Drop for ExternalChangeMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            handle.thread().unpark();
            let _ = handle.join();
        }
    }
}

#[derive(Clone)]
struct PreparedSource {
    path: PathBuf,
    source_path: String,
    source_identifier: String,
    file_name: String,
    file_type: String,
    file_size: i64,
}

enum ScanEntry {
    File(PathBuf),
    Ignored {
        source_path: String,
    },
    Failed {
        source_path: String,
        message: String,
    },
}

impl ScanEntry {
    fn source_path(&self) -> String {
        match self {
            Self::File(path) => path.to_string_lossy().into_owned(),
            Self::Ignored { source_path } | Self::Failed { source_path, .. } => source_path.clone(),
        }
    }

    fn file_name(&self) -> Option<String> {
        match self {
            Self::File(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned()),
            Self::Ignored { source_path } | Self::Failed { source_path, .. } => {
                Path::new(source_path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            }
        }
    }
}

impl LibraryService {
    pub fn new(state_dir: impl Into<PathBuf>) -> LibraryResult<Self> {
        let state_dir = state_dir.into();
        fs::create_dir_all(&state_dir)?;
        let recent = load_recent(&state_dir)?;

        Ok(Self {
            state_dir,
            current: None,
            recent,
            import_items: HashMap::new(),
        })
    }

    pub fn bootstrap(&mut self) -> LibraryResult<BootstrapState> {
        if self.current.is_none() {
            if let Some(path) = self.recent.first().map(|entry| entry.path.clone()) {
                if is_library_directory(Path::new(&path)) {
                    self.open_library(&path)?;
                }
            }
        }

        Ok(self.snapshot())
    }

    pub fn inspect_location(
        &self,
        path: impl AsRef<Path>,
    ) -> LibraryResult<LibraryLocationInspection> {
        let path = normalize_path(path.as_ref())?;
        let display_path = path.to_string_lossy().into_owned();
        let cloud_sync_warning = cloud_sync_warning(&path);

        if let Some(reason) = reserved_location_reason(&path) {
            return Ok(LibraryLocationInspection {
                path: display_path,
                status: LocationStatus::Blocked,
                is_existing_library: false,
                cloud_sync_warning,
                reason: Some(reason),
            });
        }

        if path.exists() && !path.is_dir() {
            return Ok(LibraryLocationInspection {
                path: display_path,
                status: LocationStatus::Blocked,
                is_existing_library: false,
                cloud_sync_warning,
                reason: Some("所选位置不是目录。".to_string()),
            });
        }

        if is_library_directory(&path) {
            if let Err(reason) = ensure_writable(&path) {
                return Ok(LibraryLocationInspection {
                    path: display_path,
                    status: LocationStatus::Blocked,
                    is_existing_library: true,
                    cloud_sync_warning,
                    reason: Some(reason),
                });
            }

            return Ok(LibraryLocationInspection {
                path: display_path,
                status: LocationStatus::ExistingLibrary,
                is_existing_library: true,
                cloud_sync_warning,
                reason: None,
            });
        }

        if path.exists() {
            let mut entries = fs::read_dir(&path)?;
            if entries.next().transpose()?.is_some() {
                return Ok(LibraryLocationInspection {
                    path: display_path,
                    status: LocationStatus::Blocked,
                    is_existing_library: false,
                    cloud_sync_warning,
                    reason: Some("目录不为空。请选择一个空目录或已有资料库。".to_string()),
                });
            }
        }

        if let Err(reason) = ensure_writable_for_candidate(&path) {
            return Ok(LibraryLocationInspection {
                path: display_path,
                status: LocationStatus::Blocked,
                is_existing_library: false,
                cloud_sync_warning,
                reason: Some(reason),
            });
        }

        Ok(LibraryLocationInspection {
            path: display_path,
            status: LocationStatus::Usable,
            is_existing_library: false,
            cloud_sync_warning,
            reason: None,
        })
    }

    pub fn create_library(&mut self, path: impl AsRef<Path>) -> LibraryResult<LibrarySummary> {
        let inspection = self.inspect_location(path)?;
        let path = PathBuf::from(&inspection.path);

        match inspection.status {
            LocationStatus::ExistingLibrary => {
                return Err(LibraryError::InvalidLocation(
                    "该位置已经包含资料库，请直接打开。".to_string(),
                ));
            }
            LocationStatus::Blocked => {
                return Err(LibraryError::InvalidLocation(
                    inspection
                        .reason
                        .unwrap_or_else(|| "该位置不能用作资料库。".to_string()),
                ));
            }
            LocationStatus::Usable => {}
        }

        let root_created = !path.exists();
        fs::create_dir_all(&path)?;

        let metadata = LibraryMetadata {
            format_version: FORMAT_VERSION,
            library_id: Uuid::new_v4().to_string(),
            name: directory_name(&path),
            created_at: now(),
        };

        if let Err(error) = initialize_library(&path, &metadata) {
            cleanup_failed_creation(&path, root_created);
            return Err(error);
        }

        let summary = summary_from_metadata(&path, &metadata);
        self.set_current(summary.clone())?;
        self.record_recent(&summary)?;
        Ok(summary)
    }

    pub fn open_library(&mut self, path: impl AsRef<Path>) -> LibraryResult<LibrarySummary> {
        let inspection = self.inspect_location(path)?;
        if inspection.status != LocationStatus::ExistingLibrary {
            let reason = inspection
                .reason
                .unwrap_or_else(|| "所选位置不包含可打开的资料库。".to_string());
            return Err(LibraryError::InvalidLibrary(reason));
        }

        let path = PathBuf::from(inspection.path);
        let metadata = read_metadata(&path)?;
        if metadata.format_version != FORMAT_VERSION {
            return Err(LibraryError::InvalidLibrary(format!(
                "不支持资料库格式版本 {}。",
                metadata.format_version
            )));
        }

        let connection = open_database(&path)?;
        initialize_schema(&connection)?;
        cleanup_delete_tombstones(&path, &connection)?;

        let summary = summary_from_metadata(&path, &metadata);
        self.current = Some(OpenLibrary {
            summary: summary.clone(),
            connection,
        });
        self.record_recent(&summary)?;
        self.scan_external_changes(true)?;
        Ok(summary)
    }

    /// 02 的单文件导入原语，不做重复决策。面向用户的导入必须使用 `start_import`。
    pub fn import_document(
        &mut self,
        source_path: impl AsRef<Path>,
    ) -> LibraryResult<DocumentSummary> {
        let source_path = normalize_source_file(source_path.as_ref())?;
        let source_metadata = fs::metadata(&source_path)?;
        if !source_metadata.is_file() {
            return Err(LibraryError::ImportFile(format!(
                "只能导入文件：{}",
                source_path.display()
            )));
        }

        let file_name = source_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| LibraryError::ImportFile("源文件缺少文件名。".to_string()))?;
        let capability = capability_for_path(&source_path)
            .ok_or_else(|| LibraryError::UnsupportedFile(unsupported_message()))?;
        let file_type = capability.display_type.as_str();
        let file_size = i64::try_from(source_metadata.len())
            .map_err(|_| LibraryError::ImportFile("文件大小超出支持范围。".to_string()))?;
        if let Err(message) = validate_file_content(&source_path, capability) {
            return Err(LibraryError::ImportFile(message));
        }
        let title = source_path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .filter(|stem| !stem.is_empty())
            .unwrap_or_else(|| file_name.clone());

        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        let library_root = PathBuf::from(&library.summary.path);
        let document_id = Uuid::new_v4().to_string();
        let relative_directory = PathBuf::from(DOCUMENTS_DIR).join(&document_id);
        let destination_directory = library_root.join(&relative_directory);
        fs::create_dir_all(&destination_directory)?;

        let destination_path = destination_directory.join(&file_name);
        let temporary_path = destination_directory.join(format!(".{}.importing", Uuid::new_v4()));
        if let Err(error) = fs::copy(&source_path, &temporary_path) {
            let _ = fs::remove_file(&temporary_path);
            return Err(LibraryError::Io(error));
        }
        if let Err(error) = fs::rename(&temporary_path, &destination_path) {
            let _ = fs::remove_file(&temporary_path);
            return Err(LibraryError::Io(error));
        }

        let imported_at = now();
        let source_path_text = source_path.to_string_lossy().into_owned();
        let source_identifier = source_path_text.to_ascii_lowercase();
        let library_path = relative_directory
            .join(&file_name)
            .to_string_lossy()
            .replace('\\', "/");
        let file_modified_at = fs::metadata(&destination_path)
            .ok()
            .and_then(|metadata| modified_at(&metadata));
        let hash_result = sha256_file(&destination_path);
        let (content_hash, processing_status, index_status, error_stage, error_message) =
            match hash_result {
                Ok(hash) => (
                    Some(hash),
                    DocumentProcessingStatus::Ready,
                    IndexStatus::Pending,
                    None,
                    None,
                ),
                Err(error) => (
                    None,
                    DocumentProcessingStatus::Failed,
                    IndexStatus::Failed,
                    Some("hashing".to_string()),
                    Some(format!("无法计算文件哈希：{error}")),
                ),
            };

        let document = DocumentSummary {
            id: document_id.clone(),
            title,
            description: None,
            document_date: None,
            file_name,
            file_type: file_type.to_string(),
            file_size,
            content_hash,
            collection_id: "inbox".to_string(),
            tags: Vec::new(),
            processing_status,
            index_status,
            error_stage,
            error_message,
            imported_at: imported_at.clone(),
            source_path: source_path_text,
            source_identifier,
            last_imported_at: imported_at.clone(),
        };

        library.connection.execute(
            "
            INSERT INTO documents (
                id, title, collection_id, file_name, file_type, file_size,
                content_hash, library_path, processing_status, index_status,
                error_stage, error_message, file_modified_at, imported_at, created_at, updated_at
            )
            VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14, ?14
            )
            ",
            params![
                document.id,
                document.title,
                document.collection_id,
                document.file_name,
                document.file_type,
                document.file_size,
                document.content_hash,
                library_path,
                document.processing_status.as_str(),
                document.index_status.as_str(),
                document.error_stage,
                document.error_message,
                file_modified_at,
                imported_at,
            ],
        )?;

        if let Err(error) = library.connection.execute(
            "
            INSERT INTO sources (
                id, document_id, source_path, source_identifier, last_imported_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5)
            ",
            params![
                Uuid::new_v4().to_string(),
                document.id,
                document.source_path,
                document.source_identifier,
                document.last_imported_at,
            ],
        ) {
            let message = format!("无法记录来源信息：{error}");
            library.connection.execute(
                "
                UPDATE documents
                SET processing_status = 'failed',
                    index_status = 'failed',
                    error_stage = 'source',
                    error_message = ?2,
                    updated_at = ?3
                WHERE id = ?1
                ",
                params![document.id, message, now()],
            )?;
            return Ok(DocumentSummary {
                processing_status: DocumentProcessingStatus::Failed,
                index_status: IndexStatus::Failed,
                error_stage: Some("source".to_string()),
                error_message: Some(message),
                ..document
            });
        }

        Ok(document)
    }

    pub fn start_import(&mut self, paths: Vec<String>) -> LibraryResult<ImportBatch> {
        self.start_import_with_progress(paths, |_| {})
    }

    pub fn start_import_with_progress<F>(
        &mut self,
        paths: Vec<String>,
        on_progress: F,
    ) -> LibraryResult<ImportBatch>
    where
        F: FnMut(ImportProgress),
    {
        self.start_import_to_collection_with_progress(paths, None, on_progress)
    }

    pub fn start_import_to_collection(
        &mut self,
        paths: Vec<String>,
        target_collection_id: Option<String>,
    ) -> LibraryResult<ImportBatch> {
        self.start_import_to_collection_with_progress(paths, target_collection_id, |_| {})
    }

    pub fn start_import_to_collection_with_progress<F>(
        &mut self,
        paths: Vec<String>,
        target_collection_id: Option<String>,
        mut on_progress: F,
    ) -> LibraryResult<ImportBatch>
    where
        F: FnMut(ImportProgress),
    {
        if self.current.is_none() {
            return Err(LibraryError::NoCurrentLibrary);
        }
        if let Some(collection_id) = target_collection_id.as_deref() {
            let library = self
                .current
                .as_ref()
                .ok_or(LibraryError::NoCurrentLibrary)?;
            ensure_collection_exists(&library.connection, collection_id)?;
        }

        let batch_id = Uuid::new_v4().to_string();
        let entries = scan_import_paths(&paths);
        let total = entries.len();
        let first = entries.first();
        on_progress(ImportProgress {
            batch_id: batch_id.clone(),
            total,
            completed: 0,
            current_file_name: first.and_then(ScanEntry::file_name),
            current_source_path: first.map(ScanEntry::source_path),
            item: None,
            finished: false,
        });

        let mut items = Vec::with_capacity(total);
        for (index, entry) in entries.into_iter().enumerate() {
            let current_file_name = entry.file_name();
            let current_source_path = entry.source_path();
            on_progress(ImportProgress {
                batch_id: batch_id.clone(),
                total,
                completed: index,
                current_file_name: current_file_name.clone(),
                current_source_path: Some(current_source_path.clone()),
                item: None,
                finished: false,
            });

            let item = match entry {
                ScanEntry::File(path) => self.process_import_file_with_target(
                    &path,
                    false,
                    None,
                    target_collection_id.clone(),
                )?,
                ScanEntry::Ignored { source_path } => {
                    let file_name = display_file_name(&source_path);
                    ImportItemResult {
                        item_id: Uuid::new_v4().to_string(),
                        source_path,
                        file_name,
                        file_type: None,
                        status: ImportItemStatus::Ignored,
                        document_id: None,
                        duplicate_document_id: None,
                        error_stage: None,
                        error_message: None,
                        retryable: false,
                        target_collection_id: target_collection_id.clone(),
                        collection_id: None,
                        notice: None,
                    }
                }
                ScanEntry::Failed {
                    source_path,
                    message,
                } => {
                    let mut item = self.failure_item(
                        Uuid::new_v4().to_string(),
                        source_path,
                        None,
                        None,
                        "scan",
                        message,
                        true,
                    );
                    item.target_collection_id = target_collection_id.clone();
                    self.remember_import_target(&item.item_id, &target_collection_id);
                    item
                }
            };

            on_progress(ImportProgress {
                batch_id: batch_id.clone(),
                total,
                completed: index + 1,
                current_file_name,
                current_source_path: Some(current_source_path),
                item: Some(item.clone()),
                finished: false,
            });
            items.push(item);
        }

        on_progress(ImportProgress {
            batch_id: batch_id.clone(),
            total,
            completed: total,
            current_file_name: None,
            current_source_path: None,
            item: None,
            finished: true,
        });

        let imported_count = count_items(&items, ImportItemStatus::Imported);
        let duplicate_count = count_items(&items, ImportItemStatus::Duplicate);
        let source_changed_count = count_items(&items, ImportItemStatus::SourceChanged);
        let failed_count = count_items(&items, ImportItemStatus::Failed);
        let ignored_count = count_items(&items, ImportItemStatus::Ignored);

        Ok(ImportBatch {
            batch_id,
            items,
            imported_count,
            duplicate_count,
            source_changed_count,
            failed_count,
            ignored_count,
            target_collection_id,
        })
    }

    pub fn resolve_import_item(
        &mut self,
        item_id: &str,
        decision: ImportDecision,
    ) -> LibraryResult<ImportItemResult> {
        let context = self
            .import_items
            .remove(item_id)
            .ok_or_else(|| import_item_not_found(item_id))?;
        let status = context.item.status;

        match (status, decision) {
            (ImportItemStatus::Duplicate, ImportDecision::UseExisting) => {
                let duplicate_document_id =
                    context.item.duplicate_document_id.clone().ok_or_else(|| {
                        LibraryError::ImportFile("重复导入项缺少已有文档标识。".to_string())
                    })?;
                Ok(ImportItemResult {
                    status: ImportItemStatus::Skipped,
                    document_id: Some(duplicate_document_id),
                    retryable: false,
                    ..context.item
                })
            }
            (ImportItemStatus::Duplicate, ImportDecision::ImportAnyway) => {
                let pending = context.pending.ok_or_else(|| {
                    LibraryError::ImportFile("重复导入项缺少待处理上下文。".to_string())
                })?;
                self.create_new_document(item_id.to_string(), pending)
            }
            (ImportItemStatus::Duplicate, ImportDecision::Cancel)
            | (ImportItemStatus::SourceChanged, ImportDecision::Cancel) => Ok(ImportItemResult {
                status: ImportItemStatus::Skipped,
                document_id: None,
                retryable: false,
                ..context.item
            }),
            (ImportItemStatus::SourceChanged, ImportDecision::CreateNew) => {
                let pending = context.pending.ok_or_else(|| {
                    LibraryError::ImportFile("来源变化导入项缺少待处理上下文。".to_string())
                })?;
                self.create_new_document(item_id.to_string(), pending)
            }
            (ImportItemStatus::SourceChanged, ImportDecision::ReplaceExisting) => {
                let pending = context.pending.ok_or_else(|| {
                    LibraryError::ImportFile("来源变化导入项缺少待处理上下文。".to_string())
                })?;
                self.replace_existing_document(item_id.to_string(), pending)
            }
            _ => {
                self.import_items.insert(item_id.to_string(), context);
                Err(LibraryError::InvalidImportDecision(format!(
                    "导入项状态 {status:?} 与决策 {decision:?} 不匹配。"
                )))
            }
        }
    }

    pub fn retry_import_item(&mut self, item_id: &str) -> LibraryResult<ImportItemResult> {
        if self.current.is_none() {
            return Err(LibraryError::NoCurrentLibrary);
        }

        let context = self
            .import_items
            .remove(item_id)
            .ok_or_else(|| import_item_not_found(item_id))?;
        if context.item.status != ImportItemStatus::Failed || !context.item.retryable {
            self.import_items.insert(item_id.to_string(), context);
            return Err(LibraryError::InvalidImportDecision(
                "只有失败的导入项可以重试。".to_string(),
            ));
        }

        let target_collection_id = context.target_collection_id.clone();
        self.process_import_file_with_target(
            Path::new(&context.item.source_path),
            false,
            Some(item_id.to_string()),
            target_collection_id,
        )
    }

    fn process_import_file_with_target(
        &mut self,
        source_path: &Path,
        force_import: bool,
        existing_item_id: Option<String>,
        target_collection_id: Option<String>,
    ) -> LibraryResult<ImportItemResult> {
        let mut item = self.process_import_file_impl(
            source_path,
            force_import,
            existing_item_id,
            target_collection_id.clone(),
        )?;
        if item.target_collection_id.is_none() {
            item.target_collection_id = target_collection_id.clone();
        }
        self.remember_import_target(&item.item_id, &target_collection_id);
        Ok(item)
    }

    fn process_import_file_impl(
        &mut self,
        source_path: &Path,
        force_import: bool,
        existing_item_id: Option<String>,
        target_collection_id: Option<String>,
    ) -> LibraryResult<ImportItemResult> {
        let item_id = existing_item_id.unwrap_or_else(|| Uuid::new_v4().to_string());
        let original_source_path = source_path.to_string_lossy().into_owned();
        let source_path = match normalize_source_file(source_path) {
            Ok(path) => path,
            Err(error) => {
                return Ok(self.failure_item(
                    item_id,
                    original_source_path,
                    None,
                    None,
                    "scan",
                    error.to_string(),
                    true,
                ));
            }
        };
        let source_path_text = source_path.to_string_lossy().into_owned();
        let source_metadata = match fs::metadata(&source_path) {
            Ok(metadata) => metadata,
            Err(error) => {
                return Ok(self.failure_item(
                    item_id,
                    source_path_text,
                    None,
                    None,
                    "scan",
                    format!("无法读取源文件：{error}"),
                    true,
                ));
            }
        };
        if !source_metadata.is_file() {
            return Ok(self.failure_item(
                item_id,
                source_path_text,
                None,
                None,
                "scan",
                "只能导入文件或文件夹。".to_string(),
                true,
            ));
        }

        let file_name = match source_path.file_name() {
            Some(name) if !name.is_empty() => name.to_string_lossy().into_owned(),
            _ => {
                return Ok(self.failure_item(
                    item_id,
                    source_path_text,
                    None,
                    None,
                    "scan",
                    "源文件缺少文件名。".to_string(),
                    true,
                ));
            }
        };
        let Some(capability) = capability_for_path(&source_path) else {
            return Ok(self.failure_item(
                item_id,
                source_path_text,
                Some(file_name),
                None,
                "scan",
                unsupported_message(),
                true,
            ));
        };
        let file_type = capability.display_type.clone();
        let file_size = match i64::try_from(source_metadata.len()) {
            Ok(size) => size,
            Err(_) => {
                return Ok(self.failure_item(
                    item_id,
                    source_path_text,
                    Some(file_name),
                    Some(file_type.clone()),
                    "scan",
                    "文件大小超出支持范围。".to_string(),
                    true,
                ));
            }
        };
        if let Err(message) = validate_file_content(&source_path, capability) {
            return Ok(self.failure_item(
                item_id,
                source_path_text,
                Some(file_name),
                Some(file_type.clone()),
                "validate",
                message,
                true,
            ));
        }
        let content_hash = match sha256_file(&source_path) {
            Ok(hash) => hash,
            Err(error) => {
                return Ok(self.failure_item(
                    item_id,
                    source_path_text,
                    Some(file_name),
                    Some(file_type.clone()),
                    "hash",
                    format!("无法计算文件哈希：{error}"),
                    true,
                ));
            }
        };
        let source_identifier = source_path_text.to_ascii_lowercase();
        let prepared = PreparedSource {
            path: source_path,
            source_path: source_path_text,
            source_identifier,
            file_name,
            file_type: file_type.clone(),
            file_size,
        };

        if !force_import {
            let conflict = {
                let library = self
                    .current
                    .as_ref()
                    .ok_or(LibraryError::NoCurrentLibrary)?;
                match find_source_change(
                    &library.connection,
                    &prepared.source_identifier,
                    &content_hash,
                ) {
                    Ok(Some(document_id)) => {
                        Ok(Some((ImportItemStatus::SourceChanged, document_id)))
                    }
                    Ok(None) => find_duplicate_document(&library.connection, &content_hash).map(
                        |document_id| {
                            document_id
                                .map(|document_id| (ImportItemStatus::Duplicate, document_id))
                        },
                    ),
                    Err(error) => Err(error),
                }
            };

            match conflict {
                Ok(Some((status, existing_document_id))) => {
                    return Ok(self.pending_item(
                        item_id,
                        prepared,
                        content_hash,
                        status,
                        existing_document_id,
                        target_collection_id,
                    ));
                }
                Ok(None) => {}
                Err(error) => {
                    return Ok(self.failure_item(
                        item_id,
                        prepared.source_path,
                        Some(prepared.file_name),
                        Some(prepared.file_type),
                        "database",
                        error.to_string(),
                        true,
                    ));
                }
            }
        }

        self.create_new_document(
            item_id,
            PendingImport {
                path: prepared.path,
                source_path: prepared.source_path,
                file_name: prepared.file_name,
                file_type: prepared.file_type,
                file_size: prepared.file_size,
                source_identifier: prepared.source_identifier,
                content_hash,
                existing_document_id: String::new(),
                target_collection_id,
            },
        )
    }

    fn resolve_target_collection(
        &self,
        target_collection_id: Option<&str>,
    ) -> LibraryResult<(String, Option<String>)> {
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        let Some(target_collection_id) = target_collection_id else {
            return Ok(("inbox".to_string(), None));
        };
        let exists = library.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM collections WHERE id = ?1)",
            params![target_collection_id],
            |row| row.get::<_, bool>(0),
        )?;
        if exists {
            Ok((target_collection_id.to_string(), None))
        } else {
            Ok((
                "inbox".to_string(),
                Some("目标集合已删除，文档已改为导入收件箱。".to_string()),
            ))
        }
    }

    fn remember_import_target(&mut self, item_id: &str, target_collection_id: &Option<String>) {
        if target_collection_id.is_none() {
            return;
        }
        if let Some(context) = self.import_items.get_mut(item_id) {
            context.target_collection_id = target_collection_id.clone();
            context.item.target_collection_id = target_collection_id.clone();
        }
    }

    fn create_new_document(
        &mut self,
        item_id: String,
        pending: PendingImport,
    ) -> LibraryResult<ImportItemResult> {
        let (collection_id, target_notice) =
            self.resolve_target_collection(pending.target_collection_id.as_deref())?;
        let library_root = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?
            .summary
            .path
            .clone();
        let document_id = Uuid::new_v4().to_string();
        let relative_directory = PathBuf::from(DOCUMENTS_DIR).join(&document_id);
        let destination_directory = PathBuf::from(&library_root).join(&relative_directory);
        if let Err(error) = fs::create_dir_all(&destination_directory) {
            return Ok(self.failure_item(
                item_id,
                pending.source_path,
                Some(pending.file_name),
                Some(pending.file_type),
                "copy",
                format!("无法创建资料库副本目录：{error}"),
                true,
            ));
        }

        let destination_path = destination_directory.join(&pending.file_name);
        let temporary_path = destination_directory.join(format!(".{}.importing", Uuid::new_v4()));
        if let Err(error) = fs::copy(&pending.path, &temporary_path) {
            let _ = fs::remove_file(&temporary_path);
            let _ = fs::remove_dir_all(&destination_directory);
            return Ok(self.failure_item(
                item_id,
                pending.source_path,
                Some(pending.file_name),
                Some(pending.file_type),
                "copy",
                format!("无法复制源文件：{error}"),
                true,
            ));
        }
        if let Err(error) = fs::rename(&temporary_path, &destination_path) {
            let _ = fs::remove_file(&temporary_path);
            let _ = fs::remove_dir_all(&destination_directory);
            return Ok(self.failure_item(
                item_id,
                pending.source_path,
                Some(pending.file_name),
                Some(pending.file_type),
                "copy",
                format!("无法保存资料库副本：{error}"),
                true,
            ));
        }

        let copied_hash = match sha256_file(&destination_path) {
            Ok(hash) => hash,
            Err(error) => {
                let _ = fs::remove_dir_all(&destination_directory);
                return Ok(self.failure_item(
                    item_id,
                    pending.source_path,
                    Some(pending.file_name),
                    Some(pending.file_type),
                    "hash",
                    format!("无法计算资料库副本哈希：{error}"),
                    true,
                ));
            }
        };
        if copied_hash != pending.content_hash {
            let _ = fs::remove_dir_all(&destination_directory);
            return Ok(self.failure_item(
                item_id,
                pending.source_path,
                Some(pending.file_name),
                Some(pending.file_type),
                "hash",
                "源文件在导入过程中发生变化，请重试。".to_string(),
                true,
            ));
        }

        let imported_at = now();
        let library_path = relative_directory
            .join(&pending.file_name)
            .to_string_lossy()
            .replace('\\', "/");
        let file_modified_at = fs::metadata(&destination_path)
            .ok()
            .and_then(|metadata| modified_at(&metadata));
        let document = DocumentSummary {
            id: document_id.clone(),
            title: pending
                .path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .filter(|stem| !stem.is_empty())
                .unwrap_or_else(|| pending.file_name.clone()),
            description: None,
            document_date: None,
            file_name: pending.file_name.clone(),
            file_type: pending.file_type.clone(),
            file_size: pending.file_size,
            content_hash: Some(copied_hash),
            collection_id,
            tags: Vec::new(),
            processing_status: DocumentProcessingStatus::Ready,
            index_status: IndexStatus::Pending,
            error_stage: None,
            error_message: None,
            imported_at: imported_at.clone(),
            source_path: pending.source_path.clone(),
            source_identifier: pending.source_identifier.clone(),
            last_imported_at: imported_at.clone(),
        };

        let database_result = (|| -> Result<(), (&'static str, String)> {
            let library = self
                .current
                .as_ref()
                .ok_or(("database", "请先打开资料库。".to_string()))?;
            let transaction = library
                .connection
                .unchecked_transaction()
                .map_err(|error| ("database", error.to_string()))?;
            transaction
                .execute(
                    "
                    INSERT INTO documents (
                        id, title, collection_id, file_name, file_type, file_size,
                        content_hash, library_path, processing_status, index_status,
                        error_stage, error_message, file_modified_at, imported_at, created_at, updated_at
                    )
                    VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                        NULL, NULL, ?11, ?12, ?12, ?12
                    )
                    ",
                    params![
                        &document.id,
                        &document.title,
                        &document.collection_id,
                        &document.file_name,
                        &document.file_type,
                        document.file_size,
                        document.content_hash.as_deref(),
                        &library_path,
                        document.processing_status.as_str(),
                        document.index_status.as_str(),
                        file_modified_at,
                        &imported_at,
                    ],
                )
                .map_err(|error| ("database", error.to_string()))?;
            transaction
                .execute(
                    "
                    INSERT INTO sources (
                        id, document_id, source_path, source_identifier, last_imported_at
                    )
                    VALUES (?1, ?2, ?3, ?4, ?5)
                    ",
                    params![
                        Uuid::new_v4().to_string(),
                        &document.id,
                        &document.source_path,
                        &document.source_identifier,
                        &document.last_imported_at,
                    ],
                )
                .map_err(|error| ("source", format!("无法记录来源信息：{error}")))?;
            transaction
                .commit()
                .map_err(|error| ("database", error.to_string()))?;
            Ok(())
        })();

        if let Err((stage, message)) = database_result {
            let _ = fs::remove_dir_all(&destination_directory);
            return Ok(self.failure_item(
                item_id,
                pending.source_path,
                Some(pending.file_name),
                Some(pending.file_type),
                stage,
                message,
                true,
            ));
        }

        Ok(ImportItemResult {
            item_id,
            source_path: document.source_path,
            file_name: document.file_name,
            file_type: Some(document.file_type),
            status: ImportItemStatus::Imported,
            document_id: Some(document.id),
            duplicate_document_id: None,
            error_stage: None,
            error_message: None,
            retryable: false,
            target_collection_id: pending.target_collection_id,
            collection_id: Some(document.collection_id),
            notice: target_notice,
        })
    }

    fn replace_existing_document(
        &mut self,
        item_id: String,
        pending: PendingImport,
    ) -> LibraryResult<ImportItemResult> {
        let source_path = match normalize_source_file(&pending.path) {
            Ok(path) => path,
            Err(error) => {
                return Ok(self.failure_item(
                    item_id,
                    pending.source_path,
                    Some(pending.file_name),
                    Some(pending.file_type),
                    "scan",
                    error.to_string(),
                    true,
                ));
            }
        };
        let source_path_text = source_path.to_string_lossy().into_owned();
        let source_metadata = match fs::metadata(&source_path) {
            Ok(metadata) => metadata,
            Err(error) => {
                return Ok(self.failure_item(
                    item_id,
                    source_path_text,
                    Some(pending.file_name),
                    Some(pending.file_type),
                    "scan",
                    format!("无法读取源文件：{error}"),
                    true,
                ));
            }
        };
        let file_size = match i64::try_from(source_metadata.len()) {
            Ok(size) => size,
            Err(_) => {
                return Ok(self.failure_item(
                    item_id,
                    source_path_text,
                    Some(pending.file_name),
                    Some(pending.file_type),
                    "scan",
                    "文件大小超出支持范围。".to_string(),
                    true,
                ));
            }
        };
        let source_hash = match sha256_file(&source_path) {
            Ok(hash) => hash,
            Err(error) => {
                return Ok(self.failure_item(
                    item_id,
                    source_path_text,
                    Some(pending.file_name),
                    Some(pending.file_type),
                    "hash",
                    format!("无法计算文件哈希：{error}"),
                    true,
                ));
            }
        };

        let existing = {
            let library = self
                .current
                .as_ref()
                .ok_or(LibraryError::NoCurrentLibrary)?;
            library
                .connection
                .query_row(
                    "
                    SELECT library_path
                    FROM documents
                    WHERE id = ?1 AND deleted_at IS NULL
                    ",
                    params![&pending.existing_document_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
        };
        let old_library_path = match existing {
            Ok(Some(path)) => path,
            Ok(None) => {
                return Err(LibraryError::ImportFile(
                    "要替换的文档不存在或已删除。".to_string(),
                ));
            }
            Err(error) => {
                return Ok(self.failure_item(
                    item_id,
                    source_path_text,
                    Some(pending.file_name),
                    Some(pending.file_type),
                    "database",
                    error.to_string(),
                    true,
                ));
            }
        };

        let library_root = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?
            .summary
            .path
            .clone();
        let document_directory_relative =
            PathBuf::from(DOCUMENTS_DIR).join(&pending.existing_document_id);
        let document_directory = PathBuf::from(&library_root).join(&document_directory_relative);
        if let Err(error) = fs::create_dir_all(&document_directory) {
            return Ok(self.failure_item(
                item_id,
                source_path_text,
                Some(pending.file_name),
                Some(pending.file_type),
                "copy",
                format!("无法创建替换副本目录：{error}"),
                true,
            ));
        }
        let destination_path = document_directory.join(&pending.file_name);
        let temporary_path = document_directory.join(format!(".{}.importing", Uuid::new_v4()));
        if let Err(error) = fs::copy(&source_path, &temporary_path) {
            let _ = fs::remove_file(&temporary_path);
            return Ok(self.failure_item(
                item_id,
                source_path_text,
                Some(pending.file_name),
                Some(pending.file_type),
                "copy",
                format!("无法复制源文件：{error}"),
                true,
            ));
        }
        let copied_hash = match sha256_file(&temporary_path) {
            Ok(hash) => hash,
            Err(error) => {
                let _ = fs::remove_file(&temporary_path);
                return Ok(self.failure_item(
                    item_id,
                    source_path_text,
                    Some(pending.file_name),
                    Some(pending.file_type),
                    "hash",
                    format!("无法计算替换副本哈希：{error}"),
                    true,
                ));
            }
        };
        if copied_hash != source_hash {
            let _ = fs::remove_file(&temporary_path);
            return Ok(self.failure_item(
                item_id,
                source_path_text,
                Some(pending.file_name),
                Some(pending.file_type),
                "hash",
                "源文件在替换过程中发生变化，请重试。".to_string(),
                true,
            ));
        }

        let old_copy = PathBuf::from(&library_root).join(old_library_path);
        let mut previous_copies = Vec::new();
        if old_copy != destination_path && old_copy.exists() {
            let backup_path = document_directory.join(format!(".{}.previous", Uuid::new_v4()));
            if let Err(error) = fs::rename(&old_copy, &backup_path) {
                let _ = fs::remove_file(&temporary_path);
                return Ok(self.failure_item(
                    item_id,
                    source_path_text,
                    Some(pending.file_name),
                    Some(pending.file_type),
                    "copy",
                    format!("无法备份旧资料库副本：{error}"),
                    true,
                ));
            }
            previous_copies.push((backup_path, old_copy));
        }
        if destination_path.exists() {
            let backup_path = document_directory.join(format!(".{}.previous", Uuid::new_v4()));
            if let Err(error) = fs::rename(&destination_path, &backup_path) {
                let _ = fs::remove_file(&temporary_path);
                restore_previous_copies(&previous_copies);
                return Ok(self.failure_item(
                    item_id,
                    source_path_text,
                    Some(pending.file_name),
                    Some(pending.file_type),
                    "copy",
                    format!("无法备份现有资料库副本：{error}"),
                    true,
                ));
            }
            previous_copies.push((backup_path, destination_path.clone()));
        }
        if let Err(error) = fs::rename(&temporary_path, &destination_path) {
            let _ = fs::remove_file(&temporary_path);
            restore_previous_copies(&previous_copies);
            return Ok(self.failure_item(
                item_id,
                source_path_text,
                Some(pending.file_name),
                Some(pending.file_type),
                "copy",
                format!("无法保存替换副本：{error}"),
                true,
            ));
        }

        let last_imported_at = now();
        let library_path = document_directory_relative
            .join(&pending.file_name)
            .to_string_lossy()
            .replace('\\', "/");
        let file_modified_at = fs::metadata(&destination_path)
            .ok()
            .and_then(|metadata| modified_at(&metadata));
        let database_result = (|| -> Result<(), (&'static str, String)> {
            let library = self
                .current
                .as_ref()
                .ok_or(("database", "请先打开资料库。".to_string()))?;
            let transaction = library
                .connection
                .unchecked_transaction()
                .map_err(|error| ("database", error.to_string()))?;
            let updated = transaction
                .execute(
                    "
                    UPDATE documents
                    SET file_name = ?1,
                        file_type = ?2,
                        file_size = ?3,
                        content_hash = ?4,
                        library_path = ?5,
                        processing_status = 'ready',
                        index_status = 'pending',
                        error_stage = NULL,
                        error_message = NULL,
                        file_modified_at = ?6,
                        updated_at = ?7
                    WHERE id = ?8 AND deleted_at IS NULL
                    ",
                    params![
                        &pending.file_name,
                        &pending.file_type,
                        file_size,
                        &copied_hash,
                        &library_path,
                        file_modified_at,
                        &last_imported_at,
                        &pending.existing_document_id,
                    ],
                )
                .map_err(|error| ("database", error.to_string()))?;
            if updated == 0 {
                return Err(("database", "要替换的文档不存在或已删除。".to_string()));
            }
            transaction
                .execute(
                    "DELETE FROM document_search WHERE document_id = ?1",
                    params![&pending.existing_document_id],
                )
                .map_err(|error| ("database", error.to_string()))?;
            let source_updated = transaction
                .execute(
                    "
                    UPDATE sources
                    SET source_path = ?1,
                        source_identifier = ?2,
                        last_imported_at = ?3
                    WHERE document_id = ?4
                    ",
                    params![
                        &source_path_text,
                        &pending.source_identifier,
                        &last_imported_at,
                        &pending.existing_document_id,
                    ],
                )
                .map_err(|error| ("source", format!("无法更新来源信息：{error}")))?;
            if source_updated == 0 {
                transaction
                    .execute(
                        "
                        INSERT INTO sources (
                            id, document_id, source_path, source_identifier, last_imported_at
                        )
                        VALUES (?1, ?2, ?3, ?4, ?5)
                        ",
                        params![
                            Uuid::new_v4().to_string(),
                            &pending.existing_document_id,
                            &source_path_text,
                            &pending.source_identifier,
                            &last_imported_at,
                        ],
                    )
                    .map_err(|error| ("source", format!("无法记录来源信息：{error}")))?;
            }
            transaction
                .commit()
                .map_err(|error| ("database", error.to_string()))?;
            Ok(())
        })();

        if let Err((stage, message)) = database_result {
            let _ = fs::remove_file(&destination_path);
            restore_previous_copies(&previous_copies);
            return Ok(self.failure_item(
                item_id,
                source_path_text,
                Some(pending.file_name),
                Some(pending.file_type),
                stage,
                message,
                true,
            ));
        }

        remove_previous_copies(&previous_copies);

        let document = self.load_document_summary(&pending.existing_document_id)?;
        Ok(ImportItemResult {
            item_id,
            source_path: source_path_text,
            file_name: document.file_name,
            file_type: Some(document.file_type),
            status: ImportItemStatus::Imported,
            document_id: Some(document.id),
            duplicate_document_id: None,
            error_stage: None,
            error_message: None,
            retryable: false,
            target_collection_id: pending.target_collection_id,
            collection_id: Some(document.collection_id),
            notice: None,
        })
    }

    fn pending_item(
        &mut self,
        item_id: String,
        prepared: PreparedSource,
        content_hash: String,
        status: ImportItemStatus,
        existing_document_id: String,
        target_collection_id: Option<String>,
    ) -> ImportItemResult {
        let item = ImportItemResult {
            item_id: item_id.clone(),
            source_path: prepared.source_path.clone(),
            file_name: prepared.file_name.clone(),
            file_type: Some(prepared.file_type.clone()),
            status,
            document_id: None,
            duplicate_document_id: Some(existing_document_id.clone()),
            error_stage: None,
            error_message: None,
            retryable: false,
            target_collection_id: target_collection_id.clone(),
            collection_id: None,
            notice: None,
        };
        self.import_items.insert(
            item_id,
            ImportItemContext {
                item: item.clone(),
                target_collection_id: target_collection_id.clone(),
                pending: Some(PendingImport {
                    path: prepared.path,
                    source_path: prepared.source_path,
                    file_name: prepared.file_name,
                    file_type: prepared.file_type,
                    file_size: prepared.file_size,
                    source_identifier: prepared.source_identifier,
                    content_hash,
                    existing_document_id,
                    target_collection_id,
                }),
            },
        );
        item
    }

    #[allow(clippy::too_many_arguments)]
    fn failure_item(
        &mut self,
        item_id: String,
        source_path: String,
        file_name: Option<String>,
        file_type: Option<String>,
        error_stage: &str,
        error_message: String,
        retryable: bool,
    ) -> ImportItemResult {
        let file_name = file_name.unwrap_or_else(|| display_file_name(&source_path));
        let item = ImportItemResult {
            item_id: item_id.clone(),
            source_path,
            file_name,
            file_type,
            status: ImportItemStatus::Failed,
            document_id: None,
            duplicate_document_id: None,
            error_stage: Some(error_stage.to_string()),
            error_message: Some(error_message),
            retryable,
            target_collection_id: None,
            collection_id: None,
            notice: None,
        };
        self.import_items.insert(
            item_id,
            ImportItemContext {
                item: item.clone(),
                pending: None,
                target_collection_id: None,
            },
        );
        item
    }

    pub fn list_collections(&self) -> LibraryResult<Vec<CollectionSummary>> {
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        let mut statement = library.connection.prepare(
            "
            SELECT
                c.id,
                c.name,
                c.parent_id,
                c.is_inbox,
                (
                    SELECT COUNT(*)
                    FROM documents d
                    WHERE d.collection_id = c.id AND d.deleted_at IS NULL
                )
            FROM collections c
            ORDER BY CASE WHEN c.is_inbox = 1 THEN 0 ELSE 1 END,
                     LOWER(c.name),
                     c.id
            ",
        )?;
        let collections = statement
            .query_map([], collection_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(collections)
    }

    pub fn create_collection(
        &mut self,
        name: String,
        parent_id: Option<String>,
    ) -> LibraryResult<CollectionSummary> {
        let name = validate_collection_name(&name)?.to_string();
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        if let Some(parent_id) = parent_id.as_deref() {
            ensure_collection_exists(&library.connection, parent_id)?;
        }

        let id = Uuid::new_v4().to_string();
        let timestamp = now();
        library.connection.execute(
            "
            INSERT INTO collections (id, name, parent_id, is_inbox, created_at, updated_at)
            VALUES (?1, ?2, ?3, 0, ?4, ?4)
            ",
            params![&id, &name, parent_id.as_deref(), &timestamp],
        )?;
        load_collection_summary(&library.connection, &id)
    }

    pub fn rename_collection(
        &mut self,
        collection_id: &str,
        name: String,
    ) -> LibraryResult<CollectionSummary> {
        let collection = self.load_collection_summary(collection_id)?;
        if collection.is_inbox {
            return Err(LibraryError::ProtectedCollection(
                "收件箱不能重命名。".to_string(),
            ));
        }
        let name = validate_collection_name(&name)?.to_string();
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        library.connection.execute(
            "UPDATE collections SET name = ?1, updated_at = ?2 WHERE id = ?3",
            params![&name, now(), collection_id],
        )?;
        load_collection_summary(&library.connection, collection_id)
    }

    pub fn move_collection(
        &mut self,
        collection_id: &str,
        parent_id: Option<String>,
    ) -> LibraryResult<CollectionSummary> {
        let collection = self.load_collection_summary(collection_id)?;
        if collection.is_inbox {
            return Err(LibraryError::ProtectedCollection(
                "收件箱不能移动。".to_string(),
            ));
        }

        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        if let Some(parent_id) = parent_id.as_deref() {
            ensure_collection_exists(&library.connection, parent_id)?;
            if parent_id == collection_id
                || collection_is_descendant(&library.connection, collection_id, parent_id)?
            {
                return Err(LibraryError::CollectionCycle(
                    "不能把集合移动到自身或其后代下。".to_string(),
                ));
            }
        }

        library.connection.execute(
            "
            UPDATE collections
            SET parent_id = ?1, updated_at = ?2
            WHERE id = ?3
            ",
            params![parent_id.as_deref(), now(), collection_id],
        )?;
        load_collection_summary(&library.connection, collection_id)
    }

    pub fn delete_collection(
        &mut self,
        collection_id: &str,
    ) -> LibraryResult<CollectionDeleteResult> {
        let collection = self.load_collection_summary(collection_id)?;
        if collection.is_inbox {
            return Err(LibraryError::ProtectedCollection(
                "收件箱不能删除。".to_string(),
            ));
        }

        let library = self
            .current
            .as_mut()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        let transaction = library.connection.transaction()?;
        let parent_id = transaction
            .query_row(
                "SELECT parent_id FROM collections WHERE id = ?1",
                params![collection_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .ok_or_else(|| {
                LibraryError::CollectionNotFound(format!("集合不存在：{collection_id}"))
            })?;
        let timestamp = now();
        let moved_document_count = transaction.query_row(
            "
            SELECT COUNT(*)
            FROM documents
            WHERE collection_id = ?1 AND deleted_at IS NULL
            ",
            params![collection_id],
            |row| row.get::<_, i64>(0),
        )?;
        transaction.execute(
            "
            UPDATE documents
            SET collection_id = 'inbox', updated_at = ?1
            WHERE collection_id = ?2
            ",
            params![&timestamp, collection_id],
        )?;
        transaction.execute(
            "
            UPDATE collections
            SET parent_id = ?1, updated_at = ?2
            WHERE parent_id = ?3
            ",
            params![parent_id.as_deref(), &timestamp, collection_id],
        )?;
        transaction.execute(
            "DELETE FROM collections WHERE id = ?1",
            params![collection_id],
        )?;
        transaction.commit()?;

        Ok(CollectionDeleteResult {
            collection_id: collection_id.to_string(),
            target_collection_id: "inbox".to_string(),
            moved_document_count,
        })
    }

    pub fn list_tags(&self) -> LibraryResult<Vec<TagSummary>> {
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        let mut statement = library.connection.prepare(
            "
            SELECT
                t.id,
                t.name,
                (
                    SELECT COUNT(*)
                    FROM document_tags dt
                    JOIN documents d ON d.id = dt.document_id
                    WHERE dt.tag_id = t.id AND d.deleted_at IS NULL
                )
            FROM tags t
            ORDER BY LOWER(t.name), t.id
            ",
        )?;
        let tags = statement
            .query_map([], tag_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(tags)
    }

    pub fn create_tag(&mut self, name: String) -> LibraryResult<TagSummary> {
        let name = validate_tag_name(&name)?;
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        if tag_name_exists(&library.connection, &name, None)? {
            return Err(LibraryError::TagAlreadyExists(format!(
                "标签已存在：{name}"
            )));
        }

        let id = Uuid::new_v4().to_string();
        library.connection.execute(
            "INSERT INTO tags (id, name, created_at) VALUES (?1, ?2, ?3)",
            params![&id, &name, now()],
        )?;
        load_tag_summary(&library.connection, &id)
    }

    pub fn rename_tag(&mut self, tag_id: &str, name: String) -> LibraryResult<TagSummary> {
        let name = validate_tag_name(&name)?;
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        ensure_tag_exists(&library.connection, tag_id)?;
        if tag_name_exists(&library.connection, &name, Some(tag_id))? {
            return Err(LibraryError::TagAlreadyExists(format!(
                "标签已存在：{name}"
            )));
        }

        library.connection.execute(
            "UPDATE tags SET name = ?1 WHERE id = ?2",
            params![&name, tag_id],
        )?;
        load_tag_summary(&library.connection, tag_id)
    }

    pub fn delete_tag(&mut self, tag_id: &str) -> LibraryResult<()> {
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        ensure_tag_exists(&library.connection, tag_id)?;
        library
            .connection
            .execute("DELETE FROM tags WHERE id = ?1", params![tag_id])?;
        Ok(())
    }

    pub fn add_tag_to_document(
        &mut self,
        document_id: &str,
        tag_id: &str,
    ) -> LibraryResult<DocumentSummary> {
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        ensure_document_exists(&library.connection, document_id)?;
        ensure_tag_exists(&library.connection, tag_id)?;
        library.connection.execute(
            "
            INSERT OR IGNORE INTO document_tags (document_id, tag_id)
            VALUES (?1, ?2)
            ",
            params![document_id, tag_id],
        )?;
        library.connection.execute(
            "UPDATE documents SET updated_at = ?1 WHERE id = ?2",
            params![now(), document_id],
        )?;
        load_document_summary(&library.connection, document_id)
    }

    pub fn remove_tag_from_document(
        &mut self,
        document_id: &str,
        tag_id: &str,
    ) -> LibraryResult<DocumentSummary> {
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        ensure_document_exists(&library.connection, document_id)?;
        ensure_tag_exists(&library.connection, tag_id)?;
        library.connection.execute(
            "DELETE FROM document_tags WHERE document_id = ?1 AND tag_id = ?2",
            params![document_id, tag_id],
        )?;
        library.connection.execute(
            "UPDATE documents SET updated_at = ?1 WHERE id = ?2",
            params![now(), document_id],
        )?;
        load_document_summary(&library.connection, document_id)
    }

    pub fn update_document_metadata(
        &mut self,
        document_id: &str,
        update: DocumentMetadataUpdate,
    ) -> LibraryResult<DocumentSummary> {
        let title = validate_document_title(&update.title)?;
        let description = normalize_optional_text(update.description);
        let document_date = normalize_optional_text(update.document_date);
        if let Some(date) = document_date.as_deref() {
            validate_document_date(date)?;
        }

        let mut seen_tag_ids = HashSet::new();
        let tag_ids = update
            .tag_ids
            .into_iter()
            .filter(|tag_id| seen_tag_ids.insert(tag_id.clone()))
            .collect::<Vec<_>>();

        let library = self
            .current
            .as_mut()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        let transaction = library.connection.transaction()?;
        ensure_document_exists(&transaction, document_id)?;
        ensure_collection_exists(&transaction, &update.collection_id)?;
        for tag_id in &tag_ids {
            ensure_tag_exists(&transaction, tag_id)?;
        }
        let extracted_text = transaction
            .query_row(
                "SELECT extracted_text FROM document_search WHERE document_id = ?1",
                params![document_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_default();

        let timestamp = now();
        transaction.execute(
            "
            UPDATE documents
            SET title = ?1,
                description = ?2,
                document_date = ?3,
                collection_id = ?4,
                updated_at = ?5
            WHERE id = ?6 AND deleted_at IS NULL
            ",
            params![
                &title,
                description.as_deref(),
                document_date.as_deref(),
                &update.collection_id,
                &timestamp,
                document_id,
            ],
        )?;
        transaction.execute(
            "DELETE FROM document_tags WHERE document_id = ?1",
            params![document_id],
        )?;
        for tag_id in &tag_ids {
            transaction.execute(
                "INSERT INTO document_tags (document_id, tag_id) VALUES (?1, ?2)",
                params![document_id, tag_id],
            )?;
        }
        transaction.execute(
            "DELETE FROM document_search WHERE document_id = ?1",
            params![document_id],
        )?;
        transaction.execute(
            "
            INSERT INTO document_search (document_id, title, description, extracted_text)
            VALUES (?1, ?2, ?3, ?4)
            ",
            params![document_id, &title, description.as_deref(), &extracted_text],
        )?;
        transaction.commit()?;

        load_document_summary(&library.connection, document_id)
    }

    pub fn batch_organize_documents<F>(
        &mut self,
        request: BatchDocumentOperationRequest,
        mut is_cancelled: F,
    ) -> LibraryResult<BatchDocumentOperationResult>
    where
        F: FnMut() -> bool,
    {
        let operation = request.operation.clone();
        let mut seen = HashSet::new();
        let document_ids = request
            .document_ids
            .into_iter()
            .filter(|document_id| seen.insert(document_id.clone()))
            .collect::<Vec<_>>();
        let mut results = Vec::with_capacity(document_ids.len());

        for document_id in document_ids {
            if is_cancelled() {
                results.push(BatchDocumentItemResult {
                    document_id,
                    status: BatchDocumentItemStatus::Cancelled,
                    error_code: None,
                    error_message: None,
                });
                continue;
            }

            let outcome = match &operation {
                BatchDocumentOperation::MoveToCollection { collection_id } => self
                    .move_document_to_collection(&document_id, collection_id)
                    .map(|_| ()),
                BatchDocumentOperation::AddTag { tag_id } => {
                    self.add_tag_to_document(&document_id, tag_id).map(|_| ())
                }
                BatchDocumentOperation::RemoveTag { tag_id } => self
                    .remove_tag_from_document(&document_id, tag_id)
                    .map(|_| ()),
                BatchDocumentOperation::MoveToTrash => self.move_document_to_trash(&document_id),
            };

            results.push(match outcome {
                Ok(()) => BatchDocumentItemResult {
                    document_id,
                    status: BatchDocumentItemStatus::Succeeded,
                    error_code: None,
                    error_message: None,
                },
                Err(error) => BatchDocumentItemResult {
                    document_id,
                    status: BatchDocumentItemStatus::Failed,
                    error_code: Some(error.code().to_string()),
                    error_message: Some(error.to_string()),
                },
            });
        }

        let succeeded_count = results
            .iter()
            .filter(|result| result.status == BatchDocumentItemStatus::Succeeded)
            .count();
        let failed_count = results
            .iter()
            .filter(|result| result.status == BatchDocumentItemStatus::Failed)
            .count();
        let cancelled_count = results
            .iter()
            .filter(|result| result.status == BatchDocumentItemStatus::Cancelled)
            .count();

        Ok(BatchDocumentOperationResult {
            job_id: request.job_id,
            operation,
            results,
            succeeded_count,
            failed_count,
            cancelled_count,
        })
    }

    pub fn move_document_to_collection(
        &mut self,
        document_id: &str,
        collection_id: &str,
    ) -> LibraryResult<DocumentSummary> {
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        ensure_collection_exists(&library.connection, collection_id)?;
        let updated = library.connection.execute(
            "
            UPDATE documents
            SET collection_id = ?1, updated_at = ?2
            WHERE id = ?3 AND deleted_at IS NULL
            ",
            params![collection_id, now(), document_id],
        )?;
        if updated == 0 {
            return Err(LibraryError::DocumentNotFound(format!(
                "文档不存在或已删除：{document_id}"
            )));
        }
        load_document_summary(&library.connection, document_id)
    }

    pub fn move_document_to_trash(&mut self, document_id: &str) -> LibraryResult<()> {
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        let timestamp = now();
        let updated = library.connection.execute(
            "
            UPDATE documents
            SET original_collection_id = collection_id,
                deleted_at = ?1,
                updated_at = ?1
            WHERE id = ?2 AND deleted_at IS NULL
            ",
            params![&timestamp, document_id],
        )?;
        if updated == 0 {
            return Err(LibraryError::DocumentNotFound(format!(
                "文档不存在或已在回收站中：{document_id}"
            )));
        }
        Ok(())
    }

    pub fn list_trash_documents(&self) -> LibraryResult<Vec<TrashDocumentSummary>> {
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        let mut statement = library.connection.prepare(
            "
            SELECT
                d.id,
                d.title,
                d.file_name,
                d.file_type,
                d.file_size,
                d.content_hash,
                d.collection_id,
                d.processing_status,
                d.index_status,
                d.error_stage,
                d.error_message,
                d.imported_at,
                COALESCE(s.source_path, ''),
                COALESCE(s.source_identifier, ''),
                COALESCE(s.last_imported_at, d.imported_at),
                d.description,
                d.document_date,
                d.original_collection_id,
                c.name,
                d.deleted_at
            FROM documents d
            LEFT JOIN sources s ON s.document_id = d.id
            LEFT JOIN collections c ON c.id = d.original_collection_id
            WHERE d.deleted_at IS NOT NULL
            ORDER BY d.deleted_at DESC, d.id DESC
            ",
        )?;
        let mut documents = statement
            .query_map([], |row| {
                Ok(TrashDocumentSummary {
                    document: document_from_row(row)?,
                    original_collection_id: row.get(17)?,
                    original_collection_name: row.get(18)?,
                    deleted_at: row.get(19)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        let tags = load_all_document_tags(&library.connection)?;
        for document in &mut documents {
            document.document.tags = tags.get(&document.document.id).cloned().unwrap_or_default();
        }
        Ok(documents)
    }

    pub fn restore_document(&mut self, document_id: &str) -> LibraryResult<DocumentSummary> {
        let library = self
            .current
            .as_mut()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        let transaction = library.connection.transaction()?;
        let original_collection_id = transaction
            .query_row(
                "
                SELECT original_collection_id
                FROM documents
                WHERE id = ?1 AND deleted_at IS NOT NULL
                ",
                params![document_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .ok_or_else(|| {
                LibraryError::DocumentNotFound(format!("回收站中不存在文档：{document_id}"))
            })?;
        let collection_id = match original_collection_id {
            Some(collection_id) => {
                let exists = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM collections WHERE id = ?1)",
                    params![&collection_id],
                    |row| row.get::<_, bool>(0),
                )?;
                if exists {
                    collection_id
                } else {
                    "inbox".to_string()
                }
            }
            None => "inbox".to_string(),
        };
        let updated = transaction.execute(
            "
            UPDATE documents
            SET collection_id = ?1,
                original_collection_id = NULL,
                deleted_at = NULL,
                updated_at = ?2
            WHERE id = ?3 AND deleted_at IS NOT NULL
            ",
            params![&collection_id, now(), document_id],
        )?;
        if updated == 0 {
            return Err(LibraryError::DocumentNotFound(format!(
                "回收站中不存在文档：{document_id}"
            )));
        }
        transaction.commit()?;
        load_document_summary(&library.connection, document_id)
    }

    pub fn permanently_delete_document(&mut self, document_id: &str) -> LibraryResult<()> {
        self.permanently_delete_trashed_document(document_id)
    }

    pub fn empty_trash(&mut self) -> LibraryResult<EmptyTrashResult> {
        let documents = {
            let library = self
                .current
                .as_ref()
                .ok_or(LibraryError::NoCurrentLibrary)?;
            let mut statement = library.connection.prepare(
                "
                    SELECT id, file_name
                    FROM documents
                    WHERE deleted_at IS NOT NULL
                    ORDER BY deleted_at, id
                    ",
            )?;
            let documents = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            documents
        };

        let mut deleted_count = 0;
        let mut failed_count = 0;
        let mut items = Vec::with_capacity(documents.len());
        for (document_id, file_name) in documents {
            match self.permanently_delete_trashed_document(&document_id) {
                Ok(()) => {
                    deleted_count += 1;
                    items.push(EmptyTrashItemResult {
                        document_id,
                        file_name,
                        status: EmptyTrashItemStatus::Succeeded,
                        error_code: None,
                        error_message: None,
                    });
                }
                Err(error) => {
                    failed_count += 1;
                    items.push(EmptyTrashItemResult {
                        document_id,
                        file_name,
                        status: EmptyTrashItemStatus::Failed,
                        error_code: Some(error.code().to_string()),
                        error_message: Some(error.to_string()),
                    });
                }
            }
        }
        Ok(EmptyTrashResult {
            deleted_count,
            failed_count,
            items,
        })
    }

    fn permanently_delete_trashed_document(&self, document_id: &str) -> LibraryResult<()> {
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        let stored = library
            .connection
            .query_row(
                "
                SELECT id, file_name, file_type, library_path
                FROM documents
                WHERE id = ?1 AND deleted_at IS NOT NULL
                ",
                params![document_id],
                |row| {
                    Ok(StoredDocumentFile {
                        id: row.get(0)?,
                        file_name: row.get(1)?,
                        file_type: row.get(2)?,
                        library_path: row.get(3)?,
                        content_hash: None,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| {
                LibraryError::DocumentNotFound(format!("回收站中不存在文档：{document_id}"))
            })?;
        let copy_path = self.document_copy_path(&stored)?;
        let tombstone_path = move_library_copy_to_tombstone(&copy_path, DELETE_TOMBSTONE_PREFIX)?;

        let database_result = (|| -> LibraryResult<()> {
            let transaction = library.connection.unchecked_transaction()?;
            transaction.execute(
                "DELETE FROM document_search WHERE document_id = ?1",
                params![document_id],
            )?;
            let deleted = transaction.execute(
                "DELETE FROM documents WHERE id = ?1 AND deleted_at IS NOT NULL",
                params![document_id],
            )?;
            if deleted == 0 {
                return Err(LibraryError::DocumentNotFound(format!(
                    "回收站中不存在文档：{document_id}"
                )));
            }
            transaction.commit()?;
            Ok(())
        })();

        if let Err(error) = database_result {
            if let Some(tombstone_path) = tombstone_path.as_deref() {
                restore_library_copy_tombstone(tombstone_path, &copy_path)?;
            }
            return Err(error);
        }

        if let Some(tombstone_path) = tombstone_path {
            // The database is authoritative once committed. A leftover tombstone
            // is safe and is reconciled the next time the library is opened.
            let _ = remove_library_copy(&tombstone_path);
        }
        remove_thumbnail_versions(Path::new(&library.summary.path), document_id);
        Ok(())
    }

    fn load_collection_summary(&self, collection_id: &str) -> LibraryResult<CollectionSummary> {
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        load_collection_summary(&library.connection, collection_id)
    }

    fn load_document_summary(&self, document_id: &str) -> LibraryResult<DocumentSummary> {
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        load_document_summary(&library.connection, document_id)
    }

    pub fn list_documents(&self) -> LibraryResult<Vec<DocumentSummary>> {
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        let mut statement = library.connection.prepare(
            "
            SELECT
                d.id,
                d.title,
                d.file_name,
                d.file_type,
                d.file_size,
                d.content_hash,
                d.collection_id,
                d.processing_status,
                d.index_status,
                d.error_stage,
                d.error_message,
                d.imported_at,
                COALESCE(s.source_path, ''),
                COALESCE(s.source_identifier, ''),
                COALESCE(s.last_imported_at, d.imported_at),
                d.description,
                d.document_date
            FROM documents d
            LEFT JOIN sources s ON s.document_id = d.id
            WHERE d.deleted_at IS NULL
            ORDER BY d.imported_at DESC, d.id DESC
            ",
        )?;
        let mut documents = statement
            .query_map([], document_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        let tags = load_all_document_tags(&library.connection)?;
        for document in &mut documents {
            document.tags = tags.get(&document.id).cloned().unwrap_or_default();
        }
        Ok(documents)
    }

    pub fn search_documents(
        &self,
        request: DocumentSearchQuery,
    ) -> LibraryResult<DocumentSearchResponse> {
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        let filters = normalize_search_filters(request.filters)?;
        if let Some(collection_id) = filters.collection_id.as_deref() {
            ensure_collection_exists(&library.connection, collection_id)?;
        }
        if let Some(tag_id) = filters.tag_id.as_deref() {
            ensure_tag_exists(&library.connection, tag_id)?;
        }

        let query = request.query.trim();
        let short_query = query.chars().count() <= 2;
        let mut content_matches = HashMap::new();

        if !query.is_empty() && !short_query {
            let fts_query = format!("extracted_text : {}", fts_phrase(query));
            let mut statement = library.connection.prepare(
                "
                SELECT
                    document_id,
                    snippet(document_search, 3, '', '', '…', 36)
                FROM document_search
                WHERE document_search MATCH ?1
                ",
            )?;
            let matches = statement.query_map(params![fts_query], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for result in matches {
                let (document_id, snippet) = result?;
                content_matches.insert(document_id, snippet);
            }
        }

        let documents = load_filtered_documents(&library.connection, &filters)?;
        let collection_names = load_collection_names(&library.connection)?;
        let mut results = Vec::new();
        for document in documents {
            if query.is_empty() {
                results.push(DocumentSearchResult {
                    document,
                    snippet: None,
                    match_kind: SearchMatchKind::Metadata,
                });
                continue;
            }

            let content_snippet = content_matches
                .get(&document.id)
                .filter(|snippet| !snippet.trim().is_empty())
                .cloned();
            if short_query {
                if metadata_matches(&document, query, &collection_names) {
                    results.push(DocumentSearchResult {
                        document,
                        snippet: None,
                        match_kind: SearchMatchKind::Metadata,
                    });
                }
                continue;
            }

            if let Some(snippet) = content_snippet {
                results.push(DocumentSearchResult {
                    document,
                    snippet: Some(snippet),
                    match_kind: SearchMatchKind::Content,
                });
            } else if metadata_matches(&document, query, &collection_names) {
                results.push(DocumentSearchResult {
                    document,
                    snippet: None,
                    match_kind: SearchMatchKind::Metadata,
                });
            }
        }

        Ok(DocumentSearchResponse { results })
    }

    pub fn index_pending_documents(&mut self) -> LibraryResult<IndexRunResult> {
        let mut result = IndexRunResult::default();
        while let Some(document) = self.index_next_pending_document()? {
            result.processed += 1;
            match document.index_status {
                IndexStatus::Searchable => result.searchable += 1,
                IndexStatus::Failed => result.failed += 1,
                IndexStatus::Pending => {}
            }
        }
        Ok(result)
    }

    pub fn pending_index_count(&self) -> LibraryResult<i64> {
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        library
            .connection
            .query_row(
                "
                SELECT COUNT(*)
                FROM documents
                WHERE deleted_at IS NULL AND index_status = 'pending'
                ",
                [],
                |row| row.get(0),
            )
            .map_err(LibraryError::from)
    }

    fn scan_external_changes(
        &mut self,
        verify_content_hashes: bool,
    ) -> LibraryResult<ExternalChangeScan> {
        let Some(library) = self.current.as_ref() else {
            return Ok(ExternalChangeScan::default());
        };
        let library_root = PathBuf::from(&library.summary.path);
        let documents = load_external_document_indexes(&library.connection)?;
        let mut changed_document_ids = Vec::new();
        let mut metadata_updates = Vec::new();

        for document in documents {
            if document.processing_status == DocumentProcessingStatus::Processing
                && document.index_status == IndexStatus::Pending
            {
                continue;
            }
            let path = match document_path(&library_root, &document.library_path) {
                Ok(path) => path,
                Err(_) => {
                    if document.processing_status != DocumentProcessingStatus::Failed
                        || document.index_status != IndexStatus::Failed
                    {
                        changed_document_ids.push(document.id);
                    }
                    continue;
                }
            };
            let fingerprint = file_fingerprint(&path);
            let mut changed = false;

            if !fingerprint.available || !fingerprint.is_file {
                changed = document.processing_status != DocumentProcessingStatus::Failed
                    || document.index_status != IndexStatus::Failed;
            } else if verify_content_hashes
                || fingerprint.size != document.file_size
                || fingerprint.modified_at != document.file_modified_at
                || document.file_modified_at.is_none()
            {
                match sha256_file(&path) {
                    Ok(hash) if Some(hash.as_str()) == document.content_hash.as_deref() => {
                        if fingerprint.modified_at.is_some() {
                            metadata_updates.push((document.id.clone(), fingerprint.modified_at));
                        }
                    }
                    Ok(_) | Err(_) => changed = true,
                }
            }

            if changed {
                changed_document_ids.push(document.id);
            }
        }

        let library = self
            .current
            .as_mut()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        let transaction = library.connection.transaction()?;
        let timestamp = now();
        for document_id in &changed_document_ids {
            transaction.execute(
                "
                UPDATE documents
                SET processing_status = 'processing',
                    index_status = 'pending',
                    error_stage = NULL,
                    error_message = NULL,
                    updated_at = ?1
                WHERE id = ?2 AND deleted_at IS NULL
                ",
                params![&timestamp, document_id],
            )?;
            transaction.execute(
                "DELETE FROM document_search WHERE document_id = ?1",
                params![document_id],
            )?;
        }
        for (document_id, modified_at) in metadata_updates {
            if modified_at.is_none() {
                continue;
            }
            transaction.execute(
                "
                UPDATE documents
                SET file_modified_at = ?1, updated_at = ?2
                WHERE id = ?3 AND deleted_at IS NULL
                ",
                params![modified_at, &timestamp, document_id],
            )?;
        }
        transaction.commit()?;

        let pending_count = library.connection.query_row(
            "
            SELECT COUNT(*)
            FROM documents
            WHERE deleted_at IS NULL
              AND processing_status = 'processing'
              AND index_status = 'pending'
            ",
            [],
            |row| row.get(0),
        )?;
        Ok(ExternalChangeScan {
            changed_document_ids,
            pending_count,
        })
    }

    fn process_pending_external_changes(&mut self) -> LibraryResult<IndexRunResult> {
        let mut result = IndexRunResult::default();
        loop {
            let document_id = {
                let library = self
                    .current
                    .as_ref()
                    .ok_or(LibraryError::NoCurrentLibrary)?;
                library
                    .connection
                    .query_row(
                        "
                        SELECT id
                        FROM documents
                        WHERE deleted_at IS NULL
                          AND processing_status = 'processing'
                          AND index_status = 'pending'
                        ORDER BY imported_at, id
                        LIMIT 1
                        ",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?
            };
            let Some(document_id) = document_id else {
                break;
            };
            let document = self.index_document(&document_id)?;
            result.processed += 1;
            match document.index_status {
                IndexStatus::Searchable => result.searchable += 1,
                IndexStatus::Failed => result.failed += 1,
                IndexStatus::Pending => {}
            }
        }
        Ok(result)
    }

    pub fn index_next_pending_document(&mut self) -> LibraryResult<Option<DocumentSummary>> {
        let document_id = {
            let library = self
                .current
                .as_ref()
                .ok_or(LibraryError::NoCurrentLibrary)?;
            library
                .connection
                .query_row(
                    "
                    SELECT id
                    FROM documents
                    WHERE deleted_at IS NULL AND index_status = 'pending'
                    ORDER BY imported_at, id
                    LIMIT 1
                    ",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
        };
        document_id
            .map(|document_id| self.index_document(&document_id))
            .transpose()
    }

    pub fn retry_document_index(&mut self, document_id: &str) -> LibraryResult<DocumentSummary> {
        {
            let library = self
                .current
                .as_ref()
                .ok_or(LibraryError::NoCurrentLibrary)?;
            ensure_document_exists(&library.connection, document_id)?;
            library.connection.execute(
                "
                UPDATE documents
                SET processing_status = 'processing',
                    index_status = 'pending',
                    error_stage = NULL,
                    error_message = NULL,
                    updated_at = ?1
                WHERE id = ?2 AND deleted_at IS NULL
                ",
                params![now(), document_id],
            )?;
            library.connection.execute(
                "DELETE FROM document_search WHERE document_id = ?1",
                params![document_id],
            )?;
        }
        self.index_document(document_id)
    }

    fn index_document(&mut self, document_id: &str) -> LibraryResult<DocumentSummary> {
        let stored = self.load_stored_document_index(document_id)?;
        let path = self.document_index_path(&stored)?;
        let refresh = refresh_document_copy(&path, &stored.file_type);

        let library = self
            .current
            .as_mut()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        let transaction = library.connection.transaction()?;
        transaction.execute(
            "DELETE FROM document_search WHERE document_id = ?1",
            params![document_id],
        )?;
        let mut invalidate_thumbnail = false;
        let updated = match refresh {
            Ok(refreshed) => {
                invalidate_thumbnail =
                    stored.content_hash.as_deref() != Some(refreshed.content_hash.as_str());
                transaction.execute(
                    "
                    INSERT INTO document_search (
                        document_id, title, description, extracted_text
                    )
                    SELECT id, title, description, ?1
                    FROM documents
                    WHERE id = ?2 AND deleted_at IS NULL
                    ",
                    params![&refreshed.extracted_text, document_id],
                )?;
                transaction.execute(
                    "
                    UPDATE documents
                    SET file_size = ?1,
                        content_hash = ?2,
                        file_modified_at = ?3,
                        processing_status = 'ready',
                        index_status = 'searchable',
                        error_stage = NULL,
                        error_message = NULL,
                        updated_at = ?4
                    WHERE id = ?5 AND deleted_at IS NULL
                    ",
                    params![
                        refreshed.file_size,
                        &refreshed.content_hash,
                        refreshed.file_modified_at,
                        now(),
                        document_id,
                    ],
                )?
            }
            Err(failure) => transaction.execute(
                "
                UPDATE documents
                SET file_size = COALESCE(?1, file_size),
                    content_hash = COALESCE(?2, content_hash),
                    file_modified_at = COALESCE(?3, file_modified_at),
                    processing_status = 'failed',
                    index_status = 'failed',
                    error_stage = ?4,
                    error_message = ?5,
                    updated_at = ?6
                WHERE id = ?7 AND deleted_at IS NULL
                ",
                params![
                    failure.file_size,
                    failure.content_hash.as_deref(),
                    failure.file_modified_at,
                    failure.stage,
                    failure.message,
                    now(),
                    document_id,
                ],
            )?,
        };
        if updated == 0 {
            return Err(LibraryError::DocumentNotFound(format!(
                "文档不存在或已删除：{document_id}"
            )));
        }
        transaction.commit()?;
        if invalidate_thumbnail {
            remove_thumbnail_versions(Path::new(&library.summary.path), document_id);
        }
        load_document_summary(&library.connection, document_id)
    }

    pub fn get_document_preview(
        &self,
        document_id: &str,
        page: Option<u32>,
    ) -> LibraryResult<DocumentPreview> {
        let stored = self.load_stored_document_file(document_id)?;
        match self.preview_document(&stored, page) {
            Ok(preview) => Ok(preview),
            Err(error) => Ok(DocumentPreview::Failure {
                code: error.code().to_string(),
                message: error.to_string(),
            }),
        }
    }

    pub fn get_document_thumbnail(&self, document_id: &str) -> LibraryResult<DocumentThumbnail> {
        let stored = self.load_stored_document_file(document_id)?;
        let path = self.document_copy_path(&stored)?;
        let capability = require_capability_for_file_type(&stored.file_type)?;

        let result = match capability.thumbnail {
            ThumbnailStrategy::PdfFirstPage => {
                self.thumbnail_png(&stored, &path)
                    .map(|bytes| DocumentThumbnail::Pdf {
                        data_url: data_url("image/png", &bytes),
                    })
            }
            ThumbnailStrategy::LocalImage => {
                self.thumbnail_png(&stored, &path)
                    .map(|bytes| DocumentThumbnail::Image {
                        data_url: data_url("image/png", &bytes),
                    })
            }
            ThumbnailStrategy::PptxFirstPage => {
                self.pptx_thumbnail(&stored, &path)
                    .map(|thumbnail| DocumentThumbnail::Pptx {
                        data_url: data_url(thumbnail.media_type, &thumbnail.bytes),
                    })
            }
            ThumbnailStrategy::TypeIcon => {
                return Ok(DocumentThumbnail::Fallback {
                    reason: format!("{} 使用类型图标。", stored.file_type),
                });
            }
        };

        Ok(result.unwrap_or_else(|error| DocumentThumbnail::Fallback {
            reason: format!("无法生成缩略图：{error}"),
        }))
    }

    pub fn save_document_thumbnail(
        &self,
        document_id: &str,
        thumbnail_data_url: &str,
    ) -> LibraryResult<DocumentThumbnail> {
        let stored = self.load_stored_document_file(document_id)?;
        let capability = require_capability_for_file_type(&stored.file_type)?;
        if capability.thumbnail != ThumbnailStrategy::PptxFirstPage {
            return Err(LibraryError::Preview(format!(
                "{} 不接受前端生成缩略图。",
                stored.file_type
            )));
        }

        let path = self.document_copy_path(&stored)?;
        ensure_document_copy_exists(&path)?;
        let (media_type, bytes) = decode_validated_thumbnail_data_url(thumbnail_data_url)?;
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        let thumbnail_dir = Path::new(&library.summary.path).join(THUMBNAILS_DIR);
        let content_version = stored
            .content_hash
            .as_deref()
            .filter(|hash| !hash.is_empty())
            .map(str::to_string)
            .unwrap_or(sha256_file(&path)?);
        let version = format!("{PPTX_THUMBNAIL_CACHE_SCHEME}-{content_version}");
        let extension = if media_type == "image/png" {
            "png"
        } else {
            "jpg"
        };
        let cache_path = thumbnail_cache_path_for(&thumbnail_dir, &stored.id, &version, extension);

        fs::create_dir_all(&thumbnail_dir)?;
        let temporary_path = cache_path.with_extension(format!("{extension}.tmp"));
        fs::write(&temporary_path, &bytes)?;
        if let Err(error) = fs::rename(&temporary_path, &cache_path) {
            let _ = fs::remove_file(&temporary_path);
            return Err(error.into());
        }
        remove_other_thumbnail_versions(&thumbnail_dir, &stored.id, &cache_path);

        Ok(DocumentThumbnail::Pptx {
            data_url: data_url(media_type, &bytes),
        })
    }

    fn preview_document(
        &self,
        stored: &StoredDocumentFile,
        page: Option<u32>,
    ) -> LibraryResult<DocumentPreview> {
        let path = self.document_copy_path(stored)?;
        ensure_document_copy_exists(&path)?;
        let capability = require_capability_for_file_type(&stored.file_type)?;

        match capability.preview {
            PreviewStrategy::PdfPages => {
                let page_number = page.unwrap_or(1).max(1);
                let rendered = self.render_cached_pdf_page(stored, &path, page_number - 1)?;
                Ok(DocumentPreview::Pdf {
                    data_url: data_url("image/png", &rendered.png),
                    page_count: Some(rendered.page_count),
                    page: page_number,
                })
            }
            PreviewStrategy::LocalImage => {
                let bytes = fs::read(&path)?;
                Ok(DocumentPreview::Image {
                    data_url: data_url(
                        capability
                            .media_type
                            .as_deref()
                            .unwrap_or("application/octet-stream"),
                        &bytes,
                    ),
                })
            }
            PreviewStrategy::PlainText => Ok(DocumentPreview::Text {
                text: read_utf8_text(&path)?,
            }),
            PreviewStrategy::SafeMarkdown => Ok(DocumentPreview::Markdown {
                text: read_utf8_text(&path)?,
            }),
            PreviewStrategy::DocxLayout => {
                let bytes = fs::read(&path)?;
                let degraded_features = inspect_docx_degradations(&bytes);
                let sanitized = ooxml::sanitize_docx_package(&bytes)?;
                Ok(DocumentPreview::Docx {
                    data_url: data_url(
                        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                        &sanitized,
                    ),
                    text: extract_docx_text_from_bytes(&bytes)?,
                    notice: if degraded_features.is_empty() {
                        "DOCX 版式预览为本地只读近似呈现。".to_string()
                    } else {
                        "DOCX 版式预览已呈现，部分复杂内容需要降级。".to_string()
                    },
                    degraded_features,
                })
            }
            PreviewStrategy::PptxPages => {
                let bytes = fs::read(&path)?;
                let extraction = extract_pptx_text_from_bytes(&bytes)?;
                let mut degraded_features = inspect_pptx_degradations(&bytes);
                merge_features(&mut degraded_features, extraction.degraded_features);
                let sanitized = ooxml::sanitize_pptx_package(&bytes)?;
                Ok(DocumentPreview::Pptx {
                    data_url: data_url(
                        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
                        &sanitized,
                    ),
                    text: extraction.text,
                    notice: if degraded_features.is_empty() {
                        "PPTX 版式预览为本地只读近似呈现。".to_string()
                    } else {
                        "PPTX 版式预览已呈现，部分复杂内容需要降级。".to_string()
                    },
                    degraded_features,
                })
            }
        }
    }

    fn render_cached_pdf_page(
        &self,
        stored: &StoredDocumentFile,
        path: &Path,
        page_index: u32,
    ) -> LibraryResult<thumbnail::RenderedPdfPage> {
        let version = match stored.content_hash.as_deref() {
            Some(hash) if !hash.is_empty() => hash.to_string(),
            _ => sha256_file(path)?,
        };
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        let cache_directory =
            rendered_page_cache_directory(&library.summary.id, &stored.id, &version);
        let cache_path = cache_directory.join(format!("page-{}.png", page_index + 1));

        if let Ok(bytes) = fs::read(&cache_path) {
            if is_png(&bytes) {
                let page_count = match fs::read(path)
                    .ok()
                    .as_deref()
                    .and_then(estimate_pdf_page_count)
                {
                    Some(page_count) => page_count,
                    None => thumbnail::render_pdf_page_png(path, page_index)?.page_count,
                };
                return Ok(thumbnail::RenderedPdfPage {
                    png: bytes,
                    page_count,
                });
            }
            let _ = fs::remove_file(&cache_path);
        }

        let rendered = thumbnail::render_pdf_page_png(path, page_index)?;
        if !is_png(&rendered.png) {
            return Err(LibraryError::Preview(
                "PDF 渲染器没有返回有效 PNG。".to_string(),
            ));
        }

        fs::create_dir_all(&cache_directory)?;
        let temporary_path =
            cache_directory.join(format!(".page-{}.{}.tmp", page_index + 1, Uuid::new_v4()));
        fs::write(&temporary_path, &rendered.png)?;
        if let Err(error) = fs::rename(&temporary_path, &cache_path) {
            let _ = fs::remove_file(&temporary_path);
            return Err(error.into());
        }
        remove_other_render_versions(
            cache_directory
                .parent()
                .expect("rendered page cache directory must have a document parent"),
            &cache_directory,
        );
        Ok(rendered)
    }

    fn thumbnail_png(&self, stored: &StoredDocumentFile, path: &Path) -> LibraryResult<Vec<u8>> {
        ensure_document_copy_exists(path)?;
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        let thumbnail_dir = Path::new(&library.summary.path).join(THUMBNAILS_DIR);
        let version = match stored.content_hash.as_deref() {
            Some(hash) if !hash.is_empty() => hash.to_string(),
            _ => sha256_file(path)?,
        };
        let cache_path = thumbnail_cache_path(&thumbnail_dir, &stored.id, &version);

        if let Ok(bytes) = fs::read(&cache_path) {
            if is_png(&bytes) {
                return Ok(bytes);
            }
            let _ = fs::remove_file(&cache_path);
        }

        let capability = require_capability_for_file_type(&stored.file_type)?;
        let generated = thumbnail::generate_png_thumbnail(path, capability)?;
        if !is_png(&generated) {
            return Err(LibraryError::Preview(
                "缩略图生成器没有返回有效 PNG。".to_string(),
            ));
        }

        fs::create_dir_all(&thumbnail_dir)?;
        let temporary_path = cache_path.with_extension("png.tmp");
        fs::write(&temporary_path, &generated)?;
        if let Err(error) = fs::rename(&temporary_path, &cache_path) {
            let _ = fs::remove_file(&temporary_path);
            return Err(error.into());
        }
        remove_other_thumbnail_versions(&thumbnail_dir, &stored.id, &cache_path);
        Ok(generated)
    }

    fn pptx_thumbnail(
        &self,
        stored: &StoredDocumentFile,
        path: &Path,
    ) -> LibraryResult<CachedImageThumbnail> {
        ensure_document_copy_exists(path)?;
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        let thumbnail_dir = Path::new(&library.summary.path).join(THUMBNAILS_DIR);
        let content_version = stored
            .content_hash
            .as_deref()
            .filter(|hash| !hash.is_empty())
            .map(str::to_string)
            .unwrap_or(sha256_file(path)?);
        let version = format!("{PPTX_THUMBNAIL_CACHE_SCHEME}-{content_version}");

        for (extension, media_type) in [("jpg", "image/jpeg"), ("png", "image/png")] {
            let cache_path =
                thumbnail_cache_path_for(&thumbnail_dir, &stored.id, &version, extension);
            if let Ok(bytes) = fs::read(&cache_path) {
                if is_supported_thumbnail_image(&bytes) {
                    remove_other_thumbnail_versions(&thumbnail_dir, &stored.id, &cache_path);
                    return Ok(CachedImageThumbnail { bytes, media_type });
                }
                let _ = fs::remove_file(cache_path);
            }
        }

        remove_thumbnail_versions(Path::new(&library.summary.path), &stored.id);
        Err(LibraryError::Preview("PPTX 缩略图尚未生成。".to_string()))
    }

    pub fn open_document(&self, document_id: &str) -> LibraryResult<()> {
        let stored = self.load_stored_document_file(document_id)?;
        let path = self.document_copy_path(&stored)?;
        ensure_document_copy_exists(&path)?;

        open::that(&path).map_err(|error| LibraryError::OpenDocument(error.to_string()))
    }

    pub fn open_external_url(&self, url: &str) -> LibraryResult<()> {
        let url = url.trim();
        if !is_safe_external_url(url) {
            return Err(LibraryError::InvalidExternalUrl(
                "只允许打开由用户明确点击的 HTTP 或 HTTPS 链接。".to_string(),
            ));
        }
        open::that(url).map_err(|error| LibraryError::OpenDocument(error.to_string()))
    }

    pub fn list_recent_libraries(&self) -> Vec<RecentLibrary> {
        self.recent
            .iter()
            .map(|entry| RecentLibrary {
                path: entry.path.clone(),
                name: entry.name.clone(),
                last_opened_at: entry.last_opened_at.clone(),
                is_available: is_library_directory(Path::new(&entry.path)),
            })
            .collect()
    }

    pub fn forget_recent_library(
        &mut self,
        path: impl AsRef<Path>,
    ) -> LibraryResult<Vec<RecentLibrary>> {
        let target = normalize_path(path.as_ref())?;
        let target = target.to_string_lossy();

        self.recent
            .retain(|entry| !paths_equal(&entry.path, target.as_ref()));
        self.persist_recent()?;
        Ok(self.list_recent_libraries())
    }

    pub fn current_library(&self) -> Option<&LibrarySummary> {
        self.current.as_ref().map(|library| &library.summary)
    }

    fn set_current(&mut self, summary: LibrarySummary) -> LibraryResult<()> {
        let connection = open_database(Path::new(&summary.path))?;
        initialize_schema(&connection)?;
        self.current = Some(OpenLibrary {
            summary,
            connection,
        });
        Ok(())
    }

    fn record_recent(&mut self, library: &LibrarySummary) -> LibraryResult<()> {
        let target = normalize_path(Path::new(&library.path))?;
        let target_display = target.to_string_lossy().into_owned();

        self.recent
            .retain(|entry| !paths_equal(&entry.path, &target_display));
        self.recent.insert(
            0,
            RecentLibraryRecord {
                path: target_display,
                name: library.name.clone(),
                last_opened_at: now(),
            },
        );
        self.recent.truncate(MAX_RECENT_LIBRARIES);
        self.persist_recent()
    }

    fn persist_recent(&self) -> LibraryResult<()> {
        let path = self.state_dir.join(RECENT_FILE);
        let data = serde_json::to_vec_pretty(&self.recent)?;
        fs::write(path, data)?;
        Ok(())
    }

    fn snapshot(&self) -> BootstrapState {
        BootstrapState {
            current_library: self.current_library().cloned(),
            recent_libraries: self.list_recent_libraries(),
        }
    }

    fn load_stored_document_file(&self, document_id: &str) -> LibraryResult<StoredDocumentFile> {
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        library
            .connection
            .query_row(
                "
                SELECT id, file_name, file_type, library_path, content_hash
                FROM documents
                WHERE id = ?1 AND deleted_at IS NULL
                ",
                params![document_id],
                |row| {
                    Ok(StoredDocumentFile {
                        id: row.get(0)?,
                        file_name: row.get(1)?,
                        file_type: row.get(2)?,
                        library_path: row.get(3)?,
                        content_hash: row.get(4)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| {
                LibraryError::DocumentNotFound(format!("文档不存在或已删除：{document_id}"))
            })
    }

    fn load_stored_document_index(&self, document_id: &str) -> LibraryResult<StoredDocumentIndex> {
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        library
            .connection
            .query_row(
                "
                SELECT
                    id,
                    file_name,
                    file_type,
                    file_size,
                    content_hash,
                    file_modified_at,
                    library_path,
                    processing_status,
                    index_status
                FROM documents
                WHERE id = ?1 AND deleted_at IS NULL
                ",
                params![document_id],
                stored_document_index_from_row,
            )
            .optional()?
            .ok_or_else(|| {
                LibraryError::DocumentNotFound(format!("文档不存在或已删除：{document_id}"))
            })
    }

    fn external_file_fingerprints(&self) -> HashMap<String, FileFingerprint> {
        let Some(library) = self.current.as_ref() else {
            return HashMap::new();
        };
        let library_root = PathBuf::from(&library.summary.path);
        load_external_document_indexes(&library.connection)
            .map(|documents| {
                documents
                    .into_iter()
                    .map(|document| {
                        let path = document_path(&library_root, &document.library_path)
                            .unwrap_or_else(|_| PathBuf::new());
                        (document.id, file_fingerprint(&path))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn document_copy_path(&self, stored: &StoredDocumentFile) -> LibraryResult<PathBuf> {
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        document_path(Path::new(&library.summary.path), &stored.library_path).map_err(|_| {
            LibraryError::DocumentFileMissing(format!("资料库副本路径无效：{}", stored.file_name))
        })
    }

    fn document_index_path(&self, stored: &StoredDocumentIndex) -> LibraryResult<PathBuf> {
        let library = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        document_path(Path::new(&library.summary.path), &stored.library_path).map_err(|_| {
            LibraryError::DocumentFileMissing(format!("资料库副本路径无效：{}", stored.file_name))
        })
    }
}

struct RefreshedDocument {
    file_size: i64,
    content_hash: String,
    file_modified_at: Option<i64>,
    extracted_text: String,
}

struct DocumentRefreshFailure {
    stage: &'static str,
    message: String,
    file_size: Option<i64>,
    content_hash: Option<String>,
    file_modified_at: Option<i64>,
}

fn refresh_document_copy(
    path: &Path,
    expected_file_type: &str,
) -> Result<RefreshedDocument, DocumentRefreshFailure> {
    if !path.exists() {
        if let Some(replacement) = sole_replacement_file(path) {
            if supported_file_type(&replacement).is_none() {
                return Err(DocumentRefreshFailure {
                    stage: "externalUnsupported",
                    message: format!(
                        "资料库副本已被替换为不支持的文件：{}",
                        replacement
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| replacement.to_string_lossy().into_owned())
                    ),
                    file_size: fs::metadata(&replacement)
                        .ok()
                        .and_then(|metadata| i64::try_from(metadata.len()).ok()),
                    content_hash: None,
                    file_modified_at: fs::metadata(&replacement)
                        .ok()
                        .and_then(|metadata| modified_at(&metadata)),
                });
            }
        }
        return Err(DocumentRefreshFailure {
            stage: "externalRead",
            message: format!(
                "资料库副本不存在：{}",
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.to_string_lossy().into_owned())
            ),
            file_size: None,
            content_hash: None,
            file_modified_at: None,
        });
    }
    let metadata = fs::metadata(path).map_err(|error| DocumentRefreshFailure {
        stage: "externalRead",
        message: format!(
            "无法读取资料库副本 {}：{error}",
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned())
        ),
        file_size: None,
        content_hash: None,
        file_modified_at: None,
    })?;
    if !metadata.is_file() {
        return Err(DocumentRefreshFailure {
            stage: "externalRead",
            message: "资料库副本路径不是文件，无法读取。".to_string(),
            file_size: None,
            content_hash: None,
            file_modified_at: modified_at(&metadata),
        });
    }

    let file_size = i64::try_from(metadata.len()).map_err(|_| DocumentRefreshFailure {
        stage: "externalRead",
        message: "资料库副本大小超出支持范围。".to_string(),
        file_size: None,
        content_hash: None,
        file_modified_at: modified_at(&metadata),
    })?;
    let file_modified_at = modified_at(&metadata);
    let Some(actual_file_type) = supported_file_type(path) else {
        return Err(DocumentRefreshFailure {
            stage: "externalUnsupported",
            message: "资料库副本已被替换为不支持的文件类型。".to_string(),
            file_size: Some(file_size),
            content_hash: None,
            file_modified_at,
        });
    };
    if actual_file_type != expected_file_type {
        return Err(DocumentRefreshFailure {
            stage: "externalUnsupported",
            message: format!(
                "资料库副本文件类型已变为 {actual_file_type}，与文档记录的 {expected_file_type} 不一致。"
            ),
            file_size: Some(file_size),
            content_hash: None,
            file_modified_at,
        });
    }

    let content_hash = sha256_file(path).map_err(|error| DocumentRefreshFailure {
        stage: "externalHash",
        message: format!("无法计算资料库副本哈希：{error}"),
        file_size: Some(file_size),
        content_hash: None,
        file_modified_at,
    })?;
    let capability = match capability_for_file_type(expected_file_type) {
        Some(capability) => capability,
        None => {
            return Err(DocumentRefreshFailure {
                stage: "externalUnsupported",
                message: format!("不支持的文档格式：{expected_file_type}"),
                file_size: Some(file_size),
                content_hash: Some(content_hash),
                file_modified_at,
            });
        }
    };
    if let Err(message) = validate_file_content(path, capability) {
        return Err(DocumentRefreshFailure {
            stage: "externalValidation",
            message,
            file_size: Some(file_size),
            content_hash: Some(content_hash),
            file_modified_at,
        });
    }

    let extracted_text =
        extract_search_text(path, expected_file_type).map_err(|error| DocumentRefreshFailure {
            stage: "externalExtract",
            message: error.to_string(),
            file_size: Some(file_size),
            content_hash: Some(content_hash.clone()),
            file_modified_at,
        })?;
    Ok(RefreshedDocument {
        file_size,
        content_hash,
        file_modified_at,
        extracted_text,
    })
}

fn sole_replacement_file(expected_path: &Path) -> Option<PathBuf> {
    let directory = expected_path.parent()?;
    let mut files = fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') || !entry.path().is_file() {
                return None;
            }
            Some(entry.path())
        });
    let replacement = files.next()?;
    files.next().is_none().then_some(replacement)
}

fn load_external_document_indexes(
    connection: &Connection,
) -> LibraryResult<Vec<StoredDocumentIndex>> {
    let mut statement = connection.prepare(
        "
        SELECT
            id,
            file_name,
            file_type,
            file_size,
            content_hash,
            file_modified_at,
            library_path,
            processing_status,
            index_status
        FROM documents
        WHERE deleted_at IS NULL
        ORDER BY imported_at, id
        ",
    )?;
    let documents = statement
        .query_map([], stored_document_index_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(documents)
}

fn stored_document_index_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<StoredDocumentIndex> {
    let processing_status: String = row.get(7)?;
    let processing_status = DocumentProcessingStatus::from_database(&processing_status)
        .ok_or_else(|| invalid_status_error(7, "document processing status"))?;
    let index_status: String = row.get(8)?;
    let index_status = IndexStatus::from_database(&index_status)
        .ok_or_else(|| invalid_status_error(8, "document index status"))?;

    Ok(StoredDocumentIndex {
        id: row.get(0)?,
        file_name: row.get(1)?,
        file_type: row.get(2)?,
        file_size: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
        content_hash: row.get(4)?,
        file_modified_at: row.get(5)?,
        library_path: row.get(6)?,
        processing_status,
        index_status,
    })
}

fn invalid_status_error(index: usize, label: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        rusqlite::types::Type::Text,
        Box::new(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown {label}"),
        )),
    )
}

fn document_path(library_root: &Path, library_path: &str) -> LibraryResult<PathBuf> {
    let mut path = library_root.to_path_buf();
    for component in Path::new(library_path).components() {
        use std::path::Component;
        match component {
            Component::Normal(component) => path.push(component),
            _ => {
                return Err(LibraryError::DocumentFileMissing(format!(
                    "资料库副本路径无效：{library_path}"
                )));
            }
        }
    }
    Ok(path)
}

fn file_fingerprint(path: &Path) -> FileFingerprint {
    match fs::metadata(path) {
        Ok(metadata) => FileFingerprint {
            available: true,
            is_file: metadata.is_file(),
            size: i64::try_from(metadata.len()).unwrap_or(i64::MAX),
            modified_at: modified_at(&metadata),
        },
        Err(_) => FileFingerprint {
            available: false,
            is_file: false,
            size: 0,
            modified_at: None,
        },
    }
}

fn scan_import_paths(paths: &[String]) -> Vec<ScanEntry> {
    let mut entries = Vec::new();
    for path in paths {
        scan_explicit_path(path, &mut entries);
    }
    entries
}

fn scan_explicit_path(input: &str, entries: &mut Vec<ScanEntry>) {
    if input.trim().is_empty() {
        entries.push(ScanEntry::Failed {
            source_path: input.to_string(),
            message: "请选择要导入的文件或文件夹。".to_string(),
        });
        return;
    }

    let path = PathBuf::from(input);
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) => {
            entries.push(ScanEntry::Failed {
                source_path: input.to_string(),
                message: format!("无法读取源文件或文件夹：{error}"),
            });
            return;
        }
    };
    let display_path = dunce_canonicalize(&path)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned();

    if metadata.is_dir() {
        scan_directory(Path::new(&display_path), entries);
    } else if metadata.is_file() {
        if supported_file_type(Path::new(&display_path)).is_some() {
            entries.push(ScanEntry::File(PathBuf::from(display_path)));
        } else {
            entries.push(ScanEntry::Failed {
                source_path: display_path,
                message: unsupported_message(),
            });
        }
    } else {
        entries.push(ScanEntry::Failed {
            source_path: display_path,
            message: "只能导入文件或文件夹。".to_string(),
        });
    }
}

fn scan_directory(directory: &Path, entries: &mut Vec<ScanEntry>) {
    let read_dir = match fs::read_dir(directory) {
        Ok(read_dir) => read_dir,
        Err(error) => {
            entries.push(ScanEntry::Failed {
                source_path: directory.to_string_lossy().into_owned(),
                message: format!("无法扫描文件夹：{error}"),
            });
            return;
        }
    };

    let mut directory_entries = match read_dir.collect::<Result<Vec<_>, _>>() {
        Ok(directory_entries) => directory_entries,
        Err(error) => {
            entries.push(ScanEntry::Failed {
                source_path: directory.to_string_lossy().into_owned(),
                message: format!("无法读取文件夹内容：{error}"),
            });
            return;
        }
    };
    directory_entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase());

    for entry in directory_entries {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                entries.push(ScanEntry::Failed {
                    source_path: path.to_string_lossy().into_owned(),
                    message: format!("无法读取文件类型：{error}"),
                });
                continue;
            }
        };

        if file_type.is_symlink() {
            entries.push(ScanEntry::Ignored {
                source_path: path.to_string_lossy().into_owned(),
            });
        } else if file_type.is_dir() {
            scan_directory(&path, entries);
        } else if file_type.is_file() {
            if supported_file_type(&path).is_some() {
                entries.push(ScanEntry::File(path));
            } else {
                entries.push(ScanEntry::Ignored {
                    source_path: path.to_string_lossy().into_owned(),
                });
            }
        } else {
            entries.push(ScanEntry::Ignored {
                source_path: path.to_string_lossy().into_owned(),
            });
        }
    }
}

fn count_items(items: &[ImportItemResult], status: ImportItemStatus) -> i64 {
    items.iter().filter(|item| item.status == status).count() as i64
}

fn restore_previous_copies(previous_copies: &[(PathBuf, PathBuf)]) {
    for (backup_path, original_path) in previous_copies.iter().rev() {
        let _ = fs::rename(backup_path, original_path);
    }
}

fn remove_previous_copies(previous_copies: &[(PathBuf, PathBuf)]) {
    for (backup_path, _) in previous_copies {
        let _ = fs::remove_file(backup_path);
    }
}

fn move_library_copy_to_tombstone(path: &Path, prefix: &str) -> LibraryResult<Option<PathBuf>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            let parent = path.parent().ok_or_else(|| {
                LibraryError::DocumentFileMissing(format!("资料库副本路径无效：{}", path.display()))
            })?;
            let tombstone_path = parent.join(format!(
                "{prefix}{}{DELETE_TOMBSTONE_SUFFIX}",
                Uuid::new_v4()
            ));
            fs::rename(path, &tombstone_path)?;
            Ok(Some(tombstone_path))
        }
        Ok(_) => Err(LibraryError::DocumentFileMissing(format!(
            "资料库副本路径不是文件：{}",
            path.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let Some(parent) = path.parent() else {
                return Ok(None);
            };
            Ok(find_library_tombstone(parent, prefix))
        }
        Err(error) => Err(LibraryError::Io(error)),
    }
}

fn restore_library_copy_tombstone(
    tombstone_path: &Path,
    original_path: &Path,
) -> LibraryResult<()> {
    if original_path.exists() {
        remove_library_copy(tombstone_path)?;
        return Ok(());
    }
    fs::rename(tombstone_path, original_path)?;
    Ok(())
}

fn find_library_tombstone(directory: &Path, prefix: &str) -> Option<PathBuf> {
    fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .is_some_and(|name| is_delete_tombstone_name(name, prefix))
        })
}

fn is_delete_tombstone_name(name: &std::ffi::OsStr, prefix: &str) -> bool {
    let name = name.to_string_lossy();
    name.starts_with(prefix) && name.ends_with(DELETE_TOMBSTONE_SUFFIX)
}

fn cleanup_delete_tombstones(library_root: &Path, connection: &Connection) -> LibraryResult<()> {
    let documents_directory = library_root.join(DOCUMENTS_DIR);
    let document_directories = match fs::read_dir(&documents_directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(LibraryError::Io(error)),
    };

    for document_directory in document_directories {
        let document_directory = document_directory?;
        if !document_directory.file_type()?.is_dir() {
            continue;
        }

        let document_id = document_directory
            .file_name()
            .to_string_lossy()
            .into_owned();
        let library_path = connection
            .query_row(
                "SELECT library_path FROM documents WHERE id = ?1",
                params![&document_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let directory_path = document_directory.path();
        let mut tombstones = fs::read_dir(&directory_path)?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|entry| is_delete_tombstone_name(&entry.file_name(), DELETE_TOMBSTONE_PREFIX))
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        tombstones.sort();
        let had_tombstones = !tombstones.is_empty();

        for tombstone_path in tombstones {
            match library_path.as_deref() {
                Some(library_path) => {
                    let original_path = document_path(library_root, library_path)?;
                    restore_library_copy_tombstone(&tombstone_path, &original_path)?;
                }
                None => {
                    fs::remove_file(&tombstone_path)?;
                }
            }
        }

        if had_tombstones {
            match fs::remove_dir(&directory_path) {
                Ok(()) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
                    ) => {}
                Err(error) => return Err(LibraryError::Io(error)),
            }
        }
    }
    Ok(())
}

fn remove_library_copy(path: &Path) -> LibraryResult<()> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(LibraryError::Io(error)),
    }
    if let Some(parent) = path.parent() {
        match fs::remove_dir(parent) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(error) => return Err(LibraryError::Io(error)),
        }
    }
    Ok(())
}

fn import_item_not_found(item_id: &str) -> LibraryError {
    LibraryError::ImportItemNotFound(format!("导入项不存在或已处理：{item_id}"))
}

fn find_source_change(
    connection: &Connection,
    source_identifier: &str,
    content_hash: &str,
) -> LibraryResult<Option<String>> {
    connection
        .query_row(
            "
            SELECT d.id
            FROM documents d
            JOIN sources s ON s.document_id = d.id
            WHERE d.deleted_at IS NULL
              AND s.source_identifier = ?1
              AND d.content_hash IS NOT NULL
              AND d.content_hash <> ?2
            ORDER BY COALESCE(s.last_imported_at, d.imported_at) DESC, d.rowid DESC
            LIMIT 1
            ",
            params![source_identifier, content_hash],
            |row| row.get(0),
        )
        .optional()
        .map_err(LibraryError::from)
}

fn find_duplicate_document(
    connection: &Connection,
    content_hash: &str,
) -> LibraryResult<Option<String>> {
    connection
        .query_row(
            "
            SELECT id
            FROM documents
            WHERE deleted_at IS NULL AND content_hash = ?1
            ORDER BY imported_at DESC, rowid DESC
            LIMIT 1
            ",
            params![content_hash],
            |row| row.get(0),
        )
        .optional()
        .map_err(LibraryError::from)
}

fn validate_collection_name(name: &str) -> LibraryResult<&str> {
    let name = name.trim();
    if name.is_empty() {
        return Err(LibraryError::InvalidCollection(
            "集合名称不能为空。".to_string(),
        ));
    }
    Ok(name)
}

fn ensure_collection_exists(connection: &Connection, collection_id: &str) -> LibraryResult<()> {
    let exists = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM collections WHERE id = ?1)",
        params![collection_id],
        |row| row.get::<_, bool>(0),
    )?;
    if !exists {
        return Err(LibraryError::CollectionNotFound(format!(
            "集合不存在：{collection_id}"
        )));
    }
    Ok(())
}

fn collection_is_descendant(
    connection: &Connection,
    collection_id: &str,
    potential_descendant_id: &str,
) -> LibraryResult<bool> {
    connection
        .query_row(
            "
            WITH RECURSIVE descendants(id) AS (
                SELECT id FROM collections WHERE id = ?1
                UNION ALL
                SELECT c.id
                FROM collections c
                JOIN descendants d ON c.parent_id = d.id
            )
            SELECT EXISTS(SELECT 1 FROM descendants WHERE id = ?2)
            ",
            params![collection_id, potential_descendant_id],
            |row| row.get(0),
        )
        .map_err(LibraryError::from)
}

fn validate_tag_name(name: &str) -> LibraryResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(LibraryError::InvalidTag("标签名称不能为空。".to_string()));
    }
    if name.chars().count() > 50 {
        return Err(LibraryError::InvalidTag(
            "标签名称不能超过 50 个字符。".to_string(),
        ));
    }
    Ok(name.to_string())
}

fn tag_name_exists(
    connection: &Connection,
    name: &str,
    excluded_tag_id: Option<&str>,
) -> LibraryResult<bool> {
    connection
        .query_row(
            "
            SELECT EXISTS(
                SELECT 1
                FROM tags
                WHERE name = ?1 COLLATE NOCASE
                  AND (?2 IS NULL OR id <> ?2)
            )
            ",
            params![name, excluded_tag_id],
            |row| row.get(0),
        )
        .map_err(LibraryError::from)
}

fn ensure_tag_exists(connection: &Connection, tag_id: &str) -> LibraryResult<()> {
    let exists = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM tags WHERE id = ?1)",
        params![tag_id],
        |row| row.get::<_, bool>(0),
    )?;
    if !exists {
        return Err(LibraryError::TagNotFound(format!("标签不存在：{tag_id}")));
    }
    Ok(())
}

fn ensure_document_exists(connection: &Connection, document_id: &str) -> LibraryResult<()> {
    let exists = connection.query_row(
        "
        SELECT EXISTS(
            SELECT 1
            FROM documents
            WHERE id = ?1 AND deleted_at IS NULL
        )
        ",
        params![document_id],
        |row| row.get::<_, bool>(0),
    )?;
    if !exists {
        return Err(LibraryError::DocumentNotFound(format!(
            "文档不存在或已删除：{document_id}"
        )));
    }
    Ok(())
}

fn validate_document_title(title: &str) -> LibraryResult<String> {
    let title = title.trim();
    if title.is_empty() {
        return Err(LibraryError::InvalidDocumentMetadata(
            "文档标题不能为空。".to_string(),
        ));
    }
    if title.chars().count() > 500 {
        return Err(LibraryError::InvalidDocumentMetadata(
            "文档标题不能超过 500 个字符。".to_string(),
        ));
    }
    Ok(title.to_string())
}

fn validate_document_date(date: &str) -> LibraryResult<()> {
    NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| {
        LibraryError::InvalidDocumentMetadata("文档日期必须使用 YYYY-MM-DD 格式。".to_string())
    })?;
    Ok(())
}

fn normalize_optional_text(value: Option<String>) -> Option<String> {
    value.and_then(|text| {
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    })
}

fn tag_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TagSummary> {
    Ok(TagSummary {
        id: row.get(0)?,
        name: row.get(1)?,
        document_count: row.get(2)?,
    })
}

fn load_tag_summary(connection: &Connection, tag_id: &str) -> LibraryResult<TagSummary> {
    connection
        .query_row(
            "
            SELECT
                t.id,
                t.name,
                (
                    SELECT COUNT(*)
                    FROM document_tags dt
                    JOIN documents d ON d.id = dt.document_id
                    WHERE dt.tag_id = t.id AND d.deleted_at IS NULL
                )
            FROM tags t
            WHERE t.id = ?1
            ",
            params![tag_id],
            tag_from_row,
        )
        .optional()?
        .ok_or_else(|| LibraryError::TagNotFound(format!("标签不存在：{tag_id}")))
}

fn load_document_tags(
    connection: &Connection,
    document_id: &str,
) -> LibraryResult<Vec<TagSummary>> {
    let mut statement = connection.prepare(
        "
        SELECT
            t.id,
            t.name,
            (
                SELECT COUNT(*)
                FROM document_tags count_dt
                JOIN documents count_d ON count_d.id = count_dt.document_id
                WHERE count_dt.tag_id = t.id AND count_d.deleted_at IS NULL
            )
        FROM tags t
        JOIN document_tags dt ON dt.tag_id = t.id
        WHERE dt.document_id = ?1
        ORDER BY LOWER(t.name), t.id
        ",
    )?;
    let tags = statement
        .query_map(params![document_id], tag_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(tags)
}

fn load_all_document_tags(
    connection: &Connection,
) -> LibraryResult<HashMap<String, Vec<TagSummary>>> {
    let mut statement = connection.prepare(
        "
        WITH tag_counts AS (
            SELECT dt.tag_id, COUNT(*) AS document_count
            FROM document_tags dt
            JOIN documents d ON d.id = dt.document_id
            WHERE d.deleted_at IS NULL
            GROUP BY dt.tag_id
        )
        SELECT
            dt.document_id,
            t.id,
            t.name,
            COALESCE(tag_counts.document_count, 0)
        FROM document_tags dt
        JOIN tags t ON t.id = dt.tag_id
        LEFT JOIN tag_counts ON tag_counts.tag_id = t.id
        ORDER BY dt.document_id, LOWER(t.name), t.id
        ",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            TagSummary {
                id: row.get(1)?,
                name: row.get(2)?,
                document_count: row.get(3)?,
            },
        ))
    })?;
    let mut tags_by_document = HashMap::<String, Vec<TagSummary>>::new();
    for row in rows {
        let (document_id, tag) = row?;
        tags_by_document.entry(document_id).or_default().push(tag);
    }
    Ok(tags_by_document)
}

fn collection_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CollectionSummary> {
    Ok(CollectionSummary {
        id: row.get(0)?,
        name: row.get(1)?,
        parent_id: row.get(2)?,
        is_inbox: row.get(3)?,
        document_count: row.get(4)?,
    })
}

fn load_collection_summary(
    connection: &Connection,
    collection_id: &str,
) -> LibraryResult<CollectionSummary> {
    connection
        .query_row(
            "
            SELECT
                c.id,
                c.name,
                c.parent_id,
                c.is_inbox,
                (
                    SELECT COUNT(*)
                    FROM documents d
                    WHERE d.collection_id = c.id AND d.deleted_at IS NULL
                )
            FROM collections c
            WHERE c.id = ?1
            ",
            params![collection_id],
            collection_from_row,
        )
        .optional()?
        .ok_or_else(|| LibraryError::CollectionNotFound(format!("集合不存在：{collection_id}")))
}

fn load_document_summary(
    connection: &Connection,
    document_id: &str,
) -> LibraryResult<DocumentSummary> {
    let mut document = connection
        .query_row(
            "
            SELECT
                d.id,
                d.title,
                d.file_name,
                d.file_type,
                d.file_size,
                d.content_hash,
                d.collection_id,
                d.processing_status,
                d.index_status,
                d.error_stage,
                d.error_message,
                d.imported_at,
                COALESCE(s.source_path, ''),
                COALESCE(s.source_identifier, ''),
                COALESCE(s.last_imported_at, d.imported_at),
                d.description,
                d.document_date
            FROM documents d
            LEFT JOIN sources s ON s.document_id = d.id
            WHERE d.id = ?1 AND d.deleted_at IS NULL
            ",
            params![document_id],
            document_from_row,
        )
        .optional()?
        .ok_or_else(|| {
            LibraryError::DocumentNotFound(format!("文档不存在或已删除：{document_id}"))
        })?;
    document.tags = load_document_tags(connection, document_id)?;
    Ok(document)
}

pub fn open_directory(path: impl AsRef<Path>) -> LibraryResult<()> {
    let path = path.as_ref();
    if !path.is_dir() {
        return Err(LibraryError::OpenDirectory(format!(
            "目录不存在：{}",
            path.display()
        )));
    }

    open::that(path).map_err(|error| LibraryError::OpenDirectory(error.to_string()))
}

fn load_recent(state_dir: &Path) -> LibraryResult<Vec<RecentLibraryRecord>> {
    let path = state_dir.join(RECENT_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }

    let data = fs::read(path)?;
    Ok(serde_json::from_slice(&data)?)
}

fn initialize_library(path: &Path, metadata: &LibraryMetadata) -> LibraryResult<()> {
    fs::create_dir_all(path.join(INTERNAL_DIR))?;
    fs::create_dir_all(path.join(DOCUMENTS_DIR))?;
    fs::create_dir_all(path.join(TRASH_DIR))?;
    fs::create_dir_all(path.join(THUMBNAILS_DIR))?;

    let connection = open_database(path)?;
    initialize_schema(&connection)?;

    let metadata_path = path.join(INTERNAL_DIR).join(METADATA_FILE);
    let bytes = serde_json::to_vec_pretty(metadata)?;
    fs::write(metadata_path, bytes)?;
    Ok(())
}

fn read_metadata(path: &Path) -> LibraryResult<LibraryMetadata> {
    let metadata_path = path.join(INTERNAL_DIR).join(METADATA_FILE);
    let data = fs::read(metadata_path)
        .map_err(|error| LibraryError::InvalidLibrary(format!("无法读取资料库元数据：{error}")))?;
    serde_json::from_slice(&data).map_err(LibraryError::from)
}

fn open_database(path: &Path) -> LibraryResult<Connection> {
    let database_path = path.join(INTERNAL_DIR).join(DATABASE_FILE);
    let connection = Connection::open(database_path)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    Ok(connection)
}

fn initialize_schema(connection: &Connection) -> LibraryResult<()> {
    let _: String = connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS app_metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS collections (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            parent_id TEXT REFERENCES collections(id) ON DELETE SET NULL,
            is_inbox INTEGER NOT NULL DEFAULT 0 CHECK (is_inbox IN (0, 1)),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE UNIQUE INDEX IF NOT EXISTS collections_one_inbox
            ON collections(is_inbox)
            WHERE is_inbox = 1;

        CREATE TABLE IF NOT EXISTS documents (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            description TEXT,
            document_date TEXT,
            collection_id TEXT NOT NULL REFERENCES collections(id),
            file_name TEXT NOT NULL,
            file_type TEXT NOT NULL,
            file_size INTEGER,
            content_hash TEXT,
            file_modified_at INTEGER,
            library_path TEXT NOT NULL,
            processing_status TEXT NOT NULL,
            index_status TEXT NOT NULL,
            error_stage TEXT,
            error_message TEXT,
            imported_at TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            deleted_at TEXT,
            original_collection_id TEXT
        );

        CREATE INDEX IF NOT EXISTS documents_collection_idx
            ON documents(collection_id);
        CREATE INDEX IF NOT EXISTS documents_processing_status_idx
            ON documents(processing_status);
        CREATE INDEX IF NOT EXISTS documents_content_hash_idx
            ON documents(content_hash);
        CREATE INDEX IF NOT EXISTS documents_active_import_order_idx
            ON documents(imported_at DESC, id DESC)
            WHERE deleted_at IS NULL;
        CREATE INDEX IF NOT EXISTS documents_pending_queue_idx
            ON documents(index_status, processing_status, imported_at, id)
            WHERE deleted_at IS NULL;

        CREATE TABLE IF NOT EXISTS tags (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS document_tags (
            document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            tag_id TEXT NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
            PRIMARY KEY (document_id, tag_id)
        );
        CREATE INDEX IF NOT EXISTS document_tags_tag_idx
            ON document_tags(tag_id, document_id);

        CREATE TABLE IF NOT EXISTS sources (
            id TEXT PRIMARY KEY,
            document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            source_path TEXT NOT NULL,
            source_identifier TEXT NOT NULL,
            last_imported_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS sources_document_idx
            ON sources(document_id);
        CREATE INDEX IF NOT EXISTS sources_identifier_idx
            ON sources(source_identifier);

        CREATE VIRTUAL TABLE IF NOT EXISTS document_search USING fts5(
            document_id UNINDEXED,
            title,
            description,
            extracted_text,
            tokenize = 'trigram'
        );
        ",
    )?;

    ensure_column(connection, "documents", "original_collection_id", "TEXT")?;
    ensure_column(connection, "documents", "file_modified_at", "INTEGER")?;
    connection.execute(
        "CREATE INDEX IF NOT EXISTS documents_deleted_at_idx ON documents(deleted_at)",
        [],
    )?;

    let timestamp = now();
    connection.execute(
        "
        INSERT OR IGNORE INTO collections
            (id, name, parent_id, is_inbox, created_at, updated_at)
        VALUES ('inbox', '收件箱', NULL, 1, ?1, ?1)
        ",
        params![timestamp],
    )?;
    connection.execute(
        "INSERT OR REPLACE INTO app_metadata (key, value) VALUES ('formatVersion', ?1)",
        params![FORMAT_VERSION.to_string()],
    )?;
    connection.execute(
        "INSERT OR IGNORE INTO schema_migrations (version, applied_at) VALUES (1, ?1)",
        params![timestamp],
    )?;
    connection.execute(
        "INSERT OR IGNORE INTO schema_migrations (version, applied_at) VALUES (2, ?1)",
        params![timestamp],
    )?;
    Ok(())
}

fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> LibraryResult<()> {
    let exists = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
        params![table, column],
        |row| row.get::<_, bool>(0),
    )?;
    if !exists {
        connection.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {definition}"
        ))?;
    }
    Ok(())
}

fn normalize_source_file(path: &Path) -> LibraryResult<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(LibraryError::ImportFile("请选择要导入的文件。".to_string()));
    }

    if !path.exists() {
        return Err(LibraryError::ImportFile(format!(
            "源文件不存在：{}",
            path.display()
        )));
    }

    let path = dunce_canonicalize(path)?;
    if !path.is_file() {
        return Err(LibraryError::ImportFile(format!(
            "只能导入文件：{}",
            path.display()
        )));
    }
    Ok(path)
}

fn supported_file_type(path: &Path) -> Option<&'static str> {
    capability_for_path(path).map(|capability| capability.display_type.as_str())
}

fn validate_file_content(
    path: &Path,
    capability: &super::formats::DocumentFormatCapability,
) -> Result<(), String> {
    match capability.validation {
        ValidationStrategy::PdfSignature => {
            let prefix = read_prefix(path, 1024)?;
            if prefix.windows(5).any(|window| window == b"%PDF-") {
                Ok(())
            } else {
                Err("文件内容不是有效的 PDF。".to_string())
            }
        }
        ValidationStrategy::JpegSignature => {
            expect_prefix(path, &[0xFF, 0xD8, 0xFF], "文件内容不是有效的 JPG。")
        }
        ValidationStrategy::PngSignature => expect_prefix(
            path,
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
            "文件内容不是有效的 PNG。",
        ),
        ValidationStrategy::DocxPackage => expect_prefix(path, b"PK", "文件内容不是有效的 DOCX。"),
        ValidationStrategy::PlainText => Ok(()),
        ValidationStrategy::PptxPackage => validate_pptx_package(path),
    }
}

fn validate_pptx_package(path: &Path) -> Result<(), String> {
    let archive = fs::read(path).map_err(|error| format!("无法读取文件内容：{error}"))?;
    validate_pptx_bytes(&archive).map_err(|error| error.to_string())
}

fn validate_pptx_bytes(archive: &[u8]) -> LibraryResult<()> {
    if archive.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return Err(LibraryError::ImportFile(
            "PPTX 已加密或使用受保护容器，无法导入。".to_string(),
        ));
    }
    if !archive.starts_with(b"PK\x03\x04") {
        return Err(LibraryError::ImportFile(
            "文件内容不是有效的 PPTX：缺少 OOXML 压缩包结构。".to_string(),
        ));
    }
    ooxml::validate_pptx_package(archive).map(|_| ())
}

fn expect_prefix(path: &Path, expected: &[u8], message: &str) -> Result<(), String> {
    let prefix = read_prefix(path, expected.len())?;
    if prefix == expected {
        Ok(())
    } else {
        Err(message.to_string())
    }
}

fn read_prefix(path: &Path, max_bytes: usize) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|error| format!("无法读取文件内容：{error}"))?;
    let mut buffer = vec![0_u8; max_bytes];
    let mut filled = 0;

    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("无法读取文件内容：{error}")),
        }
    }

    buffer.truncate(filled);
    Ok(buffer)
}

fn display_file_name(source_path: &str) -> String {
    Path::new(source_path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| source_path.to_string())
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];

    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

fn is_png(bytes: &[u8]) -> bool {
    bytes.len() >= 24
        && bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
        && &bytes[12..16] == b"IHDR"
        && u32::from_be_bytes(bytes[16..20].try_into().unwrap_or_default()) > 0
        && u32::from_be_bytes(bytes[20..24].try_into().unwrap_or_default()) > 0
}

fn is_supported_thumbnail_image(bytes: &[u8]) -> bool {
    image_dimensions(bytes).is_some_and(|(width, height)| {
        (160..=4096).contains(&width)
            && (90..=4096).contains(&height)
            && (0.5..=3.0).contains(&(width as f64 / height as f64))
    })
}

fn decode_validated_thumbnail_data_url(
    thumbnail_data_url: &str,
) -> LibraryResult<(&'static str, Vec<u8>)> {
    let (media_type, encoded) =
        if let Some(encoded) = thumbnail_data_url.strip_prefix("data:image/jpeg;base64,") {
            ("image/jpeg", encoded)
        } else if let Some(encoded) = thumbnail_data_url.strip_prefix("data:image/jpg;base64,") {
            ("image/jpeg", encoded)
        } else if let Some(encoded) = thumbnail_data_url.strip_prefix("data:image/png;base64,") {
            ("image/png", encoded)
        } else {
            return Err(LibraryError::Preview(
                "缩略图必须是 PNG 或 JPEG 图片。".to_string(),
            ));
        };
    let bytes = BASE64
        .decode(encoded)
        .map_err(|_| LibraryError::Preview("缩略图图片数据无效。".to_string()))?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err(LibraryError::Preview(
            "缩略图图片超过 2 MB 限制。".to_string(),
        ));
    }
    if !is_supported_thumbnail_image(&bytes) {
        return Err(LibraryError::Preview(
            "缩略图必须是尺寸合理的真实 PNG 或 JPEG 图片，不能使用类型图标。".to_string(),
        ));
    }
    Ok((media_type, bytes))
}

fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        if bytes.len() < 24 || &bytes[12..16] != b"IHDR" {
            return None;
        }
        let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
        let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
        return (width > 0 && height > 0).then_some((width, height));
    }
    jpeg_dimensions(bytes)
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut cursor = 2_usize;
    while cursor + 4 <= bytes.len() {
        if bytes[cursor] != 0xFF {
            cursor += 1;
            continue;
        }
        while cursor < bytes.len() && bytes[cursor] == 0xFF {
            cursor += 1;
        }
        let marker = *bytes.get(cursor)?;
        cursor += 1;
        if marker == 0xD9 || marker == 0xDA {
            return None;
        }
        if matches!(marker, 0x01 | 0xD0..=0xD7) {
            continue;
        }
        let segment_length =
            u16::from_be_bytes(bytes.get(cursor..cursor + 2)?.try_into().ok()?) as usize;
        if segment_length < 2 || cursor + segment_length > bytes.len() {
            return None;
        }
        if matches!(
            marker,
            0xC0 | 0xC1
                | 0xC2
                | 0xC3
                | 0xC5
                | 0xC6
                | 0xC7
                | 0xC9
                | 0xCA
                | 0xCB
                | 0xCD
                | 0xCE
                | 0xCF
        ) {
            let dimensions = bytes.get(cursor + 2..cursor + segment_length)?;
            if dimensions.len() < 5 {
                return None;
            }
            let height = u16::from_be_bytes([dimensions[1], dimensions[2]]) as u32;
            let width = u16::from_be_bytes([dimensions[3], dimensions[4]]) as u32;
            return (width > 0 && height > 0).then_some((width, height));
        }
        cursor += segment_length;
    }
    None
}

fn thumbnail_cache_path(thumbnail_dir: &Path, document_id: &str, version: &str) -> PathBuf {
    thumbnail_cache_path_for(thumbnail_dir, document_id, version, "png")
}

fn thumbnail_cache_path_for(
    thumbnail_dir: &Path,
    document_id: &str,
    version: &str,
    extension: &str,
) -> PathBuf {
    let safe_version = version
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(64)
        .collect::<String>();
    thumbnail_dir.join(format!("{document_id}-{safe_version}.{extension}"))
}

fn remove_other_thumbnail_versions(thumbnail_dir: &Path, document_id: &str, keep: &Path) {
    let Ok(entries) = fs::read_dir(thumbnail_dir) else {
        return;
    };
    let prefix = format!("{document_id}-");
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if path != keep
            && name.starts_with(&prefix)
            && (name.ends_with(".png") || name.ends_with(".jpg"))
        {
            let _ = fs::remove_file(path);
        }
    }
}

fn remove_thumbnail_versions(library_root: &Path, document_id: &str) {
    let thumbnail_dir = library_root.join(THUMBNAILS_DIR);
    let Ok(entries) = fs::read_dir(&thumbnail_dir) else {
        return;
    };
    let prefix = format!("{document_id}-");
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with(&prefix) && (name.ends_with(".png") || name.ends_with(".jpg")) {
            let _ = fs::remove_file(path);
        }
    }
}

fn rendered_page_cache_directory(library_id: &str, document_id: &str, version: &str) -> PathBuf {
    std::env::temp_dir()
        .join("personal-document-manager")
        .join("rendered-pages")
        .join(safe_cache_component(library_id))
        .join(safe_cache_component(document_id))
        .join(safe_cache_component(version))
}

fn safe_cache_component(value: &str) -> String {
    let safe = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .take(80)
        .collect::<String>();
    if safe.is_empty() {
        "unknown".to_string()
    } else {
        safe
    }
}

fn remove_other_render_versions(document_cache_directory: &Path, keep: &Path) {
    let Ok(entries) = fs::read_dir(document_cache_directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path != keep && path.is_dir() {
            let _ = fs::remove_dir_all(path);
        }
    }
}

fn data_url(media_type: &str, bytes: &[u8]) -> String {
    format!("data:{media_type};base64,{}", BASE64.encode(bytes))
}

fn read_utf8_text(path: &Path) -> LibraryResult<String> {
    let bytes = fs::read(path)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn ensure_document_copy_exists(path: &Path) -> LibraryResult<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(LibraryError::DocumentFileMissing(format!(
            "资料库副本不存在：{}",
            path.display()
        )))
    }
}

fn is_safe_external_url(url: &str) -> bool {
    if url.is_empty() || url.len() > 4096 || url.chars().any(char::is_control) {
        return false;
    }
    let lowercase = url.to_ascii_lowercase();
    lowercase.starts_with("https://") || lowercase.starts_with("http://")
}

fn estimate_pdf_page_count(bytes: &[u8]) -> Option<u32> {
    let mut maximum = 0_u32;
    for index in memchr_indices(bytes, b"/Count") {
        let mut cursor = index + b"/Count".len();
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let start = cursor;
        while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
            cursor += 1;
        }
        if cursor > start {
            if let Ok(value) = std::str::from_utf8(&bytes[start..cursor])
                .unwrap_or_default()
                .parse::<u32>()
            {
                maximum = maximum.max(value);
            }
        }
    }

    (maximum > 0).then_some(maximum)
}

fn memchr_indices(haystack: &[u8], needle: &[u8]) -> Vec<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return Vec::new();
    }

    haystack
        .windows(needle.len())
        .enumerate()
        .filter_map(|(index, window)| (window == needle).then_some(index))
        .collect()
}

fn extract_docx_text(path: &Path) -> LibraryResult<String> {
    let archive = fs::read(path)?;
    extract_docx_text_from_bytes(&archive)
}

fn extract_docx_text_from_bytes(archive: &[u8]) -> LibraryResult<String> {
    let xml = read_zip_entry(archive, "word/document.xml")?;
    let text = extract_wordprocessing_text(&xml)?;
    if text.trim().is_empty() {
        Ok("文档中没有可提取的文本。".to_string())
    } else {
        Ok(text)
    }
}

fn inspect_docx_degradations(archive: &[u8]) -> Vec<String> {
    let Ok(entry_names) = zip_entry_names(archive) else {
        return Vec::new();
    };
    let mut features = Vec::new();
    let mut push_feature = |feature: &str| {
        if !features.iter().any(|candidate| candidate == feature) {
            features.push(feature.to_string());
        }
    };

    for entry_name in entry_names {
        let lowercase_name = entry_name.to_ascii_lowercase();
        if lowercase_name.contains("word/charts/") || lowercase_name.contains("word/diagrams/") {
            push_feature("图表或 SmartArt");
        }
        if lowercase_name.contains("word/embeddings/") {
            push_feature("嵌入对象");
        }
        if lowercase_name.ends_with("vbaproject.bin") || lowercase_name.contains("activex") {
            push_feature("宏或 ActiveX");
        }

        if !(lowercase_name.ends_with(".xml") || lowercase_name.ends_with(".rels")) {
            continue;
        }
        let Ok(xml) = read_zip_entry(archive, &entry_name) else {
            continue;
        };
        let xml_text = String::from_utf8_lossy(&xml);
        let lowercase_xml = xml_text.to_ascii_lowercase();

        if lowercase_xml.contains("<w:txbxcontent")
            || lowercase_xml.contains("<wps:txbx")
            || lowercase_xml.contains("<v:textbox")
            || lowercase_xml.contains("<v:shape")
        {
            push_feature("形状或文本框");
        }
        if lowercase_xml.contains("<w:fldchar")
            || lowercase_xml.contains("<w:instrtext")
            || lowercase_xml.contains("<w:fldsimple")
        {
            push_feature("字段");
        }
        if lowercase_xml.contains("<w:object")
            || lowercase_xml.contains("<o:oleobject")
            || lowercase_xml.contains("<w:oleobject")
        {
            push_feature("嵌入对象");
        }
        if lowercase_xml.contains("<w:altchunk") {
            push_feature("替换内容");
        }

        if lowercase_name.ends_with(".rels") {
            for relationship in xml_text.split("<Relationship").skip(1) {
                if relationship.contains("TargetMode=\"External\"")
                    && !relationship.contains("/hyperlink\"")
                {
                    push_feature("远程资源");
                    break;
                }
            }
        }
    }

    features
}

fn extract_pptx_text(path: &Path) -> LibraryResult<String> {
    let archive = fs::read(path)?;
    extract_pptx_text_from_bytes(&archive).map(|extraction| extraction.text)
}

fn extract_pptx_text_from_bytes(archive: &[u8]) -> LibraryResult<PptxExtraction> {
    let mut extraction = PptxExtraction::default();
    let graph = ooxml::pptx_presentation_graph(archive)?;
    let slide_names = graph.slide_parts;

    if slide_names.is_empty() {
        return Err(LibraryError::Preview(
            "PPTX 中没有可提取的幻灯片。".to_string(),
        ));
    }

    let mut slide_text = Vec::new();
    for (index, entry_name) in slide_names.iter().enumerate() {
        match read_zip_entry_for(archive, entry_name, "PPTX")
            .and_then(|xml| extract_powerpoint_text(&xml, false))
        {
            Ok(text) if !text.trim().is_empty() => slide_text.push(text),
            Ok(_) => {}
            Err(_) => push_unique_feature(
                &mut extraction.degraded_features,
                format!("第 {} 页 XML", index + 1),
            ),
        }
    }

    for entry_name in graph.chart_parts {
        match read_zip_entry_for(archive, &entry_name, "PPTX")
            .and_then(|xml| extract_powerpoint_text(&xml, true))
        {
            Ok(text) if !text.trim().is_empty() => slide_text.push(text),
            Ok(_) => {}
            Err(_) => push_unique_feature(
                &mut extraction.degraded_features,
                "图表标签 XML".to_string(),
            ),
        }
    }

    extraction.text = normalize_extracted_text(&slide_text.join("\n"));
    if extraction.text.is_empty() {
        extraction.text = "演示文稿中没有可提取的文本。".to_string();
    }
    Ok(extraction)
}

fn extract_powerpoint_text(xml: &[u8], chart_values: bool) -> LibraryResult<String> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut text = String::new();
    let mut in_text_node = false;
    let mut in_chart_value = false;

    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element)) => match element.local_name().as_ref() {
                "t" => in_text_node = true,
                "v" if chart_values => in_chart_value = true,
                "br" => text.push('\n'),
                "tab" => text.push('\t'),
                _ => {}
            },
            Ok(Event::Empty(element)) => match element.local_name().as_ref() {
                "br" => text.push('\n'),
                "tab" => text.push('\t'),
                _ => {}
            },
            Ok(Event::Text(value)) if in_text_node || in_chart_value => {
                text.push_str(&value.xml_content(quick_xml::XmlVersion::Implicit1_0));
            }
            Ok(Event::CData(value)) if in_text_node || in_chart_value => {
                text.push_str(&value);
            }
            Ok(Event::End(element)) => match element.local_name().as_ref() {
                "t" => in_text_node = false,
                "v" if chart_values => {
                    in_chart_value = false;
                    if !text.ends_with('\n') {
                        text.push('\n');
                    }
                }
                "p" if !text.ends_with('\n') => text.push('\n'),
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(error) => {
                return Err(LibraryError::Preview(format!(
                    "无法解析 PPTX 文本：{error}"
                )));
            }
            _ => {}
        }
        buffer.clear();
    }

    Ok(text
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string())
}

fn inspect_pptx_degradations(archive: &[u8]) -> Vec<String> {
    let Ok(entry_names) = zip_entry_names_for(archive, "PPTX") else {
        return Vec::new();
    };
    let mut features = Vec::new();

    for entry_name in entry_names {
        let lowercase_name = entry_name.to_ascii_lowercase();
        if lowercase_name.contains("ppt/diagrams/") {
            push_unique_feature(&mut features, "SmartArt".to_string());
        }
        if lowercase_name.contains("ppt/charts/") {
            push_unique_feature(&mut features, "复杂图表".to_string());
        }
        if lowercase_name.contains("ppt/embeddings/") {
            push_unique_feature(&mut features, "嵌入对象".to_string());
        }
        if lowercase_name.ends_with("vbaproject.bin") || lowercase_name.contains("activex") {
            push_unique_feature(&mut features, "宏或 ActiveX".to_string());
        }

        if !lowercase_name.ends_with(".rels") {
            continue;
        }
        let Ok(xml) = read_zip_entry_for(archive, &entry_name, "PPTX") else {
            continue;
        };
        let xml_text = String::from_utf8_lossy(&xml);
        for relationship in xml_text.split("<Relationship").skip(1) {
            if relationship.contains("TargetMode=\"External\"")
                && !relationship.contains("/hyperlink\"")
            {
                push_unique_feature(&mut features, "远程资源".to_string());
                break;
            }
        }
    }

    features
}

fn merge_features(features: &mut Vec<String>, additional: Vec<String>) {
    for feature in additional {
        push_unique_feature(features, feature);
    }
}

fn push_unique_feature(features: &mut Vec<String>, feature: String) {
    if !features.iter().any(|candidate| candidate == &feature) {
        features.push(feature);
    }
}

fn zip_entry_names(archive: &[u8]) -> LibraryResult<Vec<String>> {
    zip_entry_names_for(archive, "DOCX")
}

fn zip_entry_names_for(archive: &[u8], format: &str) -> LibraryResult<Vec<String>> {
    let eocd = find_zip_eocd(archive).ok_or_else(|| {
        LibraryError::Preview(format!("{format} 文件结构无效：找不到 ZIP 中央目录。"))
    })?;
    let entry_count = read_u16(archive, eocd + 10)? as usize;
    let mut cursor = read_u32(archive, eocd + 16)? as usize;
    let mut names = Vec::with_capacity(entry_count);

    for _ in 0..entry_count {
        if read_u32(archive, cursor)? != 0x0201_4b50 {
            return Err(LibraryError::Preview(format!(
                "{format} 文件结构无效：中央目录项损坏。"
            )));
        }
        let name_length = read_u16(archive, cursor + 28)? as usize;
        let extra_length = read_u16(archive, cursor + 30)? as usize;
        let comment_length = read_u16(archive, cursor + 32)? as usize;
        let name_start = cursor + 46;
        let name_end = name_start.checked_add(name_length).ok_or_else(|| {
            LibraryError::Preview(format!("{format} 文件结构无效：文件名长度溢出。"))
        })?;
        let name = archive.get(name_start..name_end).ok_or_else(|| {
            LibraryError::Preview(format!("{format} 文件结构无效：文件名超出文件范围。"))
        })?;
        names.push(String::from_utf8_lossy(name).into_owned());
        cursor = name_end
            .checked_add(extra_length)
            .and_then(|value| value.checked_add(comment_length))
            .ok_or_else(|| {
                LibraryError::Preview(format!("{format} 文件结构无效：目录项长度溢出。"))
            })?;
    }

    Ok(names)
}

fn read_zip_entry(archive: &[u8], target_name: &str) -> LibraryResult<Vec<u8>> {
    read_zip_entry_for(archive, target_name, "DOCX")
}

fn read_zip_entry_for(archive: &[u8], target_name: &str, format: &str) -> LibraryResult<Vec<u8>> {
    let eocd = find_zip_eocd(archive).ok_or_else(|| {
        LibraryError::Preview(format!("{format} 文件结构无效：找不到 ZIP 中央目录。"))
    })?;
    let entry_count = read_u16(archive, eocd + 10)? as usize;
    let mut cursor = read_u32(archive, eocd + 16)? as usize;

    for _ in 0..entry_count {
        if read_u32(archive, cursor)? != 0x0201_4b50 {
            return Err(LibraryError::Preview(format!(
                "{format} 文件结构无效：中央目录项损坏。"
            )));
        }
        let flags = read_u16(archive, cursor + 8)?;
        let compression = read_u16(archive, cursor + 10)?;
        let compressed_size = read_u32(archive, cursor + 20)? as usize;
        let uncompressed_size = read_u32(archive, cursor + 24)? as usize;
        let name_length = read_u16(archive, cursor + 28)? as usize;
        let extra_length = read_u16(archive, cursor + 30)? as usize;
        let comment_length = read_u16(archive, cursor + 32)? as usize;
        let local_header_offset = read_u32(archive, cursor + 42)? as usize;
        let name_start = cursor + 46;
        let name_end = name_start.checked_add(name_length).ok_or_else(|| {
            LibraryError::Preview(format!("{format} 文件结构无效：文件名长度溢出。"))
        })?;
        let name = archive.get(name_start..name_end).ok_or_else(|| {
            LibraryError::Preview(format!("{format} 文件结构无效：文件名超出文件范围。"))
        })?;

        if name == target_name.as_bytes() {
            if flags & 0x0001 != 0 {
                return Err(LibraryError::Preview(format!(
                    "{format} 中的内容已加密，无法提取。"
                )));
            }
            if compressed_size == u32::MAX as usize || uncompressed_size == u32::MAX as usize {
                return Err(LibraryError::Preview(format!(
                    "暂不支持 ZIP64 格式的 {format} 文件。"
                )));
            }

            let data_start = zip_local_data_start(archive, local_header_offset, format)?;
            let data_end = data_start.checked_add(compressed_size).ok_or_else(|| {
                LibraryError::Preview(format!("{format} 文件结构无效：正文长度溢出。"))
            })?;
            let compressed = archive.get(data_start..data_end).ok_or_else(|| {
                LibraryError::Preview(format!("{format} 文件结构无效：正文超出文件范围。"))
            })?;
            return match compression {
                0 => Ok(compressed.to_vec()),
                8 => {
                    let mut decoder = DeflateDecoder::new(Cursor::new(compressed));
                    let mut output = Vec::with_capacity(uncompressed_size);
                    decoder.read_to_end(&mut output)?;
                    Ok(output)
                }
                method => Err(LibraryError::Preview(format!(
                    "{format} 使用了不支持的压缩方式：{method}。"
                ))),
            };
        }

        cursor = name_end
            .checked_add(extra_length)
            .and_then(|value| value.checked_add(comment_length))
            .ok_or_else(|| {
                LibraryError::Preview(format!("{format} 文件结构无效：目录项长度溢出。"))
            })?;
    }

    Err(LibraryError::Preview(format!(
        "{format} 文件缺少 {target_name}。"
    )))
}

fn find_zip_eocd(archive: &[u8]) -> Option<usize> {
    let search_start = archive.len().saturating_sub(65_557);
    archive[search_start..]
        .windows(4)
        .rposition(|window| window == b"PK\x05\x06")
        .map(|index| search_start + index)
}

fn zip_local_data_start(
    archive: &[u8],
    local_header_offset: usize,
    format: &str,
) -> LibraryResult<usize> {
    if read_u32(archive, local_header_offset)? != 0x0403_4b50 {
        return Err(LibraryError::Preview(format!(
            "{format} 文件结构无效：本地文件头损坏。"
        )));
    }
    let name_length = read_u16(archive, local_header_offset + 26)? as usize;
    let extra_length = read_u16(archive, local_header_offset + 28)? as usize;
    local_header_offset
        .checked_add(30)
        .and_then(|value| value.checked_add(name_length))
        .and_then(|value| value.checked_add(extra_length))
        .ok_or_else(|| LibraryError::Preview(format!("{format} 文件结构无效：本地头长度溢出。")))
}

fn read_u16(bytes: &[u8], offset: usize) -> LibraryResult<u16> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| LibraryError::Preview("DOCX 文件结构无效：文件意外结束。".to_string()))?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> LibraryResult<u32> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| LibraryError::Preview("DOCX 文件结构无效：文件意外结束。".to_string()))?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn extract_wordprocessing_text(xml: &[u8]) -> LibraryResult<String> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut text = String::new();
    let mut in_text_node = false;

    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element)) => match element.local_name().as_ref() {
                "t" => in_text_node = true,
                "tab" => text.push('\t'),
                "br" | "cr" => text.push('\n'),
                _ => {}
            },
            Ok(Event::Empty(element)) => match element.local_name().as_ref() {
                "tab" => text.push('\t'),
                "br" | "cr" => text.push('\n'),
                _ => {}
            },
            Ok(Event::Text(value)) if in_text_node => {
                text.push_str(&value.xml_content(quick_xml::XmlVersion::Implicit1_0));
            }
            Ok(Event::CData(value)) if in_text_node => {
                text.push_str(&value);
            }
            Ok(Event::End(element)) => match element.local_name().as_ref() {
                "t" => in_text_node = false,
                "p" if !text.ends_with('\n') => text.push('\n'),
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(error) => {
                return Err(LibraryError::Preview(format!(
                    "无法解析 DOCX 文本：{error}"
                )));
            }
            _ => {}
        }
        buffer.clear();
    }

    Ok(text
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string())
}

fn normalize_search_filters(
    filters: DocumentSearchFilters,
) -> LibraryResult<DocumentSearchFilters> {
    let filters = DocumentSearchFilters {
        collection_id: normalize_optional_text(filters.collection_id),
        tag_id: normalize_optional_text(filters.tag_id),
        file_type: normalize_optional_text(filters.file_type).and_then(|file_type| {
            canonical_file_type(&file_type)
                .map(str::to_string)
                .or_else(|| Some(file_type.to_ascii_uppercase()))
        }),
        document_date_from: normalize_optional_text(filters.document_date_from),
        document_date_to: normalize_optional_text(filters.document_date_to),
    };
    if let Some(date) = filters.document_date_from.as_deref() {
        validate_document_date(date)?;
    }
    if let Some(date) = filters.document_date_to.as_deref() {
        validate_document_date(date)?;
    }
    if let (Some(from), Some(to)) = (
        filters.document_date_from.as_deref(),
        filters.document_date_to.as_deref(),
    ) {
        if from > to {
            return Err(LibraryError::InvalidDocumentMetadata(
                "文档日期起始值不能晚于结束值。".to_string(),
            ));
        }
    }
    Ok(filters)
}

fn load_filtered_documents(
    connection: &Connection,
    filters: &DocumentSearchFilters,
) -> LibraryResult<Vec<DocumentSummary>> {
    let mut statement = connection.prepare(
        "
        SELECT
            d.id,
            d.title,
            d.file_name,
            d.file_type,
            d.file_size,
            d.content_hash,
            d.collection_id,
            d.processing_status,
            d.index_status,
            d.error_stage,
            d.error_message,
            d.imported_at,
            COALESCE(s.source_path, ''),
            COALESCE(s.source_identifier, ''),
            COALESCE(s.last_imported_at, d.imported_at),
            d.description,
            d.document_date
        FROM documents d
        LEFT JOIN sources s ON s.document_id = d.id
        WHERE d.deleted_at IS NULL
          AND (?1 IS NULL OR d.collection_id = ?1)
          AND (
              ?2 IS NULL OR EXISTS (
                  SELECT 1
                  FROM document_tags dt
                  WHERE dt.document_id = d.id AND dt.tag_id = ?2
              )
          )
          AND (?3 IS NULL OR UPPER(d.file_type) = UPPER(?3))
          AND (
              ?4 IS NULL OR
              (d.document_date IS NOT NULL AND d.document_date >= ?4)
          )
          AND (
              ?5 IS NULL OR
              (d.document_date IS NOT NULL AND d.document_date <= ?5)
          )
        ORDER BY d.imported_at DESC, d.id DESC
        ",
    )?;
    let mut documents = statement
        .query_map(
            params![
                filters.collection_id.as_deref(),
                filters.tag_id.as_deref(),
                filters.file_type.as_deref(),
                filters.document_date_from.as_deref(),
                filters.document_date_to.as_deref(),
            ],
            document_from_row,
        )?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    let tags = load_all_document_tags(connection)?;
    for document in &mut documents {
        document.tags = tags.get(&document.id).cloned().unwrap_or_default();
    }
    Ok(documents)
}

fn load_collection_names(connection: &Connection) -> LibraryResult<HashMap<String, String>> {
    let mut statement = connection.prepare("SELECT id, name FROM collections")?;
    let names = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<HashMap<_, _>, _>>()?;
    Ok(names)
}

fn metadata_matches(
    document: &DocumentSummary,
    query: &str,
    collection_names: &HashMap<String, String>,
) -> bool {
    let query = query.to_lowercase();
    let mut values = vec![
        document.title.clone(),
        document.description.clone().unwrap_or_default(),
        document.file_name.clone(),
        document.file_type.clone(),
        document.document_date.clone().unwrap_or_default(),
        document.source_path.clone(),
    ];
    values.extend(document.tags.iter().map(|tag| tag.name.clone()));
    if let Some(collection_name) = collection_names.get(&document.collection_id) {
        values.push(collection_name.clone());
    }
    values
        .iter()
        .any(|value| value.to_lowercase().contains(&query))
}

fn fts_phrase(query: &str) -> String {
    format!("\"{}\"", query.replace('"', "\"\""))
}

fn extract_search_text(path: &Path, file_type: &str) -> LibraryResult<String> {
    let capability = require_capability_for_file_type(file_type)?;
    match capability.text_extraction {
        TextExtractionStrategy::PdfText => extract_pdf_text(path),
        TextExtractionStrategy::DocxText => extract_docx_text(path),
        TextExtractionStrategy::PlainText => read_utf8_text(path),
        TextExtractionStrategy::None => Ok(String::new()),
        TextExtractionStrategy::PptxText => extract_pptx_text(path),
    }
}

fn extract_pdf_text(path: &Path) -> LibraryResult<String> {
    let bytes = fs::read(path)?;
    let mut cursor = 0;
    let mut text = String::new();

    while let Some(relative_stream) = find_bytes(&bytes[cursor..], b"stream") {
        let stream_keyword = cursor + relative_stream;
        let dictionary_start = bytes[..stream_keyword]
            .windows(2)
            .rposition(|window| window == b"<<")
            .unwrap_or(0);
        let dictionary = &bytes[dictionary_start..stream_keyword];
        let mut data_start = stream_keyword + b"stream".len();
        if bytes.get(data_start) == Some(&b'\r') {
            data_start += 1;
        }
        if bytes.get(data_start) == Some(&b'\n') {
            data_start += 1;
        }
        let Some(relative_end) = find_bytes(&bytes[data_start..], b"endstream") else {
            break;
        };
        let data_end = data_start + relative_end;
        let stream = &bytes[data_start..data_end];
        let decoded = decode_pdf_stream(dictionary, stream)?;
        let stream_text = extract_pdf_content_text(&decoded);
        if !stream_text.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&stream_text);
        }
        cursor = data_end + b"endstream".len();
    }

    Ok(normalize_extracted_text(&text))
}

fn decode_pdf_stream(dictionary: &[u8], stream: &[u8]) -> LibraryResult<Vec<u8>> {
    let dictionary = String::from_utf8_lossy(dictionary);
    if dictionary.contains("FlateDecode") {
        let mut decoder = ZlibDecoder::new(Cursor::new(stream));
        let mut decoded = Vec::new();
        decoder
            .read_to_end(&mut decoded)
            .map_err(|error| LibraryError::Preview(format!("无法解压 PDF 正文流：{error}")))?;
        Ok(decoded)
    } else {
        Ok(stream.to_vec())
    }
}

fn extract_pdf_content_text(content: &[u8]) -> String {
    if find_bytes(content, b"BT").is_none()
        && find_bytes(content, b"Tj").is_none()
        && find_bytes(content, b"TJ").is_none()
    {
        return String::new();
    }

    let mut cursor = 0;
    let mut parts = Vec::new();

    while cursor < content.len() {
        match content[cursor] {
            b'(' => {
                if let Some((bytes, next)) = parse_pdf_literal(content, cursor + 1) {
                    let text = decode_pdf_text_bytes(&bytes);
                    if !text.trim().is_empty() {
                        parts.push(text);
                    }
                    cursor = next;
                } else {
                    break;
                }
            }
            b'<' if content.get(cursor + 1) != Some(&b'<') => {
                if let Some((bytes, next)) = parse_pdf_hex_string(content, cursor + 1) {
                    let text = decode_pdf_text_bytes(&bytes);
                    if !text.trim().is_empty() {
                        parts.push(text);
                    }
                    cursor = next;
                } else {
                    break;
                }
            }
            _ => cursor += 1,
        }
    }

    parts.join(" ")
}

fn parse_pdf_literal(content: &[u8], mut cursor: usize) -> Option<(Vec<u8>, usize)> {
    let mut depth = 1_u32;
    let mut output = Vec::new();

    while cursor < content.len() {
        let byte = content[cursor];
        cursor += 1;
        match byte {
            b'\\' => {
                let escaped = *content.get(cursor)?;
                cursor += 1;
                match escaped {
                    b'n' => output.push(b'\n'),
                    b'r' => output.push(b'\r'),
                    b't' => output.push(b'\t'),
                    b'b' => output.push(0x08),
                    b'f' => output.push(0x0C),
                    b'\r' => {
                        if content.get(cursor) == Some(&b'\n') {
                            cursor += 1;
                        }
                    }
                    b'\n' => {}
                    b'0'..=b'7' => {
                        let mut value = u16::from(escaped - b'0');
                        for _ in 0..2 {
                            match content.get(cursor) {
                                Some(next @ b'0'..=b'7') => {
                                    value = value * 8 + u16::from(*next - b'0');
                                    cursor += 1;
                                }
                                _ => break,
                            }
                        }
                        output.push(value as u8);
                    }
                    other => output.push(other),
                }
            }
            b'(' => {
                depth += 1;
                output.push(byte);
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some((output, cursor));
                }
                output.push(byte);
            }
            other => output.push(other),
        }
    }

    None
}

fn parse_pdf_hex_string(content: &[u8], mut cursor: usize) -> Option<(Vec<u8>, usize)> {
    let mut digits = Vec::new();
    while cursor < content.len() {
        let byte = content[cursor];
        cursor += 1;
        if byte == b'>' {
            if digits.len() % 2 == 1 {
                digits.push(0);
            }
            let bytes = digits
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| {
                    let high = hex_digit(pair[0])?;
                    let low = hex_digit(pair[1])?;
                    Some((high << 4) | low)
                })
                .collect::<Option<Vec<_>>>()?;
            return Some((bytes, cursor));
        }
        if byte.is_ascii_whitespace() {
            continue;
        }
        if !byte.is_ascii_hexdigit() {
            return None;
        }
        digits.push(byte);
    }
    None
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn decode_pdf_text_bytes(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return decode_utf16_be(&bytes[2..]);
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let units = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        return String::from_utf16_lossy(&units);
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }
    if bytes.len().is_multiple_of(2) {
        let decoded = decode_utf16_be(bytes);
        if !decoded.contains('\u{FFFD}') {
            return decoded;
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}

fn decode_utf16_be(bytes: &[u8]) -> String {
    let units = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    String::from_utf16_lossy(&units)
}

fn normalize_extracted_text(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn document_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DocumentSummary> {
    let processing_status: String = row.get(7)?;
    let processing_status = DocumentProcessingStatus::from_database(&processing_status)
        .ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                7,
                rusqlite::types::Type::Text,
                Box::new(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unknown document processing status",
                )),
            )
        })?;
    let index_status: String = row.get(8)?;
    let index_status = IndexStatus::from_database(&index_status).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            8,
            rusqlite::types::Type::Text,
            Box::new(io::Error::new(
                io::ErrorKind::InvalidData,
                "unknown document index status",
            )),
        )
    })?;

    Ok(DocumentSummary {
        id: row.get(0)?,
        title: row.get(1)?,
        description: row.get(15)?,
        document_date: row.get(16)?,
        file_name: row.get(2)?,
        file_type: row.get(3)?,
        file_size: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
        content_hash: row.get(5)?,
        collection_id: row.get(6)?,
        tags: Vec::new(),
        processing_status,
        index_status,
        error_stage: row.get(9)?,
        error_message: row.get(10)?,
        imported_at: row.get(11)?,
        source_path: row.get(12)?,
        source_identifier: row.get(13)?,
        last_imported_at: row.get(14)?,
    })
}

fn normalize_path(path: &Path) -> LibraryResult<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(LibraryError::InvalidLocation(
            "请选择资料库目录。".to_string(),
        ));
    }

    let absolute = std::path::absolute(path)?;
    if absolute.exists() {
        return dunce_canonicalize(&absolute);
    }
    Ok(strip_verbatim_prefix(absolute))
}

fn dunce_canonicalize(path: &Path) -> LibraryResult<PathBuf> {
    let canonical = fs::canonicalize(path)?;
    Ok(strip_verbatim_prefix(canonical))
}

fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(stripped) = text.strip_prefix(r"\\?\") {
        PathBuf::from(stripped)
    } else {
        path
    }
}

fn summary_from_metadata(path: &Path, metadata: &LibraryMetadata) -> LibrarySummary {
    LibrarySummary {
        id: metadata.library_id.clone(),
        name: metadata.name.clone(),
        path: path.to_string_lossy().into_owned(),
        created_at: metadata.created_at.clone(),
    }
}

fn is_library_directory(path: &Path) -> bool {
    path.join(INTERNAL_DIR).join(METADATA_FILE).is_file()
        && path.join(INTERNAL_DIR).join(DATABASE_FILE).is_file()
}

fn directory_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("资料库")
        .to_string()
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn modified_at(metadata: &fs::Metadata) -> Option<i64> {
    let modified = metadata.modified().ok()?;
    let duration = modified.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(duration.as_nanos()).ok()
}

fn paths_equal(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

fn ensure_writable_for_candidate(path: &Path) -> Result<(), String> {
    let probe = if path.exists() {
        path.to_path_buf()
    } else {
        let mut ancestor = path.parent();
        let mut selected = None;
        while let Some(candidate) = ancestor {
            if candidate.exists() {
                selected = Some(candidate.to_path_buf());
                break;
            }
            ancestor = candidate.parent();
        }
        selected.unwrap_or_else(|| PathBuf::from("."))
    };

    ensure_writable(&probe)
}

fn ensure_writable(path: &Path) -> Result<(), String> {
    if !path.is_dir() {
        return Err("所选位置不是可写入的目录。".to_string());
    }

    let probe = path.join(format!(".pdm-write-test-{}", Uuid::new_v4()));
    match OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(probe);
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            Err("所选位置无法写入。请选择其他目录。".to_string())
        }
        Err(error) => Err(format!("无法验证目录写入权限：{error}")),
    }
}

fn reserved_location_reason(path: &Path) -> Option<String> {
    if is_root(path) {
        return Some("不能把整个磁盘根目录用作资料库。".to_string());
    }

    let candidate = comparable_path(path);
    let reserved = [
        std::env::var_os("WINDIR"),
        std::env::var_os("ProgramFiles"),
        std::env::var_os("ProgramFiles(x86)"),
        std::env::var_os("ProgramData"),
        std::env::var_os("USERPROFILE"),
    ];

    for root in reserved.into_iter().flatten() {
        let root = comparable_path(Path::new(&root));
        if candidate == root {
            return Some("系统目录不能用作资料库。".to_string());
        }
    }

    let protected_roots = [
        std::env::var_os("WINDIR"),
        std::env::var_os("ProgramFiles"),
        std::env::var_os("ProgramFiles(x86)"),
        std::env::var_os("ProgramData"),
    ];

    for root in protected_roots.into_iter().flatten() {
        let root = comparable_path(Path::new(&root));
        if candidate.starts_with(&format!("{root}\\")) {
            return Some("系统目录不能用作资料库。".to_string());
        }
    }

    None
}

fn is_root(path: &Path) -> bool {
    let text = path.to_string_lossy();
    let trimmed = text.trim_end_matches(['\\', '/']);
    trimmed.ends_with(':') || trimmed.is_empty()
}

fn comparable_path(path: &Path) -> String {
    strip_verbatim_prefix(path.to_path_buf())
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_ascii_lowercase()
}

fn cloud_sync_warning(path: &Path) -> Option<CloudSyncWarning> {
    let providers = [
        ("OneDrive", "onedrive"),
        ("Dropbox", "dropbox"),
        ("Google Drive", "google drive"),
        ("iCloud Drive", "icloud drive"),
        ("Box", "box sync"),
        ("pCloud", "pcloud"),
        ("坚果云", "nutstore"),
    ];

    for component in path.components() {
        let component = component.as_os_str().to_string_lossy().to_ascii_lowercase();
        for (provider, marker) in providers {
            if component.contains(marker) {
                return Some(CloudSyncWarning {
                    provider: provider.to_string(),
                    message: format!(
                        "这个位置看起来位于 {provider} 同步目录中。同步冲突可能损坏资料库，建议改用本地非同步目录。"
                    ),
                });
            }
        }
    }

    None
}

fn cleanup_failed_creation(path: &Path, root_created: bool) {
    let _ = fs::remove_dir_all(path.join(INTERNAL_DIR));
    let _ = fs::remove_dir_all(path.join(DOCUMENTS_DIR));
    let _ = fs::remove_dir_all(path.join(TRASH_DIR));
    let _ = fs::remove_dir_all(path.join(THUMBNAILS_DIR));
    if root_created {
        let _ = fs::remove_dir(path);
    }
}
