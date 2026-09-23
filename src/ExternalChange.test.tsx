import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { App } from "./App";
import { FakeBackendClient } from "./backend/fakeClient";
import type {
  BootstrapState,
  CollectionSummary,
  DocumentSummary,
  LibrarySummary
} from "./backend/types";

const library: LibrarySummary = {
  id: "library-external",
  name: "外部变化资料库",
  path: "C:\\Documents\\外部变化资料库",
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
    documentCount: 4
  }
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
    documentDate: null,
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
    importedAt: "2026-09-13T08:10:00Z",
    sourcePath: `C:\\Sources\\${title}.txt`,
    sourceIdentifier: `c:\\sources\\${title}.txt`,
    lastImportedAt: "2026-09-13T08:10:00Z",
    ...overrides
  };
}

describe("外部变化重索引状态", () => {
  it("ignores an old library's index event after switching to another library", async () => {
    const user = userEvent.setup();
    const secondPath = "D:\\Archive";
    const currentDocument = document("shared-document-id", "当前资料");
    const client = new FakeBackendClient({
      bootstrap: {
        currentLibrary: library,
        recentLibraries: [
          {
            path: library.path,
            name: library.name,
            lastOpenedAt: "2026-09-13T08:00:00Z",
            isAvailable: true
          },
          {
            path: secondPath,
            name: "归档",
            lastOpenedAt: "2026-09-12T08:00:00Z",
            isAvailable: true
          }
        ]
      },
      collections,
      documents: [currentDocument]
    });
    const openLibrary = client.openLibrary.bind(client);
    let openedLibrary: LibrarySummary | null = null;
    client.openLibrary = async (path) => {
      openedLibrary = await openLibrary(path);
      return openedLibrary;
    };

    render(<App client={client} />);
    await screen.findByText("当前资料");
    await user.click(screen.getByRole("button", { name: "设置" }));
    const settings = screen.getByRole("dialog", { name: "资料库" });
    await user.click(within(settings).getByRole("button", { name: "切换" }));
    await waitFor(() => expect(client.calls).toContain(`open:${secondPath}`));
    const results = await screen.findByRole("main", { name: "文档列表" });
    expect(within(results).getByText("可搜索")).toBeInTheDocument();

    act(() => {
      client.emitDocumentIndexChanged({
        library,
        phase: "processing",
        documentIds: [currentDocument.id],
        result: null
      });
    });
    expect(within(results).getByText("可搜索")).toBeInTheDocument();

    act(() => {
      client.emitDocumentIndexChanged({
        library: openedLibrary!,
        phase: "processing",
        documentIds: [currentDocument.id],
        result: null
      });
    });
    expect(await within(results).findByText("处理中")).toBeInTheDocument();
  });

  it("shows processing, searchable, failed retry and waiting states", async () => {
    const client = new FakeBackendClient({
      bootstrap,
      collections,
      documents: [
        document("processing", "处理中资料", {
          processingStatus: "processing",
          indexStatus: "pending"
        }),
        document("searchable", "可搜索资料"),
        document("failed", "失败资料", {
          processingStatus: "failed",
          indexStatus: "failed",
          errorStage: "externalValidation",
          errorMessage: "文件内容已经损坏。"
        }),
        document("pending", "等待资料", {
          indexStatus: "pending"
        })
      ],
      indexPendingDocuments: () => new Promise(() => {})
    });
    client.pendingIndexCount = async () => 0;

    render(<App client={client} />);
    const results = await screen.findByRole("main", { name: "文档列表" });
    const statusFor = (title: string) =>
      within(
        within(results).getByRole("row", {
          name: new RegExp(title)
        })
      );

    expect(statusFor("处理中资料").getByText("处理中")).toBeInTheDocument();
    expect(statusFor("可搜索资料").getByText("可搜索")).toBeInTheDocument();
    expect(
      statusFor("失败资料").getByText("处理失败，等待重试")
    ).toBeInTheDocument();
    expect(statusFor("等待资料").getByText("等待索引")).toBeInTheDocument();
  });

  it("refreshes the document list when indexing completes", async () => {
    const processing = document("changed", "外部修改资料", {
      processingStatus: "processing",
      indexStatus: "pending"
    });
    const refreshed = {
      ...processing,
      fileSize: 256,
      contentHash: "hash-refreshed",
      processingStatus: "ready" as const,
      indexStatus: "searchable" as const
    };
    const client = new FakeBackendClient({
      bootstrap,
      collections,
      documents: [processing]
    });

    render(<App client={client} />);
    const results = await screen.findByRole("main", { name: "文档列表" });
    expect(within(results).getByText("处理中")).toBeInTheDocument();
    await waitFor(() =>
      expect(client.calls).toContain("subscribeToDocumentIndexChanges")
    );

    client.setDocument(refreshed);
    await act(async () => {
      client.emitDocumentIndexChanged({
        library,
        phase: "completed",
        documentIds: [],
        result: { processed: 1, searchable: 1, failed: 0 }
      });
    });

    expect(await within(results).findByText("可搜索")).toBeInTheDocument();
  });

  it("marks documents processing for id-only events and refreshes on completion", async () => {
    const original = document("changed", "无结果事件资料");
    const refreshed = {
      ...original,
      contentHash: "hash-id-only-refresh",
      processingStatus: "ready" as const,
      indexStatus: "searchable" as const
    };
    const client = new FakeBackendClient({
      bootstrap,
      collections,
      documents: [original]
    });

    render(<App client={client} />);
    const results = await screen.findByRole("main", { name: "文档列表" });
    expect(within(results).getByText("可搜索")).toBeInTheDocument();
    await waitFor(() =>
      expect(client.calls).toContain("subscribeToDocumentIndexChanges")
    );

    act(() => {
      client.emitDocumentIndexChanged({
        library,
        phase: "processing",
        documentIds: [original.id],
        result: null
      });
    });
    expect(await within(results).findByText("处理中")).toBeInTheDocument();

    client.setDocument(refreshed);
    await act(async () => {
      client.emitDocumentIndexChanged({
        library,
        phase: "completed",
        documentIds: [],
        result: { processed: 1, searchable: 1, failed: 0 }
      });
    });

    await waitFor(() =>
      expect(within(results).getByText("可搜索")).toBeInTheDocument()
    );
  });

  it("keeps the failure visible when retry still cannot index", async () => {
    const user = userEvent.setup();
    const failed = document("failed", "仍需修复的资料", {
      processingStatus: "failed",
      indexStatus: "failed",
      errorStage: "externalRead",
      errorMessage: "资料库副本不存在。"
    });
    const client = new FakeBackendClient({
      bootstrap,
      collections,
      documents: [failed],
      retryDocumentIndex: async () => ({
        ...failed,
        errorStage: "externalRead",
        errorMessage: "资料库副本仍然无法读取。"
      })
    });

    render(<App client={client} />);
    const results = await screen.findByRole("main", { name: "文档列表" });
    await user.click(
      within(results).getByRole("button", {
        name: "选择文档 仍需修复的资料"
      })
    );
    expect(await screen.findByText("资料库副本不存在。")).toBeInTheDocument();
    await user.click(
      within(results).getByRole("button", {
        name: "重试索引 仍需修复的资料"
      })
    );

    expect(
      await screen.findByText("资料库副本仍然无法读取。")
    ).toBeInTheDocument();
    expect(
      within(results).getByText("处理失败，等待重试")
    ).toBeInTheDocument();
  });
});
