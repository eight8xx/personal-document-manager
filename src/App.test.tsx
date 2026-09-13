import {
  act,
  cleanup,
  render,
  screen,
  waitFor,
  within
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";

import { App } from "./App";
import { FakeBackendClient } from "./backend/fakeClient";
import type {
  BootstrapState,
  DocumentSummary,
  LibrarySummary
} from "./backend/types";

const currentLibrary: LibrarySummary = {
  id: "library-current",
  name: "个人资料",
  path: "C:\\Documents\\个人资料",
  createdAt: "2026-09-13T08:00:00Z"
};

const importedDocument: DocumentSummary = {
  id: "document-1",
  title: "项目说明",
  description: null,
  documentDate: null,
  fileName: "项目说明.md",
  fileType: "Markdown",
  fileSize: 24,
  contentHash:
    "f0e90aeef1ad3ef3666f7cf73a7938d958078cddb1dcfc6eed65a394d9940e18",
  collectionId: "inbox",
  tags: [],
  processingStatus: "ready",
  indexStatus: "pending",
  errorStage: null,
  errorMessage: null,
  importedAt: "2026-09-13T08:10:00Z",
  sourcePath: "C:\\Documents\\项目说明.md",
  sourceIdentifier: "c:\\documents\\项目说明.md",
  lastImportedAt: "2026-09-13T08:10:00Z"
};

afterEach(() => {
  cleanup();
});

function bootstrapWithLibrary(
  recentLibraries: BootstrapState["recentLibraries"] = []
): BootstrapState {
  return {
    currentLibrary,
    recentLibraries
  };
}

describe("App", () => {
  it("creates a library from the first-run wizard", async () => {
    const user = userEvent.setup();
    const client = new FakeBackendClient({
      selectedDirectory: "C:\\Documents\\我的资料库"
    });

    render(<App client={client} />);

    expect(
      await screen.findByRole("heading", { name: "选择资料库目录" })
    ).toBeInTheDocument();

    await user.click(
      screen.getByRole("button", { name: "选择资料库目录" })
    );

    expect(
      await screen.findByRole("button", { name: "创建资料库" })
    ).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "创建资料库" }));

    expect(
      await screen.findByRole("heading", { name: "空资料库" })
    ).toBeInTheDocument();
    expect(
      within(screen.getByRole("main", { name: "空资料库" })).getByRole(
        "button",
        { name: "导入文档" }
      )
    ).toBeEnabled();
    expect(client.calls).toContain(
      "create:C:\\Documents\\我的资料库"
    );
  });

  it("shows a cloud warning and lets the user go back", async () => {
    const user = userEvent.setup();
    const path = "C:\\Users\\Me\\OneDrive\\Documents\\资料库";
    const client = new FakeBackendClient({
      selectedDirectory: path,
      inspections: {
        [path]: {
          path,
          status: "usable",
          isExistingLibrary: false,
          cloudSyncWarning: {
            provider: "OneDrive",
            message: "这个位置看起来位于 OneDrive 同步目录中。"
          },
          reason: null
        }
      }
    });

    render(<App client={client} />);
    await user.click(
      await screen.findByRole("button", { name: "选择资料库目录" })
    );

    expect(
      await screen.findByRole("heading", {
        name: "这个位置可能由云盘同步"
      })
    ).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "返回修改位置" }));
    expect(
      screen.getByRole("heading", { name: "选择资料库目录" })
    ).toBeInTheDocument();
  });

  it("exposes the empty state and settings library controls", async () => {
    const user = userEvent.setup();
    const otherLibrary = {
      path: "D:\\Archive",
      name: "归档",
      lastOpenedAt: "2026-09-12T08:00:00Z",
      isAvailable: true
    };
    const client = new FakeBackendClient({
      bootstrap: bootstrapWithLibrary([
        {
          path: currentLibrary.path,
          name: currentLibrary.name,
          lastOpenedAt: "2026-09-13T08:00:00Z",
          isAvailable: true
        },
        otherLibrary
      ])
    });

    render(<App client={client} />);

    expect(
      await screen.findByRole("heading", { name: "空资料库" })
    ).toBeInTheDocument();
    expect(
      within(screen.getByRole("main", { name: "空资料库" })).getByRole(
        "button",
        { name: "导入文档" }
      )
    ).toBeEnabled();

    const settingsButton = screen.getByRole("button", { name: "设置" });
    await user.click(settingsButton);

    const dialog = screen.getByRole("dialog", { name: "资料库" });
    expect(dialog).toBeInTheDocument();
    const closeSettings = within(dialog).getByRole("button", {
      name: "关闭设置"
    });
    await waitFor(() => expect(closeSettings).toHaveFocus());
    await user.tab({ shift: true });
    expect(
      within(dialog).getAllByRole("button", { name: "移出列表" }).at(-1)
    ).toHaveFocus();
    await user.tab();
    expect(closeSettings).toHaveFocus();
    expect(
      within(dialog).getAllByText(currentLibrary.path).length
    ).toBeGreaterThan(0);
    expect(
      within(dialog).getByText("备份资料库前，请先关闭应用。")
    ).toBeInTheDocument();

    await user.click(within(dialog).getByRole("button", { name: "打开目录" }));
    expect(client.calls).toContain(
      `openDirectory:${currentLibrary.path}`
    );

    await user.click(within(dialog).getByRole("button", { name: "切换" }));
    await waitFor(() => {
      expect(client.calls).toContain("open:D:\\Archive");
    });

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: "资料库" })).not.toBeInTheDocument();
    await waitFor(() => expect(settingsButton).toHaveFocus());
  });

  it("imports a file selected from the picker and shows its status", async () => {
    const user = userEvent.setup();
    const client = new FakeBackendClient({
      bootstrap: bootstrapWithLibrary(),
      selectedDocument: importedDocument.sourcePath,
      importDocument: async () => importedDocument
    });

    render(<App client={client} />);
    const emptyLibrary = await screen.findByRole("main", {
      name: "空资料库"
    });
    await user.click(
      within(emptyLibrary).getByRole("button", { name: "导入文档" })
    );

    expect(await screen.findByText("项目说明")).toBeInTheDocument();
    expect(await screen.findByText("可搜索")).toBeInTheDocument();
    expect(client.calls).toContain("indexPendingDocuments");
    expect((await screen.findAllByText("项目说明.md")).length).toBeGreaterThan(0);
    expect(client.calls).toContain(
      `import:${importedDocument.sourcePath}`
    );
  });

  it("imports a file dropped onto the workspace", async () => {
    const client = new FakeBackendClient({
      bootstrap: bootstrapWithLibrary(),
      importDocument: async () => importedDocument
    });

    render(<App client={client} />);
    await screen.findByRole("heading", { name: "空资料库" });
    client.emitFileDrop([importedDocument.sourcePath]);

    expect(await screen.findByText("项目说明")).toBeInTheDocument();
    expect(client.calls).toContain(
      `import:${importedDocument.sourcePath}`
    );
  });

  it("keeps a just-imported document when the initial list response arrives later", async () => {
    let resolveDocuments:
      | ((documents: DocumentSummary[]) => void)
      | undefined;
    const client = new FakeBackendClient({
      bootstrap: bootstrapWithLibrary(),
      importDocument: async () => importedDocument
    });
    client.listDocuments = () =>
      new Promise((resolve) => {
        resolveDocuments = resolve;
      });

    render(<App client={client} />);
    await waitFor(() => {
      expect(client.calls).toContain("subscribeToFileDrops");
    });
    client.emitFileDrop([importedDocument.sourcePath]);
    await waitFor(() => {
      expect(client.calls).toContain(
        `import:${importedDocument.sourcePath}`
      );
    });

    await act(async () => {
      resolveDocuments?.([]);
    });

    expect(
      (await screen.findAllByText("项目说明.md")).length
    ).toBeGreaterThan(0);
  });

  it("shows an item failure when the selected file type is unsupported", async () => {
    const user = userEvent.setup();
    const client = new FakeBackendClient({
      bootstrap: bootstrapWithLibrary(),
      selectedDocument: "C:\\Documents\\installer.exe"
    });

    render(<App client={client} />);
    const emptyLibrary = await screen.findByRole("main", {
      name: "空资料库"
    });
    await user.click(
      within(emptyLibrary).getByRole("button", { name: "导入文档" })
    );

    expect(await screen.findByText("导入失败")).toBeInTheDocument();
    expect(
      screen.getByText(/仅支持 PDF、DOCX、TXT、Markdown、JPG、PNG 和 PPTX/)
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("main", { name: "文档列表" })
    ).not.toBeInTheDocument();
  });

  it("shows a retryable state when document processing fails", async () => {
    const failedDocument: DocumentSummary = {
      ...importedDocument,
      id: "document-failed",
      processingStatus: "failed",
      indexStatus: "failed",
      errorStage: "hashing",
      errorMessage: "无法计算文件哈希"
    };
    const client = new FakeBackendClient({
      bootstrap: bootstrapWithLibrary(),
      documents: [failedDocument]
    });

    render(<App client={client} />);

    expect(await screen.findByText("项目说明")).toBeInTheDocument();
    expect(screen.getByText("处理失败，等待重试")).toBeInTheDocument();
  });

  it("imports multiple dropped files as one batch", async () => {
    const client = new FakeBackendClient({
      bootstrap: bootstrapWithLibrary()
    });

    render(<App client={client} />);
    await screen.findByRole("heading", { name: "空资料库" });
    client.emitFileDrop([
      importedDocument.sourcePath,
      "C:\\Documents\\第二份.md"
    ]);

    expect(
      (await screen.findAllByText("项目说明.md")).length
    ).toBeGreaterThan(0);
    expect(screen.getAllByText("第二份.md").length).toBeGreaterThan(0);
    expect(client.calls).toContain(
      `startImport:${importedDocument.sourcePath}|C:\\Documents\\第二份.md`
    );
  });
});
