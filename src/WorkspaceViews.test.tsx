import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import "./styles.css";
import { App } from "./App";
import { FakeBackendClient } from "./backend/fakeClient";
import type {
  BootstrapState,
  CollectionSummary,
  DocumentSummary,
  LibrarySummary,
  TagSummary
} from "./backend/types";
import { DocumentEmptyState } from "./components/DocumentEmptyState";

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

const tags: TagSummary[] = [
  { id: "tag-work", name: "工作", documentCount: 1 },
  { id: "tag-important", name: "重要", documentCount: 1 },
  { id: "tag-archive", name: "长期归档", documentCount: 1 },
  { id: "tag-review", name: "待复核", documentCount: 1 },
  { id: "tag-personal", name: "个人", documentCount: 1 },
  { id: "tag-unused", name: "未使用", documentCount: 0 }
];

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

const longTitle =
  "2026 年度跨区域项目交付、验收与长期归档说明文档（最终确认版本）";

const projectDocument: DocumentSummary = {
  id: "document-project",
  title: longTitle,
  description: "用于验证三栏工作台动态内容布局。",
  documentDate: "2025-01-15",
  fileName: `${longTitle}.pdf`,
  fileType: "PDF",
  fileSize: 2048,
  contentHash: "hash-project",
  collectionId: "projects",
  tags: tags.slice(0, 5),
  processingStatus: "ready",
  indexStatus: "searchable",
  errorStage: null,
  errorMessage: null,
  importedAt: "2026-09-13T08:10:00Z",
  sourcePath: `C:\\Documents\\${longTitle}.pdf`,
  sourceIdentifier: `c:\\documents\\${longTitle}.pdf`,
  lastImportedAt: "2026-09-13T08:10:00Z"
};

function createClient(documents: DocumentSummary[] = [projectDocument]) {
  return new FakeBackendClient({
    bootstrap,
    documents,
    collections,
    tags
  });
}

function mockColorScheme(initialMatches: boolean) {
  let listener: (() => void) | undefined;
  const mediaQuery = {
    matches: initialMatches,
    media: "(prefers-color-scheme: dark)",
    onchange: null,
    addEventListener: (_type: string, nextListener: () => void) => {
      listener = nextListener;
    },
    removeEventListener: () => {
      listener = undefined;
    },
    addListener: (nextListener: () => void) => {
      listener = nextListener;
    },
    removeListener: () => {
      listener = undefined;
    },
    dispatchEvent: () => true
  };

  vi.stubGlobal("matchMedia", vi.fn(() => mediaQuery));

  return {
    setMatches(matches: boolean) {
      mediaQuery.matches = matches;
      listener?.();
    }
  };
}

beforeEach(() => {
  window.sessionStorage.clear();
});

afterEach(() => {
  delete document.documentElement.dataset.theme;
  vi.unstubAllGlobals();
});

describe("三栏工作台与列表、网格视图", () => {
  it("renders the three semantic panes and every default list field", async () => {
    const user = userEvent.setup();
    render(<App client={createClient()} />);

    const navigation = await screen.findByRole("complementary", {
      name: "集合与标签"
    });
    const results = await screen.findByRole("main", {
      name: "文档列表"
    });
    const details = screen.getByRole("complementary", {
      name: "文档详情"
    });

    expect(navigation).toBeInTheDocument();
    expect(details).toHaveTextContent("选择文档查看详情");

    const table = within(results).getByRole("table", { name: "文档结果" });
    for (const heading of [
      "标题",
      "类型",
      "文档日期",
      "集合",
      "标签",
      "处理状态"
    ]) {
      expect(
        within(table).getByRole("columnheader", { name: heading })
      ).toBeInTheDocument();
    }

    const row = within(table).getByRole("row", {
      name: new RegExp(longTitle)
    });
    expect(row).toHaveTextContent("PDF");
    expect(row).toHaveTextContent("2025-01-15");
    expect(row).toHaveTextContent("项目");
    expect(row).toHaveTextContent("工作");
    expect(row).toHaveTextContent("+2");
    expect(row).toHaveTextContent("可搜索");

    await user.click(
      within(row).getByRole("button", {
        name: `选择文档 ${longTitle}`
      })
    );

    expect(
      within(details).getByRole("heading", { name: /当前文档/ })
    ).toHaveTextContent(longTitle);
    expect(within(details).getByText("2025-01-15")).toBeInTheDocument();
    expect(within(details).getByText("项目")).toBeInTheDocument();
    expect(within(details).getByText("工作")).toBeInTheDocument();
    expect(
      within(details).getByRole("region", { name: "文档预览" })
    ).toBeInTheDocument();
  });

  it("filters by collection and tag with distinct empty states", async () => {
    const user = userEvent.setup();
    render(<App client={createClient()} />);
    await screen.findByText(longTitle);

    await user.click(screen.getByRole("button", { name: /^归档 0$/ }));
    expect(
      screen.getByRole("main", { name: "空集合" })
    ).toHaveTextContent("这个集合中没有文档");

    await user.click(
      screen.getByRole("button", { name: "按标签 未使用 筛选" })
    );
    expect(
      screen.getByRole("main", { name: "空标签" })
    ).toHaveTextContent("这个标签下没有文档");
  });

  it("keeps a distinct no-search-results boundary for the search work item", () => {
    render(<DocumentEmptyState kind="search" />);

    expect(
      screen.getByRole("main", { name: "无搜索结果" })
    ).toHaveTextContent("没有找到匹配的文档");
  });

  it("switches views and remembers the choice for this session", async () => {
    const user = userEvent.setup();
    const firstRender = render(<App client={createClient()} />);
    await screen.findByRole("main", { name: "文档列表" });

    const listButton = screen.getByRole("button", { name: "列表视图" });
    const gridButton = screen.getByRole("button", { name: "网格视图" });
    expect(listButton).toHaveAttribute("aria-pressed", "true");
    expect(gridButton).toHaveAttribute("aria-pressed", "false");

    await user.click(gridButton);
    expect(
      screen.getByRole("main", { name: "文档网格" })
    ).toBeInTheDocument();
    expect(gridButton).toHaveAttribute("aria-pressed", "true");

    firstRender.unmount();
    render(<App client={createClient()} />);

    expect(
      await screen.findByRole("main", { name: "文档网格" })
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "网格视图" })
    ).toHaveAttribute("aria-pressed", "true");
  });

  it("keeps list rows and grid cards at stable sizes with long content", async () => {
    const shortDocument: DocumentSummary = {
      ...projectDocument,
      id: "document-short",
      title: "短标题",
      fileName: "短标题.md",
      fileType: "Markdown",
      tags: []
    };
    const user = userEvent.setup();
    render(<App client={createClient([projectDocument, shortDocument])} />);
    await screen.findByText(longTitle);

    const list = screen.getByRole("main", { name: "文档列表" });
    const rows = within(list)
      .getAllByRole("row")
      .filter((row) => row.getAttribute("role") === "row" && row.closest(
        ".document-list"
      ));
    expect(rows).toHaveLength(2);
    expect(window.getComputedStyle(rows[0]).height).toBe("72px");
    expect(window.getComputedStyle(rows[1]).height).toBe(
      window.getComputedStyle(rows[0]).height
    );
    expect(window.getComputedStyle(rows[0]).overflow).toBe("hidden");

    await user.click(screen.getByRole("button", { name: "网格视图" }));
    const grid = screen.getByRole("list", { name: "文档结果" });
    const cards = within(grid).getAllByRole("listitem");
    expect(cards).toHaveLength(2);
    expect(window.getComputedStyle(cards[0]).height).toBe("236px");
    expect(window.getComputedStyle(cards[1]).height).toBe(
      window.getComputedStyle(cards[0]).height
    );
    expect(window.getComputedStyle(cards[0]).overflow).toBe("hidden");
  });

  it("moves keyboard focus from navigation through results to details", async () => {
    const user = userEvent.setup();
    render(<App client={createClient()} />);
    await screen.findByText(longTitle);

    const allDocuments = screen.getByRole("button", { name: /全部文档/ });
    allDocuments.focus();
    expect(allDocuments).toHaveFocus();

    const selection = screen.getByRole("button", {
      name: `选择文档 ${longTitle}`
    });
    await user.click(selection);
    selection.focus();
    expect(selection).toHaveFocus();
    const row = selection.closest("article");
    if (!row) {
      throw new Error("无法找到文档行。");
    }

    await user.tab();
    expect(
      screen.getByRole("combobox", {
        name: `移动 ${longTitle} 到集合`
      })
    ).toHaveFocus();

    await user.tab();
    expect(
      within(row).getByRole("button", {
        name: `将 ${longTitle} 移入回收站`
      })
    ).toHaveFocus();

    await user.tab();
    expect(
      screen.getByRole("button", {
        name: `编辑 ${longTitle} 元数据`
      })
    ).toHaveFocus();

    await user.tab();
    expect(
      screen.getByRole("button", {
        name: `在详情中编辑 ${longTitle} 的元数据`
      })
    ).toHaveFocus();
  });

  it("applies the operating system theme to the semantic workspace", async () => {
    const colorScheme = mockColorScheme(true);
    render(<App client={createClient()} />);

    expect(
      await screen.findByRole("complementary", { name: "集合与标签" })
    ).toBeInTheDocument();
    expect(document.documentElement).toHaveAttribute("data-theme", "dark");
    expect(
      window
        .getComputedStyle(document.documentElement)
        .getPropertyValue("--bg")
        .trim()
    ).toBe("#111716");

    act(() => {
      colorScheme.setMatches(false);
    });

    expect(document.documentElement).toHaveAttribute("data-theme", "light");
    expect(
      window
        .getComputedStyle(document.documentElement)
        .getPropertyValue("--bg")
        .trim()
    ).toBe("#f3f5f4");
  });
});
