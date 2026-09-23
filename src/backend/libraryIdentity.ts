import type { LibrarySummary } from "./types";

function normalizedPath(path: string): string {
  return path.replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();
}

export function sameLibraryIdentity(
  left: LibrarySummary | null | undefined,
  right: LibrarySummary | null | undefined
): boolean {
  return Boolean(
    left &&
      right &&
      left.id === right.id &&
      normalizedPath(left.path) === normalizedPath(right.path)
  );
}
