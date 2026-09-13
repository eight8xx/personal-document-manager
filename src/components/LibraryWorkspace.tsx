import {
  AlertCircle,
  Archive,
  FilePlus2,
  FileText,
  FolderOpen,
  Inbox,
  LibraryBig,
  LoaderCircle,
  Search,
  Settings,
  Tag,
  X
} from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { toBackendError } from "../backend/error";
import type {
  BackendClient,
  DocumentSummary,
  LibrarySummary
} from "../backend/types";

interface LibraryWorkspaceProps {
  client: BackendClient;
  library: LibrarySummary;
  onOpenSettings: () => void;
}

interface StatusPresentation {
  label: string;
  tone: "neutral" | "success" | "warning" | "danger";
}

function statusPresentation(document: DocumentSummary): StatusPresentation {
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

export function LibraryWorkspace({
  client,
  library,
  onOpenSettings
}: LibraryWorkspaceProps) {
  const [documents, setDocuments] = useState<DocumentSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [importing, setImporting] = useState(false);
  const [error, setError] = useState("");

  const importPath = useCallback(
    async (path: string) => {
      setImporting(true);
      setError("");

      try {
        const imported = await client.importDocument(path);
        setDocuments((current) => [
          imported,
          ...current.filter((item) => item.id !== imported.id)
        ]);
      } catch (caught) {
        setError(toBackendError(caught).message);
      } finally {
        setImporting(false);
      }
    },
    [client]
  );

  useEffect(() => {
    let active = true;
    setLoading(true);
    setDocuments([]);
    setError("");

    void client
      .listDocuments()
      .then((items) => {
        if (active) {
          setDocuments((current) => {
            const currentIds = new Set(current.map((item) => item.id));
            return [
              ...current,
              ...items.filter((item) => !currentIds.has(item.id))
            ];
          });
        }
      })
      .catch((caught) => {
        if (active) {
          setError(toBackendError(caught).message);
        }
      })
      .finally(() => {
        if (active) {
          setLoading(false);
        }
      });

    return () => {
      active = false;
    };
  }, [client, library.id]);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;

    void client
      .subscribeToFileDrops((paths) => {
        if (paths.length !== 1) {
          setError("一次只能导入一个文件。");
          return;
        }
        void importPath(paths[0]);
      })
      .then((stopListening) => {
        if (active) {
          unlisten = stopListening;
        } else {
          stopListening();
        }
      })
      .catch((caught) => {
        if (active) {
          setError(toBackendError(caught).message);
        }
      });

    return () => {
      active = false;
      unlisten?.();
    };
  }, [client, importPath]);

  async function chooseDocument() {
    setError("");
    try {
      const path = await client.pickDocumentFile();
      if (path) {
        await importPath(path);
      }
    } catch (caught) {
      setError(toBackendError(caught).message);
    }
  }

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark" aria-hidden="true">
            <span>文</span>
          </div>
          <div>
            <strong>个人文档</strong>
            <span>{library.name}</span>
          </div>
        </div>

        <nav className="primary-nav" aria-label="资料库导航">
          <button className="nav-item active" type="button">
            <LibraryBig size={18} aria-hidden="true" />
            <span>文档</span>
          </button>
          <button className="nav-item" type="button">
            <Inbox size={18} aria-hidden="true" />
            <span>收件箱</span>
          </button>
          <button className="nav-item" type="button">
            <Tag size={18} aria-hidden="true" />
            <span>标签</span>
          </button>
          <button className="nav-item" type="button">
            <Archive size={18} aria-hidden="true" />
            <span>回收站</span>
          </button>
        </nav>

        <div className="sidebar-spacer" />

        <button className="nav-item" type="button" onClick={onOpenSettings}>
          <Settings size={18} aria-hidden="true" />
          <span>设置</span>
        </button>
      </aside>

      <section className="workspace">
        <header className="workspace-header">
          <div className="library-heading">
            <FolderOpen size={19} aria-hidden="true" />
            <div>
              <strong>全部文档</strong>
              <span>{documents.length} 份文档</span>
            </div>
          </div>
          <div className="header-actions">
            <div className="search-placeholder" aria-hidden="true">
              <Search size={17} />
              <span>搜索文档</span>
            </div>
            <button
              className="button primary import-button"
              type="button"
              onClick={() => void chooseDocument()}
              disabled={loading || importing}
            >
              {importing ? (
                <LoaderCircle className="spin" size={17} aria-hidden="true" />
              ) : (
                <FilePlus2 size={17} aria-hidden="true" />
              )}
              {importing ? "正在导入" : "导入文档"}
            </button>
            <button
              className="icon-button"
              type="button"
              onClick={onOpenSettings}
              aria-label="打开设置"
              title="设置"
            >
              <Settings size={19} aria-hidden="true" />
            </button>
          </div>
        </header>

        {error ? (
          <div className="workspace-message" role="alert">
            <AlertCircle size={17} aria-hidden="true" />
            <span>{error}</span>
            <button
              className="icon-button"
              type="button"
              onClick={() => setError("")}
              aria-label="关闭导入错误"
              title="关闭"
            >
              <X size={16} aria-hidden="true" />
            </button>
          </div>
        ) : null}

        {loading ? (
          <main className="workspace-loading" aria-label="正在加载文档">
            <LoaderCircle className="spin" size={22} aria-hidden="true" />
          </main>
        ) : documents.length === 0 ? (
          <main className="empty-library" aria-label="空资料库">
            <div className="empty-illustration" aria-hidden="true">
              <FileText />
            </div>
            <h1>空资料库</h1>
            <p>资料库已经准备好，可以开始添加文档。</p>
            <button
              className="button primary"
              type="button"
              onClick={() => void chooseDocument()}
              disabled={importing}
            >
              {importing ? (
                <LoaderCircle className="spin" size={18} aria-hidden="true" />
              ) : (
                <FilePlus2 size={18} aria-hidden="true" />
              )}
              导入文档
            </button>
          </main>
        ) : (
          <main className="document-area" aria-label="文档列表">
            <div className="document-list-header" aria-hidden="true">
              <span>标题</span>
              <span>类型</span>
              <span>状态</span>
            </div>
            <div className="document-list">
              {documents.map((document) => {
                const status = statusPresentation(document);
                return (
                  <article className="document-row" key={document.id}>
                    <div className="document-title-cell">
                      <FileText size={18} aria-hidden="true" />
                      <div>
                        <strong>{document.title}</strong>
                        <span>{document.fileName}</span>
                      </div>
                    </div>
                    <span className="document-type">{document.fileType}</span>
                    <span
                      className={`status-badge ${status.tone}`}
                      title={document.errorMessage ?? undefined}
                    >
                      {status.label}
                    </span>
                  </article>
                );
              })}
            </div>
          </main>
        )}
      </section>
    </div>
  );
}
