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
  Tags
} from "lucide-react";
import { useEffect, useState } from "react";

import { toBackendError } from "../backend/error";
import type {
  BackendClient,
  CollectionSummary,
  DocumentPreview,
  DocumentSummary
} from "../backend/types";
import {
  documentStatusPresentation,
  documentTypeIcon,
  findCollectionName
} from "./DocumentResults";

interface DocumentDetailsProps {
  client: BackendClient;
  document: DocumentSummary | null;
  collections: CollectionSummary[];
  onEditDocument: (document: DocumentSummary) => void;
}

function pdfPreviewUrl(dataUrl: string, page: number) {
  return `${dataUrl.split("#", 1)[0]}#page=${page}&zoom=page-width&view=FitH`;
}

function PreviewContent({
  preview,
  document,
  page,
  onPageChange
}: {
  preview: DocumentPreview;
  document: DocumentSummary;
  page: number;
  onPageChange: (page: number) => void;
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
        <iframe
          className="pdf-preview-frame"
          title={`${document.title} PDF 预览`}
          src={pdfPreviewUrl(preview.dataUrl, page)}
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

  if (preview.kind === "text" || preview.kind === "docx") {
    return (
      <div className="text-preview">
        {preview.kind === "docx" ? (
          <p className="preview-notice">{preview.notice}</p>
        ) : null}
        <pre tabIndex={0}>{preview.text}</pre>
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
  onEditDocument
}: DocumentDetailsProps) {
  const TypeIcon = document ? documentTypeIcon(document) : FileText;
  const status = document ? documentStatusPresentation(document) : null;
  const documentId = document?.id ?? null;
  const [preview, setPreview] = useState<DocumentPreview | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [previewError, setPreviewError] = useState("");
  const [reloadToken, setReloadToken] = useState(0);
  const [openError, setOpenError] = useState("");
  const [opening, setOpening] = useState(false);
  const [pdfPage, setPdfPage] = useState(1);

  useEffect(() => {
    let active = true;
    setPreview(null);
    setPreviewError("");
    setOpenError("");
    setPdfPage(1);

    if (!documentId) {
      setPreviewLoading(false);
      return () => {
        active = false;
      };
    }

    setPreviewLoading(true);
    void client
      .getDocumentPreview(documentId)
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
  }, [client, documentId, reloadToken]);

  async function openDocument() {
    if (!documentId) {
      return;
    }

    setOpening(true);
    setOpenError("");
    try {
      await client.openDocument(documentId);
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
            <span
              className={`status-badge ${status?.tone ?? "neutral"}`}
              title={document.errorMessage ?? undefined}
            >
              {status?.label}
            </span>
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
                preview={preview}
                document={document}
                page={pdfPage}
                onPageChange={(page) => setPdfPage(Math.max(1, page))}
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
