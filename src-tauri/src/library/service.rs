use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::error::{LibraryError, LibraryResult};
use super::models::{
    BootstrapState, CloudSyncWarning, CollectionDeleteResult, CollectionSummary,
    DocumentProcessingStatus, DocumentSummary, ImportBatch, ImportDecision, ImportItemResult,
    ImportItemStatus, ImportProgress, IndexStatus, LibraryLocationInspection, LibraryMetadata,
    LibrarySummary, LocationStatus, RecentLibrary, RecentLibraryRecord,
};

const FORMAT_VERSION: u32 = 1;
const INTERNAL_DIR: &str = ".pdm";
const METADATA_FILE: &str = "library.json";
const DATABASE_FILE: &str = "library.sqlite3";
const DOCUMENTS_DIR: &str = "documents";
const TRASH_DIR: &str = "trash";
const THUMBNAILS_DIR: &str = "thumbnails";
const RECENT_FILE: &str = "recent_libraries.json";
const MAX_RECENT_LIBRARIES: usize = 10;

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

        let summary = summary_from_metadata(&path, &metadata);
        self.current = Some(OpenLibrary {
            summary: summary.clone(),
            connection,
        });
        self.record_recent(&summary)?;
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
        let file_type = supported_file_type(&source_path).ok_or_else(|| {
            LibraryError::UnsupportedFile(
                "不支持该文件格式。仅支持 PDF、DOCX、TXT、Markdown、JPG 和 PNG 文件。".to_string(),
            )
        })?;
        let file_size = i64::try_from(source_metadata.len())
            .map_err(|_| LibraryError::ImportFile("文件大小超出支持范围。".to_string()))?;
        if let Err(message) = validate_file_content(&source_path, file_type) {
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
            file_name,
            file_type: file_type.to_string(),
            file_size,
            content_hash,
            collection_id: "inbox".to_string(),
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
                error_stage, error_message, imported_at, created_at, updated_at
            )
            VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13, ?13
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
        mut on_progress: F,
    ) -> LibraryResult<ImportBatch>
    where
        F: FnMut(ImportProgress),
    {
        if self.current.is_none() {
            return Err(LibraryError::NoCurrentLibrary);
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
                ScanEntry::File(path) => self.process_import_file(&path, false, None)?,
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
                    }
                }
                ScanEntry::Failed {
                    source_path,
                    message,
                } => self.failure_item(
                    Uuid::new_v4().to_string(),
                    source_path,
                    None,
                    None,
                    "scan",
                    message,
                    true,
                ),
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

        self.process_import_file(
            Path::new(&context.item.source_path),
            false,
            Some(item_id.to_string()),
        )
    }

    fn process_import_file(
        &mut self,
        source_path: &Path,
        force_import: bool,
        existing_item_id: Option<String>,
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
        let Some(file_type) = supported_file_type(&source_path) else {
            return Ok(self.failure_item(
                item_id,
                source_path_text,
                Some(file_name),
                None,
                "scan",
                "不支持该文件格式。仅支持 PDF、DOCX、TXT、Markdown、JPG 和 PNG 文件。".to_string(),
                true,
            ));
        };
        let file_size = match i64::try_from(source_metadata.len()) {
            Ok(size) => size,
            Err(_) => {
                return Ok(self.failure_item(
                    item_id,
                    source_path_text,
                    Some(file_name),
                    Some(file_type.to_string()),
                    "scan",
                    "文件大小超出支持范围。".to_string(),
                    true,
                ));
            }
        };
        if let Err(message) = validate_file_content(&source_path, file_type) {
            return Ok(self.failure_item(
                item_id,
                source_path_text,
                Some(file_name),
                Some(file_type.to_string()),
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
                    Some(file_type.to_string()),
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
            file_type: file_type.to_string(),
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
            },
        )
    }

    fn create_new_document(
        &mut self,
        item_id: String,
        pending: PendingImport,
    ) -> LibraryResult<ImportItemResult> {
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
        let document = DocumentSummary {
            id: document_id.clone(),
            title: pending
                .path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .filter(|stem| !stem.is_empty())
                .unwrap_or_else(|| pending.file_name.clone()),
            file_name: pending.file_name.clone(),
            file_type: pending.file_type.clone(),
            file_size: pending.file_size,
            content_hash: Some(copied_hash),
            collection_id: "inbox".to_string(),
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
                        error_stage, error_message, imported_at, created_at, updated_at
                    )
                    VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                        NULL, NULL, ?11, ?11, ?11
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
                        updated_at = ?6
                    WHERE id = ?7 AND deleted_at IS NULL
                    ",
                    params![
                        &pending.file_name,
                        &pending.file_type,
                        file_size,
                        &copied_hash,
                        &library_path,
                        &last_imported_at,
                        &pending.existing_document_id,
                    ],
                )
                .map_err(|error| ("database", error.to_string()))?;
            if updated == 0 {
                return Err(("database", "要替换的文档不存在或已删除。".to_string()));
            }
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
        })
    }

    fn pending_item(
        &mut self,
        item_id: String,
        prepared: PreparedSource,
        content_hash: String,
        status: ImportItemStatus,
        existing_document_id: String,
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
        };
        self.import_items.insert(
            item_id,
            ImportItemContext {
                item: item.clone(),
                pending: Some(PendingImport {
                    path: prepared.path,
                    source_path: prepared.source_path,
                    file_name: prepared.file_name,
                    file_type: prepared.file_type,
                    file_size: prepared.file_size,
                    source_identifier: prepared.source_identifier,
                    content_hash,
                    existing_document_id,
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
        };
        self.import_items.insert(
            item_id,
            ImportItemContext {
                item: item.clone(),
                pending: None,
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
            return Err(LibraryError::ImportFile(format!(
                "文档不存在或已删除：{document_id}"
            )));
        }
        load_document_summary(&library.connection, document_id)
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
                COALESCE(s.last_imported_at, d.imported_at)
            FROM documents d
            LEFT JOIN sources s ON s.document_id = d.id
            WHERE d.deleted_at IS NULL
            ORDER BY d.imported_at DESC, d.id DESC
            ",
        )?;
        let documents = statement
            .query_map([], document_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(documents)
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
                message: "不支持该文件格式。仅支持 PDF、DOCX、TXT、Markdown、JPG 和 PNG 文件。"
                    .to_string(),
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
    connection
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
                COALESCE(s.last_imported_at, d.imported_at)
            FROM documents d
            LEFT JOIN sources s ON s.document_id = d.id
            WHERE d.id = ?1 AND d.deleted_at IS NULL
            ",
            params![document_id],
            document_from_row,
        )
        .optional()?
        .ok_or_else(|| LibraryError::ImportFile(format!("文档不存在或已删除：{document_id}")))
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
            library_path TEXT NOT NULL,
            processing_status TEXT NOT NULL,
            index_status TEXT NOT NULL,
            error_stage TEXT,
            error_message TEXT,
            imported_at TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            deleted_at TEXT
        );

        CREATE INDEX IF NOT EXISTS documents_collection_idx
            ON documents(collection_id);
        CREATE INDEX IF NOT EXISTS documents_processing_status_idx
            ON documents(processing_status);
        CREATE INDEX IF NOT EXISTS documents_content_hash_idx
            ON documents(content_hash);

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
    let extension = path.extension()?.to_string_lossy().to_ascii_lowercase();
    match extension.as_str() {
        "pdf" => Some("PDF"),
        "docx" => Some("DOCX"),
        "txt" => Some("TXT"),
        "md" | "markdown" => Some("Markdown"),
        "jpg" | "jpeg" => Some("JPG"),
        "png" => Some("PNG"),
        _ => None,
    }
}

fn validate_file_content(path: &Path, file_type: &str) -> Result<(), String> {
    match file_type {
        "PDF" => {
            let prefix = read_prefix(path, 1024)?;
            if prefix.windows(5).any(|window| window == b"%PDF-") {
                Ok(())
            } else {
                Err("文件内容不是有效的 PDF。".to_string())
            }
        }
        "JPG" => expect_prefix(path, &[0xFF, 0xD8, 0xFF], "文件内容不是有效的 JPG。"),
        "PNG" => expect_prefix(
            path,
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
            "文件内容不是有效的 PNG。",
        ),
        "DOCX" => expect_prefix(path, b"PK", "文件内容不是有效的 DOCX。"),
        _ => Ok(()),
    }
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
        file_name: row.get(2)?,
        file_type: row.get(3)?,
        file_size: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
        content_hash: row.get(5)?,
        collection_id: row.get(6)?,
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
