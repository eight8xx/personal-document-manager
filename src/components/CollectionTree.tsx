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
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type {
  CSSProperties,
  MouseEvent as ReactMouseEvent
} from "react";
import { createPortal } from "react-dom";

import type { CollectionSummary } from "../backend/types";

const MENU_GAP = 4;
const MENU_VIEWPORT_MARGIN = 8;

interface CollectionMenuPosition {
  top: number;
  left: number;
  placement: "top" | "bottom";
}

function clamp(value: number, minimum: number, maximum: number) {
  return Math.min(Math.max(value, minimum), Math.max(minimum, maximum));
}

interface CollectionTreeProps {
  collections: CollectionSummary[];
  selectedCollectionId: string | null;
  dragPayloadCount: number;
  dropTargetCollectionId: string | null;
  dropTargetRejected: boolean;
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
  dragPayloadCount,
  dropTargetCollectionId,
  dropTargetRejected,
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
  const isDropTarget = dropTargetCollectionId === collection.id;
  const isRejectedDropTarget = isDropTarget && dropTargetRejected;
  const menuContainerRef = useRef<HTMLDivElement>(null);
  const menuPopoverRef = useRef<HTMLDivElement>(null);
  const menuTriggerRef = useRef<HTMLButtonElement>(null);
  const [menuPosition, setMenuPosition] =
    useState<CollectionMenuPosition | null>(null);

  useEffect(() => {
    if (!isMenuOpen) {
      return;
    }

    function handlePointerDown(event: MouseEvent) {
      const target = event.target as Node;
      if (
        !menuContainerRef.current?.contains(target) &&
        !menuPopoverRef.current?.contains(target)
      ) {
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

  useLayoutEffect(() => {
    if (!isMenuOpen) {
      setMenuPosition(null);
      return;
    }

    function updateMenuPosition() {
      const trigger = menuTriggerRef.current;
      const menu = menuPopoverRef.current;
      if (!trigger || !menu) {
        return;
      }

      const triggerRect = trigger.getBoundingClientRect();
      const menuRect = menu.getBoundingClientRect();
      const viewportWidth =
        document.documentElement.clientWidth || window.innerWidth;
      const viewportHeight =
        document.documentElement.clientHeight || window.innerHeight;
      if (
        triggerRect.bottom < 0 ||
        triggerRect.top > viewportHeight ||
        triggerRect.right < 0 ||
        triggerRect.left > viewportWidth
      ) {
        onOpenMenuChange(null);
        return;
      }
      const spaceBelow =
        viewportHeight -
        MENU_VIEWPORT_MARGIN -
        triggerRect.bottom -
        MENU_GAP;
      const spaceAbove =
        triggerRect.top - MENU_VIEWPORT_MARGIN - MENU_GAP;
      const opensAbove =
        menuRect.height > spaceBelow && spaceAbove > spaceBelow;
      const preferredTop = opensAbove
        ? triggerRect.top - MENU_GAP - menuRect.height
        : triggerRect.bottom + MENU_GAP;
      const maximumTop =
        viewportHeight - MENU_VIEWPORT_MARGIN - menuRect.height;
      const maximumLeft =
        viewportWidth - MENU_VIEWPORT_MARGIN - menuRect.width;

      setMenuPosition({
        top: Math.round(
          clamp(preferredTop, MENU_VIEWPORT_MARGIN, maximumTop)
        ),
        left: Math.round(
          clamp(
            triggerRect.right - menuRect.width,
            MENU_VIEWPORT_MARGIN,
            maximumLeft
          )
        ),
        placement: opensAbove ? "top" : "bottom"
      });
    }

    updateMenuPosition();
    window.addEventListener("resize", updateMenuPosition);
    window.addEventListener("scroll", updateMenuPosition, true);
    return () => {
      window.removeEventListener("resize", updateMenuPosition);
      window.removeEventListener("scroll", updateMenuPosition, true);
    };
  }, [isMenuOpen]);

  useEffect(() => {
    if (!isDropTarget || expanded || !hasChildren) {
      return;
    }

    const timeout = window.setTimeout(() => {
      setExpanded(true);
    }, 500);
    return () => window.clearTimeout(timeout);
  }, [expanded, hasChildren, isDropTarget]);

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

  const menuPopoverStyle: CSSProperties = {
    top: menuPosition?.top ?? 0,
    left: menuPosition?.left ?? 0,
    visibility: menuPosition ? "visible" : "hidden"
  };

  return (
    <li className="collection-node">
      <div
        className={`collection-row${isSelected ? " active" : ""}${
          isDropTarget ? " drop-target" : ""
        }${isRejectedDropTarget ? " drop-rejected" : ""}`}
        data-collection-id={collection.id}
        data-drop-target={isDropTarget ? "true" : undefined}
        data-drop-rejected={isRejectedDropTarget ? "true" : undefined}
        data-drag-payload-count={
          isDropTarget && dragPayloadCount > 0
            ? dragPayloadCount
            : undefined
        }
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

        <div className="collection-menu" ref={menuContainerRef}>
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
          {isMenuOpen
            ? createPortal(
                <div
                  ref={menuPopoverRef}
                  className="collection-menu-popover"
                  role="menu"
                  aria-label={`管理 ${collection.name}`}
                  data-placement={menuPosition?.placement}
                  style={menuPopoverStyle}
                >
                  <button
                    type="button"
                    role="menuitem"
                    onClick={(event) =>
                      chooseMenuAction(event, () =>
                        onCreateChild(collection)
                      )
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
                          chooseMenuAction(event, () =>
                            onRename(collection)
                          )
                        }
                      >
                        <Pencil size={14} aria-hidden="true" />
                        重命名
                      </button>
                      <button
                        type="button"
                        role="menuitem"
                        onClick={(event) =>
                          chooseMenuAction(event, () =>
                            onMove(collection)
                          )
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
                          chooseMenuAction(event, () =>
                            onDelete(collection)
                          )
                        }
                      >
                        <Trash2 size={14} aria-hidden="true" />
                        删除
                      </button>
                    </>
                  ) : null}
                </div>,
                document.body
              )
            : null}
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
                dragPayloadCount,
                dropTargetCollectionId,
                dropTargetRejected,
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
