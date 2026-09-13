import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { App } from "./App";
import { BackendError } from "./backend/error";
import { FakeBackendClient } from "./backend/fakeClient";
import type {
  BootstrapState,
  CollectionSummary,
  DocumentSummary,
  LibrarySummary,
  TagSummary
} from "./backend/types";

const library: LibrarySummary = {
  id: "library-search",
  name: "搜索资料库",
  path: "C:\\Documents\\搜索资料库",
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
    documentCount: 1
  },
  {
    id: "projects",
    name: "项目",
    parentId: null,
    isInbox: false,
    documentCount: 1
  }
];

const tags: TagSummary[] = [
  { id: "work", name: "工作", documentCount: 1 }
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
    documentDate: "2025-06-01",
    fileName: `${title}.txt`,
    fileType: "TXT",
    fileSize: 128,
    contentHash: `hash-${id}`,
    collectionId: "projects",
    tags: [tags[0]],
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

describe("全文搜索与筛选", () => {
  it("searches body content, shows a matching snippet and supports short metadata queries", async () => {
    const user = userEvent.setup();
    const annual = document("annual", "年度资料");
    const ai = document("ai", "AI 方案");
    const client = new FakeBackendClient({
      bootstrap,
      documents: [annual, ai],
      collections,
      tags,
      documentContents: {
        annual: "这里包含年度报告正文，以及 mixed content。"
      }
    });

    render(<App client={client} />);
    await screen.findByText("年度资料");

    const search = screen.getByRole("searchbox", { name: "搜索文档" });
    await user.type(search, "年度报告");

    expect(await screen.findByText(/这里包含年度报告正文/)).toBeInTheDocument();
    expect(client.calls).toContain("searchDocuments:年度报告");

    await user.clear(search);
    await user.type(search, "AI");
    expect(await screen.findByText("AI 方案")).toBeInTheDocument();
    expect(screen.queryByText("年度资料")).not.toBeInTheDocument();
  });

  it("combines collection, tag, file type and document date filters and clears them", async () => {
    const user = userEvent.setup();
    const matching = document("matching", "筛选命中");
    const other = document("other", "其他文档", {
      documentDate: "2024-01-01",
      fileName: "其他文档.pdf",
      fileType: "PDF",
      collectionId: "inbox",
      tags: []
    });
    const client = new FakeBackendClient({
      bootstrap,
      documents: [matching, other],
      collections,
      tags
    });

    render(<App client={client} />);
    await screen.findByText("筛选命中");
    await user.click(screen.getByRole("button", { name: /^筛选/ }));

    await user.selectOptions(screen.getByLabelText("集合筛选"), "projects");
    await user.selectOptions(screen.getByLabelText("标签筛选"), "work");
    await user.selectOptions(screen.getByLabelText("文件类型"), "TXT");
    await user.clear(screen.getByLabelText("文档日期从"));
    await user.type(screen.getByLabelText("文档日期从"), "2025-01-01");
    await user.clear(screen.getByLabelText("文档日期至"));
    await user.type(screen.getByLabelText("文档日期至"), "2025-12-31");

    await waitFor(() => {
      expect(screen.queryByText("其他文档")).not.toBeInTheDocument();
    });
    expect(screen.getByText("筛选命中")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "清除筛选" }));
    expect(await screen.findByText("其他文档")).toBeInTheDocument();
    expect(screen.getByText("筛选命中")).toBeInTheDocument();
  });

  it("shows a distinct empty result state for an unmatched query", async () => {
    const user = userEvent.setup();
    const client = new FakeBackendClient({
      bootstrap,
      documents: [document("annual", "年度资料")],
      collections,
      tags,
      documentContents: { annual: "只有年度资料正文" }
    });

    render(<App client={client} />);
    await screen.findByText("年度资料");
    await user.type(
      screen.getByRole("searchbox", { name: "搜索文档" }),
      "完全不存在"
    );

    expect(
      await screen.findByRole("main", { name: "无搜索结果" })
    ).toHaveTextContent("没有找到匹配的文档");
  });

  it("shows a retryable error state when search fails", async () => {
    const user = userEvent.setup();
    let attempts = 0;
    const client = new FakeBackendClient({
      bootstrap,
      documents: [document("annual", "年度资料")],
      collections,
      tags,
      searchDocuments: async () => {
        attempts += 1;
        if (attempts === 1) {
          throw new BackendError({
            code: "searchTask",
            message: "搜索服务暂时不可用。"
          });
        }
        return {
          results: [
            {
              document: document("annual", "年度资料"),
              snippet: "重试后的年度资料正文",
              matchKind: "content"
            }
          ]
        };
      }
    });

    render(<App client={client} />);
    await screen.findByText("年度资料");
    await user.type(
      screen.getByRole("searchbox", { name: "搜索文档" }),
      "年度"
    );

    const error = await screen.findByRole("main", { name: "搜索失败" });
    expect(error).toHaveTextContent("搜索服务暂时不可用");
    await user.click(within(error).getByRole("button", { name: "重试搜索" }));
    expect(await screen.findByText("重试后的年度资料正文")).toBeInTheDocument();
  });

  it("keeps a failed document browsable and retries its index", async () => {
    const user = userEvent.setup();
    const failed = document("failed", "索引失败资料", {
      indexStatus: "failed",
      errorStage: "indexing",
      errorMessage: "资料库副本不存在"
    });
    const client = new FakeBackendClient({
      bootstrap,
      documents: [failed],
      collections,
      tags
    });

    render(<App client={client} />);
    await screen.findByText("索引失败资料");
    await user.click(
      screen.getByRole("button", { name: "重试索引 索引失败资料" })
    );

    await waitFor(() => {
      expect(client.calls).toContain("retryDocumentIndex:failed");
    });
    expect(await screen.findByText("可搜索")).toBeInTheDocument();
  });
});
