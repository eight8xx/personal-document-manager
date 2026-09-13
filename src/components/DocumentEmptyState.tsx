import {
  FilePlus2,
  FileSearch,
  Folder,
  Inbox,
  LoaderCircle,
  Tag as TagIcon
} from "lucide-react";

export type DocumentEmptyStateKind =
  | "library"
  | "collection"
  | "tag"
  | "search";

interface DocumentEmptyStateProps {
  kind: DocumentEmptyStateKind;
  importing?: boolean;
  onImport?: () => void;
}

const emptyStateContent: Record<
  Exclude<DocumentEmptyStateKind, "library">,
  { label: string; title: string; description: string }
> = {
  collection: {
    label: "空集合",
    title: "这个集合中没有文档",
    description: "可以将文档移动到该集合，或直接导入新文档。"
  },
  tag: {
    label: "空标签",
    title: "这个标签下没有文档",
    description: "编辑文档元数据，为文档添加这个标签。"
  },
  search: {
    label: "无搜索结果",
    title: "没有找到匹配的文档",
    description: "请调整搜索词或筛选条件后重试。"
  }
};

export function DocumentEmptyState({
  kind,
  importing = false,
  onImport
}: DocumentEmptyStateProps) {
  if (kind === "library") {
    return (
      <main className="empty-library" aria-label="空资料库">
        <div className="empty-illustration" aria-hidden="true">
          <Inbox />
        </div>
        <h1>空资料库</h1>
        <p>资料库已经准备好，可以开始添加文档。</p>
        <button
          className="button primary"
          type="button"
          onClick={onImport}
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
    );
  }

  const content = emptyStateContent[kind];
  const EmptyIcon =
    kind === "collection" ? Folder : kind === "tag" ? TagIcon : FileSearch;

  return (
    <main className="empty-library compact" aria-label={content.label}>
      <div className="empty-illustration" aria-hidden="true">
        <EmptyIcon />
      </div>
      <h1>{content.title}</h1>
      <p>{content.description}</p>
    </main>
  );
}
