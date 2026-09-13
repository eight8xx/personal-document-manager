import {
  Archive,
  FileText,
  FolderOpen,
  Inbox,
  LibraryBig,
  Search,
  Settings,
  Tag
} from "lucide-react";

import type { LibrarySummary } from "../backend/types";

interface LibraryWorkspaceProps {
  library: LibrarySummary;
  onOpenSettings: () => void;
}

export function LibraryWorkspace({
  library,
  onOpenSettings
}: LibraryWorkspaceProps) {
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
              <span>0 份文档</span>
            </div>
          </div>
          <div className="header-actions">
            <div className="search-placeholder" aria-hidden="true">
              <Search size={17} />
              <span>搜索文档</span>
            </div>
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

        <main className="empty-library">
          <div className="empty-illustration" aria-hidden="true">
            <FileText />
          </div>
          <h1>空资料库</h1>
          <p>资料库已经准备好，可以开始添加文档。</p>
          <button
            className="button primary"
            type="button"
            disabled
            title="单文件导入将在下一任务中启用"
          >
            <FileText size={18} aria-hidden="true" />
            导入文档
          </button>
        </main>
      </section>
    </div>
  );
}
