use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::error::{LibraryError, LibraryResult};
use super::models::{
    BootstrapState, CloudSyncWarning, DocumentProcessingStatus, DocumentSummary, IndexStatus,
    LibraryLocationInspection, LibraryMetadata, LibrarySummary, LocationStatus, RecentLibrary,
    RecentLibraryRecord,
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
}

struct OpenLibrary {
    summary: LibrarySummary,
    connection: Connection,
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
