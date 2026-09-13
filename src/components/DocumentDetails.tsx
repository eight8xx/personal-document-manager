import {
  CalendarDays,
  FileText,
  Folder,
  Pencil,
  Tags
} from "lucide-react";

import type {
  CollectionSummary,
  DocumentSummary
} from "../backend/types";
import {
  documentStatusPresentation,
  documentTypeIcon,
  findCollectionName
} from "./DocumentResults";

interface DocumentDetailsProps {
  document: DocumentSummary | null;
  collections: CollectionSummary[];
  onEditDocument: (document: DocumentSummary) => void;
}

export function DocumentDetails({
  document,
  collections,
  onEditDocument
}: DocumentDetailsProps) {
  const TypeIcon = document ? documentTypeIcon(document) : FileText;
  const status = document ? documentStatusPresentation(document) : null;

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
            <button
              className="button secondary details-edit"
              type="button"
              onClick={() => onEditDocument(document)}
              aria-label={`在详情中编辑 ${document.title} 的元数据`}
            >
              <Pencil size={15} aria-hidden="true" />
              编辑元数据
            </button>
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
              <dd>
                {findCollectionName(collections, document.collectionId)}
              </dd>
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
            <div>
              <FileText size={22} aria-hidden="true" />
              <span>暂无预览</span>
            </div>
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
