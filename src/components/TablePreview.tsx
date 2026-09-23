import {
  AlertCircle,
  ChevronLeft,
  ChevronRight,
  LoaderCircle,
  RotateCcw
} from "lucide-react";
import { useContext, useEffect, useRef, useState } from "react";

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

function sheetTabId(sheetIndex: number) {
  return `table-sheet-tab-${sheetIndex}`;
}

/**
 * 表格文档（CSV/XLSX）的只读分页预览。
 *
 * 只渲染后端返回的范围：切换工作表或翻页时按 TABLE_PAGE_ROWS 重新请求，
 * 不缓存整份表格，也不补齐后端省略的单元格。请求失败时保留上一次成功的
 * 范围并显示具体原因，可原地重试；预览从不修改源文件或资料库副本。
 */
export function TablePreview({ client, document, preview }: TablePreviewProps) {
  const library = useContext(LibraryContext);
  const [range, setRange] = useState<TablePreviewPayload>(preview);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  /** 最近一次请求的目标范围：失败重试要回到失败的那一页，而不是当前显示页。 */
  const requestedRange = useRef({
    sheetIndex: preview.sheetIndex,
    startRow: preview.startRow
  });
  /** 请求序号：只有最后一次请求的结果可以落地，旧响应不会覆盖新范围。 */
  const requestToken = useRef(0);

  useEffect(() => {
    requestToken.current += 1;
    requestedRange.current = {
      sheetIndex: preview.sheetIndex,
      startRow: preview.startRow
    };
    setRange(preview);
    setError("");
    setLoading(false);
  }, [preview]);

  const sheetIndex = range.sheetIndex;
  const columnCount = range.columnCount;
  const startRow = range.startRow;
  const cells = range.cells;
  const sheet = range.sheets.find((candidate) => candidate.index === sheetIndex);
  const hasSheetTabs = range.sheets.length > 1;
  const hasPrevious = startRow > 0;
  const hasNext = range.hasMoreRows;

  async function loadRange(nextSheetIndex: number, nextStartRow: number) {
    const target = {
      sheetIndex: nextSheetIndex,
      // 首页继续向前仍是第 0 行：请求的行号不会变成负数。
      startRow: Math.max(0, nextStartRow)
    };
    requestedRange.current = target;

    if (!library) {
      setError("资料库尚未就绪，暂时无法加载表格预览。");
      return;
    }

    const token = requestToken.current + 1;
    requestToken.current = token;
    setLoading(true);
    setError("");
    try {
      const next = await client.getTablePreview(library, document.id, {
        sheetIndex: target.sheetIndex,
        startRow: target.startRow,
        rowCount: TABLE_PAGE_ROWS,
        columnCount
      });
      if (requestToken.current !== token) {
        return;
      }
      setRange(next);
    } catch (caught) {
      if (requestToken.current !== token) {
        return;
      }
      setError(toBackendError(caught).message);
    } finally {
      if (requestToken.current === token) {
        setLoading(false);
      }
    }
  }

  const lastRowNumber = startRow + cells.length;
  const rangeLabel =
    cells.length === 0
      ? startRow > 0
        ? `第 ${startRow + 1} 行之后无数据`
        : "无数据行"
      : `第 ${startRow + 1} - ${lastRowNumber} 行${
          sheet?.rowCount ? ` / ${sheet.rowCount}` : ""
        }`;

  return (
    <div className="table-preview" data-preview-kind="table">
      <div className="preview-toolbar table-preview-toolbar">
        {hasSheetTabs ? (
          <div className="table-sheet-tabs" role="tablist" aria-label="工作表">
            {range.sheets.map((candidate) => (
              <button
                key={candidate.index}
                id={sheetTabId(candidate.index)}
                className="button quiet compact"
                type="button"
                role="tab"
                aria-selected={candidate.index === sheetIndex}
                disabled={loading}
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
          <span className="table-range-label">{rangeLabel}</span>
          {loading ? (
            <span className="table-range-loading" role="status">
              <LoaderCircle className="spin" size={13} aria-hidden="true" />
              正在加载
            </span>
          ) : null}
          <button
            className="icon-button compact"
            type="button"
            onClick={() =>
              void loadRange(sheetIndex, startRow - TABLE_PAGE_ROWS)
            }
            disabled={!hasPrevious || loading}
            aria-label="表格上一页"
            title="上一页"
          >
            <ChevronLeft size={15} aria-hidden="true" />
          </button>
          <button
            className="icon-button compact"
            type="button"
            onClick={() =>
              void loadRange(sheetIndex, startRow + TABLE_PAGE_ROWS)
            }
            disabled={!hasNext || loading}
            aria-label="表格下一页"
            title="下一页"
          >
            <ChevronRight size={15} aria-hidden="true" />
          </button>
        </div>
      </div>

      {error ? (
        <div className="table-preview-error" role="alert">
          <AlertCircle size={14} aria-hidden="true" />
          <span>{error}</span>
          <button
            className="button quiet compact"
            type="button"
            onClick={() =>
              void loadRange(
                requestedRange.current.sheetIndex,
                requestedRange.current.startRow
              )
            }
            disabled={loading}
          >
            {loading ? (
              <LoaderCircle className="spin" size={14} aria-hidden="true" />
            ) : (
              <RotateCcw size={14} aria-hidden="true" />
            )}
            重试预览
          </button>
        </div>
      ) : null}
      {range.notice ? (
        <p className="table-preview-notice">{range.notice}</p>
      ) : null}
      {range.degradedFeatures.length > 0 ? (
        <p
          className="table-preview-notice table-degradation-notice"
          role="status"
        >
          部分复杂内容无法完整呈现：{range.degradedFeatures.join("、")}。其余内容仍可预览。
        </p>
      ) : null}

      <div
        className="table-preview-scroll"
        role={hasSheetTabs ? "tabpanel" : undefined}
        aria-labelledby={hasSheetTabs ? sheetTabId(sheetIndex) : undefined}
        tabIndex={0}
      >
        {cells.length === 0 ? (
          <p className="table-preview-empty" role="status">
            该范围没有可显示的行。
          </p>
        ) : (
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
                    <td key={columnIndex}>
                      {cell === "" ? (
                        <span className="table-cell-empty">
                          <span className="visually-hidden">空单元格</span>
                          <span aria-hidden="true">—</span>
                        </span>
                      ) : (
                        cell
                      )}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
