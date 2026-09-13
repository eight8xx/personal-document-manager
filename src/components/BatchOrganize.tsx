import {
  AlertCircle,
  Check,
  ChevronDown,
  FolderInput,
  LoaderCircle,
  RotateCcw,
  Tag,
  Tags,
  Trash2,
  X
} from "lucide-react";
import { useEffect, useRef, useState } from "react";

import type {
  BatchDocumentOperationResult,
  CollectionSummary,
  DocumentSummary,
  TagSummary
} from "../backend/types";

export type BatchOrganizeAction =
  | "moveToCollection"
  | "addTag"
  | "removeTag"
  | "moveToTrash";

interface BatchActionMenuProps {
  count: number;
  disabled: boolean;
  onChoose: (action: BatchOrganizeAction) => void;
}

export function BatchActionMenu({
  count,
  disabled,
  onChoose
}: BatchActionMenuProps) {
  const [open, setOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) {
      return;
    }

    function handlePointerDown(event: MouseEvent) {
      if (!menuRef.current?.contains(event.target as Node)) {
        setOpen(false);
      }
    }

    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        setOpen(false);
      }
    }

    document.addEventListener("mousedown", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("mousedown", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [open]);

  function choose(action: BatchOrganizeAction) {
    setOpen(false);
    onChoose(action);
  }

  return (
    <div className="batch-action-menu" ref={menuRef}>
      <button
        className="button secondary batch-action-trigger"
        type="button"
        onClick={() => setOpen((current) => !current)}
        disabled={disabled || count === 0}
        aria-haspopup="menu"
        aria-expanded={open}
      >
        批量操作
        <ChevronDown size={15} aria-hidden="true" />
      </button>
      {open ? (
        <div className="batch-action-popover" role="menu" aria-label="批量操作">
          <button
            type="button"
            role="menuitem"
            onClick={() => choose("moveToCollection")}
          >
            <FolderInput size={15} aria-hidden="true" />
            移动到集合
          </button>
          <button
            type="button"
            role="menuitem"
            onClick={() => choose("addTag")}
          >
            <Tag size={15} aria-hidden="true" />
            添加标签
          </button>
          <button
            type="button"
            role="menuitem"
            onClick={() => choose("removeTag")}
          >
            <Tags size={15} aria-hidden="true" />
            移除标签
          </button>
          <button
            className="danger"
            type="button"
            role="menuitem"
            onClick={() => choose("moveToTrash")}
          >
            <Trash2 size={15} aria-hidden="true" />
            移入回收站
          </button>
        </div>
      ) : null}
    </div>
  );
}

interface BatchOperationDialogProps {
  action: BatchOrganizeAction;
  count: number;
  collections: CollectionSummary[];
  tags: TagSummary[];
  onClose: () => void;
  onConfirm: (targetId: string | null) => void;
}

function dialogCopy(action: BatchOrganizeAction) {
  switch (action) {
    case "moveToCollection":
      return {
        title: "批量移动到集合",
        targetLabel: "目标集合",
        confirmLabel: "确认移动"
      };
    case "addTag":
      return {
        title: "批量添加标签",
        targetLabel: "要添加的标签",
        confirmLabel: "添加标签"
      };
    case "removeTag":
      return {
        title: "批量移除标签",
        targetLabel: "要移除的标签",
        confirmLabel: "移除标签"
      };
    case "moveToTrash":
      return {
        title: "批量移入回收站",
        targetLabel: "",
        confirmLabel: "移入回收站"
      };
  }
}

export function BatchOperationDialog({
  action,
  count,
  collections,
  tags,
  onClose,
  onConfirm
}: BatchOperationDialogProps) {
  const copy = dialogCopy(action);
  const options =
    action === "moveToCollection"
      ? collections.map((collection) => ({
          id: collection.id,
          name: collection.name
        }))
      : tags.map((tag) => ({ id: tag.id, name: tag.name }));
  const [targetId, setTargetId] = useState(options[0]?.id ?? "");
  const confirmRef = useRef<HTMLButtonElement>(null);
  const needsTarget = action !== "moveToTrash";

  useEffect(() => {
    if (!needsTarget) {
      confirmRef.current?.focus();
      return;
    }

    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
      }
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [needsTarget, onClose]);

  function confirm(event: React.FormEvent) {
    event.preventDefault();
    if (needsTarget && !targetId) {
      return;
    }
    onConfirm(needsTarget ? targetId : null);
  }

  return (
    <div className="dialog-backdrop" role="presentation">
      <section
        className="compact-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="batch-operation-title"
      >
        <header className="dialog-header">
          <div>
            <p className="eyebrow">已选择 {count} 份文档</p>
            <h2 id="batch-operation-title">{copy.title}</h2>
          </div>
          <button
            className="icon-button"
            type="button"
            onClick={onClose}
            aria-label="关闭批量操作对话框"
            title="关闭"
          >
            <X size={18} aria-hidden="true" />
          </button>
        </header>
        <form className="dialog-form" onSubmit={confirm}>
          {needsTarget ? (
            options.length > 0 ? (
              <label className="field">
                <span>{copy.targetLabel}</span>
                <select
                  value={targetId}
                  onChange={(event) => setTargetId(event.target.value)}
                  autoFocus
                >
                  {options.map((option) => (
                    <option value={option.id} key={option.id}>
                      {option.name}
                    </option>
                  ))}
                </select>
              </label>
            ) : (
              <p className="dialog-error" role="alert">
                {action === "moveToCollection"
                  ? "没有可用的目标集合。"
                  : "还没有可用的标签。"}
              </p>
            )
          ) : (
            <p>
              将选中的 {count} 份文档移入回收站？资料库副本会保留，你可以稍后恢复。
            </p>
          )}
          <div className="dialog-actions">
            <button
              className="button secondary"
              type="button"
              onClick={onClose}
            >
              取消
            </button>
            <button
              ref={confirmRef}
              className={`button ${
                action === "moveToTrash" ? "danger" : "primary"
              }`}
              type="submit"
              disabled={needsTarget && !targetId}
            >
              {action === "moveToTrash" ? (
                <Trash2 size={16} aria-hidden="true" />
              ) : (
                <Check size={16} aria-hidden="true" />
              )}
              {copy.confirmLabel}
            </button>
          </div>
        </form>
      </section>
    </div>
  );
}

interface BatchResultPanelProps {
  result: BatchDocumentOperationResult;
  documents: DocumentSummary[];
  onRetry: () => void;
  onDismiss: () => void;
}

function operationLabel(result: BatchDocumentOperationResult) {
  switch (result.operation.kind) {
    case "moveToCollection":
      return "移动";
    case "addTag":
      return "添加标签";
    case "removeTag":
      return "移除标签";
    case "moveToTrash":
      return "移入回收站";
  }
}

export function BatchResultPanel({
  result,
  documents,
  onRetry,
  onDismiss
}: BatchResultPanelProps) {
  const failures = result.results.filter(
    (item) => item.status === "failed"
  );
  const cancelled = result.results.filter(
    (item) => item.status === "cancelled"
  );
  const documentTitles = new Map(
    documents.map((document) => [document.id, document.title])
  );
  const summary = [
    `成功 ${result.succeededCount} 份`,
    failures.length > 0 ? `失败 ${result.failedCount} 份` : null,
    cancelled.length > 0 ? `取消 ${result.cancelledCount} 份` : null
  ]
    .filter(Boolean)
    .join("，");

  return (
    <section
      className="batch-result-panel"
      aria-label="批量操作结果"
      aria-live="polite"
    >
      <div className="batch-result-summary">
        <span className={failures.length > 0 ? "warning" : "success"}>
          {failures.length > 0 ? (
            <AlertCircle size={17} aria-hidden="true" />
          ) : (
            <Check size={17} aria-hidden="true" />
          )}
        </span>
        <div>
          <strong>{operationLabel(result)}操作完成</strong>
          <p>{summary}</p>
        </div>
        <button
          className="icon-button"
          type="button"
          onClick={onDismiss}
          aria-label="关闭批量操作结果"
          title="关闭"
        >
          <X size={16} aria-hidden="true" />
        </button>
      </div>

      {failures.length > 0 ? (
        <div className="batch-result-failures">
          <h3>失败文档</h3>
          <ul aria-label="失败文档列表">
            {failures.map((item) => (
              <li key={item.documentId}>
                <strong>
                  {documentTitles.get(item.documentId) ?? item.documentId}
                </strong>
                <span>{item.errorMessage ?? "操作失败。"}</span>
              </li>
            ))}
          </ul>
          <button className="button secondary" type="button" onClick={onRetry}>
            <RotateCcw size={15} aria-hidden="true" />
            重试失败项目
          </button>
        </div>
      ) : null}
    </section>
  );
}

export function BatchRunningStatus({
  count,
  cancelling,
  onCancel
}: {
  count: number;
  cancelling: boolean;
  onCancel: () => void;
}) {
  return (
    <span className="batch-running-status" role="status">
      <LoaderCircle className="spin" size={15} aria-hidden="true" />
      正在处理 {count} 份文档
      <button
        className="button quiet"
        type="button"
        onClick={onCancel}
        disabled={cancelling}
      >
        {cancelling ? "正在取消" : "取消未执行项目"}
      </button>
    </span>
  );
}
