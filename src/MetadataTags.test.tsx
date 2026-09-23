import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { App } from "./App";
import { BackendError } from "./backend/error";
import { FakeBackendClient } from "./backend/fakeClient";
import type {
  BootstrapState,
  CollectionSummary,
  DocumentMetadataUpdate,
  DocumentSummary,
  LibrarySummary,
  TagSummary
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

const workTag: TagSummary = {
  id: "tag-work",
  name: "工作",
  documentCount: 1
};

const importantTag: TagSummary = {
  id: "tag-important",
  name: "重要",
  documentCount: 0
};

const document: DocumentSummary = {
  id: "document-project",
  title: "项目文档",
  description: null,
  documentDate: null,
  fileName: "项目文档.md",
  fileType: "Markdown",
  fileSize: 24,
  contentHash: "hash-project",
  collectionId: "projects",
  tags: [workTag],
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
    parentId: null,
    isInbox: false,
    documentCount: 0
  }
];

function createClient(tags: TagSummary[] = [workTag, importantTag]) {
  return new FakeBackendClient({
    bootstrap,
    documents: [document],
    collections,
    tags
  });
}

describe("标签与文档元数据", () => {
  it("creates, renames, and deletes tags while keeping the document", async () => {
    const user = userEvent.setup();
    const client = createClient();

    render(<App client={client} />);
    await screen.findByText("项目文档");
    const tagSection = screen.getByRole("region", { name: "标签" });

    await user.click(
      within(tagSection).getByRole("button", { name: "创建标签" })
    );
    let dialog = screen.getByRole("dialog", { name: "创建标签" });
    await user.click(within(dialog).getByRole("button", { name: "创建" }));
    expect(within(dialog).getByRole("alert")).toHaveTextContent(
      "标签名称不能为空。"
    );

    await user.type(within(dialog).getByLabelText("标签名称"), "重要");
    await user.click(within(dialog).getByRole("button", { name: "创建" }));
    await waitFor(() => {
      expect(client.calls).toContain("createTag:重要");
    });

    await user.click(
      within(tagSection).getByRole("button", { name: "重命名标签 工作" })
    );
    dialog = screen.getByRole("dialog", { name: "重命名标签" });
    const nameInput = within(dialog).getByLabelText("标签名称");
    await user.clear(nameInput);
    await user.type(nameInput, "项目");
    await user.click(within(dialog).getByRole("button", { name: "保存" }));

    const row = screen.getByText("项目文档").closest("article");
    if (!row) {
      throw new Error("无法找到文档行。");
    }
    await waitFor(() => {
      expect(within(tagSection).getByText("项目")).toBeInTheDocument();
      expect(
        within(row).getByText("项目", { selector: ".document-tag" })
      ).toBeInTheDocument();
    });

    await user.click(
      within(tagSection).getByRole("button", { name: "删除标签 项目" })
    );
    const deleteDialog = screen.getByRole("dialog", { name: "删除标签" });
    expect(deleteDialog).toHaveTextContent("不会删除文档");
    await user.click(
      within(deleteDialog).getByRole("button", { name: "删除标签" })
    );

    await waitFor(() => {
      expect(
        within(tagSection).queryByText("项目")
      ).not.toBeInTheDocument();
    });
    expect(screen.getByText("项目文档")).toBeInTheDocument();
    expect(
      within(row).queryByText("项目", { selector: ".document-tag" })
    ).not.toBeInTheDocument();
  });

  it("validates, saves every metadata field, and changes selected tags", async () => {
    const user = userEvent.setup();
    const client = createClient();
    let submitted:
      | { documentId: string; update: DocumentMetadataUpdate }
      | undefined;
    let resolveSave: ((document: DocumentSummary) => void) | undefined;
    client.updateDocumentMetadata = async (_library, documentId, update) => {
      submitted = { documentId, update };
      return new Promise<DocumentSummary>((resolve) => {
        resolveSave = resolve;
      });
    };

    render(<App client={client} />);
    await screen.findByText("项目文档");
    await user.click(
      screen.getByRole("button", { name: "编辑 项目文档 元数据" })
    );

    const dialog = screen.getByRole("dialog", { name: "编辑文档" });
    const titleInput = within(dialog).getByLabelText("标题");
    await user.clear(titleInput);
    await user.click(within(dialog).getByRole("button", { name: "保存" }));
    expect(within(dialog).getByRole("alert")).toHaveTextContent(
      "文档标题不能为空。"
    );
    expect(submitted).toBeUndefined();

    await user.type(titleInput, "年度项目文档");
    await user.type(
      within(dialog).getByLabelText("描述"),
      "项目归档说明"
    );
    fireEvent.change(within(dialog).getByLabelText("文档日期"), {
      target: { value: "2025-01-15" }
    });
    await user.selectOptions(
      within(dialog).getByLabelText("所属集合"),
      "archive"
    );
    await user.click(within(dialog).getByRole("checkbox", { name: "工作" }));
    await user.click(within(dialog).getByRole("checkbox", { name: "重要" }));
    await user.click(
      within(dialog).getByRole("button", { name: "重试保存" })
    );

    expect(
      await within(dialog).findByRole("button", { name: "保存中" })
    ).toBeDisabled();
    expect(titleInput).toHaveValue("年度项目文档");
    await waitFor(() => {
      expect(submitted).toEqual({
        documentId: document.id,
        update: {
          title: "年度项目文档",
          description: "项目归档说明",
          documentDate: "2025-01-15",
          collectionId: "archive",
          tagIds: [importantTag.id]
        }
      });
    });

    await act(async () => {
      resolveSave?.({
        ...document,
        title: "年度项目文档",
        description: "项目归档说明",
        documentDate: "2025-01-15",
        collectionId: "archive",
        tags: [importantTag]
      });
    });

    const row = (await screen.findByText("年度项目文档")).closest("article");
    if (!row) {
      throw new Error("无法找到更新后的文档行。");
    }
    expect(within(row).getByText("2025-01-15")).toBeInTheDocument();
    expect(within(row).getByText("重要")).toBeInTheDocument();
    expect(
      screen.queryByRole("dialog", { name: "编辑文档" })
    ).not.toBeInTheDocument();
  });

  it("keeps entered values and retries after a metadata save failure", async () => {
    const user = userEvent.setup();
    const client = createClient();
    const originalUpdate = client.updateDocumentMetadata.bind(client);
    let attempts = 0;
    client.updateDocumentMetadata = async (owner, documentId, update) => {
      attempts += 1;
      if (attempts === 1) {
        throw new BackendError({
          code: "saveFailed",
          message: "无法保存文档元数据。"
        });
      }
      return originalUpdate(owner, documentId, update);
    };

    render(<App client={client} />);
    await screen.findByText("项目文档");
    await user.click(
      screen.getByRole("button", { name: "编辑 项目文档 元数据" })
    );

    const dialog = screen.getByRole("dialog", { name: "编辑文档" });
    const titleInput = within(dialog).getByLabelText("标题");
    await user.clear(titleInput);
    await user.type(titleInput, "保留的输入");
    await user.click(within(dialog).getByRole("button", { name: "保存" }));

    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "无法保存文档元数据。"
    );
    expect(titleInput).toHaveValue("保留的输入");
    await user.click(
      within(dialog).getByRole("button", { name: "重试保存" })
    );

    expect(await screen.findByText("保留的输入")).toBeInTheDocument();
    expect(attempts).toBe(2);
    expect(
      screen.queryByRole("dialog", { name: "编辑文档" })
    ).not.toBeInTheDocument();
  });
});
