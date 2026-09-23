import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { BackendError } from "./backend/error";
import { FakeBackendClient } from "./backend/fakeClient";
import { LibraryContext } from "./backend/libraryContext";
import type {
  BackendClient,
  LibrarySummary,
  ReceiveDirectoryListingItem,
  ReceiveDirectoryOperation,
  ReceiveImportLogEntry,
  ReceiveSource,
  ReceiveSourceCandidate,
  ReceiveSourceScanResult,
  ReceiveSourceInput
} from "./backend/types";
import { ReceiveDirectoryPanel } from "./components/ReceiveDirectory";

const library: LibrarySummary = {
  id: "library-receive",
  name: "接收资料库",
  path: "C:\\Documents\\接收资料库",
  createdAt: "2026-09-23T08:00:00Z"
};

const libraryKey = `${library.id}:${library.path}`;

const qqPath = "C:\\Users\\User\\Documents\\Tencent Files\\12345\\FileRecv";
const wechatPath = "C:\\Users\\User\\Documents\\WeChat Files\\wxid\\FileStorage\\File";

function source(overrides: Partial<ReceiveSource> & { id: string; kind: ReceiveSource["kind"] }): ReceiveSource {
  return {
    displayName: overrides.kind === "qq" ? "QQ" : "微信",
    path: null,
    enabled: false,
    status: "unconfigured",
    statusMessage: null,
    pendingCount: 0,
    lastScannedAt: null,
    ...overrides
  };
}

const qqSource = source({
  id: "source-qq",
  kind: "qq",
  path: qqPath,
  enabled: true,
  status: "ready",
  pendingCount: 3,
  lastScannedAt: "2026-09-23T10:00:00Z"
});

const wechatSource = source({
  id: "source-wechat",
  kind: "wechat",
  path: wechatPath,
  enabled: true,
  status: "ready",
  lastScannedAt: "2026-09-23T10:00:00Z"
});

const missingWechat = source({
  id: "source-wechat",
  kind: "wechat",
  path: wechatPath,
  enabled: true,
  status: "missing",
  statusMessage: "目录不存在：已被移动或删除。"
});

const listings: ReceiveDirectoryListingItem[] = [
  {
    path: "C:\\QQ\\FileRecv\\发票.pdf",
    fileName: "发票.pdf",
    fileType: "PDF",
    fileSize: 1024,
    previouslySkipped: false
  },
  {
    path: "C:\\QQ\\FileRecv\\截图.png",
    fileName: "截图.png",
    fileType: "PNG",
    fileSize: 2048,
    previouslySkipped: false
  },
  {
    path: "C:\\QQ\\FileRecv\\合同.docx",
    fileName: "合同.docx",
    fileType: "DOCX",
    fileSize: 4096,
    previouslySkipped: true
  }
];

const qqCandidates: ReceiveSourceCandidate[] = [
  {
    path: qqPath,
    evidence: "检测到 QQ 文件接收目录（FileRecv）"
  }
];

const wechatCandidates: ReceiveSourceCandidate[] = [
  {
    path: "C:\\Users\\User\\Documents\\WeChat Files\\wxid\\FileStorage\\File",
    evidence: "微信默认文件存储目录"
  }
];

interface ClientOptions {
  sources?: ReceiveSource[];
  candidates?: Partial<Record<"qq" | "wechat", ReceiveSourceCandidate[]>>;
  listing?: ReceiveDirectoryListingItem[];
  log?: ReceiveImportLogEntry[];
}

function createClient({
  sources = [],
  candidates = { qq: qqCandidates, wechat: wechatCandidates },
  listing = listings,
  log = []
}: ClientOptions = {}) {
  return new FakeBackendClient({
    receiveSources: { [libraryKey]: structuredClone(sources) },
    receiveDirectoryFiles: { [libraryKey]: structuredClone(listing) },
    receiveSourceCandidates: structuredClone(candidates),
    receiveImportLog: structuredClone(log)
  });
}

function trackWrites(client: BackendClient) {
  const upserts: { sourceId: string | null; input: ReceiveSourceInput }[] = [];
  const applied: ReceiveDirectoryOperation[] = [];
  const skipped: ReceiveDirectoryOperation[] = [];
  const upsert = client.upsertReceiveSource.bind(client);
  const apply = client.applyReceiveDirectorySelection.bind(client);
  const skip = client.skipReceiveDirectoryFiles.bind(client);
  client.upsertReceiveSource = async (owner, sourceId, input) => {
    upserts.push({ sourceId, input });
    return upsert(owner, sourceId, input);
  };
  client.applyReceiveDirectorySelection = async (owner, operation) => {
    applied.push(operation);
    return apply(owner, operation);
  };
  client.skipReceiveDirectoryFiles = async (owner, operation) => {
    skipped.push(operation);
    return skip(owner, operation);
  };
  return { upserts, applied, skipped };
}

function renderPanel(client: BackendClient) {
  return render(
    <LibraryContext.Provider value={library}>
      <ReceiveDirectoryPanel client={client} />
    </LibraryContext.Provider>
  );
}

describe("接收目录配置", () => {
  it("shows qq and wechat with path, status, pending count and failure reason", async () => {
    const client = createClient({ sources: [qqSource, missingWechat] });
    renderPanel(client);

    expect(await screen.findByRole("heading", { name: "QQ" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "微信" })).toBeInTheDocument();
    expect(screen.getByText("目录可用")).toBeInTheDocument();
    expect(screen.getByText("目录不存在")).toBeInTheDocument();
    expect(screen.getByText(qqPath)).toBeInTheDocument();
    expect(screen.getByText(wechatPath)).toBeInTheDocument();
    expect(screen.getByText(/待处理 3 个文件/)).toBeInTheDocument();

    const warning = screen.getByRole("alert");
    expect(warning).toHaveTextContent("目录失效：目录不存在：已被移动或删除。");
    expect(warning).toHaveTextContent("请重新确认目录，确认前不会改用其他目录。");
    expect(
      screen.getByRole("button", { name: "重新确认 微信 接收目录" })
    ).toBeInTheDocument();
  });

  it("asks the user to confirm a candidate directory before enabling an unconfigured source", async () => {
    const user = userEvent.setup();
    const client = createClient({ sources: [] });
    const writes = trackWrites(client);
    renderPanel(client);

    await screen.findByRole("heading", { name: "QQ" });
    expect(screen.getAllByText("尚未配置接收目录")).toHaveLength(2);

    await user.click(screen.getByRole("checkbox", { name: "启用 QQ 接收目录" }));

    const candidates = await screen.findByRole("group", {
      name: "QQ 候选目录"
    });
    expect(within(candidates).getByText(qqPath)).toBeInTheDocument();
    expect(
      within(candidates).getByText("识别依据：检测到 QQ 文件接收目录（FileRecv）")
    ).toBeInTheDocument();
    // 未确认前不写入、也不静默使用任何路径。
    expect(writes.upserts).toEqual([]);
    expect(client.calls).toContain("listReceiveSourceCandidates:qq");

    await user.click(
      within(candidates).getByRole("button", { name: /使用候选目录/ })
    );

    await waitFor(() => {
      expect(writes.upserts).toHaveLength(1);
    });
    expect(writes.upserts[0].sourceId).toBeNull();
    expect(writes.upserts[0].input).toMatchObject({
      kind: "qq",
      path: qqPath,
      enabled: true
    });
    // 首次确认目录后立即列出目录内已有的受支持文件。
    expect(
      await screen.findByRole("group", { name: "QQ 目录文件清单" })
    ).toBeInTheDocument();
  });

  it("imports only the selected files and records the rest as skipped", async () => {
    const user = userEvent.setup();
    const client = createClient({ sources: [] });
    const writes = trackWrites(client);
    renderPanel(client);
    await screen.findByRole("heading", { name: "QQ" });

    await user.click(screen.getByRole("checkbox", { name: "启用 QQ 接收目录" }));
    await user.click(
      within(
        await screen.findByRole("group", { name: "QQ 候选目录" })
      ).getByRole("button", { name: /使用候选目录/ })
    );

    const listing = await screen.findByRole("group", {
      name: "QQ 目录文件清单"
    });
    expect(
      within(listing).getByText(/目录 .* 下有 3 个受支持文件/)
    ).toBeInTheDocument();
    expect(within(listing).getByText("此前已跳过")).toBeInTheDocument();
    expect(
      within(listing).getByRole("checkbox", { name: "选择 发票.pdf" })
    ).not.toBeChecked();

    await user.click(
      within(listing).getByRole("checkbox", { name: "选择 发票.pdf" })
    );
    await user.click(
      within(listing).getByRole("button", { name: "导入所选（1）" })
    );

    await waitFor(() => {
      expect(writes.applied).toHaveLength(1);
    });
    expect(writes.applied[0].paths).toEqual(["C:\\QQ\\FileRecv\\发票.pdf"]);
    expect(writes.skipped).toHaveLength(1);
    expect(writes.skipped[0].paths.sort()).toEqual(
      ["C:\\QQ\\FileRecv\\合同.docx", "C:\\QQ\\FileRecv\\截图.png"].sort()
    );
    // 未勾选的文件只出现在“已跳过”里，绝不会被导入。
    for (const path of writes.skipped[0].paths) {
      expect(writes.applied[0].paths).not.toContain(path);
    }
    await waitFor(() => {
      expect(
        screen.queryByRole("group", { name: "QQ 目录文件清单" })
      ).not.toBeInTheDocument();
    });
  });

  it("skips every file of the first listing when the user chooses so", async () => {
    const user = userEvent.setup();
    const client = createClient({ sources: [] });
    const writes = trackWrites(client);
    renderPanel(client);
    await screen.findByRole("heading", { name: "QQ" });

    await user.click(screen.getByRole("checkbox", { name: "启用 QQ 接收目录" }));
    await user.click(
      within(
        await screen.findByRole("group", { name: "QQ 候选目录" })
      ).getByRole("button", { name: /使用候选目录/ })
    );
    const listing = await screen.findByRole("group", {
      name: "QQ 目录文件清单"
    });

    await user.click(within(listing).getByRole("button", { name: "全部跳过" }));

    await waitFor(() => {
      expect(writes.skipped).toHaveLength(1);
    });
    expect(writes.skipped[0].paths).toHaveLength(3);
    expect(writes.applied).toEqual([]);
    await waitFor(() => {
      expect(
        screen.queryByRole("group", { name: "QQ 目录文件清单" })
      ).not.toBeInTheDocument();
    });
  });

  it("can be reopened to pick files that were skipped before", async () => {
    const user = userEvent.setup();
    const client = createClient({ sources: [qqSource] });
    const writes = trackWrites(client);
    renderPanel(client);
    await screen.findByRole("heading", { name: "QQ" });

    await user.click(screen.getByRole("button", { name: "打开 QQ 文件清单" }));

    const listing = await screen.findByRole("group", {
      name: "QQ 目录文件清单"
    });
    const skippedBefore = within(listing).getByRole("checkbox", {
      name: "选择 合同.docx"
    });
    expect(skippedBefore).toBeEnabled();
    expect(skippedBefore).not.toBeChecked();

    await user.click(skippedBefore);
    await user.click(
      within(listing).getByRole("button", { name: "导入所选（1）" })
    );

    await waitFor(() => {
      expect(writes.applied).toHaveLength(1);
    });
    expect(writes.applied[0].paths).toEqual(["C:\\QQ\\FileRecv\\合同.docx"]);
    expect(writes.skipped[0].paths).toHaveLength(2);
  });

  it("keeps a missing directory until the user confirms a new one", async () => {
    const user = userEvent.setup();
    const client = createClient({ sources: [missingWechat] });
    const writes = trackWrites(client);
    renderPanel(client);

    await screen.findByRole("heading", { name: "微信" });
    const originalPath = screen.getByText(wechatPath);

    await user.click(
      screen.getByRole("button", { name: "重新确认 微信 接收目录" })
    );

    const candidates = await screen.findByRole("group", {
      name: "微信 候选目录"
    });
    expect(
      within(candidates).getByText(
        "请选择新的接收目录；确认前仍保留原目录，不会自动切换。"
      )
    ).toBeInTheDocument();
    expect(originalPath).toBeInTheDocument();
    expect(writes.upserts).toEqual([]);

    await user.click(
      within(candidates).getByRole("button", { name: /使用候选目录/ })
    );

    await waitFor(() => {
      expect(writes.upserts).toHaveLength(1);
    });
    expect(writes.upserts[0].sourceId).toBe("source-wechat");
    expect(writes.upserts[0].input.path).toBe(wechatCandidates[0].path);
    expect(writes.upserts[0].input.enabled).toBe(true);
  });

  it("enables and disables qq and wechat independently", async () => {
    const user = userEvent.setup();
    const client = createClient({ sources: [qqSource, wechatSource] });
    const writes = trackWrites(client);
    renderPanel(client);

    const wechatToggle = await screen.findByRole("checkbox", {
      name: "启用 微信 接收目录"
    });
    expect(wechatToggle).toBeChecked();
    await user.click(wechatToggle);

    await waitFor(() => {
      expect(writes.upserts).toHaveLength(1);
    });
    expect(writes.upserts[0].sourceId).toBe("source-wechat");
    expect(writes.upserts[0].input.enabled).toBe(false);

    // QQ 不受影响。
    expect(
      screen.getByRole("checkbox", { name: "启用 QQ 接收目录" })
    ).toBeChecked();
    const sources = await client.listReceiveSources(library);
    expect(sources.find((candidate) => candidate.id === "source-qq")?.enabled).toBe(
      true
    );
    expect(
      sources.find((candidate) => candidate.id === "source-wechat")?.enabled
    ).toBe(false);
  });

  it("shows the scan counts of the source", async () => {
    const user = userEvent.setup();
    const client = createClient({ sources: [qqSource] });
    renderPanel(client);
    await screen.findByRole("heading", { name: "QQ" });

    await user.click(
      screen.getByRole("button", { name: "扫描 QQ 接收目录" })
    );

    const summary = await screen.findByText(/扫描完成：检查 0 个/);
    expect(summary).toHaveTextContent("待处理 3 个");
    expect(client.calls).toContain("scanReceiveSources");
  });

  it("shows a retryable error when the sources cannot be loaded", async () => {
    const user = userEvent.setup();
    const client = createClient({ sources: [qqSource] });
    const load = client.listReceiveSources.bind(client);
    let failing = true;
    client.listReceiveSources = async (owner) => {
      if (failing) {
        failing = false;
        throw new BackendError({
          code: "store",
          message: "无法读取接收来源：资料库存储不可用。"
        });
      }
      return load(owner);
    };

    renderPanel(client);

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("无法读取接收来源：资料库存储不可用。");
    await user.click(within(alert).getByRole("button", { name: "重试" }));

    expect(await screen.findByText(qqPath)).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  const logEntry = (
    fileName: string,
    overrides: Partial<ReceiveImportLogEntry> = {}
  ): ReceiveImportLogEntry => ({
    sourceId: "source-qq",
    sourcePath: `C:\\QQ\\FileRecv\\${fileName}`,
    fileName,
    status: "imported",
    documentId: `document-${fileName}`,
    collectionId: null,
    tagIds: [],
    matchedRuleIds: [],
    errorMessage: null,
    createdAt: "2026-09-23T10:00:00Z",
    ...overrides
  });

  const completedResult = (
    overrides: Partial<ReceiveSourceScanResult> = {}
  ): ReceiveSourceScanResult => ({
    sourceId: "source-qq",
    scannedCount: 2,
    importedCount: 2,
    skippedCount: 0,
    pendingCount: 5,
    failedCount: 0,
    ...overrides
  });

  it("refreshes sources and the import log when a receive import completes", async () => {
    const client = createClient({
      sources: [qqSource],
      log: [logEntry("旧文件.pdf")]
    });
    let backendUpdated = false;
    const listSources = client.listReceiveSources.bind(client);
    const listLog = client.listReceiveImportLog.bind(client);
    client.listReceiveSources = async (owner) => {
      const sources = await listSources(owner);
      return backendUpdated
        ? sources.map((source) =>
            source.id === "source-qq" ? { ...source, pendingCount: 7 } : source
          )
        : sources;
    };
    client.listReceiveImportLog = async (owner, limit) => {
      const entries = await listLog(owner, limit);
      return backendUpdated
        ? [logEntry("新文件.pdf"), ...entries]
        : entries;
    };

    renderPanel(client);
    const log = await screen.findByRole("list", { name: "接收导入日志" });
    expect(within(log).getByText("旧文件.pdf")).toBeInTheDocument();
    expect(screen.getByText(/待处理 3 个文件/)).toBeInTheDocument();

    // 后端补扫完成并推进了数据，随后发出事件。
    backendUpdated = true;
    act(() => {
      client.emitReceiveImportCompleted({
        library,
        results: [completedResult()]
      });
    });

    expect(await screen.findByText("新文件.pdf")).toBeInTheDocument();
    expect(screen.getByText(/待处理 7 个文件/)).toBeInTheDocument();
    expect(screen.getByText(/扫描完成：检查 2 个/)).toHaveTextContent(
      "待处理 5 个"
    );
  });

  it("ignores a receive import event that belongs to another library", async () => {
    const otherLibrary: LibrarySummary = {
      id: "library-other",
      name: "另一个资料库",
      path: "C:\\Documents\\另一个资料库",
      createdAt: "2026-09-23T08:00:00Z"
    };
    const client = createClient({
      sources: [qqSource],
      log: [logEntry("旧文件.pdf")]
    });
    let backendUpdated = false;
    const listSources = client.listReceiveSources.bind(client);
    const listLog = client.listReceiveImportLog.bind(client);
    client.listReceiveSources = async (owner) => {
      const sources = await listSources(owner);
      return backendUpdated
        ? sources.map((source) =>
            source.id === "source-qq" ? { ...source, pendingCount: 99 } : source
          )
        : sources;
    };
    client.listReceiveImportLog = async (owner, limit) => {
      const entries = await listLog(owner, limit);
      return backendUpdated
        ? [logEntry("别的库的文件.pdf"), ...entries]
        : entries;
    };

    renderPanel(client);
    const log = await screen.findByRole("list", { name: "接收导入日志" });
    expect(within(log).getByText("旧文件.pdf")).toBeInTheDocument();
    const callsBefore = client.calls.filter((call) =>
      call.startsWith("listReceiveImportLog")
    ).length;

    backendUpdated = true;
    act(() => {
      client.emitReceiveImportCompleted({
        library: otherLibrary,
        results: [completedResult({ scannedCount: 9, importedCount: 9 })]
      });
    });

    // 事件属于别的资料库：不刷新、不显示它的扫描结果，也不改动当前库的显示。
    await waitFor(() => {
      expect(
        client.calls.filter((call) => call.startsWith("listReceiveImportLog"))
          .length
      ).toBe(callsBefore);
    });
    expect(screen.queryByText("别的库的文件.pdf")).not.toBeInTheDocument();
    expect(screen.queryByText(/检查 9 个/)).not.toBeInTheDocument();
    expect(screen.getByText(/待处理 3 个文件/)).toBeInTheDocument();
    expect(within(log).getByText("旧文件.pdf")).toBeInTheDocument();
  });
});
