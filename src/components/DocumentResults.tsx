import {
  FileText,
  Image as ImageIcon,
  LoaderCircle,
  Pencil,
  Presentation,
  RotateCcw,
  Trash2
} from "lucide-react";
import { useContext, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type {
  KeyboardEvent as ReactKeyboardEvent,
  MouseEvent as ReactMouseEvent,
  PointerEvent as ReactPointerEvent,
  UIEvent
} from "react";

import type {
  BackendClient,
  CollectionSummary,
  DocumentSearchResult,
  DocumentSummary,
  DocumentThumbnail
} from "../backend/types";
import { LibraryContext } from "../backend/libraryContext";
import {
  documentSupportsGeneratedThumbnail,
  documentUsesLocalImage,
  documentUsesPdfPreview,
  documentUsesPptxPreview
} from "../backend/documentFormats";
import { PptxThumbnail } from "./PptxPreview";

export interface StatusPresentation {
  label: string;
  tone: "neutral" | "success" | "warning" | "danger";
}

const LIST_ROW_HEIGHT = 72;
const LIST_HEADER_HEIGHT = 38;
const GRID_CARD_HEIGHT = 236;
const GRID_GAP = 12;
const GRID_ROW_HEIGHT = GRID_CARD_HEIGHT + GRID_GAP;
const GRID_MIN_CARD_WIDTH = 190;
const GRID_HORIZONTAL_PADDING = 32;
const GRID_VERTICAL_PADDING = 16;
const WINDOW_OVERSCAN_ROWS = 5;
const FALLBACK_VIEWPORT_HEIGHT = 720;
const FALLBACK_GRID_WIDTH = 900;

type ResultView = "list" | "grid";

function gridColumns(width: number) {
  const availableWidth = Math.max(0, width - GRID_HORIZONTAL_PADDING);
  return Math.max(
    1,
    Math.floor(
      (availableWidth + GRID_GAP) / (GRID_MIN_CARD_WIDTH + GRID_GAP)
    )
  );
}

function useVirtualDocuments(
  documents: DocumentSummary[],
  highlightedDocumentId: string | null,
  view: ResultView
) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const pendingFocusIndex = useRef<number | null>(null);
  const lastPositionedHighlight = useRef<string | null>(null);
  const [viewport, setViewport] = useState({
    width: FALLBACK_GRID_WIDTH,
    height: FALLBACK_VIEWPORT_HEIGHT,
    scrollTop: 0
  });
  const viewportRef = useRef(viewport);
  function updateViewport(next: typeof viewport) {
    viewportRef.current = next;
    setViewport(next);
  }
  const documentOrder = useMemo(
    () => documents.map((document) => document.id).join("\u0000"),
    [documents]
  );
  const columns = view === "grid" ? gridColumns(viewport.width) : 1;
  const rowHeight = view === "grid" ? GRID_ROW_HEIGHT : LIST_ROW_HEIGHT;
  const leadingOffset =
    view === "grid" ? GRID_VERTICAL_PADDING : LIST_HEADER_HEIGHT;
  const trailingOffset = view === "grid" ? GRID_VERTICAL_PADDING : 0;
  const rowCount = Math.ceil(documents.length / columns);
  const totalHeight = Math.max(
    0,
    rowCount * rowHeight - (view === "grid" && rowCount > 0 ? GRID_GAP : 0)
  );
  const maxScrollTop = Math.max(
    0,
    leadingOffset + totalHeight + trailingOffset - viewport.height
  );
  const scrollTop = Math.min(viewport.scrollTop, maxScrollTop);
  const firstRow = Math.max(
    0,
    Math.floor(Math.max(0, scrollTop - leadingOffset) / rowHeight) -
      WINDOW_OVERSCAN_ROWS
  );
  const afterLastRow = Math.min(
    rowCount,
    Math.ceil((scrollTop + viewport.height - leadingOffset) / rowHeight) +
      WINDOW_OVERSCAN_ROWS
  );
  const firstIndex = firstRow * columns;
  const afterLastIndex = Math.min(documents.length, afterLastRow * columns);

  useLayoutEffect(() => {
    const element = scrollRef.current;
    if (!element) {
      return;
    }
    const measure = () => {
      const width = element.clientWidth || FALLBACK_GRID_WIDTH;
      const height = element.clientHeight || FALLBACK_VIEWPORT_HEIGHT;
      const current = viewportRef.current;
      const oldColumns = view === "grid" ? gridColumns(current.width) : 1;
      const newColumns = view === "grid" ? gridColumns(width) : 1;
      const firstVisibleIndex =
        Math.floor(
          Math.max(0, current.scrollTop - leadingOffset) / rowHeight
        ) * oldColumns;
      const nextScrollTop =
        oldColumns === newColumns
          ? element.scrollTop
          : current.scrollTop === 0
            ? 0
            : leadingOffset +
              Math.floor(firstVisibleIndex / newColumns) * rowHeight;
      if (oldColumns !== newColumns) {
        element.scrollTop = nextScrollTop;
      }
      if (
        current.width !== width ||
        current.height !== height ||
        current.scrollTop !== nextScrollTop
      ) {
        updateViewport({ width, height, scrollTop: nextScrollTop });
      }
    };
    measure();
    const observer =
      typeof ResizeObserver === "function" ? new ResizeObserver(measure) : null;
    observer?.observe(element);
    window.addEventListener("resize", measure);
    return () => {
      observer?.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, [leadingOffset, rowHeight, view]);

  useLayoutEffect(() => {
    const element = scrollRef.current;
    if (element) {
      element.scrollTop = 0;
    }
    const current = viewportRef.current;
    if (current.scrollTop !== 0) {
      updateViewport({ ...current, scrollTop: 0 });
    }
  }, [documentOrder]);

  useLayoutEffect(() => {
    if (!highlightedDocumentId) {
      lastPositionedHighlight.current = null;
      return;
    }
    if (lastPositionedHighlight.current === highlightedDocumentId) {
      return;
    }
    const element = scrollRef.current;
    if (!element) {
      return;
    }
    const index = documents.findIndex(
      (document) => document.id === highlightedDocumentId
    );
    if (index < 0) {
      return;
    }
    lastPositionedHighlight.current = highlightedDocumentId;
    const targetTop =
      leadingOffset + Math.floor(index / columns) * rowHeight;
    const currentTop = element.scrollTop;
    if (
      targetTop >= currentTop &&
      targetTop + rowHeight <= currentTop + viewport.height
    ) {
      return;
    }
    const nextTop = Math.max(0, targetTop - Math.floor(viewport.height / 2));
    element.scrollTop = nextTop;
    updateViewport({ ...viewportRef.current, scrollTop: nextTop });
  }, [columns, documentOrder, highlightedDocumentId, leadingOffset, rowHeight, viewport.height]);

  useLayoutEffect(() => {
    const index = pendingFocusIndex.current;
    if (index === null) {
      return;
    }
    const button = scrollRef.current?.querySelector<HTMLButtonElement>(
      `[data-result-index="${index}"] .${view === "grid" ? "document-grid-select" : "document-select"}`
    );
    if (button) {
      button.focus({ preventScroll: true });
      pendingFocusIndex.current = null;
    }
  }, [afterLastIndex, firstIndex, view]);

  function onScroll(event: UIEvent<HTMLDivElement>) {
    const element = event.currentTarget;
    const active = document.activeElement;
    const activeRow =
      active instanceof HTMLElement
        ? active.closest<HTMLElement>("[data-result-index]")
        : null;
    if (activeRow && element.contains(activeRow)) {
      const activeIndex = Number(activeRow.dataset.resultIndex);
      const nextFirstRow = Math.max(
        0,
        Math.floor(
          Math.max(0, element.scrollTop - leadingOffset) / rowHeight
        ) - WINDOW_OVERSCAN_ROWS
      );
      const nextLastRow = Math.min(
        rowCount,
        Math.ceil(
          (element.scrollTop + viewport.height - leadingOffset) / rowHeight
        ) +
          WINDOW_OVERSCAN_ROWS
      );
      if (
        activeIndex < nextFirstRow * columns ||
        activeIndex >= nextLastRow * columns
      ) {
        element.focus({ preventScroll: true });
      }
    }
    if (viewportRef.current.scrollTop !== element.scrollTop) {
      updateViewport({ ...viewportRef.current, scrollTop: element.scrollTop });
    }
  }

  function onKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    const target = event.target;
    if (
      target !== event.currentTarget &&
      !(target instanceof HTMLElement &&
        target.matches(".document-select, .document-grid-select"))
    ) {
      return;
    }
    const item =
      target instanceof HTMLElement
        ? target.closest<HTMLElement>("[data-result-index]")
        : null;
    const firstVisibleIndex =
      Math.floor(Math.max(0, scrollTop - leadingOffset) / rowHeight) *
      columns;
    const currentIndex = item
      ? Number(item.dataset.resultIndex)
      : Math.min(documents.length - 1, firstVisibleIndex);
    const pageRows = Math.max(1, Math.floor(viewport.height / rowHeight));
    let nextIndex: number;
    switch (event.key) {
      case "ArrowDown":
        nextIndex = currentIndex + columns;
        break;
      case "ArrowUp":
        nextIndex = currentIndex - columns;
        break;
      case "ArrowRight":
        if (view !== "grid") return;
        nextIndex = currentIndex + 1;
        break;
      case "ArrowLeft":
        if (view !== "grid") return;
        nextIndex = currentIndex - 1;
        break;
      case "PageDown":
        nextIndex = currentIndex + pageRows * columns;
        break;
      case "PageUp":
        nextIndex = currentIndex - pageRows * columns;
        break;
      case "Home":
        nextIndex = 0;
        break;
      case "End":
        nextIndex = documents.length - 1;
        break;
      default:
        return;
    }
    if (documents.length === 0) {
      return;
    }
    event.preventDefault();
    const index = Math.max(0, Math.min(documents.length - 1, nextIndex));
    pendingFocusIndex.current = index;
    const targetRow = Math.floor(index / columns);
    const targetTop = leadingOffset + targetRow * rowHeight;
    let nextTop = scrollTop;
    if (targetTop < scrollTop || targetTop + rowHeight > scrollTop + viewport.height) {
      nextTop = Math.max(0, targetTop - Math.floor(viewport.height / 2));
    }
    const element = scrollRef.current;
    if (element) {
      element.scrollTop = nextTop;
      const button = element.querySelector<HTMLButtonElement>(
        `[data-result-index="${index}"] .${view === "grid" ? "document-grid-select" : "document-select"}`
      );
      if (button) {
        button.focus({ preventScroll: true });
        pendingFocusIndex.current = null;
      }
    }
    updateViewport({ ...viewportRef.current, scrollTop: nextTop });
  }

  return {
    scrollRef,
    onScroll,
    onKeyDown,
    columns,
    firstIndex,
    afterLastIndex,
    topOffset: firstRow * rowHeight,
    totalHeight
  };
}

interface DocumentResultsProps {
  documents: DocumentSummary[];
  collections: CollectionSummary[];
  selectedDocumentId: string | null;
  selectedDocumentIds: Set<string>;
  draggingDocumentIds: Set<string>;
  selectionDisabled: boolean;
  highlightedDocumentId: string | null;
  searchResults: DocumentSearchResult[];
  retryingIndexIds: Set<string>;
  onSelectDocument: (
    documentId: string,
    event: ReactMouseEvent<HTMLButtonElement>
  ) => void;
  onStartDocumentDrag: (
    document: DocumentSummary,
    event: ReactPointerEvent<HTMLButtonElement>
  ) => void;
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
    return { label: "处理失败，等待重试", tone: "danger" };
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
  const library = useContext(LibraryContext);
  const [thumbnail, setThumbnail] = useState<DocumentThumbnail | null>(null);
  const usesGeneratedThumbnail =
    documentSupportsGeneratedThumbnail(document) &&
    !documentUsesPptxPreview(document);

  useEffect(() => {
    let active = true;
    setThumbnail(null);
    if (!usesGeneratedThumbnail) {
      return () => {
        active = false;
      };
    }
    if (!library) {
      setThumbnail({ kind: "fallback", reason: "当前资料库不可用。" });
      return () => {
        active = false;
      };
    }

    void client
      .getDocumentThumbnail(library, document.id)
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
  }, [
    client,
    library,
    document.id,
    document.contentHash,
    document.fileSize,
    document.lastImportedAt,
    usesGeneratedThumbnail
  ]);

  if (documentUsesPptxPreview(document)) {
    return <PptxThumbnail client={client} document={document} />;
  }

  if (
    thumbnail?.kind === "image" ||
    thumbnail?.kind === "pdf" ||
    thumbnail?.kind === "pptx"
  ) {
    return (
      <img
        className="document-grid-thumbnail"
        src={thumbnail.dataUrl}
        alt=""
        data-thumbnail-kind={thumbnail.kind}
        draggable={false}
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
  resultIndex,
  collections,
  selected,
  dragging,
  selectionDisabled,
  highlighted,
  searchResult,
  retryingIndex,
  onSelect,
  onStartDocumentDrag,
  onMove,
  onEdit,
  onMoveToTrash,
  onRetryIndex
}: {
  document: DocumentSummary;
  resultIndex: number;
  collections: CollectionSummary[];
  selected: boolean;
  dragging: boolean;
  selectionDisabled: boolean;
  highlighted: boolean;
  searchResult: DocumentSearchResult | null;
  retryingIndex: boolean;
  onSelect: (event: ReactMouseEvent<HTMLButtonElement>) => void;
  onStartDocumentDrag: (
    event: ReactPointerEvent<HTMLButtonElement>
  ) => void;
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
      data-result-index={resultIndex}
      className={`document-row collection-aware${selected ? " selected" : ""}${
        highlighted ? " highlighted" : ""
      }${dragging ? " dragging" : ""}`}
      role="row"
      aria-rowindex={resultIndex + 2}
      aria-current={highlighted ? "true" : undefined}
    >
      <div className="document-title-cell" role="cell">
        <button
          className="document-select"
          type="button"
          onClick={onSelect}
          onPointerDown={onStartDocumentDrag}
          disabled={selectionDisabled}
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
  selectedDocumentIds,
  draggingDocumentIds,
  selectionDisabled,
  highlightedDocumentId,
  searchResults,
  retryingIndexIds,
  onSelectDocument,
  onStartDocumentDrag,
  onMoveDocument,
  onEditDocument,
  onMoveDocumentToTrash,
  onRetryIndex
}: DocumentResultsProps) {
  const virtual = useVirtualDocuments(
    documents,
    highlightedDocumentId,
    "list"
  );
  const searchResultsById = useMemo(
    () => new Map(searchResults.map((result) => [result.document.id, result])),
    [searchResults]
  );

  return (
    <main className="document-area" aria-label="文档列表">
      <div
        className="document-list-scroll"
        ref={virtual.scrollRef}
        role="table"
        aria-label="文档结果"
        aria-rowcount={documents.length + 1}
        aria-colcount={7}
        tabIndex={0}
        onScroll={virtual.onScroll}
        onKeyDown={virtual.onKeyDown}
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
          <div
            role="presentation"
            aria-hidden="true"
            style={{ height: virtual.topOffset }}
          />
          {documents
            .slice(virtual.firstIndex, virtual.afterLastIndex)
            .map((document, windowIndex) => (
              <DocumentRow
                key={document.id}
                document={document}
                resultIndex={virtual.firstIndex + windowIndex}
                collections={collections}
                selected={selectedDocumentIds.has(document.id)}
                dragging={draggingDocumentIds.has(document.id)}
                selectionDisabled={selectionDisabled}
                highlighted={document.id === highlightedDocumentId}
                searchResult={searchResultsById.get(document.id) ?? null}
                retryingIndex={retryingIndexIds.has(document.id)}
                onSelect={(event) => onSelectDocument(document.id, event)}
                onStartDocumentDrag={(event) =>
                  onStartDocumentDrag(document, event)
                }
                onMove={(collectionId) =>
                  onMoveDocument(document, collectionId)
                }
                onEdit={() => onEditDocument(document)}
                onMoveToTrash={() => onMoveDocumentToTrash(document)}
                onRetryIndex={() => onRetryIndex(document)}
              />
            ))}
          <div
            role="presentation"
            aria-hidden="true"
            style={{
              height:
                virtual.totalHeight -
                virtual.topOffset -
                (virtual.afterLastIndex - virtual.firstIndex) *
                  LIST_ROW_HEIGHT
            }}
          />
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
  selectedDocumentIds,
  draggingDocumentIds,
  selectionDisabled,
  highlightedDocumentId,
  searchResults,
  retryingIndexIds,
  onSelectDocument,
  onStartDocumentDrag,
  onMoveDocumentToTrash,
  onRetryIndex
}: DocumentResultsProps & { client: BackendClient }) {
  const virtual = useVirtualDocuments(
    documents,
    highlightedDocumentId,
    "grid"
  );
  const searchResultsById = useMemo(
    () => new Map(searchResults.map((result) => [result.document.id, result])),
    [searchResults]
  );
  const collectionsById = useMemo(
    () => new Map(collections.map((collection) => [collection.id, collection])),
    [collections]
  );

  return (
    <main className="document-area" aria-label="文档网格">
      <div
        className="document-grid"
        ref={virtual.scrollRef}
        role="list"
        aria-label="文档结果"
        tabIndex={0}
        onScroll={virtual.onScroll}
        onKeyDown={virtual.onKeyDown}
        style={{ display: "block" }}
      >
        <div style={{ height: virtual.totalHeight, position: "relative" }}>
          <div
            style={{
              position: "absolute",
              top: virtual.topOffset,
              left: 0,
              right: 0,
              display: "grid",
              gridTemplateColumns: `repeat(${virtual.columns}, minmax(0, 1fr))`,
              gap: GRID_GAP
            }}
          >
            {documents
              .slice(virtual.firstIndex, virtual.afterLastIndex)
              .map((document, windowIndex) => {
                const status = documentStatusPresentation(document);
                const collection = collectionsById.get(document.collectionId);
                const selected = selectedDocumentIds.has(document.id);
                const highlighted = document.id === highlightedDocumentId;
                const searchResult =
                  searchResultsById.get(document.id) ?? null;
                const resultCopy =
                  searchResult?.snippet && searchResult.matchKind === "content"
                    ? searchResult.snippet
                    : document.fileName;

                return (
                  <article
                    id={`document-card-${document.id}`}
                    data-result-index={virtual.firstIndex + windowIndex}
                    className={`document-grid-item${selected ? " selected" : ""}${
                      highlighted ? " highlighted" : ""
                    }${
                      draggingDocumentIds.has(document.id) ? " dragging" : ""
                    }`}
                    role="listitem"
                    aria-setsize={documents.length}
                    aria-posinset={virtual.firstIndex + windowIndex + 1}
                    aria-current={highlighted ? "true" : undefined}
                    key={document.id}
                  >
                    <button
                      className="document-grid-select"
                      type="button"
                      onClick={(event) => onSelectDocument(document.id, event)}
                      onPointerDown={(event) =>
                        onStartDocumentDrag(document, event)
                      }
                      disabled={selectionDisabled}
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
        </div>
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
  if (documentUsesLocalImage(document)) {
    return ImageIcon;
  }
  if (documentUsesPptxPreview(document)) {
    return Presentation;
  }
  return FileText;
}

export function isImageDocument(document: DocumentSummary) {
  return documentUsesLocalImage(document);
}

export function isPdfDocument(document: DocumentSummary) {
  return documentUsesPdfPreview(document);
}
