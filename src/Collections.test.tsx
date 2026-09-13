import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import "./styles.css";
import { App } from "./App";
import { BackendError } from "./backend/error";
import { FakeBackendClient } from "./backend/fakeClient";
import type {
  BootstrapState,
  CollectionSummary,
  DocumentSummary,
  LibrarySummary
} from "./backend/types";

const library: LibrarySummary = {
  id: "library-current",
  name: "个人资料",
  path: "C:\\Documents\\个人资料",
  createdAt: "2026-09-13T08:00:00Z"
};

const bootstrap: BootstrapState = {
  currentLibrary: library,
  recentLibraries: []
};

const projectDocument: DocumentSummary = {
  id: "document-project",
  title: "项目文档",
  description: null,
  documentDate: null,
  fileName: "项目文档.md",
  fileType: "Markdown",
  fileSize: 24,
  contentHash: "hash-project",
  collectionId: "projects",
  tags: [],
  processingStatus: "ready",
  indexStatus: "searchable",
  errorStage: null,
  errorMessage: null,
  importedAt: "2026-09-13T08:10:00Z",
  sourcePath: "C:\\Documents\\项目文档.md",
  sourceIdentifier: "c:\\documents\\项目文档.md",
  lastImportedAt: "2026-09-13T08:10:00Z"
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
  },
  {
    id: "archive",
    name: "归档",
    parentId: "projects",
    isInbox: false,
    documentCount: 0
  }
];

async function clickCollectionAction(
  user: ReturnType<typeof userEvent.setup>,
  button: HTMLElement
) {
  const row = button.closest(".collection-row");
  if (!row) {
    throw new Error("无法找到集合操作行。");
  }
  await user.hover(row);
  await user.click(button);
}

describe("集合管理流程", () => {
  it("keeps the full collection name available when the row is constrained", async () => {
    const longName = "跨区域项目交付与长期归档资料";
    const client = new FakeBackendClient({
      bootstrap,
      collections: [
        ...collections,
        {
          id: "long-name",
          name: longName,
          parentId: null,
          isInbox: false,
          documentCount: 12
        }
      ],
      documents: [projectDocument]
    });

    render(<App client={client} />);
    const button = await screen.findByRole("button", {
      name: new RegExp(`^${longName}`)
    });
    const name = within(button).getByText(longName);
    const row = button.closest(".collection-row");
    const actions = row?.querySelector(".collection-actions");

    expect(button).toHaveAttribute("title", longName);
    expect(name).toHaveClass("collection-name");
    expect(actions).not.toBeNull();
    expect(window.getComputedStyle(actions as Element).position).toBe(
      "absolute"
    );
  });

  it("creates nested collections, renames them, and moves them", async () => {
    const user = userEvent.setup();
    const client = new FakeBackendClient({
      bootstrap,
      collections,
      documents: [projectDocument]
    });

    render(<App client={client} />);
    await screen.findByRole("button", { name: /全部文档/ });

    expect(
      screen.queryByRole("button", { name: "重命名收件箱" })
    ).toBeNull();
    expect(
      screen.queryByRole("button", { name: "删除收件箱" })
    ).toBeNull();

    await user.click(screen.getByRole("button", { name: "创建根集合" }));
    let dialog = screen.getByRole("dialog", { name: "创建根集合" });
    await user.type(within(dialog).getByLabelText("集合名称"), "资料");
    await user.click(within(dialog).getByRole("button", { name: "创建" }));

    expect(
      await screen.findByRole("button", { name: /资料 0/ })
    ).toBeInTheDocument();

    await clickCollectionAction(
      user,
      screen.getByRole("button", { name: "在资料中创建子集合" })
    );
    dialog = screen.getByRole("dialog", { name: "创建子集合" });
    await user.type(within(dialog).getByLabelText("集合名称"), "合同");
    await user.click(within(dialog).getByRole("button", { name: "创建" }));

    await clickCollectionAction(
      user,
      await screen.findByRole("button", { name: "重命名合同" })
    );
    dialog = screen.getByRole("dialog", { name: "重命名集合" });
    const renameInput = within(dialog).getByLabelText("集合名称");
    await user.clear(renameInput);
    await user.type(renameInput, "已签合同");
    await user.click(within(dialog).getByRole("button", { name: "保存" }));

    await clickCollectionAction(
      user,
      await screen.findByRole("button", { name: "移动已签合同" })
    );
    dialog = screen.getByRole("dialog", { name: "移动集合" });
    await user.selectOptions(
      within(dialog).getByLabelText("目标位置"),
      "projects"
    );
    await user.click(within(dialog).getByRole("button", { name: "移动" }));

    await waitFor(() => {
      expect(client.calls).toContain("moveCollection:collection-2:projects");
    });
    expect(client.calls).toContain(
      "createCollection:资料:null"
    );
    expect(client.calls).toContain(
      "createCollection:合同:collection-1"
    );
    expect(client.calls).toContain(
      "renameCollection:collection-2:已签合同"
    );
  });

  it("shows the document count and moves documents to the inbox on deletion", async () => {
    const user = userEvent.setup();
    const client = new FakeBackendClient({
      bootstrap,
      collections,
      documents: [projectDocument]
    });

    render(<App client={client} />);
    await screen.findByText("项目文档");
    await clickCollectionAction(
      user,
      screen.getByRole("button", { name: "删除项目" })
    );

    const dialog = screen.getByRole("dialog", { name: "删除集合" });
    expect(dialog).toHaveTextContent(
      "“项目”中有 1 份文档。删除集合后，这些文档将移动到收件箱。"
    );
    await user.click(
      within(dialog).getByRole("button", { name: "删除集合" })
    );

    await waitFor(() => {
      expect(client.calls).toContain("deleteCollection:projects");
    });
    expect(
      screen.queryByRole("button", { name: "删除项目" })
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /收件箱 1/ })).toBeInTheDocument();
  });

  it("surfaces collection deletion errors in the status alert", async () => {
    const user = userEvent.setup();
    const client = new FakeBackendClient({
      bootstrap,
      collections,
      documents: [projectDocument]
    });
    client.deleteCollection = async () => {
      throw new BackendError({
        code: "deleteFailed",
        message: "无法删除集合。"
      });
    };

    render(<App client={client} />);
    await screen.findByText("项目文档");
    await clickCollectionAction(
      user,
      screen.getByRole("button", { name: "删除项目" })
    );
    const dialog = screen.getByRole("dialog", { name: "删除集合" });
    await user.click(
      within(dialog).getByRole("button", { name: "删除集合" })
    );

    const alerts = await screen.findAllByRole("alert");
    expect(
      alerts.some((alert) => alert.textContent?.includes("无法删除集合。"))
    ).toBe(true);
    expect(
      screen.getByRole("dialog", { name: "删除集合" })
    ).toBeInTheDocument();
  });

  it("updates the active collection immediately after moving a document", async () => {
    const user = userEvent.setup();
    const client = new FakeBackendClient({
      bootstrap,
      collections,
      documents: [projectDocument]
    });

    render(<App client={client} />);
    await screen.findByText("项目文档");
    await user.click(screen.getByRole("button", { name: /^项目 1$/ }));

    expect(
      screen.getByRole("combobox", { name: "移动 项目文档 到集合" })
    ).toBeInTheDocument();
    await user.selectOptions(
      screen.getByRole("combobox", { name: "移动 项目文档 到集合" }),
      "archive"
    );

    await waitFor(() => {
      expect(client.calls).toContain(
        "moveDocumentToCollection:document-project:archive"
      );
    });
    expect(screen.queryByText("项目文档")).not.toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "这个集合中没有文档" })
    ).toBeInTheDocument();
  });

  it("prevents selecting a descendant as the move target", async () => {
    const user = userEvent.setup();
    const client = new FakeBackendClient({
      bootstrap,
      collections,
      documents: [projectDocument]
    });

    render(<App client={client} />);
    await screen.findByText("项目文档");
    await clickCollectionAction(
      user,
      screen.getByRole("button", { name: "移动项目" })
    );

    const dialog = screen.getByRole("dialog", { name: "移动集合" });
    const target = within(dialog).getByLabelText("目标位置");
    expect(
      within(target).queryByRole("option", { name: "归档" })
    ).not.toBeInTheDocument();
  });
});
