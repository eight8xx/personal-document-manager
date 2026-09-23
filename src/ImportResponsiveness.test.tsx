import { act, render, screen, waitFor, within } from "@testing-library/react";
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
  LibrarySummary
} from "./backend/types";

/**
 * 工作单 05 第 4 条的界面一半：批量导入进行中，进度面板与查询结果必须在
 * 同一渲染树里同时更新，读取不因导入尚未结束而被清空或阻塞。
 *
 * 用例全部用「受控未完成的 Promise」驱动导入：只要它在断言时仍未 resolve，
 * 「查询先于导入完成」就是可复现的，不依赖 sleep 猜时序。
 */

const library: LibrarySummary = {
  id: "library-responsive",
  name: "响应性资料库",
  path: "C:\\Documents\\响应性资料库",
  createdAt: "2026-09-23T08:00:00Z"
};

const bootstrap: BootstrapState = {
  currentLibrary: library,
  recentLibraries: []
};

const collections: CollectionSummary[] = [
  { id: "inbox", name: "收件箱", parentId: null, isInbox: true, documentCount: 1 }
];

function document(
  id: string,
  title: string,
  overrides: Partial<DocumentSummary> = {}
): DocumentSummary {
  return {
    id,
    title,
    description: null,
    documentDate: "2026-01-01",
    fileName: `${title}.txt`,
    fileType: "TXT",
    fileSize: 128,
    contentHash: `hash-${id}`,
    collectionId: "inbox",
    tags: [],
    processingStatus: "ready",
    indexStatus: "searchable",
    errorStage: null,
    errorMessage: null,
    importedAt: "2026-09-23T08:10:00Z",
    sourcePath: `C:\\Sources\\${title}.txt`,
    sourceIdentifier: `c:\\sources\\${title}.txt`,
    lastImportedAt: "2026-09-23T08:10:00Z",
    ...overrides
  };
}

const existingDocument = document("document-existing", "项目说明");

function importedItem(itemId: string, fileName: string): ImportItemResult {
  return {
    itemId,
    sourcePath: `C:\\Sources\\${fileName}`,
    fileName,
    fileType: fileName.split(".").at(-1)?.toUpperCase() ?? null,
    status: "imported",
    documentId: `document-${itemId}`,
    duplicateDocumentId: null,
    errorStage: null,
    errorMessage: null,
    retryable: false,
    targetCollectionId: null,
    collectionId: "inbox",
    notice: null
  };
}

function batch(batchId: string, items: ImportItemResult[]): ImportBatch {
  return {
    batchId,
    items,
    importedCount: items.filter((item) => item.status === "imported").length,
    duplicateCount: 0,
    sourceChangedCount: 0,
    failedCount: 0,
    ignoredCount: 0,
    targetCollectionId: null
  };
}

/** 受控导入：`startImport` 只有测试显式放行时才 resolve。 */
function controlledImport() {
  let resolveBatch: ((value: ImportBatch) => void) | undefined;
  let settled = false;
  return {
    startImport: () =>
      new Promise<ImportBatch>((resolve) => {
        resolveBatch = (value: ImportBatch) => {
          settled = true;
          resolve(value);
        };
      }),
    resolve(value: ImportBatch) {
      if (!resolveBatch) {
        throw new Error("导入尚未开始，无法放行批次。");
      }
      resolveBatch(value);
    },
    isSettled: () => settled
  };
}

function createClient(
  importControl: ReturnType<typeof controlledImport>,
  options: { documentContents?: Record<string, string> } = {}
) {
  const first = importedItem("item-1", "季度报表.xlsx");
  const second = importedItem("item-2", "年度资料.txt");
  const client = new FakeBackendClient({
    bootstrap,
    documents: [existingDocument],
    collections,
    selectedDocuments: [first.sourcePath, second.sourcePath],
    documentContents: options.documentContents ?? {
      "document-existing": "年度资料正文包含关键结论。"
    },
    startImport: importControl.startImport
  });
  return { client, first, second };
}

async function startControlledImport(
  user: ReturnType<typeof userEvent.setup>,
  client: FakeBackendClient
) {
  render(<App client={client} />);
  await screen.findByText(existingDocument.title);
  await waitFor(() => {
    expect(client.calls).toContain("subscribeToImportProgress");
  });

  await user.click(screen.getAllByRole("button", { name: "导入文档" })[0]);
  await waitFor(() => {
    expect(client.calls.some((call) => call.startsWith("startImport:"))).toBe(
      true
    );
  });
}

function progressPanel() {
  return screen.getByRole("region", { name: "批量导入进度" });
}

describe("批量导入期间的查询响应性", () => {
  it("updates progress and query results together while the import is unfinished", async () => {
    const user = userEvent.setup();
    const importControl = controlledImport();
    const { client, first, second } = createClient(importControl);
    await startControlledImport(user, client);

    // 导入进行中：进度事件到达，面板显示已完成项数与当前文件。
    act(() => {
      client.emitImportProgress({
        library,
        batchId: "batch-live",
        total: 2,
        completed: 1,
        currentFileName: first.fileName,
        currentSourcePath: first.sourcePath,
        item: first,
        finished: false
      });
    });

    expect(await screen.findByText("已完成 1/2")).toBeInTheDocument();
    expect(within(progressPanel()).getByText("正在导入")).toBeInTheDocument();
    expect(
      within(progressPanel()).getByText(`当前文件：${first.fileName}`)
    ).toBeInTheDocument();
    const itemRows = within(progressPanel()).getAllByRole("listitem");
    expect(itemRows).toHaveLength(1);
    expect(itemRows[0]).toHaveTextContent(first.fileName);
    expect(itemRows[0]).toHaveTextContent("已导入");
    expect(importControl.isSettled()).toBe(false);

    // 导入仍在进行中时发起查询：结果必须出现，无需等待导入结束。
    await user.type(screen.getByRole("searchbox", { name: "搜索文档" }), "年度资料");

    expect(
      await screen.findByText(/年度资料正文包含关键结论。/)
    ).toBeInTheDocument();
    expect(client.calls).toContain("searchDocuments:年度资料");
    // 读取完成时导入仍未结束：进度面板与查询结果同时可见。
    expect(importControl.isSettled()).toBe(false);
    expect(screen.getByRole("main", { name: "文档列表" })).toBeInTheDocument();
    expect(within(progressPanel()).getByText("正在导入")).toBeInTheDocument();
    expect(screen.queryByText("导入批次已完成")).not.toBeInTheDocument();

    // 后续进度事件继续更新，且不会清空查询结果。
    act(() => {
      client.emitImportProgress({
        library,
        batchId: "batch-live",
        total: 2,
        completed: 2,
        currentFileName: second.fileName,
        currentSourcePath: second.sourcePath,
        item: second,
        finished: false
      });
    });

    expect(await screen.findByText("已完成 2/2")).toBeInTheDocument();
    expect(
      within(progressPanel()).getByText(`当前文件：${second.fileName}`)
    ).toBeInTheDocument();
    expect(screen.getByText(/年度资料正文包含关键结论。/)).toBeInTheDocument();

    // 放行导入：进度面板切换到最终状态，查询结果仍正确。
    await act(async () => {
      importControl.resolve(batch("batch-live", [first, second]));
    });

    expect(await screen.findByText("导入批次已完成")).toBeInTheDocument();
    expect(screen.getByLabelText("导入结果汇总")).toHaveTextContent("已导入 2");
    expect(importControl.isSettled()).toBe(true);
    expect(screen.getByText(/年度资料正文包含关键结论。/)).toBeInTheDocument();
    expect(screen.getByRole("main", { name: "文档列表" })).toBeInTheDocument();
  });

  it("renders a document list refresh that completes before the import finishes", async () => {
    const user = userEvent.setup();
    const importControl = controlledImport();
    const { client, first } = createClient(importControl);
    const addedDuringImport = document("document-late", "导入期间出现的文档");

    let importStarted = false;
    let gatedReadStarted = false;
    let releaseRead: (() => void) | undefined;
    const listDocuments = client.listDocuments.bind(client);
    client.listDocuments = async () => {
      const items = await listDocuments();
      if (!importStarted) {
        return items;
      }
      // 导入开始之后的读取：由测试放行，用来证明它不必等导入结束。
      gatedReadStarted = true;
      await new Promise<void>((resolve) => {
        releaseRead = resolve;
      });
      return [...items, addedDuringImport];
    };

    await startControlledImport(user, client);
    importStarted = true;
    act(() => {
      client.emitImportProgress({
        library,
        batchId: "batch-read",
        total: 2,
        completed: 1,
        currentFileName: first.fileName,
        currentSourcePath: first.sourcePath,
        item: first,
        finished: false
      });
    });
    expect(await screen.findByText("已完成 1/2")).toBeInTheDocument();

    // 外部变化触发一次列表读取；此时导入仍未结束。
    act(() => {
      client.emitDocumentIndexChanged({
        library,
        phase: "completed",
        documentIds: [],
        result: null
      });
    });
    await waitFor(() => {
      expect(gatedReadStarted).toBe(true);
    });
    expect(importControl.isSettled()).toBe(false);

    await act(async () => {
      releaseRead?.();
    });

    expect(await screen.findByText(addedDuringImport.title)).toBeInTheDocument();
    // 列表读取已经返回，导入依然在跑：读取没有排队到导入之后。
    expect(importControl.isSettled()).toBe(false);
    expect(within(progressPanel()).getByText("正在导入")).toBeInTheDocument();
    expect(screen.getByText(existingDocument.title)).toBeInTheDocument();

    await act(async () => {
      importControl.resolve(batch("batch-read", [first]));
    });
    expect(await screen.findByText("导入批次已完成")).toBeInTheDocument();
    expect(screen.getByText(addedDuringImport.title)).toBeInTheDocument();
  });

  it("keeps existing search results when a batch import starts", async () => {
    const user = userEvent.setup();
    const importControl = controlledImport();
    const { client, first } = createClient(importControl);

    render(<App client={client} />);
    await screen.findByText(existingDocument.title);

    const search = screen.getByRole("searchbox", { name: "搜索文档" });
    await user.type(search, "年度资料");
    expect(
      await screen.findByText(/年度资料正文包含关键结论。/)
    ).toBeInTheDocument();

    await user.click(screen.getAllByRole("button", { name: "导入文档" })[0]);
    await waitFor(() => {
      expect(client.calls.some((call) => call.startsWith("startImport:"))).toBe(
        true
      );
    });

    act(() => {
      client.emitImportProgress({
        library,
        batchId: "batch-keep",
        total: 2,
        completed: 1,
        currentFileName: first.fileName,
        currentSourcePath: first.sourcePath,
        item: first,
        finished: false
      });
    });

    // 导入开始后，之前的查询结果仍在，且与进度面板共存。
    expect(await screen.findByText("已完成 1/2")).toBeInTheDocument();
    expect(screen.getByText(/年度资料正文包含关键结论。/)).toBeInTheDocument();
    expect(importControl.isSettled()).toBe(false);

    await act(async () => {
      importControl.resolve(batch("batch-keep", [first]));
    });
    expect(await screen.findByText("导入批次已完成")).toBeInTheDocument();
    expect(screen.getByText(/年度资料正文包含关键结论。/)).toBeInTheDocument();
  });
});
