import {
  AlertCircle,
  CheckCircle2,
  CircleSlash,
  Copy,
  LoaderCircle,
  RotateCcw,
  X
} from "lucide-react";

import type {
  ImportBatch,
  ImportItemResult,
  ImportItemStatus,
  ImportProgress
} from "../backend/types";

interface ImportBatchPanelProps {
  batch: ImportBatch | null;
  progress: ImportProgress | null;
  items: ImportItemResult[];
  retryingItemIds: Set<string>;
  onRetry: (item: ImportItemResult) => void;
  onClose: () => void;
}

const statusLabels: Record<ImportItemStatus, string> = {
  imported: "已导入",
  duplicate: "等待重复决策",
  sourceChanged: "等待来源变化决策",
  failed: "导入失败",
  ignored: "已忽略",
  skipped: "已跳过"
};

export function ImportBatchPanel({
  batch,
  progress,
  items,
  retryingItemIds,
  onRetry,
  onClose
}: ImportBatchPanelProps) {
  const total = progress?.total ?? batch?.items.length ?? items.length;
  const completed = Math.max(
    progress?.completed ?? 0,
    items.length,
    0
  );
  const importedCount = count(items, "imported");
  const duplicateCount =
    count(items, "duplicate") + count(items, "sourceChanged");
  const failedCount = count(items, "failed");
  const ignoredCount = count(items, "ignored");
  const skippedCount = count(items, "skipped");
  const finished = Boolean(batch) || (progress?.finished ?? false);

  return (
    <section className="import-panel" aria-label="批量导入进度">
      <header className="import-panel-header">
        <div className="import-panel-title">
          {finished ? (
            <CheckCircle2 size={18} aria-hidden="true" />
          ) : (
            <LoaderCircle className="spin" size={18} aria-hidden="true" />
          )}
          <div>
            <strong>{finished ? "导入批次已完成" : "正在导入"}</strong>
            <span>
              已完成 {completed}/{total}
            </span>
          </div>
        </div>
        <div className="import-summary" aria-label="导入结果汇总">
          <span>已导入 {importedCount}</span>
          {duplicateCount > 0 ? <span>待决策 {duplicateCount}</span> : null}
          {failedCount > 0 ? <span>失败 {failedCount}</span> : null}
          {ignoredCount > 0 ? <span>忽略 {ignoredCount}</span> : null}
          {skippedCount > 0 ? <span>已跳过 {skippedCount}</span> : null}
        </div>
        {finished ? (
          <button
            className="icon-button"
            type="button"
            onClick={onClose}
            aria-label="关闭导入结果"
            title="关闭导入结果"
          >
            <X size={17} aria-hidden="true" />
          </button>
        ) : null}
      </header>

      {!finished ? (
        <div className="import-current">
          <progress max={Math.max(total, 1)} value={completed} />
          <span>
            {progress?.currentFileName
              ? `当前文件：${progress.currentFileName}`
              : "正在扫描文件夹"}
          </span>
        </div>
      ) : null}

      {items.length > 0 ? (
        <div className="import-item-list" role="list">
          {items.map((item) => {
            const retrying = retryingItemIds.has(item.itemId);
            return (
              <div
                className="import-item-row"
                key={item.itemId}
                role="listitem"
              >
                <span
                  className={`import-item-status ${item.status}`}
                  aria-hidden="true"
                >
                  {item.status === "failed" ? (
                    <AlertCircle size={15} />
                  ) : item.status === "duplicate" ? (
                    <Copy size={15} />
                  ) : item.status === "ignored" ||
                    item.status === "skipped" ? (
                    <CircleSlash size={15} />
                  ) : (
                    <CheckCircle2 size={15} />
                  )}
                </span>
                <div className="import-item-copy">
                  <strong>{item.fileName}</strong>
                  <span>{statusLabels[item.status]}</span>
                  {item.status === "failed" ? (
                    <small>
                      失败阶段：{item.errorStage ?? "未知"}。原因：
                      {item.errorMessage ?? "未提供原因"}
                    </small>
                  ) : null}
                </div>
                {item.status === "failed" && item.retryable ? (
                  <button
                    className="button quiet"
                    type="button"
                    onClick={() => onRetry(item)}
                    disabled={retrying}
                    aria-label={`重试 ${item.fileName}`}
                  >
                    {retrying ? (
                      <LoaderCircle
                        className="spin"
                        size={15}
                        aria-hidden="true"
                      />
                    ) : (
                      <RotateCcw size={15} aria-hidden="true" />
                    )}
                    {retrying ? "重试中" : "重试"}
                  </button>
                ) : null}
              </div>
            );
          })}
        </div>
      ) : null}

      {ignoredCount > 0 ? (
        <p className="import-ignored-note">
          已忽略 {ignoredCount} 个不受支持的文件。
        </p>
      ) : null}
    </section>
  );
}

function count(items: ImportItemResult[], status: ImportItemStatus): number {
  return items.filter((item) => item.status === status).length;
}
