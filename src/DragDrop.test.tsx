import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { App } from "./App";
import { FakeBackendClient } from "./backend/fakeClient";
import type {
  BootstrapState,
  CollectionSummary,
  DocumentSummary,
  ImportBatch,
  LibrarySummary
} from "./backend/types";

const library: LibrarySummary = {
  id: "library-drag-drop",
  name: "拖放资料库",
  path: "C:\\Documents\\拖放资料库",
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
    documentCount: 2
  },
  {
    id: "projects",
    name: "项目",
    parentId: null,
    isInbox: false,
    documentCount: 1
  },
  {
    id: "archive",
    name: "归档",
    parentId: "projects",
    isInbox: false,
    documentCount: 0
  }
];

/**
 * 拖入文件后先出现「是否应用分类规则」询问；这里选「不应用」，
 * 走的仍是接入分类规则之前的导入路径（`applyClassification === false`）。
 */
async function importKeepingExistingFlow() {
  const user = userEvent.setup();
  await user.click(
    await screen.findByRole("button", { name: "不应用，按原有方式导入" })
  );
}

function documentFor(
  id: string,
  title: string,
  collectionId: string
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
    collectionId,
    tags: [],
    processingStatus: "ready",
    indexStatus: "searchable",
    errorStage: null,
    errorMessage: null,
    importedAt: "2026-09-13T08:10:00Z",
    sourcePath: `C:\\Sources\\${title}.txt`,
    sourceIdentifier: `c:\\sources\\${title}.txt`,
    lastImportedAt: "2026-09-13T08:10:00Z"
  };
}

const documents = [
  documentFor("first", "第一份文档", "inbox"),
  documentFor("second", "第二份文档", "inbox"),
  documentFor("third", "第三份文档", "projects")
];

function createClient() {
  return new FakeBackendClient({
    bootstrap,
    collections,
    documents
  });
}

function collectionRow(collectionName: string) {
  const row = screen
    .getByRole("button", { name: `管理 ${collectionName}` })
    .closest<HTMLElement>(".collection-row");
  if (!row) {
    throw new Error(`无法找到集合行：${collectionName}`);
  }
  return row;
}

function dragDocumentToCollection(
  title: string,
  collectionName: string
) {
  const source = screen.getByRole("button", {
    name: `选择文档 ${title}`
  });
  const target = collectionRow(collectionName);
  fireEvent.pointerDown(source, {
    button: 0,
    pointerId: 1,
    clientX: 10,
    clientY: 10
  });
  fireEvent.pointerMove(target, {
    pointerId: 1,
    clientX: 120,
    clientY: 80
  });
  fireEvent.pointerUp(target, {
    pointerId: 1,
    clientX: 120,
    clientY: 80
  });
}

describe("资料库拖放", () => {
  it("moves an unselected document without changing the list click behavior", async () => {
    const client = createClient();
    render(<App client={client} />);
    await screen.findByText("第一份文档");

    const source = screen.getByRole("button", {
      name: "选择文档 第一份文档"
    });
    const target = collectionRow("项目");
    fireEvent.pointerDown(source, {
      button: 0,
      pointerId: 1,
      clientX: 10,
      clientY: 10
    });
    fireEvent.pointerMove(target, {
      pointerId: 1,
      clientX: 120,
      clientY: 80
    });

    expect(target).toHaveClass("drop-target");
    const ghost = document.querySelector(".document-drag-ghost");
    expect(ghost).not.toBeNull();
    expect(ghost).toHaveTextContent("1 份文档");
    expect(screen.getByText("已选择 1 项")).toBeInTheDocument();

    fireEvent.pointerUp(target, {
      pointerId: 1,
      clientX: 120,
      clientY: 80
    });

    await waitFor(() => {
      expect(client.calls).toContain(
        "moveDocumentToCollection:first:projects"
      );
    });
    fireEvent.click(
      screen.getByRole("button", { name: "选择文档 第二份文档" }),
      { shiftKey: true }
    );
    expect(screen.getByText("已选择 2 项")).toBeInTheDocument();
  });

  it("moves the whole selection when dragging a selected document", async () => {
    const client = createClient();
    render(<App client={client} />);
    await screen.findByText("第一份文档");

    fireEvent.click(
      screen.getByRole("button", { name: "选择文档 第一份文档" })
    );
    fireEvent.click(
      screen.getByRole("button", { name: "选择文档 第二份文档" }),
      { ctrlKey: true }
    );
    dragDocumentToCollection("第一份文档", "项目");

    await waitFor(() => {
      expect(client.calls).toContain(
        "moveDocumentToCollection:first:projects"
      );
      expect(client.calls).toContain(
        "moveDocumentToCollection:second:projects"
      );
    });
  });

  it("rejects the current collection and does not move on an accidental gesture", async () => {
    const client = createClient();
    render(<App client={client} />);
    await screen.findByText("第一份文档");

    const source = screen.getByRole("button", {
      name: "选择文档 第一份文档"
    });
    const inbox = collectionRow("收件箱");
    fireEvent.pointerDown(source, {
      button: 0,
      pointerId: 2,
      clientX: 20,
      clientY: 20
    });
    fireEvent.pointerMove(inbox, {
      pointerId: 2,
      clientX: 24,
      clientY: 22
    });
    fireEvent.pointerUp(inbox, {
      pointerId: 2,
      clientX: 24,
      clientY: 22
    });
    expect(
      client.calls.some((call) => call.startsWith("moveDocumentToCollection:"))
    ).toBe(false);

    fireEvent.pointerDown(source, {
      button: 0,
      pointerId: 2,
      clientX: 20,
      clientY: 20
    });
    fireEvent.pointerMove(inbox, {
      pointerId: 2,
      clientX: 120,
      clientY: 80
    });
    expect(inbox).toHaveClass("drop-rejected");
    fireEvent.pointerUp(inbox, {
      pointerId: 2,
      clientX: 120,
      clientY: 80
    });

    expect(
      client.calls.some((call) => call.startsWith("moveDocumentToCollection:"))
    ).toBe(false);
  });

  it("expands a collapsed parent after hovering for 500ms", async () => {
    const client = createClient();
    render(<App client={client} />);
    await screen.findByText("第一份文档");

    fireEvent.click(
      screen.getByRole("button", { name: "收起项目" })
    );
    expect(
      screen.queryByRole("button", { name: /^归档 0$/ })
    ).not.toBeInTheDocument();

    vi.useFakeTimers();
    try {
      const source = screen.getByRole("button", {
        name: "选择文档 第一份文档"
      });
      const projects = collectionRow("项目");
      fireEvent.pointerDown(source, {
        button: 0,
        pointerId: 3,
        clientX: 10,
        clientY: 10
      });
      fireEvent.pointerMove(projects, {
        pointerId: 3,
        clientX: 120,
        clientY: 80
      });

      await act(async () => {
        vi.advanceTimersByTime(500);
      });

      const archive = collectionRow("归档");
      fireEvent.pointerMove(archive, {
        pointerId: 3,
        clientX: 150,
        clientY: 110
      });
      fireEvent.pointerUp(archive, {
        pointerId: 3,
        clientX: 150,
        clientY: 110
      });
    } finally {
      vi.useRealTimers();
    }

    await waitFor(() => {
      expect(client.calls).toContain(
        "moveDocumentToCollection:first:archive"
      );
    });
  });

  it("imports external files and folders into the collection under the pointer", async () => {
    const client = createClient();
    render(<App client={client} />);
    await screen.findByText("第一份文档");

    const projects = collectionRow("项目");
    const originalElementFromPoint = document.elementFromPoint;
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => projects)
    });
    const paths = [
      "C:\\Sources\\新增文件.md",
      "C:\\Sources\\待导入文件夹"
    ];

    client.emitFileDrop(paths, { x: 160, y: 90 }, "enter");
    await waitFor(() => {
      expect(projects).toHaveClass("drop-target");
    });
    client.emitFileDrop(paths, { x: 160, y: 90 });
    await importKeepingExistingFlow();

    await waitFor(() => {
      expect(client.calls).toContain(
        "startImport:C:\\Sources\\新增文件.md|C:\\Sources\\待导入文件夹:projects:collectionDrop"
      );
    });
    // 拖入的目标集合与「不应用分类规则」的选择都要传给后端。
    expect(client.startImportCalls.at(-1)).toMatchObject({
      paths,
      targetCollectionId: "projects",
      source: "collectionDrop",
      applyClassification: false
    });
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: originalElementFromPoint
    });
  });

  it("recognizes dropped csv and xlsx files and rejects a legacy xls in the same batch", async () => {
    const client = createClient();
    render(<App client={client} />);
    await screen.findByText("第一份文档");

    const projects = collectionRow("项目");
    const originalElementFromPoint = document.elementFromPoint;
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => projects)
    });

    try {
      const paths = [
        "C:\\Sources\\账目.csv",
        "C:\\Sources\\季度报表.xlsx",
        "C:\\Sources\\旧账目.xls"
      ];
      client.emitFileDrop(paths, { x: 160, y: 90 });
      await importKeepingExistingFlow();

      // 拖放路径不在前端过滤扩展名：整批交给后端按能力表识别，
      // 表格格式与旧格式的判定都由同一张能力表决定。
      await waitFor(() => {
        expect(client.startImportCalls.at(-1)).toMatchObject({
          paths,
          targetCollectionId: "projects",
          source: "collectionDrop"
        });
      });

      const csvRow = (
        await screen.findByRole("button", { name: "选择文档 账目" })
      ).closest("article");
      expect(csvRow).toHaveTextContent("CSV");
      const xlsxRow = (
        await screen.findByRole("button", { name: "选择文档 季度报表" })
      ).closest("article");
      expect(xlsxRow).toHaveTextContent("XLSX");

      // 旧版 XLS 不被当成表格文档：整批仍以两份表格文档成功，第三项单独报告
      // （选择器导入会给出「不支持」原因，拖放路径按既有规则记为忽略）。
      const panel = await screen.findByLabelText("批量导入进度");
      await waitFor(() => {
        expect(panel).toHaveTextContent("已导入 2");
      });
      expect(panel).toHaveTextContent(/(导入失败|已忽略)/);
      expect(
        screen.queryByRole("button", { name: "选择文档 旧账目" })
      ).not.toBeInTheDocument();
    } finally {
      Object.defineProperty(document, "elementFromPoint", {
        configurable: true,
        value: originalElementFromPoint
      });
    }
  });

  it("blocks a second import batch without replacing the visible progress", async () => {
    const client = createClient();
    let resolveBatch:
      | ((batch: ImportBatch) => void)
      | undefined;
    client.startImport = (paths, targetCollectionId) =>
      new Promise<ImportBatch>((resolve) => {
        resolveBatch = resolve;
        void paths;
        void targetCollectionId;
      });

    render(<App client={client} />);
    await waitFor(() => {
      expect(client.calls).toContain("subscribeToFileDrops");
    });

    client.emitFileDrop(["C:\\Sources\\第一批.pdf"], null);
    await importKeepingExistingFlow();
    await waitFor(() => {
      expect(
        screen.getByLabelText("批量导入进度")
      ).toBeInTheDocument();
    });

    client.emitFileDrop(["C:\\Sources\\第二批.pdf"], null);
    expect(
      await screen.findByText(
        "已有导入批次正在运行，请等待完成后再拖入文件或文件夹。"
      )
    ).toBeInTheDocument();
    expect(
      screen.getByLabelText("批量导入进度")
    ).toBeInTheDocument();

    await act(async () => {
      resolveBatch?.({
        batchId: "batch-first",
        items: [],
        importedCount: 0,
        duplicateCount: 0,
        sourceChangedCount: 0,
        failedCount: 0,
        ignoredCount: 0,
        targetCollectionId: null
      });
    });
    expect(
      await screen.findByText("导入批次已完成")
    ).toBeInTheDocument();
  });
});
