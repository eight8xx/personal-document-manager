import {
  FileText,
  Image as ImageIcon,
  LoaderCircle,
  Pencil,
  RotateCcw,
  Trash2
} from "lucide-react";
import { useEffect, useState } from "react";

import type {
  BackendClient,
  CollectionSummary,
  DocumentSearchResult,
  DocumentSummary,
  DocumentThumbnail
} from "../backend/types";

export interface StatusPresentation {
  label: string;
  tone: "neutral" | "success" | "warning" | "danger";
}

interface DocumentResultsProps {
  documents: DocumentSummary[];
  collections: CollectionSummary[];
  selectedDocumentId: string | null;
  highlightedDocumentId: string | null;
  searchResults: DocumentSearchResult[];
  retryingIndexIds: Set<string>;
  onSelectDocument: (documentId: string) => void;
  onMoveDocument: (
    document: DocumentSummary,
    collectionId: string
  ) => void;
  onEditDocument: (document: DocumentSummary) => void;
  onMoveDocumentToTrash: (document: DocumentSummary) => void;
  onRetryIndex: (document: DocumentSummary) => void;
}

export function documentStatusPresentation(
  document: DocumentSummary
): StatusPresentation {
  if (document.processingStatus === "failed") {
    return { label: "处理失败，等待重试", tone: "danger" };
  }
  if (document.processingStatus === "processing") {
    return { label: "处理中", tone: "neutral" };
  }
  if (document.indexStatus === "searchable") {
    return { label: "可搜索", tone: "success" };
  }
  if (document.indexStatus === "failed") {
    return { label: "索引失败，可重试", tone: "danger" };
  }
  return { label: "等待索引", tone: "warning" };
}

function DocumentIcon({ document }: { document: DocumentSummary }) {
  const Icon = documentTypeIcon(document);
  return <Icon size={18} aria-hidden="true" />;
}

function DocumentThumbnailVisual({
  client,
  document
}: {
  client: BackendClient;
  document: DocumentSummary;
}) {
  const [thumbnail, setThumbnail] = useState<DocumentThumbnail | null>(null);
  const usesGeneratedThumbnail = isImageDocument(document) || isPdfDocument(
    document
  );

  useEffect(() => {
    let active = true;
    setThumbnail(null);
    if (!usesGeneratedThumbnail) {
      return () => {
        active = false;
      };
    }

    void client
      .getDocumentThumbnail(document.id)
      .then((result) => {
        if (active) {
          setThumbnail(result);
        }
      })
      .catch(() => {
        if (active) {
          setThumbnail({
            kind: "fallback",
            reason: "无法加载缩略图。"
          });
        }
      });

    return () => {
      active = false;
    };
  }, [client, document.id, usesGeneratedThumbnail]);

  if (thumbnail?.kind === "image") {
    return (
      <img
        className="document-grid-thumbnail"
        src={thumbnail.dataUrl}
        alt=""
        onError={() =>
          setThumbnail({ kind: "fallback", reason: "无法显示缩略图。" })
        }
      />
    );
  }

  if (thumbnail?.kind === "pdf") {
    return (
      <iframe
        className="document-grid-thumbnail document-grid-pdf-thumbnail"
        title=""
        tabIndex={-1}
        src={`${thumbnail.dataUrl.split("#", 1)[0]}#page=1&zoom=page-width&toolbar=0&navpanes=0`}
        onError={() =>
          setThumbnail({ kind: "fallback", reason: "无法显示缩略图。" })
        }
      />
    );
  }

  return <DocumentIcon document={document} />;
}

function DocumentTags({
  document,
  compact = false,
  tableCell = false
}: {
  document: DocumentSummary;
  compact?: boolean;
  tableCell?: boolean;
}) {
  const tagNames = document.tags.map((tag) => tag.name);
  const visibleTags = document.tags.slice(0, compact ? 2 : 3);
  const remainingCount = document.tags.length - visibleTags.length;
  const accessibleLabel =
    tagNames.length > 0
      ? `${document.title} 的标签：${tagNames.join("、")}`
      : `${document.title} 无标签`;

  return (
    <span
      className={`document-tags${compact ? " compact" : ""}`}
      role={tableCell ? "cell" : undefined}
      aria-label={accessibleLabel}
      title={tagNames.join("、") || "无标签"}
    >
      {visibleTags.length === 0 ? (
        <span className="document-tag-empty">无标签</span>
      ) : (
        visibleTags.map((tag) => (
          <span className="document-tag" key={tag.id}>
            {tag.name}
          </span>
        ))
      )}
      {remainingCount > 0 ? (
        <span className="document-tag document-tag-more">
          +{remainingCount}
        </span>
      ) : null}
    </span>
  );
}

function DocumentRow({
  document,
  collections,
  selected,
  highlighted,
  searchResult,
  retryingIndex,
  onSelect,
  onMove,
  onEdit,
  onMoveToTrash,
  onRetryIndex
}: {
  document: DocumentSummary;
  collections: CollectionSummary[];
  selected: boolean;
  highlighted: boolean;
  searchResult: DocumentSearchResult | null;
  retryingIndex: boolean;
  onSelect: () => void;
  onMove: (collectionId: string) => void;
  onEdit: () => void;
  onMoveToTrash: () => void;
  onRetryIndex: () => void;
}) {
  const status = documentStatusPresentation(document);
  const resultCopy =
    searchResult?.snippet && searchResult.matchKind === "content"
      ? searchResult.snippet
      : document.fileName;

  return (
    <article
      id={`document-row-${document.id}`}
      className={`document-row collection-aware${selected ? " selected" : ""}${
        highlighted ? " highlighted" : ""
      }`}
      role="row"
      aria-current={highlighted ? "true" : undefined}
    >
      <div className="document-title-cell" role="cell">
        <button
          className="document-select"
          type="button"
          onClick={onSelect}
          aria-pressed={selected}
          aria-label={`选择文档 ${document.title}`}
        >
          <DocumentIcon document={document} />
          <span className="document-title-copy">
            <strong title={document.title}>{document.title}</strong>
            <span
              className={
                searchResult?.snippet && searchResult.matchKind === "content"
                  ? "document-match-snippet"
                  : undefined
              }
              title={resultCopy}
            >
              {searchResult?.snippet && searchResult.matchKind === "content" ? (
                <span className="visually-hidden">正文匹配片段：</span>
              ) : null}
              {resultCopy}
            </span>
          </span>
        </button>
      </div>
      <span
        className="document-type"
        role="cell"
        title={document.fileType}
      >
        {document.fileType}
      </span>
      <time
        className={`document-date${document.documentDate ? "" : " unset"}`}
        role="cell"
        dateTime={document.documentDate ?? undefined}
      >
        {document.documentDate ?? "未设置"}
      </time>
      <label className="document-collection" role="cell">
        <span className="visually-hidden">
          移动 {document.title} 到集合
        </span>
        <select
          value={document.collectionId}
          onChange={(event) => onMove(event.target.value)}
          aria-label={`移动 ${document.title} 到集合`}
        >
          {collections.map((collection) => (
            <option key={collection.id} value={collection.id}>
              {collection.name}
            </option>
          ))}
        </select>
      </label>
      <DocumentTags document={document} tableCell />
      <div className="document-status-cell" role="cell">
        <span
          className={`status-badge ${status.tone}`}
          title={document.errorMessage ?? undefined}
        >
          {status.label}
        </span>
        {document.indexStatus === "failed" ? (
          <button
            className="icon-button compact retry-index"
            type="button"
            onClick={onRetryIndex}
            disabled={retryingIndex}
            aria-label={`重试索引 ${document.title}`}
            title="重试索引"
          >
            {retryingIndex ? (
              <LoaderCircle className="spin" size={14} aria-hidden="true" />
            ) : (
              <RotateCcw size={14} aria-hidden="true" />
            )}
          </button>
        ) : null}
      </div>
      <div className="document-actions-cell" role="cell">
        <button
          className="icon-button compact document-trash"
          type="button"
          onClick={onMoveToTrash}
          aria-label={`将 ${document.title} 移入回收站`}
          title="移入回收站"
        >
          <Trash2 size={14} aria-hidden="true" />
        </button>
        <button
          className="icon-button compact document-edit"
          type="button"
          onClick={onEdit}
          aria-label={`编辑 ${document.title} 元数据`}
          title="编辑元数据"
        >
          <Pencil size={14} aria-hidden="true" />
        </button>
      </div>
    </article>
  );
}

export function DocumentList({
  documents,
  collections,
  selectedDocumentId,
  highlightedDocumentId,
  searchResults,
  retryingIndexIds,
  onSelectDocument,
  onMoveDocument,
  onEditDocument,
  onMoveDocumentToTrash,
  onRetryIndex
}: DocumentResultsProps) {
  return (
    <main className="document-area" aria-label="文档列表">
      <div
        className="document-list-scroll"
        role="table"
        aria-label="文档结果"
        aria-rowcount={documents.length + 1}
        aria-colcount={7}
      >
        <div
          className="document-list-header collection-aware"
          role="row"
        >
          <span role="columnheader">标题</span>
          <span role="columnheader">类型</span>
          <span role="columnheader">文档日期</span>
          <span role="columnheader">集合</span>
          <span role="columnheader">标签</span>
          <span role="columnheader">处理状态</span>
          <span role="columnheader">
            <span className="visually-hidden">操作</span>
          </span>
        </div>
        <div className="document-list" role="rowgroup">
          {documents.map((document) => (
            <DocumentRow
              key={document.id}
              document={document}
              collections={collections}
              selected={document.id === selectedDocumentId}
              highlighted={document.id === highlightedDocumentId}
              searchResult={
                searchResults.find(
                  (result) => result.document.id === document.id
                ) ?? null
              }
              retryingIndex={retryingIndexIds.has(document.id)}
              onSelect={() => onSelectDocument(document.id)}
              onMove={(collectionId) =>
                onMoveDocument(document, collectionId)
              }
              onEdit={() => onEditDocument(document)}
              onMoveToTrash={() => onMoveDocumentToTrash(document)}
              onRetryIndex={() => onRetryIndex(document)}
            />
          ))}
        </div>
      </div>
    </main>
  );
}

export function DocumentGrid({
  client,
  documents,
  collections,
  selectedDocumentId,
  highlightedDocumentId,
  searchResults,
  retryingIndexIds,
  onSelectDocument,
  onMoveDocumentToTrash,
  onRetryIndex
}: DocumentResultsProps & { client: BackendClient }) {
  return (
    <main className="document-area" aria-label="文档网格">
      <div className="document-grid" role="list" aria-label="文档结果">
        {documents.map((document) => {
          const status = documentStatusPresentation(document);
          const collection = collections.find(
            (candidate) => candidate.id === document.collectionId
          );
          const selected = document.id === selectedDocumentId;
          const highlighted = document.id === highlightedDocumentId;
          const searchResult =
            searchResults.find((result) => result.document.id === document.id) ??
            null;
          const resultCopy =
            searchResult?.snippet && searchResult.matchKind === "content"
              ? searchResult.snippet
              : document.fileName;

          return (
            <article
              id={`document-card-${document.id}`}
              className={`document-grid-item${selected ? " selected" : ""}${
                highlighted ? " highlighted" : ""
              }`}
              role="listitem"
              aria-current={highlighted ? "true" : undefined}
              key={document.id}
            >
              <button
                className="document-grid-select"
                type="button"
                onClick={() => onSelectDocument(document.id)}
                aria-pressed={selected}
                aria-label={`选择文档 ${document.title}`}
              >
                <span className="document-grid-visual" aria-hidden="true">
                  <DocumentThumbnailVisual
                    client={client}
                    document={document}
                  />
                  <span>{document.fileType}</span>
                </span>
                <strong title={document.title}>{document.title}</strong>
                <span className="document-grid-file" title={resultCopy}>
                  {resultCopy}
                </span>
                <span className="document-grid-metadata">
                  <span title={document.documentDate ?? "未设置"}>
                    {document.documentDate ?? "未设置"}
                  </span>
                  <span title={collection?.name ?? "未知集合"}>
                    {collection?.name ?? "未知集合"}
                  </span>
                </span>
                <DocumentTags document={document} compact />
                <span className={`status-badge ${status.tone}`}>
                  {status.label}
                </span>
              </button>
              {document.indexStatus === "failed" ? (
                <button
                  className="icon-button compact grid-retry-index"
                  type="button"
                  onClick={() => onRetryIndex(document)}
                  disabled={retryingIndexIds.has(document.id)}
                  aria-label={`重试索引 ${document.title}`}
                  title="重试索引"
                >
                  {retryingIndexIds.has(document.id) ? (
                    <LoaderCircle
                      className="spin"
                      size={14}
                      aria-hidden="true"
                    />
                  ) : (
                    <RotateCcw size={14} aria-hidden="true" />
                  )}
                </button>
              ) : null}
              <button
                className="icon-button compact grid-move-to-trash"
                type="button"
                onClick={() => onMoveDocumentToTrash(document)}
                aria-label={`将 ${document.title} 移入回收站`}
                title="移入回收站"
              >
                <Trash2 size={14} aria-hidden="true" />
              </button>
            </article>
          );
        })}
      </div>
    </main>
  );
}

export function findCollectionName(
  collections: CollectionSummary[],
  collectionId: string
) {
  return (
    collections.find((collection) => collection.id === collectionId)?.name ??
    "未知集合"
  );
}

export function documentTypeIcon(document: DocumentSummary) {
  if (isImageDocument(document)) {
    return ImageIcon;
  }
  return FileText;
}

export function isImageDocument(document: DocumentSummary) {
  return ["JPG", "JPEG", "PNG"].includes(document.fileType.toUpperCase());
}

export function isPdfDocument(document: DocumentSummary) {
  return document.fileType.toUpperCase() === "PDF";
}
