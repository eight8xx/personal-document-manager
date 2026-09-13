import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

import type {
  BackendClient,
  BackendErrorShape,
  BootstrapState,
  LibraryLocationInspection,
  LibrarySummary,
  RecentLibrary
} from "./types";

export class BackendError extends Error {
  readonly code: string;

  constructor({ code, message }: BackendErrorShape) {
    super(message);
    this.name = "BackendError";
    this.code = code;
  }
}

export function toBackendError(error: unknown): BackendError {
  if (error instanceof BackendError) {
    return error;
  }

  if (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    "message" in error &&
    typeof error.code === "string" &&
    typeof error.message === "string"
  ) {
    return new BackendError({ code: error.code, message: error.message });
  }

  if (typeof error === "string") {
    return new BackendError({ code: "unknown", message: error });
  }

  return new BackendError({
    code: "unknown",
    message: "操作失败，请稍后重试。"
  });
}

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
  listRecentLibraries: () =>
    invoke<RecentLibrary[]>("list_recent_libraries"),
  forgetRecentLibrary: (path) =>
    invoke<RecentLibrary[]>("forget_recent_library", { path }),
  openLibraryDirectory: (path) =>
    invoke<void>("open_library_directory", { path })
};
