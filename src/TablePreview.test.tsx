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
  TablePreviewRequest
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
});
