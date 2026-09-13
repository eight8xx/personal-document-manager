import {
  AlertCircle,
  ChevronLeft,
  ChevronRight,
  Maximize2,
  RotateCcw,
  ZoomIn,
  ZoomOut
} from "lucide-react";
import type { Options } from "docx-preview";
import {
  useCallback,
  useEffect,
  useRef,
  useState
} from "react";
import type { CSSProperties } from "react";

import { toBackendError } from "../backend/error";
import { safeExternalUrl } from "../backend/url";
import type {
  BackendClient,
  DocumentPreview,
  DocumentSummary
} from "../backend/types";

type DocxDocumentPreview = Extract<DocumentPreview, { kind: "docx" }>;

const DOCX_MEDIA_TYPE =
  "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
const MIN_SCALE = 0.4;
const MAX_SCALE = 2.5;
const SCALE_STEP = 0.1;

const renderOptions: Partial<Options> = {
  inWrapper: true,
  hideWrapperOnPrint: false,
  ignoreWidth: false,
  ignoreHeight: false,
  ignoreFonts: false,
  breakPages: true,
  debug: false,
  experimental: false,
  className: "docx",
  trimXmlDeclaration: true,
  renderHeaders: true,
  renderFooters: true,
  renderFootnotes: true,
  renderEndnotes: true,
  ignoreLastRenderedPageBreak: false,
  useBase64URL: false,
  renderChanges: false,
  renderComments: false,
  renderAltChunks: false
};

function clampScale(value: number) {
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, value));
}

function blobFromDataUrl(dataUrl: string) {
  const match = /^data:([^;,]+)?(;base64)?,([\s\S]*)$/.exec(dataUrl);
  if (!match) {
    throw new Error("DOCX 数据格式无效。");
  }
  const [, mediaType = DOCX_MEDIA_TYPE, base64Marker, payload] = match;
  if (base64Marker) {
    const binary = window.atob(payload);
    const bytes = new Uint8Array(binary.length);
    for (let index = 0; index < binary.length; index += 1) {
      bytes[index] = binary.charCodeAt(index);
    }
    return new Blob([bytes], {
      type: mediaType || DOCX_MEDIA_TYPE
    });
  }
  return new Blob([decodeURIComponent(payload)], {
    type: mediaType || DOCX_MEDIA_TYPE
  });
}

function isRemoteResource(value: string) {
  return /^(?:https?:)?\/\//i.test(value.trim());
}

function sanitizeCssText(cssText: string) {
  let blocked = false;
  const nextText = cssText
    .replace(/@import[^;]+;/gi, () => {
      blocked = true;
      return "";
    })
    .replace(
      /url\(\s*(['"]?)(?:https?:)?\/\/[^)]*\)/gi,
      () => {
        blocked = true;
        return 'url("")';
      }
    );
  return { blocked, cssText: nextText };
}

function replaceWithBlockedRegion(
  element: Element,
  runtimeDegradations: Set<string>,
  label: string
) {
  const placeholder = document.createElement("span");
  placeholder.className = "docx-blocked-region";
  placeholder.textContent = `[${label}已降级]`;
  element.replaceWith(placeholder);
  runtimeDegradations.add(label);
}

function sanitizeRenderedNode(
  node: Node,
  runtimeDegradations: Set<string>
) {
  if (node.nodeType !== Node.ELEMENT_NODE) {
    return;
  }
  const element = node as Element;

  if (element.tagName === "STYLE") {
    const result = sanitizeCssText(element.textContent ?? "");
    element.textContent = result.cssText;
    if (result.blocked) {
      runtimeDegradations.add("远程资源");
    }
  }

  for (const descendant of Array.from(element.querySelectorAll("*"))) {
    if (["SCRIPT", "IFRAME", "OBJECT", "EMBED"].includes(descendant.tagName)) {
      replaceWithBlockedRegion(
        descendant,
        runtimeDegradations,
        "嵌入对象"
      );
      continue;
    }
    if (descendant.tagName === "LINK") {
      descendant.remove();
      runtimeDegradations.add("远程资源");
      continue;
    }

    const source = descendant.getAttribute("src");
    const sourceSet = descendant.getAttribute("srcset");
    if ((source && isRemoteResource(source)) || (sourceSet && isRemoteResource(sourceSet))) {
      if (descendant.tagName === "IMG") {
        replaceWithBlockedRegion(
          descendant,
          runtimeDegradations,
          "远程图片"
        );
      } else {
        descendant.removeAttribute("src");
        descendant.removeAttribute("srcset");
        runtimeDegradations.add("远程资源");
      }
      continue;
    }

    const style = descendant.getAttribute("style");
    if (style) {
      const result = sanitizeCssText(style);
      if (result.blocked) {
        descendant.setAttribute("style", result.cssText);
        runtimeDegradations.add("远程资源");
      }
    }
  }
}

function scrollToBookmark(anchor: string) {
  const decoded = decodeURIComponent(anchor);
  const target =
    document.getElementById(decoded) ??
    Array.from(document.querySelectorAll<HTMLElement>("a[name]")).find(
      (candidate) => candidate.getAttribute("name") === decoded
    );
  target?.scrollIntoView({ block: "start" });
}

function configureHyperlinks(
  root: HTMLElement,
  client: BackendClient,
  signal: AbortSignal,
  onLinkError: (message: string) => void
) {
  for (const anchor of Array.from(root.querySelectorAll<HTMLAnchorElement>("a"))) {
    const rawHref = anchor.getAttribute("href")?.trim() ?? "";
    anchor.removeAttribute("href");
    anchor.removeAttribute("target");

    const isBookmark = rawHref.startsWith("#") && rawHref.length > 1;
    const externalUrl = isBookmark ? null : safeExternalUrl(rawHref);
    if (!isBookmark && !externalUrl) {
      anchor.classList.add("docx-blocked-link");
      anchor.setAttribute("aria-disabled", "true");
      anchor.title = "已阻止不安全的链接";
      continue;
    }

    anchor.classList.add("docx-safe-link");
    anchor.setAttribute("role", "link");
    anchor.tabIndex = 0;
    const activate = () => {
      if (externalUrl) {
        void client.openExternalUrl(externalUrl).catch((caught) => {
          onLinkError(toBackendError(caught).message);
        });
      } else {
        scrollToBookmark(rawHref.slice(1));
      }
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

function collectObjectUrls(...roots: Array<HTMLElement | null>) {
  const urls = new Set<string>();
  const pattern = /blob:[^"')\s]+/g;
  for (const root of roots) {
    if (!root) {
      continue;
    }
    const values = [root.innerHTML];
    for (const element of Array.from(root.querySelectorAll("*"))) {
      for (const attribute of ["src", "href", "style"]) {
        const value = element.getAttribute(attribute);
        if (value) {
          values.push(value);
        }
      }
    }
    for (const value of values) {
      for (const match of value.matchAll(pattern)) {
        urls.add(match[0]);
      }
    }
  }
  return urls;
}

function revokeObjectUrls(urls: Set<string>) {
  if (typeof URL.revokeObjectURL !== "function") {
    return;
  }
  for (const url of urls) {
    if (url.startsWith("blob:")) {
      URL.revokeObjectURL(url);
    }
  }
}

function pageSections(root: HTMLElement) {
  return Array.from(
    root.querySelectorAll<HTMLElement>(
      ".docx-wrapper > section.docx, .docx-wrapper > section"
    )
  );
}

export function DocxPreview({
  client,
  document,
  preview
}: {
  client: BackendClient;
  document: DocumentSummary;
  preview: DocxDocumentPreview;
}) {
  const bodyRef = useRef<HTMLDivElement | null>(null);
  const styleRef = useRef<HTMLDivElement | null>(null);
  const viewportRef = useRef<HTMLDivElement | null>(null);
  const timerRef = useRef<number | null>(null);
  const renderedObjectUrlsRef = useRef(new Set<string>());
  const [renderError, setRenderError] = useState("");
  const [linkError, setLinkError] = useState("");
  const [renderAttempt, setRenderAttempt] = useState(0);
  const [page, setPage] = useState(1);
  const [pageCount, setPageCount] = useState(1);
  const [zoom, setZoom] = useState(1);
  const [fitWidth, setFitWidth] = useState(true);
  const [fitScale, setFitScale] = useState(1);
  const [runtimeDegradations, setRuntimeDegradations] = useState<string[]>([]);
  const effectiveScale = fitWidth ? fitScale : zoom;
  const degradedFeatures = Array.from(
    new Set([...preview.degradedFeatures, ...runtimeDegradations])
  );

  const updateFitScale = useCallback(() => {
    const viewport = viewportRef.current;
    const body = bodyRef.current;
    if (!viewport || !body) {
      return;
    }
    const section = pageSections(body)[0];
    const inlineWidth = Number.parseFloat(section?.style.width ?? "");
    const measuredWidth = section?.getBoundingClientRect().width ?? 0;
    const naturalWidth =
      Number.isFinite(inlineWidth) && inlineWidth > 0
        ? inlineWidth
        : measuredWidth > 0
          ? measuredWidth
          : 794;
    const availableWidth = Math.max(240, (viewport.clientWidth || 640) - 24);
    setFitScale(clampScale(availableWidth / naturalWidth));
  }, []);

  useEffect(() => {
    let active = true;
    const abortController = new AbortController();
    const body = bodyRef.current;
    const styles = styleRef.current;
    setRenderError("");
    setLinkError("");
    setPage(1);
    setPageCount(1);
    setZoom(1);
    setFitWidth(true);
    setRuntimeDegradations([]);
    body?.replaceChildren();
    styles?.replaceChildren();

    try {
      const blob = blobFromDataUrl(preview.dataUrl);
      void import("docx-preview")
        .then(async ({ parseAsync, renderDocument }) => {
          const wordDocument = await parseAsync(blob, renderOptions);
          return renderDocument(wordDocument, renderOptions);
        })
        .then((nodes) => {
          if (!active || !body || !styles) {
            return;
          }
          const runtimeDegradations = new Set<string>();
          const styleNodes: Node[] = [];
          const bodyNodes: Node[] = [];
          for (const node of nodes) {
            sanitizeRenderedNode(node, runtimeDegradations);
            if (node.nodeName === "STYLE") {
              styleNodes.push(node);
            } else {
              bodyNodes.push(node);
            }
          }
          styles.replaceChildren(...styleNodes);
          body.replaceChildren(...bodyNodes);

          configureHyperlinks(
            body,
            client,
            abortController.signal,
            setLinkError
          );
          const sections = pageSections(body);
          sections.forEach((section, index) => {
            section.dataset.docxPage = String(index + 1);
          });
          setPageCount(Math.max(1, sections.length));
          setRuntimeDegradations([...runtimeDegradations]);
          renderedObjectUrlsRef.current = collectObjectUrls(body, styles);
          updateFitScale();
        })
        .catch((caught) => {
          if (active) {
            setRenderError(
              caught instanceof Error
                ? caught.message
                : "DOCX 版式预览渲染失败。"
            );
          }
        });
    } catch (caught) {
      setRenderError(
        caught instanceof Error ? caught.message : "DOCX 数据格式无效。"
      );
    }

    return () => {
      active = false;
      abortController.abort();
      if (timerRef.current !== null) {
        window.clearTimeout(timerRef.current);
        timerRef.current = null;
      }
      revokeObjectUrls(renderedObjectUrlsRef.current);
      renderedObjectUrlsRef.current = new Set();
      body?.replaceChildren();
      styles?.replaceChildren();
    };
  }, [
    client,
    document.contentHash,
    document.id,
    preview,
    renderAttempt,
    updateFitScale
  ]);

  useEffect(() => {
    if (renderError) {
      return;
    }
    const sections = bodyRef.current ? pageSections(bodyRef.current) : [];
    if (sections.length === 0) {
      return;
    }
    if (timerRef.current !== null) {
      window.clearTimeout(timerRef.current);
    }
    const targetPage = Math.min(page, sections.length);
    const timer = window.setTimeout(() => {
      sections[targetPage - 1]?.scrollIntoView({
        block: "start",
        behavior: "auto"
      });
      timerRef.current = null;
    }, 0);
    timerRef.current = timer;
    return () => {
      window.clearTimeout(timer);
      if (timerRef.current === timer) {
        timerRef.current = null;
      }
    };
  }, [page, pageCount, renderError]);

  useEffect(() => {
    const viewport = viewportRef.current;
    if (!viewport) {
      return;
    }
    updateFitScale();
    if (typeof ResizeObserver === "function") {
      const observer = new ResizeObserver(updateFitScale);
      observer.observe(viewport);
      return () => observer.disconnect();
    }
    window.addEventListener("resize", updateFitScale);
    return () => window.removeEventListener("resize", updateFitScale);
  }, [pageCount, updateFitScale]);

  function changePage(nextPage: number) {
    setPage(Math.min(pageCount, Math.max(1, nextPage)));
  }

  function zoomBy(delta: number) {
    const baseScale = fitWidth ? fitScale : zoom;
    setFitWidth(false);
    setZoom(clampScale(baseScale + delta));
  }

  if (renderError) {
    return (
      <div className="docx-layout-preview">
        <div className="preview-error docx-render-error" role="alert">
          <AlertCircle size={22} aria-hidden="true" />
          <strong>DOCX 版式预览渲染失败</strong>
          <span>{renderError}</span>
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
        </div>
        <div className="docx-fallback-text">
          <strong>提取正文</strong>
          <pre tabIndex={0}>{preview.text}</pre>
        </div>
      </div>
    );
  }

  return (
    <div className="docx-layout-preview">
      <div className="preview-toolbar docx-preview-toolbar">
        <div className="docx-page-navigation">
          <button
            className="icon-button compact"
            type="button"
            onClick={() => changePage(page - 1)}
            disabled={page <= 1}
            aria-label="DOCX 上一页"
            title="上一页"
          >
            <ChevronLeft size={15} aria-hidden="true" />
          </button>
          <span>
            第 {page} / {pageCount} 页
          </span>
          <button
            className="icon-button compact"
            type="button"
            onClick={() => changePage(page + 1)}
            disabled={page >= pageCount}
            aria-label="DOCX 下一页"
            title="下一页"
          >
            <ChevronRight size={15} aria-hidden="true" />
          </button>
          <label className="docx-page-jump">
            <span>跳转</span>
            <input
              type="number"
              min={1}
              max={pageCount}
              value={page}
              aria-label="跳转到 DOCX 页码"
              onChange={(event) => {
                const next = Number.parseInt(event.target.value, 10);
                if (Number.isFinite(next)) {
                  changePage(next);
                }
              }}
            />
          </label>
        </div>
        <div className="docx-zoom-controls">
          <button
            className="icon-button compact"
            type="button"
            onClick={() => zoomBy(-SCALE_STEP)}
            aria-label="缩小 DOCX 预览"
            title="缩小"
          >
            <ZoomOut size={15} aria-hidden="true" />
          </button>
          <button
            className={`button quiet docx-fit-width${
              fitWidth ? " active" : ""
            }`}
            type="button"
            onClick={() => setFitWidth(true)}
            aria-label="适配 DOCX 预览宽度"
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
            aria-label="放大 DOCX 预览"
            title="放大"
          >
            <ZoomIn size={15} aria-hidden="true" />
          </button>
          <span className="docx-zoom-value">
            {Math.round(effectiveScale * 100)}%
          </span>
        </div>
      </div>

      {degradedFeatures.length > 0 ? (
        <p className="preview-notice docx-degradation-notice" role="status">
          部分复杂内容无法完整呈现：{degradedFeatures.join("、")}。其余内容仍可预览。
        </p>
      ) : (
        <p className="preview-notice">{preview.notice}</p>
      )}

      <div className="docx-preview-styles" ref={styleRef} aria-hidden="true" />
      <div
        className={`docx-preview-pages${fitWidth ? " fit-width" : ""}`}
        ref={viewportRef}
      >
        <div
          className="docx-preview-render"
          ref={bodyRef}
          style={
            {
              "--docx-preview-scale": String(effectiveScale)
            } as CSSProperties
          }
        />
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
