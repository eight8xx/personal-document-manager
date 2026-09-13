import {
  ChevronDown,
  ChevronRight,
  Folder,
  FolderInput,
  FolderPlus,
  Inbox,
  Pencil,
  Trash2
} from "lucide-react";
import { useState } from "react";

import type { CollectionSummary } from "../backend/types";

interface CollectionTreeProps {
  collections: CollectionSummary[];
  selectedCollectionId: string | null;
  onSelect: (collectionId: string) => void;
  onCreateChild: (collection: CollectionSummary) => void;
  onRename: (collection: CollectionSummary) => void;
  onMove: (collection: CollectionSummary) => void;
  onDelete: (collection: CollectionSummary) => void;
}

interface CollectionNodeProps extends CollectionTreeProps {
  collection: CollectionSummary;
  depth: number;
}

function CollectionNode({
  collection,
  collections,
  depth,
  selectedCollectionId,
  onSelect,
  onCreateChild,
  onRename,
  onMove,
  onDelete
}: CollectionNodeProps) {
  const [expanded, setExpanded] = useState(true);
  const children = collections.filter(
    (candidate) => candidate.parentId === collection.id
  );
  const hasChildren = children.length > 0;
  const isSelected = selectedCollectionId === collection.id;
  const CollectionIcon = collection.isInbox ? Inbox : Folder;

  return (
    <li className="collection-node">
      <div
        className={`collection-row${isSelected ? " active" : ""}`}
        style={{ paddingInlineStart: `${8 + depth * 15}px` }}
      >
        {hasChildren ? (
          <button
            className="collection-toggle"
            type="button"
            onClick={() => setExpanded((current) => !current)}
            aria-label={`${expanded ? "收起" : "展开"}${collection.name}`}
            title={expanded ? "收起" : "展开"}
          >
            {expanded ? (
              <ChevronDown size={14} aria-hidden="true" />
            ) : (
              <ChevronRight size={14} aria-hidden="true" />
            )}
          </button>
        ) : (
          <span className="collection-toggle-spacer" aria-hidden="true" />
        )}

        <button
          className="collection-select"
          type="button"
          onClick={() => onSelect(collection.id)}
          aria-current={isSelected ? "page" : undefined}
          title={collection.name}
        >
          <CollectionIcon size={16} aria-hidden="true" />
          <span className="collection-name">{collection.name}</span>
          <em>{collection.documentCount}</em>
        </button>

        {!collection.isInbox ? (
          <div className="collection-actions">
            <button
              className="icon-button compact"
              type="button"
              onClick={() => onCreateChild(collection)}
              aria-label={`在${collection.name}中创建子集合`}
              title="创建子集合"
            >
              <FolderPlus size={14} aria-hidden="true" />
            </button>
            <button
              className="icon-button compact"
              type="button"
              onClick={() => onRename(collection)}
              aria-label={`重命名${collection.name}`}
              title="重命名"
            >
              <Pencil size={14} aria-hidden="true" />
            </button>
            <button
              className="icon-button compact"
              type="button"
              onClick={() => onMove(collection)}
              aria-label={`移动${collection.name}`}
              title="移动到其它父集合"
            >
              <FolderInput size={14} aria-hidden="true" />
            </button>
            <button
              className="icon-button compact danger"
              type="button"
              onClick={() => onDelete(collection)}
              aria-label={`删除${collection.name}`}
              title="删除集合"
            >
              <Trash2 size={14} aria-hidden="true" />
            </button>
          </div>
        ) : null}
      </div>

      {hasChildren && expanded ? (
        <ul className="collection-children">
          {children.map((child) => (
            <CollectionNode
              key={child.id}
              {...{
                collection: child,
                collections,
                depth: depth + 1,
                selectedCollectionId,
                onSelect,
                onCreateChild,
                onRename,
                onMove,
                onDelete
              }}
            />
          ))}
        </ul>
      ) : null}
    </li>
  );
}

export function CollectionTree(props: CollectionTreeProps) {
  const roots = props.collections.filter(
    (collection) => collection.parentId === null
  );

  return (
    <ul className="collection-tree" aria-label="集合树">
      {roots.map((collection) => (
        <CollectionNode
          key={collection.id}
          {...props}
          collection={collection}
          depth={0}
        />
      ))}
    </ul>
  );
}
