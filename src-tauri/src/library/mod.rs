mod error;
mod models;
mod service;

pub use error::{LibraryError, LibraryResult};
pub use models::{
    BootstrapState, CloudSyncWarning, CollectionDeleteResult, CollectionSummary,
    DocumentMetadataUpdate, DocumentPreview, DocumentProcessingStatus, DocumentSearchFilters,
    DocumentSearchQuery, DocumentSearchResponse, DocumentSearchResult, DocumentSummary,
    DocumentThumbnail, ImportBatch, ImportDecision, ImportItemResult, ImportItemStatus,
    ImportProgress, IndexRunResult, IndexStatus, LibraryLocationInspection, LibrarySummary,
    LocationStatus, RecentLibrary, SearchMatchKind, TagSummary,
};
pub use service::{open_directory, LibraryService};
