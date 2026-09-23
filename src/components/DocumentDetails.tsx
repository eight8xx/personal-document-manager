import {
  AlertCircle,
  CalendarDays,
  ChevronLeft,
  ChevronRight,
  ExternalLink,
  FileText,
  Folder,
  LoaderCircle,
  Pencil,
  RotateCcw,
  Tags,
  Trash2
} from "lucide-react";
import { useContext, useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";

import { toBackendError } from "../backend/error";
import { LibraryContext } from "../backend/libraryContext";
import { safeExternalUrl } from "../backend/url";
import type {
  BackendClient,
  CollectionSummary,
  DocumentPreview,
  DocumentSummary
} from "../backend/types";
import { DocxPreview } from "./DocxPreview";
import { PptxPreview } from "./PptxPreview";
import {
  documentStatusPresentation,
  documentTypeIcon,
  findCollectionName
} from "./DocumentResults";

interface DocumentDetailsProps {
  client: BackendClient;
  document: DocumentSummary | null;
  collections: CollectionSummary[];
  retryingIndex: boolean;
  onEditDocument: (document: DocumentSummary) => void;
  onMoveDocumentToTrash: (document: DocumentSummary) => void;
  onRetryIndex: (document: DocumentSummary) => void;
}

function MarkdownPreview({
  text,
  client
}: {
  text: string;
  client: BackendClient;
}) {
  const [linkError, setLinkError] = useState("");
  const pattern = /(!?)\[([^\]\n]+)\]\((https?:\/\/[^\s)]+)\)/g;
  const parts: ReactNode[] = [];
  let cursor = 0;
  let match: RegExpExecArray | null;

  while ((match = pattern.exec(text)) !== null) {
    if (match.index > cursor) {
      parts.push(text.slice(cursor, match.index));
    }
    const [, imageMarker, label, rawUrl] = match;
    const url = safeExternalUrl(rawUrl);
    if (imageMarker || !url) {
      parts.push(match[0]);
    } else {
      parts.push(
        <button
          className="markdown-link"
          type="button"
          key={`${match.index}:${url}`}
          onClick={() => {
            setLinkError("");
            void client.openExternalUrl(url).catch((caught) => {
              setLinkError(toBackendError(caught).message);
            });
          }}
        >
          {label}
        </button>
      );
    }
    cursor = match.index + match[0].length;
  }
  if (cursor < text.length) {
    parts.push(text.slice(cursor));
  }

  return (
    <div className="text-preview markdown-preview">
      <pre tabIndex={0}>{parts}</pre>
      {linkError ? (
        <p className="preview-inline-error" role="alert">
          <AlertCircle size={14} aria-hidden="true" />
          {linkError}
        </p>
      ) : null}
    </div>
  );
}

function PreviewContent({
  client,
  preview,
  document,
  page,
  onPageChange,
  onRetry
}: {
  client: BackendClient;
  preview: DocumentPreview;
  document: DocumentSummary;
  page: number;
  onPageChange: (page: number) => void;
  onRetry: () => void;
}) {
  if (preview.kind === "pdf") {
    const hasNextPage = preview.pageCount === null || page < preview.pageCount;
    return (
      <div className="pdf-preview">
        <div className="preview-toolbar">
          <span>
            第 {page} 页
            {preview.pageCount ? ` / ${preview.pageCount}` : ""}
          </span>
          <div>
            <button
              className="icon-button compact"
              type="button"
              onClick={() => onPageChange(page - 1)}
              disabled={page <= 1}
              aria-label="PDF 上一页"
              title="上一页"
            >
              <ChevronLeft size={15} aria-hidden="true" />
            </button>
            <button
              className="icon-button compact"
              type="button"
              onClick={() => onPageChange(page + 1)}
              disabled={!hasNextPage}
              aria-label="PDF 下一页"
              title="下一页"
            >
              <ChevronRight size={15} aria-hidden="true" />
            </button>
          </div>
        </div>
        <img
          className="pdf-preview-page"
          src={preview.dataUrl}
          alt={`${document.title} 第 ${page} 页预览`}
          data-preview-kind="pdf-page"
        />
      </div>
    );
  }

  if (preview.kind === "image") {
    return (
      <div className="image-preview">
        <img src={preview.dataUrl} alt={`${document.title} 预览`} />
      </div>
    );
  }

  if (preview.kind === "docx") {
    return (
      <DocxPreview
        client={client}
        document={document}
        preview={preview}
      />
    );
  }

  if (preview.kind === "pptx") {
    return (
      <PptxPreview
        client={client}
        document={document}
        preview={preview}
      />
    );
  }

  if (preview.kind === "text") {
    return (
      <div className="text-preview">
        <pre tabIndex={0}>{preview.text}</pre>
      </div>
    );
  }

  if (preview.kind === "markdown") {
    return <MarkdownPreview text={preview.text} client={client} />;
  }

  if (preview.kind === "failure") {
    return (
      <div className="preview-error" role="alert">
        <AlertCircle size={22} aria-hidden="true" />
        <strong>无法加载预览</strong>
        <span>{preview.message}</span>
        <button className="button quiet" type="button" onClick={onRetry}>
          重试预览
        </button>
      </div>
    );
  }

  return (
    <div className="preview-fallback">
      <FileText size={24} aria-hidden="true" />
      <span>{preview.message}</span>
    </div>
  );
}

export function DocumentDetails({
  client,
  document,
  collections,
  retryingIndex,
  onEditDocument,
  onMoveDocumentToTrash,
  onRetryIndex
}: DocumentDetailsProps) {
  const library = useContext(LibraryContext);
  const TypeIcon = document ? documentTypeIcon(document) : FileText;
  const status = document ? documentStatusPresentation(document) : null;
  const documentId = document?.id ?? null;
  const documentContentHash = document?.contentHash ?? null;
  const documentFileSize = document?.fileSize ?? null;
  const documentLastImportedAt = document?.lastImportedAt ?? null;
  const previewKey = documentId
    ? `${documentId}:${documentContentHash}:${documentFileSize}:${documentLastImportedAt}`
    : "";
  const previousPreviewKey = useRef(previewKey);
  const [preview, setPreview] = useState<DocumentPreview | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [previewError, setPreviewError] = useState("");
  const [reloadToken, setReloadToken] = useState(0);
  const [openError, setOpenError] = useState("");
  const [opening, setOpening] = useState(false);
  const [pdfPage, setPdfPage] = useState(1);

  useEffect(() => {
    let active = true;
    const contentChanged = previousPreviewKey.current !== previewKey;
    previousPreviewKey.current = previewKey;
    const requestedPage = contentChanged ? 1 : pdfPage;
    if (contentChanged) {
      setPreview(null);
      setPdfPage(1);
      setOpenError("");
    }
    setPreviewError("");

    if (!documentId || !library) {
      setPreviewLoading(false);
      return () => {
        active = false;
      };
    }

    setPreviewLoading(true);
    void client
      .getDocumentPreview(library, documentId, requestedPage)
      .then((result) => {
        if (active) {
          setPreview(result);
        }
      })
      .catch((caught) => {
        if (active) {
          setPreviewError(toBackendError(caught).message);
        }
      })
      .finally(() => {
        if (active) {
          setPreviewLoading(false);
        }
      });

    return () => {
      active = false;
    };
  }, [
    client,
    library,
    documentId,
    previewKey,
    pdfPage,
    reloadToken
  ]);

  async function openDocument() {
    if (!documentId || !library) {
      return;
    }

    setOpening(true);
    setOpenError("");
    try {
      await client.openDocument(library, documentId);
    } catch (caught) {
      setOpenError(toBackendError(caught).message);
    } finally {
      setOpening(false);
    }
  }

  return (
    <aside className="details-panel" aria-label="文档详情">
      <header className="details-panel-header">
        <p className="eyebrow">当前文档</p>
        <h2>文档详情</h2>
      </header>

      {document ? (
        <div className="details-content">
          <section className="details-summary" aria-labelledby="detail-title">
            <div className="details-title-row">
              <span className="details-type-icon" aria-hidden="true">
                <TypeIcon size={21} />
              </span>
              <div>
                <h3 id="detail-title" title={document.title}>
                  <span className="visually-hidden">当前文档：</span>
                  {document.title}
                </h3>
                <p title={document.fileName}>{document.fileName}</p>
              </div>
            </div>
            <div className="details-actions">
              <button
                className="button secondary details-edit"
                type="button"
                onClick={() => onEditDocument(document)}
                aria-label={`在详情中编辑 ${document.title} 的元数据`}
              >
                <Pencil size={15} aria-hidden="true" />
                编辑元数据
              </button>
              <button
                className="button secondary"
                type="button"
                onClick={() => void openDocument()}
                disabled={opening}
                aria-label={`用系统默认程序打开 ${document.title}`}
              >
                {opening ? (
                  <LoaderCircle className="spin" size={15} aria-hidden="true" />
                ) : (
                  <ExternalLink size={15} aria-hidden="true" />
                )}
                外部打开
              </button>
              <button
                className="button danger-quiet details-trash"
                type="button"
                onClick={() => onMoveDocumentToTrash(document)}
                aria-label={`将 ${document.title} 移入回收站`}
              >
                <Trash2 size={15} aria-hidden="true" />
                移入回收站
              </button>
            </div>
            {openError ? (
              <p className="details-inline-error" role="alert">
                <AlertCircle size={14} aria-hidden="true" />
                {openError}
              </p>
            ) : null}
          </section>

          <dl className="details-metadata">
            <div>
              <dt>
                <FileText size={14} aria-hidden="true" />
                类型
              </dt>
              <dd>{document.fileType}</dd>
            </div>
            <div>
              <dt>
                <CalendarDays size={14} aria-hidden="true" />
                文档日期
              </dt>
              <dd className={document.documentDate ? "" : "muted"}>
                {document.documentDate ?? "未设置"}
              </dd>
            </div>
            <div>
              <dt>
                <Folder size={14} aria-hidden="true" />
                集合
              </dt>
              <dd>{findCollectionName(collections, document.collectionId)}</dd>
            </div>
            <div>
              <dt>
                <Tags size={14} aria-hidden="true" />
                标签
              </dt>
              <dd>
                {document.tags.length > 0 ? (
                  <span className="details-tags">
                    {document.tags.map((tag) => (
                      <span key={tag.id}>{tag.name}</span>
                    ))}
                  </span>
                ) : (
                  <span className="muted">无标签</span>
                )}
              </dd>
            </div>
          </dl>

          <section className="details-status" aria-labelledby="status-title">
            <h3 id="status-title">处理状态</h3>
            <div className="details-status-actions">
              <span
                className={`status-badge ${status?.tone ?? "neutral"}`}
                title={document.errorMessage ?? undefined}
              >
                {status?.label}
              </span>
              {document.indexStatus === "failed" ? (
                <button
                  className="button quiet"
                  type="button"
                  onClick={() => onRetryIndex(document)}
                  disabled={retryingIndex}
                  aria-label={`重试索引 ${document.title}`}
                >
                  {retryingIndex ? (
                    <LoaderCircle
                      className="spin"
                      size={14}
                      aria-hidden="true"
                    />
                  ) : (
                    <RotateCcw size={14} aria-hidden="true" />
                  )}
                  重试索引
                </button>
              ) : null}
            </div>
            {document.errorMessage ? (
              <p className="details-status-message" role="status">
                {document.errorMessage}
              </p>
            ) : null}
          </section>

          <section className="detail-preview" aria-label="文档预览">
            {previewLoading ? (
              <div className="preview-loading" role="status">
                <LoaderCircle className="spin" size={22} aria-hidden="true" />
                <span>正在加载预览</span>
              </div>
            ) : previewError ? (
              <div className="preview-error" role="alert">
                <AlertCircle size={22} aria-hidden="true" />
                <strong>无法加载预览</strong>
                <span>{previewError}</span>
                <button
                  className="button quiet"
                  type="button"
                  onClick={() => setReloadToken((value) => value + 1)}
                >
                  重试预览
                </button>
              </div>
            ) : preview ? (
              <PreviewContent
                client={client}
                preview={preview}
                document={document}
                page={pdfPage}
                onPageChange={(page) => setPdfPage(Math.max(1, page))}
                onRetry={() => setReloadToken((value) => value + 1)}
              />
            ) : (
              <div className="preview-fallback">
                <FileText size={22} aria-hidden="true" />
                <span>暂无预览</span>
              </div>
            )}
          </section>
        </div>
      ) : (
        <div className="details-empty">
          <FileText size={25} aria-hidden="true" />
          <h3>选择文档查看详情</h3>
          <p>详情和预览将在这里显示。</p>
        </div>
      )}
    </aside>
  );
}
