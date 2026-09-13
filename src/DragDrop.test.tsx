import {
  act,
  fireEvent,
  render,
  screen,
  waitFor
} from "@testing-library/react";
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

    await waitFor(() => {
      expect(client.calls).toContain(
        "startImport:C:\\Sources\\新增文件.md|C:\\Sources\\待导入文件夹:projects"
      );
    });
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: originalElementFromPoint
    });
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
