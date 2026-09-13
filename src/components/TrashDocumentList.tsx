import {
  FileText,
  LoaderCircle,
  RotateCcw,
  Trash2
} from "lucide-react";

import type {
  DocumentSummary,
  TrashDocumentSummary
} from "../backend/types";
import { documentTypeIcon } from "./DocumentResults";

interface TrashDocumentListProps {
  documents: TrashDocumentSummary[];
  restoringIds: Set<string>;
  permanentlyDeletingIds: Set<string>;
  onRestore: (document: DocumentSummary) => void;
  onPermanentlyDelete: (document: DocumentSummary) => void;
}

function TrashDocumentIcon({ document }: { document: DocumentSummary }) {
  const Icon = documentTypeIcon(document);
  return <Icon size={17} aria-hidden="true" />;
}

function deletedAtCopy(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return date.toLocaleString("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit"
  });
}

export function TrashDocumentList({
  documents,
  restoringIds,
  permanentlyDeletingIds,
  onRestore,
  onPermanentlyDelete
}: TrashDocumentListProps) {
  if (documents.length === 0) {
    return (
      <main className="trash-empty-state" aria-label="回收站">
        <Trash2 size={25} aria-hidden="true" />
        <h2>回收站为空</h2>
        <p>移入回收站的文档会保留在这里，直到你恢复或永久删除。</p>
      </main>
    );
  }

  return (
    <main className="document-area" aria-label="回收站">
      <div
        className="trash-list-scroll"
        role="table"
        aria-label="回收站文档"
        aria-rowcount={documents.length + 1}
        aria-colcount={5}
      >
        <div className="trash-list-header" role="row">
          <span role="columnheader">标题</span>
          <span role="columnheader">类型</span>
          <span role="columnheader">原集合</span>
          <span role="columnheader">移入时间</span>
          <span role="columnheader">
            <span className="visually-hidden">操作</span>
          </span>
        </div>
        <div className="trash-list" role="rowgroup">
          {documents.map((item) => {
            const document = item.document;
            const restoring = restoringIds.has(document.id);
            const permanentlyDeleting = permanentlyDeletingIds.has(
              document.id
            );
            return (
              <article
                className="trash-row"
                role="row"
                key={document.id}
              >
                <div className="trash-title-cell" role="cell">
                  <span className="trash-type-icon" aria-hidden="true">
                    <TrashDocumentIcon document={document} />
                  </span>
                  <span className="trash-title-copy">
                    <strong title={document.title}>{document.title}</strong>
                    <span title={document.fileName}>{document.fileName}</span>
                  </span>
                </div>
                <span className="document-type" role="cell">
                  {document.fileType}
                </span>
                <span
                  className={`trash-collection${
                    item.originalCollectionName ? "" : " missing"
                  }`}
                  role="cell"
                  title={
                    item.originalCollectionName ?? "原集合已删除，恢复时进入收件箱"
                  }
                >
                  {item.originalCollectionName ?? "原集合已删除"}
                </span>
                <time
                  className="trash-deleted-at"
                  role="cell"
                  dateTime={item.deletedAt}
                >
                  {deletedAtCopy(item.deletedAt)}
                </time>
                <div className="trash-actions" role="cell">
                  <button
                    className="button secondary"
                    type="button"
                    onClick={() => onRestore(document)}
                    disabled={restoring || permanentlyDeleting}
                    aria-label={`恢复 ${document.title}`}
                  >
                    {restoring ? (
                      <LoaderCircle
                        className="spin"
                        size={14}
                        aria-hidden="true"
                      />
                    ) : (
                      <RotateCcw size={14} aria-hidden="true" />
                    )}
                    恢复
                  </button>
                  <button
                    className="button danger-quiet"
                    type="button"
                    onClick={() => onPermanentlyDelete(document)}
                    disabled={restoring || permanentlyDeleting}
                    aria-label={`永久删除 ${document.title}`}
                  >
                    {permanentlyDeleting ? (
                      <LoaderCircle
                        className="spin"
                        size={14}
                        aria-hidden="true"
                      />
                    ) : (
                      <Trash2 size={14} aria-hidden="true" />
                    )}
                    永久删除
                  </button>
                </div>
              </article>
            );
          })}
        </div>
      </div>
    </main>
  );
}

export function TrashDocumentFallback() {
  return (
    <main className="trash-empty-state" aria-label="回收站">
      <FileText size={24} aria-hidden="true" />
      <span>无法显示回收站文档。</span>
    </main>
  );
}
