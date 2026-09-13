import { RotateCcw } from "lucide-react";

import type { CollectionSummary, TagSummary } from "../backend/types";

interface SearchFiltersProps {
  collections: CollectionSummary[];
  tags: TagSummary[];
  fileTypes: string[];
  collectionId: string | null;
  tagId: string | null;
  fileType: string | null;
  documentDateFrom: string | null;
  documentDateTo: string | null;
  hasActiveFilters: boolean;
  onChange: (change: {
    collectionId?: string | null;
    tagId?: string | null;
    fileType?: string | null;
    documentDateFrom?: string | null;
    documentDateTo?: string | null;
  }) => void;
  onClear: () => void;
}

export function SearchFilters({
  collections,
  tags,
  fileTypes,
  collectionId,
  tagId,
  fileType,
  documentDateFrom,
  documentDateTo,
  hasActiveFilters,
  onChange,
  onClear
}: SearchFiltersProps) {
  return (
    <section className="search-filter-panel" aria-label="搜索筛选">
      <label className="filter-field">
        <span>集合</span>
        <select
          aria-label="集合筛选"
          value={collectionId ?? ""}
          onChange={(event) =>
            onChange({ collectionId: event.target.value || null })
          }
        >
          <option value="">全部集合</option>
          {collections.map((collection) => (
            <option key={collection.id} value={collection.id}>
              {collection.name}
            </option>
          ))}
        </select>
      </label>

      <label className="filter-field">
        <span>标签</span>
        <select
          aria-label="标签筛选"
          value={tagId ?? ""}
          onChange={(event) => onChange({ tagId: event.target.value || null })}
        >
          <option value="">全部标签</option>
          {tags.map((tag) => (
            <option key={tag.id} value={tag.id}>
              {tag.name}
            </option>
          ))}
        </select>
      </label>

      <label className="filter-field">
        <span>文件类型</span>
        <select
          aria-label="文件类型"
          value={fileType ?? ""}
          onChange={(event) =>
            onChange({ fileType: event.target.value || null })
          }
        >
          <option value="">全部类型</option>
          {fileTypes.map((type) => (
            <option key={type} value={type}>
              {type}
            </option>
          ))}
        </select>
      </label>

      <label className="filter-field date-filter">
        <span>文档日期从</span>
        <input
          type="date"
          aria-label="文档日期从"
          value={documentDateFrom ?? ""}
          onChange={(event) =>
            onChange({ documentDateFrom: event.target.value || null })
          }
        />
      </label>

      <label className="filter-field date-filter">
        <span>文档日期至</span>
        <input
          type="date"
          aria-label="文档日期至"
          value={documentDateTo ?? ""}
          onChange={(event) =>
            onChange({ documentDateTo: event.target.value || null })
          }
        />
      </label>

      <button
        className="button quiet clear-filters"
        type="button"
        onClick={onClear}
        disabled={!hasActiveFilters}
      >
        <RotateCcw size={15} aria-hidden="true" />
        清除筛选
      </button>
    </section>
  );
}
