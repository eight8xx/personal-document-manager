import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { BackendError } from "./backend/error";
import { FakeBackendClient } from "./backend/fakeClient";
import { LibraryContext } from "./backend/libraryContext";
import type {
  BackendClient,
  ClassificationRule,
  CollectionSummary,
  LibrarySummary,
  TagSummary
} from "./backend/types";
import {
  CLASSIFICATION_PREVIEW_PAGE_SIZE,
  ImportClassificationPreview
} from "./components/ImportClassificationPreview";

const library: LibrarySummary = {
  id: "library-import",
  name: "导入资料库",
  path: "C:\\Documents\\导入资料库",
  createdAt: "2026-09-23T08:00:00Z"
};

const collections: CollectionSummary[] = [
  { id: "inbox", name: "收件箱", parentId: null, isInbox: true, documentCount: 0 },
  { id: "finance", name: "财务", parentId: null, isInbox: false, documentCount: 0 },
  { id: "contracts", name: "合同", parentId: null, isInbox: false, documentCount: 0 }
];

const tags: TagSummary[] = [
  { id: "tag-reimburse", name: "报销", documentCount: 0 },
  { id: "tag-important", name: "重要", documentCount: 0 }
];

const invoiceRule: ClassificationRule = {
  id: "rule-invoice",
  name: "发票归档",
  enabled: true,
  position: 1,
  fileNamePattern: "发票",
  fileType: "PDF",
  sourceDirectory: null,
  collectionId: "finance",
  tagIds: ["tag-reimburse"]
};

const screenshotRule: ClassificationRule = {
  id: "rule-screenshot",
  name: "截图标签",
  enabled: true,
  position: 2,
  fileNamePattern: "截图",
  fileType: null,
  sourceDirectory: null,
  collectionId: null,
  tagIds: ["tag-important"]
};

const disabledRule: ClassificationRule = {
  id: "rule-disabled",
  name: "已停用的规则",
  enabled: false,
  position: 3,
  fileNamePattern: "",
  fileType: null,
  sourceDirectory: null,
  collectionId: "contracts",
  tagIds: ["tag-important"]
};

function createClient(rules: ClassificationRule[] = [invoiceRule, screenshotRule]) {
  return new FakeBackendClient({
    collections: structuredClone(collections),
    tags: structuredClone(tags),
    classificationRules: {
      [`${library.id}:${library.path}`]: structuredClone(rules)
    }
  });
}

interface RenderOptions {
  paths?: string[];
  targetCollectionId?: string | null;
}

function renderPreview(
  client: BackendClient,
  {
    paths = ["C:\\QQ\\发票2026.pdf", "C:\\QQ\\截图.png", "C:\\微信\\合同.docx"],
    targetCollectionId = null
  }: RenderOptions = {}
) {
  const onDecide = vi.fn();
  const onCancel = vi.fn();
  const view = render(
    <LibraryContext.Provider value={library}>
      <ImportClassificationPreview
        client={client}
        paths={paths}
        targetCollectionId={targetCollectionId}
        collections={collections}
        tags={tags}
        onDecide={onDecide}
        onCancel={onCancel}
      />
    </LibraryContext.Provider>
  );
  return { ...view, onDecide, onCancel };
}

describe("批量导入的分类预览", () => {
  it("previews the expected collection, tags and matched rules of every file", async () => {
    const client = createClient();
    renderPreview(client);

    const table = await screen.findByRole("table", { name: "分类预览" });
    const invoiceRow = within(table).getByRole("row", { name: /发票2026\.pdf/ });
    expect(within(invoiceRow).getByRole("cell", { name: "PDF" })).toBeInTheDocument();
    expect(
      within(invoiceRow).getByRole("cell", { name: "财务" })
    ).toBeInTheDocument();
    expect(
      within(invoiceRow).getByRole("cell", { name: "报销" })
    ).toBeInTheDocument();
    expect(
      within(invoiceRow).getByRole("cell", { name: "rule-invoice" })
    ).toBeInTheDocument();

    const screenshotRow = within(table).getByRole("row", {
      name: /截图\.png/
    });
    expect(
      within(screenshotRow).getByRole("cell", { name: "收件箱" })
    ).toBeInTheDocument();
    expect(
      within(screenshotRow).getByRole("cell", { name: "重要" })
    ).toBeInTheDocument();

    const contractRow = within(table).getByRole("row", { name: /合同\.docx/ });
    expect(
      within(contractRow).getByRole("cell", { name: "未命中规则" })
    ).toBeInTheDocument();
    expect(
      within(contractRow).getByRole("cell", { name: "无标签" })
    ).toBeInTheDocument();
  });

  it("keeps a tag-only match in the inbox and ignores disabled rules", async () => {
    const client = createClient([screenshotRule, disabledRule]);
    renderPreview(client, { paths: ["C:\\QQ\\截图.png", "C:\\QQ\\说明.pdf"] });

    const table = await screen.findByRole("table", { name: "分类预览" });
    const screenshotRow = within(table).getByRole("row", { name: /截图\.png/ });
    expect(
      within(screenshotRow).getByRole("cell", { name: "收件箱" })
    ).toBeInTheDocument();
    expect(
      within(screenshotRow).getByRole("cell", { name: "重要" })
    ).toBeInTheDocument();

    const pdfRow = within(table).getByRole("row", { name: /说明\.pdf/ });
    expect(within(pdfRow).getByRole("cell", { name: "未命中规则" })).toBeInTheDocument();
    expect(within(table).queryByText("rule-disabled")).not.toBeInTheDocument();
  });

  it("lets an explicit target collection win while rule tags still apply", async () => {
    const client = createClient();
    renderPreview(client, { targetCollectionId: "contracts" });

    const table = await screen.findByRole("table", { name: "分类预览" });
    const invoiceRow = within(table).getByRole("row", { name: /发票2026\.pdf/ });
    expect(
      within(invoiceRow).getByRole("cell", { name: "合同" })
    ).toBeInTheDocument();
    expect(
      within(invoiceRow).getByRole("cell", { name: "报销" })
    ).toBeInTheDocument();
    expect(
      await screen.findByRole("note")
    ).toHaveTextContent(
      "已指定目标集合「合同」，它优先于规则集合；命中的规则标签仍会应用。"
    );
  });

  it("pages the whole batch instead of asking per file", async () => {
    const user = userEvent.setup();
    const client = createClient();
    const paths = Array.from(
      { length: CLASSIFICATION_PREVIEW_PAGE_SIZE + 2 },
      (_, index) => `C:\\QQ\\发票-${index + 1}.pdf`
    );
    renderPreview(client, { paths });

    const table = await screen.findByRole("table", { name: "分类预览" });
    expect(within(table).getAllByRole("row")).toHaveLength(
      CLASSIFICATION_PREVIEW_PAGE_SIZE + 1
    );
    expect(screen.getByText(/第 1 \/ 2 页/)).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "分类预览上一页" })
    ).toBeDisabled();
    // 只有这一个对话框，不逐项弹窗。
    expect(screen.getAllByRole("dialog")).toHaveLength(1);

    await user.click(screen.getByRole("button", { name: "分类预览下一页" }));

    expect(screen.getByText(/第 2 \/ 2 页/)).toBeInTheDocument();
    expect(within(table).getAllByRole("row")).toHaveLength(3);
    expect(
      screen.getByRole("rowheader", { name: `发票-${CLASSIFICATION_PREVIEW_PAGE_SIZE + 2}.pdf` })
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "分类预览下一页" })
    ).toBeDisabled();
    expect(client.calls).toEqual(["previewClassification:12"]);
  });

  it("keeps the existing import behaviour when the user declines the rules", async () => {
    const user = userEvent.setup();
    const client = createClient();
    const { onDecide, onCancel } = renderPreview(client);
    await screen.findByRole("table", { name: "分类预览" });

    await user.click(
      screen.getByRole("button", { name: "不应用，按原有方式导入" })
    );

    expect(onDecide).toHaveBeenCalledWith("keepExisting");
    expect(onCancel).not.toHaveBeenCalled();
    // 预览组件自己不导入任何文件。
    expect(
      client.calls.filter((call) => !call.startsWith("previewClassification"))
    ).toEqual([]);
  });

  it("applies the rules only after the user confirms", async () => {
    const user = userEvent.setup();
    const client = createClient();
    const { onDecide } = renderPreview(client);
    await screen.findByRole("table", { name: "分类预览" });

    const apply = screen.getByRole("button", { name: "应用分类规则并导入" });
    expect(apply).toBeEnabled();
    await user.click(apply);

    expect(onDecide).toHaveBeenCalledWith("applyRules");
    expect(
      client.calls.filter((call) => !call.startsWith("previewClassification"))
    ).toEqual([]);
  });

  it("shows a retryable error when the preview cannot be computed", async () => {
    const user = userEvent.setup();
    const client = createClient();
    const preview = client.previewClassification.bind(client);
    let failing = true;
    let attempts = 0;
    client.previewClassification = async (owner, request) => {
      attempts += 1;
      if (failing) {
        failing = false;
        throw new BackendError({
          code: "store",
          message: "无法计算分类结果：分类规则存储不可用。"
        });
      }
      return preview(owner, request);
    };

    renderPreview(client);

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("无法计算分类结果：分类规则存储不可用。");
    expect(
      screen.getByRole("button", { name: "应用分类规则并导入" })
    ).toBeDisabled();

    await user.click(within(alert).getByRole("button", { name: "重试预览" }));

    expect(await screen.findByRole("table", { name: "分类预览" })).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(attempts).toBe(2);
    expect(
      client.calls.filter((call) => call.startsWith("previewClassification"))
    ).toHaveLength(1);
  });

  it("cancels without deciding when the dialog is closed", async () => {
    const user = userEvent.setup();
    const client = createClient();
    const { onDecide, onCancel } = renderPreview(client);
    await screen.findByRole("table", { name: "分类预览" });

    await user.click(screen.getByRole("button", { name: "关闭分类预览" }));

    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onDecide).not.toHaveBeenCalled();

    await user.keyboard("{Escape}");
    expect(onCancel).toHaveBeenCalledTimes(2);
  });

  it("shows an empty state when there is nothing to import", async () => {
    const client = createClient();
    renderPreview(client, { paths: [] });

    expect(await screen.findByText("没有待导入的文件。")).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
    await waitFor(() => {
      expect(
        screen.getByRole("button", { name: "应用分类规则并导入" })
      ).toBeDisabled();
    });
    expect(client.calls).toEqual([]);
  });
});
