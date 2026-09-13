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

async function openCollectionMenu(
  user: ReturnType<typeof userEvent.setup>,
  collectionName: string
) {
  await user.click(
    screen.getByRole("button", { name: `管理 ${collectionName}` })
  );
  return screen.getByRole("menu", { name: `管理 ${collectionName}` });
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
    const menu = row?.querySelector(".collection-menu");
    const trigger = within(row as HTMLElement).getByRole("button", {
      name: `管理 ${longName}`
    });

    expect(button).toHaveAttribute("title", longName);
    expect(name).toHaveClass("collection-name");
    expect(menu).not.toBeNull();
    expect(trigger).toHaveAttribute("title", `管理 ${longName}`);
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("renders the collection menu outside the collection scroll container", async () => {
    const user = userEvent.setup();
    const client = new FakeBackendClient({
      bootstrap,
      collections,
      documents: [projectDocument]
    });

    render(<App client={client} />);
    await screen.findByRole("button", { name: "管理 项目" });
    const menu = await openCollectionMenu(user, "项目");
    const scrollContainer = document.querySelector(".collection-tree");

    expect(scrollContainer).not.toBeNull();
    expect(scrollContainer).not.toContainElement(menu);
    expect(menu.parentElement).toBe(document.body);
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
    await screen.findByRole("button", { name: "管理 收件箱" });

    const inboxMenu = await openCollectionMenu(user, "收件箱");
    expect(
      within(inboxMenu).getAllByRole("menuitem").map((item) => item.textContent)
    ).toEqual(["创建子集合"]);
    await user.keyboard("{Escape}");

    await user.click(screen.getByRole("button", { name: "创建根集合" }));
    let dialog = screen.getByRole("dialog", { name: "创建根集合" });
    await user.type(within(dialog).getByLabelText("集合名称"), "资料");
    await user.click(within(dialog).getByRole("button", { name: "创建" }));

    expect(
      await screen.findByRole("button", { name: /资料 0/ })
    ).toBeInTheDocument();

    let menu = await openCollectionMenu(user, "资料");
    await user.click(
      within(menu).getByRole("menuitem", { name: "创建子集合" })
    );
    dialog = screen.getByRole("dialog", { name: "创建子集合" });
    await user.type(within(dialog).getByLabelText("集合名称"), "合同");
    await user.click(within(dialog).getByRole("button", { name: "创建" }));

    await screen.findByRole("button", { name: /合同 0/ });
    menu = await openCollectionMenu(user, "合同");
    await user.click(
      within(menu).getByRole("menuitem", { name: "重命名" })
    );
    dialog = screen.getByRole("dialog", { name: "重命名集合" });
    const renameInput = within(dialog).getByLabelText("集合名称");
    await user.clear(renameInput);
    await user.type(renameInput, "已签合同");
    await user.click(within(dialog).getByRole("button", { name: "保存" }));

    await screen.findByRole("button", { name: /已签合同 0/ });
    menu = await openCollectionMenu(user, "已签合同");
    await user.click(
      within(menu).getByRole("menuitem", { name: "移动" })
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
    const menu = await openCollectionMenu(user, "项目");
    await user.click(
      within(menu).getByRole("menuitem", { name: "删除" })
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
      screen.queryByRole("button", { name: "管理 项目" })
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
    const menu = await openCollectionMenu(user, "项目");
    await user.click(
      within(menu).getByRole("menuitem", { name: "删除" })
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
    const menu = await openCollectionMenu(user, "项目");
    await user.click(
      within(menu).getByRole("menuitem", { name: "移动" })
    );

    const dialog = screen.getByRole("dialog", { name: "移动集合" });
    const target = within(dialog).getByLabelText("目标位置");
    expect(
      within(target).queryByRole("option", { name: "归档" })
    ).not.toBeInTheDocument();
  });

  it("selects collections from the row without opening its menu", async () => {
    const user = userEvent.setup();
    const client = new FakeBackendClient({
      bootstrap,
      collections,
      documents: [projectDocument]
    });

    render(<App client={client} />);
    const projects = await screen.findByRole("button", { name: /^项目 1$/ });

    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    await user.click(projects);

    expect(projects).toHaveAttribute("aria-current", "page");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("supports keyboard opening and closing with focus restoration", async () => {
    const user = userEvent.setup();
    const client = new FakeBackendClient({
      bootstrap,
      collections,
      documents: [projectDocument]
    });

    render(<App client={client} />);
    const trigger = await screen.findByRole("button", {
      name: "管理 项目"
    });

    trigger.focus();
    await user.keyboard("{Enter}");
    expect(
      screen.getByRole("menu", { name: "管理 项目" })
    ).toBeInTheDocument();

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();

    await user.keyboard(" ");
    expect(
      screen.getByRole("menu", { name: "管理 项目" })
    ).toBeInTheDocument();
  });

  it("closes the open menu on an outside click and only opens one at a time", async () => {
    const user = userEvent.setup();
    const client = new FakeBackendClient({
      bootstrap,
      collections,
      documents: [projectDocument]
    });

    render(<App client={client} />);
    await screen.findByRole("button", { name: "管理 项目" });

    await user.click(screen.getByRole("button", { name: "管理 项目" }));
    expect(
      screen.getByRole("menu", { name: "管理 项目" })
    ).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "管理 归档" }));
    expect(screen.getAllByRole("menu")).toHaveLength(1);
    expect(
      screen.getByRole("menu", { name: "管理 归档" })
    ).toBeInTheDocument();

    await user.click(document.body);
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("keeps the current collection selected when a menu action is chosen", async () => {
    const user = userEvent.setup();
    const client = new FakeBackendClient({
      bootstrap,
      collections,
      documents: [projectDocument]
    });

    render(<App client={client} />);
    const projects = await screen.findByRole("button", { name: /^项目 1$/ });
    await user.click(projects);

    const menu = await openCollectionMenu(user, "归档");
    await user.click(
      within(menu).getByRole("menuitem", { name: "重命名" })
    );

    expect(
      screen.getByRole("dialog", { name: "重命名集合" })
    ).toBeInTheDocument();
    expect(projects).toHaveAttribute("aria-current", "page");
    expect(
      screen.getByRole("button", { name: /^归档 0$/ })
    ).not.toHaveAttribute("aria-current");
  });
});
