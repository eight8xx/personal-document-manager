import { Copy, FileWarning, LoaderCircle, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { toBackendError } from "../backend/error";
import type {
  DocumentSummary,
  ImportDecision,
  ImportItemResult
} from "../backend/types";

interface ImportDecisionDialogProps {
  item: ImportItemResult;
  documents: DocumentSummary[];
  onResolve: (decision: ImportDecision) => Promise<void>;
}

export function ImportDecisionDialog({
  item,
  documents,
  onResolve
}: ImportDecisionDialogProps) {
  const [confirmingReplacement, setConfirmingReplacement] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const primaryActionRef = useRef<HTMLButtonElement>(null);
  const duplicateDocument = documents.find(
    (document) => document.id === item.duplicateDocumentId
  );

  useEffect(() => {
    setConfirmingReplacement(false);
    setError("");
    primaryActionRef.current?.focus();
  }, [item.itemId]);

  async function resolve(decision: ImportDecision) {
    setBusy(true);
    setError("");
    try {
      await onResolve(decision);
    } catch (caught) {
      setError(toBackendError(caught).message);
      setBusy(false);
    }
  }

  if (item.status === "sourceChanged" && confirmingReplacement) {
    return (
      <div className="dialog-backdrop" role="presentation">
        <section
          className="compact-dialog"
          role="dialog"
          aria-modal="true"
          aria-labelledby="replace-document-title"
        >
          <header className="dialog-header">
            <div>
              <p className="eyebrow">来源变化</p>
              <h2 id="replace-document-title">确认替换文档</h2>
            </div>
            <button
              className="icon-button"
              type="button"
              onClick={() => setConfirmingReplacement(false)}
              disabled={busy}
              aria-label="返回来源变化决策"
              title="返回"
            >
              <X size={18} aria-hidden="true" />
            </button>
          </header>
          <div className="dialog-body">
            <p>
              替换后，已有文档
              {duplicateDocument ? `“${duplicateDocument.title}”` : ""}
              的旧内容不会保留。
            </p>
            {error ? (
              <p className="dialog-error" role="alert">
                {error}
              </p>
            ) : null}
            <div className="dialog-actions">
              <button
                className="button secondary"
                type="button"
                onClick={() => setConfirmingReplacement(false)}
                disabled={busy}
              >
                返回
              </button>
              <button
                className="button danger"
                type="button"
                onClick={() => void resolve("replaceExisting")}
                disabled={busy}
              >
                {busy ? (
                  <LoaderCircle className="spin" size={16} aria-hidden="true" />
                ) : null}
                确认替换
              </button>
            </div>
          </div>
        </section>
      </div>
    );
  }

  const isDuplicate = item.status === "duplicate";

  return (
    <div className="dialog-backdrop" role="presentation">
      <section
        className="compact-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="import-decision-title"
      >
        <header className="dialog-header">
          <div>
            <p className="eyebrow">{isDuplicate ? "重复内容" : "来源变化"}</p>
            <h2 id="import-decision-title">
              {isDuplicate ? "发现重复文档" : "源文件已发生变化"}
            </h2>
          </div>
          <button
            ref={primaryActionRef}
            className="icon-button"
            type="button"
            onClick={() => void resolve("cancel")}
            disabled={busy}
            aria-label="取消处理当前导入项目"
            title="取消"
          >
            <X size={18} aria-hidden="true" />
          </button>
        </header>
        <div className="dialog-body">
          <div className="decision-summary">
            {isDuplicate ? (
              <Copy size={20} aria-hidden="true" />
            ) : (
              <FileWarning size={20} aria-hidden="true" />
            )}
            <p>
              {isDuplicate
                ? `“${item.fileName}”与资料库中的文档内容完全相同。`
                : `“${item.fileName}”的来源路径已有文档，但源文件内容发生变化。`}
            </p>
          </div>
          {error ? (
            <p className="dialog-error" role="alert">
              {error}
            </p>
          ) : null}
          <div className="dialog-actions wrap">
            <button
              className="button quiet"
              type="button"
              onClick={() => void resolve("cancel")}
              disabled={busy}
            >
              取消
            </button>
            {isDuplicate ? (
              <>
                <button
                  className="button secondary"
                  type="button"
                  onClick={() => void resolve("importAnyway")}
                  disabled={busy}
                >
                  仍然单独导入
                </button>
                <button
                  className="button primary"
                  type="button"
                  onClick={() => void resolve("useExisting")}
                  disabled={busy}
                >
                  {busy ? (
                    <LoaderCircle
                      className="spin"
                      size={16}
                      aria-hidden="true"
                    />
                  ) : null}
                  打开已有文档
                </button>
              </>
            ) : (
              <>
                <button
                  className="button secondary"
                  type="button"
                  onClick={() => void resolve("createNew")}
                  disabled={busy}
                >
                  创建新文档
                </button>
                <button
                  className="button danger"
                  type="button"
                  onClick={() => setConfirmingReplacement(true)}
                  disabled={busy}
                >
                  替换已有文档
                </button>
              </>
            )}
          </div>
        </div>
      </section>
    </div>
  );
}
