import {
  act,
  fireEvent,
  render,
  screen,
  waitFor
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { App } from "./App";
import { FakeBackendClient } from "./backend/fakeClient";
import type {
  BootstrapState,
  CollectionSummary,
  DocumentSummary,
  IndexRunResult,
  LibrarySummary
} from "./backend/types";

const library: LibrarySummary = {
  id: "library-scale",
  name: "规模验收资料库",
  path: "C:\\Documents\\规模验收资料库",
  createdAt: "2026-09-13T08:00:00Z"
};

const bootstrap: BootstrapState = {
  currentLibrary: library,
  recentLibraries: []
};

const collections: CollectionSummary[] = [
  {
    id: "inbox",
    name: "收件箱",
    parentId: null,
    isInbox: true,
    documentCount: 10_000
  }
];

function scaleDocument(index: number): DocumentSummary {
  return {
    id: `document-${index}`,
    title: `规模验收资料 ${index.toString().padStart(5, "0")}`,
    description: null,
    documentDate: "2026-02-01",
    fileName: `acceptance-${index.toString().padStart(5, "0")}.txt`,
    fileType: index % 6 === 0 ? "PDF" : "TXT",
    fileSize: 256,
    contentHash: `hash-${index}`,
    collectionId: "inbox",
    tags: [],
    processingStatus: "ready",
    indexStatus: "searchable",
    errorStage: null,
    errorMessage: null,
    importedAt: "2026-09-13T08:10:00Z",
    sourcePath: `C:\\Sources\\acceptance-${index}.txt`,
    sourceIdentifier: `c:\\sources\\acceptance-${index}.txt`,
    lastImportedAt: "2026-09-13T08:10:00Z"
  };
}

describe("一万份资料库的列表与网格渲染规模", () => {
  it("shows live indexing progress while a large index run is active", async () => {
    const indexPromise = new Promise<IndexRunResult>(() => {});
    const client = new FakeBackendClient({
      bootstrap,
      documents: Array.from({ length: 100 }, (_, index) => ({
        ...scaleDocument(index),
        indexStatus: "pending"
      })),
      collections,
      indexPendingDocuments: () => indexPromise
    });
    render(<App client={client} />);

    const status = await screen.findByRole("status");
    await waitFor(() => {
      expect(status).toHaveTextContent(/正在建立索引 \d+\/100/);
    });

    act(() => {
      client.emitDocumentIndexChanged({
        phase: "processing",
        documentIds: [],
        result: { processed: 40, searchable: 39, failed: 1 }
      });
    });

    expect(
      screen.getByText("正在建立索引 40/100")
    ).toBeInTheDocument();
  }, 10_000);

  it("renders an initial bounded window and reveals more on scroll", async () => {
    const user = userEvent.setup();
    const client = new FakeBackendClient({
      bootstrap,
      documents: Array.from({ length: 10_000 }, (_, index) =>
        scaleDocument(index)
      ),
      collections
    });
    const { container } = render(<App client={client} />);

    await screen.findByRole("main", { name: "文档列表" });
    await waitFor(() => {
      expect(container.querySelectorAll(".document-row")).toHaveLength(200);
    });

    fireEvent.scroll(screen.getByRole("table", { name: "文档结果" }));
    await waitFor(() => {
      expect(container.querySelectorAll(".document-row")).toHaveLength(400);
    });

    await user.click(screen.getByRole("button", { name: "网格视图" }));
    await waitFor(() => {
      expect(container.querySelectorAll(".document-grid-item")).toHaveLength(
        200
      );
    });
  });
});
