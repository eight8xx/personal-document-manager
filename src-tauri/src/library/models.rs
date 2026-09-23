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
    pub description: Option<String>,
    pub document_date: Option<String>,
    pub file_name: String,
    pub file_type: String,
    pub file_size: i64,
    pub content_hash: Option<String>,
    pub collection_id: String,
    pub tags: Vec<TagSummary>,
    pub processing_status: DocumentProcessingStatus,
    pub index_status: IndexStatus,
    pub error_stage: Option<String>,
    pub error_message: Option<String>,
    pub imported_at: String,
    pub source_path: String,
    pub source_identifier: String,
    pub last_imported_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashDocumentSummary {
    pub document: DocumentSummary,
    pub original_collection_id: Option<String>,
    pub original_collection_name: Option<String>,
    pub deleted_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EmptyTrashItemStatus {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmptyTrashItemResult {
    pub document_id: String,
    pub file_name: String,
    pub status: EmptyTrashItemStatus,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmptyTrashResult {
    pub deleted_count: i64,
    pub failed_count: i64,
    pub items: Vec<EmptyTrashItemResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DocumentPreview {
    Pdf {
        data_url: String,
        page_count: Option<u32>,
        page: u32,
    },
    Image {
        data_url: String,
    },
    Text {
        text: String,
    },
    Markdown {
        text: String,
    },
    Docx {
        data_url: String,
        text: String,
        notice: String,
        degraded_features: Vec<String>,
    },
    Pptx {
        data_url: String,
        text: String,
        notice: String,
        degraded_features: Vec<String>,
    },
    Table {
        sheets: Vec<TableSheet>,
        sheet_index: usize,
        start_row: usize,
        cells: Vec<Vec<String>>,
        row_numbers: Vec<usize>,
        column_count: usize,
        has_more_rows: bool,
        degraded_features: Vec<String>,
        notice: Option<String>,
    },
    Failure {
        code: String,
        message: String,
    },
    Unsupported {
        message: String,
    },
}

/// 表格文档（CSV/XLSX）的工作表元数据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableSheet {
    pub index: usize,
    pub name: String,
    /// 未读取过该工作表时为 `None`，避免为切换工作表扫描整本工作簿。
    pub row_count: Option<usize>,
    pub column_count: Option<usize>,
}

/// 表格分页预览请求；缺省值由服务层补齐。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TablePreviewRequest {
    #[serde(default)]
    pub sheet_index: usize,
    #[serde(default)]
    pub start_row: usize,
    #[serde(default)]
    pub row_count: usize,
    #[serde(default)]
    pub column_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DocumentThumbnail {
    Pdf { data_url: String },
    Image { data_url: String },
    Pptx { data_url: String },
    Fallback { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagSummary {
    pub id: String,
    pub name: String,
    pub document_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentMetadataUpdate {
    pub title: String,
    pub description: Option<String>,
    pub document_date: Option<String>,
    pub collection_id: String,
    pub tag_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BatchDocumentOperation {
    MoveToCollection { collection_id: String },
    AddTag { tag_id: String },
    RemoveTag { tag_id: String },
    MoveToTrash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchDocumentOperationRequest {
    pub job_id: String,
    pub document_ids: Vec<String>,
    pub operation: BatchDocumentOperation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BatchDocumentItemStatus {
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchDocumentItemResult {
    pub document_id: String,
    pub status: BatchDocumentItemStatus,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchDocumentOperationResult {
    pub job_id: String,
    pub operation: BatchDocumentOperation,
    pub results: Vec<BatchDocumentItemResult>,
    pub succeeded_count: usize,
    pub failed_count: usize,
    pub cancelled_count: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DocumentSearchFilters {
    pub collection_id: Option<String>,
    pub tag_id: Option<String>,
    pub file_type: Option<String>,
    pub document_date_from: Option<String>,
    pub document_date_to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSearchQuery {
    pub query: String,
    pub filters: DocumentSearchFilters,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SearchMatchKind {
    Content,
    Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSearchResult {
    pub document: DocumentSummary,
    pub snippet: Option<String>,
    pub match_kind: SearchMatchKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSearchResponse {
    pub results: Vec<DocumentSearchResult>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexRunResult {
    pub processed: i64,
    pub searchable: i64,
    pub failed: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DocumentIndexPhase {
    Processing,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentIndexChangedEvent {
    pub library: LibrarySummary,
    pub phase: DocumentIndexPhase,
    pub document_ids: Vec<String>,
    pub result: Option<IndexRunResult>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImportSource {
    FilePicker,
    CollectionDrop,
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
    pub target_collection_id: Option<String>,
    pub collection_id: Option<String>,
    pub notice: Option<String>,
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
    pub target_collection_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportProgress {
    pub library: LibrarySummary,
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

// ---------------------------------------------------------------------------
// 分类规则（工作单 10）
// ---------------------------------------------------------------------------

/// 分类规则：按文件名、类型、来源目录为新建文档指定集合与标签。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(not(test), allow(dead_code))]
pub struct ClassificationRule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// 用户顺序；第一条指定集合的命中规则决定集合。
    pub position: i64,
    pub file_name_pattern: String,
    pub file_type: Option<String>,
    pub source_directory: Option<String>,
    pub collection_id: Option<String>,
    pub tag_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassificationRuleInput {
    pub name: String,
    pub enabled: bool,
    pub file_name_pattern: String,
    pub file_type: Option<String>,
    pub source_directory: Option<String>,
    pub collection_id: Option<String>,
    pub tag_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassificationRuleUpdate {
    pub id: String,
    #[serde(flatten)]
    pub input: ClassificationRuleInput,
}

/// 规则编辑操作；JSON 形状与前端判别联合一致（internally tagged，字段名 camelCase）。
///
/// `rename_all_fields` 是必需的：`rename_all` 只作用于变体名，字段仍需 camelCase
/// 才能与前端发来的 `ruleId` / `orderedRuleIds` 对上。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ClassificationRuleOperation {
    Create {
        rule: ClassificationRuleInput,
    },
    Update {
        rule: ClassificationRuleUpdate,
    },
    Delete {
        rule_id: String,
    },
    SetEnabled {
        rule_id: String,
        enabled: bool,
    },
    Reorder {
        ordered_rule_ids: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassificationPreviewItem {
    pub source_path: String,
    pub file_name: String,
    pub file_type: Option<String>,
    pub collection_id: String,
    pub tag_ids: Vec<String>,
    pub matched_rule_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassificationPreviewRequest {
    pub paths: Vec<String>,
    pub target_collection_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassificationPreviewResponse {
    pub items: Vec<ClassificationPreviewItem>,
}

// ---------------------------------------------------------------------------
// 接收目录（工作单 11/12/13）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReceiveSourceKind {
    Qq,
    Wechat,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReceiveSourceStatus {
    Unconfigured,
    Ready,
    Missing,
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiveSourceCandidate {
    pub path: String,
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiveSource {
    pub id: String,
    pub kind: ReceiveSourceKind,
    pub display_name: String,
    /// 用户确认的目录；未确认时为 `None`，此时不扫描不导入。
    pub path: Option<String>,
    pub enabled: bool,
    pub status: ReceiveSourceStatus,
    pub status_message: Option<String>,
    pub pending_count: i64,
    pub last_scanned_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiveSourceInput {
    pub kind: ReceiveSourceKind,
    pub display_name: String,
    pub path: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiveSourceCandidates {
    pub kind: ReceiveSourceKind,
    pub candidates: Vec<ReceiveSourceCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiveDirectoryListingItem {
    pub path: String,
    pub file_name: String,
    pub file_type: Option<String>,
    pub file_size: i64,
    /// 用户此前明确未选择的文件；补扫不会自动导入它们。
    pub previously_skipped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiveDirectoryListing {
    pub source_id: String,
    pub path: String,
    pub items: Vec<ReceiveDirectoryListingItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiveDirectoryOperation {
    pub source_id: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiveSourceScanResult {
    pub source_id: String,
    pub scanned_count: i64,
    pub imported_count: i64,
    pub skipped_count: i64,
    pub pending_count: i64,
    pub failed_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiveImportLogEntry {
    pub source_id: String,
    pub source_path: String,
    pub file_name: String,
    pub status: ImportItemStatus,
    pub document_id: Option<String>,
    pub collection_id: Option<String>,
    pub tag_ids: Vec<String>,
    pub matched_rule_ids: Vec<String>,
    pub error_message: Option<String>,
    pub created_at: String,
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
