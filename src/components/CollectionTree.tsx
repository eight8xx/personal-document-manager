import {
  ChevronDown,
  ChevronRight,
  Folder,
  FolderInput,
  FolderPlus,
  Inbox,
  MoreHorizontal,
  Pencil,
  Trash2
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { MouseEvent as ReactMouseEvent } from "react";

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
  openMenuId: string | null;
  onOpenMenuChange: (collectionId: string | null) => void;
}

function CollectionNode({
  collection,
  collections,
  depth,
  openMenuId,
  onOpenMenuChange,
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
  const isMenuOpen = openMenuId === collection.id;
  const menuRef = useRef<HTMLDivElement>(null);
  const menuTriggerRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!isMenuOpen) {
      return;
    }

    function handlePointerDown(event: MouseEvent) {
      if (!menuRef.current?.contains(event.target as Node)) {
        onOpenMenuChange(null);
      }
    }

    function handleKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape") {
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      onOpenMenuChange(null);
      menuTriggerRef.current?.focus();
    }

    document.addEventListener("mousedown", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("mousedown", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [isMenuOpen, onOpenMenuChange]);

  function toggleMenu(event: ReactMouseEvent<HTMLButtonElement>) {
    event.preventDefault();
    event.stopPropagation();
    onOpenMenuChange(isMenuOpen ? null : collection.id);
  }

  function chooseMenuAction(
    event: ReactMouseEvent<HTMLButtonElement>,
    action: () => void
  ) {
    event.preventDefault();
    event.stopPropagation();
    onOpenMenuChange(null);
    action();
  }

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
            onClick={(event) => {
              event.preventDefault();
              event.stopPropagation();
              setExpanded((current) => !current);
            }}
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
          onClick={(event) => {
            event.preventDefault();
            event.stopPropagation();
            onSelect(collection.id);
          }}
          aria-current={isSelected ? "page" : undefined}
          title={collection.name}
        >
          <CollectionIcon size={16} aria-hidden="true" />
          <span className="collection-name">{collection.name}</span>
          <em>{collection.documentCount}</em>
        </button>

        <div className="collection-menu" ref={menuRef}>
          <button
            ref={menuTriggerRef}
            className="icon-button compact collection-menu-trigger"
            type="button"
            onClick={toggleMenu}
            aria-haspopup="menu"
            aria-expanded={isMenuOpen}
            aria-label={`管理 ${collection.name}`}
            title={`管理 ${collection.name}`}
          >
            <MoreHorizontal size={15} aria-hidden="true" />
          </button>
          {isMenuOpen ? (
            <div
              className="collection-menu-popover"
              role="menu"
              aria-label={`管理 ${collection.name}`}
            >
              <button
                type="button"
                role="menuitem"
                onClick={(event) =>
                  chooseMenuAction(event, () => onCreateChild(collection))
                }
              >
                <FolderPlus size={14} aria-hidden="true" />
                创建子集合
              </button>
              {!collection.isInbox ? (
                <>
                  <button
                    type="button"
                    role="menuitem"
                    onClick={(event) =>
                      chooseMenuAction(event, () => onRename(collection))
                    }
                  >
                    <Pencil size={14} aria-hidden="true" />
                    重命名
                  </button>
                  <button
                    type="button"
                    role="menuitem"
                    onClick={(event) =>
                      chooseMenuAction(event, () => onMove(collection))
                    }
                  >
                    <FolderInput size={14} aria-hidden="true" />
                    移动
                  </button>
                  <button
                    className="danger"
                    type="button"
                    role="menuitem"
                    onClick={(event) =>
                      chooseMenuAction(event, () => onDelete(collection))
                    }
                  >
                    <Trash2 size={14} aria-hidden="true" />
                    删除
                  </button>
                </>
              ) : null}
            </div>
          ) : null}
        </div>
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
                openMenuId,
                onOpenMenuChange,
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
  const [openMenuId, setOpenMenuId] = useState<string | null>(null);
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
          openMenuId={openMenuId}
          onOpenMenuChange={setOpenMenuId}
        />
      ))}
    </ul>
  );
}
