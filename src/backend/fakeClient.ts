import { BackendError } from "./error";
import type {
  BackendClient,
  BootstrapState,
  DocumentSummary,
  FileDropHandler,
  LibraryLocationInspection,
  LibrarySummary,
  RecentLibrary
} from "./types";

export interface FakeBackendOptions {
  bootstrap?: BootstrapState;
  selectedDirectory?: string | null;
  selectedDocument?: string | null;
  documents?: DocumentSummary[];
  inspections?: Record<string, LibraryLocationInspection>;
  createLibrary?: (path: string) => Promise<LibrarySummary>;
  importDocument?: (path: string) => Promise<DocumentSummary>;
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

function defaultDocument(path: string): DocumentSummary {
  const fileName = path.split(/[\\/]/).filter(Boolean).at(-1) ?? "未命名文档";
  const extension = fileName.split(".").at(-1)?.toLowerCase() ?? "";
  const fileType =
    {
      pdf: "PDF",
      docx: "DOCX",
      txt: "TXT",
      md: "Markdown",
      markdown: "Markdown",
      jpg: "JPG",
      jpeg: "JPG",
      png: "PNG"
    }[extension] ?? extension.toUpperCase();
  const importedAt = "2026-09-13T08:10:00Z";

  return {
    id: `document-${fileName}`,
    title: fileName.replace(/\.[^.]+$/, ""),
    fileName,
    fileType,
    fileSize: 0,
    contentHash: null,
    collectionId: "inbox",
    processingStatus: "ready",
    indexStatus: "pending",
    errorStage: null,
    errorMessage: null,
    importedAt,
    sourcePath: path,
    sourceIdentifier: path.toLocaleLowerCase(),
    lastImportedAt: importedAt
  };
}

export class FakeBackendClient implements BackendClient {
  calls: string[] = [];
  private state: BootstrapState;
  private documents: DocumentSummary[];
  private readonly selectedDirectory: string | null;
  private readonly selectedDocument: string | null;
  private readonly inspections: Record<string, LibraryLocationInspection>;
  private readonly createLibraryImpl: (path: string) => Promise<LibrarySummary>;
  private readonly importDocumentImpl:
    | ((path: string) => Promise<DocumentSummary>)
    | null;
  private fileDropHandlers = new Set<FileDropHandler>();

  constructor(options: FakeBackendOptions = {}) {
    this.state = structuredClone(options.bootstrap ?? emptyBootstrap);
    this.documents = structuredClone(options.documents ?? []);
    this.selectedDirectory = options.selectedDirectory ?? null;
    this.selectedDocument = options.selectedDocument ?? null;
    this.inspections = options.inspections ?? {};
    this.createLibraryImpl =
      options.createLibrary ?? (async (path) => defaultLibrary(path));
    this.importDocumentImpl = options.importDocument ?? null;
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

  async pickDocumentFile(): Promise<string | null> {
    this.calls.push("pickDocumentFile");
    return this.selectedDocument;
  }

  async importDocument(path: string): Promise<DocumentSummary> {
    this.calls.push(`import:${path}`);
    if (this.importDocumentImpl) {
      const document = await this.importDocumentImpl(path);
      this.documents = [
        document,
        ...this.documents.filter((item) => item.id !== document.id)
      ];
      return document;
    }

    const extension = path.split(".").at(-1)?.toLowerCase();
    if (
      !extension ||
      !["pdf", "docx", "txt", "md", "markdown", "jpg", "jpeg", "png"].includes(
        extension
      )
    ) {
      throw new BackendError({
        code: "unsupportedFile",
        message:
          "不支持该文件格式。仅支持 PDF、DOCX、TXT、Markdown、JPG 和 PNG 文件。"
      });
    }

    const document = defaultDocument(path);
    this.documents = [document, ...this.documents];
    return document;
  }

  async listDocuments(): Promise<DocumentSummary[]> {
    this.calls.push("listDocuments");
    return structuredClone(this.documents);
  }

  async subscribeToFileDrops(handler: FileDropHandler): Promise<() => void> {
    this.calls.push("subscribeToFileDrops");
    this.fileDropHandlers.add(handler);
    return () => {
      this.fileDropHandlers.delete(handler);
    };
  }

  emitFileDrop(paths: string[]) {
    for (const handler of this.fileDropHandlers) {
      handler(paths);
    }
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
