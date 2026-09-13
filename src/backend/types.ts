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

export interface BackendErrorShape {
  code: string;
  message: string;
}

export type FileDropHandler = (paths: string[]) => void;

export interface BackendClient {
  bootstrap(): Promise<BootstrapState>;
  inspectLibraryLocation(path: string): Promise<LibraryLocationInspection>;
  pickLibraryDirectory(): Promise<string | null>;
  createLibrary(path: string): Promise<LibrarySummary>;
  openLibrary(path: string): Promise<LibrarySummary>;
  pickDocumentFile(): Promise<string | null>;
  importDocument(path: string): Promise<DocumentSummary>;
  listDocuments(): Promise<DocumentSummary[]>;
  subscribeToFileDrops(handler: FileDropHandler): Promise<() => void>;
  listRecentLibraries(): Promise<RecentLibrary[]>;
  forgetRecentLibrary(path: string): Promise<RecentLibrary[]>;
  openLibraryDirectory(path: string): Promise<void>;
}
