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
      kind: "table";
      /** 表格预览：CSV 与 XLSX 共用同一载荷形状。 */
      /** 可切换的工作表；CSV 只有一张合成表。 */
      sheets: TableSheet[];
      sheetIndex: number;
      /** 本次返回的起始行，从 0 开始。 */
      startRow: number;
      /** 本次返回的单元格，每行长度不超过 columnCount，不补齐缺失单元格。 */
      cells: string[][];
      /** 与 cells 一一对应的行号（从 0 开始）；XLSX 稀疏表会跳号。 */
      rowNumbers?: number[];
      columnCount: number;
      /** 是否还有更多行可读取。 */
      hasMoreRows: boolean;
      /**
       * 请求范围之后下一个含数据的行索引（0 基）；稀疏工作簿可以据此一次跳到数据行。
       * 缺席表示没有更多数据，或来源行是连续的（CSV）不需要跳转。
       */
      nextDataRow?: number;
      degradedFeatures: string[];
      notice: string | null;
    }
  | {
      kind: "unsupported";
      message: string;
    };

/** 表格文档（CSV/XLSX）的工作表元数据。 */
export interface TableSheet {
  index: number;
  name: string;
  /** 已知时的工作表行数；无缓存或未扫描时为 null。 */
  rowCount: number | null;
  columnCount: number | null;
}

export interface TablePreviewRequest {
  sheetIndex?: number;
  startRow?: number;
  rowCount?: number;
  columnCount?: number;
}

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
  | "pptx"
  | "csv"
  | "xlsx";

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
    | "pptxPackage"
    | "csvText"
    | "xlsxPackage";
  preview:
    | "pdfPages"
    | "docxLayout"
    | "plainText"
    | "safeMarkdown"
    | "localImage"
    | "pptxPages"
    | "tablePaged";
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
    | "pptxText"
    | "tableText";
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

// 分类规则（工作单 10）：按文件名、类型、来源目录为新建文档指定集合与标签。
export interface ClassificationRule {
  id: string;
  name: string;
  enabled: boolean;
  /** 用户顺序；数字越小越先匹配，第一条指定集合的命中规则决定集合。 */
  position: number;
  /** 文件名子串匹配；空字符串表示不限。 */
  fileNamePattern: string;
  /** 文件类型（显示名，如 PDF、CSV）；空字符串表示不限。 */
  fileType: string | null;
  /** 来源目录前缀匹配；空字符串表示不限。 */
  sourceDirectory: string | null;
  /** 命中的新建文档归档到该集合；null 表示本规则只贡献标签。 */
  collectionId: string | null;
  tagIds: string[];
}

export interface ClassificationRuleInput {
  name: string;
  enabled: boolean;
  fileNamePattern: string;
  fileType: string | null;
  sourceDirectory: string | null;
  collectionId: string | null;
  tagIds: string[];
}

export interface ClassificationRuleUpdate extends ClassificationRuleInput {
  id: string;
}

export type ClassificationRuleOperation =
  | { kind: "create"; rule: ClassificationRuleInput }
  | { kind: "update"; rule: ClassificationRuleUpdate }
  | { kind: "delete"; ruleId: string }
  | { kind: "setEnabled"; ruleId: string; enabled: boolean }
  | { kind: "reorder"; orderedRuleIds: string[] };

/** 一条规则对单个来源文件的预计结果。 */
export interface ClassificationPreviewItem {
  sourcePath: string;
  fileName: string;
  fileType: string | null;
  /** 预计归档集合；未命中任何规则时为收件箱。 */
  collectionId: string;
  tagIds: string[];
  /** 命中的规则 ID，按用户顺序；集合来自第一个指定集合的命中规则。 */
  matchedRuleIds: string[];
}

export interface ClassificationPreviewRequest {
  paths: string[];
  /** 用户显式指定的目标集合，优先于规则集合，但仍应用规则标签。 */
  targetCollectionId: string | null;
}

export interface ClassificationPreviewResponse {
  items: ClassificationPreviewItem[];
}

// 接收目录（工作单 11/12/13）：每个资料库分别配置 QQ、微信等来源。
export type ReceiveSourceKind = "qq" | "wechat" | "other";

export type ReceiveSourceStatus =
  | "unconfigured"
  | "ready"
  | "missing"
  | "unreadable";

export interface ReceiveSourceCandidate {
  path: string;
  /** 识别依据，供用户确认时参考。 */
  evidence: string;
}

export interface ReceiveSource {
  id: string;
  kind: ReceiveSourceKind;
  displayName: string;
  /** 用户确认的目录；未确认时为 null，此时不扫描不导入。 */
  path: string | null;
  enabled: boolean;
  status: ReceiveSourceStatus;
  /** 状态原因的补充说明，例如目录不存在或权限不足。 */
  statusMessage: string | null;
  /** 扫描到的待处理文件数（含待决项）。 */
  pendingCount: number;
  lastScannedAt: string | null;
}

export interface ReceiveSourceInput {
  kind: ReceiveSourceKind;
  displayName: string;
  path: string;
  enabled: boolean;
}

export interface ReceiveSourceCandidates {
  kind: ReceiveSourceKind;
  candidates: ReceiveSourceCandidate[];
}

/** 首次启用（或重新定位）后目录内已存在的受支持文件清单。 */
export interface ReceiveDirectoryListingItem {
  path: string;
  fileName: string;
  fileType: string | null;
  fileSize: number;
  /** 用户此前明确未选择的文件；补扫不会自动导入它们。 */
  previouslySkipped: boolean;
}

export interface ReceiveDirectoryListing {
  sourceId: string;
  path: string;
  items: ReceiveDirectoryListingItem[];
}

export interface ReceiveDirectoryOperation {
  sourceId: string;
  paths: string[];
}

export interface ReceiveSourceScanResult {
  sourceId: string;
  scannedCount: number;
  importedCount: number;
  skippedCount: number;
  pendingCount: number;
  failedCount: number;
}

/** 接收目录实时导入一轮结束后由后端发出；只针对产生它的资料库。 */
export interface ReceiveImportCompletedEvent {
  library: LibrarySummary;
  results: ReceiveSourceScanResult[];
}

export type ReceiveImportCompletedHandler = (
  event: ReceiveImportCompletedEvent
) => void;

export interface ReceiveImportLogEntry {
  sourceId: string;
  sourcePath: string;
  fileName: string;
  /** 结果状态沿用导入项状态，便于界面复用同一套展示。 */
  status: ImportItemStatus;
  documentId: string | null;
  collectionId: string | null;
  tagIds: string[];
  matchedRuleIds: string[];
  errorMessage: string | null;
  createdAt: string;
  /**
   * 来源内容变化产生的待决项标识；只有 `status === "sourceChanged"` 且未处理时有值。
   * 界面用它调用 `resolveImportItem` 决定「新建文档」还是「替换已有文档」。
   */
  itemId?: string | null;
  /** 待决项被处理的时间；有值表示这条待决已经结束，不再计入「待处理」。 */
  resolvedAt?: string | null;
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
  library: LibrarySummary;
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

export type ImportSource = "filePicker" | "collectionDrop";

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
  targetCollectionId: string | null;
  collectionId: string | null;
  notice: string | null;
}

export interface ImportBatch {
  batchId: string;
  items: ImportItemResult[];
  importedCount: number;
  duplicateCount: number;
  sourceChangedCount: number;
  failedCount: number;
  ignoredCount: number;
  targetCollectionId: string | null;
}

export interface ImportProgress {
  library: LibrarySummary;
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

export interface FileDropPosition {
  x: number;
  y: number;
}

export interface FileDropEvent {
  type: "enter" | "over" | "drop" | "leave";
  paths: string[];
  position: FileDropPosition | null;
}

export type FileDropHandler = (event: FileDropEvent) => void;
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
  importDocument(library: LibrarySummary, path: string): Promise<DocumentSummary>;
  startImport(
    library: LibrarySummary,
    paths: string[],
    targetCollectionId?: string | null,
    source?: ImportSource,
    /**
     * 是否应用当前资料库的分类规则；缺省为应用。
     * 显式传 false 时保持「按原有方式导入」的行为。
     */
    applyClassification?: boolean
  ): Promise<ImportBatch>;
  resolveImportItem(
    library: LibrarySummary,
    itemId: string,
    decision: ImportDecision
  ): Promise<ImportItemResult>;
  retryImportItem(library: LibrarySummary, itemId: string): Promise<ImportItemResult>;
  subscribeToImportProgress(
    handler: ImportProgressHandler
  ): Promise<() => void>;
  listDocuments(): Promise<DocumentSummary[]>;
  subscribeToFileDrops(handler: FileDropHandler): Promise<() => void>;
  getDocumentPreview(
    library: LibrarySummary,
    documentId: string,
    page?: number
  ): Promise<DocumentPreview>;
  getDocumentThumbnail(
    library: LibrarySummary,
    documentId: string
  ): Promise<DocumentThumbnail>;
  saveDocumentThumbnail(
    library: LibrarySummary,
    documentId: string,
    contentHash: string,
    thumbnailDataUrl: string
  ): Promise<DocumentThumbnail>;
  listDocumentSheets(
    library: LibrarySummary,
    documentId: string
  ): Promise<TableSheet[]>;
  getTablePreview(
    library: LibrarySummary,
    documentId: string,
    request?: TablePreviewRequest
  ): Promise<Extract<DocumentPreview, { kind: "table" }>>;
  openDocument(library: LibrarySummary, documentId: string): Promise<void>;
  openExternalUrl(url: string): Promise<void>;
  listDocumentFormatCapabilities(): Promise<DocumentFormatCapability[]>;
  listRecentLibraries(): Promise<RecentLibrary[]>;
  forgetRecentLibrary(path: string): Promise<RecentLibrary[]>;
  openLibraryDirectory(path: string): Promise<void>;
  listCollections(): Promise<CollectionSummary[]>;
  createCollection(
    library: LibrarySummary,
    name: string,
    parentId: string | null
  ): Promise<CollectionSummary>;
  renameCollection(
    library: LibrarySummary,
    collectionId: string,
    name: string
  ): Promise<CollectionSummary>;
  moveCollection(
    library: LibrarySummary,
    collectionId: string,
    parentId: string | null
  ): Promise<CollectionSummary>;
  deleteCollection(
    library: LibrarySummary,
    collectionId: string
  ): Promise<CollectionDeleteResult>;
  moveDocumentToCollection(
    library: LibrarySummary,
    documentId: string,
    collectionId: string
  ): Promise<DocumentSummary>;
  moveDocumentToTrash(library: LibrarySummary, documentId: string): Promise<void>;
  listTrashDocuments(): Promise<TrashDocumentSummary[]>;
  restoreDocument(library: LibrarySummary, documentId: string): Promise<DocumentSummary>;
  permanentlyDeleteDocument(library: LibrarySummary, documentId: string): Promise<void>;
  emptyTrash(library: LibrarySummary): Promise<EmptyTrashResult>;
  listTags(): Promise<TagSummary[]>;
  createTag(library: LibrarySummary, name: string): Promise<TagSummary>;
  renameTag(library: LibrarySummary, tagId: string, name: string): Promise<TagSummary>;
  deleteTag(library: LibrarySummary, tagId: string): Promise<void>;
  addTagToDocument(
    library: LibrarySummary,
    documentId: string,
    tagId: string
  ): Promise<DocumentSummary>;
  removeTagFromDocument(
    library: LibrarySummary,
    documentId: string,
    tagId: string
  ): Promise<DocumentSummary>;
  updateDocumentMetadata(
    library: LibrarySummary,
    documentId: string,
    update: DocumentMetadataUpdate
  ): Promise<DocumentSummary>;
  batchOrganizeDocuments(
    library: LibrarySummary,
    request: BatchDocumentOperationRequest
  ): Promise<BatchDocumentOperationResult>;
  cancelBatchDocumentOperation(jobId: string): Promise<boolean>;
  listClassificationRules(
    library: LibrarySummary
  ): Promise<ClassificationRule[]>;
  classificationRuleOperation(
    library: LibrarySummary,
    operation: ClassificationRuleOperation
  ): Promise<ClassificationRule[]>;
  previewClassification(
    library: LibrarySummary,
    request: ClassificationPreviewRequest
  ): Promise<ClassificationPreviewResponse>;
  listReceiveSources(library: LibrarySummary): Promise<ReceiveSource[]>;
  listReceiveSourceCandidates(
    library: LibrarySummary,
    kind: ReceiveSourceKind
  ): Promise<ReceiveSourceCandidates>;
  upsertReceiveSource(
    library: LibrarySummary,
    sourceId: string | null,
    input: ReceiveSourceInput
  ): Promise<ReceiveSource[]>;
  removeReceiveSource(
    library: LibrarySummary,
    sourceId: string
  ): Promise<ReceiveSource[]>;
  listReceiveDirectoryFiles(
    library: LibrarySummary,
    sourceId: string
  ): Promise<ReceiveDirectoryListing>;
  applyReceiveDirectorySelection(
    library: LibrarySummary,
    operation: ReceiveDirectoryOperation
  ): Promise<ReceiveSourceScanResult>;
  skipReceiveDirectoryFiles(
    library: LibrarySummary,
    operation: ReceiveDirectoryOperation
  ): Promise<ReceiveSource[]>;
  scanReceiveSources(library: LibrarySummary): Promise<ReceiveSourceScanResult[]>;
  subscribeToReceiveImportCompleted(
    handler: ReceiveImportCompletedHandler
  ): Promise<() => void>;
  listReceiveImportLog(
    library: LibrarySummary,
    limit?: number
  ): Promise<ReceiveImportLogEntry[]>;
  searchDocuments(
    request: DocumentSearchQuery
  ): Promise<DocumentSearchResponse>;
  pendingIndexCount(): Promise<number>;
  indexPendingDocuments(library: LibrarySummary): Promise<IndexRunResult>;
  retryDocumentIndex(
    library: LibrarySummary,
    documentId: string
  ): Promise<DocumentSummary>;
  subscribeToDocumentIndexChanges(
    handler: DocumentIndexChangedHandler
  ): Promise<() => void>;
}
