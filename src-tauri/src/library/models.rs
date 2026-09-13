use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocationStatus {
    Usable,
    ExistingLibrary,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSyncWarning {
    pub provider: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryLocationInspection {
    pub path: String,
    pub status: LocationStatus,
    pub is_existing_library: bool,
    pub cloud_sync_warning: Option<CloudSyncWarning>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibrarySummary {
    pub id: String,
    pub name: String,
    pub path: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentLibrary {
    pub path: String,
    pub name: String,
    pub last_opened_at: String,
    pub is_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapState {
    pub current_library: Option<LibrarySummary>,
    pub recent_libraries: Vec<RecentLibrary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DocumentProcessingStatus {
    Processing,
    Ready,
    Failed,
}

impl DocumentProcessingStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Processing => "processing",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }

    pub(crate) fn from_database(value: &str) -> Option<Self> {
        match value {
            "processing" => Some(Self::Processing),
            "ready" => Some(Self::Ready),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IndexStatus {
    Pending,
    Searchable,
    Failed,
}

impl IndexStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Searchable => "searchable",
            Self::Failed => "failed",
        }
    }

    pub(crate) fn from_database(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "searchable" => Some(Self::Searchable),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSummary {
    pub id: String,
    pub title: String,
    pub file_name: String,
    pub file_type: String,
    pub file_size: i64,
    pub content_hash: Option<String>,
    pub collection_id: String,
    pub processing_status: DocumentProcessingStatus,
    pub index_status: IndexStatus,
    pub error_stage: Option<String>,
    pub error_message: Option<String>,
    pub imported_at: String,
    pub source_path: String,
    pub source_identifier: String,
    pub last_imported_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImportItemStatus {
    Imported,
    Duplicate,
    SourceChanged,
    Failed,
    Ignored,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImportDecision {
    UseExisting,
    ImportAnyway,
    Cancel,
    CreateNew,
    ReplaceExisting,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportItemResult {
    pub item_id: String,
    pub source_path: String,
    pub file_name: String,
    pub file_type: Option<String>,
    pub status: ImportItemStatus,
    pub document_id: Option<String>,
    pub duplicate_document_id: Option<String>,
    pub error_stage: Option<String>,
    pub error_message: Option<String>,
    pub retryable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportBatch {
    pub batch_id: String,
    pub items: Vec<ImportItemResult>,
    pub imported_count: i64,
    pub duplicate_count: i64,
    pub source_changed_count: i64,
    pub failed_count: i64,
    pub ignored_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportProgress {
    pub batch_id: String,
    pub total: usize,
    pub completed: usize,
    pub current_file_name: Option<String>,
    pub current_source_path: Option<String>,
    pub item: Option<ImportItemResult>,
    pub finished: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionSummary {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub is_inbox: bool,
    pub document_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionDeleteResult {
    pub collection_id: String,
    pub target_collection_id: String,
    pub moved_document_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LibraryMetadata {
    pub format_version: u32,
    pub library_id: String,
    pub name: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecentLibraryRecord {
    pub path: String,
    pub name: String,
    pub last_opened_at: String,
}
