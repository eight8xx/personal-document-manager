import { BackendError } from "./error";
import type {
  BackendClient,
  BootstrapState,
  CollectionDeleteResult,
  CollectionSummary,
  DocumentMetadataUpdate,
  DocumentPreview,
  DocumentSearchQuery,
  DocumentSearchResponse,
  DocumentSearchResult,
  DocumentSummary,
  DocumentThumbnail,
  FileDropHandler,
  ImportBatch,
  ImportDecision,
  ImportItemResult,
  ImportProgress,
  ImportProgressHandler,
  IndexRunResult,
  LibraryLocationInspection,
  LibrarySummary,
  RecentLibrary,
  TagSummary
} from "./types";

export interface FakeBackendOptions {
  bootstrap?: BootstrapState;
  selectedDirectory?: string | null;
  selectedDocument?: string | null;
  selectedDocuments?: string[];
  selectedFolder?: string | null;
  documents?: DocumentSummary[];
  collections?: CollectionSummary[];
  tags?: TagSummary[];
  inspections?: Record<string, LibraryLocationInspection>;
  createLibrary?: (path: string) => Promise<LibrarySummary>;
  importDocument?: (path: string) => Promise<DocumentSummary>;
  startImport?: (paths: string[]) => Promise<ImportBatch>;
  resolveImportItem?: (
    itemId: string,
    decision: ImportDecision
  ) => Promise<ImportItemResult>;
  retryImportItem?: (itemId: string) => Promise<ImportItemResult>;
  documentPreviews?: Record<string, DocumentPreview>;
  documentThumbnails?: Record<string, DocumentThumbnail>;
  getDocumentPreview?: (documentId: string) => Promise<DocumentPreview>;
  getDocumentThumbnail?: (documentId: string) => Promise<DocumentThumbnail>;
  openDocument?: (documentId: string) => Promise<void>;
  documentContents?: Record<string, string>;
  indexFailures?: Record<string, string>;
  searchDocuments?: (
    request: DocumentSearchQuery
  ) => Promise<DocumentSearchResponse>;
  indexPendingDocuments?: () => Promise<IndexRunResult>;
  retryDocumentIndex?: (documentId: string) => Promise<DocumentSummary>;
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
    description: null,
    documentDate: null,
    fileName,
    fileType,
    fileSize: 0,
    contentHash: null,
    collectionId: "inbox",
    tags: [],
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

function snippetAround(text: string, query: string): string | null {
  const normalized = text.replace(/\s+/g, " ").trim();
  const index = normalized
    .toLocaleLowerCase()
    .indexOf(query.toLocaleLowerCase());
  if (index < 0) {
    return null;
  }
  const start = Math.max(0, index - 48);
  const end = Math.min(normalized.length, index + query.length + 72);
  return `${start > 0 ? "…" : ""}${normalized.slice(start, end)}${
    end < normalized.length ? "…" : ""
  }`;
}

export class FakeBackendClient implements BackendClient {
  calls: string[] = [];
  private state: BootstrapState;
  private documents: DocumentSummary[];
  private collections: CollectionSummary[];
  private tags: TagSummary[];
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
  private readonly documentPreviews: Record<string, DocumentPreview>;
  private readonly documentThumbnails: Record<string, DocumentThumbnail>;
  private readonly getDocumentPreviewImpl:
    | ((documentId: string) => Promise<DocumentPreview>)
    | null;
  private readonly getDocumentThumbnailImpl:
    | ((documentId: string) => Promise<DocumentThumbnail>)
    | null;
  private readonly openDocumentImpl:
    | ((documentId: string) => Promise<void>)
    | null;
  private readonly documentContents: Record<string, string>;
  private readonly indexFailures: Record<string, string>;
  private readonly searchDocumentsImpl:
    | ((
        request: DocumentSearchQuery
      ) => Promise<DocumentSearchResponse>)
    | null;
  private readonly indexPendingDocumentsImpl:
    | (() => Promise<IndexRunResult>)
    | null;
  private readonly retryDocumentIndexImpl:
    | ((documentId: string) => Promise<DocumentSummary>)
    | null;
  private fileDropHandlers = new Set<FileDropHandler>();
  private importProgressHandlers = new Set<ImportProgressHandler>();
  private importBatches = new Map<string, ImportBatch>();
  private nextCollectionId = 1;
  private nextTagId = 1;

  constructor(options: FakeBackendOptions = {}) {
    this.state = structuredClone(options.bootstrap ?? emptyBootstrap);
    this.documents = structuredClone(options.documents ?? []);
    this.collections = structuredClone(options.collections ?? [inbox]);
    this.tags = structuredClone(options.tags ?? []);
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
    this.documentPreviews = structuredClone(options.documentPreviews ?? {});
    this.documentThumbnails = structuredClone(
      options.documentThumbnails ?? {}
    );
    this.getDocumentPreviewImpl = options.getDocumentPreview ?? null;
    this.getDocumentThumbnailImpl = options.getDocumentThumbnail ?? null;
    this.openDocumentImpl = options.openDocument ?? null;
    this.documentContents = structuredClone(options.documentContents ?? {});
    this.indexFailures = structuredClone(options.indexFailures ?? {});
    this.searchDocumentsImpl = options.searchDocuments ?? null;
    this.indexPendingDocumentsImpl = options.indexPendingDocuments ?? null;
    this.retryDocumentIndexImpl = options.retryDocumentIndex ?? null;
    this.refreshCollectionCounts();
    this.refreshTagCounts();
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

  async searchDocuments(
    request: DocumentSearchQuery
  ): Promise<DocumentSearchResponse> {
    this.calls.push(`searchDocuments:${request.query}`);
    if (this.searchDocumentsImpl) {
      return this.searchDocumentsImpl(request);
    }

    const query = request.query.trim();
    const normalizedQuery = query.toLocaleLowerCase();
    const shortQuery = [...query].length <= 2;
    const filtered = this.documents.filter((document) => {
      const { filters } = request;
      if (
        filters.collectionId &&
        document.collectionId !== filters.collectionId
      ) {
        return false;
      }
      if (
        filters.tagId &&
        !document.tags.some((tag) => tag.id === filters.tagId)
      ) {
        return false;
      }
      if (
        filters.fileType &&
        document.fileType.toUpperCase() !== filters.fileType.toUpperCase()
      ) {
        return false;
      }
      if (
        filters.documentDateFrom &&
        (!document.documentDate ||
          document.documentDate < filters.documentDateFrom)
      ) {
        return false;
      }
      if (
        filters.documentDateTo &&
        (!document.documentDate ||
          document.documentDate > filters.documentDateTo)
      ) {
        return false;
      }
      return true;
    });

    const results: DocumentSearchResult[] = [];
    for (const document of filtered) {
      if (!query) {
        results.push({
          document,
          snippet: null,
          matchKind: "metadata"
        });
        continue;
      }

      const content = this.documentContents[document.id] ?? "";
      const contentSnippet = snippetAround(content, query);
      const collectionName =
        this.collections.find(
          (collection) => collection.id === document.collectionId
        )?.name ?? "";
      const metadata = [
        document.title,
        document.description ?? "",
        document.fileName,
        document.fileType,
        document.documentDate ?? "",
        document.sourcePath,
        collectionName,
        ...document.tags.map((tag) => tag.name)
      ]
        .join("\n")
        .toLocaleLowerCase();
      const metadataMatch = metadata.includes(normalizedQuery);

      if (shortQuery) {
        if (metadataMatch) {
          results.push({
            document,
            snippet: null,
            matchKind: "metadata"
          });
        }
      } else if (contentSnippet) {
        results.push({
          document,
          snippet: contentSnippet,
          matchKind: "content"
        });
      } else if (metadataMatch) {
        results.push({
          document,
          snippet: null,
          matchKind: "metadata"
        });
      }
    }

    return { results: structuredClone(results) };
  }

  async indexPendingDocuments(): Promise<IndexRunResult> {
    this.calls.push("indexPendingDocuments");
    if (this.indexPendingDocumentsImpl) {
      return this.indexPendingDocumentsImpl();
    }

    const result: IndexRunResult = {
      processed: 0,
      searchable: 0,
      failed: 0
    };
    for (const document of this.documents) {
      if (document.indexStatus !== "pending") {
        continue;
      }
      result.processed += 1;
      const failure = this.indexFailures[document.id];
      if (failure) {
        document.indexStatus = "failed";
        document.errorStage = "indexing";
        document.errorMessage = failure;
        result.failed += 1;
      } else {
        document.indexStatus = "searchable";
        document.errorStage = null;
        document.errorMessage = null;
        result.searchable += 1;
      }
    }
    return structuredClone(result);
  }

  async retryDocumentIndex(documentId: string): Promise<DocumentSummary> {
    this.calls.push(`retryDocumentIndex:${documentId}`);
    if (this.retryDocumentIndexImpl) {
      const updated = await this.retryDocumentIndexImpl(documentId);
      this.documents = this.documents.map((document) =>
        document.id === updated.id ? structuredClone(updated) : document
      );
      return structuredClone(updated);
    }

    const document = this.requireDocument(documentId);
    const failure = this.indexFailures[documentId];
    if (failure) {
      document.indexStatus = "failed";
      document.errorStage = "indexing";
      document.errorMessage = failure;
    } else {
      document.indexStatus = "searchable";
      document.errorStage = null;
      document.errorMessage = null;
    }
    return structuredClone(document);
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

  async getDocumentPreview(documentId: string): Promise<DocumentPreview> {
    this.calls.push(`getDocumentPreview:${documentId}`);
    if (this.getDocumentPreviewImpl) {
      return this.getDocumentPreviewImpl(documentId);
    }
    if (this.documentPreviews[documentId]) {
      return structuredClone(this.documentPreviews[documentId]);
    }

    const document = this.requireDocument(documentId);
    if (document.fileType === "PDF") {
      return {
        kind: "pdf",
        dataUrl: "data:application/pdf;base64,JVBERi0xLjQ=",
        pageCount: null
      };
    }
    if (document.fileType === "JPG" || document.fileType === "PNG") {
      return {
        kind: "image",
        dataUrl: "data:image/png;base64,iVBORw0KGgo="
      };
    }
    if (document.fileType === "DOCX") {
      return {
        kind: "docx",
        text: `${document.title} 的提取文本`,
        notice: "DOCX 预览仅显示提取文本，不是完整版式预览。"
      };
    }
    if (document.fileType === "TXT" || document.fileType === "Markdown") {
      return {
        kind: "text",
        text: `${document.title} 的只读预览文本`
      };
    }
    return {
      kind: "unsupported",
      message: `暂不支持预览 ${document.fileType} 格式。`
    };
  }

  async getDocumentThumbnail(documentId: string): Promise<DocumentThumbnail> {
    this.calls.push(`getDocumentThumbnail:${documentId}`);
    if (this.getDocumentThumbnailImpl) {
      return this.getDocumentThumbnailImpl(documentId);
    }
    if (this.documentThumbnails[documentId]) {
      return structuredClone(this.documentThumbnails[documentId]);
    }

    const document = this.requireDocument(documentId);
    if (document.fileType === "PDF") {
      return {
        kind: "pdf",
        dataUrl: "data:application/pdf;base64,JVBERi0xLjQ="
      };
    }
    if (document.fileType === "JPG" || document.fileType === "PNG") {
      return {
        kind: "image",
        dataUrl: "data:image/png;base64,iVBORw0KGgo="
      };
    }
    return {
      kind: "fallback",
      reason: `${document.fileType} 使用类型图标。`
    };
  }

  async openDocument(documentId: string): Promise<void> {
    this.calls.push(`openDocument:${documentId}`);
    if (this.openDocumentImpl) {
      await this.openDocumentImpl(documentId);
      return;
    }
    this.requireDocument(documentId);
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

  async listTags(): Promise<TagSummary[]> {
    this.calls.push("listTags");
    return structuredClone(this.tags);
  }

  async createTag(name: string): Promise<TagSummary> {
    this.calls.push(`createTag:${name}`);
    const trimmedName = name.trim();
    this.validateTagName(trimmedName);
    if (
      this.tags.some(
        (tag) => tag.name.toLocaleLowerCase() === trimmedName.toLocaleLowerCase()
      )
    ) {
      throw new BackendError({
        code: "tagAlreadyExists",
        message: "标签已存在。"
      });
    }

    const tag: TagSummary = {
      id: `tag-${this.nextTagId++}`,
      name: trimmedName,
      documentCount: 0
    };
    this.tags.push(tag);
    this.sortTags();
    return structuredClone(tag);
  }

  async renameTag(tagId: string, name: string): Promise<TagSummary> {
    this.calls.push(`renameTag:${tagId}:${name}`);
    const trimmedName = name.trim();
    this.validateTagName(trimmedName);
    const tag = this.requireTag(tagId);
    if (
      this.tags.some(
        (candidate) =>
          candidate.id !== tagId &&
          candidate.name.toLocaleLowerCase() ===
            trimmedName.toLocaleLowerCase()
      )
    ) {
      throw new BackendError({
        code: "tagAlreadyExists",
        message: "标签已存在。"
      });
    }

    tag.name = trimmedName;
    for (const document of this.documents) {
      document.tags = document.tags.map((candidate) =>
        candidate.id === tagId ? { ...candidate, name: trimmedName } : candidate
      );
    }
    this.sortTags();
    return structuredClone(tag);
  }

  async deleteTag(tagId: string): Promise<void> {
    this.calls.push(`deleteTag:${tagId}`);
    this.requireTag(tagId);
    this.tags = this.tags.filter((tag) => tag.id !== tagId);
    for (const document of this.documents) {
      document.tags = document.tags.filter((tag) => tag.id !== tagId);
    }
  }

  async addTagToDocument(
    documentId: string,
    tagId: string
  ): Promise<DocumentSummary> {
    this.calls.push(`addTagToDocument:${documentId}:${tagId}`);
    const document = this.requireDocument(documentId);
    const tag = this.requireTag(tagId);
    if (!document.tags.some((candidate) => candidate.id === tagId)) {
      document.tags.push({ ...tag, documentCount: 0 });
    }
    this.refreshTagCounts();
    return structuredClone(this.requireDocument(documentId));
  }

  async removeTagFromDocument(
    documentId: string,
    tagId: string
  ): Promise<DocumentSummary> {
    this.calls.push(`removeTagFromDocument:${documentId}:${tagId}`);
    const document = this.requireDocument(documentId);
    this.requireTag(tagId);
    document.tags = document.tags.filter((tag) => tag.id !== tagId);
    this.refreshTagCounts();
    return structuredClone(document);
  }

  async updateDocumentMetadata(
    documentId: string,
    update: DocumentMetadataUpdate
  ): Promise<DocumentSummary> {
    this.calls.push(
      `updateDocumentMetadata:${documentId}:${update.title}:${
        update.documentDate ?? "null"
      }:${update.collectionId}:${update.tagIds.join("|")}`
    );
    const title = update.title.trim();
    if (!title) {
      throw new BackendError({
        code: "invalidDocumentMetadata",
        message: "文档标题不能为空。"
      });
    }
    if (
      update.documentDate &&
      !/^\d{4}-\d{2}-\d{2}$/.test(update.documentDate)
    ) {
      throw new BackendError({
        code: "invalidDocumentMetadata",
        message: "文档日期必须使用 YYYY-MM-DD 格式。"
      });
    }
    const document = this.requireDocument(documentId);
    if (!this.collections.some((item) => item.id === update.collectionId)) {
      throw new BackendError({
        code: "collectionNotFound",
        message: "目标集合不存在。"
      });
    }
    const selectedTags = [...new Set(update.tagIds)].map((tagId) =>
      this.requireTag(tagId)
    );

    document.title = title;
    document.description = update.description?.trim() || null;
    document.documentDate = update.documentDate || null;
    document.collectionId = update.collectionId;
    document.tags = selectedTags.map((tag) => ({
      ...tag,
      documentCount: 0
    }));
    this.refreshCollectionCounts();
    this.refreshTagCounts();
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

  private requireDocument(documentId: string): DocumentSummary {
    const document = this.documents.find((item) => item.id === documentId);
    if (!document) {
      throw new BackendError({
        code: "documentNotFound",
        message: "文档不存在。"
      });
    }
    return document;
  }

  private requireTag(tagId: string): TagSummary {
    const tag = this.tags.find((item) => item.id === tagId);
    if (!tag) {
      throw new BackendError({
        code: "tagNotFound",
        message: "标签不存在。"
      });
    }
    return tag;
  }

  private validateTagName(name: string) {
    if (!name) {
      throw new BackendError({
        code: "invalidTag",
        message: "标签名称不能为空。"
      });
    }
    if ([...name].length > 50) {
      throw new BackendError({
        code: "invalidTag",
        message: "标签名称不能超过 50 个字符。"
      });
    }
  }

  private sortTags() {
    this.tags.sort((left, right) =>
      left.name.localeCompare(right.name, "zh-CN")
    );
  }

  private refreshTagCounts() {
    for (const tag of this.tags) {
      tag.documentCount = this.documents.filter((document) =>
        document.tags.some((candidate) => candidate.id === tag.id)
      ).length;
    }
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
