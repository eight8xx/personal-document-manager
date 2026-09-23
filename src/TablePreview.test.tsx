import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { BackendError } from "./backend/error";
import { FakeBackendClient } from "./backend/fakeClient";
import { LibraryContext } from "./backend/libraryContext";
import type {
  BackendClient,
  DocumentPreview,
  DocumentSummary,
  LibrarySummary,
  TablePreviewRequest,
  TableSheet
} from "./backend/types";
import { DocumentDetails } from "./components/DocumentDetails";
import { TABLE_PAGE_ROWS, TablePreview } from "./components/TablePreview";

type TablePreviewPayload = Extract<DocumentPreview, { kind: "table" }>;

const library: LibrarySummary = {
  id: "library-tables",
  name: "表格资料库",
  path: "C:\\Documents\\表格资料库",
  createdAt: "2026-09-13T08:00:00Z"
};

function documentSummary({
  id,
  title,
  fileName,
  fileType
}: {
  id: string;
  title: string;
  fileName: string;
  fileType: string;
}): DocumentSummary {
  return {
    id,
    title,
    description: null,
    documentDate: null,
    fileName,
    fileType,
    fileSize: 2048,
    contentHash: `hash-${id}`,
    collectionId: "inbox",
    tags: [],
    processingStatus: "ready",
    indexStatus: "searchable",
    errorStage: null,
    errorMessage: null,
    importedAt: "2026-09-13T08:10:00Z",
    sourcePath: `C:\\Sources\\${fileName}`,
    sourceIdentifier: `c:\\sources\\${fileName}`,
    lastImportedAt: "2026-09-13T08:10:00Z"
  };
}

const csvDocument = documentSummary({
  id: "csv-1",
  title: "销售记录",
  fileName: "销售记录.csv",
  fileType: "CSV"
});

const xlsxDocument = documentSummary({
  id: "xlsx-1",
  title: "季度报表",
  fileName: "季度报表.xlsx",
  fileType: "XLSX"
});

/** 记录每次表格范围请求，并在需要时让下一次请求失败。 */
function recordingClient(documents: DocumentSummary[]) {
  const client = new FakeBackendClient({ documents });
  const requests: TablePreviewRequest[] = [];
  let pendingFailure: Error | null = null;
  const callGetTablePreview = client.getTablePreview.bind(client);

  client.getTablePreview = async (owner, documentId, request = {}) => {
    requests.push({ ...request });
    if (pendingFailure) {
      const failure = pendingFailure;
      pendingFailure = null;
      throw failure;
    }
    return callGetTablePreview(owner, documentId, request);
  };

  return {
    client,
    requests,
    failNextRequest(failure: Error) {
      pendingFailure = failure;
    }
  };
}

interface StaticTableOptions {
  /** 在返回固定范围前等待，用于观察请求进行中的界面状态。 */
  gate?: Promise<void>;
  gateStartRow?: number;
}

/** 固定载荷的假客户端：模拟后端返回稀疏、含引号或带提示的范围。 */
function staticTableClient(
  document: DocumentSummary,
  payload: TablePreviewPayload,
  options: StaticTableOptions = {}
) {
  const client = new FakeBackendClient({ documents: [document] });
  const requests: TablePreviewRequest[] = [];

  client.getTablePreview = async (_owner, _documentId, request = {}) => {
    requests.push({ ...request });
    if (
      options.gate &&
      options.gateStartRow !== undefined &&
      request.startRow === options.gateStartRow
    ) {
      await options.gate;
    }
    return structuredClone(payload);
  };

  return { client, requests };
}

function renderPreview(
  client: BackendClient,
  document: DocumentSummary,
  preview: TablePreviewPayload
) {
  return render(
    <LibraryContext.Provider value={library}>
      <TablePreview client={client} document={document} preview={preview} />
    </LibraryContext.Provider>
  );
}

/** 模拟 DocumentDetails 打开文档时已经拿到的第一段范围。 */
function initialPreview(client: BackendClient, document: DocumentSummary) {
  return client.getTablePreview(library, document.id, {});
}

describe("CSV/XLSX 表格预览", () => {
  it("renders the range returned by the backend without requesting it again", async () => {
    const recorder = recordingClient([xlsxDocument]);
    const preview = await initialPreview(recorder.client, xlsxDocument);
    expect(recorder.requests).toHaveLength(1);

    renderPreview(recorder.client, xlsxDocument, preview);

    expect(screen.getAllByRole("row")).toHaveLength(preview.cells.length);
    expect(screen.getByRole("cell", { name: "R1C1" })).toBeInTheDocument();
    expect(screen.getByRole("rowheader", { name: "1" })).toBeInTheDocument();
    expect(screen.getByRole("table", { name: "季度报表 · 汇总" })).toBeInTheDocument();
    expect(recorder.requests).toHaveLength(1);
  });

  it("shows the synthesized sheet name and row range for a single-sheet CSV", async () => {
    const recorder = recordingClient([csvDocument]);
    const preview = await initialPreview(recorder.client, csvDocument);

    renderPreview(recorder.client, csvDocument, preview);

    expect(screen.queryByRole("tablist")).not.toBeInTheDocument();
    expect(screen.getByRole("table", { name: "销售记录 · CSV" })).toBeInTheDocument();
    expect(
      screen.getByText(`第 1 - ${preview.cells.length} 行`)
    ).toBeInTheDocument();
  });

  it("uses the sparse row numbers from the backend instead of sequential numbering", () => {
    const payload: TablePreviewPayload = {
      kind: "table",
      sheets: [{ index: 0, name: "明细", rowCount: 8, columnCount: 2 }],
      sheetIndex: 0,
      startRow: 0,
      cells: [["备注"], ["", "42"]],
      rowNumbers: [0, 6],
      columnCount: 2,
      hasMoreRows: false,
      degradedFeatures: [],
      notice: null
    };
    const { client } = staticTableClient(xlsxDocument, payload);

    renderPreview(client, xlsxDocument, payload);

    // 稀疏表把真实 Excel 行号（第 1 行、第 7 行）呈现给用户。
    expect(screen.getByRole("rowheader", { name: "1" })).toBeInTheDocument();
    expect(screen.getByRole("rowheader", { name: "7" })).toBeInTheDocument();
    expect(screen.queryByRole("rowheader", { name: "2" })).not.toBeInTheDocument();
    expect(screen.getByText("第 1 - 7 行 / 8")).toBeInTheDocument();
  });

  it("falls back to sequential numbering when the backend omits row numbers", () => {
    const payload: TablePreviewPayload = {
      kind: "table",
      sheets: [{ index: 0, name: "CSV", rowCount: 3, columnCount: 1 }],
      sheetIndex: 0,
      startRow: 1,
      cells: [["第二行"], ["第三行"]],
      columnCount: 1,
      hasMoreRows: false,
      degradedFeatures: [],
      notice: null
    };
    const { client } = staticTableClient(csvDocument, payload);

    renderPreview(client, csvDocument, payload);

    expect(screen.getByRole("rowheader", { name: "2" })).toBeInTheDocument();
    expect(screen.getByRole("rowheader", { name: "3" })).toBeInTheDocument();
    expect(screen.getByText("第 2 - 3 行 / 3")).toBeInTheDocument();
  });

  it("disables both paging buttons on a range that is the first and last page", async () => {
    const recorder = recordingClient([xlsxDocument]);
    const preview = await initialPreview(recorder.client, xlsxDocument);
    expect(preview.hasMoreRows).toBe(false);

    renderPreview(recorder.client, xlsxDocument, preview);

    expect(screen.getByRole("button", { name: "表格上一页" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "表格下一页" })).toBeDisabled();
    expect(screen.getByText("第 1 - 12 行 / 12")).toBeInTheDocument();
    expect(recorder.requests).toHaveLength(1);
  });

  it("pages with TABLE_PAGE_ROWS and never requests a negative start row", async () => {
    const user = userEvent.setup();
    const recorder = recordingClient([xlsxDocument]);
    const preview = await initialPreview(recorder.client, xlsxDocument);
    renderPreview(recorder.client, xlsxDocument, preview);
    // 第一条请求来自打开文档时的首次预览，之后的请求才由表格交互发起。
    const initialRequestCount = recorder.requests.length;

    await user.click(screen.getByRole("tab", { name: "明细" }));
    await screen.findByText("第 1 - 50 行 / 200");

    const next = screen.getByRole("button", { name: "表格下一页" });
    expect(next).toBeEnabled();
    await user.click(next);
    await screen.findByText("第 51 - 100 行 / 200");
    expect(recorder.requests.at(-1)).toEqual({
      sheetIndex: 1,
      startRow: TABLE_PAGE_ROWS,
      rowCount: TABLE_PAGE_ROWS,
      columnCount: expect.any(Number)
    });

    const previous = screen.getByRole("button", { name: "表格上一页" });
    expect(previous).toBeEnabled();
    await user.click(previous);
    await screen.findByText("第 1 - 50 行 / 200");
    expect(recorder.requests.at(-1)).toMatchObject({ startRow: 0 });
    expect(screen.getByRole("button", { name: "表格上一页" })).toBeDisabled();

    for (const request of recorder.requests.slice(initialRequestCount)) {
      expect(request.startRow ?? 0).toBeGreaterThanOrEqual(0);
      expect(request.rowCount).toBe(TABLE_PAGE_ROWS);
    }
    expect(screen.getAllByRole("row")).toHaveLength(TABLE_PAGE_ROWS);
  });

  it("resets to the first row when switching sheets and marks the active tab", async () => {
    const user = userEvent.setup();
    const recorder = recordingClient([xlsxDocument]);
    const preview = await initialPreview(recorder.client, xlsxDocument);
    renderPreview(recorder.client, xlsxDocument, preview);

    await user.click(screen.getByRole("tab", { name: "明细" }));
    await screen.findByText("第 1 - 50 行 / 200");
    expect(recorder.requests.at(-1)).toMatchObject({
      sheetIndex: 1,
      startRow: 0
    });

    await user.click(screen.getByRole("button", { name: "表格下一页" }));
    await screen.findByText("第 51 - 100 行 / 200");

    await user.click(screen.getByRole("tab", { name: "汇总" }));
    await screen.findByText("第 1 - 12 行 / 12");
    expect(recorder.requests.at(-1)).toMatchObject({
      sheetIndex: 0,
      startRow: 0
    });
    expect(screen.getByRole("tab", { name: "汇总" })).toHaveAttribute(
      "aria-selected",
      "true"
    );
    expect(screen.getByRole("tab", { name: "明细" })).toHaveAttribute(
      "aria-selected",
      "false"
    );
  });

  it("mounts only the requested range for a 200-row worksheet", async () => {
    const user = userEvent.setup();
    const recorder = recordingClient([xlsxDocument]);
    const preview = await initialPreview(recorder.client, xlsxDocument);
    renderPreview(recorder.client, xlsxDocument, preview);

    await user.click(screen.getByRole("tab", { name: "明细" }));
    await screen.findByText("第 1 - 50 行 / 200");

    const requested = recorder.requests.at(-1)!;
    expect(requested.rowCount).toBe(TABLE_PAGE_ROWS);
    expect(requested.rowCount!).toBeLessThanOrEqual(TABLE_PAGE_ROWS);

    const returnedRange = await recorder.client.getTablePreview(
      library,
      xlsxDocument.id,
      requested
    );
    expect(returnedRange.cells).toHaveLength(TABLE_PAGE_ROWS);
    expect(screen.getAllByRole("row")).toHaveLength(returnedRange.cells.length);
    expect(screen.getByRole("rowheader", { name: "50" })).toBeInTheDocument();
    expect(screen.queryByRole("rowheader", { name: "51" })).not.toBeInTheDocument();
  });

  it("renders a distinguishable placeholder for empty cells", async () => {
    const payload: TablePreviewPayload = {
      kind: "table",
      sheets: [{ index: 0, name: "CSV", rowCount: 2, columnCount: 3 }],
      sheetIndex: 0,
      startRow: 0,
      cells: [
        ["名称", "", "数量"],
        ["笔记本", "已归档", ""]
      ],
      columnCount: 3,
      hasMoreRows: false,
      degradedFeatures: [],
      notice: null
    };
    const { client, requests } = staticTableClient(csvDocument, payload);
    const preview = await client.getTablePreview(library, csvDocument.id, {});

    renderPreview(client, csvDocument, preview);

    expect(screen.getAllByText("空单元格")).toHaveLength(2);
    expect(screen.getAllByText("—")).toHaveLength(2);
    expect(screen.getByRole("cell", { name: "名称" })).toBeInTheDocument();
    expect(screen.getByRole("cell", { name: "已归档" })).toBeInTheDocument();
    expect(requests).toHaveLength(1);
  });

  it("renders quoted cells with commas, quotes and newlines verbatim as text", async () => {
    const payload: TablePreviewPayload = {
      kind: "table",
      sheets: [{ index: 0, name: "CSV", rowCount: 3, columnCount: 2 }],
      sheetIndex: 0,
      startRow: 0,
      cells: [
        ["备注", "明细"],
        ['第一行\n第二行', '含"引号"和,逗号'],
        ["<b>不是标签</b>", ""]
      ],
      columnCount: 2,
      hasMoreRows: false,
      degradedFeatures: [],
      notice: null
    };
    const { client } = staticTableClient(csvDocument, payload);
    const preview = await client.getTablePreview(library, csvDocument.id, {});

    const { container } = renderPreview(client, csvDocument, preview);

    // getByText 会折叠空白，因此按折叠后的文本查询，再断言换行原样保留。
    const multiline = screen.getByText("第一行 第二行");
    expect(multiline.textContent).toBe("第一行\n第二行");
    const quoted = screen.getByText('含"引号"和,逗号');
    expect(quoted.textContent).toBe('含"引号"和,逗号');
    expect(
      screen.getByRole("cell", { name: /含"引号"和,逗号/ })
    ).toBeInTheDocument();
    expect(screen.getByText("<b>不是标签</b>")).toBeInTheDocument();
    expect(container.querySelector("b")).toBeNull();
  });

  it("surfaces the backend notice and degraded features without extra requests", async () => {
    const recorder = recordingClient([csvDocument]);
    const preview = await initialPreview(recorder.client, csvDocument);
    renderPreview(recorder.client, csvDocument, {
      ...preview,
      notice: "CSV 已按 UTF-8 解码，开头的 BOM 已忽略。",
      degradedFeatures: ["含控制字符的单元格已按纯文本呈现"]
    });

    expect(
      screen.getByText("CSV 已按 UTF-8 解码，开头的 BOM 已忽略。")
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        "部分复杂内容无法完整呈现：含控制字符的单元格已按纯文本呈现。其余内容仍可预览。"
      )
    ).toBeInTheDocument();
    expect(recorder.requests).toHaveLength(1);
  });

  it("keeps the loaded range, explains the failure and retries the failed range read-only", async () => {
    const user = userEvent.setup();
    const recorder = recordingClient([xlsxDocument]);
    const preview = await initialPreview(recorder.client, xlsxDocument);
    recorder.failNextRequest(
      new BackendError({
        code: "preview",
        message: "读取工作表「明细」失败：工作簿结构已损坏。"
      })
    );
    renderPreview(recorder.client, xlsxDocument, preview);
    expect(screen.getAllByRole("row")).toHaveLength(12);

    await user.click(screen.getByRole("tab", { name: "明细" }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("读取工作表「明细」失败：工作簿结构已损坏。");
    // 失败不修改任何状态：已加载的范围和当前工作表保持不变。
    expect(screen.getAllByRole("row")).toHaveLength(12);
    expect(screen.getByRole("tab", { name: "汇总" })).toHaveAttribute(
      "aria-selected",
      "true"
    );

    await user.click(screen.getByRole("button", { name: "重试预览" }));

    await screen.findByText("第 1 - 50 行 / 200");
    expect(recorder.requests.at(-1)).toMatchObject({
      sheetIndex: 1,
      startRow: 0,
      rowCount: TABLE_PAGE_ROWS
    });
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    // 预览只读取工作表与范围，从不写入资料库副本。
    expect(recorder.client.calls).not.toHaveLength(0);
    expect(
      recorder.client.calls.filter(
        (call) =>
          !call.startsWith("getTablePreview:") &&
          !call.startsWith("listDocumentSheets:")
      )
    ).toEqual([]);
  });

  it("disables paging and sheet switching while a range request is in flight", async () => {
    const user = userEvent.setup();
    let releaseGate: () => void = () => {};
    const gate = new Promise<void>((resolve) => {
      releaseGate = resolve;
    });
    const recorder = recordingClient([xlsxDocument]);
    const preview = await initialPreview(recorder.client, xlsxDocument);
    const callGetTablePreview = recorder.client.getTablePreview.bind(
      recorder.client
    );
    recorder.client.getTablePreview = async (owner, documentId, request = {}) => {
      if (request.sheetIndex === 1 && request.startRow === TABLE_PAGE_ROWS) {
        await gate;
      }
      return callGetTablePreview(owner, documentId, request);
    };
    renderPreview(recorder.client, xlsxDocument, preview);

    await user.click(screen.getByRole("tab", { name: "明细" }));
    await screen.findByText("第 1 - 50 行 / 200");

    await user.click(screen.getByRole("button", { name: "表格下一页" }));

    expect(await screen.findByText("正在加载")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "表格下一页" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "表格上一页" })).toBeDisabled();
    expect(screen.getByRole("tab", { name: "汇总" })).toBeDisabled();
    expect(screen.getByText("第 1 - 50 行 / 200")).toBeInTheDocument();

    await act(async () => {
      releaseGate();
      await Promise.resolve();
    });

    await waitFor(() => {
      expect(screen.queryByText("正在加载")).not.toBeInTheDocument();
    });
    await screen.findByText("第 51 - 100 行 / 200");
    expect(screen.getByRole("button", { name: "表格下一页" })).toBeEnabled();
  });

  it("renders inside the document details panel and pages from the opened preview", async () => {
    const user = userEvent.setup();
    const source = recordingClient([csvDocument]);
    const openedPreview = await initialPreview(source.client, csvDocument);
    const client = new FakeBackendClient({
      documents: [csvDocument],
      getDocumentPreview: async () => openedPreview
    });
    const tableRequests: TablePreviewRequest[] = [];
    const callGetTablePreview = client.getTablePreview.bind(client);
    client.getTablePreview = async (owner, documentId, request = {}) => {
      tableRequests.push({ ...request });
      return callGetTablePreview(owner, documentId, request);
    };

    render(
      <LibraryContext.Provider value={library}>
        <DocumentDetails
          client={client}
          document={csvDocument}
          collections={[]}
          retryingIndex={false}
          onEditDocument={() => {}}
          onMoveDocumentToTrash={() => {}}
          onRetryIndex={() => {}}
        />
      </LibraryContext.Provider>
    );

    expect(
      await screen.findByRole("table", { name: "销售记录 · CSV" })
    ).toBeInTheDocument();
    // 打开文档时已经拿到第一段范围，表格挂载不会重复请求。
    expect(tableRequests).toHaveLength(0);
    expect(screen.getAllByRole("row")).toHaveLength(openedPreview.cells.length);

    await user.click(screen.getByRole("button", { name: "表格下一页" }));

    await screen.findByText(`第 ${TABLE_PAGE_ROWS + 1} - 100 行`);
    expect(tableRequests.at(-1)).toMatchObject({
      sheetIndex: 0,
      startRow: TABLE_PAGE_ROWS,
      rowCount: TABLE_PAGE_ROWS
    });
    // 详情面板同样只读取预览与范围，不写入资料库副本。
    expect(
      client.calls.filter(
        (call) =>
          !call.startsWith("getDocumentPreview:") &&
          !call.startsWith("getTablePreview:") &&
          !call.startsWith("listDocumentSheets:")
      )
    ).toEqual([]);
  });

  it("opens the library copy externally from a table preview without any write", async () => {
    const user = userEvent.setup();
    const source = recordingClient([xlsxDocument]);
    const openedPreview = await initialPreview(source.client, xlsxDocument);
    const client = new FakeBackendClient({
      documents: [xlsxDocument],
      getDocumentPreview: async () => openedPreview
    });
    let openCalls = 0;
    client.openDocument = async (owner, documentId) => {
      openCalls += 1;
      await FakeBackendClient.prototype.openDocument.call(
        client,
        owner,
        documentId
      );
    };

    render(
      <LibraryContext.Provider value={library}>
        <DocumentDetails
          client={client}
          document={xlsxDocument}
          collections={[]}
          retryingIndex={false}
          onEditDocument={() => {}}
          onMoveDocumentToTrash={() => {}}
          onRetryIndex={() => {}}
        />
      </LibraryContext.Provider>
    );

    // 先切到第二张工作表：外部打开针对的仍是这份文档的资料库副本。
    await user.click(await screen.findByRole("tab", { name: "明细" }));
    await screen.findByRole("table", { name: "季度报表 · 明细" });

    await user.click(
      screen.getByRole("button", { name: "用系统默认程序打开 季度报表" })
    );

    await waitFor(() => {
      expect(openCalls).toBe(1);
    });
    // 只传文档 ID：后端把它解析成资料库目录下的副本路径（源文件仅被读取）。
    expect(client.calls).toContain("openDocument:xlsx-1");
    expect(
      client.calls.some((call) => call.includes(xlsxDocument.sourcePath))
    ).toBe(false);
    expect(
      client.calls.filter(
        (call) =>
          !call.startsWith("getDocumentPreview:") &&
          !call.startsWith("getTablePreview:") &&
          !call.startsWith("listDocumentSheets:") &&
          !call.startsWith("openDocument:")
      )
    ).toEqual([]);
  });

  it("jumps to the next data row of a sparse sheet with a single page click", async () => {
    const user = userEvent.setup();
    const requests: TablePreviewRequest[] = [];
    const sheet: TableSheet = {
      index: 0,
      name: "稀疏",
      rowCount: 1005,
      columnCount: 3
    };
    const client = new FakeBackendClient({ documents: [xlsxDocument] });
    client.getTablePreview = async (_owner, _documentId, request = {}) => {
      requests.push({ ...request });
      const startRow = request.startRow ?? 0;
      if (startRow < 999) {
        // 数据在 Excel 第 1000 行：首屏没有可显示的行，只给出下一个含数据的行。
        return {
          kind: "table",
          sheets: [sheet],
          sheetIndex: 0,
          startRow,
          cells: [],
          rowNumbers: [],
          columnCount: 3,
          hasMoreRows: true,
          nextDataRow: 999,
          degradedFeatures: [],
          notice: null
        };
      }
      return {
        kind: "table",
        sheets: [sheet],
        sheetIndex: 0,
        startRow,
        cells: [
          ["甲", "乙", "丙"],
          ["丁", "戊", "己"]
        ],
        rowNumbers: [999, 1000],
        columnCount: 3,
        hasMoreRows: false,
        degradedFeatures: [],
        notice: null
      };
    };
    const preview = await initialPreview(client, xlsxDocument);
    renderPreview(client, xlsxDocument, preview);

    // 首屏为空，但「下一页」可用。
    expect(
      screen.getByText("该范围没有可显示的行。")
    ).toBeInTheDocument();
    const next = screen.getByRole("button", { name: "表格下一页" });
    expect(next).toBeEnabled();
    expect(screen.getByRole("button", { name: "表格上一页" })).toBeDisabled();

    await user.click(next);

    // 一次点击直接请求下一个含数据的行，而不是逐页 +50。
    await waitFor(() => {
      expect(requests).toHaveLength(2);
    });
    expect(requests[1]).toEqual({
      sheetIndex: 0,
      startRow: 999,
      rowCount: TABLE_PAGE_ROWS,
      columnCount: 3
    });
    expect(await screen.findByText("甲")).toBeInTheDocument();
    // 行号沿用后端给出的真实稀疏行号（Excel 第 1000、1001 行）。
    expect(screen.getByRole("rowheader", { name: "1000" })).toBeInTheDocument();
    expect(screen.getByRole("rowheader", { name: "1001" })).toBeInTheDocument();
    // 已到数据末尾：没有更多数据，按钮禁用。
    expect(screen.getByRole("button", { name: "表格下一页" })).toBeDisabled();
  });

  it("keeps dense paging sequential without skipping or repeating rows", async () => {
    const user = userEvent.setup();
    const recorder = recordingClient([xlsxDocument]);
    const preview = await initialPreview(recorder.client, xlsxDocument);
    renderPreview(recorder.client, xlsxDocument, preview);

    await user.click(screen.getByRole("tab", { name: "明细" }));
    await screen.findByText("第 1 - 50 行 / 200");

    // 稠密表的 nextDataRow 就是下一页起点，行为与逐页 +50 一致。
    await user.click(screen.getByRole("button", { name: "表格下一页" }));
    await screen.findByText("第 51 - 100 行 / 200");
    await user.click(screen.getByRole("button", { name: "表格下一页" }));
    await screen.findByText("第 101 - 150 行 / 200");

    const pageRequests = recorder.requests.slice(1).map((request) => request.startRow);
    expect(pageRequests).toEqual([0, 50, 100]);
    const rendered = screen
      .getAllByRole("rowheader")
      .map((cell) => cell.textContent);
    expect(rendered).toHaveLength(TABLE_PAGE_ROWS);
    expect(new Set(rendered).size).toBe(TABLE_PAGE_ROWS);
    expect(rendered[0]).toBe("101");
    expect(rendered.at(-1)).toBe("150");
  });

  it("disables the next button when the range has no more data", async () => {
    const payload: TablePreviewPayload = {
      kind: "table",
      sheets: [{ index: 0, name: "稀疏", rowCount: 1000, columnCount: 2 }],
      sheetIndex: 0,
      startRow: 999,
      cells: [["甲", "乙"]],
      rowNumbers: [999],
      columnCount: 2,
      hasMoreRows: false,
      degradedFeatures: [],
      notice: null
    };
    const { client, requests } = staticTableClient(xlsxDocument, payload);
    const preview = await client.getTablePreview(library, xlsxDocument.id, {});

    renderPreview(client, xlsxDocument, preview);

    expect(screen.getByRole("rowheader", { name: "1000" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "表格下一页" })).toBeDisabled();
    expect(requests).toHaveLength(1);
  });
});
