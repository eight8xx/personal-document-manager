import { LoaderCircle, Save, X } from "lucide-react";
import { useEffect, useState } from "react";

import { toBackendError } from "../backend/error";
import type {
  CollectionSummary,
  DocumentMetadataUpdate,
  DocumentSummary,
  TagSummary
} from "../backend/types";

interface DocumentMetadataDialogProps {
  document: DocumentSummary;
  collections: CollectionSummary[];
  tags: TagSummary[];
  onClose: () => void;
  onSave: (update: DocumentMetadataUpdate) => Promise<void>;
}

export function DocumentMetadataDialog({
  document: initialDocument,
  collections,
  tags,
  onClose,
  onSave
}: DocumentMetadataDialogProps) {
  const [title, setTitle] = useState(initialDocument.title);
  const [description, setDescription] = useState(
    initialDocument.description ?? ""
  );
  const [documentDate, setDocumentDate] = useState(
    initialDocument.documentDate ?? ""
  );
  const [collectionId, setCollectionId] = useState(
    initialDocument.collectionId
  );
  const [selectedTagIds, setSelectedTagIds] = useState(
    new Set(initialDocument.tags.map((tag) => tag.id))
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

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

  function toggleTag(tagId: string) {
    setSelectedTagIds((current) => {
      const next = new Set(current);
      if (next.has(tagId)) {
        next.delete(tagId);
      } else {
        next.add(tagId);
      }
      return next;
    });
  }

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    if (!title.trim()) {
      setError("文档标题不能为空。");
      return;
    }

    setBusy(true);
    setError("");
    try {
      await onSave({
        title: title.trim(),
        description: description.trim() || null,
        documentDate: documentDate || null,
        collectionId,
        tagIds: [...selectedTagIds]
      });
    } catch (caught) {
      setError(toBackendError(caught).message);
      setBusy(false);
    }
  }

  return (
    <div className="dialog-backdrop" role="presentation">
      <section
        className="document-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="document-metadata-title"
      >
        <header className="dialog-header">
          <div>
            <p className="eyebrow">文档元数据</p>
            <h2 id="document-metadata-title">编辑文档</h2>
          </div>
          <button
            className="icon-button"
            type="button"
            onClick={onClose}
            disabled={busy}
            aria-label="关闭文档元数据对话框"
            title="关闭"
          >
            <X size={18} aria-hidden="true" />
          </button>
        </header>

        <form className="dialog-form" onSubmit={(event) => void submit(event)}>
          <label className="field" htmlFor="document-title">
            <span>标题</span>
            <input
              id="document-title"
              name="title"
              value={title}
              onChange={(event) => setTitle(event.target.value)}
              autoFocus
              disabled={busy}
              maxLength={500}
            />
          </label>

          <label className="field" htmlFor="document-description">
            <span>描述</span>
            <textarea
              id="document-description"
              name="description"
              value={description}
              onChange={(event) => setDescription(event.target.value)}
              disabled={busy}
              rows={4}
            />
          </label>

          <div className="field-row">
            <label className="field" htmlFor="document-date">
              <span>文档日期</span>
              <input
                id="document-date"
                name="documentDate"
                type="date"
                value={documentDate}
                onChange={(event) => setDocumentDate(event.target.value)}
                disabled={busy}
              />
            </label>

            <label className="field" htmlFor="document-collection">
              <span>所属集合</span>
              <select
                id="document-collection"
                name="collectionId"
                value={collectionId}
                onChange={(event) => setCollectionId(event.target.value)}
                disabled={busy}
              >
                {collections.map((collection) => (
                  <option key={collection.id} value={collection.id}>
                    {collection.name}
                  </option>
                ))}
              </select>
            </label>
          </div>

          <fieldset className="tag-picker" disabled={busy}>
            <legend>标签</legend>
            {tags.length === 0 ? (
              <p className="tag-picker-empty">暂无标签，请先在侧栏创建。</p>
            ) : (
              <div className="tag-picker-options">
                {tags.map((tag) => (
                  <label className="tag-picker-option" key={tag.id}>
                    <input
                      type="checkbox"
                      checked={selectedTagIds.has(tag.id)}
                      onChange={() => toggleTag(tag.id)}
                    />
                    <span>{tag.name}</span>
                  </label>
                ))}
              </div>
            )}
          </fieldset>

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
                <Save size={16} aria-hidden="true" />
              )}
              {busy ? "保存中" : error ? "重试保存" : "保存"}
            </button>
          </div>
        </form>
      </section>
    </div>
  );
}
