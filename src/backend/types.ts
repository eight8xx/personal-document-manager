export type LocationStatus = "usable" | "existingLibrary" | "blocked";

export interface CloudSyncWarning {
  provider: string;
  message: string;
}

export interface LibraryLocationInspection {
  path: string;
  status: LocationStatus;
  isExistingLibrary: boolean;
  cloudSyncWarning: CloudSyncWarning | null;
  reason: string | null;
}

export interface LibrarySummary {
  id: string;
  name: string;
  path: string;
  createdAt: string;
}

export interface RecentLibrary {
  path: string;
  name: string;
  lastOpenedAt: string;
  isAvailable: boolean;
}

export interface BootstrapState {
  currentLibrary: LibrarySummary | null;
  recentLibraries: RecentLibrary[];
}

export type DocumentProcessingStatus = "processing" | "ready" | "failed";
export type IndexStatus = "pending" | "searchable" | "failed";

export interface DocumentSummary {
  id: string;
  title: string;
  description: string | null;
  documentDate: string | null;
  fileName: string;
  fileType: string;
  fileSize: number;
  contentHash: string | null;
  collectionId: string;
  tags: TagSummary[];
  processingStatus: DocumentProcessingStatus;
  indexStatus: IndexStatus;
  errorStage: string | null;
  errorMessage: string | null;
  importedAt: string;
  sourcePath: string;
  sourceIdentifier: string;
  lastImportedAt: string;
}

export interface TrashDocumentSummary {
  document: DocumentSummary;
  originalCollectionId: string | null;
  originalCollectionName: string | null;
  deletedAt: string;
}

export type EmptyTrashItemStatus = "succeeded" | "failed";

export interface EmptyTrashItemResult {
  documentId: string;
  fileName: string;
  status: EmptyTrashItemStatus;
  errorCode: string | null;
  errorMessage: string | null;
}

export interface EmptyTrashResult {
  deletedCount: number;
  failedCount: number;
  items: EmptyTrashItemResult[];
}

export type DocumentPreview =
  | {
      kind: "pdf";
      dataUrl: string;
      pageCount: number | null;
      page: number;
    }
  | {
      kind: "image";
      dataUrl: string;
    }
  | {
      kind: "text";
      text: string;
    }
  | {
      kind: "markdown";
      text: string;
    }
  | {
      kind: "docx";
      dataUrl: string;
      text: string;
      notice: string;
      degradedFeatures: string[];
    }
  | {
      kind: "pptx";
      dataUrl: string;
      text: string;
      notice: string;
      degradedFeatures: string[];
    }
  | {
      kind: "failure";
      code: string;
      message: string;
    }
  | {
      kind: "unsupported";
      message: string;
    };

export type DocumentThumbnail =
  | {
      kind: "pdf" | "image" | "pptx";
      dataUrl: string;
    }
  | {
      kind: "fallback";
      reason: string;
    };

export type DocumentFormatId =
  | "pdf"
  | "docx"
  | "txt"
  | "markdown"
  | "jpg"
  | "png"
  | "pptx";

export type FormatSecurityPolicy = "blocked" | "userInitiated";

export interface DocumentFormatSecurity {
  macros: FormatSecurityPolicy;
  scripts: FormatSecurityPolicy;
  embeddedObjects: FormatSecurityPolicy;
  remoteResources: FormatSecurityPolicy;
  mediaAutoplay: FormatSecurityPolicy;
  sourceMutation: FormatSecurityPolicy;
  externalNavigation: FormatSecurityPolicy;
}

export interface DocumentFormatCapability {
  id: DocumentFormatId;
  displayType: string;
  extensions: string[];
  importEnabled: boolean;
  validation:
    | "pdfSignature"
    | "docxPackage"
    | "plainText"
    | "jpegSignature"
    | "pngSignature"
    | "pptxPackage";
  preview:
    | "pdfPages"
    | "docxLayout"
    | "plainText"
    | "safeMarkdown"
    | "localImage"
    | "pptxPages";
  thumbnail:
    | "pdfFirstPage"
    | "localImage"
    | "typeIcon"
    | "pptxFirstPage";
  textExtraction:
    | "pdfText"
    | "docxText"
    | "plainText"
    | "none"
    | "pptxText";
  searchable: boolean;
  mediaType: string | null;
  renderer: string;
  security: DocumentFormatSecurity;
}

export interface TagSummary {
  id: string;
  name: string;
  documentCount: number;
}

export interface DocumentMetadataUpdate {
  title: string;
  description: string | null;
  documentDate: string | null;
  collectionId: string;
  tagIds: string[];
}

export type BatchDocumentOperation =
  | { kind: "moveToCollection"; collectionId: string }
  | { kind: "addTag"; tagId: string }
  | { kind: "removeTag"; tagId: string }
  | { kind: "moveToTrash" };

export interface BatchDocumentOperationRequest {
  jobId: string;
  documentIds: string[];
  operation: BatchDocumentOperation;
}

export type BatchDocumentItemStatus =
  | "succeeded"
  | "failed"
  | "cancelled";

export interface BatchDocumentItemResult {
  documentId: string;
  status: BatchDocumentItemStatus;
  errorCode: string | null;
  errorMessage: string | null;
}

export interface BatchDocumentOperationResult {
  jobId: string;
  operation: BatchDocumentOperation;
  results: BatchDocumentItemResult[];
  succeededCount: number;
  failedCount: number;
  cancelledCount: number;
}

export interface DocumentSearchFilters {
  collectionId: string | null;
  tagId: string | null;
  fileType: string | null;
  documentDateFrom: string | null;
  documentDateTo: string | null;
}

export interface DocumentSearchQuery {
  query: string;
  filters: DocumentSearchFilters;
}

export type SearchMatchKind = "content" | "metadata";

export interface DocumentSearchResult {
  document: DocumentSummary;
  snippet: string | null;
  matchKind: SearchMatchKind;
}

export interface DocumentSearchResponse {
  results: DocumentSearchResult[];
}

export interface IndexRunResult {
  processed: number;
  searchable: number;
  failed: number;
}

export type DocumentIndexPhase = "processing" | "completed";

export interface DocumentIndexChangedEvent {
  phase: DocumentIndexPhase;
  documentIds: string[];
  result: IndexRunResult | null;
}

export type ImportItemStatus =
  | "imported"
  | "duplicate"
  | "sourceChanged"
  | "failed"
  | "ignored"
  | "skipped";

export type ImportDecision =
  | "useExisting"
  | "importAnyway"
  | "cancel"
  | "createNew"
  | "replaceExisting";

export interface ImportItemResult {
  itemId: string;
  sourcePath: string;
  fileName: string;
  fileType: string | null;
  status: ImportItemStatus;
  documentId: string | null;
  duplicateDocumentId: string | null;
  errorStage: string | null;
  errorMessage: string | null;
  retryable: boolean;
}

export interface ImportBatch {
  batchId: string;
  items: ImportItemResult[];
  importedCount: number;
  duplicateCount: number;
  sourceChangedCount: number;
  failedCount: number;
  ignoredCount: number;
}

export interface ImportProgress {
  batchId: string;
  total: number;
  completed: number;
  currentFileName: string | null;
  currentSourcePath: string | null;
  item: ImportItemResult | null;
  finished: boolean;
}

export interface CollectionSummary {
  id: string;
  name: string;
  parentId: string | null;
  isInbox: boolean;
  documentCount: number;
}

export interface CollectionDeleteResult {
  collectionId: string;
  targetCollectionId: string;
  movedDocumentCount: number;
}

export interface BackendErrorShape {
  code: string;
  message: string;
}

export type FileDropHandler = (paths: string[]) => void;
export type ImportProgressHandler = (progress: ImportProgress) => void;
export type DocumentIndexChangedHandler = (
  event: DocumentIndexChangedEvent
) => void;

export interface BackendClient {
  bootstrap(): Promise<BootstrapState>;
  inspectLibraryLocation(path: string): Promise<LibraryLocationInspection>;
  pickLibraryDirectory(): Promise<string | null>;
  createLibrary(path: string): Promise<LibrarySummary>;
  openLibrary(path: string): Promise<LibrarySummary>;
  pickDocumentFile(): Promise<string | null>;
  pickDocumentFiles(): Promise<string[]>;
  pickDocumentFolder(): Promise<string | null>;
  importDocument(path: string): Promise<DocumentSummary>;
  startImport(paths: string[]): Promise<ImportBatch>;
  resolveImportItem(
    itemId: string,
    decision: ImportDecision
  ): Promise<ImportItemResult>;
  retryImportItem(itemId: string): Promise<ImportItemResult>;
  subscribeToImportProgress(
    handler: ImportProgressHandler
  ): Promise<() => void>;
  listDocuments(): Promise<DocumentSummary[]>;
  subscribeToFileDrops(handler: FileDropHandler): Promise<() => void>;
  getDocumentPreview(
    documentId: string,
    page?: number
  ): Promise<DocumentPreview>;
  getDocumentThumbnail(documentId: string): Promise<DocumentThumbnail>;
  saveDocumentThumbnail(
    documentId: string,
    thumbnailDataUrl: string
  ): Promise<DocumentThumbnail>;
  openDocument(documentId: string): Promise<void>;
  openExternalUrl(url: string): Promise<void>;
  listDocumentFormatCapabilities(): Promise<DocumentFormatCapability[]>;
  listRecentLibraries(): Promise<RecentLibrary[]>;
  forgetRecentLibrary(path: string): Promise<RecentLibrary[]>;
  openLibraryDirectory(path: string): Promise<void>;
  listCollections(): Promise<CollectionSummary[]>;
  createCollection(
    name: string,
    parentId: string | null
  ): Promise<CollectionSummary>;
  renameCollection(
    collectionId: string,
    name: string
  ): Promise<CollectionSummary>;
  moveCollection(
    collectionId: string,
    parentId: string | null
  ): Promise<CollectionSummary>;
  deleteCollection(collectionId: string): Promise<CollectionDeleteResult>;
  moveDocumentToCollection(
    documentId: string,
    collectionId: string
  ): Promise<DocumentSummary>;
  moveDocumentToTrash(documentId: string): Promise<void>;
  listTrashDocuments(): Promise<TrashDocumentSummary[]>;
  restoreDocument(documentId: string): Promise<DocumentSummary>;
  permanentlyDeleteDocument(documentId: string): Promise<void>;
  emptyTrash(): Promise<EmptyTrashResult>;
  listTags(): Promise<TagSummary[]>;
  createTag(name: string): Promise<TagSummary>;
  renameTag(tagId: string, name: string): Promise<TagSummary>;
  deleteTag(tagId: string): Promise<void>;
  addTagToDocument(
    documentId: string,
    tagId: string
  ): Promise<DocumentSummary>;
  removeTagFromDocument(
    documentId: string,
    tagId: string
  ): Promise<DocumentSummary>;
  updateDocumentMetadata(
    documentId: string,
    update: DocumentMetadataUpdate
  ): Promise<DocumentSummary>;
  batchOrganizeDocuments(
    request: BatchDocumentOperationRequest
  ): Promise<BatchDocumentOperationResult>;
  cancelBatchDocumentOperation(jobId: string): Promise<boolean>;
  searchDocuments(
    request: DocumentSearchQuery
  ): Promise<DocumentSearchResponse>;
  pendingIndexCount(): Promise<number>;
  indexPendingDocuments(): Promise<IndexRunResult>;
  retryDocumentIndex(documentId: string): Promise<DocumentSummary>;
  subscribeToDocumentIndexChanges(
    handler: DocumentIndexChangedHandler
  ): Promise<() => void>;
}
