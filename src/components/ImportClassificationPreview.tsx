import {
  ChevronLeft,
  ChevronRight,
  ListChecks,
  LoaderCircle,
  RotateCcw,
  X
} from "lucide-react";
import { useContext, useEffect, useState } from "react";

import { toBackendError } from "../backend/error";
import { LibraryContext } from "../backend/libraryContext";
import type {
  BackendClient,
  ClassificationPreviewItem,
  CollectionSummary,
  TagSummary
} from "../backend/types";

/** 每页展示的预览项数量；预览一次性返回全部条目，界面自己分页。 */
export const CLASSIFICATION_PREVIEW_PAGE_SIZE = 10;

export type ImportClassificationDecision = "applyRules" | "keepExisting";

interface ImportClassificationPreviewProps {
  client: BackendClient;
  /** 本批待导入的源文件路径。 */
  paths: string[];
  /** 用户显式指定的目标集合；优先于规则集合，但规则标签仍生效。 */
  targetCollectionId: string | null;
  collections: CollectionSummary[];
  tags: TagSummary[];
  onDecide: (decision: ImportClassificationDecision) => void;
  onCancel: () => void;
}

function nameOf(
  items: { id: string; name: string }[],
  id: string
): string {
  return items.find((item) => item.id === id)?.name ?? id;
}

function collectionLabel(
  collections: CollectionSummary[],
  collectionId: string
) {
  return nameOf(collections, collectionId);
}

/**
 * 人工批量导入前的「是否应用分类规则」询问（工作单 10）。
 *
 * 一次展示整批文件的预计集合、标签与命中规则并分页浏览，不逐项弹窗；
 * 组件本身不导入任何文件，只把用户的选择交回调用方。
 */
export function ImportClassificationPreview({
  client,
  paths,
  targetCollectionId,
  collections,
  tags,
  onDecide,
  onCancel
}: ImportClassificationPreviewProps) {
  const library = useContext(LibraryContext);
  const [items, setItems] = useState<ClassificationPreviewItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [page, setPage] = useState(0);
  const [reloadToken, setReloadToken] = useState(0);
  const requestKey = `${targetCollectionId ?? ""}\n${paths.join("\n")}`;

  useEffect(() => {
    let active = true;
    setPage(0);
    if (!library || paths.length === 0) {
      setItems([]);
      setLoading(false);
      return () => {
        active = false;
      };
    }
    setLoading(true);
    setError("");
    void client
      .previewClassification(library, { paths, targetCollectionId })
      .then((response) => {
        if (active) {
          setItems(response.items);
        }
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
    // paths/targetCollectionId 用 requestKey 参与依赖，避免调用方每次渲染都重新请求。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, library, requestKey, reloadToken]);

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        onCancel();
      }
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [onCancel]);

  const pageCount = Math.max(
    1,
    Math.ceil(items.length / CLASSIFICATION_PREVIEW_PAGE_SIZE)
  );
  const currentPage = Math.min(page, pageCount - 1);
  const visible = items.slice(
    currentPage * CLASSIFICATION_PREVIEW_PAGE_SIZE,
    currentPage * CLASSIFICATION_PREVIEW_PAGE_SIZE +
      CLASSIFICATION_PREVIEW_PAGE_SIZE
  );
  const explicitTarget = targetCollectionId
    ? collectionLabel(collections, targetCollectionId)
    : null;

  return (
    <div className="dialog-backdrop" role="presentation">
      <section
        className="classification-preview-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="classification-preview-title"
      >
        <header className="dialog-header">
          <div>
            <p className="eyebrow">批量导入</p>
            <h2 id="classification-preview-title">是否应用分类规则？</h2>
          </div>
          <button
            className="icon-button"
            type="button"
            onClick={onCancel}
            aria-label="关闭分类预览"
            title="关闭"
          >
            <X size={18} aria-hidden="true" />
          </button>
        </header>

        <div className="dialog-body classification-preview-body">
          <p className="muted-copy">
            共 {items.length} 个文件。确认后将按规则归档并应用标签；选择“不应用”则保持原有导入方式。
          </p>
          {explicitTarget ? (
            <p className="classification-preview-target" role="note">
              已指定目标集合「{explicitTarget}」，它优先于规则集合；命中的规则标签仍会应用。
            </p>
          ) : null}

          {loading ? (
            <p className="rules-status" role="status">
              <LoaderCircle className="spin" size={14} aria-hidden="true" />
              正在计算分类结果
            </p>
          ) : null}

          {error ? (
            <p className="dialog-error" role="alert">
              {error}
              <button
                className="button quiet"
                type="button"
                onClick={() => setReloadToken((value) => value + 1)}
              >
                <RotateCcw size={14} aria-hidden="true" />
                重试预览
              </button>
            </p>
          ) : null}

          {!loading && !error && items.length === 0 ? (
            <p className="muted-copy" role="status">
              没有待导入的文件。
            </p>
          ) : null}

          {items.length > 0 ? (
            <div className="classification-preview-scroll">
              <table>
                <caption>分类预览</caption>
                <thead>
                  <tr>
                    <th scope="col">文件名</th>
                    <th scope="col">类型</th>
                    <th scope="col">预计集合</th>
                    <th scope="col">标签</th>
                    <th scope="col">命中规则</th>
                  </tr>
                </thead>
                <tbody>
                  {visible.map((item) => (
                    <tr key={item.sourcePath}>
                      <th scope="row" title={item.sourcePath}>
                        {item.fileName}
                      </th>
                      <td>{item.fileType ?? "未知"}</td>
                      <td>{collectionLabel(collections, item.collectionId)}</td>
                      <td>
                        {item.tagIds.length > 0
                          ? item.tagIds
                              .map((tagId) => nameOf(tags, tagId))
                              .join("、")
                          : "无标签"}
                      </td>
                      <td>
                        {item.matchedRuleIds.length > 0
                          ? item.matchedRuleIds.join("、")
                          : "未命中规则"}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : null}

          {items.length > CLASSIFICATION_PREVIEW_PAGE_SIZE ? (
            <div className="classification-preview-pager">
              <span>
                第 {currentPage + 1} / {pageCount} 页
              </span>
              <button
                className="icon-button compact"
                type="button"
                onClick={() => setPage(Math.max(0, currentPage - 1))}
                disabled={currentPage === 0}
                aria-label="分类预览上一页"
                title="上一页"
              >
                <ChevronLeft size={15} aria-hidden="true" />
              </button>
              <button
                className="icon-button compact"
                type="button"
                onClick={() =>
                  setPage(Math.min(pageCount - 1, currentPage + 1))
                }
                disabled={currentPage >= pageCount - 1}
                aria-label="分类预览下一页"
                title="下一页"
              >
                <ChevronRight size={15} aria-hidden="true" />
              </button>
            </div>
          ) : null}

          <div className="dialog-actions">
            <button
              className="button secondary"
              type="button"
              onClick={() => onDecide("keepExisting")}
            >
              不应用，按原有方式导入
            </button>
            <button
              className="button primary"
              type="button"
              onClick={() => onDecide("applyRules")}
              disabled={loading || Boolean(error) || items.length === 0}
            >
              <ListChecks size={15} aria-hidden="true" />
              应用分类规则并导入
            </button>
          </div>
        </div>
      </section>
    </div>
  );
}
