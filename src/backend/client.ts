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
  IndexRunResult,
  LibraryLocationInspection,
  LibrarySummary,
  RecentLibrary,
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
  importDocument: (path) =>
    invoke<DocumentSummary>("import_document", { path }),
  startImport: (paths) => invoke<ImportBatch>("start_import", { paths }),
  resolveImportItem: (itemId, decision) =>
    invoke<ImportItemResult>("resolve_import_item", { itemId, decision }),
  retryImportItem: (itemId) =>
    invoke<ImportItemResult>("retry_import_item", { itemId }),
  subscribeToImportProgress: async (handler) =>
    listen<ImportProgress>("import-progress", (event) => {
      handler(event.payload);
    }),
  listDocuments: () => invoke<DocumentSummary[]>("list_documents"),
  subscribeToFileDrops: async (handler) =>
    getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === "drop") {
        handler(event.payload.paths);
      }
    }),
  getDocumentPreview: (documentId, page) =>
    invoke<DocumentPreview>("get_document_preview", { documentId, page }),
  getDocumentThumbnail: (documentId) =>
    invoke<DocumentThumbnail>("get_document_thumbnail", { documentId }),
  openDocument: (documentId) =>
    invoke<void>("open_document", { documentId }),
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
  createCollection: (name, parentId) =>
    invoke<CollectionSummary>("create_collection", { name, parentId }),
  renameCollection: (collectionId, name) =>
    invoke<CollectionSummary>("rename_collection", {
      collectionId,
      name
    }),
  moveCollection: (collectionId, parentId) =>
    invoke<CollectionSummary>("move_collection", {
      collectionId,
      parentId
    }),
  deleteCollection: (collectionId) =>
    invoke<CollectionDeleteResult>("delete_collection", { collectionId }),
  moveDocumentToCollection: (documentId, collectionId) =>
    invoke<DocumentSummary>("move_document_to_collection", {
      documentId,
      collectionId
    }),
  moveDocumentToTrash: (documentId) =>
    invoke<void>("move_document_to_trash", { documentId }),
  listTrashDocuments: () =>
    invoke<TrashDocumentSummary[]>("list_trash_documents"),
  restoreDocument: (documentId) =>
    invoke<DocumentSummary>("restore_document", { documentId }),
  permanentlyDeleteDocument: (documentId) =>
    invoke<void>("permanently_delete_document", { documentId }),
  emptyTrash: () => invoke<EmptyTrashResult>("empty_trash"),
  listTags: () => invoke<TagSummary[]>("list_tags"),
  createTag: (name) => invoke<TagSummary>("create_tag", { name }),
  renameTag: (tagId, name) =>
    invoke<TagSummary>("rename_tag", { tagId, name }),
  deleteTag: (tagId) => invoke<void>("delete_tag", { tagId }),
  addTagToDocument: (documentId, tagId) =>
    invoke<DocumentSummary>("add_tag_to_document", { documentId, tagId }),
  removeTagFromDocument: (documentId, tagId) =>
    invoke<DocumentSummary>("remove_tag_from_document", {
      documentId,
      tagId
    }),
  updateDocumentMetadata: (documentId, update: DocumentMetadataUpdate) =>
    invoke<DocumentSummary>("update_document_metadata", {
      documentId,
      update
    }),
  batchOrganizeDocuments: (request: BatchDocumentOperationRequest) =>
    invoke<BatchDocumentOperationResult>("batch_organize_documents", {
      request
    }),
  cancelBatchDocumentOperation: (jobId) =>
    invoke<boolean>("cancel_batch_document_operation", { jobId }),
  searchDocuments: (request: DocumentSearchQuery) =>
    invoke<DocumentSearchResponse>("search_documents", { request }),
  pendingIndexCount: () => invoke<number>("pending_index_count"),
  indexPendingDocuments: () =>
    invoke<IndexRunResult>("index_pending_documents"),
  retryDocumentIndex: (documentId) =>
    invoke<DocumentSummary>("retry_document_index", { documentId }),
  subscribeToDocumentIndexChanges: async (handler) =>
    listen<DocumentIndexChangedEvent>("document-index-changed", (event) => {
      handler(event.payload);
    })
};
