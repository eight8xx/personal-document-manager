import type { PptxViewer } from "@file-viewer/pptx";
import {
  AlertCircle,
  ChevronLeft,
  ChevronRight,
  ExternalLink,
  Maximize2,
  Presentation,
  RotateCcw,
  ZoomIn,
  ZoomOut
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useRef,
  useState
} from "react";
import type { CSSProperties } from "react";

import { documentFormatIdForType } from "../backend/documentFormats";
import { toBackendError } from "../backend/error";
import { safeExternalUrl } from "../backend/url";
import type {
  BackendClient,
  DocumentPreview,
  DocumentSummary
} from "../backend/types";
import {
  cachePptxRender,
  getCachedPptxRender
} from "./previewRenderCache";

type PptxDocumentPreview = Extract<DocumentPreview, { kind: "pptx" }>;

const PPTX_MEDIA_TYPE =
  "application/vnd.openxmlformats-officedocument.presentationml.presentation";
const MIN_SCALE = 0.4;
const MAX_SCALE = 2.5;
const SCALE_STEP = 0.1;
const THUMBNAIL_TIMEOUT_MS = 15_000;

function clampScale(value: number) {
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, value));
}

function blobFromDataUrl(dataUrl: string) {
  const match = /^data:([^;,]+)?(;base64)?,([\s\S]*)$/.exec(dataUrl);
  if (!match) {
    throw new Error("PPTX 数据格式无效。");
  }
  const [, mediaType = PPTX_MEDIA_TYPE, base64Marker, payload] = match;
  if (base64Marker) {
    const binary = window.atob(payload);
    const bytes = new Uint8Array(binary.length);
    for (let index = 0; index < binary.length; index += 1) {
      bytes[index] = binary.charCodeAt(index);
    }
    return new Blob([bytes], { type: mediaType || PPTX_MEDIA_TYPE });
  }
  return new Blob([decodeURIComponent(payload)], {
    type: mediaType || PPTX_MEDIA_TYPE
  });
}

function blobToArrayBuffer(blob: Blob) {
  if (typeof blob.arrayBuffer === "function") {
    return blob.arrayBuffer();
  }
  return new Promise<ArrayBuffer>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      if (reader.result instanceof ArrayBuffer) {
        resolve(reader.result);
      } else {
        reject(new Error("无法读取 PPTX 二进制数据。"));
      }
    };
    reader.onerror = () => reject(reader.error ?? new Error("无法读取 PPTX。"));
    reader.readAsArrayBuffer(blob);
  });
}

function isRemoteResource(value: string) {
  return /^(?:https?:)?\/\//i.test(value.trim()) || /^ftp:/i.test(value.trim());
}

function sanitizePptxCss(cssText: string) {
  return cssText
    .replace(/@import[^;]+;/gi, "")
    .replace(/url\(\s*(['"]?)(?:https?:|\/\/|ftp:)[^)]*\)/gi, 'url("")');
}

function sanitizePptxRoot(root: ParentNode) {
  for (const element of Array.from(
    root.querySelectorAll(
      "script, iframe, object, embed, form, video, audio, source"
    )
  )) {
    element.remove();
  }
  for (const element of Array.from(root.querySelectorAll("*"))) {
    for (const attribute of Array.from(element.attributes)) {
      if (attribute.name.toLocaleLowerCase().startsWith("on")) {
        element.removeAttribute(attribute.name);
      }
    }
    element.removeAttribute("srcdoc");
    const style = element.getAttribute("style");
    if (style) {
      element.setAttribute("style", sanitizePptxCss(style));
    }
    for (const attributeName of ["src", "srcset", "poster", "xlink:href"]) {
      const value = element.getAttribute(attributeName);
      if (value && isRemoteResource(value)) {
        element.removeAttribute(attributeName);
      }
    }
  }
  for (const style of Array.from(root.querySelectorAll("style"))) {
    style.textContent = sanitizePptxCss(style.textContent ?? "");
  }
}

function configureHyperlinks(
  root: ParentNode,
  client: BackendClient,
  signal: AbortSignal,
  onLinkError: (message: string) => void
) {
  for (const anchor of Array.from(
    root.querySelectorAll<HTMLAnchorElement>("a")
  )) {
    if (anchor.dataset.pptxLinkConfigured === "true") {
      continue;
    }
    anchor.dataset.pptxLinkConfigured = "true";
    const rawHref =
      anchor.getAttribute("href")?.trim() ??
      anchor.dataset.pptxSafeHref?.trim() ??
      "";
    anchor.removeAttribute("href");
    anchor.removeAttribute("target");
    anchor.removeAttribute("rel");
    const externalUrl = safeExternalUrl(rawHref);
    if (!externalUrl) {
      anchor.classList.add("pptx-blocked-link");
      anchor.setAttribute("aria-disabled", "true");
      anchor.title = "已阻止不安全的链接";
      continue;
    }

    anchor.dataset.pptxSafeHref = externalUrl;
    anchor.classList.add("pptx-safe-link");
    anchor.setAttribute("role", "link");
    anchor.tabIndex = 0;
    const activate = () => {
      void client.openExternalUrl(externalUrl).catch((caught) => {
        onLinkError(toBackendError(caught).message);
      });
    };
    anchor.addEventListener(
      "click",
      (event) => {
        event.preventDefault();
        activate();
      },
      { signal }
    );
    anchor.addEventListener(
      "keydown",
      (event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          activate();
        }
      },
      { signal }
    );
  }
}

function rendererErrorMessage(error: unknown) {
  if (error instanceof Error && error.message) {
    return error.message;
  }
  if (
    error &&
    typeof error === "object" &&
    "message" in error &&
    typeof error.message === "string"
  ) {
    return error.message;
  }
  return "PPTX 版式预览渲染失败。";
}

function slideElements(root: HTMLElement | null) {
  if (!root) {
    return [];
  }
  return Array.from(
    root.querySelectorAll<HTMLElement>(
      ".flyfish-pptx-content > .slide, .flyfish-pptx-content > .flyfish-pptx-slide-slot > .slide"
    )
  );
}

function pptxCacheHtml(host: HTMLElement) {
  const clone = host.cloneNode(true) as HTMLElement;
  for (const anchor of Array.from(
    clone.querySelectorAll<HTMLAnchorElement>("a[data-pptx-link-configured]")
  )) {
    anchor.removeAttribute("data-pptx-link-configured");
  }
  return clone.innerHTML;
}

function SlideNavigationThumbnail({
  element,
  index,
  active,
  onSelect
}: {
  element: HTMLElement | null;
  index: number;
  active: boolean;
  onSelect: () => void;
}) {
  let sanitizedHtml = "";
  if (element) {
    const clone = element.cloneNode(true) as HTMLElement;
    clone.removeAttribute("id");
    for (const node of Array.from(clone.querySelectorAll("[id]"))) {
      node.removeAttribute("id");
    }
    for (const anchor of Array.from(clone.querySelectorAll("a"))) {
      const text = document.createElement("span");
      text.textContent = anchor.textContent;
      anchor.replaceWith(text);
    }
    sanitizePptxRoot(clone);
    sanitizedHtml = clone.outerHTML;
  }

  return (
    <button
      className={`pptx-slide-navigation${active ? " active" : ""}`}
      type="button"
      onClick={onSelect}
      aria-label={`转到第 ${index} 张幻灯片`}
      aria-current={active ? "page" : undefined}
    >
      <span className="pptx-slide-navigation-visual" aria-hidden="true">
        {sanitizedHtml ? (
          <span
            className="pptx-slide-navigation-clone"
            dangerouslySetInnerHTML={{ __html: sanitizedHtml }}
          />
        ) : (
          <span>{index}</span>
        )}
      </span>
      <span className="pptx-slide-navigation-number">{index}</span>
    </button>
  );
}

export function PptxPreview({
  client,
  document,
  preview
}: {
  client: BackendClient;
  document: DocumentSummary;
  preview: PptxDocumentPreview;
}) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const viewerRef = useRef<PptxViewer | null>(null);
  const safetyTimerRef = useRef<number | null>(null);
  const [renderError, setRenderError] = useState("");
  const [linkError, setLinkError] = useState("");
  const [renderAttempt, setRenderAttempt] = useState(0);
  const [rendering, setRendering] = useState(true);
  const [page, setPage] = useState(1);
  const [pageCount, setPageCount] = useState(1);
  const [zoom, setZoom] = useState(1);
  const [fitWidth, setFitWidth] = useState(true);
  const [renderedSlides, setRenderedSlides] = useState<
    Map<number, HTMLElement>
  >(new Map());
  const [runtimeDegradations, setRuntimeDegradations] = useState<string[]>([]);
  const [opening, setOpening] = useState(false);
  const effectiveScale = fitWidth ? 1 : zoom;
  const resolvedPageCount = Math.max(pageCount, renderedSlides.size, 1);
  const degradedFeatures = Array.from(
    new Set([...preview.degradedFeatures, ...runtimeDegradations])
  );

  const refreshSafety = useCallback(
    (abortController: AbortController) => {
      const host = hostRef.current;
      if (!host || abortController.signal.aborted) {
        return;
      }
      sanitizePptxRoot(host);
      configureHyperlinks(
        host,
        client,
        abortController.signal,
        setLinkError
      );
    },
    [client]
  );

  const scheduleSafetyRefresh = useCallback(
    (abortController: AbortController) => {
      refreshSafety(abortController);
      if (safetyTimerRef.current !== null) {
        window.clearTimeout(safetyTimerRef.current);
      }
      safetyTimerRef.current = window.setTimeout(() => {
        safetyTimerRef.current = null;
        refreshSafety(abortController);
      }, 0);
    },
    [refreshSafety]
  );

  useEffect(() => {
    let active = true;
    let renderFailed = false;
    const abortController = new AbortController();
    const host = hostRef.current;
    viewerRef.current?.destroy();
    viewerRef.current = null;
    host?.replaceChildren();
    setRenderError("");
    setLinkError("");
    setRuntimeDegradations([]);
    setRenderedSlides(new Map());
    setPage(1);
    setPageCount(1);
    setZoom(1);
    setFitWidth(true);
    setRendering(true);

    try {
      const formatId = documentFormatIdForType(document.fileType);
      const cached =
        formatId === null ? null : getCachedPptxRender(formatId, document);
      if (cached && host) {
        host.innerHTML = cached.html;
        const elements = slideElements(host);
        setRenderedSlides(
          new Map(
            elements.map((element, index) => [index + 1, element] as const)
          )
        );
        setPageCount(Math.max(cached.slideCount, elements.length, 1));
        setRendering(false);
        refreshSafety(abortController);
        return () => {
          active = false;
          abortController.abort();
          host.replaceChildren();
        };
      }

      const blob = blobFromDataUrl(preview.dataUrl);
      void blobToArrayBuffer(blob)
        .then((buffer) =>
          import("@file-viewer/pptx").then(({ PptxViewer }) =>
            PptxViewer.open(buffer, host!, {
              fitMode: "contain",
              zoomPercent: 100,
              lazySlides: false,
              lazyMedia: false,
              engineOptions: {
                mediaProcess: false,
                keyBoardShortCut: false
              },
              onSlideRendered(index, element) {
                if (!active || !(element instanceof HTMLElement)) {
                  return;
                }
                setRenderedSlides((current) => {
                  const next = new Map(current);
                  next.set(index, element);
                  return next;
                });
                setPageCount((current) => Math.max(current, index, 1));
                scheduleSafetyRefresh(abortController);
              },
              onSlideError(index) {
                if (active) {
                  setRuntimeDegradations((current) =>
                    current.includes(`第 ${index} 张幻灯片`)
                      ? current
                      : [...current, `第 ${index} 张幻灯片`]
                  );
                }
              },
              onRenderComplete() {
                if (!active) {
                  return;
                }
                const resolvedCount = Math.max(
                  viewerRef.current?.slideCount ?? 0,
                  slideElements(hostRef.current).length,
                  1
                );
                setPageCount((current) => Math.max(current, resolvedCount));
                setRendering(false);
                refreshSafety(abortController);
                const renderedHost = hostRef.current;
                if (formatId && renderedHost) {
                  cachePptxRender(formatId, document, {
                    html: pptxCacheHtml(renderedHost),
                    slideCount: resolvedCount
                  });
                }
                scheduleSafetyRefresh(abortController);
              },
              onError(error) {
                if (active) {
                  renderFailed = true;
                  viewerRef.current?.destroy();
                  viewerRef.current = null;
                  setRendering(false);
                  setRenderError(rendererErrorMessage(error));
                }
              }
            })
          )
        )
        .then((viewer) => {
          if (!active || renderFailed) {
            viewer.destroy();
            return;
          }
          viewerRef.current = viewer;
          scheduleSafetyRefresh(abortController);
        })
        .catch((caught) => {
          if (active) {
            renderFailed = true;
            viewerRef.current?.destroy();
            viewerRef.current = null;
            setRendering(false);
            setRenderError(rendererErrorMessage(caught));
          }
        });
    } catch (caught) {
      setRendering(false);
      setRenderError(rendererErrorMessage(caught));
    }

    return () => {
      active = false;
      abortController.abort();
      if (safetyTimerRef.current !== null) {
        window.clearTimeout(safetyTimerRef.current);
        safetyTimerRef.current = null;
      }
      viewerRef.current?.destroy();
      viewerRef.current = null;
      host?.replaceChildren();
    };
  }, [
    client,
    document.contentHash,
    document.fileSize,
    document.fileType,
    document.id,
    document.lastImportedAt,
    preview,
    renderAttempt,
    refreshSafety,
    scheduleSafetyRefresh
  ]);

  function changePage(nextPage: number) {
    const resolvedNext = Math.min(
      resolvedPageCount,
      Math.max(1, nextPage)
    );
    setPage(resolvedNext);
    const viewer = viewerRef.current;
    const element =
      viewer?.ensureSlideRendered(resolvedNext) ??
      renderedSlides.get(resolvedNext) ??
      slideElements(hostRef.current)[resolvedNext - 1];
    element?.scrollIntoView?.({ block: "start", behavior: "auto" });
  }

  function zoomBy(delta: number) {
    const baseScale = fitWidth ? 1 : zoom;
    const next = clampScale(baseScale + delta);
    setFitWidth(false);
    setZoom(next);
    void viewerRef.current?.setZoom(next * 100);
  }

  function fitPreviewWidth() {
    setFitWidth(true);
    setZoom(1);
    void viewerRef.current?.setZoom(100).then(() => {
      viewerRef.current?.refreshLayout();
    });
  }

  async function openExternal() {
    setOpening(true);
    setLinkError("");
    try {
      await client.openDocument(document.id);
    } catch (caught) {
      setLinkError(toBackendError(caught).message);
    } finally {
      setOpening(false);
    }
  }

  if (renderError) {
    return (
      <div className="pptx-layout-preview">
        <div className="preview-error pptx-render-error" role="alert">
          <AlertCircle size={22} aria-hidden="true" />
          <strong>PPTX 版式预览渲染失败</strong>
          <span>{renderError}</span>
          <div className="pptx-error-actions">
            <button
              className="button quiet"
              type="button"
              onClick={() => {
                setRenderError("");
                setRenderAttempt((attempt) => attempt + 1);
              }}
            >
              <RotateCcw size={14} aria-hidden="true" />
              重试版式预览
            </button>
            <button
              className="button quiet"
              type="button"
              onClick={() => void openExternal()}
              disabled={opening}
              aria-label={`用系统默认程序打开 ${document.title}`}
            >
              <ExternalLink size={14} aria-hidden="true" />
              外部打开
            </button>
          </div>
        </div>
        <div className="pptx-fallback-text">
          <strong>提取正文</strong>
          <pre tabIndex={0}>{preview.text}</pre>
        </div>
      </div>
    );
  }

  return (
    <div className="pptx-layout-preview">
      <div className="preview-toolbar pptx-preview-toolbar">
        <div className="pptx-page-navigation">
          <button
            className="icon-button compact"
            type="button"
            onClick={() => changePage(page - 1)}
            disabled={page <= 1}
            aria-label="PPTX 上一页"
            title="上一张"
          >
            <ChevronLeft size={15} aria-hidden="true" />
          </button>
          <span>
            第 {page} / {resolvedPageCount} 张
          </span>
          <button
            className="icon-button compact"
            type="button"
            onClick={() => changePage(page + 1)}
            disabled={page >= resolvedPageCount}
            aria-label="PPTX 下一页"
            title="下一张"
          >
            <ChevronRight size={15} aria-hidden="true" />
          </button>
        </div>
        <div className="pptx-zoom-controls">
          <button
            className="icon-button compact"
            type="button"
            onClick={() => zoomBy(-SCALE_STEP)}
            aria-label="缩小 PPTX 预览"
            title="缩小"
          >
            <ZoomOut size={15} aria-hidden="true" />
          </button>
          <button
            className={`button quiet pptx-fit-width${
              fitWidth ? " active" : ""
            }`}
            type="button"
            onClick={fitPreviewWidth}
            aria-label="适配 PPTX 预览宽度"
            aria-pressed={fitWidth}
            title="适配宽度"
          >
            <Maximize2 size={14} aria-hidden="true" />
            适配宽度
          </button>
          <button
            className="icon-button compact"
            type="button"
            onClick={() => zoomBy(SCALE_STEP)}
            aria-label="放大 PPTX 预览"
            title="放大"
          >
            <ZoomIn size={15} aria-hidden="true" />
          </button>
          <span className="pptx-zoom-value">
            {Math.round(effectiveScale * 100)}%
          </span>
        </div>
      </div>

      {degradedFeatures.length > 0 ? (
        <p className="preview-notice pptx-degradation-notice" role="status">
          部分复杂内容无法完整呈现：{degradedFeatures.join("、")}。其余内容仍可预览。
        </p>
      ) : (
        <p className="preview-notice">{preview.notice}</p>
      )}

      <div className="pptx-preview-body">
        <nav className="pptx-slide-navigation-list" aria-label="幻灯片缩略图">
          {Array.from({ length: resolvedPageCount }, (_, index) => index + 1).map(
            (slideNumber) => (
              <SlideNavigationThumbnail
                key={slideNumber}
                element={renderedSlides.get(slideNumber) ?? null}
                index={slideNumber}
                active={page === slideNumber}
                onSelect={() => changePage(slideNumber)}
              />
            )
          )}
        </nav>
        <div className="pptx-preview-viewport">
          {rendering ? (
            <span className="pptx-render-progress" role="status">
              正在渲染幻灯片
            </span>
          ) : null}
          <div
            className="pptx-preview-render"
            ref={hostRef}
            style={
              {
                "--pptx-preview-scale": String(effectiveScale)
              } as CSSProperties
            }
          />
        </div>
      </div>
      {linkError ? (
        <p className="preview-inline-error" role="alert">
          <AlertCircle size={14} aria-hidden="true" />
          {linkError}
        </p>
      ) : null}
    </div>
  );
}

async function captureFirstSlideThumbnail(host: HTMLElement) {
  const slide = slideElements(host)[0];
  if (!slide || typeof Image === "undefined") {
    return null;
  }
  const width = Math.max(1, slide.getBoundingClientRect().width || slide.offsetWidth);
  const height = Math.max(
    1,
    slide.getBoundingClientRect().height || slide.offsetHeight
  );
  if (width <= 1 || height <= 1) {
    return null;
  }

  const clone = slide.cloneNode(true) as HTMLElement;
  clone.removeAttribute("id");
  for (const node of Array.from(clone.querySelectorAll("[id]"))) {
    node.removeAttribute("id");
  }
  sanitizePptxRoot(clone);
  const svg = `
    <svg xmlns="http://www.w3.org/2000/svg" width="320" height="180">
      <foreignObject width="100%" height="100%">
        <div xmlns="http://www.w3.org/1999/xhtml"
          style="width:${width}px;height:${height}px;transform:scale(${320 / width},${
            180 / height
          });transform-origin:top left;background:#fff">
          ${clone.outerHTML}
        </div>
      </foreignObject>
    </svg>`;
  const image = new Image();
  const loaded = new Promise<void>((resolve, reject) => {
    image.onload = () => resolve();
    image.onerror = () => reject(new Error("无法生成幻灯片缩略图。"));
  });
  image.src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
  await loaded;

  const canvas = document.createElement("canvas");
  canvas.width = 320;
  canvas.height = 180;
  const context = canvas.getContext("2d");
  if (!context) {
    return null;
  }
  context.fillStyle = "#ffffff";
  context.fillRect(0, 0, canvas.width, canvas.height);
  context.drawImage(image, 0, 0, canvas.width, canvas.height);
  return canvas.toDataURL("image/jpeg", 0.82);
}

const thumbnailCache = new Map<string, Promise<DocumentPreview>>();

async function loadPptxPreview(
  client: BackendClient,
  documentId: string,
  contentHash: string,
  lastImportedAt: string
) {
  const key = `${documentId}:${contentHash || lastImportedAt}`;
  let pending = thumbnailCache.get(key);
  if (!pending) {
    pending = client.getDocumentPreview(documentId);
    thumbnailCache.set(key, pending);
    void pending.catch(() => thumbnailCache.delete(key));
  }
  return pending;
}

export function PptxThumbnail({
  client,
  document
}: {
  client: BackendClient;
  document: DocumentSummary;
}) {
  const containerRef = useRef<HTMLSpanElement | null>(null);
  const generationRef = useRef(0);
  const startedGenerationRef = useRef(0);
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);

  const generate = useCallback(
    async (
      documentId: string,
      contentHash: string,
      lastImportedAt: string,
      signal: AbortSignal,
      generation: number
    ) => {
      const container = containerRef.current;
      if (!container) {
        return;
      }
      const isCurrent = () =>
        !signal.aborted && generationRef.current === generation;
      if (!isCurrent()) {
        return;
      }
      const viewerState = { current: null as PptxViewer | null };
      const host = globalThis.document.createElement("div");
      host.className = "pptx-thumbnail-render-host";
      host.setAttribute("aria-hidden", "true");
      let timer: number | null = null;
      let removeAbortListener: () => void = () => undefined;
      try {
        const cached = await client.getDocumentThumbnail(documentId);
        if (!isCurrent()) {
          return;
        }
        if (
          cached.kind === "pptx" ||
          cached.kind === "image" ||
          cached.kind === "pdf"
        ) {
          setDataUrl(cached.dataUrl);
          return;
        }

        const preview = await loadPptxPreview(
          client,
          documentId,
          contentHash,
          lastImportedAt
        );
        if (!isCurrent()) {
          return;
        }
        if (preview.kind !== "pptx") {
          throw new Error("PPTX 预览数据不可用。");
        }
        const buffer = await blobToArrayBuffer(blobFromDataUrl(preview.dataUrl));
        if (!isCurrent()) {
          return;
        }
        container.append(host);

        const thumb = await new Promise<string | null>((resolve, reject) => {
          let settled = false;
          const settle = (value: string | null) => {
            if (settled) {
              return;
            }
            settled = true;
            resolve(value);
          };
          const abort = () => settle(null);
          signal.addEventListener("abort", abort, { once: true });
          removeAbortListener = () =>
            signal.removeEventListener("abort", abort);
          if (signal.aborted) {
            settle(null);
            return;
          }
          timer = window.setTimeout(
            () => reject(new Error("PPTX 缩略图生成超时。")),
            THUMBNAIL_TIMEOUT_MS
          );
          void import("@file-viewer/pptx")
            .then(({ PptxViewer }) =>
              PptxViewer.open(buffer, host, {
                fitMode: "contain",
                zoomPercent: 100,
                lazySlides: false,
                lazyMedia: false,
                engineOptions: {
                  mediaProcess: false,
                  keyBoardShortCut: false
                },
                async onRenderComplete() {
                  if (settled || !isCurrent()) {
                    return;
                  }
                  const captured = await captureFirstSlideThumbnail(host);
                  settle(isCurrent() ? captured : null);
                },
                onError(error) {
                  if (!isCurrent()) {
                    settle(null);
                    return;
                  }
                  reject(
                    error instanceof Error
                      ? error
                      : new Error(rendererErrorMessage(error))
                  );
                }
              })
            )
            .then((opened) => {
              viewerState.current = opened;
              if (settled || !isCurrent()) {
                opened.destroy();
              }
            })
            .catch((error) => {
              if (isCurrent()) {
                reject(error);
              } else {
                settle(null);
              }
            });
        });
        removeAbortListener();

        if (!isCurrent()) {
          return;
        }
        if (!thumb || !/^data:image\/(?:png|jpeg|jpg);base64,/i.test(thumb)) {
          throw new Error("PPTX 渲染器没有生成有效图片。");
        }
        const saved = await client.saveDocumentThumbnail(
          documentId,
          contentHash,
          thumb
        );
        if (!isCurrent()) {
          return;
        }
        if (saved.kind === "fallback") {
          throw new Error(saved.reason);
        }
        setDataUrl(saved.dataUrl);
      } catch {
        if (isCurrent()) {
          setFailed(true);
        }
      } finally {
        removeAbortListener();
        if (timer !== null) {
          window.clearTimeout(timer);
        }
        viewerState.current?.destroy();
        host.remove();
      }
    },
    [client]
  );

  useEffect(() => {
    const generation = generationRef.current + 1;
    generationRef.current = generation;
    setDataUrl(null);
    setFailed(false);
    const container = containerRef.current;
    if (!container) {
      return;
    }
    const documentId = document.id;
    const contentHash = document.contentHash?.trim() ?? "";
    const lastImportedAt = document.lastImportedAt;
    const abortController = new AbortController();

    const start = () => {
      if (
        abortController.signal.aborted ||
        startedGenerationRef.current === generation
      ) {
        return;
      }
      startedGenerationRef.current = generation;
      if (!contentHash) {
        setFailed(true);
        return;
      }
      void generate(
        documentId,
        contentHash,
        lastImportedAt,
        abortController.signal,
        generation
      );
    };
    if (typeof IntersectionObserver !== "function") {
      start();
      return;
    }
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          observer.disconnect();
          start();
        }
      },
      { rootMargin: "160px" }
    );
    observer.observe(container);
    return () => {
      abortController.abort();
      observer.disconnect();
    };
  }, [document.contentHash, document.id, document.lastImportedAt, generate]);

  if (dataUrl) {
    return (
      <span className="document-grid-thumbnail pptx-generated" ref={containerRef}>
        <img
          src={dataUrl}
          alt=""
          data-thumbnail-kind="pptx"
          draggable={false}
        />
      </span>
    );
  }
  return (
    <span
      className={`pptx-thumbnail-host${failed ? " failed" : ""}`}
      ref={containerRef}
      data-thumbnail-failed={failed ? "true" : undefined}
    >
      <Presentation size={24} aria-hidden="true" />
    </span>
  );
}
