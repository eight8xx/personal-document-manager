import { BackendError } from "./error";
import type {
  BackendClient,
  BootstrapState,
  CollectionDeleteResult,
  CollectionSummary,
  DocumentSummary,
  FileDropHandler,
  ImportBatch,
  ImportDecision,
  ImportItemResult,
  ImportProgress,
  ImportProgressHandler,
  LibraryLocationInspection,
  LibrarySummary,
  RecentLibrary
} from "./types";

export interface FakeBackendOptions {
  bootstrap?: BootstrapState;
  selectedDirectory?: string | null;
  selectedDocument?: string | null;
  selectedDocuments?: string[];
  selectedFolder?: string | null;
  documents?: DocumentSummary[];
  collections?: CollectionSummary[];
  inspections?: Record<string, LibraryLocationInspection>;
  createLibrary?: (path: string) => Promise<LibrarySummary>;
  importDocument?: (path: string) => Promise<DocumentSummary>;
  startImport?: (paths: string[]) => Promise<ImportBatch>;
  resolveImportItem?: (
    itemId: string,
    decision: ImportDecision
  ) => Promise<ImportItemResult>;
  retryImportItem?: (itemId: string) => Promise<ImportItemResult>;
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

const inbox: CollectionSummary = {
  id: "inbox",
  name: "收件箱",
  parentId: null,
  isInbox: true,
  documentCount: 0
};

function fileTypeForPath(path: string): string | null {
  const extension = path.split(".").at(-1)?.toLowerCase() ?? "";
  if (!extension) {
    return null;
  }

  return (
    {
      pdf: "PDF",
      docx: "DOCX",
      txt: "TXT",
      md: "Markdown",
      markdown: "Markdown",
      jpg: "JPG",
      jpeg: "JPG",
      png: "PNG"
    }[extension] ?? extension.toUpperCase()
  );
}

function countsForItems(items: ImportItemResult[]) {
  return {
    importedCount: items.filter((item) => item.status === "imported").length,
    duplicateCount: items.filter((item) => item.status === "duplicate").length,
    sourceChangedCount: items.filter(
      (item) => item.status === "sourceChanged"
    ).length,
    failedCount: items.filter((item) => item.status === "failed").length,
    ignoredCount: items.filter((item) => item.status === "ignored").length
  };
}

export class FakeBackendClient implements BackendClient {
  calls: string[] = [];
  private state: BootstrapState;
  private documents: DocumentSummary[];
  private collections: CollectionSummary[];
  private readonly selectedDirectory: string | null;
  private readonly selectedDocument: string | null;
  private readonly selectedDocuments: string[];
  private readonly selectedFolder: string | null;
  private readonly inspections: Record<string, LibraryLocationInspection>;
  private readonly createLibraryImpl: (path: string) => Promise<LibrarySummary>;
  private readonly importDocumentImpl:
    | ((path: string) => Promise<DocumentSummary>)
    | null;
  private readonly startImportImpl:
    | ((paths: string[]) => Promise<ImportBatch>)
    | null;
  private readonly resolveImportItemImpl:
    | ((
        itemId: string,
        decision: ImportDecision
      ) => Promise<ImportItemResult>)
    | null;
  private readonly retryImportItemImpl:
    | ((itemId: string) => Promise<ImportItemResult>)
    | null;
  private fileDropHandlers = new Set<FileDropHandler>();
  private importProgressHandlers = new Set<ImportProgressHandler>();
  private importBatches = new Map<string, ImportBatch>();
  private nextCollectionId = 1;

  constructor(options: FakeBackendOptions = {}) {
    this.state = structuredClone(options.bootstrap ?? emptyBootstrap);
    this.documents = structuredClone(options.documents ?? []);
    this.collections = structuredClone(options.collections ?? [inbox]);
    if (!this.collections.some((collection) => collection.isInbox)) {
      this.collections.unshift(structuredClone(inbox));
    }
    this.selectedDirectory = options.selectedDirectory ?? null;
    this.selectedDocument = options.selectedDocument ?? null;
    this.selectedDocuments = [...(options.selectedDocuments ?? [])];
    this.selectedFolder = options.selectedFolder ?? null;
    this.inspections = options.inspections ?? {};
    this.createLibraryImpl =
      options.createLibrary ?? (async (path) => defaultLibrary(path));
    this.importDocumentImpl = options.importDocument ?? null;
    this.startImportImpl = options.startImport ?? null;
    this.resolveImportItemImpl = options.resolveImportItem ?? null;
    this.retryImportItemImpl = options.retryImportItem ?? null;
    this.refreshCollectionCounts();
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

  async pickDocumentFiles(): Promise<string[]> {
    this.calls.push("pickDocumentFiles");
    if (this.selectedDocuments.length > 0) {
      return [...this.selectedDocuments];
    }
    return this.selectedDocument ? [this.selectedDocument] : [];
  }

  async pickDocumentFolder(): Promise<string | null> {
    this.calls.push("pickDocumentFolder");
    return this.selectedFolder;
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

  async startImport(paths: string[]): Promise<ImportBatch> {
    this.calls.push(`startImport:${paths.join("|")}`);
    if (this.startImportImpl) {
      const batch = await this.startImportImpl(paths);
      this.importBatches.set(batch.batchId, structuredClone(batch));
      return structuredClone(batch);
    }

    const batchId = `batch-${this.importBatches.size + 1}`;
    const items: ImportItemResult[] = [];
    this.emitImportProgress({
      batchId,
      total: paths.length,
      completed: 0,
      currentFileName: null,
      currentSourcePath: null,
      item: null,
      finished: false
    });

    for (const [index, path] of paths.entries()) {
      const fileName = path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
      let item: ImportItemResult;

      try {
        const document = await this.importDocument(path);
        item = {
          itemId: `${batchId}-item-${index + 1}`,
          sourcePath: path,
          fileName,
          fileType: document.fileType,
          status: "imported",
          documentId: document.id,
          duplicateDocumentId: null,
          errorStage: null,
          errorMessage: null,
          retryable: false
        };
      } catch (caught) {
        item = {
          itemId: `${batchId}-item-${index + 1}`,
          sourcePath: path,
          fileName,
          fileType: fileTypeForPath(path),
          status: "failed",
          documentId: null,
          duplicateDocumentId: null,
          errorStage: "copying",
          errorMessage:
            caught instanceof Error ? caught.message : "无法导入文档。",
          retryable: true
        };
      }

      items.push(item);
      this.emitImportProgress({
        batchId,
        total: paths.length,
        completed: index + 1,
        currentFileName: fileName,
        currentSourcePath: path,
        item,
        finished: index === paths.length - 1
      });
    }

    const batch: ImportBatch = {
      batchId,
      items,
      ...countsForItems(items)
    };
    this.importBatches.set(batchId, structuredClone(batch));
    return structuredClone(batch);
  }

  async resolveImportItem(
    itemId: string,
    decision: ImportDecision
  ): Promise<ImportItemResult> {
    this.calls.push(`resolveImportItem:${itemId}:${decision}`);
    if (this.resolveImportItemImpl) {
      const item = await this.resolveImportItemImpl(itemId, decision);
      this.replaceStoredImportItem(item);
      return structuredClone(item);
    }

    const stored = this.findStoredImportItem(itemId);
    const status =
      decision === "cancel"
        ? "skipped"
        : decision === "useExisting"
          ? "skipped"
          : "imported";
    const item: ImportItemResult = {
      ...stored,
      status,
      documentId:
        decision === "useExisting"
          ? stored.duplicateDocumentId
          : stored.documentId,
      errorStage: null,
      errorMessage: null,
      retryable: false
    };
    this.replaceStoredImportItem(item);
    return structuredClone(item);
  }

  async retryImportItem(itemId: string): Promise<ImportItemResult> {
    this.calls.push(`retryImportItem:${itemId}`);
    if (this.retryImportItemImpl) {
      const item = await this.retryImportItemImpl(itemId);
      this.replaceStoredImportItem(item);
      return structuredClone(item);
    }

    const stored = this.findStoredImportItem(itemId);
    const item: ImportItemResult = {
      ...stored,
      status: "imported",
      documentId: stored.documentId ?? `retried-${itemId}`,
      errorStage: null,
      errorMessage: null,
      retryable: false
    };
    this.replaceStoredImportItem(item);
    return structuredClone(item);
  }

  async subscribeToImportProgress(
    handler: ImportProgressHandler
  ): Promise<() => void> {
    this.calls.push("subscribeToImportProgress");
    this.importProgressHandlers.add(handler);
    return () => {
      this.importProgressHandlers.delete(handler);
    };
  }

  emitImportProgress(progress: ImportProgress) {
    for (const handler of this.importProgressHandlers) {
      handler(structuredClone(progress));
    }
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

  async listCollections(): Promise<CollectionSummary[]> {
    this.calls.push("listCollections");
    return structuredClone(this.collections);
  }

  async createCollection(
    name: string,
    parentId: string | null
  ): Promise<CollectionSummary> {
    this.calls.push(`createCollection:${name}:${parentId ?? "null"}`);
    const trimmedName = name.trim();
    if (!trimmedName) {
      throw new BackendError({
        code: "invalidCollectionName",
        message: "集合名称不能为空。"
      });
    }
    if (parentId && !this.collections.some((item) => item.id === parentId)) {
      throw new BackendError({
        code: "collectionNotFound",
        message: "父集合不存在。"
      });
    }

    const collection: CollectionSummary = {
      id: `collection-${this.nextCollectionId++}`,
      name: trimmedName,
      parentId,
      isInbox: false,
      documentCount: 0
    };
    this.collections.push(collection);
    return structuredClone(collection);
  }

  async renameCollection(
    collectionId: string,
    name: string
  ): Promise<CollectionSummary> {
    this.calls.push(`renameCollection:${collectionId}:${name}`);
    const collection = this.requireCollection(collectionId);
    if (collection.isInbox) {
      throw new BackendError({
        code: "inboxProtected",
        message: "收件箱不能重命名。"
      });
    }
    const trimmedName = name.trim();
    if (!trimmedName) {
      throw new BackendError({
        code: "invalidCollectionName",
        message: "集合名称不能为空。"
      });
    }
    collection.name = trimmedName;
    return structuredClone(collection);
  }

  async moveCollection(
    collectionId: string,
    parentId: string | null
  ): Promise<CollectionSummary> {
    this.calls.push(`moveCollection:${collectionId}:${parentId ?? "null"}`);
    const collection = this.requireCollection(collectionId);
    if (collection.isInbox) {
      throw new BackendError({
        code: "inboxProtected",
        message: "收件箱不能移动。"
      });
    }
    if (parentId === collectionId) {
      throw new BackendError({
        code: "collectionCycle",
        message: "不能将集合移动到自身。"
      });
    }
    const descendants = this.collectionDescendantIds(collectionId);
    if (parentId && descendants.has(parentId)) {
      throw new BackendError({
        code: "collectionCycle",
        message: "不能将集合移动到其子集合中。"
      });
    }
    if (parentId && !this.collections.some((item) => item.id === parentId)) {
      throw new BackendError({
        code: "collectionNotFound",
        message: "目标集合不存在。"
      });
    }
    collection.parentId = parentId;
    return structuredClone(collection);
  }

  async deleteCollection(
    collectionId: string
  ): Promise<CollectionDeleteResult> {
    this.calls.push(`deleteCollection:${collectionId}`);
    const collection = this.requireCollection(collectionId);
    if (collection.isInbox) {
      throw new BackendError({
        code: "inboxProtected",
        message: "收件箱不能删除。"
      });
    }

    let movedDocumentCount = 0;
    this.documents = this.documents.map((document) => {
      if (document.collectionId !== collectionId) {
        return document;
      }
      movedDocumentCount += 1;
      return { ...document, collectionId: "inbox" };
    });

    for (const child of this.collections) {
      if (child.parentId === collectionId) {
        child.parentId = collection.parentId ?? "inbox";
      }
    }
    this.collections = this.collections.filter(
      (item) => item.id !== collectionId
    );
    this.refreshCollectionCounts();

    return {
      collectionId,
      targetCollectionId: "inbox",
      movedDocumentCount
    };
  }

  async moveDocumentToCollection(
    documentId: string,
    collectionId: string
  ): Promise<DocumentSummary> {
    this.calls.push(`moveDocumentToCollection:${documentId}:${collectionId}`);
    const document = this.documents.find((item) => item.id === documentId);
    if (!document) {
      throw new BackendError({
        code: "documentNotFound",
        message: "文档不存在。"
      });
    }
    if (!this.collections.some((item) => item.id === collectionId)) {
      throw new BackendError({
        code: "collectionNotFound",
        message: "目标集合不存在。"
      });
    }

    document.collectionId = collectionId;
    this.refreshCollectionCounts();
    return structuredClone(document);
  }

  private snapshot(): BootstrapState {
    return {
      currentLibrary: this.state.currentLibrary,
      recentLibraries: [...this.state.recentLibraries]
    };
  }

  private findStoredImportItem(itemId: string): ImportItemResult {
    for (const batch of this.importBatches.values()) {
      const item = batch.items.find((candidate) => candidate.itemId === itemId);
      if (item) {
        return structuredClone(item);
      }
    }
    throw new BackendError({
      code: "importItemNotFound",
      message: "导入项目不存在。"
    });
  }

  private replaceStoredImportItem(nextItem: ImportItemResult) {
    for (const [batchId, batch] of this.importBatches) {
      const index = batch.items.findIndex(
        (item) => item.itemId === nextItem.itemId
      );
      if (index === -1) {
        continue;
      }
      const items = [...batch.items];
      items[index] = structuredClone(nextItem);
      this.importBatches.set(batchId, {
        ...batch,
        items,
        ...countsForItems(items)
      });
      return;
    }
  }

  private requireCollection(collectionId: string): CollectionSummary {
    const collection = this.collections.find(
      (item) => item.id === collectionId
    );
    if (!collection) {
      throw new BackendError({
        code: "collectionNotFound",
        message: "集合不存在。"
      });
    }
    return collection;
  }

  private collectionDescendantIds(collectionId: string): Set<string> {
    const descendants = new Set<string>();
    const visit = (parentId: string) => {
      for (const collection of this.collections) {
        if (
          collection.parentId === parentId &&
          !descendants.has(collection.id)
        ) {
          descendants.add(collection.id);
          visit(collection.id);
        }
      }
    };
    visit(collectionId);
    return descendants;
  }

  private refreshCollectionCounts() {
    for (const collection of this.collections) {
      collection.documentCount = this.documents.filter(
        (document) => document.collectionId === collection.id
      ).length;
    }
  }
}
