import { BackendError } from "./client";
import type {
  BackendClient,
  BootstrapState,
  LibraryLocationInspection,
  LibrarySummary,
  RecentLibrary
} from "./types";

export interface FakeBackendOptions {
  bootstrap?: BootstrapState;
  selectedDirectory?: string | null;
  inspections?: Record<string, LibraryLocationInspection>;
  createLibrary?: (path: string) => Promise<LibrarySummary>;
}

const emptyBootstrap: BootstrapState = {
  currentLibrary: null,
  recentLibraries: []
};

function defaultLibrary(path: string): LibrarySummary {
  return {
    id: "library-1",
    name: path.split(/[\\/]/).filter(Boolean).at(-1) ?? "资料库",
    path,
    createdAt: "2026-09-13T08:00:00Z"
  };
}

export class FakeBackendClient implements BackendClient {
  calls: string[] = [];
  private state: BootstrapState;
  private readonly selectedDirectory: string | null;
  private readonly inspections: Record<string, LibraryLocationInspection>;
  private readonly createLibraryImpl: (path: string) => Promise<LibrarySummary>;

  constructor(options: FakeBackendOptions = {}) {
    this.state = structuredClone(options.bootstrap ?? emptyBootstrap);
    this.selectedDirectory = options.selectedDirectory ?? null;
    this.inspections = options.inspections ?? {};
    this.createLibraryImpl =
      options.createLibrary ?? (async (path) => defaultLibrary(path));
  }

  async bootstrap(): Promise<BootstrapState> {
    this.calls.push("bootstrap");
    return this.snapshot();
  }

  async inspectLibraryLocation(
    path: string
  ): Promise<LibraryLocationInspection> {
    this.calls.push(`inspect:${path}`);
    return (
      this.inspections[path] ?? {
        path,
        status: "usable",
        isExistingLibrary: false,
        cloudSyncWarning: null,
        reason: null
      }
    );
  }

  async pickLibraryDirectory(): Promise<string | null> {
    this.calls.push("pickLibraryDirectory");
    return this.selectedDirectory;
  }

  async createLibrary(path: string): Promise<LibrarySummary> {
    this.calls.push(`create:${path}`);
    const library = await this.createLibraryImpl(path);
    this.state.currentLibrary = library;
    this.state.recentLibraries = [
      {
        path,
        name: library.name,
        lastOpenedAt: "2026-09-13T08:00:00Z",
        isAvailable: true
      },
      ...this.state.recentLibraries.filter((item) => item.path !== path)
    ];
    return library;
  }

  async openLibrary(path: string): Promise<LibrarySummary> {
    this.calls.push(`open:${path}`);
    const recent = this.state.recentLibraries.find(
      (item) => item.path === path
    );

    if (!recent?.isAvailable) {
      throw new BackendError({
        code: "libraryUnavailable",
        message: "资料库不可用。"
      });
    }

    const library = defaultLibrary(path);
    this.state.currentLibrary = library;
    return library;
  }

  async listRecentLibraries(): Promise<RecentLibrary[]> {
    this.calls.push("listRecentLibraries");
    return [...this.state.recentLibraries];
  }

  async forgetRecentLibrary(path: string): Promise<RecentLibrary[]> {
    this.calls.push(`forget:${path}`);
    this.state.recentLibraries = this.state.recentLibraries.filter(
      (item) => item.path !== path
    );
    return [...this.state.recentLibraries];
  }

  async openLibraryDirectory(path: string): Promise<void> {
    this.calls.push(`openDirectory:${path}`);
  }

  private snapshot(): BootstrapState {
    return {
      currentLibrary: this.state.currentLibrary,
      recentLibraries: [...this.state.recentLibraries]
    };
  }
}
