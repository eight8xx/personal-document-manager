import { LoaderCircle, Trash2, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { toBackendError } from "../backend/error";
import type { DocumentSummary } from "../backend/types";

interface MoveDocumentToTrashDialogProps {
  document: DocumentSummary;
  onClose: () => void;
  onConfirm: () => Promise<void>;
}

interface PermanentDeleteDocumentDialogProps {
  document: DocumentSummary;
  onClose: () => void;
  onConfirm: () => Promise<void>;
}

interface EmptyTrashDialogProps {
  count: number;
  onClose: () => void;
  onConfirm: () => Promise<void>;
}

interface ConfirmTrashActionDialogProps {
  eyebrow: string;
  title: string;
  body: string;
  confirmLabel: string;
  onClose: () => void;
  onConfirm: () => Promise<void>;
}

function ConfirmTrashActionDialog({
  eyebrow,
  title,
  body,
  confirmLabel,
  onClose,
  onConfirm
}: ConfirmTrashActionDialogProps) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const confirmRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    confirmRef.current?.focus();
  }, []);

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape" && !busy) {
        event.preventDefault();
        onClose();
      }
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [busy, onClose]);

  async function confirm() {
    setBusy(true);
    setError("");
    try {
      await onConfirm();
    } catch (caught) {
      setError(toBackendError(caught).message);
      setBusy(false);
    }
  }

  return (
    <div className="dialog-backdrop" role="presentation">
      <section
        className="compact-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="trash-action-title"
      >
        <header className="dialog-header">
          <div>
            <p className="eyebrow">{eyebrow}</p>
            <h2 id="trash-action-title">{title}</h2>
          </div>
          <button
            className="icon-button"
            type="button"
            onClick={onClose}
            disabled={busy}
            aria-label="关闭回收站操作对话框"
            title="关闭"
          >
            <X size={18} aria-hidden="true" />
          </button>
        </header>
        <div className="dialog-body">
          <p>{body}</p>
          {error ? (
            <p className="dialog-error" role="alert">
              {error}
            </p>
          ) : null}
          <div className="dialog-actions">
            <button
              className="button secondary"
              type="button"
              onClick={onClose}
              disabled={busy}
            >
              取消
            </button>
            <button
              ref={confirmRef}
              className="button danger"
              type="button"
              onClick={() => void confirm()}
              disabled={busy}
            >
              {busy ? (
                <LoaderCircle className="spin" size={16} aria-hidden="true" />
              ) : (
                <Trash2 size={16} aria-hidden="true" />
              )}
              {busy ? "正在处理" : confirmLabel}
            </button>
          </div>
        </div>
      </section>
    </div>
  );
}

export function MoveDocumentToTrashDialog({
  document,
  onClose,
  onConfirm
}: MoveDocumentToTrashDialogProps) {
  return (
    <ConfirmTrashActionDialog
      eyebrow="文档"
      title="移入回收站"
      body={`将“${document.title}”移入回收站？资料库副本会保留，你可以稍后恢复。`}
      confirmLabel="移入回收站"
      onClose={onClose}
      onConfirm={onConfirm}
    />
  );
}

export function PermanentDeleteDocumentDialog({
  document,
  onClose,
  onConfirm
}: PermanentDeleteDocumentDialogProps) {
  return (
    <ConfirmTrashActionDialog
      eyebrow="不可恢复"
      title="永久删除文档"
      body={`永久删除“${document.title}”？这会移除文档记录、标签关联、搜索索引和资料库副本，且无法恢复。源文件不会被删除。`}
      confirmLabel="永久删除"
      onClose={onClose}
      onConfirm={onConfirm}
    />
  );
}

export function EmptyTrashDialog({
  count,
  onClose,
  onConfirm
}: EmptyTrashDialogProps) {
  return (
    <ConfirmTrashActionDialog
      eyebrow="不可恢复"
      title="清空回收站"
      body={`将永久删除回收站中的 ${count} 份文档。这会移除文档记录、标签关联、搜索索引和资料库副本，且无法恢复。源文件不会被删除。`}
      confirmLabel="清空回收站"
      onClose={onClose}
      onConfirm={onConfirm}
    />
  );
}
