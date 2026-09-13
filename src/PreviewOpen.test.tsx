import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { parseAsync, renderDocument } from "docx-preview";
import {
  afterEach,
  beforeEach,
  describe,
  expect,
  it,
  vi
} from "vitest";

import { App } from "./App";
import { BackendError } from "./backend/error";
import { FakeBackendClient } from "./backend/fakeClient";
import type {
  BootstrapState,
  DocumentSummary,
  LibrarySummary
} from "./backend/types";

vi.mock("docx-preview", () => ({
  parseAsync: vi.fn(),
  renderDocument: vi.fn()
}));

const parseDocxMock = vi.mocked(parseAsync);
const renderDocxMock = vi.mocked(renderDocument);

const library: LibrarySummary = {
  id: "library-preview",
  name: "预览资料库",
  path: "C:\\Documents\\预览资料库",
  createdAt: "2026-09-13T08:00:00Z"
};

const bootstrap: BootstrapState = {
  currentLibrary: library,
  recentLibraries: []
};

function documentFor(
  id: string,
  title: string,
  fileType: string
): DocumentSummary {
  const extension = fileType === "Markdown" ? "md" : fileType.toLowerCase();
  return {
    id,
    title,
    description: null,
    documentDate: null,
    fileName: `${title}.${extension}`,
    fileType,
    fileSize: 128,
    contentHash: `hash-${id}`,
    collectionId: "inbox",
    tags: [],
    processingStatus: "ready",
    indexStatus: "searchable",
    errorStage: null,
    errorMessage: null,
    importedAt: "2026-09-13T08:10:00Z",
    sourcePath: `C:\\Sources\\${title}.${extension}`,
    sourceIdentifier: `c:\\sources\\${title}.${extension}`,
    lastImportedAt: "2026-09-13T08:10:00Z"
  };
}

const documents = [
  documentFor("pdf", "年度报告", "PDF"),
  documentFor("image", "扫描图", "PNG"),
  documentFor("text", "纯文本", "TXT"),
  documentFor("markdown", "项目说明", "Markdown"),
  documentFor("docx", "会议记录", "DOCX")
];

afterEach(() => {
  cleanup();
  window.sessionStorage.clear();
});

beforeEach(() => {
  parseDocxMock.mockReset();
  renderDocxMock.mockReset();
  parseDocxMock.mockResolvedValue({});
  renderDocxMock.mockResolvedValue([
    Object.assign(document.createElement("div"), {
      className: "docx-wrapper",
      innerHTML:
        '<section class="docx" style="width: 800px; min-height: 1000px"><article><p>会议记录版式内容</p></article></section>'
    })
  ]);
});

describe("文档预览与外部打开", () => {
  it("shows loading and renders PDF, image, text, Markdown and DOCX previews", async () => {
    let resolvePreview:
      | ((value: Awaited<ReturnType<FakeBackendClient["getDocumentPreview"]>>) => void)
      | undefined;
    let firstPreviewPending = true;
    const client = new FakeBackendClient({
      bootstrap,
      documents,
      documentPreviews: {
        pdf: {
          kind: "pdf",
          dataUrl: "data:image/png;base64,page-one",
          pageCount: 3,
          page: 1
        }
      }
    });
    client.getDocumentPreview = (documentId, page) => {
      if (documentId === "pdf" && firstPreviewPending) {
        firstPreviewPending = false;
        return new Promise((resolve) => {
          resolvePreview = resolve;
        });
      }
      return FakeBackendClient.prototype.getDocumentPreview.call(
        client,
        documentId,
        page
      );
    };

    const user = userEvent.setup();
    render(<App client={client} />);
    await screen.findByText("年度报告");

    await user.click(
      screen.getByRole("button", { name: "选择文档 年度报告" })
    );
    expect(screen.getByRole("status")).toHaveTextContent("正在加载预览");

    await act(async () => {
      resolvePreview?.({
        kind: "pdf",
        dataUrl: "data:image/png;base64,page-one",
        pageCount: 3,
        page: 1
      });
    });

    const pdfPage = await screen.findByRole("img", {
      name: "年度报告 第 1 页预览"
    });
    expect(pdfPage).toHaveAttribute(
      "src",
      "data:image/png;base64,page-one"
    );
    await user.click(screen.getByRole("button", { name: "PDF 下一页" }));
    expect(
      await screen.findByRole("img", {
        name: "年度报告 第 2 页预览"
      })
    ).toBeInTheDocument();
    expect(screen.getByText("第 2 页 / 3")).toBeInTheDocument();

    await user.click(
      screen.getByRole("button", { name: "选择文档 扫描图" })
    );
    const image = await screen.findByRole("img", { name: "扫描图 预览" });
    expect(image).toHaveAttribute("src", expect.stringContaining("image/png"));
    expect(
      screen.queryByRole("img", { name: "年度报告 第 2 页预览" })
    ).not.toBeInTheDocument();

    await user.click(
      screen.getByRole("button", { name: "选择文档 纯文本" })
    );
    expect(
      await screen.findByText("纯文本 的只读预览文本")
    ).toBeInTheDocument();

    await user.click(
      screen.getByRole("button", { name: "选择文档 项目说明" })
    );
    expect(
      await screen.findByText("项目说明 的只读预览文本")
    ).toBeInTheDocument();

    await user.click(
      screen.getByRole("button", { name: "选择文档 会议记录" })
    );
    expect(
      await screen.findByText("会议记录版式内容")
    ).toBeInTheDocument();
    expect(
      screen.getByText("DOCX 版式预览为本地只读近似呈现。")
    ).toBeInTheDocument();
  });

  it("falls back to a type icon when a thumbnail fails without blocking preview or open", async () => {
    const client = new FakeBackendClient({
      bootstrap,
      documents: documents.slice(0, 2),
      getDocumentThumbnail: async () => {
        throw new BackendError({
          code: "thumbnailFailed",
          message: "缩略图生成失败。"
        });
      }
    });
    const user = userEvent.setup();
    const { container } = render(<App client={client} />);
    await screen.findByText("年度报告");

    await user.click(screen.getByRole("button", { name: "网格视图" }));
    await waitFor(() => {
      expect(
        client.calls.filter((call) =>
          call.startsWith("getDocumentThumbnail:")
        )
      ).toHaveLength(2);
    });
    expect(
      container.querySelectorAll(".document-grid-visual > svg")
    ).toHaveLength(2);

    await user.click(
      screen.getByRole("button", { name: "选择文档 年度报告" })
    );
    expect(
      await screen.findByRole("img", { name: "年度报告 第 1 页预览" })
    ).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", {
        name: "用系统默认程序打开 年度报告"
      })
    );
    expect(client.calls).toContain("openDocument:pdf");
  });

  it("renders generated image data for PDF and image thumbnails and unmounts them with the grid", async () => {
    const client = new FakeBackendClient({
      bootstrap,
      documents: documents.slice(0, 2)
    });
    const user = userEvent.setup();
    const { container } = render(<App client={client} />);
    await screen.findByText("年度报告");

    await user.click(screen.getByRole("button", { name: "网格视图" }));
    await waitFor(() => {
      expect(
        container.querySelectorAll(".document-grid-thumbnail")
      ).toHaveLength(2);
    });
    const thumbnails = Array.from(
      container.querySelectorAll<HTMLImageElement>(".document-grid-thumbnail")
    );
    expect(thumbnails.map((thumbnail) => thumbnail.dataset.thumbnailKind)).toEqual([
      "pdf",
      "image"
    ]);
    for (const thumbnail of thumbnails) {
      expect(thumbnail.src).toContain("data:image/png");
    }
    expect(container.querySelector("iframe")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "列表视图" }));
    expect(container.querySelector(".document-grid-thumbnail")).not.toBeInTheDocument();
  });

  it("uses one consistent type icon for DOCX, TXT and Markdown in the grid", async () => {
    const client = new FakeBackendClient({
      bootstrap,
      documents: [documents[2], documents[3], documents[4]]
    });
    const user = userEvent.setup();
    const { container } = render(<App client={client} />);
    await screen.findByText("纯文本");

    await user.click(screen.getByRole("button", { name: "网格视图" }));

    expect(
      container.querySelectorAll(
        ".document-grid-visual > svg.lucide-file-text"
      )
    ).toHaveLength(3);
    expect(
      client.calls.filter((call) =>
        call.startsWith("getDocumentThumbnail:")
      )
    ).toHaveLength(0);
  });

  it("shows a concrete preview failure and can retry", async () => {
    let attempts = 0;
    const client = new FakeBackendClient({
      bootstrap,
      documents: [documents[2]],
      getDocumentPreview: async () => {
        attempts += 1;
        if (attempts === 1) {
          throw new BackendError({
            code: "documentFileMissing",
            message: "资料库副本不存在：C:\\Library\\missing.txt"
          });
        }
        return { kind: "text", text: "重试后的文本" };
      }
    });
    const user = userEvent.setup();
    render(<App client={client} />);
    await screen.findByText("纯文本");

    await user.click(
      screen.getByRole("button", { name: "选择文档 纯文本" })
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "资料库副本不存在"
    );

    await user.click(screen.getByRole("button", { name: "重试预览" }));
    expect(await screen.findByText("重试后的文本")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("renders Markdown as text, leaves raw HTML inert, and opens links only after a click", async () => {
    const user = userEvent.setup();
    const client = new FakeBackendClient({
      bootstrap,
      documents: [documents[3]],
      getDocumentPreview: async () => ({
        kind: "markdown",
        text: "<script>window.fail = true</script>\n[打开资料](https://example.com/report)\n[javascript](javascript:alert(1))"
      })
    });
    render(<App client={client} />);
    await screen.findByText("项目说明");

    await user.click(
      screen.getByRole("button", { name: "选择文档 项目说明" })
    );
    expect(
      await screen.findByText(/<script>window\.fail = true<\/script>/)
    ).toBeInTheDocument();

    const safeLink = screen.getByRole("button", { name: "打开资料" });
    expect(client.calls).not.toContain(
      "openExternalUrl:https://example.com/report"
    );
    await user.click(safeLink);
    expect(client.calls).toContain(
      "openExternalUrl:https://example.com/report"
    );
    expect(
      screen.queryByRole("button", { name: "javascript" })
    ).not.toBeInTheDocument();
    expect(
      screen.getByText(/\[javascript\]\(javascript:alert\(1\)\)/)
    ).toBeInTheDocument();
  });

  it("keeps metadata and external open available when preview returns a structured failure", async () => {
    const client = new FakeBackendClient({
      bootstrap,
      documents: [documents[0]],
      getDocumentPreview: async () => ({
        kind: "failure",
        code: "unsafePreview",
        message: "PDF 包含被禁止的脚本。"
      })
    });
    const user = userEvent.setup();
    render(<App client={client} />);
    await screen.findByText("年度报告");

    await user.click(
      screen.getByRole("button", { name: "选择文档 年度报告" })
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "PDF 包含被禁止的脚本。"
    );
    expect(screen.getByRole("heading", { name: /年度报告/ })).toBeInTheDocument();
    expect(
      screen.getByRole("button", {
        name: "用系统默认程序打开 年度报告"
      })
    ).toBeEnabled();
  });

  it("refreshes thumbnail and preview when the same document id gets a new content hash", async () => {
    const original = documents[0];
    const refreshed = {
      ...original,
      fileSize: 512,
      contentHash: "hash-pdf-refreshed",
      lastImportedAt: "2026-09-13T09:30:00Z"
    };
    let thumbnailAttempts = 0;
    let previewAttempts = 0;
    const client = new FakeBackendClient({
      bootstrap,
      documents: [original],
      getDocumentThumbnail: async () => {
        thumbnailAttempts += 1;
        return {
          kind: "pdf",
          dataUrl: "data:image/png;base64,iVBORw0KGgo="
        };
      },
      getDocumentPreview: async () => {
        previewAttempts += 1;
        return {
          kind: "pdf",
          dataUrl:
            previewAttempts === 1
              ? "data:image/png;base64,first-page"
              : "data:image/png;base64,refreshed-page",
          pageCount: 1,
          page: 1
        };
      }
    });
    const user = userEvent.setup();
    render(<App client={client} />);
    await screen.findByText("年度报告");

    await user.click(screen.getByRole("button", { name: "网格视图" }));
    await waitFor(() => {
      expect(
        client.calls.filter((call) =>
          call.startsWith("getDocumentThumbnail:")
        )
      ).toHaveLength(1);
    });
    await user.click(
      screen.getByRole("button", { name: "选择文档 年度报告" })
    );
    const pdfPage = await screen.findByRole("img", {
      name: "年度报告 第 1 页预览"
    });
    expect(pdfPage).toHaveAttribute(
      "src",
      "data:image/png;base64,first-page"
    );
    await waitFor(() =>
      expect(client.calls).toContain("subscribeToDocumentIndexChanges")
    );

    client.setDocument(refreshed);
    await act(async () => {
      client.emitDocumentIndexChanged({
        phase: "completed",
        documentIds: [],
        result: { processed: 1, searchable: 1, failed: 0 }
      });
      await Promise.resolve();
    });

    await waitFor(() => {
      expect(
        client.calls.filter((call) =>
          call.startsWith("getDocumentThumbnail:")
        )
      ).toHaveLength(2);
      expect(
        client.calls.filter((call) =>
          call.startsWith("getDocumentPreview:")
        )
      ).toHaveLength(2);
    });
    await waitFor(() => {
      expect(
        screen.getByRole("img", { name: "年度报告 第 1 页预览" })
      ).toHaveAttribute(
        "src",
        "data:image/png;base64,refreshed-page"
      );
    });
  });

  it("shows the external program startup error and keeps the selected document unchanged", async () => {
    const client = new FakeBackendClient({
      bootstrap,
      documents: [documents[4]],
      openDocument: async () => {
        throw new BackendError({
          code: "openDocument",
          message: "系统默认程序启动失败：找不到关联程序。"
        });
      }
    });
    const user = userEvent.setup();
    render(<App client={client} />);
    await screen.findByText("会议记录");

    await user.click(
      screen.getByRole("button", { name: "选择文档 会议记录" })
    );
    await screen.findByText("会议记录版式内容");
    await user.click(
      screen.getByRole("button", {
        name: "用系统默认程序打开 会议记录"
      })
    );

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "系统默认程序启动失败：找不到关联程序。"
    );
    expect(screen.getByRole("heading", { name: /会议记录/ })).toBeInTheDocument();
    expect(screen.getByText("会议记录版式内容")).toBeInTheDocument();
  });
});
