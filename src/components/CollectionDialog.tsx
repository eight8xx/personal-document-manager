import { FolderTree, LoaderCircle, Trash2, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { toBackendError } from "../backend/error";
import type { CollectionSummary } from "../backend/types";

export type CollectionAction =
  | { type: "create"; parent: CollectionSummary | null }
  | { type: "rename"; collection: CollectionSummary }
  | { type: "move"; collection: CollectionSummary };

interface CollectionActionDialogProps {
  action: CollectionAction;
  collections: CollectionSummary[];
  onClose: () => void;
  onSubmit: (value: string | null) => Promise<void>;
}

interface DeleteCollectionDialogProps {
  collection: CollectionSummary;
  onClose: () => void;
  onConfirm: () => Promise<void>;
}

const inputId = "collection-name";

export function CollectionActionDialog({
  action,
  collections,
  onClose,
  onSubmit
}: CollectionActionDialogProps) {
  const initialName = action.type === "rename" ? action.collection.name : "";
  const [name, setName] = useState(initialName);
  const [parentId, setParentId] = useState(
    action.type === "move" ? action.collection.parentId ?? "" : ""
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  const selectRef = useRef<HTMLSelectElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
    selectRef.current?.focus();
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

  const title =
    action.type === "create"
      ? action.parent
        ? "创建子集合"
        : "创建根集合"
      : action.type === "rename"
        ? "重命名集合"
        : "移动集合";

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      await onSubmit(action.type === "move" ? parentId || null : name.trim());
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
        aria-labelledby="collection-dialog-title"
      >
        <header className="dialog-header">
          <div>
            <p className="eyebrow">集合</p>
            <h2 id="collection-dialog-title">{title}</h2>
          </div>
          <button
            className="icon-button"
            type="button"
            onClick={onClose}
            disabled={busy}
            aria-label="关闭集合对话框"
            title="关闭"
          >
            <X size={18} aria-hidden="true" />
          </button>
        </header>

        <form className="dialog-form" onSubmit={(event) => void submit(event)}>
          {action.type === "move" ? (
            <label className="field" htmlFor="collection-parent">
              <span>目标位置</span>
              <select
                ref={selectRef}
                id="collection-parent"
                name="parentId"
                value={parentId}
                onChange={(event) => setParentId(event.target.value)}
                disabled={busy}
              >
                <option value="">无（根集合）</option>
                {collections
                  .filter(
                    (collection) =>
                      collection.id !== action.collection.id &&
                      !isDescendant(
                        collection.id,
                        action.collection.id,
                        collections
                      )
                  )
                  .map((collection) => (
                    <option key={collection.id} value={collection.id}>
                      {collection.name}
                    </option>
                  ))}
              </select>
            </label>
          ) : (
            <label className="field" htmlFor={inputId}>
              <span>集合名称</span>
              <input
                ref={inputRef}
                id={inputId}
                name="name"
                value={name}
                onChange={(event) => setName(event.target.value)}
                autoComplete="off"
                disabled={busy}
              />
            </label>
          )}

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
                <FolderTree size={16} aria-hidden="true" />
              )}
              {action.type === "create"
                ? "创建"
                : action.type === "rename"
                  ? "保存"
                  : "移动"}
            </button>
          </div>
        </form>
      </section>
    </div>
  );
}

export function DeleteCollectionDialog({
  collection,
  onClose,
  onConfirm
}: DeleteCollectionDialogProps) {
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
        aria-labelledby="delete-collection-title"
      >
        <header className="dialog-header">
          <div>
            <p className="eyebrow">集合</p>
            <h2 id="delete-collection-title">删除集合</h2>
          </div>
          <button
            className="icon-button"
            type="button"
            onClick={onClose}
            disabled={busy}
            aria-label="关闭删除集合对话框"
            title="关闭"
          >
            <X size={18} aria-hidden="true" />
          </button>
        </header>
        <div className="dialog-body">
          <p>
            {collection.documentCount > 0
              ? `“${collection.name}”中有 ${collection.documentCount} 份文档。删除集合后，这些文档将移动到收件箱。`
              : `删除“${collection.name}”？空集合删除后只会移除集合本身。`}
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
              删除集合
            </button>
          </div>
        </div>
      </section>
    </div>
  );
}

function isDescendant(
  candidateId: string,
  collectionId: string,
  collections: CollectionSummary[]
): boolean {
  let current = collections.find((item) => item.id === candidateId);
  const visited = new Set<string>();

  while (current?.parentId && !visited.has(current.id)) {
    if (current.parentId === collectionId) {
      return true;
    }
    visited.add(current.id);
    current = collections.find((item) => item.id === current?.parentId);
  }

  return false;
}
