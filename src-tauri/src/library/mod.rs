mod error;
mod formats;
mod limits;
mod models;
mod ooxml;
mod pdf_security;
mod service;
mod store;
pub mod table;
mod thumbnail;

pub use error::{LibraryError, LibraryResult};
pub use formats::{
    canonical_file_type, document_format_capabilities, DocumentFormatCapability, DocumentFormatId,
    FormatSecurity, PreviewStrategy, SecurityPolicy, TextExtractionStrategy, ThumbnailStrategy,
    ValidationStrategy,
};
pub use limits::{
    ArchiveLimits, ExpansionBudget, INDEX_ARCHIVE_EXPANDED_BYTES, MAX_ARCHIVE_INPUT_BYTES,
    MAX_ENTRY_COMPRESSION_RATIO, MAX_ENTRY_DECLARED_BYTES, MAX_EXTRACTED_TEXT_CHARS,
    MAX_TABLE_CELL_CHARS, MAX_TABLE_PREVIEW_COLUMNS, MAX_TABLE_PREVIEW_ROWS,
    PREVIEW_ARCHIVE_EXPANDED_BYTES, TABLE_PREVIEW_ARCHIVE_EXPANDED_BYTES,
};
pub use models::{
    BatchDocumentItemResult, BatchDocumentItemStatus, BatchDocumentOperation,
    BatchDocumentOperationRequest, BatchDocumentOperationResult, BootstrapState,
    ClassificationPreviewItem, ClassificationPreviewRequest, ClassificationPreviewResponse,
    ClassificationRule, ClassificationRuleInput, ClassificationRuleOperation,
    ClassificationRuleUpdate, CloudSyncWarning, CollectionDeleteResult, CollectionSummary,
    DocumentIndexChangedEvent, DocumentIndexPhase, DocumentMetadataUpdate, DocumentPreview,
    DocumentProcessingStatus, DocumentSearchFilters, DocumentSearchQuery, DocumentSearchResponse,
    DocumentSearchResult, DocumentSummary, DocumentThumbnail, EmptyTrashItemResult,
    EmptyTrashItemStatus, EmptyTrashResult, ImportBatch, ImportDecision, ImportItemResult,
    ImportItemStatus, ImportProgress, ImportSource, IndexRunResult, IndexStatus,
    LibraryLocationInspection, LibrarySummary, LocationStatus, ReceiveDirectoryListing,
    ReceiveDirectoryListingItem, ReceiveDirectoryOperation, ReceiveImportLogEntry, ReceiveSource,
    ReceiveSourceCandidate, ReceiveSourceCandidates, ReceiveSourceInput, ReceiveSourceKind,
    ReceiveSourceScanResult, ReceiveSourceStatus, RecentLibrary, SearchMatchKind,
    TablePreviewRequest, TableSheet, TagSummary, TrashDocumentSummary,
};
pub use service::{open_directory, ExternalChangeMonitor, LibraryService};
