import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { parseAsync, renderDocument } from "docx-preview";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { FakeBackendClient } from "./backend/fakeClient";
import type {
  BootstrapState,
  DocumentPreview,
  DocumentSummary
} from "./backend/types";
import { DocxPreview } from "./components/DocxPreview";

vi.mock("docx-preview", () => ({
  parseAsync: vi.fn(),
  renderDocument: vi.fn()
}));

const parseMock = vi.mocked(parseAsync);
const renderMock = vi.mocked(renderDocument);

const bootstrap: BootstrapState = {
  currentLibrary: null,
  recentLibraries: []
};

const documentSummary: DocumentSummary = {
  id: "docx-1",
  title: "会议记录",
  description: null,
  documentDate: null,
  fileName: "会议记录.docx",
  fileType: "DOCX",
  fileSize: 256,
  contentHash: "hash-1",
  collectionId: "inbox",
  tags: [],
  processingStatus: "ready",
  indexStatus: "searchable",
  errorStage: null,
  errorMessage: null,
  importedAt: "2026-09-13T08:10:00Z",
  sourcePath: "C:\\Sources\\会议记录.docx",
  sourceIdentifier: "c:\\sources\\会议记录.docx",
  lastImportedAt: "2026-09-13T08:10:00Z"
};

const docxPreview: Extract<DocumentPreview, { kind: "docx" }> = {
  kind: "docx",
  dataUrl:
    "data:application/vnd.openxmlformats-officedocument.wordprocessingml.document;base64,UEsFBgAAAAAAAAAAAAAAAAAAAAAAAA==",
  text: "提取出的会议正文",
  notice: "DOCX 版式预览为本地只读近似呈现。",
  degradedFeatures: []
};

function mockRenderedDocument(options: {
  content?: string;
  pages?: number;
  wrapClassName?: string;
}) {
  const wrapper = document.createElement("div");
  wrapper.className = options.wrapClassName ?? "docx-wrapper";
  for (let pageIndex = 0; pageIndex < (options.pages ?? 1); pageIndex += 1) {
    const section = document.createElement("section");
    section.className = "docx";
    section.style.width = "800px";
    section.style.minHeight = "1000px";
    const article = document.createElement("article");
    article.innerHTML =
      pageIndex === 0
        ? options.content ??
          "<p>段落内容</p><table><tr><td>表格内容</td></tr></table><img src=\"blob:docx-image\" alt=\"本地图片\"><ol><li>列表内容</li></ol><header>页眉</header><footer>页脚</footer><math><mi>x</mi></math>"
        : `<p>第 ${pageIndex + 1} 页</p>`;
    section.appendChild(article);
    wrapper.appendChild(section);
  }
  return [wrapper];
}

function renderPreview(
  client: FakeBackendClient,
  preview: Extract<DocumentPreview, { kind: "docx" }> = docxPreview,
  document: DocumentSummary = documentSummary
) {
  return render(
    <DocxPreview
      client={client}
      document={document}
      preview={preview}
    />
  );
}

beforeEach(() => {
  parseMock.mockReset();
  renderMock.mockReset();
  parseMock.mockResolvedValue({});
  renderMock.mockResolvedValue(mockRenderedDocument({}));
});

describe("DOCX 版式预览", () => {
  it("renders structured DOCX content and uses the safe docx-preview options", async () => {
    const client = new FakeBackendClient({ bootstrap });
    const { container } = renderPreview(client);

    expect(await screen.findByText("段落内容")).toBeInTheDocument();
    expect(screen.getByText("表格内容")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "本地图片" })).toBeInTheDocument();
    expect(screen.getByText("列表内容")).toBeInTheDocument();
    expect(screen.getByText("页眉")).toBeInTheDocument();
    expect(screen.getByText("页脚")).toBeInTheDocument();
    expect(container.querySelector("math")).not.toBeNull();

    expect(parseMock).toHaveBeenCalledTimes(1);
    expect(parseMock.mock.calls[0][0]).toBeInstanceOf(Blob);
    expect((parseMock.mock.calls[0][0] as Blob).type).toBe(
      "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    );
    expect(parseMock.mock.calls[0][1]).toMatchObject({
      renderAltChunks: false,
      renderHeaders: true,
      renderFooters: true,
      renderFootnotes: true,
      renderEndnotes: true,
      breakPages: true,
      ignoreWidth: false,
      ignoreHeight: false
    });
    expect(renderMock.mock.calls[0][1]).toMatchObject({
      renderAltChunks: false,
      renderHeaders: true,
      renderFooters: true
    });
  });

  it("sanitizes links and remote or embedded resources before mounting them", async () => {
    const client = new FakeBackendClient({ bootstrap });
    const user = userEvent.setup();
    renderMock.mockResolvedValue(
      mockRenderedDocument({
        content:
          "<p><a href=\"https://example.com/report\">安全链接</a></p>" +
          "<p><a href=\"javascript:alert(1)\">不安全链接</a></p>" +
          "<img src=\"https://example.com/tracker.png\" alt=\"远程图片\">" +
          "<script>window.fail = true</script>" +
          "<div style=\"background-image: url(https://example.com/bg.png)\">样式</div>"
      })
    );
    const { container } = renderPreview(client);

    const safeLink = await screen.findByRole("link", {
      name: "安全链接"
    });
    expect(safeLink).not.toHaveAttribute("href");
    expect(client.calls).not.toContain(
      "openExternalUrl:https://example.com/report"
    );
    await user.click(safeLink);
    expect(client.calls).toContain(
      "openExternalUrl:https://example.com/report"
    );

    expect(screen.getByText("不安全链接")).not.toHaveAttribute("role", "link");
    expect(
      screen.getByText("[远程图片已降级]")
    ).toBeInTheDocument();
    expect(
      screen.getByText("[嵌入对象已降级]")
    ).toBeInTheDocument();
    expect(container.querySelector("script")).not.toBeInTheDocument();
    expect(container.querySelector("img[src^='https:']")).not.toBeInTheDocument();
    expect(
      screen.getByText("样式").getAttribute("style")
    ).not.toContain("https://example.com");
    expect(
      screen.getByRole("status")
    ).toHaveTextContent("远程图片");
  });

  it("keeps the rest of the document visible when preview regions are degraded", async () => {
    const client = new FakeBackendClient({ bootstrap });
    renderPreview(client, {
      ...docxPreview,
      notice: "DOCX 版式预览已呈现，部分复杂内容需要降级。",
      degradedFeatures: ["文本框", "图表"]
    });

    expect(await screen.findByText("段落内容")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent(
      "文本框、图表"
    );
    expect(screen.getByText("表格内容")).toBeInTheDocument();
  });

  it("falls back to extracted text and allows retrying an overall render failure", async () => {
    const client = new FakeBackendClient({ bootstrap });
    const user = userEvent.setup();
    renderMock
      .mockRejectedValueOnce(new Error("DOCX 结构无法解析。"))
      .mockResolvedValueOnce(mockRenderedDocument({}));
    renderPreview(client);

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "DOCX 结构无法解析。"
    );
    expect(screen.getByText("提取出的会议正文")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "重试版式预览" }));
    await waitFor(() => {
      expect(parseMock).toHaveBeenCalledTimes(2);
      expect(renderMock).toHaveBeenCalledTimes(2);
    });
    expect(await screen.findByText("段落内容")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("supports fit width, zoom and page jumps with stable page controls", async () => {
    const client = new FakeBackendClient({ bootstrap });
    const user = userEvent.setup();
    const scrollIntoView = vi.fn();
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
      configurable: true,
      value: scrollIntoView
    });
    renderMock.mockResolvedValue(mockRenderedDocument({ pages: 3 }));
    const { container } = renderPreview(client);

    expect(await screen.findByText("第 1 / 3 页")).toBeInTheDocument();
    const renderHost = container.querySelector<HTMLElement>(
      ".docx-preview-render"
    )!;
    const initialScale = Number.parseFloat(
      renderHost.style.getPropertyValue("--docx-preview-scale")
    );
    expect(initialScale).toBeGreaterThan(0);
    expect(
      screen.getByRole("button", { name: "适配 DOCX 预览宽度" })
    ).toHaveAttribute("aria-pressed", "true");

    await user.click(screen.getByRole("button", { name: "DOCX 下一页" }));
    expect(await screen.findByText("第 2 / 3 页")).toBeInTheDocument();
    await waitFor(() => expect(scrollIntoView).toHaveBeenCalled());

    const pageInput = screen.getByRole("spinbutton", {
      name: "跳转到 DOCX 页码"
    });
    await user.clear(pageInput);
    await user.type(pageInput, "3");
    expect(await screen.findByText("第 3 / 3 页")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "放大 DOCX 预览" }));
    expect(
      screen.getByRole("button", { name: "适配 DOCX 预览宽度" })
    ).toHaveAttribute("aria-pressed", "false");
    expect(
      Number.parseFloat(
        renderHost.style.getPropertyValue("--docx-preview-scale")
      )
    ).toBeGreaterThan(initialScale);

    await user.click(screen.getByRole("button", { name: "缩小 DOCX 预览" }));
    await user.click(
      screen.getByRole("button", { name: "适配 DOCX 预览宽度" })
    );
    expect(
      screen.getByRole("button", { name: "适配 DOCX 预览宽度" })
    ).toHaveAttribute("aria-pressed", "true");
  });

  it("rerenders on content hash changes and releases DOM, object URLs and timers", async () => {
    const client = new FakeBackendClient({ bootstrap });
    const revokeObjectURL = vi.fn();
    Object.defineProperty(URL, "revokeObjectURL", {
      configurable: true,
      value: revokeObjectURL
    });
    const clearTimeout = vi.spyOn(window, "clearTimeout");
    renderMock
      .mockResolvedValueOnce(
        mockRenderedDocument({
          content:
            "<p>旧版式内容</p><img src=\"blob:docx-image\" alt=\"旧图片\">"
        })
      )
      .mockResolvedValueOnce(
        mockRenderedDocument({
          content:
            "<p>新版式内容</p><img src=\"blob:new-image\" alt=\"新图片\">"
        })
      );
    const { rerender, container } = renderPreview(client);

    expect(await screen.findByText("旧版式内容")).toBeInTheDocument();
    expect(
      await screen.findByText("第 1 / 1 页")
    ).toBeInTheDocument();

    const refreshedDocument = {
      ...documentSummary,
      contentHash: "hash-2",
      lastImportedAt: "2026-09-13T09:30:00Z"
    };
    rerender(
      <DocxPreview
        client={client}
        document={refreshedDocument}
        preview={{
          ...docxPreview,
          dataUrl:
            "data:application/vnd.openxmlformats-officedocument.wordprocessingml.document;base64,UEsFBgAAAAAAAAAAAAAAAAAAAAAAAA=="
        }}
      />
    );

    expect(await screen.findByText("新版式内容")).toBeInTheDocument();
    expect(screen.queryByText("旧版式内容")).not.toBeInTheDocument();
    await waitFor(() => expect(parseMock).toHaveBeenCalledTimes(2));
    expect(revokeObjectURL).toHaveBeenCalledWith("blob:docx-image");
    expect(clearTimeout).toHaveBeenCalled();

    rerender(<div />);
    expect(
      container.querySelector(".docx-preview-render")
    ).not.toBeInTheDocument();
    expect(revokeObjectURL).toHaveBeenCalledWith("blob:new-image");
    expect(revokeObjectURL.mock.calls.length).toBeGreaterThanOrEqual(2);
  });

  it("does not load a remote link before the user activates it", () => {
    const openExternalUrl = vi.fn(async () => undefined);
    const client = new FakeBackendClient({
      bootstrap,
      openExternalUrl
    });
    renderMock.mockResolvedValue(
      mockRenderedDocument({
        content:
          "<p><a href=\"http://example.com/private\">仅点击后打开</a></p>"
      })
    );
    renderPreview(client);

    return screen.findByRole("link", { name: "仅点击后打开" }).then(() => {
      expect(openExternalUrl).not.toHaveBeenCalled();
      fireEvent.click(
        screen.getByRole("link", { name: "仅点击后打开" })
      );
      expect(openExternalUrl).toHaveBeenCalledWith(
        "http://example.com/private"
      );
    });
  });
});
