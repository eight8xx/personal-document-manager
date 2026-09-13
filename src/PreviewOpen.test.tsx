import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";

import { App } from "./App";
import { BackendError } from "./backend/error";
import { FakeBackendClient } from "./backend/fakeClient";
import type {
  BootstrapState,
  DocumentSummary,
  LibrarySummary
} from "./backend/types";

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
  window.sessionStorage.clear();
});

describe("文档预览与外部打开", () => {
  it("shows loading and renders PDF, image, text, Markdown and DOCX previews", async () => {
    let resolvePreview:
      | ((value: Awaited<ReturnType<FakeBackendClient["getDocumentPreview"]>>) => void)
      | undefined;
    const client = new FakeBackendClient({
      bootstrap,
      documents,
      documentPreviews: {
        pdf: {
          kind: "pdf",
          dataUrl: "data:application/pdf;base64,JVBERi0xLjQ=",
          pageCount: 3
        }
      }
    });
    client.getDocumentPreview = (documentId) => {
      if (documentId === "pdf") {
        return new Promise((resolve) => {
          resolvePreview = resolve;
        });
      }
      return FakeBackendClient.prototype.getDocumentPreview.call(
        client,
        documentId
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
        dataUrl: "data:application/pdf;base64,JVBERi0xLjQ=",
        pageCount: 3
      });
    });

    const pdfFrame = await screen.findByTitle("年度报告 PDF 预览");
    expect(pdfFrame).toHaveAttribute(
      "src",
      expect.stringContaining("#page=1")
    );
    await user.click(screen.getByRole("button", { name: "PDF 下一页" }));
    expect(screen.getByTitle("年度报告 PDF 预览")).toHaveAttribute(
      "src",
      expect.stringContaining("#page=2")
    );
    expect(screen.getByText("第 2 页 / 3")).toBeInTheDocument();

    await user.click(
      screen.getByRole("button", { name: "选择文档 扫描图" })
    );
    const image = await screen.findByRole("img", { name: "扫描图 预览" });
    expect(image).toHaveAttribute("src", expect.stringContaining("image/png"));

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
      await screen.findByText("DOCX 预览仅显示提取文本，不是完整版式预览。")
    ).toBeInTheDocument();
    expect(screen.getByText("会议记录 的提取文本")).toBeInTheDocument();
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
    expect(await screen.findByTitle("年度报告 PDF 预览")).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", {
        name: "用系统默认程序打开 年度报告"
      })
    );
    expect(client.calls).toContain("openDocument:pdf");
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
    await screen.findByText("会议记录 的提取文本");
    await user.click(
      screen.getByRole("button", {
        name: "用系统默认程序打开 会议记录"
      })
    );

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "系统默认程序启动失败：找不到关联程序。"
    );
    expect(screen.getByRole("heading", { name: /会议记录/ })).toBeInTheDocument();
    expect(screen.getByText("会议记录 的提取文本")).toBeInTheDocument();
  });
});
