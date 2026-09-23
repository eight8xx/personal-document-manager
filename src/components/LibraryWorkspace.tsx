import {
  AlertCircle,
  FilePlus2,
  Files,
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
  SlidersHorizontal,
  Tag as TagIcon,
  Trash2,
  X
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import type {
  MouseEvent as ReactMouseEvent,
  PointerEvent as ReactPointerEvent,
  RefObject
} from "react";

import { BackendError, toBackendError } from "../backend/error";
import { importableDocumentTypes } from "../backend/documentFormats";
import { sameLibraryIdentity } from "../backend/libraryIdentity";
import type {
  BackendClient,
  BatchDocumentOperation,
  BatchDocumentOperationResult,
  CollectionDeleteResult,
  CollectionSummary,
  DocumentIndexChangedEvent,
  DocumentMetadataUpdate,
  DocumentSearchFilters,
  DocumentSearchResponse,
  DocumentSummary,
  FileDropEvent,
  ImportBatch,
  ImportDecision,
  ImportItemResult,
  ImportProgress,
  ImportSource,
  LibrarySummary,
  TagSummary,
  TrashDocumentSummary
} from "../backend/types";
import {
  BatchActionMenu,
  BatchOperationDialog,
  BatchResultPanel,
  BatchRunningStatus
} from "./BatchOrganize";
import type { BatchOrganizeAction } from "./BatchOrganize";
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
import { SearchFilters } from "./SearchFilters";
import {
  DeleteTagDialog,
  TagActionDialog
} from "./TagDialog";
import type { TagAction } from "./TagDialog";
import { TrashDocumentList } from "./TrashDocumentList";
import {
  EmptyTrashDialog,
  MoveDocumentToTrashDialog,
  PermanentDeleteDocumentDialog
} from "./TrashDialogs";

interface LibraryWorkspaceProps {
  client: BackendClient;
  library: LibrarySummary;
  showImportRestartNotice: boolean;
  settingsButtonRef: RefObject<HTMLButtonElement | null>;
  onUnfinishedImportChange: (
    owner: LibrarySummary,
    unfinished: boolean
  ) => void;
  onOpenSettings: () => void;
}

interface ImportRun {
  batchId: string | null;
  batch: ImportBatch | null;
  progress: ImportProgress | null;
  items: ImportItemResult[];
}

type DocumentView = "list" | "grid";
type DropHitKind = "collection" | "allDocuments" | "invalid";

interface DropHit {
  kind: DropHitKind;
  collectionId: string | null;
}

interface PendingDocumentDrag {
  document: DocumentSummary;
  pointerId: number;
  startX: number;
  startY: number;
  selected: boolean;
}

interface InternalDocumentDrag {
  documentIds: string[];
  pointerId: number;
  x: number;
  y: number;
  targetCollectionId: string | null;
  rejected: boolean;
}

interface ExternalFileDrag {
  targetCollectionId: string | null;
  count: number;
}

const DOCUMENT_VIEW_STORAGE_KEY = "personal-document-manager.document-view";
const DRAG_START_DISTANCE = 6;

function pointerIdFrom(event: ReactPointerEvent<HTMLElement>) {
  return Number.isFinite(event.pointerId) ? event.pointerId : 0;
}

function pointerCoordinate(value: number, fallback: number) {
  return Number.isFinite(value) ? value : fallback;
}

function dropHitFromElement(element: Element | null): DropHit {
  if (element?.closest("[data-drop-kind='all-documents']")) {
    return { kind: "allDocuments", collectionId: null };
  }
  const collection = element?.closest<HTMLElement>("[data-collection-id]");
  if (collection?.dataset.collectionId) {
    return {
      kind: "collection",
      collectionId: collection.dataset.collectionId
    };
  }
  return { kind: "invalid", collectionId: null };
}

function dropHitFromPointerEvent(
  event: ReactPointerEvent<HTMLElement>
): DropHit {
  return dropHitFromElement(
    event.target instanceof Element ? event.target : null
  );
}

function dropHitFromPosition(
  position: FileDropEvent["position"]
): DropHit {
  if (!position || !document.elementFromPoint) {
    return { kind: "invalid", collectionId: null };
  }
  try {
    return dropHitFromElement(
      document.elementFromPoint(position.x, position.y)
    );
  } catch {
    return { kind: "invalid", collectionId: null };
  }
}

function createBatchJobId() {
  return (
    globalThis.crypto?.randomUUID?.() ??
    `batch-${Date.now()}-${Math.random().toString(16).slice(2)}`
  );
}

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

function progressFromBatch(
  batch: ImportBatch,
  library: LibrarySummary
): ImportProgress {
  const lastItem = batch.items.at(-1) ?? null;
  return {
    library,
    batchId: batch.batchId,
    total: batch.items.length,
    completed: batch.items.length,
    currentFileName: lastItem?.fileName ?? null,
    currentSourcePath: lastItem?.sourcePath ?? null,
    item: lastItem,
    finished: true
  };
}

function documentMatchesFilters(
  document: DocumentSummary,
  filters: DocumentSearchFilters
) {
  if (
    filters.collectionId &&
    document.collectionId !== filters.collectionId
  ) {
    return false;
  }
  if (
    filters.tagId &&
    !document.tags.some((tag) => tag.id === filters.tagId)
  ) {
    return false;
  }
  if (
    filters.fileType &&
    document.fileType.toUpperCase() !== filters.fileType.toUpperCase()
  ) {
    return false;
  }
  if (
    filters.documentDateFrom &&
    (!document.documentDate ||
      document.documentDate < filters.documentDateFrom)
  ) {
    return false;
  }
  if (
    filters.documentDateTo &&
    (!document.documentDate ||
      document.documentDate > filters.documentDateTo)
  ) {
    return false;
  }
  return true;
}

export function LibraryWorkspace({
  client,
  library,
  showImportRestartNotice,
  settingsButtonRef,
  onUnfinishedImportChange,
  onOpenSettings
}: LibraryWorkspaceProps) {
  const [documents, setDocuments] = useState<DocumentSummary[]>([]);
  const [trashDocuments, setTrashDocuments] = useState<
    TrashDocumentSummary[]
  >([]);
  const [collections, setCollections] = useState<CollectionSummary[]>([]);
  const [tags, setTags] = useState<TagSummary[]>([]);
  const [searchQuery, setSearchQuery] = useState("");
  const [fileTypeFilter, setFileTypeFilter] = useState<string | null>(null);
  const [documentDateFrom, setDocumentDateFrom] = useState<string | null>(
    null
  );
  const [documentDateTo, setDocumentDateTo] = useState<string | null>(null);
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [searchResponse, setSearchResponse] =
    useState<DocumentSearchResponse | null>(null);
  const [searching, setSearching] = useState(false);
  const [searchError, setSearchError] = useState("");
  const [searchRevision, setSearchRevision] = useState(0);
  const [indexing, setIndexing] = useState(false);
  const [indexProgress, setIndexProgress] = useState<{
    processed: number;
    total: number;
  } | null>(null);
  const indexingRef = useRef(false);
  const [selectedCollectionId, setSelectedCollectionId] = useState<
    string | null
  >(null);
  const [selectedTagId, setSelectedTagId] = useState<string | null>(null);
  const [selectedDocumentId, setSelectedDocumentId] = useState<string | null>(
    null
  );
  const [selectedDocumentIds, setSelectedDocumentIds] = useState<Set<string>>(
    new Set()
  );
  const selectionAnchorIdRef = useRef<string | null>(null);
  const [showingTrash, setShowingTrash] = useState(false);
  const [documentView, setDocumentView] =
    useState<DocumentView>(storedDocumentView);
  const [loading, setLoading] = useState(true);
  const [importing, setImporting] = useState(false);
  const importingRef = useRef(false);
  const [importRun, setImportRun] = useState<ImportRun | null>(null);
  const [importRestartNotice, setImportRestartNotice] = useState(
    showImportRestartNotice
  );
  const [internalDocumentDrag, setInternalDocumentDrag] =
    useState<InternalDocumentDrag | null>(null);
  const pendingDocumentDragRef = useRef<PendingDocumentDrag | null>(null);
  const activeDocumentDragRef = useRef<InternalDocumentDrag | null>(null);
  const suppressDocumentClickRef = useRef(false);
  const [externalFileDrag, setExternalFileDrag] =
    useState<ExternalFileDrag | null>(null);
  const [retryingItemIds, setRetryingItemIds] = useState<Set<string>>(
    new Set()
  );
  const [retryingIndexIds, setRetryingIndexIds] = useState<Set<string>>(
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
  const [moveToTrashTarget, setMoveToTrashTarget] =
    useState<DocumentSummary | null>(null);
  const [permanentDeleteTarget, setPermanentDeleteTarget] =
    useState<DocumentSummary | null>(null);
  const [emptyTrashOpen, setEmptyTrashOpen] = useState(false);
  const [restoringTrashIds, setRestoringTrashIds] = useState<Set<string>>(
    new Set()
  );
  const [permanentlyDeletingIds, setPermanentlyDeletingIds] = useState<
    Set<string>
  >(new Set());
  const [highlightedDocumentId, setHighlightedDocumentId] = useState<
    string | null
  >(null);
  const [batchAction, setBatchAction] =
    useState<BatchOrganizeAction | null>(null);
  const [batchResult, setBatchResult] =
    useState<BatchDocumentOperationResult | null>(null);
  const [batchRunning, setBatchRunning] = useState<{
    jobId: string;
    count: number;
  } | null>(null);
  const batchRunningRef = useRef(false);
  const [cancellingBatch, setCancellingBatch] = useState(false);
  const [error, setError] = useState("");
  const hasUnfinishedImport =
    importing ||
    importRun?.items.some(
      (item) =>
        item.status === "duplicate" ||
        item.status === "sourceChanged" ||
        (item.status === "failed" && item.retryable)
    ) === true;

  useEffect(() => {
    onUnfinishedImportChange(library, hasUnfinishedImport);
  }, [hasUnfinishedImport, library, onUnfinishedImportChange]);

  const refreshDocuments = useCallback(async (replace = false) => {
    const items = await client.listDocuments();
    setDocuments((current) =>
      replace ? items : mergeById(current, items, documentId)
    );
    return items;
  }, [client]);

  const refreshTrashDocuments = useCallback(async () => {
    const items = await client.listTrashDocuments();
    setTrashDocuments(items);
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

  const refreshLibraryData = useCallback(async (replaceDocuments = false) => {
    await Promise.all([
      refreshDocuments(replaceDocuments),
      refreshTrashDocuments(),
      refreshCollections(),
      refreshTags()
    ]);
  }, [
    refreshCollections,
    refreshDocuments,
    refreshTags,
    refreshTrashDocuments
  ]);

  const runPendingIndexing = useCallback(async () => {
    if (indexingRef.current) {
      return;
    }
    indexingRef.current = true;
    try {
      const pendingCount = await client.pendingIndexCount();
      if (pendingCount <= 0) {
        return;
      }
      setDocuments((current) =>
        current.map((document) =>
          document.indexStatus === "pending" &&
          document.processingStatus === "ready"
            ? { ...document, processingStatus: "processing" }
            : document
        )
      );
      setIndexProgress({ processed: 0, total: pendingCount });
      setIndexing(true);
      const result = await client.indexPendingDocuments(library);
      if (result.processed > 0) {
        await refreshDocuments();
        setSearchRevision((value) => value + 1);
      }
    } catch (caught) {
      setError(toBackendError(caught).message);
    } finally {
      indexingRef.current = false;
      setIndexing(false);
      setIndexProgress(null);
    }
  }, [client, library, refreshDocuments]);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setDocuments([]);
    setTrashDocuments([]);
    setCollections([]);
    setTags([]);
    setError("");
    setSelectedCollectionId(null);
    setSelectedTagId(null);
    setSelectedDocumentId(null);
    setSelectedDocumentIds(new Set());
    setBatchResult(null);
    setShowingTrash(false);

    void Promise.all([
      client.listDocuments(),
      client.listTrashDocuments(),
      client.listCollections(),
      client.listTags()
    ])
      .then(([documentItems, trashItems, collectionItems, tagItems]) => {
        if (!active) {
          return;
        }
        setDocuments((current) => mergeById(current, documentItems, documentId));
        setTrashDocuments(trashItems);
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

  useEffect(() => {
    if (
      !loading &&
      documents.some(
        (document) =>
          document.processingStatus === "ready" &&
          document.indexStatus === "pending"
      )
    ) {
      void runPendingIndexing();
    }
  }, [documents, loading, runPendingIndexing]);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;

    void client
      .subscribeToDocumentIndexChanges((event: DocumentIndexChangedEvent) => {
        if (!active || !sameLibraryIdentity(event.library, library)) {
          return;
        }
        if (event.phase === "processing") {
          if (event.documentIds.length > 0) {
            const changedIds = new Set(event.documentIds);
            setDocuments((current) =>
              current.map((document) =>
                changedIds.has(document.id)
                  ? {
                      ...document,
                      processingStatus: "processing",
                      indexStatus: "pending"
                    }
                  : document
              )
            );
            setIndexing(true);
            setIndexProgress((current) =>
              current ?? {
                processed: 0,
                total: event.documentIds.length
              }
            );
          }
          if (event.result) {
            const processed = event.result.processed;
            setIndexProgress((current) => {
              const total =
                current && current.total > 0 ? current.total : processed;
              return { processed, total };
            });
            setDocuments((current) =>
              current.map((document) =>
                document.processingStatus === "ready" &&
                document.indexStatus === "pending"
                  ? { ...document, processingStatus: "processing" }
                  : document
              )
            );
          }
          return;
        }

        setIndexing(false);
        setIndexProgress(null);
        void refreshDocuments()
          .then(() => {
            if (active) {
              setSearchRevision((value) => value + 1);
            }
          })
          .catch((caught) => {
            if (active) {
              setError(toBackendError(caught).message);
            }
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
  }, [client, library, refreshDocuments]);

  useEffect(() => {
    const query = searchQuery.trim();
    if (!query) {
      setSearchResponse(null);
      setSearchError("");
      setSearching(false);
      return;
    }

    let active = true;
    setSearching(true);
    setSearchError("");
    const timeout = window.setTimeout(() => {
      void client
        .searchDocuments({
          query,
          filters: {
            collectionId: selectedCollectionId,
            tagId: selectedTagId,
            fileType: fileTypeFilter,
            documentDateFrom,
            documentDateTo
          }
        })
        .then((response) => {
          if (active) {
            setSearchResponse(response);
          }
        })
        .catch((caught) => {
          if (active) {
            setSearchError(toBackendError(caught).message);
          }
        })
        .finally(() => {
          if (active) {
            setSearching(false);
          }
        });
    }, 150);

    return () => {
      active = false;
      window.clearTimeout(timeout);
    };
  }, [
    client,
    documents,
    documentDateFrom,
    documentDateTo,
    fileTypeFilter,
    searchQuery,
    searchRevision,
    selectedCollectionId,
    selectedTagId
  ]);

  const importPaths = useCallback(
    async (
      paths: string[],
      targetCollectionId: string | null = null,
      source: ImportSource = "filePicker"
    ) => {
      const uniquePaths = [...new Set(paths.filter(Boolean))];
      if (uniquePaths.length === 0) {
        return;
      }
      if (importingRef.current) {
        setError(
          "已有导入批次正在运行，请等待完成后再拖入文件或文件夹。"
        );
        return;
      }

      importingRef.current = true;
      setImporting(true);
      setImportRestartNotice(false);
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
        const batch = await client.startImport(
          library,
          uniquePaths,
          targetCollectionId,
          source
        );
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
                : progressFromBatch(batch, library),
            items: mergeById(currentItems, batch.items, importItemId)
          };
        });
        await refreshLibraryData();
      } catch (caught) {
        setError(toBackendError(caught).message);
      } finally {
        importingRef.current = false;
        setImporting(false);
      }
    },
    [client, library, refreshLibraryData]
  );

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;

    void client
      .subscribeToFileDrops((event) => {
        if (event.type === "leave") {
          setExternalFileDrag(null);
          return;
        }

        const hit = dropHitFromPosition(event.position);
        if (event.type !== "drop") {
          setExternalFileDrag((current) => ({
            targetCollectionId: hit.collectionId,
            count:
              event.paths.length > 0
                ? event.paths.length
                : current?.count ?? 0
          }));
          return;
        }

        setExternalFileDrag(null);
        void importPaths(event.paths, hit.collectionId, "collectionDrop");
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
        if (!active || !sameLibraryIdentity(progress.library, library)) {
          return;
        }
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
  }, [client, library]);

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
      library,
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
      const updated = await client.retryImportItem(library, item.itemId);
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

  async function retryDocumentIndex(document: DocumentSummary) {
    setRetryingIndexIds((current) => new Set(current).add(document.id));
    setError("");
    try {
      const updated = await client.retryDocumentIndex(library, document.id);
      setDocuments((current) =>
        current.map((candidate) =>
          candidate.id === updated.id ? updated : candidate
        )
      );
      setSearchRevision((value) => value + 1);
    } catch (caught) {
      setError(toBackendError(caught).message);
    } finally {
      setRetryingIndexIds((current) => {
        const next = new Set(current);
        next.delete(document.id);
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
          library,
          value ?? "",
          collectionAction.parent?.id ?? null
        );
        await refreshCollections();
        setSelectedCollectionId(created.id);
      } else if (collectionAction.type === "rename") {
        await client.renameCollection(
          library,
          collectionAction.collection.id,
          value ?? ""
        );
        await refreshCollections();
      } else {
        await client.moveCollection(library, collectionAction.collection.id, value);
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
        library,
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
      const created = await client.createTag(library, name);
      setTags((current) =>
        [...current.filter((tag) => tag.id !== created.id), created].sort(
          (left, right) => left.name.localeCompare(right.name, "zh-CN")
        )
      );
    } else {
      const renamed = await client.renameTag(library, tagAction.tag.id, name);
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
    await client.deleteTag(library, target.id);
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
      library,
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
        library,
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

  async function moveDocumentToTrash() {
    if (!moveToTrashTarget) {
      return;
    }

    setError("");
    try {
      await client.moveDocumentToTrash(library, moveToTrashTarget.id);
      if (selectedDocumentId === moveToTrashTarget.id) {
        setSelectedDocumentId(null);
      }
      if (highlightedDocumentId === moveToTrashTarget.id) {
        setHighlightedDocumentId(null);
      }
      setDocuments((current) =>
        current.filter((document) => document.id !== moveToTrashTarget.id)
      );
      setMoveToTrashTarget(null);
      await refreshLibraryData();
    } catch (caught) {
      setError(toBackendError(caught).message);
      throw caught;
    }
  }

  async function restoreTrashDocument(document: DocumentSummary) {
    setRestoringTrashIds((current) => new Set(current).add(document.id));
    setError("");
    try {
      await client.restoreDocument(library, document.id);
      await refreshLibraryData();
    } catch (caught) {
      setError(toBackendError(caught).message);
    } finally {
      setRestoringTrashIds((current) => {
        const next = new Set(current);
        next.delete(document.id);
        return next;
      });
    }
  }

  async function permanentlyDeleteTrashDocument() {
    if (!permanentDeleteTarget) {
      return;
    }

    setPermanentlyDeletingIds((current) =>
      new Set(current).add(permanentDeleteTarget.id)
    );
    setError("");
    try {
      await client.permanentlyDeleteDocument(library, permanentDeleteTarget.id);
      setPermanentDeleteTarget(null);
      await refreshLibraryData();
    } catch (caught) {
      setError(toBackendError(caught).message);
      throw caught;
    } finally {
      setPermanentlyDeletingIds((current) => {
        const next = new Set(current);
        next.delete(permanentDeleteTarget.id);
        return next;
      });
    }
  }

  async function emptyTrash() {
    setError("");
    try {
      const result = await client.emptyTrash(library);
      await refreshLibraryData();
      if (result.failedCount > 0) {
        const failures = result.items
          .filter((item) => item.status === "failed")
          .slice(0, 3)
          .map(
            (item) =>
              `${item.fileName}：${item.errorMessage ?? "未知原因"}`
          )
          .join("；");
        throw new BackendError({
          code: "emptyTrashPartial",
          message:
            `已永久删除 ${result.deletedCount} 份，` +
            `${result.failedCount} 份失败：${failures}。` +
            "失败项仍保留在回收站，可以重试。"
        });
      }
      setEmptyTrashOpen(false);
    } catch (caught) {
      setError(toBackendError(caught).message);
      throw caught;
    }
  }

  async function runBatchOperation(
    operation: BatchDocumentOperation,
    documentIds: string[]
  ) {
    const uniqueDocumentIds = [...new Set(documentIds)];
    if (batchRunningRef.current || uniqueDocumentIds.length === 0) {
      return;
    }

    const jobId = createBatchJobId();
    batchRunningRef.current = true;
    setBatchRunning({ jobId, count: uniqueDocumentIds.length });
    setBatchResult(null);
    setCancellingBatch(false);
    setError("");

    try {
      const result = await client.batchOrganizeDocuments(library, {
        jobId,
        documentIds: uniqueDocumentIds,
        operation
      });
      setBatchResult(result);

      const failedIds = result.results
        .filter((item) => item.status === "failed")
        .map((item) => item.documentId);
      setSelectedDocumentIds(new Set(failedIds));

      if (
        operation.kind === "moveToTrash" &&
        selectedDocumentId &&
        result.results.some(
          (item) =>
            item.documentId === selectedDocumentId &&
            item.status === "succeeded"
        )
      ) {
        setSelectedDocumentId(null);
      }
      if (
        operation.kind === "moveToTrash" &&
        highlightedDocumentId &&
        result.results.some(
          (item) =>
            item.documentId === highlightedDocumentId &&
            item.status === "succeeded"
        )
      ) {
        setHighlightedDocumentId(null);
      }

      await refreshLibraryData(true);
      setSearchRevision((value) => value + 1);
    } catch (caught) {
      setError(toBackendError(caught).message);
    } finally {
      batchRunningRef.current = false;
      setBatchRunning(null);
      setCancellingBatch(false);
    }
  }

  function startBatchOperation(targetId: string | null) {
    if (!batchAction) {
      return;
    }

    const action = batchAction;
    const operation: BatchDocumentOperation =
      action === "moveToCollection"
        ? { kind: "moveToCollection", collectionId: targetId ?? "" }
        : action === "addTag"
          ? { kind: "addTag", tagId: targetId ?? "" }
          : action === "removeTag"
            ? { kind: "removeTag", tagId: targetId ?? "" }
            : { kind: "moveToTrash" };
    setBatchAction(null);
    void runBatchOperation(operation, [...selectedDocumentIds]);
  }

  async function cancelBatchOperation() {
    if (!batchRunning || cancellingBatch) {
      return;
    }

    setCancellingBatch(true);
    try {
      await client.cancelBatchDocumentOperation(batchRunning.jobId);
    } catch (caught) {
      setCancellingBatch(false);
      setError(toBackendError(caught).message);
    }
  }

  function retryFailedBatchOperation() {
    if (!batchResult) {
      return;
    }
    const failedIds = batchResult.results
      .filter((item) => item.status === "failed")
      .map((item) => item.documentId);
    void runBatchOperation(batchResult.operation, failedIds);
  }

  function startDocumentDrag(
    document: DocumentSummary,
    event: ReactPointerEvent<HTMLButtonElement>
  ) {
    if (
      batchRunningRef.current ||
      (event.button !== undefined && event.button !== 0)
    ) {
      return;
    }
    pendingDocumentDragRef.current = {
      document,
      pointerId: pointerIdFrom(event),
      startX: pointerCoordinate(event.clientX, 0),
      startY: pointerCoordinate(event.clientY, 0),
      selected: selectedDocumentIds.has(document.id)
    };
  }

  function updateDocumentDrag(event: ReactPointerEvent<HTMLElement>) {
    const pending = pendingDocumentDragRef.current;
    if (!pending || pending.pointerId !== pointerIdFrom(event)) {
      return;
    }

    let active = activeDocumentDragRef.current;
    if (!active) {
      const clientX = pointerCoordinate(
        event.clientX,
        pending.startX + DRAG_START_DISTANCE
      );
      const clientY = pointerCoordinate(
        event.clientY,
        pending.startY + DRAG_START_DISTANCE
      );
      const distance = Math.hypot(
        clientX - pending.startX,
        clientY - pending.startY
      );
      if (distance < DRAG_START_DISTANCE) {
        return;
      }

      const documentIds = pending.selected
        ? [...selectedDocumentIds]
        : [pending.document.id];
      if (!pending.selected) {
        selectionAnchorIdRef.current = pending.document.id;
        setSelectedDocumentIds(new Set(documentIds));
        setSelectedDocumentId(pending.document.id);
      }
      active = {
        documentIds,
        pointerId: pending.pointerId,
        x: clientX,
        y: clientY,
        targetCollectionId: null,
        rejected: true
      };
    }

    event.preventDefault();
    const hit = dropHitFromPointerEvent(event);
    const clientX = pointerCoordinate(event.clientX, active.x);
    const clientY = pointerCoordinate(event.clientY, active.y);
    const documentsById = new Map(
      documents.map((document) => [document.id, document])
    );
    const movableDocumentCount =
      hit.kind === "collection"
        ? active.documentIds.filter(
            (documentId) =>
              documentsById.get(documentId)?.collectionId !==
              hit.collectionId
          ).length
        : 0;
    const nextDrag: InternalDocumentDrag = {
      ...active,
      x: clientX,
      y: clientY,
      targetCollectionId: hit.collectionId,
      rejected: hit.kind !== "collection" || movableDocumentCount === 0
    };
    activeDocumentDragRef.current = nextDrag;
    setInternalDocumentDrag(nextDrag);
  }

  function finishDocumentDrag(event: ReactPointerEvent<HTMLElement>) {
    const pending = pendingDocumentDragRef.current;
    if (!pending || pending.pointerId !== pointerIdFrom(event)) {
      return;
    }

    const active = activeDocumentDragRef.current;
    pendingDocumentDragRef.current = null;
    if (!active) {
      return;
    }

    activeDocumentDragRef.current = null;
    setInternalDocumentDrag(null);
    suppressDocumentClickRef.current = true;
    window.setTimeout(() => {
      suppressDocumentClickRef.current = false;
    }, 0);

    const hit = dropHitFromPointerEvent(event);
    if (hit.kind !== "collection" || !hit.collectionId) {
      return;
    }
    const documentsById = new Map(
      documents.map((document) => [document.id, document])
    );
    const documentIds = active.documentIds.filter(
      (documentId) =>
        documentsById.get(documentId)?.collectionId !== hit.collectionId
    );
    if (documentIds.length > 0) {
      void runBatchOperation(
        {
          kind: "moveToCollection",
          collectionId: hit.collectionId
        },
        documentIds
      );
    }
  }

  function cancelDocumentDrag(event: ReactPointerEvent<HTMLElement>) {
    if (pendingDocumentDragRef.current?.pointerId !== pointerIdFrom(event)) {
      return;
    }
    pendingDocumentDragRef.current = null;
    activeDocumentDragRef.current = null;
    setInternalDocumentDrag(null);
  }

  const selectedCollection = selectedCollectionId
    ? collections.find(
        (collection) => collection.id === selectedCollectionId
      ) ?? null
    : null;
  const selectedTag = selectedTagId
    ? tags.find((tag) => tag.id === selectedTagId) ?? null
    : null;
  const normalizedQuery = searchQuery.trim();
  const searchActive = normalizedQuery.length > 0;
  const activeFilters: DocumentSearchFilters = {
    collectionId: selectedCollectionId,
    tagId: selectedTagId,
    fileType: fileTypeFilter,
    documentDateFrom,
    documentDateTo
  };
  const hasActiveFilters = Object.values(activeFilters).some(Boolean);
  const filteredDocuments = documents.filter((document) =>
    documentMatchesFilters(document, activeFilters)
  );
  const searchResults = (searchResponse?.results ?? []).filter((result) =>
    documentMatchesFilters(result.document, activeFilters)
  );
  const visibleDocuments = searchActive
    ? searchResults.map((result) => result.document)
    : filteredDocuments;
  const selectedCount = selectedDocumentIds.size;
  const draggingDocumentIds = new Set(
    internalDocumentDrag?.documentIds ?? []
  );
  const dragPayloadCount =
    internalDocumentDrag?.documentIds.length ??
    externalFileDrag?.count ??
    0;
  const dropTargetCollectionId =
    internalDocumentDrag?.targetCollectionId ??
    externalFileDrag?.targetCollectionId ??
    null;
  const dropTargetRejected = internalDocumentDrag?.rejected ?? false;
  const allVisibleSelected =
    visibleDocuments.length > 0 &&
    visibleDocuments.every((document) => selectedDocumentIds.has(document.id));
  const selectedDocument = selectedDocumentId
    ? visibleDocuments.find(
        (document) => document.id === selectedDocumentId
      ) ?? null
    : null;
  const fileTypes = [...importableDocumentTypes].sort((left, right) =>
    left.localeCompare(right, "zh-CN")
  );
  let emptyStateKind: DocumentEmptyStateKind | null = null;
  if (documents.length === 0) {
    emptyStateKind = "library";
  } else if (visibleDocuments.length === 0) {
    const hasAdditionalFilters = Boolean(
      fileTypeFilter || documentDateFrom || documentDateTo
    );
    if (searchActive) {
      emptyStateKind = "search";
    } else if (selectedTagId && !hasAdditionalFilters) {
      emptyStateKind = "tag";
    } else if (selectedCollectionId && !hasAdditionalFilters) {
      emptyStateKind = "collection";
    } else {
      emptyStateKind = "search";
    }
  }
  const decisionItem =
    importRun?.items.find(
      (item) => item.itemId === activeDecisionItemId
    ) ?? null;

  function clearDocumentSelection() {
    selectionAnchorIdRef.current = null;
    setSelectedDocumentIds(new Set());
  }

  function selectDocument(
    documentId: string,
    event: ReactMouseEvent<HTMLButtonElement>
  ) {
    if (suppressDocumentClickRef.current) {
      return;
    }
    if (batchRunningRef.current) {
      return;
    }

    if (event.shiftKey) {
      const anchorId = selectionAnchorIdRef.current;
      const anchorIndex = anchorId
        ? visibleDocuments.findIndex((document) => document.id === anchorId)
        : -1;
      const targetIndex = visibleDocuments.findIndex(
        (document) => document.id === documentId
      );
      if (anchorIndex >= 0 && targetIndex >= 0) {
        const start = Math.min(anchorIndex, targetIndex);
        const end = Math.max(anchorIndex, targetIndex);
        setSelectedDocumentIds(
          new Set(
            visibleDocuments
              .slice(start, end + 1)
              .map((document) => document.id)
          )
        );
        setSelectedDocumentId(documentId);
        return;
      }
    }

    if (event.ctrlKey || event.metaKey) {
      selectionAnchorIdRef.current = documentId;
      setSelectedDocumentIds((current) => {
        const next = new Set(current);
        if (next.has(documentId)) {
          next.delete(documentId);
        } else {
          next.add(documentId);
        }
        return next;
      });
      setSelectedDocumentId(documentId);
      return;
    }

    selectionAnchorIdRef.current = documentId;
    setSelectedDocumentIds(new Set([documentId]));
    setSelectedDocumentId(documentId);
  }

  function selectAllVisibleDocuments() {
    if (batchRunningRef.current) {
      return;
    }
    setSelectedDocumentIds(
      new Set(visibleDocuments.map((document) => document.id))
    );
  }

  function selectAllDocuments() {
    setShowingTrash(false);
    clearSearchFilters();
    clearDocumentSelection();
  }

  function selectCollection(collectionId: string) {
    setShowingTrash(false);
    setSelectedCollectionId(collectionId);
    setSelectedDocumentId(null);
    clearDocumentSelection();
  }

  function selectTag(tagId: string) {
    setShowingTrash(false);
    setSelectedTagId(tagId);
    setSelectedDocumentId(null);
    clearDocumentSelection();
  }

  function selectTrash() {
    clearSearchFilters();
    clearDocumentSelection();
    setHighlightedDocumentId(null);
    setShowingTrash(true);
  }

  function changeSearchFilters(change: {
    collectionId?: string | null;
    tagId?: string | null;
    fileType?: string | null;
    documentDateFrom?: string | null;
    documentDateTo?: string | null;
  }) {
    if ("collectionId" in change) {
      setSelectedCollectionId(change.collectionId ?? null);
    }
    if ("tagId" in change) {
      setSelectedTagId(change.tagId ?? null);
    }
    if ("fileType" in change) {
      setFileTypeFilter(change.fileType ?? null);
    }
    if ("documentDateFrom" in change) {
      setDocumentDateFrom(change.documentDateFrom ?? null);
    }
    if ("documentDateTo" in change) {
      setDocumentDateTo(change.documentDateTo ?? null);
    }
    setSelectedDocumentId(null);
    clearDocumentSelection();
  }

  function clearSearchFilters() {
    setSelectedCollectionId(null);
    setSelectedTagId(null);
    setFileTypeFilter(null);
    setDocumentDateFrom(null);
    setDocumentDateTo(null);
    setSelectedDocumentId(null);
    clearDocumentSelection();
  }

  function changeDocumentView(view: DocumentView) {
    setDocumentView(view);
    rememberDocumentView(view);
  }

  return (
    <div
      className={`app-shell${
        internalDocumentDrag ? " dragging-documents" : ""
      }${
        internalDocumentDrag?.rejected ? " drag-rejected" : ""
      }`}
      onPointerMove={updateDocumentDrag}
      onPointerUp={finishDocumentDrag}
      onPointerCancel={cancelDocumentDrag}
    >
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
              !showingTrash && !hasActiveFilters
                ? " active"
                : ""
            }`}
            type="button"
            data-drop-kind="all-documents"
            onClick={selectAllDocuments}
          >
            <LibraryBig size={18} aria-hidden="true" />
            <span>全部文档</span>
            <em>{documents.length}</em>
          </button>
          <button
            className={`nav-item${showingTrash ? " active" : ""}`}
            type="button"
            onClick={selectTrash}
          >
            <Trash2 size={18} aria-hidden="true" />
            <span>回收站</span>
            <em>{trashDocuments.length}</em>
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
            dragPayloadCount={dragPayloadCount}
            dropTargetCollectionId={dropTargetCollectionId}
            dropTargetRejected={dropTargetRejected}
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

        <button
          ref={settingsButtonRef}
          className="nav-item"
          type="button"
          onClick={onOpenSettings}
        >
          <Settings size={18} aria-hidden="true" />
          <span>设置</span>
        </button>
      </aside>

      <section className="workspace">
        <header className="workspace-header">
          <div className="library-heading">
            {showingTrash ? (
              <Trash2 size={19} aria-hidden="true" />
            ) : selectedTag ? (
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
                {showingTrash
                  ? "回收站"
                  : selectedTag?.name ??
                    selectedCollection?.name ??
                    "全部文档"}
              </strong>
              <span>
                {showingTrash
                  ? `${trashDocuments.length} 份文档`
                  : `${visibleDocuments.length} 份文档`}
              </span>
            </div>
          </div>
          <div className="header-actions">
            {!showingTrash ? (
              <>
                <form
                  className="search-form"
                  role="search"
                  onSubmit={(event) => event.preventDefault()}
                >
                  <Search size={17} aria-hidden="true" />
                  <input
                    type="search"
                    aria-label="搜索文档"
                    placeholder="搜索标题、描述和正文"
                    value={searchQuery}
                    onChange={(event) => {
                      setSearchQuery(event.target.value);
                      clearDocumentSelection();
                    }}
                  />
                  {searchQuery ? (
                    <button
                      className="icon-button compact"
                      type="button"
                      onClick={() => setSearchQuery("")}
                      aria-label="清除搜索词"
                      title="清除搜索词"
                    >
                      <X size={14} aria-hidden="true" />
                    </button>
                  ) : null}
                </form>
                <button
                  className={`button secondary filter-button${
                    hasActiveFilters ? " active" : ""
                  }`}
                  type="button"
                  onClick={() => setFiltersOpen((current) => !current)}
                  aria-expanded={filtersOpen || hasActiveFilters}
                  aria-controls="search-filter-panel"
                >
                  <SlidersHorizontal size={16} aria-hidden="true" />
                  筛选
                  {hasActiveFilters ? (
                    <span className="filter-count" aria-hidden="true">
                      {
                        [
                          selectedCollectionId,
                          selectedTagId,
                          fileTypeFilter,
                          documentDateFrom,
                          documentDateTo
                        ].filter(Boolean).length
                      }
                    </span>
                  ) : null}
                </button>
                {indexing ? (
                  <span className="indexing-status" role="status">
                    <LoaderCircle
                      className="spin"
                      size={15}
                      aria-hidden="true"
                    />
                    {indexProgress
                      ? `正在建立索引 ${indexProgress.processed}/${indexProgress.total}`
                      : "正在建立索引"}
                  </span>
                ) : null}
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
                    <LoaderCircle
                      className="spin"
                      size={17}
                      aria-hidden="true"
                    />
                  ) : (
                    <FilePlus2 size={17} aria-hidden="true" />
                  )}
                  {importing ? "正在导入" : "导入文档"}
                </button>
              </>
            ) : (
              <button
                className="button danger"
                type="button"
                onClick={() => setEmptyTrashOpen(true)}
                disabled={trashDocuments.length === 0}
              >
                <Trash2 size={16} aria-hidden="true" />
                清空回收站
              </button>
            )}
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

        {!showingTrash && (filtersOpen || hasActiveFilters) ? (
          <div id="search-filter-panel">
            <SearchFilters
              collections={collections}
              tags={tags}
              fileTypes={fileTypes}
              collectionId={selectedCollectionId}
              tagId={selectedTagId}
              fileType={fileTypeFilter}
              documentDateFrom={documentDateFrom}
              documentDateTo={documentDateTo}
              hasActiveFilters={hasActiveFilters}
              onChange={changeSearchFilters}
              onClear={clearSearchFilters}
            />
          </div>
        ) : null}

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

        {importRestartNotice ? (
          <p className="muted-copy" role="status">
            此前未完成的导入请重新发起。
          </p>
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
            {showingTrash ? (
              <TrashDocumentList
                documents={trashDocuments}
                restoringIds={restoringTrashIds}
                permanentlyDeletingIds={permanentlyDeletingIds}
                onRestore={(document) =>
                  void restoreTrashDocument(document)
                }
                onPermanentlyDelete={setPermanentDeleteTarget}
              />
            ) : searchError ? (
              <main className="search-error-state" aria-label="搜索失败">
                <AlertCircle size={22} aria-hidden="true" />
                <strong>搜索失败</strong>
                <span>{searchError}</span>
                <button
                  className="button quiet"
                  type="button"
                  onClick={() => setSearchRevision((value) => value + 1)}
                >
                  重试搜索
                </button>
              </main>
            ) : searching && searchActive && !searchResponse ? (
              <main className="search-loading-state" aria-label="正在搜索">
                <LoaderCircle className="spin" size={22} aria-hidden="true" />
                <span>正在搜索</span>
              </main>
            ) : emptyStateKind ? (
              <DocumentEmptyState
                kind={emptyStateKind}
                importing={importing}
                onImport={() => void chooseDocuments()}
              />
            ) : (
              <>
                <div className="document-view-toolbar">
                  <div className="batch-selection-controls">
                    <strong>已选择 {selectedCount} 项</strong>
                    <button
                      className="button quiet"
                      type="button"
                      onClick={selectAllVisibleDocuments}
                      disabled={allVisibleSelected || Boolean(batchRunning)}
                    >
                      全选当前结果
                    </button>
                    <button
                      className="button quiet"
                      type="button"
                      onClick={clearDocumentSelection}
                      disabled={
                        selectedCount === 0 || Boolean(batchRunning)
                      }
                    >
                      清空选择
                    </button>
                    <BatchActionMenu
                      count={selectedCount}
                      disabled={Boolean(batchRunning)}
                      onChoose={setBatchAction}
                    />
                    {batchRunning ? (
                      <BatchRunningStatus
                        count={batchRunning.count}
                        cancelling={cancellingBatch}
                        onCancel={() => void cancelBatchOperation()}
                      />
                    ) : null}
                  </div>
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
                {batchResult ? (
                  <BatchResultPanel
                    result={batchResult}
                    documents={documents}
                    onRetry={retryFailedBatchOperation}
                    onDismiss={() => setBatchResult(null)}
                  />
                ) : null}
                {documentView === "list" ? (
                  <DocumentList
                    documents={visibleDocuments}
                    collections={collections}
                    selectedDocumentId={selectedDocumentId}
                    selectedDocumentIds={selectedDocumentIds}
                    draggingDocumentIds={draggingDocumentIds}
                    selectionDisabled={Boolean(batchRunning)}
                    highlightedDocumentId={highlightedDocumentId}
                    searchResults={searchResults}
                    retryingIndexIds={retryingIndexIds}
                    onSelectDocument={selectDocument}
                    onStartDocumentDrag={startDocumentDrag}
                    onMoveDocument={(document, targetCollectionId) =>
                      void moveDocument(document, targetCollectionId)
                    }
                    onEditDocument={setMetadataTarget}
                    onMoveDocumentToTrash={setMoveToTrashTarget}
                    onRetryIndex={(document) =>
                      void retryDocumentIndex(document)
                    }
                  />
                ) : (
                  <DocumentGrid
                    client={client}
                    documents={visibleDocuments}
                    collections={collections}
                    selectedDocumentId={selectedDocumentId}
                    selectedDocumentIds={selectedDocumentIds}
                    draggingDocumentIds={draggingDocumentIds}
                    selectionDisabled={Boolean(batchRunning)}
                    highlightedDocumentId={highlightedDocumentId}
                    searchResults={searchResults}
                    retryingIndexIds={retryingIndexIds}
                    onSelectDocument={selectDocument}
                    onStartDocumentDrag={startDocumentDrag}
                    onMoveDocument={(document, targetCollectionId) =>
                      void moveDocument(document, targetCollectionId)
                    }
                    onEditDocument={setMetadataTarget}
                    onMoveDocumentToTrash={setMoveToTrashTarget}
                    onRetryIndex={(document) =>
                      void retryDocumentIndex(document)
                    }
                  />
                )}
              </>
            )}
          </>
        )}
      </section>

      <DocumentDetails
        client={client}
        document={showingTrash ? null : selectedDocument}
        collections={collections}
        retryingIndex={
          selectedDocument ? retryingIndexIds.has(selectedDocument.id) : false
        }
        onEditDocument={setMetadataTarget}
        onMoveDocumentToTrash={setMoveToTrashTarget}
        onRetryIndex={(document) => void retryDocumentIndex(document)}
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

      {batchAction ? (
        <BatchOperationDialog
          action={batchAction}
          count={selectedCount}
          collections={collections}
          tags={tags}
          onClose={() => setBatchAction(null)}
          onConfirm={startBatchOperation}
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

      {moveToTrashTarget ? (
        <MoveDocumentToTrashDialog
          document={moveToTrashTarget}
          onClose={() => setMoveToTrashTarget(null)}
          onConfirm={moveDocumentToTrash}
        />
      ) : null}

      {permanentDeleteTarget ? (
        <PermanentDeleteDocumentDialog
          document={permanentDeleteTarget}
          onClose={() => setPermanentDeleteTarget(null)}
          onConfirm={permanentlyDeleteTrashDocument}
        />
      ) : null}

      {emptyTrashOpen ? (
        <EmptyTrashDialog
          count={trashDocuments.length}
          onClose={() => setEmptyTrashOpen(false)}
          onConfirm={emptyTrash}
        />
      ) : null}

      {decisionItem ? (
        <ImportDecisionDialog
          item={decisionItem}
          documents={documents}
          onResolve={resolveDecision}
        />
      ) : null}

      {internalDocumentDrag ? (
        <div
          className={`document-drag-ghost${
            internalDocumentDrag.rejected ? " rejected" : ""
          }`}
          style={{
            left: internalDocumentDrag.x + 14,
            top: internalDocumentDrag.y + 14
          }}
          data-drag-payload={internalDocumentDrag.documentIds.length}
          aria-hidden="true"
        >
          <Files size={16} />
          <span>{internalDocumentDrag.documentIds.length} 份文档</span>
          <small>
            {internalDocumentDrag.rejected
              ? "禁止放置"
              : "移动到集合"}
          </small>
        </div>
      ) : null}
    </div>
  );
}
