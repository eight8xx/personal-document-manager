mod error;
mod formats;
mod models;
mod service;
mod thumbnail;

pub use error::{LibraryError, LibraryResult};
pub use formats::{
    canonical_file_type, document_format_capabilities, DocumentFormatCapability, DocumentFormatId,
    FormatSecurity, PreviewStrategy, SecurityPolicy, TextExtractionStrategy, ThumbnailStrategy,
    ValidationStrategy,
};
pub use models::{
    BatchDocumentItemResult, BatchDocumentItemStatus, BatchDocumentOperation,
    BatchDocumentOperationRequest, BatchDocumentOperationResult, BootstrapState, CloudSyncWarning,
    CollectionDeleteResult, CollectionSummary, DocumentIndexChangedEvent, DocumentIndexPhase,
    DocumentMetadataUpdate, DocumentPreview, DocumentProcessingStatus, DocumentSearchFilters,
    DocumentSearchQuery, DocumentSearchResponse, DocumentSearchResult, DocumentSummary,
    DocumentThumbnail, EmptyTrashItemResult, EmptyTrashItemStatus, EmptyTrashResult, ImportBatch,
    ImportDecision, ImportItemResult, ImportItemStatus, ImportProgress, IndexRunResult,
    IndexStatus, LibraryLocationInspection, LibrarySummary, LocationStatus, RecentLibrary,
    SearchMatchKind, TagSummary, TrashDocumentSummary,
};
pub use service::{open_directory, ExternalChangeMonitor, LibraryService};
