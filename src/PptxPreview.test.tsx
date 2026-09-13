import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { App } from "./App";
import { FakeBackendClient } from "./backend/fakeClient";
import type {
  BootstrapState,
  DocumentPreview,
  DocumentSummary,
  LibrarySummary
} from "./backend/types";
import { PptxPreview, PptxThumbnail } from "./components/PptxPreview";

const pptxMocks = vi.hoisted(() => ({
  open: vi.fn()
}));

vi.mock("@file-viewer/pptx", () => ({
  PptxViewer: {
    open: pptxMocks.open
  }
}));

interface MockRendererOptions {
  onSlideRendered?: (index: number, element: Element | null) => void;
  onSlideError?: (index: number, error: unknown) => void;
  onRenderComplete?: () => void;
  onThumbnail?: (base64Jpeg: string) => void;
  onError?: (error: unknown) => void;
}

const library: LibrarySummary = {
  id: "library-pptx",
  name: "演示资料库",
  path: "C:\\Documents\\演示资料库",
  createdAt: "2026-09-13T08:00:00Z"
};

const bootstrap: BootstrapState = {
  currentLibrary: library,
  recentLibraries: []
};

const documentSummary: DocumentSummary = {
  id: "pptx-1",
  title: "季度汇报",
  description: null,
  documentDate: null,
  fileName: "季度汇报.pptx",
  fileType: "PPTX",
  fileSize: 512,
  contentHash: "hash-pptx-1",
  collectionId: "inbox",
  tags: [],
  processingStatus: "ready",
  indexStatus: "searchable",
  errorStage: null,
  errorMessage: null,
  importedAt: "2026-09-13T08:10:00Z",
  sourcePath: "C:\\Sources\\季度汇报.pptx",
  sourceIdentifier: "c:\\sources\\季度汇报.pptx",
  lastImportedAt: "2026-09-13T08:10:00Z"
};

const pptxPreview: Extract<DocumentPreview, { kind: "pptx" }> = {
  kind: "pptx",
  dataUrl:
    "data:application/vnd.openxmlformats-officedocument.presentationml.presentation;base64,UEsFBgAAAAAAAAAAAAAAAAAAAAAAAA==",
  text: "提取出的季度汇报正文",
  notice: "PPTX 版式预览为本地只读近似呈现。",
  degradedFeatures: []
};

function createViewerMock({
  slideCount = 2,
  contents = ["幻灯片一", "幻灯片二"],
  errors = []
}: {
  slideCount?: number;
  contents?: string[];
  errors?: number[];
} = {}) {
  const elements = new Map<number, HTMLElement>();
  const destroy = vi.fn();
  const setZoom = vi.fn(async () => undefined);
  const refreshLayout = vi.fn();
  const viewer = {
    slideCount,
    ensureSlideRendered: vi.fn((index: number) => elements.get(index) ?? null),
    setZoom,
    refreshLayout,
    destroy
  };

  function contentHost(host: HTMLElement) {
    let content = host.querySelector<HTMLElement>(".flyfish-pptx-content");
    if (!content) {
      content = document.createElement("div");
      content.className = "flyfish-pptx-content";
      host.append(content);
    }
    return content;
  }

  pptxMocks.open.mockImplementation(
    async (_buffer: ArrayBuffer, host: HTMLElement, options: MockRendererOptions) => {
      for (let index = 1; index <= slideCount; index += 1) {
        const slide = document.createElement("section");
        slide.className = "slide";
        slide.style.width = "800px";
        slide.style.height = "450px";
        if (errors.includes(index)) {
          slide.className = "slide flyfish-pptx-slide-error";
          slide.textContent = `第 ${index} 张幻灯片解析失败`;
        } else {
          slide.textContent = contents[index - 1] ?? `第 ${index} 张幻灯片`;
        }
        contentHost(host).append(slide);
        elements.set(index, slide);
        options.onSlideRendered?.(index, slide);
        if (errors.includes(index)) {
          options.onSlideError?.(index, new Error("slide failed"));
        }
      }
      options.onRenderComplete?.();
      return viewer;
    }
  );
  return { viewer, destroy, setZoom, refreshLayout };
}

beforeEach(() => {
  pptxMocks.open.mockReset();
  Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
    configurable: true,
    value: vi.fn()
  });
});

describe("PPTX 导入与版式预览", () => {
  it("imports a selected PPTX through the shared format capability", async () => {
    const user = userEvent.setup();
    const client = new FakeBackendClient({
      bootstrap,
      selectedDocuments: [documentSummary.sourcePath]
    });

    render(<App client={client} />);
    const emptyLibrary = await screen.findByRole("main", { name: "空资料库" });
    await user.click(
      emptyLibrary.querySelector<HTMLButtonElement>(
        ".empty-library .button.primary"
      ) ?? screen.getByRole("button", { name: "导入文档" })
    );

    expect(await screen.findByText("季度汇报")).toBeInTheDocument();
    expect(screen.getByText("PPTX")).toBeInTheDocument();
    expect(client.calls).toContain(
      `startImport:${documentSummary.sourcePath}`
    );
  });

  it("renders slides, supports page and thumbnail navigation, fit width and zoom", async () => {
    createViewerMock();
    const client = new FakeBackendClient({
      bootstrap,
      getDocumentPreview: async () => pptxPreview
    });
    const user = userEvent.setup();
    render(
      <PptxPreview
        client={client}
        document={documentSummary}
        preview={pptxPreview}
      />
    );

    expect(await screen.findByText("幻灯片一")).toBeInTheDocument();
    expect(await screen.findByText("第 1 / 2 张")).toBeInTheDocument();
    expect(pptxMocks.open).toHaveBeenCalledTimes(1);
    expect(pptxMocks.open.mock.calls[0][2]).toMatchObject({
      lazySlides: false,
      lazyMedia: false,
      engineOptions: {
        mediaProcess: false,
        keyBoardShortCut: false
      }
    });

    await user.click(screen.getByRole("button", { name: "PPTX 下一页" }));
    expect(await screen.findByText("第 2 / 2 张")).toBeInTheDocument();
    expect(HTMLElement.prototype.scrollIntoView).toHaveBeenCalled();

    await user.click(
      screen.getByRole("button", { name: "转到第 1 张幻灯片" })
    );
    expect(await screen.findByText("第 1 / 2 张")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "放大 PPTX 预览" }));
    expect(
      screen.getByRole("button", { name: "适配 PPTX 预览宽度" })
    ).toHaveAttribute("aria-pressed", "false");
    await waitFor(() => {
      expect(pptxMocks.open).toHaveBeenCalled();
    });

    await user.click(
      screen.getByRole("button", { name: "适配 PPTX 预览宽度" })
    );
    expect(
      screen.getByRole("button", { name: "适配 PPTX 预览宽度" })
    ).toHaveAttribute("aria-pressed", "true");
  });

  it("removes scripts, embedded media and remote resources and opens links only on click", async () => {
    createViewerMock({ slideCount: 1, contents: [""] });
    pptxMocks.open.mockImplementationOnce(
      async (_buffer: ArrayBuffer, host: HTMLElement, options: MockRendererOptions) => {
        const slide = document.createElement("section");
        slide.className = "slide";
        slide.style.width = "800px";
        slide.style.height = "450px";
        slide.innerHTML =
          "<p><a href=\"https://example.com/deck\">安全链接</a></p>" +
          "<a href=\"javascript:alert(1)\">不安全链接</a>" +
          "<img src=\"https://example.com/tracker.png\" alt=\"远程图片\">" +
          "<script>window.failed = true</script>" +
          "<iframe src=\"https://example.com/embed\"></iframe>" +
          "<video src=\"https://example.com/video.mp4\"></video>";
        let content = host.querySelector<HTMLElement>(".flyfish-pptx-content");
        if (!content) {
          content = document.createElement("div");
          content.className = "flyfish-pptx-content";
          host.append(content);
        }
        content.append(slide);
        options.onSlideRendered?.(1, slide);
        options.onRenderComplete?.();
        return {
          slideCount: 1,
          ensureSlideRendered: () => slide,
          setZoom: vi.fn(async () => undefined),
          refreshLayout: vi.fn(),
          destroy: vi.fn()
        };
      }
    );
    const client = new FakeBackendClient({ bootstrap });
    const user = userEvent.setup();
    const { container } = render(
      <PptxPreview
        client={client}
        document={documentSummary}
        preview={pptxPreview}
      />
    );

    const safeLink = await screen.findByRole("link", { name: "安全链接" });
    expect(safeLink).not.toHaveAttribute("href");
    expect(client.calls).not.toContain(
      "openExternalUrl:https://example.com/deck"
    );
    await user.click(safeLink);
    expect(client.calls).toContain(
      "openExternalUrl:https://example.com/deck"
    );

    for (const element of screen.getAllByText("不安全链接")) {
      expect(element).not.toHaveAttribute("role", "link");
    }
    expect(container.querySelector("script")).not.toBeInTheDocument();
    expect(container.querySelector("iframe")).not.toBeInTheDocument();
    expect(container.querySelector("video")).not.toBeInTheDocument();
    expect(container.querySelector("img[src^='https:']")).not.toBeInTheDocument();
  });

  it("keeps other slides visible when one slide fails with a local degradation", async () => {
    createViewerMock({
      slideCount: 2,
      contents: ["第一页仍可预览"],
      errors: [2]
    });
    const client = new FakeBackendClient({ bootstrap });
    render(
      <PptxPreview
        client={client}
        document={documentSummary}
        preview={pptxPreview}
      />
    );

    expect(await screen.findByText("第一页仍可预览")).toBeInTheDocument();
    expect(screen.getAllByText("第 2 张幻灯片解析失败").length).toBeGreaterThan(0);
    expect(
      await screen.findByText(/部分复杂内容无法完整呈现.*第 2 张幻灯片/)
    ).toBeInTheDocument();
  });

  it("falls back to extracted text and external open when the renderer fails", async () => {
    pptxMocks.open.mockRejectedValueOnce(new Error("PPTX Worker 启动失败。"));
    const client = new FakeBackendClient({
      bootstrap,
      openDocument: async () => undefined
    });
    const user = userEvent.setup();
    render(
      <PptxPreview
        client={client}
        document={documentSummary}
        preview={pptxPreview}
      />
    );

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "PPTX Worker 启动失败。"
    );
    expect(screen.getByText("提取出的季度汇报正文")).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", {
        name: `用系统默认程序打开 ${documentSummary.title}`
      })
    );
    expect(client.calls).toContain(`openDocument:${documentSummary.id}`);
  });

  it("lazily generates and persists a real renderer thumbnail for the grid", async () => {
    const thumbnailDataUrl =
      "data:image/jpeg;base64,/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAP//////////////////////////////////////////////////////////////////////////////////////2wBDAf//////////////////////////////////////////////////////////////////////////////////////wAARCAABAAEDASIAAhEBAxEB/8QAFQABAQAAAAAAAAAAAAAAAAAAAAf/xAAUEAEAAAAAAAAAAAAAAAAAAAAA/9oADAMBAAIQAxAAAAF//8QAFBABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQABBQJ//8QAFBEBAAAAAAAAAAAAAAAAAAAAAP/aAAgBAwEBPwF//8QAFBEBAAAAAAAAAAAAAAAAAAAAAP/aAAgBAgEBPwF//8QAFBABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAGPwJ//8QAFBABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQABPyF//9oADAMBAAIAAwAAABD/xAAUEQEAAAAAAAAAAAAAAAAAAAAA/9oACAEDAQE/EH//xAAUEQEAAAAAAAAAAAAAAAAAAAAA/9oACAECAQE/EH//xAAUEAEAAAAAAAAAAAAAAAAAAAAA/9oACAEBAAE/EH//2Q==";
    const saveDocumentThumbnail = vi.fn(async (_id, dataUrl: string) => ({
      kind: "pptx" as const,
      dataUrl
    }));
    const client = new FakeBackendClient({
      bootstrap,
      getDocumentThumbnail: async () => ({
        kind: "fallback",
        reason: "PPTX 缩略图尚未生成。"
      }),
      getDocumentPreview: async () => pptxPreview,
      saveDocumentThumbnail
    });
    pptxMocks.open.mockImplementationOnce(
      async (_buffer: ArrayBuffer, host: HTMLElement, options: MockRendererOptions) => {
        const slide = document.createElement("section");
        slide.className = "slide";
        slide.style.width = "800px";
        slide.style.height = "450px";
        slide.textContent = "第一页";
        let content = host.querySelector<HTMLElement>(".flyfish-pptx-content");
        if (!content) {
          content = document.createElement("div");
          content.className = "flyfish-pptx-content";
          host.append(content);
        }
        content.append(slide);
        options.onThumbnail?.(
          thumbnailDataUrl.replace("data:image/jpeg;base64,", "")
        );
        options.onRenderComplete?.();
        return {
          slideCount: 1,
          ensureSlideRendered: () => slide,
          setZoom: vi.fn(async () => undefined),
          refreshLayout: vi.fn(),
          destroy: vi.fn()
        };
      }
    );

    const { container } = render(
      <PptxThumbnail client={client} document={documentSummary} />
    );

    const image = await waitFor(() => {
      const candidate = container.querySelector<HTMLImageElement>(
        ".pptx-generated img"
      );
      expect(candidate).not.toBeNull();
      return candidate!;
    });
    expect(image).toHaveAttribute("src", thumbnailDataUrl);
    expect(image).toHaveAttribute("data-thumbnail-kind", "pptx");
    await waitFor(() => {
      expect(saveDocumentThumbnail).toHaveBeenCalledWith(
        documentSummary.id,
        thumbnailDataUrl
      );
    });
  });

  it("falls back to the PPTX type icon when thumbnail rendering fails", async () => {
    pptxMocks.open.mockRejectedValueOnce(new Error("thumbnail renderer failed"));
    const client = new FakeBackendClient({
      bootstrap,
      getDocumentThumbnail: async () => ({
        kind: "fallback",
        reason: "PPTX 缩略图尚未生成。"
      }),
      getDocumentPreview: async () => pptxPreview
    });
    const { container } = render(
      <PptxThumbnail client={client} document={documentSummary} />
    );

    await waitFor(() => {
      expect(
        container.querySelector('[data-thumbnail-failed="true"]')
      ).toBeInTheDocument();
    });
    expect(
      container.querySelector("svg.lucide-presentation")
    ).toBeInTheDocument();
  });
});
