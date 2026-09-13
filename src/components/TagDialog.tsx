import { LoaderCircle, Tag as TagIcon, Trash2, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { toBackendError } from "../backend/error";
import type { TagSummary } from "../backend/types";

export type TagAction =
  | { type: "create" }
  | { type: "rename"; tag: TagSummary };

interface TagActionDialogProps {
  action: TagAction;
  onClose: () => void;
  onSubmit: (name: string) => Promise<void>;
}

interface DeleteTagDialogProps {
  tag: TagSummary;
  onClose: () => void;
  onConfirm: () => Promise<void>;
}

export function TagActionDialog({
  action,
  onClose,
  onSubmit
}: TagActionDialogProps) {
  const [name, setName] = useState(
    action.type === "rename" ? action.tag.name : ""
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  const title = action.type === "create" ? "创建标签" : "重命名标签";

  useEffect(() => {
    inputRef.current?.focus();
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

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    if (!name.trim()) {
      setError("标签名称不能为空。");
      return;
    }

    setBusy(true);
    setError("");
    try {
      await onSubmit(name.trim());
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
        aria-labelledby="tag-dialog-title"
      >
        <header className="dialog-header">
          <div>
            <p className="eyebrow">标签</p>
            <h2 id="tag-dialog-title">{title}</h2>
          </div>
          <button
            className="icon-button"
            type="button"
            onClick={onClose}
            disabled={busy}
            aria-label="关闭标签对话框"
            title="关闭"
          >
            <X size={18} aria-hidden="true" />
          </button>
        </header>

        <form className="dialog-form" onSubmit={(event) => void submit(event)}>
          <label className="field" htmlFor="tag-name">
            <span>标签名称</span>
            <input
              ref={inputRef}
              id="tag-name"
              name="name"
              value={name}
              onChange={(event) => setName(event.target.value)}
              autoComplete="off"
              disabled={busy}
              maxLength={50}
            />
          </label>

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
            <button className="button primary" type="submit" disabled={busy}>
              {busy ? (
                <LoaderCircle className="spin" size={16} aria-hidden="true" />
              ) : (
                <TagIcon size={16} aria-hidden="true" />
              )}
              {busy ? "保存中" : action.type === "create" ? "创建" : "保存"}
            </button>
          </div>
        </form>
      </section>
    </div>
  );
}

export function DeleteTagDialog({
  tag,
  onClose,
  onConfirm
}: DeleteTagDialogProps) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const confirmRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    confirmRef.current?.focus();
  }, []);

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
        aria-labelledby="delete-tag-title"
      >
        <header className="dialog-header">
          <div>
            <p className="eyebrow">标签</p>
            <h2 id="delete-tag-title">删除标签</h2>
          </div>
          <button
            className="icon-button"
            type="button"
            onClick={onClose}
            disabled={busy}
            aria-label="关闭删除标签对话框"
            title="关闭"
          >
            <X size={18} aria-hidden="true" />
          </button>
        </header>
        <div className="dialog-body">
          <p>
            {tag.documentCount > 0
              ? `删除“${tag.name}”会从 ${tag.documentCount} 份文档中移除该标签，但不会删除文档。`
              : `删除“${tag.name}”？该标签尚未用于任何文档。`}
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
              删除标签
            </button>
          </div>
        </div>
      </section>
    </div>
  );
}
