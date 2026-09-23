use std::collections::{HashMap, HashSet, VecDeque};
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
    require_capability_for_file_type, unsupported_message, DocumentFormatId, PreviewStrategy,
    TextExtractionStrategy, ThumbnailStrategy, ValidationStrategy,
};
use super::limits::{
    bound_extracted_text, read_stream_limited, ArchiveLimits, ExpansionBudget,
    MAX_EXTRACTED_TEXT_CHARS, MAX_TABLE_PREVIEW_ROWS,
};
use super::models::{
    BatchDocumentItemResult, BatchDocumentItemStatus, BatchDocumentOperation,
    BatchDocumentOperationRequest, BatchDocumentOperationResult, BootstrapState, CloudSyncWarning,
    ClassificationPreviewItem, ClassificationPreviewRequest, ClassificationPreviewResponse,
    ClassificationRule, ClassificationRuleInput, ClassificationRuleOperation,
    CollectionDeleteResult, CollectionSummary,
    DocumentIndexChangedEvent, DocumentIndexPhase, DocumentMetadataUpdate, DocumentPreview,
    DocumentProcessingStatus, DocumentSearchFilters, DocumentSearchQuery, DocumentSearchResponse,
    DocumentSearchResult, DocumentSummary, DocumentThumbnail, EmptyTrashItemResult,
    EmptyTrashItemStatus, EmptyTrashResult, ImportBatch, ImportDecision, ImportItemResult,
    ImportItemStatus, ImportProgress, ImportSource, IndexRunResult, IndexStatus,
    LibraryLocationInspection, LibraryMetadata, LibrarySummary, LocationStatus,
    ReceiveDirectoryListing, ReceiveDirectoryListingItem, ReceiveDirectoryOperation,
    ReceiveImportLogEntry, ReceiveSource, ReceiveSourceCandidate, ReceiveSourceCandidates,
    ReceiveSourceInput, ReceiveSourceKind, ReceiveSourceScanResult, ReceiveSourceStatus,
    RecentLibrary, RecentLibraryRecord, SearchMatchKind, TablePreviewRequest, TableSheet, TagSummary,
    TrashDocumentSummary,
};
use super::{ooxml, store, table, thumbnail};

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
const REPLACEMENT_BACKUP_SUFFIX: &str = ".previous";
const IMPORT_TEMPORARY_SUFFIX: &str = ".importing";
/// 纯文本读取的字节上限：UTF-8 每字符最多 4 字节，另加 4 字节用于识别截断。
const MAX_TEXT_READ_BYTES: u64 = MAX_EXTRACTED_TEXT_CHARS as u64 * 4 + 4;
/// 表格预览未指定列数时的默认列数；上限由 `limits` 模块统一约束。
const DEFAULT_TABLE_PREVIEW_COLUMNS: usize = 16;
/// 首次启用接收目录时一次列出的文件数上限，避免超大目录拖慢界面。
const MAX_RECEIVE_LISTING_ITEMS: usize = 500;
/// 用户在清单里完成选择时，最多为多少个「未勾选的既有文件」写入跳过记录。
///
/// 这是一次性的安全操作（清单只展示前 500 条，剩下的必须显式记为跳过，否则会被静默导入），
/// 上限取得比清单高得多，避免同一类漏洞在更大目录上重演。
const MAX_RECEIVE_SKIP_ENUMERATION: usize = 50_000;
/// 接收目录扫描的递归深度上限（QQ 的 FileRecv 常有按日期分层的子目录）。
const MAX_RECEIVE_SCAN_DEPTH: usize = 4;
/// 失败项在周期补扫里自动重试前的冷却时间（秒），避免刷日志。
const RECEIVE_RETRY_COOLDOWN_SECONDS: i64 = 60;
/// 接收目录里默认排除的临时/中间文件后缀。
const RECEIVE_TEMP_EXTENSIONS: [&str; 15] = [
    "tmp",
    "temp",
    "part",
    "partial",
    "crdownload",
    "download",
    "filepart",
    "bak",
    "db",
    "db-wal",
    "db-shm",
    "sqlite",
    "sqlite3",
    "dat",
    "lnk",
];

pub struct LibraryService {
    state_dir: PathBuf,
    current: Option<OpenLibrary>,
    recent: Vec<RecentLibraryRecord>,
    import_items: HashMap<String, ImportItemContext>,
    active_import_batches: HashMap<String, ActiveImportBatch>,
}

struct OpenLibrary {
    summary: LibrarySummary,
    connection: Connection,
}

/// 正在分步执行的导入批次。
///
/// 批次状态由 `LibraryService` 持有但**分步推进**：调用方每一步单独加锁，
/// 因此列表、搜索和预览请求可以在两步之间拿到锁，不必等整批导入结束。
/// 每一项仍会重新核对所属资料库（`begin_import_batch` 与 `import_batch_step`），
/// 所以 01/02 的库边界语义在分步后依然成立。
struct ActiveImportBatch {
    library: LibrarySummary,
    batch_id: String,
    entries: VecDeque<ScanEntry>,
    items: Vec<ImportItemResult>,
    total: usize,
    target_collection_id: Option<String>,
    /// 本批是否把已启用的分类规则应用到新建成功的文档上。
    classification: ClassificationMode,
    /// 接收目录导入时记录来源，逐项写入接收导入日志并更新来源状态。
    receive_source_id: Option<String>,
}

/// 本批新建文档是否应用分类规则。
///
/// 人工批量导入默认应用（与 `preview_classification` 的语义一致），
/// 用户选择「不应用」时由命令层显式传 `KeepExisting`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClassificationMode {
    ApplyRules,
    KeepExisting,
}

impl ClassificationMode {
    fn applies(self) -> bool {
        matches!(self, Self::ApplyRules)
    }
}

/// 一条源路径的分类结果；`matched_rule_ids` 按规则顺序排列。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ClassificationPlan {
    collection_id: String,
    tag_ids: Vec<String>,
    matched_rule_ids: Vec<String>,
}

struct ImportItemContext {
    owner_library: LibrarySummary,
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
                let mut scanned_library: Option<LibrarySummary> = None;

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
                        let Some(library) = service.current_library().cloned() else {
                            previous_fingerprints.clear();
                            quiet_deadline = None;
                            scanned_library = None;
                            continue;
                        };
                        let scan = match service.scan_external_changes(false) {
                            Ok(scan) => scan,
                            Err(_) => continue,
                        };
                        let fingerprints = service.external_file_fingerprints();
                        (scan, fingerprints, library)
                    };
                    let (scan, fingerprints, library) = scan;
                    if scanned_library.as_ref().is_some_and(|previous| {
                        previous.id != library.id || !paths_equal(&previous.path, &library.path)
                    }) {
                        previous_fingerprints.clear();
                        quiet_deadline = None;
                    }
                    scanned_library = Some(library.clone());

                    let fingerprints_changed =
                        !previous_fingerprints.is_empty() && fingerprints != previous_fingerprints;
                    previous_fingerprints = fingerprints;

                    if !scan.changed_document_ids.is_empty() || fingerprints_changed {
                        quiet_deadline = Some(Instant::now() + quiet_period);
                        if !scan.changed_document_ids.is_empty() {
                            on_event(DocumentIndexChangedEvent {
                                library: library.clone(),
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
                        if service.ensure_current_library(&library).is_err() {
                            quiet_deadline = None;
                            continue;
                        }
                        service.process_pending_external_changes()
                    };
                    let Ok(result) = result else {
                        quiet_deadline = None;
                        continue;
                    };
                    quiet_deadline = None;
                    on_event(DocumentIndexChangedEvent {
                        library,
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
            active_import_batches: HashMap::new(),
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
            return Err(creation_error_after_cleanup(&path, root_created, error));
        }

        let summary = summary_from_metadata(&path, &metadata);
        let connection = match open_database(&path) {
            Ok(connection) => connection,
            Err(error) => return Err(creation_error_after_cleanup(&path, root_created, error)),
        };
        if let Err(error) = initialize_schema(&connection) {
            drop(connection);
            return Err(creation_error_after_cleanup(&path, root_created, error));
        }
        if let Err(error) = self.record_recent(&summary) {
            drop(connection);
            return Err(creation_error_after_cleanup(&path, root_created, error));
        }
        self.current = Some(OpenLibrary {
            summary: summary.clone(),
            connection,
        });
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
        let recovery_failures = reconcile_interrupted_copy_operations(&path, &connection)?;

        let summary = summary_from_metadata(&path, &metadata);
        let mut candidate = OpenLibrary {
            summary: summary.clone(),
            connection,
        };
        Self::scan_external_changes_for(&mut candidate, true)?;
        // 无法自动恢复的结论必须在启动扫描之后写入：扫描会把内容不一致的副本改写成
        // “待索引”，随后重新索引就会丢掉恢复失败的原因，让用户看不到可诊断状态。
        for failure in &recovery_failures {
            report_recovery_failure(
                &candidate.connection,
                &failure.document_id,
                &failure.message,
            )?;
        }
        self.record_recent(&summary)?;
        self.current = Some(candidate);
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
        let temporary_path = import_temporary_path(&destination_directory);
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
        self.start_import_to_collection_with_progress(
            paths,
            None,
            ImportSource::FilePicker,
            on_progress,
        )
    }

    pub fn start_import_to_collection(
        &mut self,
        paths: Vec<String>,
        target_collection_id: Option<String>,
        source: ImportSource,
    ) -> LibraryResult<ImportBatch> {
        self.start_import_to_collection_with_progress(paths, target_collection_id, source, |_| {})
    }

    pub fn start_import_to_collection_with_progress<F>(
        &mut self,
        paths: Vec<String>,
        target_collection_id: Option<String>,
        source: ImportSource,
        on_progress: F,
    ) -> LibraryResult<ImportBatch>
    where
        F: FnMut(ImportProgress),
    {
        self.start_import_to_collection_with_classification(
            paths,
            target_collection_id,
            source,
            true,
            on_progress,
        )
    }

    /// 人工批量导入的入口：`apply_classification` 为假时完全保留原有导入行为
    /// （不套用规则集合与标签），对应界面里「不应用，按原有方式导入」。
    pub fn start_import_to_collection_with_classification<F>(
        &mut self,
        paths: Vec<String>,
        target_collection_id: Option<String>,
        source: ImportSource,
        apply_classification: bool,
        mut on_progress: F,
    ) -> LibraryResult<ImportBatch>
    where
        F: FnMut(ImportProgress),
    {
        let first = self.begin_import_batch_with_classification(
            paths,
            target_collection_id,
            source,
            apply_classification,
        )?;
        let batch_id = first.batch_id.clone();
        on_progress(first);
        let result = self.drive_import_batch(&batch_id, &mut on_progress);
        if result.is_err() {
            let _ = self.abort_import_batch(&batch_id);
        }
        result
    }

    /// 开始一批导入：扫描路径、校验目标集合并登记批次，返回批次的第一条进度事件。
    ///
    /// 本方法**不复制任何文件**。调用方随后在每一步之间释放服务锁，用
    /// [`Self::peek_import_progress`] 与 [`Self::import_batch_step`] 推进，
    /// 最后用 [`Self::finish_import_batch`] 收尾；失败或取消时调用
    /// [`Self::abort_import_batch`] 丢弃批次。
    pub fn begin_import_batch(
        &mut self,
        paths: Vec<String>,
        target_collection_id: Option<String>,
        source: ImportSource,
    ) -> LibraryResult<ImportProgress> {
        self.begin_import_batch_with_classification(
            paths,
            target_collection_id,
            source,
            true,
        )
    }

    /// 开始一批导入并显式指定是否应用分类规则。
    pub fn begin_import_batch_with_classification(
        &mut self,
        paths: Vec<String>,
        target_collection_id: Option<String>,
        source: ImportSource,
        apply_classification: bool,
    ) -> LibraryResult<ImportProgress> {
        self.begin_import_batch_with_mode(
            paths,
            target_collection_id,
            source,
            if apply_classification {
                ClassificationMode::ApplyRules
            } else {
                ClassificationMode::KeepExisting
            },
            None,
        )
    }

    /// 开始一批接收目录导入：记录来源，逐项写接收导入日志并应用分类规则。
    pub fn begin_receive_import_batch(
        &mut self,
        source_id: &str,
        paths: Vec<String>,
    ) -> LibraryResult<ImportProgress> {
        self.begin_import_batch_with_mode(
            paths,
            None,
            ImportSource::FilePicker,
            ClassificationMode::ApplyRules,
            Some(source_id.to_string()),
        )
    }

    fn begin_import_batch_with_mode(
        &mut self,
        paths: Vec<String>,
        target_collection_id: Option<String>,
        source: ImportSource,
        classification: ClassificationMode,
        receive_source_id: Option<String>,
    ) -> LibraryResult<ImportProgress> {
        let library = self
            .current_library()
            .cloned()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        if let Some(collection_id) = target_collection_id.as_deref() {
            let library = self
                .current
                .as_ref()
                .ok_or(LibraryError::NoCurrentLibrary)?;
            ensure_collection_exists(&library.connection, collection_id)?;
        }

        let batch_id = Uuid::new_v4().to_string();
        let entries = scan_import_paths(&paths, source);
        let total = entries.len();
        let first = entries.first();
        let progress = ImportProgress {
            library: library.clone(),
            batch_id: batch_id.clone(),
            total,
            completed: 0,
            current_file_name: first.and_then(ScanEntry::file_name),
            current_source_path: first.map(ScanEntry::source_path),
            item: None,
            finished: false,
        };
        self.active_import_batches.insert(
            batch_id.clone(),
            ActiveImportBatch {
                library,
                batch_id,
                entries: entries.into(),
                items: Vec::with_capacity(total),
                total,
                target_collection_id,
                classification,
                receive_source_id,
            },
        );
        Ok(progress)
    }

    /// 查看批次里下一个待处理条目对应的进度事件；返回 `None` 表示批次已处理完。
    pub fn peek_import_progress(&self, batch_id: &str) -> LibraryResult<Option<ImportProgress>> {
        let batch = self.active_import_batch(batch_id)?;
        let Some(entry) = batch.entries.front() else {
            return Ok(None);
        };
        Ok(Some(ImportProgress {
            library: batch.library.clone(),
            batch_id: batch.batch_id.clone(),
            total: batch.total,
            completed: batch.items.len(),
            current_file_name: entry.file_name(),
            current_source_path: Some(entry.source_path()),
            item: None,
            finished: false,
        }))
    }

    /// 处理批次里的下一个条目，返回「这一项已完成」的进度事件。
    ///
    /// 每次调用都会先核对批次所属资料库：导入期间切换资料库会让本批以
    /// `invalidLibrary` 中止，而不是把内容写进另一个资料库。
    pub fn import_batch_step(&mut self, batch_id: &str) -> LibraryResult<ImportProgress> {
        let library = {
            let batch = self.active_import_batch(batch_id)?;
            batch.library.clone()
        };
        self.ensure_current_library(&library)?;
        let library = self
            .current_library()
            .cloned()
            .ok_or(LibraryError::NoCurrentLibrary)?;

        let batch = self
            .active_import_batches
            .get_mut(batch_id)
            .ok_or_else(|| import_batch_not_found(batch_id))?;
        let Some(entry) = batch.entries.pop_front() else {
            return Err(import_batch_not_found(batch_id));
        };
        let target_collection_id = batch.target_collection_id.clone();
        let total = batch.total;
        let classification = batch.classification;
        let receive_source_id = batch.receive_source_id.clone();
        let current_file_name = entry.file_name();
        let current_source_path = entry.source_path();

        let item = match entry {
            ScanEntry::File(path) => self.process_import_file_with_target(
                &path,
                false,
                None,
                target_collection_id.clone(),
                classification,
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

        let batch = self
            .active_import_batches
            .get_mut(batch_id)
            .ok_or_else(|| import_batch_not_found(batch_id))?;
        batch.items.push(item.clone());
        let completed = batch.items.len();
        let batch_id_text = batch.batch_id.clone();

        // 接收目录导入逐项写日志：命中规则、实际集合与标签都留痕，供用户事后改正。
        if let Some(source_id) = receive_source_id.as_deref() {
            let matched_rule_ids = self
                .classification_plan(&item.source_path, classification, None)?
                .map(|plan| plan.matched_rule_ids)
                .unwrap_or_default();
            self.record_receive_import_log(source_id, &item, &matched_rule_ids)?;
        }

        Ok(ImportProgress {
            library,
            batch_id: batch_id_text,
            total,
            completed,
            current_file_name,
            current_source_path: Some(current_source_path),
            item: Some(item),
            finished: false,
        })
    }

    /// 返回整批结果并结束批次。
    pub fn finish_import_batch(&mut self, batch_id: &str) -> LibraryResult<ImportBatch> {
        let batch = self
            .active_import_batches
            .remove(batch_id)
            .ok_or_else(|| import_batch_not_found(batch_id))?;
        Ok(import_batch_from_items(
            batch.batch_id,
            batch.items,
            batch.target_collection_id,
        ))
    }

    /// 批次完成时的收尾进度事件（`finished = true`）。
    pub fn import_batch_finished_progress(&self, batch_id: &str) -> LibraryResult<ImportProgress> {
        let batch = self.active_import_batch(batch_id)?;
        Ok(ImportProgress {
            library: batch.library.clone(),
            batch_id: batch.batch_id.clone(),
            total: batch.total,
            completed: batch.items.len(),
            current_file_name: None,
            current_source_path: None,
            item: None,
            finished: true,
        })
    }

    /// 放弃批次（取消、出错或资料库已切换）；不触碰已经写入的文档与副本。
    pub fn abort_import_batch(&mut self, batch_id: &str) -> LibraryResult<()> {
        self.active_import_batches.remove(batch_id);
        Ok(())
    }

    fn active_import_batch(&self, batch_id: &str) -> LibraryResult<&ActiveImportBatch> {
        self.active_import_batches
            .get(batch_id)
            .ok_or_else(|| import_batch_not_found(batch_id))
    }

    /// 在当前已持有的服务锁内跑完整批，供同一把锁下的调用方复用。
    fn drive_import_batch<F>(
        &mut self,
        batch_id: &str,
        on_progress: &mut F,
    ) -> LibraryResult<ImportBatch>
    where
        F: FnMut(ImportProgress),
    {
        while let Some(before) = self.peek_import_progress(batch_id)? {
            on_progress(before);
            on_progress(self.import_batch_step(batch_id)?);
        }
        let finished = self.import_batch_finished_progress(batch_id)?;
        let batch = self.finish_import_batch(batch_id)?;
        on_progress(finished);
        Ok(batch)
    }

    pub fn resolve_import_item(
        &mut self,
        item_id: &str,
        decision: ImportDecision,
    ) -> LibraryResult<ImportItemResult> {
        self.ensure_import_item_belongs_to_current_library(item_id)?;
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
                self.create_new_document(item_id.to_string(), pending, ClassificationMode::ApplyRules)
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
                self.create_new_document(item_id.to_string(), pending, ClassificationMode::ApplyRules)
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
        self.ensure_import_item_belongs_to_current_library(item_id)?;
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
            ClassificationMode::ApplyRules,
        )
    }

    fn ensure_import_item_belongs_to_current_library(&self, item_id: &str) -> LibraryResult<()> {
        let current = self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        let context = self
            .import_items
            .get(item_id)
            .ok_or_else(|| import_item_not_found(item_id))?;
        if current.summary.id != context.owner_library.id
            || !paths_equal(&current.summary.path, &context.owner_library.path)
        {
            return Err(LibraryError::InvalidImportDecision(
                "该导入项属于其他资料库。请切回原资料库继续处理，或重新发起导入。".to_string(),
            ));
        }
        Ok(())
    }

    fn process_import_file_with_target(
        &mut self,
        source_path: &Path,
        force_import: bool,
        existing_item_id: Option<String>,
        target_collection_id: Option<String>,
        classification: ClassificationMode,
    ) -> LibraryResult<ImportItemResult> {
        let mut item = self.process_import_file_impl(
            source_path,
            force_import,
            existing_item_id,
            target_collection_id.clone(),
            classification,
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
        classification: ClassificationMode,
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
            classification,
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
        classification: ClassificationMode,
    ) -> LibraryResult<ImportItemResult> {
        let (resolved_collection_id, target_notice) =
            self.resolve_target_collection(pending.target_collection_id.as_deref())?;
        // 只有用户确实指定了目标集合（并且它仍然存在）时才算「显式目标」；
        // 否则由分类规则决定集合，未命中才落回收件箱。
        let explicit_collection_id = pending
            .target_collection_id
            .as_deref()
            .filter(|_| target_notice.is_none());
        let plan = self.classification_plan(
            &pending.source_path,
            classification,
            explicit_collection_id,
        )?;
        let collection_id = plan
            .as_ref()
            .map(|plan| plan.collection_id.clone())
            .unwrap_or(resolved_collection_id);
        let tag_ids = plan
            .as_ref()
            .map(|plan| plan.tag_ids.clone())
            .unwrap_or_default();
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
        let temporary_path = import_temporary_path(&destination_directory);
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
            for tag_id in &tag_ids {
                transaction
                    .execute(
                        "INSERT OR IGNORE INTO document_tags (document_id, tag_id) VALUES (?1, ?2)",
                        params![&document.id, tag_id],
                    )
                    .map_err(|error| ("tag", format!("无法应用分类标签：{error}")))?;
            }
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
        let temporary_path = import_temporary_path(&document_directory);
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
            let backup_path = replacement_backup_path(&document_directory);
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
            let backup_path = replacement_backup_path(&document_directory);
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
        let owner_library = self
            .current_library()
            .expect("创建待决导入项前必须打开资料库")
            .clone();
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
                owner_library,
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
        let owner_library = self
            .current_library()
            .expect("创建失败导入项前必须打开资料库")
            .clone();
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
                owner_library,
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
        let Some(library) = self.current.as_mut() else {
            return Ok(ExternalChangeScan::default());
        };
        Self::scan_external_changes_for(library, verify_content_hashes)
    }

    fn scan_external_changes_for(
        library: &mut OpenLibrary,
        verify_content_hashes: bool,
    ) -> LibraryResult<ExternalChangeScan> {
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

    /// 列出表格文档的工作表；非表格格式返回空列表。
    pub fn list_document_sheets(&self, document_id: &str) -> LibraryResult<Vec<TableSheet>> {
        let stored = self.load_stored_document_file(document_id)?;
        let path = self.document_copy_path(&stored)?;
        ensure_document_copy_exists(&path)?;
        let capability = require_capability_for_file_type(&stored.file_type)?;
        if capability.preview != PreviewStrategy::TablePaged {
            return Ok(Vec::new());
        }
        let bytes = read_table_bytes_within_limits(&path)?;
        Ok(table_sheets_for(&bytes, capability.id)?.into())
    }

    /// 读取表格文档的有限行列范围；失败以 `DocumentPreview::Failure` 返回，界面可解释。
    pub fn get_table_preview(
        &self,
        document_id: &str,
        request: TablePreviewRequest,
    ) -> LibraryResult<DocumentPreview> {
        let stored = self.load_stored_document_file(document_id)?;
        let path = self.document_copy_path(&stored)?;
        let capability = require_capability_for_file_type(&stored.file_type)?;
        if capability.preview != PreviewStrategy::TablePaged {
            return Ok(DocumentPreview::Unsupported {
                message: format!("{} 不是表格文档。", capability.display_type),
            });
        }
        match read_table_preview(&path, capability.id, &request) {
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
        expected_content_hash: &str,
        thumbnail_data_url: &str,
    ) -> LibraryResult<DocumentThumbnail> {
        let stored = self.load_stored_document_file(document_id)?;
        if stored.content_hash.as_deref() != Some(expected_content_hash) {
            return Err(LibraryError::StaleThumbnail(
                "缩略图已过期：文档内容版本已变化，已拒绝写入缓存。".to_string(),
            ));
        }
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
                text: bounded_preview_text(read_utf8_text(&path)?),
            }),
            PreviewStrategy::SafeMarkdown => Ok(DocumentPreview::Markdown {
                text: bounded_preview_text(read_utf8_text(&path)?),
            }),
            PreviewStrategy::DocxLayout => {
                let bytes = read_archive_within_limits(&path, ArchiveLimits::for_preview())?;
                let mut degraded_features = inspect_docx_degradations(&bytes);
                let sanitized = ooxml::sanitize_docx_package(&bytes)?;
                let text = bound_extracted_text(extract_docx_text_from_bytes(&bytes)?);
                if text.truncated {
                    push_unique_feature(
                        &mut degraded_features,
                        format!("正文超过 {MAX_EXTRACTED_TEXT_CHARS} 个字符，预览只显示前半部分"),
                    );
                }
                Ok(DocumentPreview::Docx {
                    data_url: data_url(
                        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                        &sanitized,
                    ),
                    text: text.text,
                    notice: if degraded_features.is_empty() {
                        "DOCX 版式预览为本地只读近似呈现。".to_string()
                    } else {
                        "DOCX 版式预览已呈现，部分复杂内容需要降级。".to_string()
                    },
                    degraded_features,
                })
            }
            PreviewStrategy::PptxPages => {
                let bytes = read_archive_within_limits(&path, ArchiveLimits::for_preview())?;
                let extraction = extract_pptx_text_from_bytes(&bytes)?;
                let mut degraded_features = inspect_pptx_degradations(&bytes);
                merge_features(&mut degraded_features, extraction.degraded_features);
                let sanitized = ooxml::sanitize_pptx_package(&bytes)?;
                let text = bound_extracted_text(extraction.text);
                if text.truncated {
                    push_unique_feature(
                        &mut degraded_features,
                        format!("正文超过 {MAX_EXTRACTED_TEXT_CHARS} 个字符，预览只显示前半部分"),
                    );
                }
                Ok(DocumentPreview::Pptx {
                    data_url: data_url(
                        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
                        &sanitized,
                    ),
                    text: text.text,
                    notice: if degraded_features.is_empty() {
                        "PPTX 版式预览为本地只读近似呈现。".to_string()
                    } else {
                        "PPTX 版式预览已呈现，部分复杂内容需要降级。".to_string()
                    },
                    degraded_features,
                })
            }
            // 表格文档：首屏返回第一段范围，翻页与切表由 get_table_preview 处理。
            PreviewStrategy::TablePaged => {
                read_table_preview(&path, capability.id, &TablePreviewRequest::default())
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

        let mut recent = self.recent.clone();
        recent.retain(|entry| !paths_equal(&entry.path, target.as_ref()));
        self.persist_recent(&recent)?;
        self.recent = recent;
        Ok(self.list_recent_libraries())
    }

    pub fn current_library(&self) -> Option<&LibrarySummary> {
        self.current.as_ref().map(|library| &library.summary)
    }

    /// 仅测试使用：借用当前资料库的连接，供存储层用例建表与校验。
    #[cfg(test)]
    pub(crate) fn with_test_connection<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        let library = self.current.as_ref().expect("测试需要已打开的资料库");
        f(&library.connection)
    }

    pub fn ensure_current_library(&self, expected: &LibrarySummary) -> LibraryResult<()> {
        let current = self
            .current_library()
            .ok_or(LibraryError::NoCurrentLibrary)?;
        if current.id != expected.id || !paths_equal(&current.path, &expected.path) {
            return Err(LibraryError::InvalidLibrary(
                "资料库已切换，请在当前资料库重新发起操作。".to_string(),
            ));
        }
        Ok(())
    }

    fn record_recent(&mut self, library: &LibrarySummary) -> LibraryResult<()> {        let target = normalize_path(Path::new(&library.path))?;
        let target_display = target.to_string_lossy().into_owned();

        let mut recent = self.recent.clone();
        recent.retain(|entry| !paths_equal(&entry.path, &target_display));
        recent.insert(
            0,
            RecentLibraryRecord {
                path: target_display,
                name: library.name.clone(),
                last_opened_at: now(),
            },
        );
        recent.truncate(MAX_RECENT_LIBRARIES);
        self.persist_recent(&recent)?;
        self.recent = recent;
        Ok(())
    }

    fn persist_recent(&self, recent: &[RecentLibraryRecord]) -> LibraryResult<()> {
        let path = self.state_dir.join(RECENT_FILE);
        let temporary = self
            .state_dir
            .join(format!(".{RECENT_FILE}.{}.tmp", Uuid::new_v4()));
        let data = serde_json::to_vec_pretty(recent)?;
        if let Err(error) = fs::write(&temporary, data) {
            let _ = fs::remove_file(&temporary);
            return Err(error.into());
        }
        if let Err(error) = fs::rename(&temporary, &path) {
            let _ = fs::remove_file(&temporary);
            return Err(error.into());
        }
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

impl LibraryService {
    fn library_connection(&self) -> LibraryResult<&Connection> {
        Ok(&self
            .current
            .as_ref()
            .ok_or(LibraryError::NoCurrentLibrary)?
            .connection)
    }

    /// 按当前启用规则为一条源路径算出分类计划；`KeepExisting` 时返回 `None`。
    fn classification_plan(
        &self,
        source_path: &str,
        mode: ClassificationMode,
        explicit_collection_id: Option<&str>,
    ) -> LibraryResult<Option<ClassificationPlan>> {
        if !mode.applies() {
            return Ok(None);
        }
        let Some(library) = self.current.as_ref() else {
            return Ok(None);
        };
        let rules = store::list_classification_rules(&library.connection)?;
        Ok(Some(plan_classification(
            &library.connection,
            &rules,
            source_path,
            explicit_collection_id,
        )?))
    }

    // ---- 分类规则（工作单 10） ----

    pub fn list_classification_rules(&self) -> LibraryResult<Vec<ClassificationRule>> {
        store::list_classification_rules(self.library_connection()?)
    }

    /// 应用一次规则编辑操作；引用了不存在的集合或标签时给出明确原因。
    pub fn apply_classification_rule_operation(
        &mut self,
        operation: ClassificationRuleOperation,
    ) -> LibraryResult<Vec<ClassificationRule>> {
        let connection = self.library_connection()?;
        validate_classification_operation(connection, &operation)?;
        store::apply_classification_rule_operation(connection, &operation, &now())
    }

    /// 预览一批文件的分类结果；与真实导入共用 [`plan_classification`]。
    pub fn preview_classification(
        &self,
        request: ClassificationPreviewRequest,
    ) -> LibraryResult<ClassificationPreviewResponse> {
        let connection = self.library_connection()?;
        // 只有用户确实指定且仍然存在的目标集合才算显式目标，否则由规则决定。
        let explicit_collection_id = match request.target_collection_id.as_deref() {
            Some(target) => {
                let (resolved, notice) = self.resolve_target_collection(Some(target))?;
                notice.is_none().then_some(resolved)
            }
            None => None,
        };
        let rules = store::list_classification_rules(connection)?;
        let mut items = Vec::with_capacity(request.paths.len());
        for source_path in &request.paths {
            let plan = plan_classification(
                connection,
                &rules,
                source_path,
                explicit_collection_id.as_deref(),
            )?;
            items.push(ClassificationPreviewItem {
                source_path: source_path.clone(),
                file_name: display_file_name(source_path),
                file_type: supported_file_type(Path::new(source_path)).map(str::to_string),
                collection_id: plan.collection_id,
                tag_ids: plan.tag_ids,
                matched_rule_ids: plan.matched_rule_ids,
            });
        }
        Ok(ClassificationPreviewResponse { items })
    }

    // ---- 接收来源（工作单 11/12） ----

    /// 读取当前资料库的接收来源；顺带刷新目录状态与待处理数。
    pub fn list_receive_sources(&self) -> LibraryResult<Vec<ReceiveSource>> {
        let connection = self.library_connection()?;
        let mut sources = store::list_receive_sources(connection)?;
        for source in sources.iter_mut() {
            refresh_receive_source_status(connection, source)?;
        }
        Ok(sources)
    }

    pub fn list_receive_source_candidates(
        &self,
        kind: ReceiveSourceKind,
    ) -> LibraryResult<ReceiveSourceCandidates> {
        let library_root = PathBuf::from(&self.library_summary()?.path);
        Ok(ReceiveSourceCandidates {
            kind,
            candidates: receive_source_candidates(kind, &library_root),
        })
    }

    /// 新增或更新一个接收来源；路径变化时重新要求用户确认清单。
    pub fn upsert_receive_source(
        &mut self,
        source_id: Option<&str>,
        input: ReceiveSourceInput,
    ) -> LibraryResult<Vec<ReceiveSource>> {
        let display_name = input.display_name.trim().to_string();
        if display_name.is_empty() {
            return Err(LibraryError::InvalidLocation(
                "接收来源名称不能为空。".to_string(),
            ));
        }
        if input.path.trim().is_empty() {
            return Err(LibraryError::InvalidLocation(
                "请先选择并确认接收目录。".to_string(),
            ));
        }
        let library_root = PathBuf::from(&self.library_summary()?.path);
        let confirmed = validate_receive_path(&library_root, input.path.trim())?;
        let confirmed_text = confirmed.to_string_lossy().into_owned();
        let normalized_input = ReceiveSourceInput {
            kind: input.kind,
            display_name,
            path: confirmed_text.clone(),
            enabled: input.enabled,
        };

        let connection = self.library_connection()?;
        let previous_path = source_id.and_then(|id| {
            store::list_receive_sources(connection)
                .ok()
                .and_then(|sources| {
                    sources
                        .into_iter()
                        .find(|source| source.id == id)
                        .and_then(|source| source.path)
                })
        });
        let path_changed = previous_path
            .as_deref()
            .is_none_or(|previous| !paths_equal(previous, &confirmed_text));

        store::upsert_receive_source(connection, source_id, &normalized_input, &now())?;
        let sources = store::list_receive_sources(connection)?;
        let target = match source_id {
            Some(id) => sources.iter().find(|source| source.id == id),
            None => sources
                .iter()
                .find(|source| paths_equal(source.path.as_deref().unwrap_or_default(), &confirmed_text)),
        };
        if let Some(source) = target {
            if path_changed {
                // 重新定位后必须重新走一次「首次清单」，未确认前不自动导入既有文件。
                store::clear_receive_source_scan_state(connection, &source.id)?;
            }
        }
        self.list_receive_sources()
    }

    pub fn remove_receive_source(&mut self, source_id: &str) -> LibraryResult<Vec<ReceiveSource>> {
        let connection = self.library_connection()?;
        store::remove_receive_source(connection, source_id)?;
        self.list_receive_sources()
    }

    /// 首次启用或重新定位后列出目录内已有的受支持文件。
    pub fn list_receive_directory_files(
        &self,
        source_id: &str,
    ) -> LibraryResult<ReceiveDirectoryListing> {
        let connection = self.library_connection()?;
        let source = load_receive_source(connection, source_id)?;
        let Some(path) = source.path.clone() else {
            return Err(LibraryError::InvalidLocation(
                "该接收来源尚未确认目录。".to_string(),
            ));
        };
        let root = PathBuf::from(&path);
        if !root.is_dir() {
            return Err(LibraryError::InvalidLocation(format!(
                "接收目录不存在或无法访问：{path}。请重新确认目录。"
            )));
        }
        let mut items = Vec::new();
        for file in collect_receive_files(&root, MAX_RECEIVE_SCAN_DEPTH)
            .into_iter()
            .take(MAX_RECEIVE_LISTING_ITEMS)
        {
            let file_path = file.to_string_lossy().into_owned();
            let metadata = fs::metadata(&file).ok();
            items.push(ReceiveDirectoryListingItem {
                previously_skipped: store::is_source_skipped(connection, source_id, &file_path)?,
                path: file_path,
                file_name: file
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                file_type: supported_file_type(&file).map(str::to_string),
                file_size: metadata
                    .as_ref()
                    .and_then(|metadata| i64::try_from(metadata.len()).ok())
                    .unwrap_or(0),
            });
        }
        Ok(ReceiveDirectoryListing {
            source_id: source.id,
            path,
            items,
        })
    }

    /// 记录用户明确跳过的既有文件；它们不会被后续补扫或实时导入自动导入。
    pub fn skip_receive_directory_files(
        &mut self,
        operation: ReceiveDirectoryOperation,
    ) -> LibraryResult<Vec<ReceiveSource>> {
        let connection = self.library_connection()?;
        load_receive_source(connection, &operation.source_id)?;
        store::record_skipped_sources(
            connection,
            &operation.source_id,
            &operation.paths,
            &now(),
        )?;
        mark_receive_source_scanned(connection, &operation.source_id)?;
        self.list_receive_sources()
    }

    /// 列表内尚未被用户跳过、且仍需导入的文件。
    ///
    /// 已经导入过（含相同内容跳过）的源文件不会再被重复导入，避免周期补扫刷日志；
    /// 来源内容变化的待决项不再自动重复导入。
    ///
    /// `retry_failed` 区分调用方：周期补扫传 `false`，失败项要等冷却时间过后才重试（避免每次补扫刷日志）；
    /// 用户主动点「扫描」传 `true`，立即重试失败项，让「恢复后重试」不必干等冷却。
    pub fn receive_pending_files(
        &self,
        source_id: &str,
        retry_failed: bool,
    ) -> LibraryResult<Vec<String>> {
        let connection = self.library_connection()?;
        let source = load_receive_source(connection, source_id)?;
        if !source.enabled {
            return Ok(Vec::new());
        }
        // 用户还没做完首次选择（或刚重新定位）时不自动导入任何既有文件。
        // 做完选择后，目录内没被勾选的既有文件都已记为「明确跳过」，
        // 因此补扫只会看到首次确认之后新到的文件（清单 500 条上限不再造成静默导入）。
        let Some(_watermark) = store::receive_source_first_scan_watermark(connection, source_id)?
        else {
            return Ok(Vec::new());
        };
        let Some(path) = source.path.clone() else {
            return Ok(Vec::new());
        };
        let root = PathBuf::from(&path);
        if !root.is_dir() {
            return Ok(Vec::new());
        }
        let history = receive_handled_files(connection, source_id)?;
        let mut pending = Vec::new();
        for file in collect_receive_files(&root, MAX_RECEIVE_SCAN_DEPTH) {
            let file_path = file.to_string_lossy().into_owned();
            if store::is_source_skipped(connection, source_id, &file_path)? {
                continue;
            }
            match history.get(&file_path) {
                Some((status, created_at)) => {
                    let retry = match status {
                        // 已导入过的文件只在源文件确实变化时再尝试（内容变化 → 待决项）。
                        ImportItemStatus::Imported | ImportItemStatus::Duplicate => {
                            source_file_changed(connection, &file_path, &file)?
                        }
                        // 失败项：周期补扫等冷却，用户主动重试立即放行。
                        ImportItemStatus::Failed => {
                            retry_failed || created_at.as_str() <= receive_retry_cutoff().as_str()
                        }
                        // 待决项等用户决定，已跳过/忽略的不再自动导入。
                        _ => false,
                    };
                    if !retry {
                        continue;
                    }
                }
                None => {
                    // 既有但没被展示/勾选的文件已经在选择时记为跳过；能走到这里的都是
                    // 首次确认之后新到（或改名出现）的文件，按既有语义导入。
                }
            }
            pending.push(file_path);
        }
        Ok(pending)
    }

    /// 读取最近的接收导入日志，供界面查看与事后改正分类结果。
    pub fn list_receive_import_log(
        &self,
        limit: usize,
    ) -> LibraryResult<Vec<ReceiveImportLogEntry>> {
        let connection = self.library_connection()?;
        store::list_import_log(connection, limit.clamp(1, 500))
    }

    /// 开始一批**用户在首次清单里选择**的接收导入。
    ///
    /// 选择即声明：此时目录内所有受支持的、用户没勾选的文件都会被记为「明确跳过」——
    /// 清单有 500 条上限，不这样做的话第 501 个之后的既有文件既没展示也没记录，
    /// 会被周期补扫静默导入。补扫/监视自己的批量导入走
    /// [`Self::begin_receive_import_batch`]，不会写入跳过记录。
    pub fn begin_receive_selection_batch(
        &mut self,
        source_id: &str,
        selected: Vec<String>,
    ) -> LibraryResult<ImportProgress> {
        let connection = self.library_connection()?;
        let source = load_receive_source(connection, source_id)?;
        if let Some(root) = source.path.clone().map(PathBuf::from).filter(|p| p.is_dir()) {
            let selected_set = selected.iter().cloned().collect::<HashSet<_>>();
            let unselected = collect_receive_files(&root, MAX_RECEIVE_SCAN_DEPTH)
                .into_iter()
                .take(MAX_RECEIVE_SKIP_ENUMERATION)
                .map(|path| path.to_string_lossy().into_owned())
                .filter(|path| !selected_set.contains(path))
                .collect::<Vec<_>>();
            let transaction = connection.unchecked_transaction()?;
            store::record_skipped_sources(connection, source_id, &unselected, &now())?;
            transaction.commit()?;
        }
        self.begin_receive_import_batch(source_id, selected)
    }

    /// 结束一批接收导入：汇总计数、更新来源状态并写回扫描时间。
    pub fn finish_receive_import_batch(
        &mut self,
        batch_id: &str,
    ) -> LibraryResult<ReceiveSourceScanResult> {
        let source_id = {
            let batch = self.active_import_batch(batch_id)?;
            batch
                .receive_source_id
                .clone()
                .ok_or_else(|| LibraryError::ImportFile("该批次不是接收目录导入。".to_string()))?
        };
        let batch = self.finish_import_batch(batch_id)?;
        let connection = self.library_connection()?;
        let result = receive_scan_result(&source_id, batch.items.len(), &batch.items);
        mark_receive_source_scanned(connection, &source_id)?;
        refresh_source_status_by_id(connection, &source_id)?;
        Ok(result)
    }

    /// 写入一条接收导入日志（供界面事后查看与改正）。
    fn record_receive_import_log(
        &self,
        source_id: &str,
        item: &ImportItemResult,
        matched_rule_ids: &[String],
    ) -> LibraryResult<()> {
        let connection = self.library_connection()?;
        let plan = if item.status == ImportItemStatus::Imported {
            item.document_id
                .as_deref()
                .and_then(|document_id| document_classification(connection, document_id).ok())
        } else {
            None
        };
        store::record_import_log(
            connection,
            source_id,
            &item.source_path,
            &item.file_name,
            item.status,
            item.document_id.as_deref(),
            plan.as_ref()
                .map(|plan| plan.collection_id.clone())
                .or_else(|| item.collection_id.clone())
                .as_deref(),
            plan.as_ref()
                .map(|plan| plan.tag_ids.clone())
                .unwrap_or_default()
                .as_slice(),
            matched_rule_ids,
            item.error_message.as_deref(),
            &now(),
        )
    }

    fn library_summary(&self) -> LibraryResult<LibrarySummary> {
        self.current_library()
            .cloned()
            .ok_or(LibraryError::NoCurrentLibrary)
    }
}

/// 规则命中判定：同一条规则内填写的条件必须全部满足，空条件表示不限制。
fn rule_matches(
    rule: &ClassificationRule,
    source_path: &str,
    file_name: &str,
    file_type: Option<&str>,
) -> bool {
    let pattern = rule.file_name_pattern.trim();
    if !pattern.is_empty()
        && !file_name
            .to_lowercase()
            .contains(&pattern.to_lowercase())
    {
        return false;
    }
    if let Some(expected_type) = rule
        .file_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if file_type != Some(expected_type) {
            return false;
        }
    }
    if let Some(directory) = rule
        .source_directory
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if !path_is_within(source_path, directory) {
            return false;
        }
    }
    true
}

/// 判断 `path` 是否位于 `directory` 之下（忽略大小写与分隔符差异）。
fn path_is_within(path: &str, directory: &str) -> bool {
    let path_key = path.replace('\\', "/").to_lowercase();
    let mut directory_key = directory.replace('\\', "/").to_lowercase();
    while directory_key.ends_with('/') {
        directory_key.pop();
    }
    if directory_key.is_empty() {
        return true;
    }
    path_key == directory_key || path_key.starts_with(&format!("{directory_key}/"))
}

fn inbox_collection_id(connection: &Connection) -> LibraryResult<String> {
    let id: Option<String> = connection
        .query_row(
            "SELECT id FROM collections WHERE is_inbox = 1 ORDER BY id ASC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(id.unwrap_or_else(|| "inbox".to_string()))
}

/// 分类匹配的唯一实现：预览与实际导入都走这里。
fn plan_classification(
    connection: &Connection,
    rules: &[ClassificationRule],
    source_path: &str,
    explicit_collection_id: Option<&str>,
) -> LibraryResult<ClassificationPlan> {
    let file_name = display_file_name(source_path);
    let file_type = supported_file_type(Path::new(source_path));
    let matched = rules
        .iter()
        .filter(|rule| rule.enabled && rule_matches(rule, source_path, &file_name, file_type))
        .collect::<Vec<_>>();

    let collection_id = match explicit_collection_id {
        Some(explicit) => explicit.to_string(),
        None => match matched.iter().find_map(|rule| rule.collection_id.clone()) {
            Some(collection_id) => collection_id,
            None => inbox_collection_id(connection)?,
        },
    };

    let mut tag_ids = Vec::new();
    for rule in &matched {
        for tag_id in &rule.tag_ids {
            if !tag_ids.iter().any(|existing| existing == tag_id) {
                tag_ids.push(tag_id.clone());
            }
        }
    }

    Ok(ClassificationPlan {
        collection_id,
        tag_ids,
        matched_rule_ids: matched.iter().map(|rule| rule.id.clone()).collect(),
    })
}

/// 校验规则编辑里引用的集合与标签确实存在，避免只靠外键报错。
fn validate_classification_operation(
    connection: &Connection,
    operation: &ClassificationRuleOperation,
) -> LibraryResult<()> {
    let inputs: Vec<&ClassificationRuleInput> = match operation {
        ClassificationRuleOperation::Create { rule } => vec![rule],
        ClassificationRuleOperation::Update { rule } => vec![&rule.input],
        _ => Vec::new(),
    };
    for input in inputs {
        if input.name.trim().is_empty() {
            return Err(LibraryError::InvalidCollection(
                "规则名称不能为空。".to_string(),
            ));
        }
        if input
            .file_name_pattern
            .trim()
            .is_empty()
            && input.file_type.is_none()
            && input.source_directory.is_none()
            && input.collection_id.is_none()
            && input.tag_ids.is_empty()
        {
            return Err(LibraryError::InvalidCollection(
                "请至少填写一个匹配条件，或指定目标集合与标签。".to_string(),
            ));
        }
        if let Some(collection_id) = input.collection_id.as_deref() {
            ensure_collection_exists(connection, collection_id)?;
        }
        for tag_id in &input.tag_ids {
            ensure_tag_exists(connection, tag_id)?;
        }
    }
    Ok(())
}

fn load_receive_source(connection: &Connection, source_id: &str) -> LibraryResult<ReceiveSource> {
    store::list_receive_sources(connection)?
        .into_iter()
        .find(|source| source.id == source_id)
        .ok_or_else(|| LibraryError::InvalidLocation("接收来源不存在。".to_string()))
}

/// 目录状态：不存在 → missing，无法读取 → unreadable，否则 ready。
fn receive_status_for(path: &Path) -> (ReceiveSourceStatus, Option<String>) {
    match fs::read_dir(path) {
        Ok(_) => (ReceiveSourceStatus::Ready, None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => (
            ReceiveSourceStatus::Missing,
            Some("接收目录不存在或已被移动，请重新确认目录。".to_string()),
        ),
        Err(error) => (
            ReceiveSourceStatus::Unreadable,
            Some(format!("无法读取接收目录：{error}。")),
        ),
    }
}

fn refresh_receive_source_status(
    connection: &Connection,
    source: &mut ReceiveSource,
) -> LibraryResult<()> {
    match source.path.clone() {
        Some(path) => {
            let (status, message) = receive_status_for(Path::new(&path));
            source.status = status;
            source.status_message = message;
        }
        None => {
            source.status = ReceiveSourceStatus::Unconfigured;
            source.status_message = Some("尚未确认接收目录。".to_string());
        }
    }
    connection.execute(
        "UPDATE receive_sources SET status = ?2, status_message = ?3 WHERE id = ?1",
        params![
            &source.id,
            receive_status_to_str(source.status),
            source.status_message.as_deref()
        ],
    )?;
    // 待处理项是「同一来源内容变化」等待用户决定的项，由导入日志里的 sourceChanged 表示。
    source.pending_count = connection.query_row(
        "SELECT COUNT(*) FROM receive_import_log WHERE source_id = ?1 AND status = 'sourceChanged'",
        params![&source.id],
        |row| row.get(0),
    )?;
    Ok(())
}

fn refresh_source_status_by_id(connection: &Connection, source_id: &str) -> LibraryResult<()> {
    let mut source = load_receive_source(connection, source_id)?;
    refresh_receive_source_status(connection, &mut source)
}

fn receive_status_to_str(status: ReceiveSourceStatus) -> &'static str {
    match status {
        ReceiveSourceStatus::Unconfigured => "unconfigured",
        ReceiveSourceStatus::Ready => "ready",
        ReceiveSourceStatus::Missing => "missing",
        ReceiveSourceStatus::Unreadable => "unreadable",
    }
}

fn mark_receive_source_scanned(connection: &Connection, source_id: &str) -> LibraryResult<()> {
    // 水位与 `modified_at` 同单位（Unix 纳秒），否则「文件比水位新」的判断会失真。
    store::confirm_receive_source_first_scan(
        connection,
        source_id,
        &now(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default(),
    )
}

/// 校验确认的接收目录：必须是可访问的文件夹，且与资料库互不包含。
fn validate_receive_path(library_root: &Path, path: &str) -> LibraryResult<PathBuf> {
    let candidate = PathBuf::from(path);
    let metadata = fs::metadata(&candidate).map_err(|error| {
        LibraryError::InvalidLocation(format!("无法访问接收目录：{error}"))
    })?;
    if !metadata.is_dir() {
        return Err(LibraryError::InvalidLocation(
            "接收目录必须是文件夹。".to_string(),
        ));
    }
    let candidate = dunce_canonicalize(&candidate)?;
    let library_root = normalize_path(library_root)?;
    let candidate_key = comparable_path(&candidate);
    let library_key = comparable_path(&library_root);
    if candidate_key == library_key
        || candidate_key.starts_with(&format!("{library_key}\\"))
        || candidate_key.starts_with(&format!("{library_key}/"))
    {
        return Err(LibraryError::InvalidLocation(
            "不能把资料库自身或其内部目录设为接收目录。".to_string(),
        ));
    }
    if library_key.starts_with(&format!("{candidate_key}\\"))
        || library_key.starts_with(&format!("{candidate_key}/"))
    {
        return Err(LibraryError::InvalidLocation(
            "接收目录不能包含资料库本身，否则会把资料库副本当成新文件。".to_string(),
        ));
    }
    Ok(candidate)
}

/// 本机候选接收目录；候选只用于帮助定位，未经用户确认不会开始扫描。
fn receive_source_candidates(
    kind: ReceiveSourceKind,
    library_root: &Path,
) -> Vec<ReceiveSourceCandidate> {
    let Some(home) = user_home_directory() else {
        return Vec::new();
    };
    let documents = home.join("Documents");
    let mut candidates: Vec<(PathBuf, &str)> = Vec::new();
    match kind {
        ReceiveSourceKind::Wechat => {
            candidates.push((documents.join("WeChat Files"), "微信默认文档目录"));
            candidates.push((documents.join("xwechat_files"), "微信 4.x 默认文档目录"));
        }
        ReceiveSourceKind::Qq => {
            let tencent = documents.join("Tencent Files");
            candidates.push((tencent.clone(), "QQ 默认文档目录"));
            let mut accounts = fs::read_dir(&tencent)
                .map(|entries| {
                    entries
                        .filter_map(Result::ok)
                        .map(|entry| entry.path())
                        .filter(|path| path.is_dir())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            accounts.sort();
            for account in accounts {
                candidates.push((account.join("FileRecv"), "QQ 用户接收文件目录"));
            }
        }
        ReceiveSourceKind::Other => {}
    }

    let library_key = comparable_path(library_root);
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for (path, evidence) in candidates {
        if !path.is_dir() {
            continue;
        }
        let key = comparable_path(&path);
        if key == library_key || !seen.insert(key) {
            continue;
        }
        result.push(ReceiveSourceCandidate {
            path: path.to_string_lossy().into_owned(),
            evidence: evidence.to_string(),
        });
    }
    result
}

fn user_home_directory() -> Option<PathBuf> {
    for key in ["USERPROFILE", "HOME"] {
        if let Ok(value) = std::env::var(key) {
            let path = PathBuf::from(value.trim());
            if !path.as_os_str().is_empty() && path.is_dir() {
                return Some(path);
            }
        }
    }
    None
}

/// 接收目录里的可导入文件：普通文件、受支持类型，且不是临时文件或聊天数据库。
fn is_receive_candidate_file(path: &Path) -> bool {
    let Some(name) = path.file_name().map(|name| name.to_string_lossy()) else {
        return false;
    };
    let lowercase = name.to_ascii_lowercase();
    if lowercase.starts_with('.') || lowercase.starts_with('~') {
        return false;
    }
    if let Some(extension) = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
    {
        if RECEIVE_TEMP_EXTENSIONS.contains(&extension.as_str()) {
            return false;
        }
    }
    let stem = lowercase
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(lowercase.as_str());
    !["-temp", "_temp", ".temp", " temp", "-partial", ".part"]
        .iter()
        .any(|suffix| stem.ends_with(suffix))
}

fn is_ignored_receive_directory(path: &Path) -> bool {
    let Some(name) = path.file_name().map(|name| name.to_string_lossy()) else {
        return false;
    };
    let lowercase = name.to_ascii_lowercase();
    if lowercase.starts_with('.') {
        return true;
    }
    matches!(
        lowercase.as_str(),
        "$recycle.bin" | "system volume information" | "cache" | "temp" | "tmp"
    )
}

/// 列出接收目录内所有可导入文件（含子目录，深度受限），按路径排序。
fn collect_receive_files(root: &Path, depth: usize) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_receive_files_into(root, depth, &mut files);
    files.sort();
    files
}

fn collect_receive_files_into(directory: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    if depth == 0 || files.len() >= MAX_RECEIVE_LISTING_ITEMS * 8 {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            continue;
        }
        if metadata.is_dir() {
            if !is_ignored_receive_directory(&path) {
                collect_receive_files_into(&path, depth - 1, files);
            }
            continue;
        }
        if !metadata.is_file() || !is_receive_candidate_file(&path) {
            continue;
        }
        if supported_file_type(&path).is_none() {
            continue;
        }
        files.push(path);
    }
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

/// 汇总一批接收导入的结果。
fn receive_scan_result(
    source_id: &str,
    scanned_count: usize,
    items: &[ImportItemResult],
) -> ReceiveSourceScanResult {
    let mut imported_count = 0;
    let mut skipped_count = 0;
    let mut pending_count = 0;
    let mut failed_count = 0;
    for item in items {
        match item.status {
            ImportItemStatus::Imported => imported_count += 1,
            ImportItemStatus::SourceChanged => pending_count += 1,
            ImportItemStatus::Failed => failed_count += 1,
            ImportItemStatus::Duplicate | ImportItemStatus::Ignored | ImportItemStatus::Skipped => {
                skipped_count += 1
            }
        }
    }
    ReceiveSourceScanResult {
        source_id: source_id.to_string(),
        scanned_count: scanned_count as i64,
        imported_count,
        skipped_count,
        pending_count,
        failed_count,
    }
}

/// 读取一个来源已处理过的源文件及其最近一次导入状态。
fn receive_handled_files(
    connection: &Connection,
    source_id: &str,
) -> LibraryResult<HashMap<String, (ImportItemStatus, String)>> {
    let mut statement = connection.prepare(
        // 同一毫秒内的多条日志用 rowid（插入顺序）决定先后，避免随机 UUID 改变“最近一次”的判定。
        "SELECT source_path, status, created_at
           FROM receive_import_log
          WHERE source_id = ?1
          ORDER BY created_at ASC, rowid ASC",
    )?;
    let rows = statement.query_map(params![source_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut handled = HashMap::new();
    for row in rows {
        let (path, status, created_at) = row?;
        handled.insert(path, (import_status_from_name(&status), created_at));
    }
    Ok(handled)
}

fn import_status_from_name(value: &str) -> ImportItemStatus {
    match value {
        "imported" => ImportItemStatus::Imported,
        "duplicate" => ImportItemStatus::Duplicate,
        "sourceChanged" => ImportItemStatus::SourceChanged,
        "failed" => ImportItemStatus::Failed,
        "ignored" => ImportItemStatus::Ignored,
        _ => ImportItemStatus::Skipped,
    }
}

/// 失败项自动重试的冷却时间戳（与 `now()` 同一格式，便于字符串比较）。
fn receive_retry_cutoff() -> String {
    (Utc::now() - chrono::TimeDelta::seconds(RECEIVE_RETRY_COOLDOWN_SECONDS))
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// 源文件的内容是否已经与资料库记录不同（内容变化 → 保留待决项）。
///
/// 先用大小/修改时间快速排除，只有指纹变化时才计算哈希，避免每次补扫都重算全目录。
fn source_file_changed(
    connection: &Connection,
    source_path: &str,
    path: &Path,
) -> LibraryResult<bool> {
    let recorded: Option<(i64, Option<i64>, Option<String>)> = connection
        .query_row(
            "SELECT d.file_size, d.file_modified_at, d.content_hash
               FROM sources s
               JOIN documents d ON d.id = s.document_id
              WHERE s.source_identifier = ?1 AND d.deleted_at IS NULL
              ORDER BY s.last_imported_at DESC, s.rowid DESC
              LIMIT 1",
            params![source_path.to_ascii_lowercase()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((file_size, file_modified_at, content_hash)) = recorded else {
        return Ok(true);
    };
    let metadata = fs::metadata(path)?;
    let current_size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);
    if current_size == file_size && modified_at(&metadata) == file_modified_at {
        return Ok(false);
    }
    let Some(expected_hash) = content_hash.filter(|hash| !hash.is_empty()) else {
        return Ok(true);
    };
    match sha256_file(path) {
        Ok(hash) => Ok(hash != expected_hash),
        Err(_) => Ok(true),
    }
}

/// 读取一个文档当前生效的集合与标签，供接收导入日志记录实际结果。
fn document_classification(
    connection: &Connection,
    document_id: &str,
) -> LibraryResult<ClassificationPlan> {
    let collection_id: Option<String> = connection
        .query_row(
            "SELECT collection_id FROM documents WHERE id = ?1",
            params![document_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let mut statement = connection.prepare(
        "SELECT tag_id FROM document_tags WHERE document_id = ?1 ORDER BY tag_id ASC",
    )?;
    let rows = statement.query_map(params![document_id], |row| row.get::<_, String>(0))?;
    let mut tag_ids = Vec::new();
    for row in rows {
        tag_ids.push(row?);
    }
    Ok(ClassificationPlan {
        collection_id: collection_id.unwrap_or_else(|| "inbox".to_string()),
        tag_ids,
        matched_rule_ids: Vec::new(),
    })
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

fn scan_import_paths(paths: &[String], source: ImportSource) -> Vec<ScanEntry> {
    let mut entries = Vec::new();
    for path in paths {
        scan_explicit_path(path, source, &mut entries);
    }
    entries
}

fn scan_explicit_path(input: &str, source: ImportSource, entries: &mut Vec<ScanEntry>) {
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
        } else if source == ImportSource::CollectionDrop {
            entries.push(ScanEntry::Ignored {
                source_path: display_path,
            });
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

fn replacement_backup_path(directory: &Path) -> PathBuf {
    directory.join(format!(".{}{REPLACEMENT_BACKUP_SUFFIX}", Uuid::new_v4()))
}

fn import_temporary_path(directory: &Path) -> PathBuf {
    directory.join(format!(".{}{IMPORT_TEMPORARY_SUFFIX}", Uuid::new_v4()))
}

fn is_replacement_backup_name(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    name.starts_with('.') && name.ends_with(REPLACEMENT_BACKUP_SUFFIX)
}

fn is_import_temporary_name(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    name.starts_with('.') && name.ends_with(IMPORT_TEMPORARY_SUFFIX)
}

fn is_replacement_artifact_name(name: &std::ffi::OsStr) -> bool {
    is_replacement_backup_name(name) || is_import_temporary_name(name)
}

/// 在读入整份压缩文档之前校验输入大小：超限时直接失败，不为超限文件分配整块内存。
fn read_archive_within_limits(path: &Path, limits: ArchiveLimits) -> LibraryResult<Vec<u8>> {
    let budget = ExpansionBudget::new(limits);
    let metadata = fs::metadata(path)?;
    budget.check_input_size(metadata.len())?;
    let bytes = fs::read(path)?;
    budget.check_input_size(bytes.len() as u64)?;
    Ok(bytes)
}

/// 读取表格文档字节，使用表格分页预览的预算（比索引更小）。
fn read_table_bytes_within_limits(path: &Path) -> LibraryResult<Vec<u8>> {
    read_archive_within_limits(path, ArchiveLimits::for_table_preview())
}

/// 表格文档的工作表清单；CSV 返回一张合成工作表。
fn table_sheets_for(bytes: &[u8], format_id: DocumentFormatId) -> LibraryResult<Vec<TableSheet>> {
    let range = match format_id {
        DocumentFormatId::Csv => table::read_csv_table(bytes, 0, 1, 1, ArchiveLimits::for_table_preview())?,
        DocumentFormatId::Xlsx => {
            table::read_xlsx_table(bytes, 0, 0, 1, 1, ArchiveLimits::for_table_preview())?
        }
        other => {
            return Err(LibraryError::Preview(format!(
                "{} 不是表格文档。",
                other.as_str()
            )))
        }
    };
    Ok(range.sheets.into_iter().map(TableSheet::from).collect())
}

/// 读取表格的一页范围，并转换成预览载荷。
fn read_table_preview(
    path: &Path,
    format_id: DocumentFormatId,
    request: &TablePreviewRequest,
) -> LibraryResult<DocumentPreview> {
    let bytes = read_table_bytes_within_limits(path)?;
    let limits = ArchiveLimits::for_table_preview();
    // 0 表示「使用默认值」，避免界面必须知道服务端上限。
    let row_count = match request.row_count {
        0 => MAX_TABLE_PREVIEW_ROWS,
        requested => requested,
    };
    let column_count = match request.column_count {
        0 => DEFAULT_TABLE_PREVIEW_COLUMNS,
        requested => requested,
    };

    let range = match format_id {
        DocumentFormatId::Csv => table::read_csv_table(
            &bytes,
            request.start_row,
            row_count,
            column_count,
            limits,
        )?,
        DocumentFormatId::Xlsx => table::read_xlsx_table(
            &bytes,
            request.sheet_index,
            request.start_row,
            row_count,
            column_count,
            limits,
        )?,
        other => {
            return Err(LibraryError::Preview(format!(
                "{} 不是表格文档。",
                other.as_str()
            )))
        }
    };

    Ok(DocumentPreview::Table {
        sheets: range.sheets.into_iter().map(TableSheet::from).collect(),
        sheet_index: range.sheet_index,
        start_row: range.start_row,
        cells: range.cells,
        row_numbers: range.row_numbers,
        column_count: range.column_count,
        has_more_rows: range.has_more_rows,
        degraded_features: range.degraded_features,
        notice: range.notice,
    })
}

impl From<table::TableSheetInfo> for TableSheet {
    fn from(info: table::TableSheetInfo) -> Self {
        Self {
            index: info.index,
            name: info.name,
            row_count: info.row_count,
            column_count: info.column_count,
        }
    }
}

/// 预览返回的纯文本按上限截断；截断时在末尾给出可理解的降级说明。
fn bounded_preview_text(text: String) -> String {
    let bounded = bound_extracted_text(text);
    if !bounded.truncated {
        return bounded.text;
    }
    format!(
        "{}\n\n……（正文超过 {MAX_EXTRACTED_TEXT_CHARS} 个字符的预览上限，已截断）",
        bounded.text
    )
}

/// 打开资料库时对账中断遗留的文件操作。数据库记录是权威：墓碑和替换备份都要么让
/// 记录的副本重新可读，要么在记录已经指向其他内容时被清理。返回无法自动恢复的文档，
/// 由调用方在启动扫描之后写入失败状态，避免结论被扫描覆盖。
///
/// **逐文档隔离**：单个文档目录的 IO 失败（残留被占用、只读、权限异常）只让那一份文档
/// 进入恢复失败状态，其余文档与整个资料库照常打开——用户不该因为一个删不掉的残留打不开资料库。
/// 只有「文档目录本身读不了」与数据库/JSON 错误才整体失败。
fn reconcile_interrupted_copy_operations(
    library_root: &Path,
    connection: &Connection,
) -> LibraryResult<Vec<RecoveryFailure>> {
    let documents_directory = library_root.join(DOCUMENTS_DIR);
    let document_directories = match fs::read_dir(&documents_directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        // 文档区整个读不了说明资料库结构或权限坏了：这里必须整体失败，不能假装没事。
        Err(error) => return Err(LibraryError::Io(error)),
    };
    let known_copies = library_copy_paths(library_root, connection)?;
    let mut failures = Vec::new();

    for document_directory in document_directories {
        // 单个目录项读不出来（元数据损坏、目录被占用）只跳过它。
        let Ok(document_directory) = document_directory else {
            continue;
        };
        if !document_directory
            .file_type()
            .map(|file_type| file_type.is_dir())
            .unwrap_or(false)
        {
            continue;
        }
        let document_id = document_directory
            .file_name()
            .to_string_lossy()
            .into_owned();
        match reconcile_document_directory(
            library_root,
            connection,
            &known_copies,
            &document_directory.path(),
            &document_id,
        ) {
            Ok(Some(failure)) => failures.push(failure),
            Ok(None) => {}
            // 数据库/JSON 错误是全局性的：继续整体失败。
            Err(error @ (LibraryError::Database(_) | LibraryError::Json(_))) => return Err(error),
            // 其它错误（IO、记录路径非法）按文档隔离，并给出可诊断原因。
            Err(error) => failures.push(RecoveryFailure {
                document_id,
                message: format!(
                    "上次中断的文件操作无法自动完成（{error}）。请关闭占用该文件的程序后重新打开资料库，或从源文件重新导入。"
                ),
            }),
        }
    }
    Ok(failures)
}

/// 对账单个文档目录；只有数据库/JSON 错误返回 `Err`，文件级问题转成 `RecoveryFailure`。
fn reconcile_document_directory(
    library_root: &Path,
    connection: &Connection,
    known_copies: &HashSet<PathBuf>,
    directory_path: &Path,
    document_id: &str,
) -> LibraryResult<Option<RecoveryFailure>> {
    let recorded = connection
        .query_row(
            "SELECT library_path, content_hash, deleted_at FROM documents WHERE id = ?1",
            params![document_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?;
    let entries = fs::read_dir(directory_path)?.collect::<Result<Vec<_>, _>>()?;

    let mut tombstones = entries
        .iter()
        .filter(|entry| is_delete_tombstone_name(&entry.file_name(), DELETE_TOMBSTONE_PREFIX))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    tombstones.sort();
    let had_tombstones = !tombstones.is_empty();

    for tombstone_path in tombstones {
        match recorded.as_ref().map(|(library_path, _, _)| library_path) {
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
        match fs::remove_dir(directory_path) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(error) => return Err(LibraryError::Io(error)),
        }
    }

    let mut backups = Vec::new();
    let mut temporaries = Vec::new();
    for entry in entries {
        let name = entry.file_name();
        if !is_replacement_artifact_name(&name) {
            continue;
        }
        let path = entry.path();
        if known_copies.contains(&path) {
            continue;
        }
        if is_replacement_backup_name(&name) {
            backups.push(path);
        } else {
            temporaries.push(path);
        }
    }
    if backups.is_empty() && temporaries.is_empty() {
        return Ok(None);
    }
    backups.sort();
    temporaries.sort();
    // 备份一定是文档的原内容，临时副本只有在内容核对通过时才会被采用。
    let leftovers = backups.into_iter().chain(temporaries).collect::<Vec<_>>();

    match recorded.as_ref() {
        Some((library_path, content_hash, None)) => recover_interrupted_replacement(
            library_root,
            document_id,
            library_path,
            content_hash.as_deref(),
            &leftovers,
        ),
        _ => {
            remove_leftover_files(&leftovers);
            Ok(None)
        }
    }
}

/// 无法自动恢复的替换：打开资料库时保留文件现场，并把原因写进文档记录。
struct RecoveryFailure {
    document_id: String,
    message: String,
}

/// 让记录的副本重新可读，或在不一致时报告可诊断的失败。返回后目录里不再有中断残留。
///
/// 三种结果的取舍：
/// - 记录路径的副本已与 `content_hash` 一致 → 只是残留删不掉也照样可用（清理是 best-effort）；
/// - 副本缺失/不一致、候选采用失败 → 返回 `Err`，由调用方转成该文档的 `RecoveryFailure`
///   并写进可见状态，**不吞**；
/// - 核对不出任何记录版本 → 返回 `RecoveryFailure`（保持原有语义）。
fn recover_interrupted_replacement(
    library_root: &Path,
    document_id: &str,
    library_path: &str,
    content_hash: Option<&str>,
    leftovers: &[PathBuf],
) -> LibraryResult<Option<RecoveryFailure>> {
    let destination = document_path(library_root, library_path)?;
    if copy_matches_recorded_content(&destination, content_hash) {
        // 清理失败（占用/只读）不影响该文档可用：留下现场，下次打开再试。
        remove_leftover_files(leftovers);
        return Ok(None);
    }

    for candidate in leftovers {
        if !copy_matches_recorded_content(candidate, content_hash) {
            continue;
        }
        let remaining = leftovers
            .iter()
            .filter(|path| *path != candidate)
            .cloned()
            .collect::<Vec<_>>();
        // 让记录的副本就位属于「恢复」而不是「清理」：失败必须让用户看见。
        remove_library_copy(&destination)?;
        fs::rename(candidate, &destination)?;
        remove_leftover_files(&remaining);
        return Ok(None);
    }

    Ok(Some(RecoveryFailure {
        document_id: document_id.to_string(),
        message: format!(
            "上次替换副本未完成，且无法自动恢复：资料库副本与记录不一致（{}）。请从源文件重新导入。",
            destination.display()
        ),
    }))
}

fn copy_matches_recorded_content(path: &Path, content_hash: Option<&str>) -> bool {
    if !path.is_file() {
        return false;
    }
    let Some(expected) = content_hash else {
        // 没有记录哈希时无法核对内容，副本存在即视为一致。
        return true;
    };
    sha256_file(path).is_ok_and(|hash| hash == expected)
}

fn library_copy_paths(
    library_root: &Path,
    connection: &Connection,
) -> LibraryResult<HashSet<PathBuf>> {
    let mut statement = connection.prepare("SELECT library_path FROM documents")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut paths = HashSet::new();
    for row in rows {
        if let Ok(path) = document_path(library_root, &row?) {
            paths.insert(path);
        }
    }
    Ok(paths)
}

/// 尽力删除残留，返回**删不掉**的路径（被其它进程占用、只读属性、同步盘持锁等）。
///
/// 清理失败不升级为错误：走到这里时记录的副本已经与 `content_hash` 一致，残留只是冗余备份或
/// 半成品，删不掉不影响文档可用性。留下现场并在下次打开资料库时重试，好过让整个资料库打不开。
fn remove_leftover_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut remaining = Vec::new();
    for path in paths {
        if remove_library_copy(path).is_err() {
            remaining.push(path.clone());
        }
    }
    remaining
}

fn report_recovery_failure(
    connection: &Connection,
    document_id: &str,
    message: &str,
) -> LibraryResult<()> {
    let transaction = connection.unchecked_transaction()?;
    transaction.execute(
        "
        UPDATE documents
        SET processing_status = 'failed',
            index_status = 'failed',
            error_stage = 'recovery',
            error_message = ?1,
            updated_at = ?2
        WHERE id = ?3 AND deleted_at IS NULL
        ",
        params![message, now(), document_id],
    )?;
    transaction.execute(
        "DELETE FROM document_search WHERE document_id = ?1",
        params![document_id],
    )?;
    transaction.commit()?;
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

fn import_batch_not_found(batch_id: &str) -> LibraryError {
    LibraryError::ImportFile(format!(
        "该导入批次已结束或失效：{batch_id}。请重新发起导入。"
    ))
}

/// 把逐项结果汇总成整批结果，计数口径与 `start_import_to_collection` 一致。
fn import_batch_from_items(
    batch_id: String,
    items: Vec<ImportItemResult>,
    target_collection_id: Option<String>,
) -> ImportBatch {
    let imported_count = count_items(&items, ImportItemStatus::Imported);
    let duplicate_count = count_items(&items, ImportItemStatus::Duplicate);
    let source_changed_count = count_items(&items, ImportItemStatus::SourceChanged);
    let failed_count = count_items(&items, ImportItemStatus::Failed);
    let ignored_count = count_items(&items, ImportItemStatus::Ignored);
    ImportBatch {
        batch_id,
        items,
        imported_count,
        duplicate_count,
        source_changed_count,
        failed_count,
        ignored_count,
        target_collection_id,
    }
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
    super::store::initialize_store_schema(connection)?;
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
    // 压缩文档与表格文档在导入时就按输入上限拦下，避免把超限文件整份复制进资料库
    // （也避免 validate_table_file 先把整份超大文件读进内存）。CSV 与 XLSX 用同一个上限，
    // 与索引、表格分页预览路径的 `ArchiveLimits` 完全一致。
    if matches!(
        capability.validation,
        ValidationStrategy::DocxPackage
            | ValidationStrategy::PptxPackage
            | ValidationStrategy::XlsxPackage
            | ValidationStrategy::CsvText
    ) {
        let input_bytes = fs::metadata(path)
            .map_err(|error| format!("无法读取文件内容：{error}"))?
            .len();
        ExpansionBudget::new(ArchiveLimits::for_index())
            .check_input_size(input_bytes)
            .map_err(|error| error.to_string())?;
    }
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
        // 表格文档做真实结构校验：CSV 检查可安全解码，XLSX 检查真实工作簿结构，
        // 不只依赖扩展名或 ZIP 文件头。
        ValidationStrategy::CsvText | ValidationStrategy::XlsxPackage => {
            validate_table_file(path, capability.id.as_str())
        }
    }
}

/// 按格式校验表格文档；返回面向用户的失败原因。
fn validate_table_file(path: &Path, format_id: &str) -> Result<(), String> {
    let bytes = fs::read(path).map_err(|error| format!("无法读取文件内容：{error}"))?;
    table::validate_table_document(&bytes, format_id)
}

fn validate_pptx_package(path: &Path) -> Result<(), String> {
    let archive = read_archive_within_limits(path, ArchiveLimits::for_index())
        .map_err(|error| error.to_string())?;
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
    // 文本按字节流读取并设上限：UTF-8 每个字符最多 4 字节，再多读一点用于识别截断。
    // 这样超大文本不会在被截断之前先整份读进内存（字符级上限见 bound_extracted_text）。
    let file = File::open(path)?;
    let mut buffer = Vec::new();
    file.take(MAX_TEXT_READ_BYTES).read_to_end(&mut buffer)?;
    Ok(String::from_utf8_lossy(&buffer).into_owned())
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
    let archive = read_archive_within_limits(path, ArchiveLimits::for_index())?;
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
    // 降级检查属于预览工作：所有部件共用一份预览预算，累计超限后剩余部件按读取失败跳过。
    let mut budget = ExpansionBudget::new(ArchiveLimits::for_preview());
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
        let Ok(xml) = read_zip_entry_for(archive, &entry_name, "DOCX", &mut budget) else {
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
    let archive = read_archive_within_limits(path, ArchiveLimits::for_index())?;
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

    // 所有幻灯片与图表共用一份索引预算，累计解压量超限后剩余页按降级处理。
    let mut budget = ExpansionBudget::new(ArchiveLimits::for_index());
    let mut slide_text = Vec::new();
    for (index, entry_name) in slide_names.iter().enumerate() {
        match read_zip_entry_for(archive, entry_name, "PPTX", &mut budget)
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
        match read_zip_entry_for(archive, &entry_name, "PPTX", &mut budget)
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
    let Ok(entry_names) = zip_entry_names_for(archive, "PPTX", ArchiveLimits::for_preview()) else {
        return Vec::new();
    };
    // 降级检查属于预览工作：所有部件共用一份预览预算，超限后剩余部件按读取失败跳过。
    let mut budget = ExpansionBudget::new(ArchiveLimits::for_preview());
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
        let Ok(xml) = read_zip_entry_for(archive, &entry_name, "PPTX", &mut budget) else {
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
    zip_entry_names_for(archive, "DOCX", ArchiveLimits::for_preview())
}

fn zip_entry_names_for(
    archive: &[u8],
    format: &str,
    limits: ArchiveLimits,
) -> LibraryResult<Vec<String>> {
    ExpansionBudget::new(limits).check_input_size(archive.len() as u64)?;
    let eocd = find_zip_eocd(archive).ok_or_else(|| {
        LibraryError::Preview(format!("{format} 文件结构无效：找不到 ZIP 中央目录。"))
    })?;
    let entry_count = read_u16(archive, eocd + 10)? as usize;
    let mut cursor = read_u32(archive, eocd + 16)? as usize;
    // 每个中央目录项至少 46 字节，按文件长度收敛预分配。
    let mut names = Vec::with_capacity(entry_count.min(archive.len() / 46 + 1));

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
    let mut budget = ExpansionBudget::new(ArchiveLimits::for_index());
    read_zip_entry_for(archive, target_name, "DOCX", &mut budget)
}

/// 在既有预算内读取单个条目：先按声明大小与压缩比筛选，再限流解压并计入累计量。
fn read_zip_entry_for(
    archive: &[u8],
    target_name: &str,
    format: &str,
    budget: &mut ExpansionBudget,
) -> LibraryResult<Vec<u8>> {
    budget.check_input_size(archive.len() as u64)?;
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
            budget.check_entry_size(target_name, uncompressed_size as u64)?;
            budget.check_compression_ratio(
                target_name,
                compressed_size as u64,
                uncompressed_size as u64,
            )?;

            let data_start = zip_local_data_start(archive, local_header_offset, format)?;
            let data_end = data_start.checked_add(compressed_size).ok_or_else(|| {
                LibraryError::Preview(format!("{format} 文件结构无效：正文长度溢出。"))
            })?;
            let compressed = archive.get(data_start..data_end).ok_or_else(|| {
                LibraryError::Preview(format!("{format} 文件结构无效：正文超出文件范围。"))
            })?;
            return match compression {
                0 => {
                    let contents = compressed.to_vec();
                    budget.charge(target_name, contents.len())?;
                    Ok(contents)
                }
                8 => read_stream_limited(
                    DeflateDecoder::new(Cursor::new(compressed)),
                    target_name,
                    uncompressed_size as u64,
                    budget,
                ),
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
    let text = match capability.text_extraction {
        TextExtractionStrategy::PdfText => extract_pdf_text(path),
        TextExtractionStrategy::DocxText => extract_docx_text(path),
        TextExtractionStrategy::PlainText => read_utf8_text(path),
        TextExtractionStrategy::None => Ok(String::new()),
        TextExtractionStrategy::PptxText => extract_pptx_text(path),
        // 表格文档按单元格可见文字建立索引；公式缓存值缺失的单元格不参与索引。
        TextExtractionStrategy::TableText => {
            let bytes = read_table_bytes_within_limits(path)?;
            table::extract_table_text(&bytes, file_type)
        }
    }?;
    // 索引正文有明确上限：只把前 MAX_EXTRACTED_TEXT_CHARS 个字符写进 FTS，
    // 避免单份大文档把索引撑到无界（上限同时用于预览截断提示）。
    Ok(bound_extracted_text(text).text)
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

fn creation_error_after_cleanup(
    path: &Path,
    root_created: bool,
    cause: LibraryError,
) -> LibraryError {
    match cleanup_failed_creation(path, root_created) {
        Ok(()) => cause,
        Err(cleanup_error) => LibraryError::InvalidLibrary(format!(
            "创建资料库失败：{cause}。清理目录 {} 时又发生错误：{cleanup_error}。该位置可能留有部分创建内容，请检查后再重试。",
            path.display()
        )),
    }
}

fn cleanup_failed_creation(path: &Path, root_created: bool) -> io::Result<()> {
    let internal = path.join(INTERNAL_DIR);
    for file_name in [
        format!("{DATABASE_FILE}-wal"),
        format!("{DATABASE_FILE}-shm"),
        format!("{DATABASE_FILE}-journal"),
        DATABASE_FILE.to_string(),
        METADATA_FILE.to_string(),
    ] {
        remove_created_file(&internal.join(file_name))?;
    }
    for directory in [INTERNAL_DIR, DOCUMENTS_DIR, TRASH_DIR, THUMBNAILS_DIR] {
        remove_created_directory(&path.join(directory))?;
    }
    if root_created {
        remove_created_directory(path)?;
    } else if fs::read_dir(path)?.next().transpose()?.is_some() {
        return Err(io::Error::other("创建前为空的目录仍包含其他文件"));
    }
    Ok(())
}

fn remove_created_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn remove_created_directory(path: &Path) -> io::Result<()> {
    match fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
