import {
  act,
  render,
  screen,
  waitFor,
  within
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { App } from "./App";
import { FakeBackendClient } from "./backend/fakeClient";
import type {
  BootstrapState,
  DocumentSummary,
  ImportBatch,
  ImportItemResult,
  IndexRunResult,
  LibrarySummary
} from "./backend/types";

const library: LibrarySummary = {
  id: "library-current",
  name: "个人资料",
  path: "C:\\Documents\\个人资料",
  createdAt: "2026-09-13T08:00:00Z"
};

const existingDocument: DocumentSummary = {
  id: "document-existing",
  title: "项目说明",
  description: null,
  documentDate: null,
  fileName: "项目说明.md",
  fileType: "Markdown",
  fileSize: 24,
  contentHash: "hash-existing",
  collectionId: "inbox",
  tags: [],
  processingStatus: "ready",
  indexStatus: "searchable",
  errorStage: null,
  errorMessage: null,
  importedAt: "2026-09-13T08:10:00Z",
  sourcePath: "C:\\Documents\\项目说明.md",
  sourceIdentifier: "c:\\documents\\项目说明.md",
  lastImportedAt: "2026-09-13T08:10:00Z"
};

const bootstrap: BootstrapState = {
  currentLibrary: library,
  recentLibraries: []
};

function result(
  itemId: string,
  fileName: string,
  status: ImportItemResult["status"],
  overrides: Partial<ImportItemResult> = {}
): ImportItemResult {
  return {
    itemId,
    sourcePath: `C:\\Documents\\${fileName}`,
    fileName,
    fileType: fileName.split(".").at(-1)?.toUpperCase() ?? null,
    status,
    documentId: status === "imported" ? `document-${itemId}` : null,
    duplicateDocumentId: null,
    errorStage: null,
    errorMessage: null,
    retryable: status === "failed",
    targetCollectionId: null,
    collectionId: null,
    notice: null,
    ...overrides
  };
}

function batch(batchId: string, items: ImportItemResult[]): ImportBatch {
  return {
    batchId,
    items,
    importedCount: items.filter((item) => item.status === "imported").length,
    duplicateCount: items.filter((item) => item.status === "duplicate").length,
    sourceChangedCount: items.filter(
      (item) => item.status === "sourceChanged"
    ).length,
    failedCount: items.filter((item) => item.status === "failed").length,
    ignoredCount: items.filter((item) => item.status === "ignored").length,
    targetCollectionId: null
  };
}

describe("批量导入流程", () => {
  it("merges import progress events with the command result", async () => {
    const user = userEvent.setup();
    let resolveBatch: ((value: ImportBatch) => void) | undefined;
    const progressItem = result("item-1", "正常.pdf", "imported");
    const duplicateItem = result("item-2", "重复.md", "duplicate", {
      duplicateDocumentId: existingDocument.id
    });
    const ignoredItem = result("item-3", "说明.exe", "ignored");
    const client = new FakeBackendClient({
      bootstrap,
      documents: [existingDocument],
      selectedDocuments: [
        progressItem.sourcePath,
        duplicateItem.sourcePath,
        ignoredItem.sourcePath
      ],
      startImport: () =>
        new Promise((resolve) => {
          resolveBatch = resolve;
        })
    });

    render(<App client={client} />);
    await screen.findByRole("button", { name: /全部文档/ });
    await waitFor(() => {
      expect(client.calls).toContain("subscribeToImportProgress");
    });
    await user.click(screen.getByRole("button", { name: "导入文档" }));
    await waitFor(() => {
      expect(client.calls.some((call) => call.startsWith("startImport:"))).toBe(
        true
      );
    });

    act(() => {
      client.emitImportProgress({
        batchId: "batch-merge",
        total: 3,
        completed: 0,
        currentFileName: progressItem.fileName,
        currentSourcePath: progressItem.sourcePath,
        item: null,
        finished: false
      });
    });
    expect(await screen.findByText("已完成 0/3")).toBeInTheDocument();
    expect(
      within(screen.getByLabelText("批量导入进度")).getByText("正在导入")
    ).toBeInTheDocument();

    act(() => {
      client.emitImportProgress({
        batchId: "batch-merge",
        total: 3,
        completed: 1,
        currentFileName: progressItem.fileName,
        currentSourcePath: progressItem.sourcePath,
        item: progressItem,
        finished: false
      });
    });

    expect(await screen.findByText("已完成 1/3")).toBeInTheDocument();
    expect(screen.getByText("当前文件：正常.pdf")).toBeInTheDocument();

    await act(async () => {
      resolveBatch?.(
        batch("batch-merge", [progressItem, duplicateItem, ignoredItem])
      );
    });

    expect(await screen.findByText("导入批次已完成")).toBeInTheDocument();
    const summary = screen.getByLabelText("导入结果汇总");
    expect(summary).toHaveTextContent("已导入 1");
    expect(summary).toHaveTextContent("待决策 1");
    expect(summary).toHaveTextContent("忽略 1");
    expect(
      screen.getByText("已忽略 1 个不受支持的文件。")
    ).toBeInTheDocument();
    expect(
      await screen.findByRole("dialog", { name: "发现重复文档" })
    ).toBeInTheDocument();
  });

  it("loads the live pending total after import instead of using the old snapshot", async () => {
    const user = userEvent.setup();
    const sourcePath = "C:\\Sources\\fresh-import.txt";
    const client = new FakeBackendClient({
      bootstrap,
      selectedDocuments: [sourcePath]
    });
    client.indexPendingDocuments = () =>
      new Promise<IndexRunResult>(() => {});

    render(<App client={client} />);
    await screen.findByRole("button", { name: /全部文档/ });
    await user.click(
      screen.getAllByRole("button", { name: "导入文档" })[0]
    );

    await waitFor(() => {
      expect(client.calls).toContain("pendingIndexCount");
    });
    const status = await screen.findByRole("status");
    expect(status).toHaveTextContent("正在建立索引 0/1");
    const results = await screen.findByRole("main", { name: "文档列表" });
    expect(within(results).getByText("处理中")).toBeInTheDocument();
  });

  it("opens an existing duplicate and highlights it in the document list", async () => {
    const user = userEvent.setup();
    const duplicateItem = result("duplicate-item", "重复.md", "duplicate", {
      duplicateDocumentId: existingDocument.id
    });
    const client = new FakeBackendClient({
      bootstrap,
      documents: [existingDocument],
      selectedDocuments: [duplicateItem.sourcePath],
      startImport: async () => batch("batch-duplicate", [duplicateItem])
    });

    render(<App client={client} />);
    await screen.findByText("项目说明");
    await user.click(screen.getByRole("button", { name: "导入文档" }));

    const dialog = await screen.findByRole("dialog", {
      name: "发现重复文档"
    });
    await user.click(
      within(dialog).getByRole("button", { name: "打开已有文档" })
    );

    await waitFor(() => {
      expect(client.calls).toContain(
        "resolveImportItem:duplicate-item:useExisting"
      );
    });
    const row = screen
      .getAllByText("项目说明")
      .map((element) => element.closest("article"))
      .find((element): element is HTMLElement => element !== null);
    if (!row) {
      throw new Error("无法找到已有文档行。");
    }
    expect(row).toHaveAttribute("aria-current", "true");
    expect(screen.queryByRole("dialog", { name: "发现重复文档" })).toBeNull();
  });

  it("requires confirmation before replacing a changed source document", async () => {
    const user = userEvent.setup();
    const changedItem = result(
      "source-changed-item",
      "项目说明.md",
      "sourceChanged",
      { duplicateDocumentId: existingDocument.id }
    );
    const client = new FakeBackendClient({
      bootstrap,
      documents: [existingDocument],
      selectedDocuments: [changedItem.sourcePath],
      startImport: async () => batch("batch-source", [changedItem])
    });

    render(<App client={client} />);
    await screen.findByText("项目说明");
    await user.click(screen.getByRole("button", { name: "导入文档" }));

    const decisionDialog = await screen.findByRole("dialog", {
      name: "源文件已发生变化"
    });
    await user.click(
      within(decisionDialog).getByRole("button", {
        name: "替换已有文档"
      })
    );

    const confirmation = screen.getByRole("dialog", {
      name: "确认替换文档"
    });
    expect(confirmation).toHaveTextContent("旧内容不会保留");
    expect(
      client.calls.some((call) => call.includes("replaceExisting"))
    ).toBe(false);

    await user.click(
      within(confirmation).getByRole("button", { name: "确认替换" })
    );
    await waitFor(() => {
      expect(client.calls).toContain(
        "resolveImportItem:source-changed-item:replaceExisting"
      );
    });
  });

  it("retries a failed item without offering retry for successful items", async () => {
    const user = userEvent.setup();
    const successfulItem = result("ok-item", "正常.pdf", "imported");
    const failedItem = result("failed-item", "损坏.pdf", "failed", {
      errorStage: "copying",
      errorMessage: "无法读取文件"
    });
    const client = new FakeBackendClient({
      bootstrap,
      selectedDocuments: [successfulItem.sourcePath, failedItem.sourcePath],
      startImport: async () =>
        batch("batch-failed", [successfulItem, failedItem])
    });

    render(<App client={client} />);
    const emptyLibrary = await screen.findByRole("main", {
      name: "空资料库"
    });
    await user.click(
      within(emptyLibrary).getByRole("button", { name: "导入文档" })
    );

    const failedRow = (
      await screen.findByText("损坏.pdf")
    ).closest<HTMLElement>('[role="listitem"]');
    if (!failedRow) {
      throw new Error("无法找到失败项目行。");
    }
    expect(failedRow).toHaveTextContent("失败阶段：copying");
    expect(failedRow).toHaveTextContent("原因：无法读取文件");
    expect(
      screen.queryByRole("button", { name: "重试 正常.pdf" })
    ).toBeNull();

    await user.click(
      within(failedRow).getByRole("button", { name: "重试 损坏.pdf" })
    );
    await waitFor(() => {
      expect(client.calls).toContain("retryImportItem:failed-item");
    });
    expect(
      within(
        screen.getByText("损坏.pdf").closest<HTMLElement>(
          '[role="listitem"]'
        )!
      ).getByText("已导入")
    ).toBeInTheDocument();
  });

  it("imports a selected folder through the batch command", async () => {
    const user = userEvent.setup();
    const folderItem = result("folder-item", "资料.pdf", "imported");
    const client = new FakeBackendClient({
      bootstrap,
      selectedFolder: "C:\\Documents\\待导入",
      startImport: async () => batch("batch-folder", [folderItem])
    });

    render(<App client={client} />);
    await screen.findByRole("heading", { name: "空资料库" });
    await user.click(screen.getByRole("button", { name: "导入文件夹" }));

    expect(await screen.findByText("资料.pdf")).toBeInTheDocument();
    expect(client.calls).toContain("startImport:C:\\Documents\\待导入");
  });
});
