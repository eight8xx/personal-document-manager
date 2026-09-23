import { ChevronLeft, ChevronRight, Table2 } from "lucide-react";
import { useContext, useEffect, useState } from "react";

import { toBackendError } from "../backend/error";
import { LibraryContext } from "../backend/libraryContext";
import type {
  BackendClient,
  DocumentPreview,
  DocumentSummary
} from "../backend/types";

type TablePreviewPayload = Extract<DocumentPreview, { kind: "table" }>;

/** 每次请求的行数上限；界面不会一次挂载整份表格。 */
export const TABLE_PAGE_ROWS = 50;

interface TablePreviewProps {
  client: BackendClient;
  document: DocumentSummary;
  /** 打开文档时后端已返回的第一段范围，避免重复请求。 */
  preview: TablePreviewPayload;
}

/**
 * 表格文档（CSV/XLSX）的只读分页预览。
 *
 * 只渲染后端返回的范围：切换工作表或翻页时重新请求，不缓存整份表格。
 * 任何一侧失败都显示可理解的原因，且不修改资料库副本。
 */
export function TablePreview({ client, document, preview }: TablePreviewProps) {
  const library = useContext(LibraryContext);
  const [range, setRange] = useState<TablePreviewPayload>(preview);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    setRange(preview);
    setError("");
  }, [preview]);

  const sheetIndex = range.sheetIndex;
  const columnCount = range.columnCount;
  const startRow = range.startRow;
  const cells = range.cells;
  const loadingMore = loading;

  async function loadRange(nextSheetIndex: number, nextStartRow: number) {
    if (!library) {
      return;
    }
    setLoading(true);
    setError("");
    try {
      const next = await client.getTablePreview(library, document.id, {
        sheetIndex: nextSheetIndex,
        startRow: nextStartRow,
        rowCount: TABLE_PAGE_ROWS,
        columnCount
      });
      setRange(next);
    } catch (caught) {
      setError(toBackendError(caught).message);
    } finally {
      setLoading(false);
    }
  }

  const sheet = range.sheets.find((candidate) => candidate.index === sheetIndex);
  const hasPrevious = startRow > 0;
  const hasNext = range.hasMoreRows;

  if (error && cells.length === 0) {
    return (
      <div className="preview-error" role="alert">
        <Table2 size={22} aria-hidden="true" />
        <strong>无法加载表格预览</strong>
        <span>{error}</span>
        <button
          className="button quiet"
          type="button"
          onClick={() => void loadRange(sheetIndex, startRow)}
        >
          重试预览
        </button>
      </div>
    );
  }

  return (
    <div className="table-preview" data-preview-kind="table">
      <div className="preview-toolbar table-preview-toolbar">
        {range.sheets.length > 1 ? (
          <div className="table-sheet-tabs" role="tablist" aria-label="工作表">
            {range.sheets.map((candidate) => (
              <button
                key={candidate.index}
                className="button quiet compact"
                type="button"
                role="tab"
                aria-selected={candidate.index === sheetIndex}
                disabled={loadingMore}
                onClick={() => void loadRange(candidate.index, 0)}
              >
                {candidate.name}
              </button>
            ))}
          </div>
        ) : (
          <span className="table-sheet-name">{sheet?.name ?? "表格"}</span>
        )}
        <div className="table-range-controls">
          <span>
            第 {startRow + 1} - {startRow + cells.length} 行
            {sheet?.rowCount ? ` / ${sheet.rowCount}` : ""}
          </span>
          <button
            className="icon-button compact"
            type="button"
            onClick={() => void loadRange(sheetIndex, Math.max(0, startRow - TABLE_PAGE_ROWS))}
            disabled={!hasPrevious || loadingMore}
            aria-label="表格上一页"
            title="上一页"
          >
            <ChevronLeft size={15} aria-hidden="true" />
          </button>
          <button
            className="icon-button compact"
            type="button"
            onClick={() => void loadRange(sheetIndex, startRow + TABLE_PAGE_ROWS)}
            disabled={!hasNext || loadingMore}
            aria-label="表格下一页"
            title="下一页"
          >
            <ChevronRight size={15} aria-hidden="true" />
          </button>
        </div>
      </div>

      {error ? (
        <p className="table-preview-notice" role="alert">
          {error}
        </p>
      ) : null}
      {range.notice ? (
        <p className="table-preview-notice">{range.notice}</p>
      ) : null}
      {range.degradedFeatures.length > 0 ? (
        <p className="table-preview-notice">
          已降级：{range.degradedFeatures.join("、")}
        </p>
      ) : null}

      <div className="table-preview-scroll">
        <table>
          <caption>
            {document.title}
            {sheet ? ` · ${sheet.name}` : ""}
          </caption>
          <tbody>
            {cells.map((row, rowOffset) => (
              <tr key={startRow + rowOffset}>
                <th scope="row">{startRow + rowOffset + 1}</th>
                {row.map((cell, columnIndex) => (
                  <td key={columnIndex}>{cell}</td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
