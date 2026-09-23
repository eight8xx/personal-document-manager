import { BackendError, toBackendError } from "./error";
import { sameLibraryIdentity } from "./libraryIdentity";
import {
  documentFormatCapabilities,
  documentFormatForPath,
  documentFormatForType,
  fileTypeForPath,
  unsupportedDocumentMessage
} from "./documentFormats";
import type {
  BackendClient,
  BatchDocumentItemResult,
  BatchDocumentOperationRequest,
  BatchDocumentOperationResult,
  BootstrapState,
  ClassificationPreviewRequest,
  ClassificationPreviewResponse,
  ClassificationRule,
  ClassificationRuleOperation,
  CollectionDeleteResult,
  CollectionSummary,
  DocumentIndexChangedEvent,
  DocumentIndexChangedHandler,
  DocumentFormatCapability,
  DocumentMetadataUpdate,
  DocumentPreview,
  DocumentSearchQuery,
  DocumentSearchResponse,
  DocumentSearchResult,
  DocumentSummary,
  DocumentThumbnail,
  EmptyTrashResult,
  FileDropHandler,
  ImportBatch,
  ImportDecision,
  ImportItemResult,
  ImportProgress,
  ImportProgressHandler,
  ImportSource,
  IndexRunResult,
  LibraryLocationInspection,
  LibrarySummary,
  RecentLibrary,
  ReceiveDirectoryListing,
  ReceiveDirectoryListingItem,
  ReceiveDirectoryOperation,
  ReceiveImportCompletedEvent,
  ReceiveImportCompletedHandler,
  ReceiveImportLogEntry,
  ReceiveSource,
  ReceiveSourceCandidate,
  ReceiveSourceCandidates,
  ReceiveSourceInput,
  ReceiveSourceKind,
  ReceiveSourceScanResult,
  TablePreviewRequest,
  TableSheet,
  TagSummary,
  TrashDocumentSummary
} from "./types";

export interface FakeBackendOptions {
  bootstrap?: BootstrapState;
  strictLibraryIdentity?: boolean;
  selectedDirectory?: string | null;
  selectedDocument?: string | null;
  selectedDocuments?: string[];
  selectedFolder?: string | null;
  documents?: DocumentSummary[];
  trashDocuments?: TrashDocumentSummary[];
  collections?: CollectionSummary[];
  tags?: TagSummary[];
  inspections?: Record<string, LibraryLocationInspection>;
  createLibrary?: (path: string) => Promise<LibrarySummary>;
  importDocument?: (path: string) => Promise<DocumentSummary>;
  startImport?: (
    paths: string[],
    targetCollectionId?: string | null,
    source?: ImportSource
  ) => Promise<ImportBatch>;
  resolveImportItem?: (
    itemId: string,
    decision: ImportDecision
  ) => Promise<ImportItemResult>;
  retryImportItem?: (itemId: string) => Promise<ImportItemResult>;
  documentPreviews?: Record<string, DocumentPreview>;
  documentThumbnails?: Record<string, DocumentThumbnail>;
  getDocumentPreview?: (
    documentId: string,
    page?: number
  ) => Promise<DocumentPreview>;
  getDocumentThumbnail?: (documentId: string) => Promise<DocumentThumbnail>;
  saveDocumentThumbnail?: (
    documentId: string,
    contentHash: string,
    thumbnailDataUrl: string
  ) => Promise<DocumentThumbnail>;
  openDocument?: (documentId: string) => Promise<void>;
  openExternalUrl?: (url: string) => Promise<void>;
  documentContents?: Record<string, string>;
  indexFailures?: Record<string, string>;
  searchDocuments?: (
    request: DocumentSearchQuery
  ) => Promise<DocumentSearchResponse>;
  indexPendingDocuments?: () => Promise<IndexRunResult>;
  retryDocumentIndex?: (documentId: string) => Promise<DocumentSummary>;
  batchOrganizeDocuments?: (
    request: BatchDocumentOperationRequest
  ) => Promise<BatchDocumentOperationResult>;
  cancelBatchDocumentOperation?: (jobId: string) => Promise<boolean>;
  /** 按 `${library.id}:${library.path}` 预置分类规则。 */
  classificationRules?: Record<string, ClassificationRule[]>;
  /** 按 `${library.id}:${library.path}` 预置接收来源。 */
  receiveSources?: Record<string, ReceiveSource[]>;
  /** 按 `${library.id}:${library.path}` 预置首次启用后的候选文件清单。 */
  receiveDirectoryFiles?: Record<string, ReceiveDirectoryListingItem[]>;
  /** 可注入的候选接收目录；未提供时微信给一个候选、QQ 给空列表。 */
  receiveSourceCandidates?: Partial<
    Record<ReceiveSourceKind, ReceiveSourceCandidate[]>
  >;
  receiveImportLog?: ReceiveImportLogEntry[];
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
  const fileType = fileTypeForPath(fileName) ?? fileName.toUpperCase();
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
  private trashDocuments: TrashDocumentSummary[];
  private collections: CollectionSummary[];
  private tags: TagSummary[];
  private readonly selectedDirectory: string | null;
  private readonly strictLibraryIdentity: boolean;
  private readonly selectedDocument: string | null;
  private readonly selectedDocuments: string[];
  private readonly selectedFolder: string | null;
  private readonly inspections: Record<string, LibraryLocationInspection>;
  private readonly createLibraryImpl: (path: string) => Promise<LibrarySummary>;
  private readonly importDocumentImpl:
    | ((path: string) => Promise<DocumentSummary>)
    | null;
  private readonly startImportImpl:
    | ((
        paths: string[],
        targetCollectionId?: string | null,
        source?: ImportSource
      ) => Promise<ImportBatch>)
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
    | ((documentId: string, page?: number) => Promise<DocumentPreview>)
    | null;
  private readonly getDocumentThumbnailImpl:
    | ((documentId: string) => Promise<DocumentThumbnail>)
    | null;
  private readonly saveDocumentThumbnailImpl:
    | ((
        documentId: string,
        contentHash: string,
        thumbnailDataUrl: string
      ) => Promise<DocumentThumbnail>)
    | null;
  private readonly openDocumentImpl:
    | ((documentId: string) => Promise<void>)
    | null;
  private readonly openExternalUrlImpl:
    | ((url: string) => Promise<void>)
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
  private readonly batchOrganizeDocumentsImpl:
    | ((
        request: BatchDocumentOperationRequest
      ) => Promise<BatchDocumentOperationResult>)
    | null;
  private readonly cancelBatchDocumentOperationImpl:
    | ((jobId: string) => Promise<boolean>)
    | null;
  private fileDropHandlers = new Set<FileDropHandler>();
  private importProgressHandlers = new Set<ImportProgressHandler>();
  private documentIndexChangedHandlers =
    new Set<DocumentIndexChangedHandler>();
  private importBatches = new Map<string, ImportBatch>();
  private activeBatchJobs = new Set<string>();
  private cancelledBatchJobs = new Set<string>();
  private nextCollectionId = 1;
  private nextTagId = 1;
  private classificationRules = new Map<string, ClassificationRule[]>();
  private nextClassificationRuleId = 1;
  private receiveSources = new Map<string, ReceiveSource[]>();
  private receiveDirectoryFiles = new Map<
    string,
    ReceiveDirectoryListingItem[]
  >();
  private receiveScanResults: ReceiveSourceScanResult[] = [];
  private receiveImportLog: ReceiveImportLogEntry[] = [];
  private nextReceiveSourceId = 1;
  private receiveImportCompletedHandlers = new Set<ReceiveImportCompletedHandler>();
  /** 每次 startImport 的分类开关：null 表示调用方未传（按应用规则处理）。 */
  readonly importClassificationDecisions: (boolean | null)[] = [];
  /** 测试可注入的候选目录；默认只给微信一个候选，QQ 留空以覆盖「无候选」路径。 */
  private readonly receiveSourceCandidates: Partial<
    Record<ReceiveSourceKind, ReceiveSourceCandidate[]>
  > = {};

  constructor(options: FakeBackendOptions = {}) {
    this.state = structuredClone(options.bootstrap ?? emptyBootstrap);
    this.strictLibraryIdentity = options.strictLibraryIdentity ?? false;
    this.classificationRules = new Map(
      Object.entries(options.classificationRules ?? {}).map(
        ([key, rules]) => [key, structuredClone(rules)] as const
      )
    );
    this.receiveSources = new Map(
      Object.entries(options.receiveSources ?? {}).map(
        ([key, sources]) => [key, structuredClone(sources)] as const
      )
    );
    this.receiveDirectoryFiles = new Map(
      Object.entries(options.receiveDirectoryFiles ?? {}).map(
        ([key, items]) => [key, structuredClone(items)] as const
      )
    );
    this.receiveSourceCandidates = structuredClone(
      options.receiveSourceCandidates ?? {}
    );
    this.receiveImportLog = structuredClone(options.receiveImportLog ?? []);
    this.documents = structuredClone(options.documents ?? []);
    this.trashDocuments = structuredClone(options.trashDocuments ?? []);
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
    this.saveDocumentThumbnailImpl =
      options.saveDocumentThumbnail ?? null;
    this.openDocumentImpl = options.openDocument ?? null;
    this.openExternalUrlImpl = options.openExternalUrl ?? null;
    this.documentContents = structuredClone(options.documentContents ?? {});
    this.indexFailures = structuredClone(options.indexFailures ?? {});
    this.searchDocumentsImpl = options.searchDocuments ?? null;
    this.indexPendingDocumentsImpl = options.indexPendingDocuments ?? null;
    this.retryDocumentIndexImpl = options.retryDocumentIndex ?? null;
    this.batchOrganizeDocumentsImpl =
      options.batchOrganizeDocuments ?? null;
    this.cancelBatchDocumentOperationImpl =
      options.cancelBatchDocumentOperation ?? null;
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

  async importDocument(
    _library: LibrarySummary,
    path: string
  ): Promise<DocumentSummary> {
    this.assertCurrentLibrary(_library);
    this.calls.push(`import:${path}`);
    if (this.importDocumentImpl) {
      const document = await this.importDocumentImpl(path);
      this.documents = [
        document,
        ...this.documents.filter((item) => item.id !== document.id)
      ];
      return document;
    }

    if (!documentFormatForPath(path)) {
      throw new BackendError({
        code: "unsupportedFile",
        message: unsupportedDocumentMessage()
      });
    }

    const document = defaultDocument(path);
    this.documents = [document, ...this.documents];
    return document;
  }

  async startImport(
    _library: LibrarySummary,
    paths: string[],
    targetCollectionId: string | null = null,
    source: ImportSource = "filePicker",
    applyClassification?: boolean
  ): Promise<ImportBatch> {
    this.assertCurrentLibrary(_library);
    // 记录分类开关，供测试断言「不应用规则」分支确实传到了后端。
    this.importClassificationDecisions.push(
      applyClassification === undefined ? null : applyClassification
    );
    this.calls.push(
      `startImport:${paths.join("|")}${
        targetCollectionId ? `:${targetCollectionId}` : ""
      }${source === "collectionDrop" ? `:${source}` : ""}${
        applyClassification === undefined ? "" : `:classify=${applyClassification}`
      }`
    );
    if (this.startImportImpl) {
      const resolved = await this.startImportImpl(
        paths,
        targetCollectionId,
        source
      );
      const batch = {
        ...resolved,
        targetCollectionId:
          resolved.targetCollectionId ?? targetCollectionId,
        items: resolved.items.map((item) => ({
          ...item,
          targetCollectionId: item.targetCollectionId ?? targetCollectionId,
          collectionId: item.collectionId ?? null,
          notice: item.notice ?? null
        }))
      };
      this.importBatches.set(batch.batchId, structuredClone(batch));
      return structuredClone(batch);
    }

    const batchId = `batch-${this.importBatches.size + 1}`;
    const target = targetCollectionId
      ? this.collections.find(
          (collection) => collection.id === targetCollectionId
        )
      : this.collections.find((collection) => collection.isInbox);
    const targetNotice = targetCollectionId && !target
      ? "目标集合已删除，文档已改为导入收件箱。"
      : null;
    const collectionId = target?.id ?? "inbox";
    const items: ImportItemResult[] = [];
    this.emitImportProgress({
      library: _library,
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
        const document = await this.importDocument(_library, path);
        document.collectionId = collectionId;
        this.refreshCollectionCounts();
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
          retryable: false,
          targetCollectionId: targetCollectionId ?? null,
          collectionId: document.collectionId,
          notice: targetNotice
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
          retryable: true,
          targetCollectionId: targetCollectionId ?? null,
          collectionId: null,
          notice: null
        };
      }

      items.push(item);
      this.emitImportProgress({
        library: _library,
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
      ...countsForItems(items),
      targetCollectionId: targetCollectionId ?? null
    };
    this.importBatches.set(batchId, structuredClone(batch));
    return structuredClone(batch);
  }

  async resolveImportItem(
    _library: LibrarySummary,
    itemId: string,
    decision: ImportDecision
  ): Promise<ImportItemResult> {
    this.assertCurrentLibrary(_library);
    this.calls.push(`resolveImportItem:${itemId}:${decision}`);
    if (this.resolveImportItemImpl) {
      const resolved = await this.resolveImportItemImpl(itemId, decision);
      const stored = this.findStoredImportItem(itemId);
      const item = {
        ...stored,
        ...resolved,
        targetCollectionId:
          resolved.targetCollectionId ?? stored.targetCollectionId,
        collectionId: resolved.collectionId ?? stored.collectionId,
        notice: resolved.notice ?? stored.notice
      };
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
      retryable: false,
      targetCollectionId: stored.targetCollectionId,
      collectionId: stored.collectionId,
      notice: stored.notice
    };
    this.replaceStoredImportItem(item);
    return structuredClone(item);
  }

  async retryImportItem(
    _library: LibrarySummary,
    itemId: string
  ): Promise<ImportItemResult> {
    this.assertCurrentLibrary(_library);
    this.calls.push(`retryImportItem:${itemId}`);
    if (this.retryImportItemImpl) {
      const resolved = await this.retryImportItemImpl(itemId);
      const stored = this.findStoredImportItem(itemId);
      const item = {
        ...stored,
        ...resolved,
        targetCollectionId:
          resolved.targetCollectionId ?? stored.targetCollectionId,
        collectionId: resolved.collectionId ?? stored.collectionId,
        notice: resolved.notice ?? stored.notice
      };
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
      retryable: false,
      targetCollectionId: stored.targetCollectionId,
      collectionId: stored.collectionId,
      notice: stored.notice
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

  async pendingIndexCount(): Promise<number> {
    this.calls.push("pendingIndexCount");
    return this.documents.filter(
      (document) => document.indexStatus === "pending"
    ).length;
  }

  async indexPendingDocuments(_library: LibrarySummary): Promise<IndexRunResult> {
    this.assertCurrentLibrary(_library);
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

  async retryDocumentIndex(
    _library: LibrarySummary,
    documentId: string
  ): Promise<DocumentSummary> {
    this.assertCurrentLibrary(_library);
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

  emitFileDrop(
    paths: string[],
    position: { x: number; y: number } | null = null,
    type: "enter" | "over" | "drop" = "drop"
  ) {
    for (const handler of this.fileDropHandlers) {
      handler({ type, paths, position });
    }
  }

  async subscribeToDocumentIndexChanges(
    handler: DocumentIndexChangedHandler
  ): Promise<() => void> {
    this.calls.push("subscribeToDocumentIndexChanges");
    this.documentIndexChangedHandlers.add(handler);
    return () => {
      this.documentIndexChangedHandlers.delete(handler);
    };
  }

  emitDocumentIndexChanged(event: DocumentIndexChangedEvent) {
    for (const handler of this.documentIndexChangedHandlers) {
      handler(structuredClone(event));
    }
  }

  setDocument(document: DocumentSummary) {
    this.documents = this.documents.map((candidate) =>
      candidate.id === document.id ? structuredClone(document) : candidate
    );
  }

  async getDocumentPreview(
    _library: LibrarySummary,
    documentId: string,
    page?: number
  ): Promise<DocumentPreview> {
    this.assertCurrentLibrary(_library);
    this.calls.push(`getDocumentPreview:${documentId}`);
    if (this.getDocumentPreviewImpl) {
      return this.getDocumentPreviewImpl(documentId, page);
    }
    if (this.documentPreviews[documentId]) {
      return structuredClone(this.documentPreviews[documentId]);
    }

    const document = this.requireDocument(documentId);
    const capability = documentFormatForType(document.fileType);
    if (!capability) {
      return {
        kind: "unsupported",
        message: `暂不支持预览 ${document.fileType} 格式。`
      };
    }
    if (capability.preview === "pdfPages") {
      return {
        kind: "pdf",
        dataUrl: "data:image/png;base64,iVBORw0KGgo=",
        pageCount: 3,
        page: page ?? 1
      };
    }
    if (capability.preview === "localImage") {
      return {
        kind: "image",
        dataUrl: "data:image/png;base64,iVBORw0KGgo="
      };
    }
    if (capability.preview === "docxLayout") {
      return {
        kind: "docx",
        dataUrl:
          "data:application/vnd.openxmlformats-officedocument.wordprocessingml.document;base64,UEsFBgAAAAAAAAAAAAAAAAAAAAAAAA==",
        text: `${document.title} 的提取文本`,
        notice: "DOCX 版式预览为本地只读近似呈现。",
        degradedFeatures: []
      };
    }
    if (capability.preview === "pptxPages") {
      return {
        kind: "pptx",
        dataUrl:
          "data:application/vnd.openxmlformats-officedocument.presentationml.presentation;base64,UEsFBgAAAAAAAAAAAAAAAAAAAAAAAA==",
        text: `${document.title} 的提取文本`,
        notice: "PPTX 版式预览为本地只读近似呈现。",
        degradedFeatures: []
      };
    }
    if (capability.preview === "safeMarkdown") {
      return {
        kind: "markdown",
        text: `${document.title} 的只读预览文本`
      };
    }
    if (capability.preview === "plainText") {
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

  async getDocumentThumbnail(
    _library: LibrarySummary,
    documentId: string
  ): Promise<DocumentThumbnail> {
    this.assertCurrentLibrary(_library);
    this.calls.push(`getDocumentThumbnail:${documentId}`);
    if (this.getDocumentThumbnailImpl) {
      return this.getDocumentThumbnailImpl(documentId);
    }
    if (this.documentThumbnails[documentId]) {
      return structuredClone(this.documentThumbnails[documentId]);
    }

    const document = this.requireDocument(documentId);
    const capability = documentFormatForType(document.fileType);
    if (capability?.thumbnail === "pdfFirstPage") {
      return {
        kind: "pdf",
        dataUrl:
          "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="
      };
    }
    if (capability?.thumbnail === "localImage") {
      return {
        kind: "image",
        dataUrl:
          "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="
      };
    }
    if (capability?.thumbnail === "pptxFirstPage") {
      return {
        kind: "fallback",
        reason: "PPTX 缩略图尚未生成。"
      };
    }
    return {
      kind: "fallback",
      reason: `${document.fileType} 使用类型图标。`
    };
  }

  async saveDocumentThumbnail(
    _library: LibrarySummary,
    documentId: string,
    contentHash: string,
    thumbnailDataUrl: string
  ): Promise<DocumentThumbnail> {
    this.assertCurrentLibrary(_library);
    this.calls.push(`saveDocumentThumbnail:${documentId}:${contentHash}`);
    if (this.saveDocumentThumbnailImpl) {
      const thumbnail = await this.saveDocumentThumbnailImpl(
        documentId,
        contentHash,
        thumbnailDataUrl
      );
      this.documentThumbnails[documentId] = structuredClone(thumbnail);
      return structuredClone(thumbnail);
    }

    const document = this.requireDocument(documentId);
    if (document.contentHash !== contentHash) {
      throw new BackendError({
        code: "staleThumbnail",
        message: "缩略图已过期：文档内容版本已变化，已拒绝写入缓存。"
      });
    }
    if (!/^data:image\/(?:png|jpeg|jpg);base64,/i.test(thumbnailDataUrl)) {
      throw new BackendError({
        code: "preview",
        message: "缩略图必须是 PNG 或 JPEG 图片。"
      });
    }
    const thumbnail: DocumentThumbnail = {
      kind: "pptx",
      dataUrl: thumbnailDataUrl
    };
    this.documentThumbnails[documentId] = structuredClone(thumbnail);
    return structuredClone(thumbnail);
  }

  /** 表格文档的默认工作表清单：CSV 一张合成表，XLSX 两张表。 */
  async listDocumentSheets(
    _library: LibrarySummary,
    documentId: string
  ): Promise<TableSheet[]> {
    this.assertCurrentLibrary(_library);
    this.calls.push(`listDocumentSheets:${documentId}`);
    const document = this.requireDocument(documentId);
    const capability = documentFormatForType(document.fileType);
    if (capability?.preview !== "tablePaged") {
      throw new BackendError({
        code: "preview",
        message: `${document.fileType} 不是表格文档。`
      });
    }
    if (capability.id === "csv") {
      return [{ index: 0, name: "CSV", rowCount: null, columnCount: null }];
    }
    return [
      { index: 0, name: "汇总", rowCount: 12, columnCount: 4 },
      { index: 1, name: "明细", rowCount: 200, columnCount: 6 }
    ];
  }

  /** 表格文档的默认分页预览：按请求范围生成确定性单元格。 */
  async getTablePreview(
    _library: LibrarySummary,
    documentId: string,
    request: TablePreviewRequest = {}
  ): Promise<Extract<DocumentPreview, { kind: "table" }>> {
    this.assertCurrentLibrary(_library);
    this.calls.push(`getTablePreview:${documentId}`);
    const document = this.requireDocument(documentId);
    const capability = documentFormatForType(document.fileType);
    if (capability?.preview !== "tablePaged") {
      throw new BackendError({
        code: "preview",
        message: `${document.fileType} 不是表格文档。`
      });
    }

    const sheets = await this.listDocumentSheets(_library, documentId);
    const sheetIndex = request.sheetIndex ?? 0;
    const sheet = sheets.find((candidate) => candidate.index === sheetIndex);
    if (!sheet) {
      throw new BackendError({
        code: "preview",
        message: `工作表 ${sheetIndex} 不存在。`
      });
    }

    const startRow = Math.max(0, request.startRow ?? 0);
    const rowCount = Math.max(1, request.rowCount ?? 50);
    const columnCount = Math.max(1, request.columnCount ?? 8);
    const totalRows = sheet.rowCount ?? 120;
    const visibleRows = Math.max(0, Math.min(rowCount, totalRows - startRow));
    const cells = Array.from({ length: visibleRows }, (_, offset) =>
      Array.from({ length: columnCount }, (_, column) =>
        `R${startRow + offset + 1}C${column + 1}`
      )
    );
    // 与真实后端一致：行号与单元格一一对应，连续范围内即连续行号。
    const rowNumbers = Array.from(
      { length: visibleRows },
      (_, offset) => startRow + offset
    );

    return {
      kind: "table",
      sheets,
      sheetIndex,
      startRow,
      cells,
      rowNumbers,
      columnCount,
      hasMoreRows: startRow + visibleRows < totalRows,
      degradedFeatures: [],
      notice: null
    };
  }

  async openDocument(
    _library: LibrarySummary,
    documentId: string
  ): Promise<void> {
    this.assertCurrentLibrary(_library);
    this.calls.push(`openDocument:${documentId}`);
    if (this.openDocumentImpl) {
      await this.openDocumentImpl(documentId);
      return;
    }
    this.requireDocument(documentId);
  }

  async openExternalUrl(url: string): Promise<void> {
    this.calls.push(`openExternalUrl:${url}`);
    if (this.openExternalUrlImpl) {
      await this.openExternalUrlImpl(url);
      return;
    }
    if (!/^https?:\/\//i.test(url)) {
      throw new BackendError({
        code: "unsafeUrl",
        message: "只允许打开 HTTP 或 HTTPS 链接。"
      });
    }
  }

  async listDocumentFormatCapabilities(): Promise<
    DocumentFormatCapability[]
  > {
    this.calls.push("listDocumentFormatCapabilities");
    return structuredClone(documentFormatCapabilities);
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
    _library: LibrarySummary,
    name: string,
    parentId: string | null
  ): Promise<CollectionSummary> {
    this.assertCurrentLibrary(_library);
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
    _library: LibrarySummary,
    collectionId: string,
    name: string
  ): Promise<CollectionSummary> {
    this.assertCurrentLibrary(_library);
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
    _library: LibrarySummary,
    collectionId: string,
    parentId: string | null
  ): Promise<CollectionSummary> {
    this.assertCurrentLibrary(_library);
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
    _library: LibrarySummary,
    collectionId: string
  ): Promise<CollectionDeleteResult> {
    this.assertCurrentLibrary(_library);
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
    _library: LibrarySummary,
    documentId: string,
    collectionId: string
  ): Promise<DocumentSummary> {
    this.assertCurrentLibrary(_library);
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

  async moveDocumentToTrash(
    _library: LibrarySummary,
    documentId: string
  ): Promise<void> {
    this.assertCurrentLibrary(_library);
    this.calls.push(`moveDocumentToTrash:${documentId}`);
    const document = this.requireDocument(documentId);
    this.documents = this.documents.filter((item) => item.id !== documentId);
    this.trashDocuments = [
      {
        document: structuredClone(document),
        originalCollectionId: document.collectionId,
        originalCollectionName:
          this.collections.find(
            (collection) => collection.id === document.collectionId
          )?.name ?? null,
        deletedAt: "2026-09-13T09:00:00Z"
      },
      ...this.trashDocuments.filter(
        (item) => item.document.id !== documentId
      )
    ];
    this.refreshCollectionCounts();
    this.refreshTagCounts();
  }

  async listTrashDocuments(): Promise<TrashDocumentSummary[]> {
    this.calls.push("listTrashDocuments");
    return structuredClone(this.trashDocuments);
  }

  async restoreDocument(
    _library: LibrarySummary,
    documentId: string
  ): Promise<DocumentSummary> {
    this.assertCurrentLibrary(_library);
    this.calls.push(`restoreDocument:${documentId}`);
    const trashDocument = this.trashDocuments.find(
      (item) => item.document.id === documentId
    );
    if (!trashDocument) {
      throw new BackendError({
        code: "documentNotFound",
        message: "回收站中不存在文档。"
      });
    }
    const collectionId = this.collections.some(
      (collection) => collection.id === trashDocument.originalCollectionId
    )
      ? trashDocument.originalCollectionId!
      : "inbox";
    const restored = {
      ...structuredClone(trashDocument.document),
      collectionId
    };
    this.trashDocuments = this.trashDocuments.filter(
      (item) => item.document.id !== documentId
    );
    this.documents = [restored, ...this.documents];
    this.refreshCollectionCounts();
    this.refreshTagCounts();
    return structuredClone(restored);
  }

  async permanentlyDeleteDocument(
    _library: LibrarySummary,
    documentId: string
  ): Promise<void> {
    this.assertCurrentLibrary(_library);
    this.calls.push(`permanentlyDeleteDocument:${documentId}`);
    if (
      !this.trashDocuments.some((item) => item.document.id === documentId)
    ) {
      throw new BackendError({
        code: "documentNotFound",
        message: "回收站中不存在文档。"
      });
    }
    this.trashDocuments = this.trashDocuments.filter(
      (item) => item.document.id !== documentId
    );
    this.refreshTagCounts();
  }

  async emptyTrash(_library: LibrarySummary): Promise<EmptyTrashResult> {
    this.assertCurrentLibrary(_library);
    this.calls.push("emptyTrash");
    const items = this.trashDocuments.map(({ document }) => ({
      documentId: document.id,
      fileName: document.fileName,
      status: "succeeded" as const,
      errorCode: null,
      errorMessage: null
    }));
    this.trashDocuments = [];
    this.refreshTagCounts();
    return {
      deletedCount: items.length,
      failedCount: 0,
      items
    };
  }

  async listTags(): Promise<TagSummary[]> {
    this.calls.push("listTags");
    return structuredClone(this.tags);
  }

  async createTag(_library: LibrarySummary, name: string): Promise<TagSummary> {
    this.assertCurrentLibrary(_library);
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

  async renameTag(
    _library: LibrarySummary,
    tagId: string,
    name: string
  ): Promise<TagSummary> {
    this.assertCurrentLibrary(_library);
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

  async deleteTag(_library: LibrarySummary, tagId: string): Promise<void> {
    this.assertCurrentLibrary(_library);
    this.calls.push(`deleteTag:${tagId}`);
    this.requireTag(tagId);
    this.tags = this.tags.filter((tag) => tag.id !== tagId);
    for (const document of this.documents) {
      document.tags = document.tags.filter((tag) => tag.id !== tagId);
    }
  }

  async addTagToDocument(
    _library: LibrarySummary,
    documentId: string,
    tagId: string
  ): Promise<DocumentSummary> {
    this.assertCurrentLibrary(_library);
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
    _library: LibrarySummary,
    documentId: string,
    tagId: string
  ): Promise<DocumentSummary> {
    this.assertCurrentLibrary(_library);
    this.calls.push(`removeTagFromDocument:${documentId}:${tagId}`);
    const document = this.requireDocument(documentId);
    this.requireTag(tagId);
    document.tags = document.tags.filter((tag) => tag.id !== tagId);
    this.refreshTagCounts();
    return structuredClone(document);
  }

  async updateDocumentMetadata(
    _library: LibrarySummary,
    documentId: string,
    update: DocumentMetadataUpdate
  ): Promise<DocumentSummary> {
    this.assertCurrentLibrary(_library);
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

  async batchOrganizeDocuments(
    library: LibrarySummary,
    request: BatchDocumentOperationRequest
  ): Promise<BatchDocumentOperationResult> {
    this.assertCurrentLibrary(library);
    this.calls.push(
      `batchOrganizeDocuments:${request.jobId}:${request.operation.kind}`
    );
    if (this.batchOrganizeDocumentsImpl) {
      return this.batchOrganizeDocumentsImpl(request);
    }

    this.activeBatchJobs.add(request.jobId);
    const documentIds = [...new Set(request.documentIds)];
    const results: BatchDocumentItemResult[] = [];
    try {
      for (const documentId of documentIds) {
        if (this.cancelledBatchJobs.has(request.jobId)) {
          results.push({
            documentId,
            status: "cancelled",
            errorCode: null,
            errorMessage: null
          });
          continue;
        }

        try {
          switch (request.operation.kind) {
            case "moveToCollection":
              await this.moveDocumentToCollection(
                library,
                documentId,
                request.operation.collectionId
              );
              break;
            case "addTag":
              await this.addTagToDocument(
                library,
                documentId,
                request.operation.tagId
              );
              break;
            case "removeTag":
              await this.removeTagFromDocument(
                library,
                documentId,
                request.operation.tagId
              );
              break;
            case "moveToTrash":
              await this.moveDocumentToTrash(library, documentId);
              break;
          }
          results.push({
            documentId,
            status: "succeeded",
            errorCode: null,
            errorMessage: null
          });
        } catch (caught) {
          const error = toBackendError(caught);
          results.push({
            documentId,
            status: "failed",
            errorCode: error.code,
            errorMessage: error.message
          });
        }
      }
    } finally {
      this.activeBatchJobs.delete(request.jobId);
      this.cancelledBatchJobs.delete(request.jobId);
    }

    return {
      jobId: request.jobId,
      operation: structuredClone(request.operation),
      results,
      succeededCount: results.filter(
        (result) => result.status === "succeeded"
      ).length,
      failedCount: results.filter((result) => result.status === "failed")
        .length,
      cancelledCount: results.filter(
        (result) => result.status === "cancelled"
      ).length
    };
  }

  async cancelBatchDocumentOperation(jobId: string): Promise<boolean> {
    this.calls.push(`cancelBatchDocumentOperation:${jobId}`);
    if (this.cancelBatchDocumentOperationImpl) {
      return this.cancelBatchDocumentOperationImpl(jobId);
    }
    if (!this.activeBatchJobs.has(jobId)) {
      return false;
    }
    this.cancelledBatchJobs.add(jobId);
    return true;
  }

  /** 规则与来源按资料库隔离保存，与真实后端一致。 */
  private libraryKey(library: LibrarySummary) {
    return `${library.id}:${library.path}`;
  }

  async listClassificationRules(
    library: LibrarySummary
  ): Promise<ClassificationRule[]> {
    this.assertCurrentLibrary(library);
    this.calls.push("listClassificationRules");
    return structuredClone(
      this.classificationRules.get(this.libraryKey(library)) ?? []
    );
  }

  async classificationRuleOperation(
    library: LibrarySummary,
    operation: ClassificationRuleOperation
  ): Promise<ClassificationRule[]> {
    this.assertCurrentLibrary(library);
    this.calls.push(`classificationRuleOperation:${operation.kind}`);
    const key = this.libraryKey(library);
    const current = this.classificationRules.get(key) ?? [];
    let next = [...current];

    if (operation.kind === "create") {
      const rule: ClassificationRule = {
        id: `rule-${this.nextClassificationRuleId++}`,
        name: operation.rule.name,
        enabled: operation.rule.enabled,
        position: next.length + 1,
        fileNamePattern: operation.rule.fileNamePattern,
        fileType: operation.rule.fileType,
        sourceDirectory: operation.rule.sourceDirectory,
        collectionId: operation.rule.collectionId,
        tagIds: [...operation.rule.tagIds]
      };
      next.push(rule);
    } else if (operation.kind === "update") {
      next = next.map((rule) =>
        rule.id === operation.rule.id
          ? {
              ...rule,
              name: operation.rule.name,
              enabled: operation.rule.enabled,
              fileNamePattern: operation.rule.fileNamePattern,
              fileType: operation.rule.fileType,
              sourceDirectory: operation.rule.sourceDirectory,
              collectionId: operation.rule.collectionId,
              tagIds: [...operation.rule.tagIds]
            }
          : rule
      );
    } else if (operation.kind === "delete") {
      next = next.filter((rule) => rule.id !== operation.ruleId);
    } else if (operation.kind === "setEnabled") {
      next = next.map((rule) =>
        rule.id === operation.ruleId
          ? { ...rule, enabled: operation.enabled }
          : rule
      );
    } else {
      const byId = new Map(next.map((rule) => [rule.id, rule]));
      const ordered = operation.orderedRuleIds
        .map((ruleId) => byId.get(ruleId))
        .filter((rule): rule is ClassificationRule => Boolean(rule));
      const rest = next.filter(
        (rule) => !operation.orderedRuleIds.includes(rule.id)
      );
      next = [...ordered, ...rest];
    }

    next = next.map((rule, index) => ({ ...rule, position: index + 1 }));
    this.classificationRules.set(key, next);
    return structuredClone(next);
  }

  async previewClassification(
    library: LibrarySummary,
    request: ClassificationPreviewRequest
  ): Promise<ClassificationPreviewResponse> {
    this.assertCurrentLibrary(library);
    this.calls.push(`previewClassification:${request.paths.length}`);
    const rules = (
      this.classificationRules.get(this.libraryKey(library)) ?? []
    ).filter((rule) => rule.enabled);
    const inbox = this.collections.find((collection) => collection.isInbox);

    const items = request.paths.map((sourcePath) => {
      const fileName = sourcePath.split(/[\\/]/).at(-1) ?? sourcePath;
      const fileType = fileTypeForPath(sourcePath);
      const matched = rules.filter((rule) => {
        const directoryMatches =
          !rule.sourceDirectory ||
          sourcePath.toLocaleLowerCase().startsWith(
            rule.sourceDirectory.toLocaleLowerCase()
          );
        const typeMatches = !rule.fileType || rule.fileType === fileType;
        const nameMatches =
          !rule.fileNamePattern ||
          fileName
            .toLocaleLowerCase()
            .includes(rule.fileNamePattern.toLocaleLowerCase());
        return directoryMatches && typeMatches && nameMatches;
      });
      const collectionRule = matched.find((rule) => rule.collectionId);
      const collectionId =
        request.targetCollectionId ??
        collectionRule?.collectionId ??
        inbox?.id ??
        "inbox";
      const tagIds = [
        ...new Set(matched.flatMap((rule) => rule.tagIds))
      ];
      return {
        sourcePath,
        fileName,
        fileType,
        collectionId,
        tagIds,
        matchedRuleIds: matched.map((rule) => rule.id)
      };
    });

    return { items };
  }

  async listReceiveSources(library: LibrarySummary): Promise<ReceiveSource[]> {
    this.assertCurrentLibrary(library);
    this.calls.push("listReceiveSources");
    return structuredClone(this.receiveSources.get(this.libraryKey(library)) ?? []);
  }

  async listReceiveSourceCandidates(
    library: LibrarySummary,
    kind: ReceiveSourceKind
  ): Promise<ReceiveSourceCandidates> {
    this.assertCurrentLibrary(library);
    this.calls.push(`listReceiveSourceCandidates:${kind}`);
    const candidates =
      this.receiveSourceCandidates[kind] ??
      (kind === "wechat"
        ? [{ path: "C:\\Users\\User\\Documents\\WeChat Files", evidence: "微信默认文档目录" }]
        : []);
    return { kind, candidates: structuredClone(candidates) };
  }

  async upsertReceiveSource(
    library: LibrarySummary,
    sourceId: string | null,
    input: ReceiveSourceInput
  ): Promise<ReceiveSource[]> {
    this.assertCurrentLibrary(library);
    this.calls.push(`upsertReceiveSource:${sourceId ?? "new"}`);
    const key = this.libraryKey(library);
    const current = this.receiveSources.get(key) ?? [];
    const existing = current.find((source) => source.id === sourceId);
    const source: ReceiveSource = {
      id: existing?.id ?? `source-${this.nextReceiveSourceId++}`,
      kind: input.kind,
      displayName: input.displayName,
      path: input.path,
      enabled: input.enabled,
      status: existing?.status === "unreadable" ? "unreadable" : "ready",
      statusMessage: null,
      pendingCount: existing?.pendingCount ?? 0,
      lastScannedAt: existing?.lastScannedAt ?? null
    };
    const next = existing
      ? current.map((candidate) =>
          candidate.id === source.id ? source : candidate
        )
      : [...current, source];
    this.receiveSources.set(key, next);
    return structuredClone(next);
  }

  async removeReceiveSource(
    library: LibrarySummary,
    sourceId: string
  ): Promise<ReceiveSource[]> {
    this.assertCurrentLibrary(library);
    this.calls.push(`removeReceiveSource:${sourceId}`);
    const key = this.libraryKey(library);
    const next = (this.receiveSources.get(key) ?? []).filter(
      (source) => source.id !== sourceId
    );
    this.receiveSources.set(key, next);
    return structuredClone(next);
  }

  async listReceiveDirectoryFiles(
    library: LibrarySummary,
    sourceId: string
  ): Promise<ReceiveDirectoryListing> {
    this.assertCurrentLibrary(library);
    this.calls.push(`listReceiveDirectoryFiles:${sourceId}`);
    const source = (this.receiveSources.get(this.libraryKey(library)) ?? []).find(
      (candidate) => candidate.id === sourceId
    );
    if (!source) {
      throw new BackendError({
        code: "receiveSourceNotFound",
        message: "接收来源不存在。"
      });
    }
    return {
      sourceId,
      path: source.path ?? "",
      items: structuredClone(
        this.receiveDirectoryFiles.get(this.libraryKey(library)) ?? []
      )
    };
  }

  async applyReceiveDirectorySelection(
    library: LibrarySummary,
    operation: ReceiveDirectoryOperation
  ): Promise<ReceiveSourceScanResult> {
    this.assertCurrentLibrary(library);
    this.calls.push(
      `applyReceiveDirectorySelection:${operation.sourceId}:${operation.paths.length}`
    );
    const key = this.libraryKey(library);
    const selected = new Set(operation.paths);
    const listing = this.receiveDirectoryFiles.get(key) ?? [];
    const remaining = listing.filter((item) => !selected.has(item.path));
    this.receiveDirectoryFiles.set(key, remaining);
    const sources = this.receiveSources.get(key) ?? [];
    const source = sources.find((candidate) => candidate.id === operation.sourceId);
    const pendingCount = Math.max(
      0,
      (source?.pendingCount ?? 0) + selected.size
    );
    this.receiveSources.set(
      key,
      sources.map((candidate) =>
        candidate.id === operation.sourceId
          ? {
              ...candidate,
              pendingCount,
              lastScannedAt: new Date().toISOString()
            }
          : candidate
      )
    );
    const result: ReceiveSourceScanResult = {
      sourceId: operation.sourceId,
      scannedCount: selected.size,
      importedCount: 0,
      skippedCount: 0,
      pendingCount,
      failedCount: 0
    };
    this.receiveScanResults.push(result);
    return structuredClone(result);
  }

  async skipReceiveDirectoryFiles(
    library: LibrarySummary,
    operation: ReceiveDirectoryOperation
  ): Promise<ReceiveSource[]> {
    this.assertCurrentLibrary(library);
    this.calls.push(
      `skipReceiveDirectoryFiles:${operation.sourceId}:${operation.paths.length}`
    );
    const key = this.libraryKey(library);
    const skipped = new Set(operation.paths);
    const listing = this.receiveDirectoryFiles.get(key) ?? [];
    this.receiveDirectoryFiles.set(
      key,
      listing.map((item) =>
        skipped.has(item.path) ? { ...item, previouslySkipped: true } : item
      )
    );
    return structuredClone(this.receiveSources.get(key) ?? []);
  }

  async scanReceiveSources(
    library: LibrarySummary
  ): Promise<ReceiveSourceScanResult[]> {
    this.assertCurrentLibrary(library);
    this.calls.push("scanReceiveSources");
    const key = this.libraryKey(library);
    const results = (this.receiveSources.get(key) ?? [])
      .filter((source) => source.enabled)
      .map((source) => ({
        sourceId: source.id,
        scannedCount: 0,
        importedCount: 0,
        skippedCount: 0,
        pendingCount: source.pendingCount,
        failedCount: 0
      }));
    this.receiveScanResults.push(...results);
    return structuredClone(results);
  }

  async listReceiveImportLog(
    library: LibrarySummary,
    limit = 100
  ): Promise<ReceiveImportLogEntry[]> {
    this.assertCurrentLibrary(library);
    this.calls.push(`listReceiveImportLog:${limit}`);
    return structuredClone(this.receiveImportLog.slice(0, limit));
  }

  async subscribeToReceiveImportCompleted(
    handler: ReceiveImportCompletedHandler
  ): Promise<() => void> {
    this.receiveImportCompletedHandlers.add(handler);
    return () => {
      this.receiveImportCompletedHandlers.delete(handler);
    };
  }

  /** 测试用：模拟后端发出一次接收导入完成事件。 */
  emitReceiveImportCompleted(event: ReceiveImportCompletedEvent) {
    for (const handler of this.receiveImportCompletedHandlers) {
      handler(structuredClone(event));
    }
  }

  private snapshot(): BootstrapState {
    return {
      currentLibrary: this.state.currentLibrary,
      recentLibraries: [...this.state.recentLibraries]
    };
  }

  private assertCurrentLibrary(library: LibrarySummary) {
    if (
      this.strictLibraryIdentity &&
      !sameLibraryIdentity(library, this.state.currentLibrary)
    ) {
      throw new BackendError({
        code: "invalidLibrary",
        message: "资料库已切换，请在当前资料库重新发起操作。"
      });
    }
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
