import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

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
import { ClassificationRulesPanel } from "./components/ClassificationRules";

const library: LibrarySummary = {
  id: "library-rules",
  name: "规则资料库",
  path: "C:\\Documents\\规则资料库",
  createdAt: "2026-09-23T08:00:00Z"
};

const otherLibrary: LibrarySummary = {
  id: "library-other",
  name: "另一个资料库",
  path: "C:\\Documents\\另一个资料库",
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

function rule(overrides: Partial<ClassificationRule> & { id: string }): ClassificationRule {
  return {
    name: overrides.id,
    enabled: true,
    position: 1,
    fileNamePattern: "",
    fileType: null,
    sourceDirectory: null,
    collectionId: null,
    tagIds: [],
    ...overrides
  };
}

const invoiceRule = rule({
  id: "rule-invoice",
  name: "发票归档",
  position: 1,
  fileNamePattern: "发票",
  fileType: "PDF",
  collectionId: "finance",
  tagIds: ["tag-reimburse"]
});

const screenshotRule = rule({
  id: "rule-screenshot",
  name: "微信截图",
  position: 2,
  fileNamePattern: "截图",
  fileType: null,
  enabled: false,
  tagIds: ["tag-important"]
});

function libraryKey(value: LibrarySummary) {
  return `${value.id}:${value.path}`;
}

function createClient(
  rules: ClassificationRule[] = [invoiceRule, screenshotRule],
  target: LibrarySummary = library
) {
  return new FakeBackendClient({
    collections: structuredClone(collections),
    tags: structuredClone(tags),
    classificationRules: { [libraryKey(target)]: structuredClone(rules) }
  });
}

function renderPanel(client: BackendClient, value: LibrarySummary = library) {
  return render(
    <LibraryContext.Provider value={value}>
      <ClassificationRulesPanel
        client={client}
        collections={collections}
        tags={tags}
      />
    </LibraryContext.Provider>
  );
}

describe("分类规则编辑器", () => {
  it("lists rules in user order with conditions, target collection and tags", async () => {
    const client = createClient();
    renderPanel(client);

    const list = await screen.findByRole("list", { name: "分类规则列表" });
    const headings = within(list)
      .getAllByRole("heading", { level: 4 })
      .map((heading) => heading.textContent);
    expect(headings).toEqual(["发票归档", "微信截图"]);

    expect(
      screen.getByText(
        "文件名包含“发票”，且类型为 PDF → 归档到「财务」，添加标签：报销"
      )
    ).toBeInTheDocument();
    expect(
      screen.getByText("文件名包含“截图” → 不改变集合，添加标签：重要")
    ).toBeInTheDocument();
    expect(screen.getByText("已启用")).toBeInTheDocument();
    expect(screen.getByText("已停用")).toBeInTheDocument();
    expect(screen.getByText("第 1 条")).toBeInTheDocument();
    expect(screen.getByText("第 2 条")).toBeInTheDocument();
  });

  it("creates a rule with a condition, target collection and tags", async () => {
    const user = userEvent.setup();
    const client = createClient([]);
    renderPanel(client);
    await screen.findByText("还没有分类规则。新建规则后，导入的新文档会自动归档。");

    await user.click(screen.getByRole("button", { name: "新建规则" }));
    await user.type(screen.getByLabelText("规则名称"), "合同归档");
    await user.type(screen.getByLabelText("文件名匹配"), "合同");
    await user.selectOptions(screen.getByLabelText("文件类型"), "PDF");
    await user.selectOptions(screen.getByLabelText("目标集合"), "contracts");
    await user.click(screen.getByRole("checkbox", { name: "报销" }));
    await user.click(screen.getByRole("button", { name: "保存规则" }));

    await waitFor(() => {
      expect(client.calls).toContain("classificationRuleOperation:create");
    });
    expect(screen.queryByRole("form", { name: "新建分类规则" })).not.toBeInTheDocument();
    expect(
      await screen.findByText(
        "文件名包含“合同”，且类型为 PDF → 归档到「合同」，添加标签：报销"
      )
    ).toBeInTheDocument();
    expect(screen.getByText("第 1 条")).toBeInTheDocument();
  });

  it("rejects an empty rule without calling the backend", async () => {
    const user = userEvent.setup();
    const client = createClient([]);
    renderPanel(client);
    await screen.findByText(/还没有分类规则/);

    await user.click(screen.getByRole("button", { name: "新建规则" }));
    await user.click(screen.getByRole("button", { name: "保存规则" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "规则名称不能为空。"
    );

    await user.type(screen.getByLabelText("规则名称"), "空规则");
    await user.click(screen.getByRole("button", { name: "保存规则" }));
    expect(
      await screen.findByRole("alert")
    ).toHaveTextContent("请至少填写一个匹配条件，或指定目标集合与标签。");
    expect(
      client.calls.filter((call) => call.startsWith("classificationRuleOperation"))
    ).toEqual([]);
  });

  it("disables a rule through the enable switch and keeps it listed", async () => {
    const user = userEvent.setup();
    const client = createClient();
    renderPanel(client);
    await screen.findByRole("list", { name: "分类规则列表" });

    const toggle = screen.getByRole("checkbox", { name: "启用规则 发票归档" });
    expect(toggle).toBeChecked();
    await user.click(toggle);

    await waitFor(() => {
      expect(client.calls).toContain("classificationRuleOperation:setEnabled");
    });
    await waitFor(() => {
      expect(
        screen.getByRole("checkbox", { name: "启用规则 发票归档" })
      ).not.toBeChecked();
    });
    expect(screen.getAllByText("已停用")).toHaveLength(2);
    expect(screen.getByText("发票归档")).toBeInTheDocument();
  });

  it("reorders rules with up and down buttons and disables the boundaries", async () => {
    const user = userEvent.setup();
    const client = createClient();
    renderPanel(client);
    const list = await screen.findByRole("list", { name: "分类规则列表" });

    expect(
      screen.getByRole("button", { name: "上移规则 发票归档" })
    ).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "下移规则 微信截图" })
    ).toBeDisabled();

    await user.click(screen.getByRole("button", { name: "下移规则 发票归档" }));

    await waitFor(() => {
      expect(client.calls).toContain("classificationRuleOperation:reorder");
    });
    await waitFor(() => {
      const headings = within(list)
        .getAllByRole("heading", { level: 4 })
        .map((heading) => heading.textContent);
      expect(headings).toEqual(["微信截图", "发票归档"]);
    });
    expect(
      screen.getByRole("button", { name: "上移规则 微信截图" })
    ).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "下移规则 发票归档" })
    ).toBeDisabled();
    expect(screen.getByText("第 1 条")).toBeInTheDocument();
  });

  it("edits an existing rule and keeps its position", async () => {
    const user = userEvent.setup();
    const client = createClient();
    renderPanel(client);
    await screen.findByRole("list", { name: "分类规则列表" });

    await user.click(screen.getByRole("button", { name: "编辑规则 微信截图" }));
    const form = await screen.findByRole("form", { name: "编辑分类规则" });
    expect(within(form).getByLabelText("规则名称")).toHaveValue("微信截图");
    expect(within(form).getByLabelText("文件名匹配")).toHaveValue("截图");

    await user.clear(within(form).getByLabelText("文件名匹配"));
    await user.type(within(form).getByLabelText("文件名匹配"), "截图2026");
    await user.click(within(form).getByRole("button", { name: "保存规则" }));

    await waitFor(() => {
      expect(client.calls).toContain("classificationRuleOperation:update");
    });
    expect(
      await screen.findByText("文件名包含“截图2026” → 不改变集合，添加标签：重要")
    ).toBeInTheDocument();
    expect(screen.getByText("第 2 条")).toBeInTheDocument();
  });

  it("deletes a rule only after the inline confirmation", async () => {
    const user = userEvent.setup();
    const client = createClient();
    renderPanel(client);
    await screen.findByRole("list", { name: "分类规则列表" });

    await user.click(screen.getByRole("button", { name: "删除规则 发票归档" }));
    const confirm = await screen.findByRole("group", {
      name: "确认删除规则 发票归档"
    });
    await user.click(within(confirm).getByRole("button", { name: "取消" }));
    expect(
      client.calls.filter((call) => call === "classificationRuleOperation:delete")
    ).toEqual([]);

    await user.click(screen.getByRole("button", { name: "删除规则 发票归档" }));
    await user.click(
      within(
        await screen.findByRole("group", { name: "确认删除规则 发票归档" })
      ).getByRole("button", { name: "确认删除" })
    );

    await waitFor(() => {
      expect(client.calls).toContain("classificationRuleOperation:delete");
    });
    await waitFor(() => {
      expect(screen.queryByText("发票归档")).not.toBeInTheDocument();
    });
    expect(screen.getByText("微信截图")).toBeInTheDocument();
  });

  it("shows a retryable error when rules cannot be loaded", async () => {
    const user = userEvent.setup();
    const client = createClient();
    const load = client.listClassificationRules.bind(client);
    let failing = true;
    client.listClassificationRules = async (owner) => {
      if (failing) {
        failing = false;
        throw new BackendError({
          code: "store",
          message: "无法读取分类规则：资料库存储不可用。"
        });
      }
      return load(owner);
    };

    renderPanel(client);

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("无法读取分类规则：资料库存储不可用。");
    await user.click(within(alert).getByRole("button", { name: "重试" }));

    expect(await screen.findByText("发票归档")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("reloads the rules of the library in context", async () => {
    const client = new FakeBackendClient({
      collections: structuredClone(collections),
      tags: structuredClone(tags),
      classificationRules: {
        [libraryKey(library)]: [structuredClone(invoiceRule)],
        [libraryKey(otherLibrary)]: [
          structuredClone(rule({ id: "rule-other", name: "另一个库的规则" }))
        ]
      }
    });

    const { rerender } = renderPanel(client, library);
    expect(await screen.findByText("发票归档")).toBeInTheDocument();

    rerender(
      <LibraryContext.Provider value={otherLibrary}>
        <ClassificationRulesPanel
          client={client}
          collections={collections}
          tags={tags}
        />
      </LibraryContext.Provider>
    );

    expect(await screen.findByText("另一个库的规则")).toBeInTheDocument();
    expect(screen.queryByText("发票归档")).not.toBeInTheDocument();
  });
});
