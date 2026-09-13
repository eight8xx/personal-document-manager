import {
  AlertCircle,
  FilePlus2,
  Folder,
  FolderOpen,
  FolderPlus,
  Inbox,
  LayoutGrid,
  LibraryBig,
  List,
  LoaderCircle,
  Pencil,
  Plus,
  Search,
  Settings,
  Tag as TagIcon,
  Trash2,
  X
} from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { toBackendError } from "../backend/error";
import type {
  BackendClient,
  CollectionDeleteResult,
  CollectionSummary,
  DocumentMetadataUpdate,
  DocumentSummary,
  ImportBatch,
  ImportDecision,
  ImportItemResult,
  ImportProgress,
  LibrarySummary,
  TagSummary
} from "../backend/types";
import {
  CollectionActionDialog,
  DeleteCollectionDialog
} from "./CollectionDialog";
import type { CollectionAction } from "./CollectionDialog";
import { CollectionTree } from "./CollectionTree";
import { DocumentDetails } from "./DocumentDetails";
import {
  DocumentEmptyState
} from "./DocumentEmptyState";
import type { DocumentEmptyStateKind } from "./DocumentEmptyState";
import { DocumentMetadataDialog } from "./DocumentMetadataDialog";
import { DocumentGrid, DocumentList } from "./DocumentResults";
import { ImportBatchPanel } from "./ImportBatchPanel";
import { ImportDecisionDialog } from "./ImportDecisionDialog";
import {
  DeleteTagDialog,
  TagActionDialog
} from "./TagDialog";
import type { TagAction } from "./TagDialog";

interface LibraryWorkspaceProps {
  client: BackendClient;
  library: LibrarySummary;
  onOpenSettings: () => void;
}

interface ImportRun {
  batchId: string | null;
  batch: ImportBatch | null;
  progress: ImportProgress | null;
  items: ImportItemResult[];
}

type DocumentView = "list" | "grid";

const DOCUMENT_VIEW_STORAGE_KEY = "personal-document-manager.document-view";

function storedDocumentView(): DocumentView {
  try {
    return window.sessionStorage.getItem(DOCUMENT_VIEW_STORAGE_KEY) ===
      "grid"
      ? "grid"
      : "list";
  } catch {
    return "list";
  }
}

function rememberDocumentView(view: DocumentView) {
  try {
    window.sessionStorage.setItem(DOCUMENT_VIEW_STORAGE_KEY, view);
  } catch {
    // Session storage is an enhancement; the in-memory selection still works.
  }
}

function mergeById<T>(
  current: T[],
  incoming: T[],
  getId: (item: T) => string
): T[] {
  const incomingIds = new Set(incoming.map(getId));
  return [
    ...incoming,
    ...current.filter((item) => !incomingIds.has(getId(item)))
  ];
}

const documentId = (document: DocumentSummary) => document.id;
const collectionId = (collection: CollectionSummary) => collection.id;
const tagId = (tag: TagSummary) => tag.id;
const importItemId = (item: ImportItemResult) => item.itemId;

function replaceImportItem(
  items: ImportItemResult[],
  nextItem: ImportItemResult
): ImportItemResult[] {
  return items.map((item) =>
    item.itemId === nextItem.itemId ? nextItem : item
  );
}

function progressFromBatch(batch: ImportBatch): ImportProgress {
  const lastItem = batch.items.at(-1) ?? null;
  return {
    batchId: batch.batchId,
    total: batch.items.length,
    completed: batch.items.length,
    currentFileName: lastItem?.fileName ?? null,
    currentSourcePath: lastItem?.sourcePath ?? null,
    item: lastItem,
    finished: true
  };
}

export function LibraryWorkspace({
  client,
  library,
  onOpenSettings
}: LibraryWorkspaceProps) {
  const [documents, setDocuments] = useState<DocumentSummary[]>([]);
  const [collections, setCollections] = useState<CollectionSummary[]>([]);
  const [tags, setTags] = useState<TagSummary[]>([]);
  const [selectedCollectionId, setSelectedCollectionId] = useState<
    string | null
  >(null);
  const [selectedTagId, setSelectedTagId] = useState<string | null>(null);
  const [selectedDocumentId, setSelectedDocumentId] = useState<string | null>(
    null
  );
  const [documentView, setDocumentView] =
    useState<DocumentView>(storedDocumentView);
  const [loading, setLoading] = useState(true);
  const [importing, setImporting] = useState(false);
  const [importRun, setImportRun] = useState<ImportRun | null>(null);
  const [retryingItemIds, setRetryingItemIds] = useState<Set<string>>(
    new Set()
  );
  const [activeDecisionItemId, setActiveDecisionItemId] = useState<
    string | null
  >(null);
  const [collectionAction, setCollectionAction] =
    useState<CollectionAction | null>(null);
  const [deleteTarget, setDeleteTarget] =
    useState<CollectionSummary | null>(null);
  const [tagAction, setTagAction] = useState<TagAction | null>(null);
  const [deleteTagTarget, setDeleteTagTarget] = useState<TagSummary | null>(
    null
  );
  const [metadataTarget, setMetadataTarget] =
    useState<DocumentSummary | null>(null);
  const [highlightedDocumentId, setHighlightedDocumentId] = useState<
    string | null
  >(null);
  const [error, setError] = useState("");

  const refreshDocuments = useCallback(async () => {
    const items = await client.listDocuments();
    setDocuments((current) => mergeById(current, items, documentId));
    return items;
  }, [client]);

  const refreshCollections = useCallback(async () => {
    const items = await client.listCollections();
    setCollections(items);
    return items;
  }, [client]);

  const refreshTags = useCallback(async () => {
    const items = await client.listTags();
    setTags(items);
    return items;
  }, [client]);

  const refreshLibraryData = useCallback(async () => {
    await Promise.all([
      refreshDocuments(),
      refreshCollections(),
      refreshTags()
    ]);
  }, [refreshCollections, refreshDocuments, refreshTags]);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setDocuments([]);
    setCollections([]);
    setTags([]);
    setError("");
    setSelectedCollectionId(null);
    setSelectedTagId(null);
    setSelectedDocumentId(null);

    void Promise.all([
      client.listDocuments(),
      client.listCollections(),
      client.listTags()
    ])
      .then(([documentItems, collectionItems, tagItems]) => {
        if (!active) {
          return;
        }
        setDocuments((current) => mergeById(current, documentItems, documentId));
        setCollections((current) =>
          mergeById(current, collectionItems, collectionId)
        );
        setTags((current) => mergeById(current, tagItems, tagId));
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

  const importPaths = useCallback(
    async (paths: string[]) => {
      const uniquePaths = [...new Set(paths.filter(Boolean))];
      if (uniquePaths.length === 0) {
        return;
      }

      setImporting(true);
      setError("");
      setHighlightedDocumentId(null);
      setActiveDecisionItemId(null);
      setImportRun({
        batchId: null,
        batch: null,
        progress: null,
        items: []
      });

      try {
        const batch = await client.startImport(uniquePaths);
        setImportRun((current) => {
          if (current?.batchId && current.batchId !== batch.batchId) {
            return current;
          }
          const currentItems =
            current?.batchId === batch.batchId ? current.items : [];
          return {
            batchId: batch.batchId,
            batch,
            progress:
              current?.progress?.batchId === batch.batchId
                ? current.progress
                : progressFromBatch(batch),
            items: mergeById(currentItems, batch.items, importItemId)
          };
        });
        await refreshLibraryData();
      } catch (caught) {
        setError(toBackendError(caught).message);
      } finally {
        setImporting(false);
      }
    },
    [client, refreshLibraryData]
  );

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;

    void client
      .subscribeToFileDrops((paths) => {
        void importPaths(paths);
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
  }, [client, importPaths]);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;

    void client
      .subscribeToImportProgress((progress) => {
        setImportRun((current) => {
          if (!current) {
            return current;
          }
          if (current.batchId && current.batchId !== progress.batchId) {
            return current;
          }
          return {
            ...current,
            batchId: progress.batchId,
            progress,
            items: progress.item
              ? mergeById(current.items, [progress.item], importItemId)
              : current.items
          };
        });
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
  }, [client]);

  useEffect(() => {
    const pendingItem = importRun?.items.find(
      (item) => item.status === "duplicate" || item.status === "sourceChanged"
    );
    if (!pendingItem) {
      setActiveDecisionItemId(null);
      return;
    }
    if (activeDecisionItemId === pendingItem.itemId) {
      return;
    }
    setActiveDecisionItemId(pendingItem.itemId);
  }, [activeDecisionItemId, importRun]);

  async function chooseDocuments() {
    setError("");
    try {
      const paths = await client.pickDocumentFiles();
      await importPaths(paths);
    } catch (caught) {
      setError(toBackendError(caught).message);
    }
  }

  async function chooseFolder() {
    setError("");
    try {
      const path = await client.pickDocumentFolder();
      if (path) {
        await importPaths([path]);
      }
    } catch (caught) {
      setError(toBackendError(caught).message);
    }
  }

  async function resolveDecision(decision: ImportDecision) {
    if (!activeDecisionItemId) {
      return;
    }

    setError("");
    const updated = await client.resolveImportItem(
      activeDecisionItemId,
      decision
    );
    setImportRun((current) => {
      if (!current) {
        return current;
      }
      return {
        ...current,
        items: replaceImportItem(current.items, updated)
      };
    });
    setActiveDecisionItemId(null);

      if (decision === "useExisting") {
        setSelectedCollectionId(null);
        setSelectedTagId(null);
        const existingDocumentId =
          updated.duplicateDocumentId ?? updated.documentId;
        if (existingDocumentId) {
          setSelectedDocumentId(existingDocumentId);
          setHighlightedDocumentId(existingDocumentId);
        window.setTimeout(() => {
          document
            .getElementById(`document-row-${existingDocumentId}`)
            ?.scrollIntoView?.({ block: "center" });
        }, 0);
      }
    } else if (decision !== "cancel") {
      await refreshLibraryData();
    }
  }

  async function retryItem(item: ImportItemResult) {
    setRetryingItemIds((current) => new Set(current).add(item.itemId));
    setError("");
    try {
      const updated = await client.retryImportItem(item.itemId);
      setImportRun((current) =>
        current
          ? {
              ...current,
              items: replaceImportItem(current.items, updated)
            }
          : current
      );
      await refreshLibraryData();
    } catch (caught) {
      setError(toBackendError(caught).message);
    } finally {
      setRetryingItemIds((current) => {
        const next = new Set(current);
        next.delete(item.itemId);
        return next;
      });
    }
  }

  async function submitCollectionAction(value: string | null) {
    if (!collectionAction) {
      return;
    }

    setError("");
    try {
      if (collectionAction.type === "create") {
        const created = await client.createCollection(
          value ?? "",
          collectionAction.parent?.id ?? null
        );
        await refreshCollections();
        setSelectedCollectionId(created.id);
      } else if (collectionAction.type === "rename") {
        await client.renameCollection(
          collectionAction.collection.id,
          value ?? ""
        );
        await refreshCollections();
      } else {
        await client.moveCollection(collectionAction.collection.id, value);
        await refreshCollections();
      }
      setCollectionAction(null);
    } catch (caught) {
      setError(toBackendError(caught).message);
      throw caught;
    }
  }

  async function deleteCollection() {
    if (!deleteTarget) {
      return;
    }

    setError("");
    try {
      const result: CollectionDeleteResult = await client.deleteCollection(
        deleteTarget.id
      );
      await refreshLibraryData();
      if (selectedCollectionId === result.collectionId) {
        setSelectedCollectionId(result.targetCollectionId);
      }
      setDeleteTarget(null);
    } catch (caught) {
      setError(toBackendError(caught).message);
      throw caught;
    }
  }

  async function submitTagAction(name: string) {
    if (!tagAction) {
      return;
    }

    setError("");
    if (tagAction.type === "create") {
      const created = await client.createTag(name);
      setTags((current) =>
        [...current.filter((tag) => tag.id !== created.id), created].sort(
          (left, right) => left.name.localeCompare(right.name, "zh-CN")
        )
      );
    } else {
      const renamed = await client.renameTag(tagAction.tag.id, name);
      setTags((current) =>
        current
          .map((tag) => (tag.id === renamed.id ? renamed : tag))
          .sort((left, right) => left.name.localeCompare(right.name, "zh-CN"))
      );
      setDocuments((current) =>
        current.map((document) => ({
          ...document,
          tags: document.tags.map((tag) =>
            tag.id === renamed.id ? renamed : tag
          )
        }))
      );
    }
    setTagAction(null);
  }

  async function deleteTag() {
    if (!deleteTagTarget) {
      return;
    }

    setError("");
    const target = deleteTagTarget;
    await client.deleteTag(target.id);
    setTags((current) => current.filter((tag) => tag.id !== target.id));
    setDocuments((current) =>
      current.map((document) => ({
        ...document,
        tags: document.tags.filter((tag) => tag.id !== target.id)
      }))
    );
    if (selectedTagId === target.id) {
      setSelectedTagId(null);
      setSelectedDocumentId(null);
    }
    setDeleteTagTarget(null);
  }

  async function saveDocumentMetadata(update: DocumentMetadataUpdate) {
    if (!metadataTarget) {
      return;
    }

    setError("");
    const updated = await client.updateDocumentMetadata(
      metadataTarget.id,
      update
    );
    setDocuments((current) =>
      current.map((document) =>
        document.id === updated.id ? updated : document
      )
    );
    setMetadataTarget(null);
    void refreshCollections().catch((caught) => {
      setError(toBackendError(caught).message);
    });
    void refreshTags().catch((caught) => {
      setError(toBackendError(caught).message);
    });
  }

  async function moveDocument(document: DocumentSummary, collectionId: string) {
    if (document.collectionId === collectionId) {
      return;
    }

    setError("");
    try {
      const moved = await client.moveDocumentToCollection(
        document.id,
        collectionId
      );
      setDocuments((current) =>
        current.map((item) => (item.id === moved.id ? moved : item))
      );
      if (
        selectedCollectionId &&
        selectedCollectionId !== moved.collectionId
      ) {
        setSelectedDocumentId(null);
      }
      await refreshCollections();
    } catch (caught) {
      setError(toBackendError(caught).message);
      await refreshDocuments();
    }
  }

  const selectedCollection = selectedCollectionId
    ? collections.find(
        (collection) => collection.id === selectedCollectionId
      ) ?? null
    : null;
  const selectedTag = selectedTagId
    ? tags.find((tag) => tag.id === selectedTagId) ?? null
    : null;
  const visibleDocuments = selectedCollectionId
    ? documents.filter(
        (document) => document.collectionId === selectedCollectionId
      )
    : selectedTagId
      ? documents.filter((document) =>
          document.tags.some((tag) => tag.id === selectedTagId)
        )
      : documents;
  const selectedDocument = selectedDocumentId
    ? visibleDocuments.find(
        (document) => document.id === selectedDocumentId
      ) ?? null
    : null;
  const emptyStateKind: DocumentEmptyStateKind | null =
    documents.length === 0
      ? "library"
      : visibleDocuments.length === 0
        ? selectedTagId
          ? "tag"
          : "collection"
        : null;
  const decisionItem =
    importRun?.items.find(
      (item) => item.itemId === activeDecisionItemId
    ) ?? null;

  function selectAllDocuments() {
    setSelectedCollectionId(null);
    setSelectedTagId(null);
    setSelectedDocumentId(null);
  }

  function selectCollection(collectionId: string) {
    setSelectedCollectionId(collectionId);
    setSelectedTagId(null);
    setSelectedDocumentId(null);
  }

  function selectTag(tagId: string) {
    setSelectedTagId(tagId);
    setSelectedCollectionId(null);
    setSelectedDocumentId(null);
  }

  function changeDocumentView(view: DocumentView) {
    setDocumentView(view);
    rememberDocumentView(view);
  }

  return (
    <div className="app-shell">
      <aside className="sidebar" aria-label="集合与标签">
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
          <button
            className={`nav-item${
              selectedCollectionId === null && selectedTagId === null
                ? " active"
                : ""
            }`}
            type="button"
            onClick={selectAllDocuments}
          >
            <LibraryBig size={18} aria-hidden="true" />
            <span>全部文档</span>
            <em>{documents.length}</em>
          </button>
        </nav>

        <section
          className="collection-section"
          aria-labelledby="collections-title"
        >
          <div className="collection-section-header">
            <h2 id="collections-title">集合</h2>
            <button
              className="icon-button compact"
              type="button"
              onClick={() =>
                setCollectionAction({ type: "create", parent: null })
              }
              aria-label="创建根集合"
              title="创建根集合"
            >
              <FolderPlus size={15} aria-hidden="true" />
            </button>
          </div>
          <CollectionTree
            collections={collections}
            selectedCollectionId={selectedCollectionId}
            onSelect={selectCollection}
            onCreateChild={(collection) =>
              setCollectionAction({ type: "create", parent: collection })
            }
            onRename={(collection) =>
              setCollectionAction({ type: "rename", collection })
            }
            onMove={(collection) =>
              setCollectionAction({ type: "move", collection })
            }
            onDelete={setDeleteTarget}
          />
        </section>

        <section className="tag-section" aria-labelledby="tags-title">
          <div className="tag-section-header">
            <h2 id="tags-title">标签</h2>
            <button
              className="icon-button compact"
              type="button"
              onClick={() => setTagAction({ type: "create" })}
              aria-label="创建标签"
              title="创建标签"
            >
              <Plus size={15} aria-hidden="true" />
            </button>
          </div>
          {tags.length === 0 ? (
            <p className="tag-section-empty">暂无标签</p>
          ) : (
            <ul className="tag-list">
              {tags.map((tag) => (
                <li
                  className={`tag-row${
                    selectedTagId === tag.id ? " active" : ""
                  }`}
                  key={tag.id}
                >
                  <button
                    className="tag-select"
                    type="button"
                    onClick={() => selectTag(tag.id)}
                    aria-current={
                      selectedTagId === tag.id ? "page" : undefined
                    }
                    aria-label={`按标签 ${tag.name} 筛选`}
                  >
                    <TagIcon size={14} aria-hidden="true" />
                    <span title={tag.name}>{tag.name}</span>
                    <em>{tag.documentCount}</em>
                  </button>
                  <div className="tag-actions">
                    <button
                      className="icon-button compact"
                      type="button"
                      onClick={() => setTagAction({ type: "rename", tag })}
                      aria-label={`重命名标签 ${tag.name}`}
                      title="重命名标签"
                    >
                      <Pencil size={13} aria-hidden="true" />
                    </button>
                    <button
                      className="icon-button compact danger"
                      type="button"
                      onClick={() => setDeleteTagTarget(tag)}
                      aria-label={`删除标签 ${tag.name}`}
                      title="删除标签"
                    >
                      <Trash2 size={13} aria-hidden="true" />
                    </button>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </section>

        <div className="sidebar-spacer" />

        <button className="nav-item" type="button" onClick={onOpenSettings}>
          <Settings size={18} aria-hidden="true" />
          <span>设置</span>
        </button>
      </aside>

      <section className="workspace">
        <header className="workspace-header">
          <div className="library-heading">
            {selectedTag ? (
              <TagIcon size={19} aria-hidden="true" />
            ) : selectedCollection?.isInbox ? (
              <Inbox size={19} aria-hidden="true" />
            ) : selectedCollection ? (
              <Folder size={19} aria-hidden="true" />
            ) : (
              <FolderOpen size={19} aria-hidden="true" />
            )}
            <div>
              <strong>
                {selectedTag?.name ?? selectedCollection?.name ?? "全部文档"}
              </strong>
              <span>{visibleDocuments.length} 份文档</span>
            </div>
          </div>
          <div className="header-actions">
            <div className="search-placeholder" aria-hidden="true">
              <Search size={17} />
              <span>搜索文档</span>
            </div>
            <button
              className="button secondary import-button"
              type="button"
              onClick={() => void chooseFolder()}
              disabled={loading || importing}
            >
              <FolderPlus size={17} aria-hidden="true" />
              导入文件夹
            </button>
            <button
              className="button primary import-button"
              type="button"
              onClick={() => void chooseDocuments()}
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
              aria-label="关闭错误提示"
              title="关闭"
            >
              <X size={16} aria-hidden="true" />
            </button>
          </div>
        ) : null}

        {importRun ? (
          <ImportBatchPanel
            batch={importRun.batch}
            progress={importRun.progress}
            items={importRun.items}
            retryingItemIds={retryingItemIds}
            onRetry={(item) => void retryItem(item)}
            onClose={() => setImportRun(null)}
          />
        ) : null}

        {loading ? (
          <main className="workspace-loading" aria-label="正在加载文档">
            <LoaderCircle className="spin" size={22} aria-hidden="true" />
          </main>
        ) : (
          <>
            {emptyStateKind ? (
              <DocumentEmptyState
                kind={emptyStateKind}
                importing={importing}
                onImport={() => void chooseDocuments()}
              />
            ) : (
              <>
                <div className="document-view-toolbar">
                  <span>
                    {documentView === "list" ? "列表视图" : "网格视图"}
                  </span>
                  <div
                    className="view-switcher"
                    role="group"
                    aria-label="视图切换"
                  >
                    <button
                      className="view-switcher-button"
                      type="button"
                      onClick={() => changeDocumentView("list")}
                      aria-pressed={documentView === "list"}
                      aria-label="列表视图"
                      title="列表视图"
                    >
                      <List size={16} aria-hidden="true" />
                    </button>
                    <button
                      className="view-switcher-button"
                      type="button"
                      onClick={() => changeDocumentView("grid")}
                      aria-pressed={documentView === "grid"}
                      aria-label="网格视图"
                      title="网格视图"
                    >
                      <LayoutGrid size={16} aria-hidden="true" />
                    </button>
                  </div>
                </div>
                {documentView === "list" ? (
                  <DocumentList
                    documents={visibleDocuments}
                    collections={collections}
                    selectedDocumentId={selectedDocumentId}
                    highlightedDocumentId={highlightedDocumentId}
                    onSelectDocument={setSelectedDocumentId}
                    onMoveDocument={(document, targetCollectionId) =>
                      void moveDocument(document, targetCollectionId)
                    }
                    onEditDocument={setMetadataTarget}
                  />
                ) : (
                  <DocumentGrid
                    client={client}
                    documents={visibleDocuments}
                    collections={collections}
                    selectedDocumentId={selectedDocumentId}
                    highlightedDocumentId={highlightedDocumentId}
                    onSelectDocument={setSelectedDocumentId}
                    onMoveDocument={(document, targetCollectionId) =>
                      void moveDocument(document, targetCollectionId)
                    }
                    onEditDocument={setMetadataTarget}
                  />
                )}
              </>
            )}
          </>
        )}
      </section>

      <DocumentDetails
        client={client}
        document={selectedDocument}
        collections={collections}
        onEditDocument={setMetadataTarget}
      />

      {collectionAction ? (
        <CollectionActionDialog
          action={collectionAction}
          collections={collections}
          onClose={() => setCollectionAction(null)}
          onSubmit={submitCollectionAction}
        />
      ) : null}

      {deleteTarget ? (
        <DeleteCollectionDialog
          collection={deleteTarget}
          onClose={() => setDeleteTarget(null)}
          onConfirm={deleteCollection}
        />
      ) : null}

      {tagAction ? (
        <TagActionDialog
          action={tagAction}
          onClose={() => setTagAction(null)}
          onSubmit={submitTagAction}
        />
      ) : null}

      {deleteTagTarget ? (
        <DeleteTagDialog
          tag={deleteTagTarget}
          onClose={() => setDeleteTagTarget(null)}
          onConfirm={deleteTag}
        />
      ) : null}

      {metadataTarget ? (
        <DocumentMetadataDialog
          document={metadataTarget}
          collections={collections}
          tags={tags}
          onClose={() => setMetadataTarget(null)}
          onSave={saveDocumentMetadata}
        />
      ) : null}

      {decisionItem ? (
        <ImportDecisionDialog
          item={decisionItem}
          documents={documents}
          onResolve={resolveDecision}
        />
      ) : null}
    </div>
  );
}
