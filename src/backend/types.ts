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

export interface BackendErrorShape {
  code: string;
  message: string;
}

export interface BackendClient {
  bootstrap(): Promise<BootstrapState>;
  inspectLibraryLocation(path: string): Promise<LibraryLocationInspection>;
  pickLibraryDirectory(): Promise<string | null>;
  createLibrary(path: string): Promise<LibrarySummary>;
  openLibrary(path: string): Promise<LibrarySummary>;
  listRecentLibraries(): Promise<RecentLibrary[]>;
  forgetRecentLibrary(path: string): Promise<RecentLibrary[]>;
  openLibraryDirectory(path: string): Promise<void>;
}
