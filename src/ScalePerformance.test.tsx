import {
  act,
  fireEvent,
  render,
  screen,
  within,
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
  ImportBatch,
  ImportItemResult,
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

    const status = await screen.findByRole(
      "status",
      undefined,
      { timeout: 3_000 }
    );
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

  it("recycles list rows across a ten-thousand-document scroll and keeps selection usable", async () => {
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
    const list = screen.getByRole("table", { name: "文档结果" });
    await waitFor(() => {
      expect(container.querySelectorAll(".document-row").length).toBeLessThan(50);
    });
    expect(within(list).getByRole("button", { name: "选择文档 规模验收资料 00000" })).toBeInTheDocument();

    list.scrollTop = 5_000 * 72;
    fireEvent.scroll(list);
    await waitFor(() => {
      expect(within(list).getByRole("button", { name: "选择文档 规模验收资料 05000" })).toBeInTheDocument();
    });
    expect(within(list).queryByRole("button", { name: "选择文档 规模验收资料 00000" })).not.toBeInTheDocument();
    expect(container.querySelectorAll(".document-row").length).toBeLessThan(50);

    await user.click(within(list).getByRole("button", { name: "选择文档 规模验收资料 05000" }));
    expect(screen.getByText("已选择 1 项")).toBeInTheDocument();

    list.scrollTop = 9_990 * 72;
    fireEvent.scroll(list);
    await waitFor(() => {
      expect(within(list).getByRole("button", { name: "选择文档 规模验收资料 09990" })).toBeInTheDocument();
    });
    expect(screen.getByRole("heading", { name: /规模验收资料 05000/ })).toBeInTheDocument();
    expect(container.querySelectorAll(".document-row").length).toBeLessThan(50);

    list.scrollTop = 5_000 * 72;
    fireEvent.scroll(list);
    const selected = await within(list).findByRole("button", { name: "选择文档 规模验收资料 05000" });
    expect(selected).toHaveAttribute("aria-pressed", "true");
    selected.focus();
    await user.keyboard("{End}");
    await waitFor(() => {
      expect(within(list).getByRole("button", { name: "选择文档 规模验收资料 09999" })).toHaveFocus();
    });
    await user.keyboard("{Enter}");
    expect(within(list).getByRole("button", { name: "选择文档 规模验收资料 09999" })).toHaveAttribute("aria-pressed", "true");
    expect(container.querySelectorAll(".document-row").length).toBeLessThan(50);
  });

  it("does not return to an old highlighted document when the list viewport changes height", async () => {
    const user = userEvent.setup();
    const duplicate: ImportItemResult = {
      itemId: "duplicate-500",
      sourcePath: "C:\\Sources\\duplicate.txt",
      fileName: "duplicate.txt",
      fileType: "TXT",
      status: "duplicate",
      documentId: null,
      duplicateDocumentId: "document-500",
      errorStage: null,
      errorMessage: null,
      retryable: false,
      targetCollectionId: null,
      collectionId: null,
      notice: null
    };
    const importBatch: ImportBatch = {
      batchId: "highlight-existing",
      items: [duplicate],
      importedCount: 0,
      duplicateCount: 1,
      sourceChangedCount: 0,
      failedCount: 0,
      ignoredCount: 0,
      targetCollectionId: null
    };
    const client = new FakeBackendClient({
      bootstrap,
      documents: Array.from({ length: 1_000 }, (_, index) =>
        scaleDocument(index)
      ),
      collections,
      selectedDocuments: [duplicate.sourcePath],
      startImport: async () => importBatch
    });
    render(<App client={client} />);
    const list = await screen.findByRole("table", { name: "文档结果" });
    await user.click(screen.getByRole("button", { name: "导入文档" }));
    const dialog = await screen.findByRole("dialog", { name: "发现重复文档" });
    await user.click(within(dialog).getByRole("button", { name: "打开已有文档" }));
    await waitFor(() => {
      expect(within(list).getByRole("button", { name: "选择文档 规模验收资料 00500" }).closest("article")).toHaveAttribute("aria-current", "true");
    });

    list.scrollTop = 900 * 72 + 38;
    fireEvent.scroll(list);
    await waitFor(() => {
      expect(within(list).getByRole("button", { name: "选择文档 规模验收资料 00900" })).toBeInTheDocument();
    });
    Object.defineProperty(list, "clientHeight", { configurable: true, value: 420 });
    fireEvent.resize(window);
    expect(list.scrollTop).toBe(900 * 72 + 38);
    expect(within(list).getByRole("button", { name: "选择文档 规模验收资料 00900" })).toBeInTheDocument();
  });

  it("starts keyboard navigation at the visible row after a focused row is recycled", async () => {
    const user = userEvent.setup();
    const client = new FakeBackendClient({
      bootstrap,
      documents: Array.from({ length: 1_000 }, (_, index) =>
        scaleDocument(index)
      ),
      collections
    });
    render(<App client={client} />);
    const list = await screen.findByRole("table", { name: "文档结果" });
    const first = within(list).getByRole("button", { name: "选择文档 规模验收资料 00000" });
    first.focus();
    list.scrollTop = 500 * 72 + 38;
    fireEvent.scroll(list);
    expect(list).toHaveFocus();
    expect(within(list).getByRole("button", { name: "选择文档 规模验收资料 00500" })).toBeInTheDocument();

    await user.keyboard("{ArrowDown}");
    expect(within(list).getByRole("button", { name: "选择文档 规模验收资料 00501" })).toHaveFocus();
  });

  it("recycles grid cards and preserves their order when the viewport width changes", async () => {
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

    await user.click(screen.getByRole("button", { name: "网格视图" }));
    const grid = screen.getByRole("list", { name: "文档结果" });
    Object.defineProperty(grid, "clientWidth", { configurable: true, value: 430 });
    Object.defineProperty(grid, "clientHeight", { configurable: true, value: 500 });
    fireEvent.resize(window);
    await waitFor(() => {
      expect(container.querySelectorAll(".document-grid-item").length).toBeLessThan(40);
    });

    grid.scrollTop = 1_000 * 248;
    fireEvent.scroll(grid);
    await waitFor(() => {
      expect(within(grid).getByRole("button", { name: "选择文档 规模验收资料 02000" })).toBeInTheDocument();
    });
    expect(within(grid).queryByRole("button", { name: "选择文档 规模验收资料 00000" })).not.toBeInTheDocument();
    expect(container.querySelectorAll(".document-grid-item").length).toBeLessThan(40);

    Object.defineProperty(grid, "clientWidth", { configurable: true, value: 900 });
    fireEvent.resize(window);
    await waitFor(() => {
      expect(within(grid).getByRole("button", { name: "选择文档 规模验收资料 02000" })).toBeInTheDocument();
    });
    expect(container.querySelectorAll(".document-grid-item").length).toBeLessThan(60);

    grid.scrollTop = 10_000 * 248;
    fireEvent.scroll(grid);
    await waitFor(() => {
      expect(within(grid).getByRole("button", { name: "选择文档 规模验收资料 09999" })).toBeInTheDocument();
    });
    expect(container.querySelectorAll(".document-grid-item").length).toBeLessThan(60);

    within(grid).getByRole("button", { name: "选择文档 规模验收资料 09999" }).focus();
    await user.keyboard("{Home}");
    await waitFor(() => {
      expect(within(grid).getByRole("button", { name: "选择文档 规模验收资料 00000" })).toHaveFocus();
    });
    await user.keyboard("{Enter}");
    expect(within(grid).getByRole("button", { name: "选择文档 规模验收资料 00000" })).toHaveAttribute("aria-pressed", "true");

    grid.scrollTop = 10_000 * 248;
    fireEvent.scroll(grid);
    await waitFor(() => {
      expect(within(grid).getByRole("button", { name: "选择文档 规模验收资料 09999" })).toBeInTheDocument();
    });
    await user.click(screen.getByRole("button", { name: "筛选" }));
    await user.selectOptions(screen.getByRole("combobox", { name: "文件类型" }), "PDF");
    await waitFor(() => {
      expect(within(grid).getByRole("button", { name: "选择文档 规模验收资料 00000" })).toBeInTheDocument();
    });
    expect(grid.scrollTop).toBe(0);
    expect(container.querySelectorAll(".document-grid-item").length).toBeLessThan(60);
  });
});
