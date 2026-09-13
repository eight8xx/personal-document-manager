import {
  fireEvent,
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
  BatchDocumentOperationRequest,
  BatchDocumentOperationResult,
  BootstrapState,
  CollectionSummary,
  DocumentSummary,
  LibrarySummary,
  TagSummary
} from "./backend/types";

const library: LibrarySummary = {
  id: "library-batch",
  name: "批量整理资料库",
  path: "C:\\Documents\\批量整理资料库",
  createdAt: "2026-09-13T08:00:00Z"
};

const bootstrap: BootstrapState = {
  currentLibrary: library,
  recentLibraries: []
};

const tags: TagSummary[] = [
  { id: "important", name: "重要", documentCount: 1 },
  { id: "archive", name: "归档", documentCount: 0 }
];

const collections: CollectionSummary[] = [
  {
    id: "inbox",
    name: "收件箱",
    parentId: null,
    isInbox: true,
    documentCount: 3
  },
  {
    id: "projects",
    name: "项目",
    parentId: null,
    isInbox: false,
    documentCount: 0
  }
];

function documentFor(
  id: string,
  title: string,
  overrides: Partial<DocumentSummary> = {}
): DocumentSummary {
  return {
    id,
    title,
    description: null,
    documentDate: null,
    fileName: `${title}.txt`,
    fileType: "TXT",
    fileSize: 32,
    contentHash: `hash-${id}`,
    collectionId: "inbox",
    tags: id === "first" ? [tags[0]] : [],
    processingStatus: "ready",
    indexStatus: "searchable",
    errorStage: null,
    errorMessage: null,
    importedAt: "2026-09-13T08:10:00Z",
    sourcePath: `C:\\Sources\\${title}.txt`,
    sourceIdentifier: `c:\\sources\\${title}.txt`,
    lastImportedAt: "2026-09-13T08:10:00Z",
    ...overrides
  };
}

const documents = [
  documentFor("first", "第一份文档"),
  documentFor("second", "第二份文档"),
  documentFor("third", "第三份文档")
];

function createClient() {
  return new FakeBackendClient({
    bootstrap,
    documents,
    collections,
    tags
  });
}

describe("批量整理操作", () => {
  it("supports multi-select in list and grid with select-all and clear controls", async () => {
    const user = userEvent.setup();
    render(<App client={createClient()} />);
    await screen.findByText("第一份文档");

    expect(screen.getByText("已选择 0 项")).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: "选择文档 第一份文档" })
    );
    fireEvent.click(
      screen.getByRole("button", { name: "选择文档 第二份文档" }),
      { ctrlKey: true }
    );
    expect(screen.getByText("已选择 2 项")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "网格视图" }));
    expect(
      screen.getByRole("button", { name: "选择文档 第一份文档" })
    ).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(
      screen.getByRole("button", { name: "选择文档 第三份文档" }),
      { ctrlKey: true }
    );
    expect(screen.getByText("已选择 3 项")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "清空选择" }));
    expect(screen.getByText("已选择 0 项")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "全选当前结果" }));
    expect(screen.getByText("已选择 3 项")).toBeInTheDocument();
  });

  it("supports Ctrl and Meta toggling while preserving the latest selection anchor", async () => {
    const user = userEvent.setup();
    render(<App client={createClient()} />);
    await screen.findByText("第一份文档");

    const first = screen.getByRole("button", {
      name: "选择文档 第一份文档"
    });
    const second = screen.getByRole("button", {
      name: "选择文档 第二份文档"
    });
    const third = screen.getByRole("button", {
      name: "选择文档 第三份文档"
    });

    await user.click(first);
    fireEvent.click(third, { ctrlKey: true });
    expect(screen.getByText("已选择 2 项")).toBeInTheDocument();

    fireEvent.click(second, { metaKey: true });
    fireEvent.click(second, { metaKey: true });
    expect(screen.getByText("已选择 2 项")).toBeInTheDocument();
    expect(second).toHaveAttribute("aria-pressed", "false");

    fireEvent.click(third, { shiftKey: true });
    expect(screen.getByText("已选择 2 项")).toBeInTheDocument();
    expect(first).toHaveAttribute("aria-pressed", "false");
    expect(second).toHaveAttribute("aria-pressed", "true");
    expect(third).toHaveAttribute("aria-pressed", "true");

    await user.click(first);
    expect(screen.getByText("已选择 1 项")).toBeInTheDocument();
    expect(second).toHaveAttribute("aria-pressed", "false");
    expect(third).toHaveAttribute("aria-pressed", "false");

    fireEvent.click(third, { shiftKey: true });
    expect(screen.getByText("已选择 3 项")).toBeInTheDocument();
  });

  it("uses the filtered visible order for Shift selection in list and grid", async () => {
    const user = userEvent.setup();
    const client = new FakeBackendClient({
      bootstrap,
      documents: [
        documentFor("first", "第一份文档"),
        documentFor("second", "第二份文档", {
          fileName: "第二份文档.pdf",
          fileType: "PDF"
        }),
        documentFor("third", "第三份文档")
      ],
      collections,
      tags
    });
    render(<App client={client} />);
    await screen.findByText("第一份文档");

    await user.click(screen.getByRole("button", { name: /^筛选/ }));
    await user.selectOptions(screen.getByLabelText("文件类型"), "TXT");
    expect(screen.queryByText("第二份文档")).not.toBeInTheDocument();

    const first = screen.getByRole("button", {
      name: "选择文档 第一份文档"
    });
    const third = screen.getByRole("button", {
      name: "选择文档 第三份文档"
    });
    await user.click(first);
    fireEvent.click(third, { shiftKey: true });
    expect(screen.getByText("已选择 2 项")).toBeInTheDocument();
    expect(first).toHaveAttribute("aria-pressed", "true");
    expect(third).toHaveAttribute("aria-pressed", "true");

    await user.click(screen.getByRole("button", { name: "清空选择" }));
    await user.click(screen.getByRole("button", { name: "网格视图" }));
    const gridFirst = screen.getByRole("button", {
      name: "选择文档 第一份文档"
    });
    const gridThird = screen.getByRole("button", {
      name: "选择文档 第三份文档"
    });
    await user.click(gridFirst);
    fireEvent.click(gridThird, { shiftKey: true });

    expect(screen.getByText("已选择 2 项")).toBeInTheDocument();
    expect(gridFirst).toHaveAttribute("aria-pressed", "true");
    expect(gridThird).toHaveAttribute("aria-pressed", "true");
    expect(
      screen.queryByRole("button", { name: "选择文档 第二份文档" })
    ).not.toBeInTheDocument();
  });

  it("opens the batch menu and moves selected documents to a collection", async () => {
    const user = userEvent.setup();
    const client = createClient();
    render(<App client={client} />);
    await screen.findByText("第一份文档");

    await user.click(
      screen.getByRole("button", { name: "选择文档 第一份文档" })
    );
    fireEvent.click(
      screen.getByRole("button", { name: "选择文档 第二份文档" }),
      { ctrlKey: true }
    );
    await user.click(screen.getByRole("button", { name: "批量操作" }));
    await user.click(
      screen.getByRole("menuitem", { name: "移动到集合" })
    );

    const dialog = screen.getByRole("dialog", {
      name: "批量移动到集合"
    });
    expect(dialog).toHaveTextContent("已选择 2 份文档");
    await user.selectOptions(
      within(dialog).getByLabelText("目标集合"),
      "projects"
    );
    await user.click(
      within(dialog).getByRole("button", { name: "确认移动" })
    );

    const result = await screen.findByRole("region", {
      name: "批量操作结果"
    });
    expect(result).toHaveTextContent("成功 2 份");
    expect(client.calls.some((call) => call.startsWith("batchOrganizeDocuments:")))
      .toBe(true);
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /项目 2/ })).toBeInTheDocument();
    });
  });

  it("confirms before moving selected documents to trash", async () => {
    const user = userEvent.setup();
    render(<App client={createClient()} />);
    await screen.findByText("第一份文档");

    await user.click(
      screen.getByRole("button", { name: "选择文档 第一份文档" })
    );
    fireEvent.click(
      screen.getByRole("button", { name: "选择文档 第二份文档" }),
      { ctrlKey: true }
    );
    await user.click(screen.getByRole("button", { name: "批量操作" }));
    await user.click(
      screen.getByRole("menuitem", { name: "移入回收站" })
    );

    const dialog = screen.getByRole("dialog", {
      name: "批量移入回收站"
    });
    expect(dialog).toHaveTextContent("将选中的 2 份文档移入回收站");
    expect(dialog).toHaveTextContent("资料库副本会保留");
    await user.click(
      within(dialog).getByRole("button", { name: "移入回收站" })
    );

    expect(
      await screen.findByRole("region", { name: "批量操作结果" })
    ).toHaveTextContent("成功 2 份");
    expect(screen.queryByText("第一份文档")).not.toBeInTheDocument();
    expect(screen.queryByText("第二份文档")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /回收站 2/ })).toBeInTheDocument();
  });

  it("lists partial failures and retries only the failed documents", async () => {
    const user = userEvent.setup();
    const client = createClient();
    const originalBatch = client.batchOrganizeDocuments.bind(client);
    let attempts = 0;
    client.batchOrganizeDocuments = async (
      request: BatchDocumentOperationRequest
    ): Promise<BatchDocumentOperationResult> => {
      attempts += 1;
      if (attempts === 1) {
        const [firstId, secondId] = request.documentIds;
        await client.moveDocumentToCollection(firstId, "projects");
        return {
          jobId: request.jobId,
          operation: request.operation,
          results: [
            {
              documentId: firstId,
              status: "succeeded",
              errorCode: null,
              errorMessage: null
            },
            {
              documentId: secondId,
              status: "failed",
              errorCode: "documentBusy",
              errorMessage: "文档暂时无法移动。"
            }
          ],
          succeededCount: 1,
          failedCount: 1,
          cancelledCount: 0
        };
      }
      return originalBatch(request);
    };

    render(<App client={client} />);
    await screen.findByText("第一份文档");
    await user.click(
      screen.getByRole("button", { name: "选择文档 第一份文档" })
    );
    fireEvent.click(
      screen.getByRole("button", { name: "选择文档 第二份文档" }),
      { ctrlKey: true }
    );
    await user.click(screen.getByRole("button", { name: "批量操作" }));
    await user.click(
      screen.getByRole("menuitem", { name: "移动到集合" })
    );
    await user.selectOptions(screen.getByLabelText("目标集合"), "projects");
    await user.click(screen.getByRole("button", { name: "确认移动" }));

    const result = await screen.findByRole("region", {
      name: "批量操作结果"
    });
    expect(result).toHaveTextContent("成功 1 份，失败 1 份");
    const failures = within(result).getByRole("list", {
      name: "失败文档列表"
    });
    expect(within(failures).getByText("第二份文档")).toBeInTheDocument();
    expect(within(failures).getByText("文档暂时无法移动。")).toBeInTheDocument();
    expect(screen.getByText("已选择 1 项")).toBeInTheDocument();

    await user.click(
      within(result).getByRole("button", { name: "重试失败项目" })
    );

    await waitFor(() => {
      expect(
        screen.getByRole("region", { name: "批量操作结果" })
      ).toHaveTextContent("成功 1 份");
    });
    expect(
      screen.queryByRole("list", { name: "失败文档列表" })
    ).not.toBeInTheDocument();
    expect(attempts).toBe(2);
  });

  it("prevents duplicate submission and returns cancelled items", async () => {
    const user = userEvent.setup();
    const client = createClient();
    let resolveBatch:
      | ((result: BatchDocumentOperationResult) => void)
      | undefined;
    let cancellationRequested = false;
    client.batchOrganizeDocuments = async (request) =>
      new Promise<BatchDocumentOperationResult>((resolve) => {
        resolveBatch = resolve;
        void request;
      });
    client.cancelBatchDocumentOperation = async () => {
      cancellationRequested = true;
      const request = {
        jobId: "pending",
        documentIds: ["first", "second"]
      };
      resolveBatch?.({
        jobId: "pending",
        operation: { kind: "moveToTrash" },
        results: request.documentIds.map((documentId, index) => ({
          documentId,
          status: index === 0 ? "succeeded" : "cancelled",
          errorCode: null,
          errorMessage: null
        })),
        succeededCount: 1,
        failedCount: 0,
        cancelledCount: 1
      });
      return true;
    };

    render(<App client={client} />);
    await screen.findByText("第一份文档");
    await user.click(
      screen.getByRole("button", { name: "选择文档 第一份文档" })
    );
    fireEvent.click(
      screen.getByRole("button", { name: "选择文档 第二份文档" }),
      { ctrlKey: true }
    );
    await user.click(screen.getByRole("button", { name: "批量操作" }));
    await user.click(
      screen.getByRole("menuitem", { name: "移入回收站" })
    );
    await user.click(
      within(
        screen.getByRole("dialog", { name: "批量移入回收站" })
      ).getByRole("button", { name: "移入回收站" })
    );

    expect(
      await screen.findByText("正在处理 2 份文档")
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "批量操作" })).toBeDisabled();
    await user.click(
      screen.getByRole("button", { name: "取消未执行项目" })
    );

    const result = await screen.findByRole("region", {
      name: "批量操作结果"
    });
    expect(result).toHaveTextContent("成功 1 份，取消 1 份");
    expect(cancellationRequested).toBe(true);
  });
});
