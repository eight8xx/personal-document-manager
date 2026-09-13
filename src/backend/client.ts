import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";

import type {
  BackendClient,
  BootstrapState,
  DocumentSummary,
  LibraryLocationInspection,
  LibrarySummary,
  RecentLibrary
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
          extensions: [
            "pdf",
            "docx",
            "txt",
            "md",
            "markdown",
            "jpg",
            "jpeg",
            "png"
          ]
        }
      ]
    });
    return typeof selected === "string" ? selected : null;
  },
  importDocument: (path) =>
    invoke<DocumentSummary>("import_document", { path }),
  listDocuments: () => invoke<DocumentSummary[]>("list_documents"),
  subscribeToFileDrops: async (handler) =>
    getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === "drop") {
        handler(event.payload.paths);
      }
    }),
  listRecentLibraries: () =>
    invoke<RecentLibrary[]>("list_recent_libraries"),
  forgetRecentLibrary: (path) =>
    invoke<RecentLibrary[]>("forget_recent_library", { path }),
  openLibraryDirectory: (path) =>
    invoke<void>("open_library_directory", { path })
};
