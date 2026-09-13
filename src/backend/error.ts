import type { BackendErrorShape } from "./types";

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
