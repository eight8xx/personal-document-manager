import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";

import { importableDocumentExtensions } from "./documentFormats";
import type {
  BackendClient,
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
  DocumentFormatCapability,
  DocumentMetadataUpdate,
  DocumentPreview,
  DocumentSearchQuery,
  DocumentSearchResponse,
  DocumentSummary,
  DocumentThumbnail,
  EmptyTrashResult,
  ImportBatch,
  ImportDecision,
  ImportItemResult,
  ImportProgress,
  ImportSource,
  IndexRunResult,
  LibraryLocationInspection,
  LibrarySummary,
  ReceiveDirectoryListing,
  ReceiveDirectoryOperation,
  ReceiveImportLogEntry,
  ReceiveSource,
  ReceiveSourceCandidates,
  ReceiveSourceInput,
  ReceiveSourceKind,
  ReceiveSourceScanResult,
  RecentLibrary,
  TableSheet,
  TagSummary,
  TrashDocumentSummary
} from "./types";

export const tauriBackendClient: BackendClient = {
  bootstrap: () => invoke<BootstrapState>("bootstrap"),
  inspectLibraryLocation: (path) =>
    invoke<LibraryLocationInspection>("inspect_library_location", { path }),
  pickLibraryDirectory: async () => {
    const selected = await open({
      directory: true,
      multiple: false,
      title: "选择资料库目录"
    });
    return typeof selected === "string" ? selected : null;
  },
  createLibrary: (path) => invoke<LibrarySummary>("create_library", { path }),
  openLibrary: (path) => invoke<LibrarySummary>("open_library", { path }),
  pickDocumentFile: async () => {
    const selected = await open({
      directory: false,
      multiple: false,
      title: "选择要导入的文档",
      filters: [
        {
          name: "支持的文档",
          extensions: importableDocumentExtensions
        }
      ]
    });
    return typeof selected === "string" ? selected : null;
  },
  pickDocumentFiles: async () => {
    const selected = await open({
      directory: false,
      multiple: true,
      title: "选择要导入的文档",
      filters: [
        {
          name: "支持的文档",
          extensions: importableDocumentExtensions
        }
      ]
    });
    return Array.isArray(selected) ? selected : [];
  },
  pickDocumentFolder: async () => {
    const selected = await open({
      directory: true,
      multiple: false,
      title: "选择要导入的文件夹"
    });
    return typeof selected === "string" ? selected : null;
  },
  importDocument: (library, path) =>
    invoke<DocumentSummary>("import_document", { library, path }),
  startImport: (
    library,
    paths,
    targetCollectionId = null,
    source = "filePicker"
  ) =>
    invoke<ImportBatch>("start_import", {
      library,
      paths,
      targetCollectionId,
      source
    }),
  resolveImportItem: (library, itemId, decision) =>
    invoke<ImportItemResult>("resolve_import_item", { library, itemId, decision }),
  retryImportItem: (library, itemId) =>
    invoke<ImportItemResult>("retry_import_item", { library, itemId }),
  subscribeToImportProgress: async (handler) =>
    listen<ImportProgress>("import-progress", (event) => {
      handler(event.payload);
    }),
  listDocuments: () => invoke<DocumentSummary[]>("list_documents"),
  subscribeToFileDrops: async (handler) => {
    const scaleFactor = window.devicePixelRatio || 1;
    return getCurrentWebview().onDragDropEvent((event) => {
      const payload = event.payload;
      if (payload.type === "leave") {
        handler({ type: "leave", paths: [], position: null });
        return;
      }
      handler({
        type: payload.type,
        paths: payload.type === "over" ? [] : payload.paths,
        position: {
          x: payload.position.x / scaleFactor,
          y: payload.position.y / scaleFactor
        }
      });
    });
  },
  getDocumentPreview: (library, documentId, page) =>
    invoke<DocumentPreview>("get_document_preview", { library, documentId, page }),
  getDocumentThumbnail: (library, documentId) =>
    invoke<DocumentThumbnail>("get_document_thumbnail", { library, documentId }),
  saveDocumentThumbnail: (library, documentId, contentHash, thumbnailDataUrl) =>
    invoke<DocumentThumbnail>("save_document_thumbnail", {
      library,
      documentId,
      contentHash,
      thumbnailDataUrl
    }),
  listDocumentSheets: (library, documentId) =>
    invoke<TableSheet[]>("list_document_sheets", { library, documentId }),
  getTablePreview: (library, documentId, request = {}) =>
    invoke<Extract<DocumentPreview, { kind: "table" }>>("get_table_preview", {
      library,
      documentId,
      sheetIndex: request.sheetIndex ?? 0,
      startRow: request.startRow ?? 0,
      rowCount: request.rowCount ?? 0,
      columnCount: request.columnCount ?? 0
    }),
  openDocument: (library, documentId) =>
    invoke<void>("open_document", { library, documentId }),
  openExternalUrl: (url) => invoke<void>("open_external_url", { url }),
  listDocumentFormatCapabilities: () =>
    invoke<DocumentFormatCapability[]>("list_document_format_capabilities"),
  listRecentLibraries: () =>
    invoke<RecentLibrary[]>("list_recent_libraries"),
  forgetRecentLibrary: (path) =>
    invoke<RecentLibrary[]>("forget_recent_library", { path }),
  openLibraryDirectory: (path) =>
    invoke<void>("open_library_directory", { path }),
  listCollections: () => invoke<CollectionSummary[]>("list_collections"),
  createCollection: (library, name, parentId) =>
    invoke<CollectionSummary>("create_collection", { library, name, parentId }),
  renameCollection: (library, collectionId, name) =>
    invoke<CollectionSummary>("rename_collection", {
      library,
      collectionId,
      name
    }),
  moveCollection: (library, collectionId, parentId) =>
    invoke<CollectionSummary>("move_collection", {
      library,
      collectionId,
      parentId
    }),
  deleteCollection: (library, collectionId) =>
    invoke<CollectionDeleteResult>("delete_collection", { library, collectionId }),
  moveDocumentToCollection: (library, documentId, collectionId) =>
    invoke<DocumentSummary>("move_document_to_collection", {
      library,
      documentId,
      collectionId
    }),
  moveDocumentToTrash: (library, documentId) =>
    invoke<void>("move_document_to_trash", { library, documentId }),
  listTrashDocuments: () =>
    invoke<TrashDocumentSummary[]>("list_trash_documents"),
  restoreDocument: (library, documentId) =>
    invoke<DocumentSummary>("restore_document", { library, documentId }),
  permanentlyDeleteDocument: (library, documentId) =>
    invoke<void>("permanently_delete_document", { library, documentId }),
  emptyTrash: (library) => invoke<EmptyTrashResult>("empty_trash", { library }),
  listTags: () => invoke<TagSummary[]>("list_tags"),
  createTag: (library, name) => invoke<TagSummary>("create_tag", { library, name }),
  renameTag: (library, tagId, name) =>
    invoke<TagSummary>("rename_tag", { library, tagId, name }),
  deleteTag: (library, tagId) => invoke<void>("delete_tag", { library, tagId }),
  addTagToDocument: (library, documentId, tagId) =>
    invoke<DocumentSummary>("add_tag_to_document", { library, documentId, tagId }),
  removeTagFromDocument: (library, documentId, tagId) =>
    invoke<DocumentSummary>("remove_tag_from_document", {
      library,
      documentId,
      tagId
    }),
  updateDocumentMetadata: (library, documentId, update: DocumentMetadataUpdate) =>
    invoke<DocumentSummary>("update_document_metadata", {
      library,
      documentId,
      update
    }),
  batchOrganizeDocuments: (
    library: LibrarySummary,
    request: BatchDocumentOperationRequest
  ) =>
    invoke<BatchDocumentOperationResult>("batch_organize_documents", {
      library,
      request
    }),
  cancelBatchDocumentOperation: (jobId) =>
    invoke<boolean>("cancel_batch_document_operation", { jobId }),
  listClassificationRules: (library) =>
    invoke<ClassificationRule[]>("list_classification_rules", { library }),
  classificationRuleOperation: (library, operation) =>
    invoke<ClassificationRule[]>("apply_classification_rule_operation", {
      library,
      operation
    }),
  previewClassification: (library, request) =>
    invoke<ClassificationPreviewResponse>("preview_classification", {
      library,
      request
    }),
  listReceiveSources: (library) =>
    invoke<ReceiveSource[]>("list_receive_sources", { library }),
  listReceiveSourceCandidates: (library, kind) =>
    invoke<ReceiveSourceCandidates>("list_receive_source_candidates", {
      library,
      kind
    }),
  upsertReceiveSource: (library, sourceId, input) =>
    invoke<ReceiveSource[]>("upsert_receive_source", {
      library,
      sourceId,
      input
    }),
  removeReceiveSource: (library, sourceId) =>
    invoke<ReceiveSource[]>("remove_receive_source", { library, sourceId }),
  listReceiveDirectoryFiles: (library, sourceId) =>
    invoke<ReceiveDirectoryListing>("list_receive_directory_files", {
      library,
      sourceId
    }),
  applyReceiveDirectorySelection: (library, operation) =>
    invoke<ReceiveSourceScanResult>("apply_receive_directory_selection", {
      library,
      operation
    }),
  skipReceiveDirectoryFiles: (library, operation) =>
    invoke<ReceiveSource[]>("skip_receive_directory_files", {
      library,
      operation
    }),
  scanReceiveSources: (library) =>
    invoke<ReceiveSourceScanResult[]>("scan_receive_sources", { library }),
  listReceiveImportLog: (library, limit = 100) =>
    invoke<ReceiveImportLogEntry[]>("list_receive_import_log", {
      library,
      limit
    }),
  searchDocuments: (request: DocumentSearchQuery) =>
    invoke<DocumentSearchResponse>("search_documents", { request }),
  pendingIndexCount: () => invoke<number>("pending_index_count"),
  indexPendingDocuments: (library) =>
    invoke<IndexRunResult>("index_pending_documents", { library }),
  retryDocumentIndex: (library, documentId) =>
    invoke<DocumentSummary>("retry_document_index", { library, documentId }),
  subscribeToDocumentIndexChanges: async (handler) =>
    listen<DocumentIndexChangedEvent>("document-index-changed", (event) => {
      handler(event.payload);
    })
};
