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
  TagSummary,
  TrashDocumentSummary
} from "./backend/types";

const library: LibrarySummary = {
  id: "library-trash",
  name: "回收站资料库",
  path: "C:\\Documents\\回收站资料库",
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
    documentCount: 0
  },
  {
    id: "projects",
    name: "项目",
    parentId: null,
    isInbox: false,
    documentCount: 1
  }
];

const tag: TagSummary = {
  id: "important",
  name: "重要",
  documentCount: 1
};

function documentFor(
  id: string,
  title: string,
  collectionId = "projects"
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
    tags: [tag],
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

function trashSummary(
  document: DocumentSummary,
  originalCollectionName: string | null = "项目"
): TrashDocumentSummary {
  return {
    document,
    originalCollectionId: document.collectionId,
    originalCollectionName,
    deletedAt: "2026-09-13T09:00:00Z"
  };
}

describe("回收站、恢复与永久删除", () => {
  it("moves a document to trash and restores it to the original collection", async () => {
    const user = userEvent.setup();
    const document = documentFor("project", "项目文档");
    const client = new FakeBackendClient({
      bootstrap,
      documents: [document],
      collections,
      tags: [tag]
    });

    render(<App client={client} />);
    await screen.findByText("项目文档");

    await user.click(
      screen.getByRole("button", {
        name: "将 项目文档 移入回收站"
      })
    );
    let dialog = screen.getByRole("dialog", { name: "移入回收站" });
    expect(dialog).toHaveTextContent("资料库副本会保留");
    await user.click(
      within(dialog).getByRole("button", { name: "移入回收站" })
    );

    await waitFor(() => {
      expect(screen.queryByText("项目文档")).not.toBeInTheDocument();
    });
    expect(client.calls).toContain("moveDocumentToTrash:project");
    await user.click(screen.getByRole("button", { name: /回收站 1/ }));

    const trash = await screen.findByRole("main", { name: "回收站" });
    expect(within(trash).getByText("项目文档")).toBeInTheDocument();
    expect(within(trash).getByText("项目")).toBeInTheDocument();
    await user.click(
      within(trash).getByRole("button", { name: "恢复 项目文档" })
    );

    expect(
      await within(trash).findByText("回收站为空")
    ).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /全部文档 1/ }));
    const row = (await screen.findByText("项目文档")).closest("article");
    expect(row).not.toBeNull();
    expect(within(row!).getByText("项目")).toBeInTheDocument();
  });

  it("requires confirmation and keeps the dialog open after a permanent-delete failure", async () => {
    const user = userEvent.setup();
    const document = documentFor("report", "年度报告");
    const client = new FakeBackendClient({
      bootstrap,
      trashDocuments: [trashSummary(document)],
      collections,
      tags: [tag]
    });
    const originalDelete = client.permanentlyDeleteDocument.bind(client);
    let attempts = 0;
    client.permanentlyDeleteDocument = async (documentId) => {
      attempts += 1;
      if (attempts === 1) {
        throw new BackendError({
          code: "deleteFailed",
          message: "无法删除资料库副本。"
        });
      }
      return originalDelete(documentId);
    };

    render(<App client={client} />);
    await screen.findByRole("button", { name: /回收站 1/ });
    await user.click(screen.getByRole("button", { name: /回收站 1/ }));
    await user.click(
      await screen.findByRole("button", { name: "永久删除 年度报告" })
    );

    const dialog = screen.getByRole("dialog", { name: "永久删除文档" });
    expect(dialog).toHaveTextContent("无法恢复");
    expect(dialog).toHaveTextContent("源文件不会被删除");
    await user.click(
      within(dialog).getByRole("button", { name: "永久删除" })
    );

    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "无法删除资料库副本。"
    );
    expect(
      screen.getByRole("button", { name: "永久删除 年度报告" })
    ).toBeInTheDocument();

    await user.click(
      within(dialog).getByRole("button", { name: "永久删除" })
    );
    expect(await screen.findByText("回收站为空")).toBeInTheDocument();
    expect(attempts).toBe(2);
  });

  it("shows the trash count, confirms emptying, and recovers from a failure", async () => {
    const user = userEvent.setup();
    const first = documentFor("first", "第一份");
    const second = documentFor("second", "第二份", "inbox");
    const client = new FakeBackendClient({
      bootstrap,
      trashDocuments: [
        trashSummary(first),
        trashSummary(second, null)
      ],
      collections,
      tags: [tag]
    });
    const originalEmpty = client.emptyTrash.bind(client);
    let attempts = 0;
    client.emptyTrash = async () => {
      attempts += 1;
      if (attempts === 1) {
        throw new BackendError({
          code: "emptyTrashFailed",
          message: "清空回收站时发生错误。"
        });
      }
      return originalEmpty();
    };

    render(<App client={client} />);
    await screen.findByRole("button", { name: /回收站 2/ });
    await user.click(screen.getByRole("button", { name: /回收站 2/ }));

    const trash = await screen.findByRole("main", { name: "回收站" });
    expect(within(trash).getByText("原集合已删除")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "清空回收站" }));

    const dialog = screen.getByRole("dialog", { name: "清空回收站" });
    expect(dialog).toHaveTextContent("2 份文档");
    await user.click(
      within(dialog).getByRole("button", { name: "清空回收站" })
    );
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "清空回收站时发生错误。"
    );

    await user.click(
      within(dialog).getByRole("button", { name: "清空回收站" })
    );
    expect(await screen.findByText("回收站为空")).toBeInTheDocument();
    expect(attempts).toBe(2);
  });
});
