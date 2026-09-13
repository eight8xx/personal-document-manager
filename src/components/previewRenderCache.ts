import type { DocumentFormatId, DocumentSummary } from "../backend/types";

const MAX_CACHE_ENTRIES = 12;

interface PptxRenderSnapshot {
  html: string;
  slideCount: number;
}

const docxRenderCache = new Map<string, Node[]>();
const pptxRenderCache = new Map<string, PptxRenderSnapshot>();

function contentVersion(document: DocumentSummary) {
  const hash = document.contentHash?.trim();
  if (hash) {
    return `hash:${hash}`;
  }
  return `fallback:${document.fileSize}:${document.lastImportedAt}`;
}

function renderCacheKey(
  formatId: DocumentFormatId,
  document: DocumentSummary
) {
  return `${formatId}:${contentVersion(document)}`;
}

function touch<K, V>(cache: Map<K, V>, key: K): V | undefined {
  const value = cache.get(key);
  if (value === undefined) {
    return undefined;
  }
  cache.delete(key);
  cache.set(key, value);
  return value;
}

function insert<K, V>(cache: Map<K, V>, key: K, value: V) {
  cache.delete(key);
  cache.set(key, value);
  while (cache.size > MAX_CACHE_ENTRIES) {
    const oldest = cache.keys().next().value;
    if (oldest === undefined) {
      break;
    }
    cache.delete(oldest);
  }
}

export function getCachedDocxRender(
  formatId: DocumentFormatId,
  document: DocumentSummary
): Node[] | null {
  const nodes = touch(docxRenderCache, renderCacheKey(formatId, document));
  return nodes?.map((node) => node.cloneNode(true)) ?? null;
}

export function cacheDocxRender(
  formatId: DocumentFormatId,
  document: DocumentSummary,
  nodes: Node[]
) {
  insert(
    docxRenderCache,
    renderCacheKey(formatId, document),
    nodes.map((node) => node.cloneNode(true))
  );
}

export function getCachedPptxRender(
  formatId: DocumentFormatId,
  document: DocumentSummary
): PptxRenderSnapshot | null {
  return touch(pptxRenderCache, renderCacheKey(formatId, document)) ?? null;
}

export function cachePptxRender(
  formatId: DocumentFormatId,
  document: DocumentSummary,
  snapshot: PptxRenderSnapshot
) {
  if (/blob:/i.test(snapshot.html)) {
    return;
  }
  insert(pptxRenderCache, renderCacheKey(formatId, document), { ...snapshot });
}

export function clearPreviewRenderCache() {
  docxRenderCache.clear();
  pptxRenderCache.clear();
}
