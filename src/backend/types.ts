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
  fileName: string;
  fileType: string;
  fileSize: number;
  contentHash: string | null;
  collectionId: string;
  processingStatus: DocumentProcessingStatus;
  indexStatus: IndexStatus;
  errorStage: string | null;
  errorMessage: string | null;
  importedAt: string;
  sourcePath: string;
  sourceIdentifier: string;
  lastImportedAt: string;
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
}
