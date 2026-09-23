import {
  ArrowDown,
  ArrowUp,
  LoaderCircle,
  Pencil,
  Plus,
  RotateCcw,
  Trash2,
  X
} from "lucide-react";
import { useContext, useEffect, useRef, useState } from "react";

import { importableDocumentTypes } from "../backend/documentFormats";
import { toBackendError } from "../backend/error";
import { LibraryContext } from "../backend/libraryContext";
import type {
  BackendClient,
  ClassificationRule,
  ClassificationRuleOperation,
  CollectionSummary,
  TagSummary
} from "../backend/types";

/** 新建/编辑表单的草稿；与 `ClassificationRuleInput` 同形状。 */
export interface ClassificationRuleDraft {
  name: string;
  enabled: boolean;
  fileNamePattern: string;
  fileType: string | null;
  sourceDirectory: string | null;
  collectionId: string | null;
  tagIds: string[];
}

interface ClassificationRulesPanelProps {
  client: BackendClient;
  collections: CollectionSummary[];
  tags: TagSummary[];
}

const EMPTY_DRAFT: ClassificationRuleDraft = {
  name: "",
  enabled: true,
  fileNamePattern: "",
  fileType: null,
  sourceDirectory: null,
  collectionId: null,
  tagIds: []
};

function draftFromRule(rule: ClassificationRule): ClassificationRuleDraft {
  return {
    name: rule.name,
    enabled: rule.enabled,
    fileNamePattern: rule.fileNamePattern,
    fileType: rule.fileType,
    sourceDirectory: rule.sourceDirectory,
    collectionId: rule.collectionId,
    tagIds: [...rule.tagIds]
  };
}

function collectionName(
  collections: CollectionSummary[],
  collectionId: string | null
) {
  if (!collectionId) {
    return null;
  }
  return (
    collections.find((collection) => collection.id === collectionId)?.name ??
    collectionId
  );
}

function tagNames(tags: TagSummary[], tagIds: string[]) {
  return tagIds.map(
    (tagId) => tags.find((tag) => tag.id === tagId)?.name ?? tagId
  );
}

/** 把一条规则翻译成一句可读的条件描述。 */
export function describeClassificationRule(
  rule: ClassificationRule,
  collections: CollectionSummary[],
  tags: TagSummary[]
) {
  const conditions: string[] = [];
  if (rule.fileNamePattern) {
    conditions.push(`文件名包含“${rule.fileNamePattern}”`);
  }
  if (rule.fileType) {
    conditions.push(`类型为 ${rule.fileType}`);
  }
  if (rule.sourceDirectory) {
    conditions.push(`来源目录 ${rule.sourceDirectory}`);
  }
  const targets: string[] = [];
  const target = collectionName(collections, rule.collectionId);
  targets.push(target ? `归档到「${target}」` : "不改变集合");
  const names = tagNames(tags, rule.tagIds);
  if (names.length > 0) {
    targets.push(`添加标签：${names.join("、")}`);
  }
  const conditionText =
    conditions.length > 0 ? conditions.join("，且") : "匹配所有文件";
  return `${conditionText} → ${targets.join("，")}`;
}

/**
 * 分类规则编辑器（工作单 10）。
 *
 * 规则属于当前资料库：列表按用户的 `position` 顺序展示，可新建、编辑、删除、
 * 启用/停用与上下排序。所有写入都通过 `classificationRuleOperation` 交给后端，
 * 界面只镜像后端返回的规则列表。
 */
export function ClassificationRulesPanel({
  client,
  collections,
  tags
}: ClassificationRulesPanelProps) {
  const library = useContext(LibraryContext);
  const [rules, setRules] = useState<ClassificationRule[]>([]);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [editor, setEditor] = useState<
    { mode: "create" } | { mode: "edit"; ruleId: string } | null
  >(null);
  const [draft, setDraft] = useState<ClassificationRuleDraft>(EMPTY_DRAFT);
  const [formError, setFormError] = useState("");
  const [confirmingDelete, setConfirmingDelete] = useState<string | null>(null);
  const nameInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    let active = true;
    if (!library) {
      setRules([]);
      setLoading(false);
      return () => {
        active = false;
      };
    }
    setLoading(true);
    setError("");
    void client
      .listClassificationRules(library)
      .then((loaded) => {
        if (active) {
          setRules(loaded);
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
  }, [client, library]);

  useEffect(() => {
    if (editor) {
      nameInputRef.current?.focus();
    }
  }, [editor]);

  const orderedRules = [...rules].sort(
    (left, right) => left.position - right.position
  );

  async function reload() {
    if (!library) {
      return;
    }
    setLoading(true);
    setError("");
    try {
      setRules(await client.listClassificationRules(library));
    } catch (caught) {
      setError(toBackendError(caught).message);
    } finally {
      setLoading(false);
    }
  }

  async function runOperation(operation: ClassificationRuleOperation) {
    if (!library) {
      return false;
    }
    setBusy(true);
    setError("");
    try {
      setRules(await client.classificationRuleOperation(library, operation));
      return true;
    } catch (caught) {
      setError(toBackendError(caught).message);
      return false;
    } finally {
      setBusy(false);
    }
  }

  function openCreate() {
    setDraft({ ...EMPTY_DRAFT, tagIds: [] });
    setFormError("");
    setConfirmingDelete(null);
    setEditor({ mode: "create" });
  }

  function openEdit(rule: ClassificationRule) {
    setDraft(draftFromRule(rule));
    setFormError("");
    setConfirmingDelete(null);
    setEditor({ mode: "edit", ruleId: rule.id });
  }

  function closeEditor() {
    setEditor(null);
    setFormError("");
  }

  function toggleDraftTag(tagId: string) {
    setDraft((current) => ({
      ...current,
      tagIds: current.tagIds.includes(tagId)
        ? current.tagIds.filter((candidate) => candidate !== tagId)
        : [...current.tagIds, tagId]
    }));
  }

  async function submitDraft(event: React.FormEvent) {
    event.preventDefault();
    const name = draft.name.trim();
    if (!name) {
      setFormError("规则名称不能为空。");
      return;
    }
    const hasCondition =
      draft.fileNamePattern.trim().length > 0 ||
      Boolean(draft.fileType) ||
      Boolean(draft.sourceDirectory?.trim());
    if (!hasCondition && !draft.collectionId && draft.tagIds.length === 0) {
      setFormError("请至少填写一个匹配条件，或指定目标集合与标签。");
      return;
    }

    const input = {
      name,
      enabled: draft.enabled,
      fileNamePattern: draft.fileNamePattern.trim(),
      fileType: draft.fileType,
      sourceDirectory: draft.sourceDirectory?.trim()
        ? draft.sourceDirectory.trim()
        : null,
      collectionId: draft.collectionId,
      tagIds: draft.tagIds
    };
    const saved =
      editor?.mode === "edit"
        ? await runOperation({
            kind: "update",
            rule: { id: editor.ruleId, ...input }
          })
        : await runOperation({ kind: "create", rule: input });
    if (saved) {
      closeEditor();
    }
  }

  async function toggleEnabled(rule: ClassificationRule, enabled: boolean) {
    await runOperation({ kind: "setEnabled", ruleId: rule.id, enabled });
  }

  async function move(rule: ClassificationRule, offset: -1 | 1) {
    const index = orderedRules.findIndex((candidate) => candidate.id === rule.id);
    const target = index + offset;
    if (index < 0 || target < 0 || target >= orderedRules.length) {
      return;
    }
    const reordered = [...orderedRules];
    const [moved] = reordered.splice(index, 1);
    reordered.splice(target, 0, moved);
    await runOperation({
      kind: "reorder",
      orderedRuleIds: reordered.map((candidate) => candidate.id)
    });
  }

  async function deleteRule(rule: ClassificationRule) {
    const deleted = await runOperation({ kind: "delete", ruleId: rule.id });
    if (deleted) {
      setConfirmingDelete(null);
    }
  }

  return (
    <section className="classification-rules" aria-labelledby="rules-title">
      <div className="section-title-row">
        <div>
          <p className="eyebrow">自动分类</p>
          <h3 id="rules-title">分类规则</h3>
        </div>
        <button
          className="button primary"
          type="button"
          onClick={openCreate}
          disabled={busy || !library}
        >
          <Plus size={15} aria-hidden="true" />
          新建规则
        </button>
      </div>
      <p className="muted-copy">
        新建导入的文档会按顺序匹配规则：目标集合取第一条指定集合的命中规则，
        所有命中规则的标签会合并去重；未命中时进入收件箱。
      </p>

      {loading ? (
        <p className="rules-status" role="status">
          <LoaderCircle className="spin" size={14} aria-hidden="true" />
          正在加载分类规则
        </p>
      ) : null}

      {error ? (
        <p className="dialog-error" role="alert">
          {error}
          <button
            className="button quiet"
            type="button"
            onClick={() => void reload()}
            disabled={busy}
          >
            <RotateCcw size={14} aria-hidden="true" />
            重试
          </button>
        </p>
      ) : null}

      {!loading && orderedRules.length === 0 ? (
        <p className="muted-copy">还没有分类规则。新建规则后，导入的新文档会自动归档。</p>
      ) : null}

      <ul className="classification-rule-list" aria-label="分类规则列表">
        {orderedRules.map((rule, index) => (
          <li className="classification-rule" key={rule.id}>
            <div className="classification-rule-main">
              <div className="classification-rule-title">
                <h4>{rule.name}</h4>
                <span
                  className={`status-badge ${rule.enabled ? "success" : ""}`}
                >
                  {rule.enabled ? "已启用" : "已停用"}
                </span>
                <span className="classification-rule-order">
                  第 {index + 1} 条
                </span>
              </div>
              <p className="classification-rule-conditions">
                {describeClassificationRule(rule, collections, tags)}
              </p>
            </div>

            <div className="classification-rule-actions">
              <label className="classification-rule-toggle">
                <input
                  type="checkbox"
                  checked={rule.enabled}
                  disabled={busy}
                  aria-label={`启用规则 ${rule.name}`}
                  onChange={(event) =>
                    void toggleEnabled(rule, event.target.checked)
                  }
                />
                <span>启用</span>
              </label>
              <button
                className="icon-button compact"
                type="button"
                onClick={() => void move(rule, -1)}
                disabled={busy || index === 0}
                aria-label={`上移规则 ${rule.name}`}
                title="上移"
              >
                <ArrowUp size={14} aria-hidden="true" />
              </button>
              <button
                className="icon-button compact"
                type="button"
                onClick={() => void move(rule, 1)}
                disabled={busy || index === orderedRules.length - 1}
                aria-label={`下移规则 ${rule.name}`}
                title="下移"
              >
                <ArrowDown size={14} aria-hidden="true" />
              </button>
              <button
                className="button quiet"
                type="button"
                onClick={() => openEdit(rule)}
                disabled={busy}
                aria-label={`编辑规则 ${rule.name}`}
              >
                <Pencil size={14} aria-hidden="true" />
                编辑
              </button>
              <button
                className="button danger-quiet"
                type="button"
                onClick={() => setConfirmingDelete(rule.id)}
                disabled={busy}
                aria-label={`删除规则 ${rule.name}`}
              >
                <Trash2 size={14} aria-hidden="true" />
                删除
              </button>
            </div>

            {confirmingDelete === rule.id ? (
              <div className="classification-rule-confirm" role="group" aria-label={`确认删除规则 ${rule.name}`}>
                <span>删除后不再自动分类，已导入的文档不受影响。</span>
                <button
                  className="button danger"
                  type="button"
                  onClick={() => void deleteRule(rule)}
                  disabled={busy}
                >
                  确认删除
                </button>
                <button
                  className="button secondary"
                  type="button"
                  onClick={() => setConfirmingDelete(null)}
                  disabled={busy}
                >
                  取消
                </button>
              </div>
            ) : null}
          </li>
        ))}
      </ul>

      {editor ? (
        <form
          className="classification-rule-editor"
          aria-label={
            editor.mode === "create" ? "新建分类规则" : "编辑分类规则"
          }
          onSubmit={(event) => void submitDraft(event)}
        >
          <div className="section-title-row">
            <h4>{editor.mode === "create" ? "新建规则" : "编辑规则"}</h4>
            <button
              className="icon-button compact"
              type="button"
              onClick={closeEditor}
              disabled={busy}
              aria-label="关闭规则编辑器"
              title="关闭"
            >
              <X size={15} aria-hidden="true" />
            </button>
          </div>

          <div className="field-row">
            <label className="field" htmlFor="classification-name">
              <span>规则名称</span>
              <input
                ref={nameInputRef}
                id="classification-name"
                name="name"
                value={draft.name}
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    name: event.target.value
                  }))
                }
                autoComplete="off"
                disabled={busy}
                maxLength={80}
              />
            </label>
            <label className="field" htmlFor="classification-pattern">
              <span>文件名匹配</span>
              <input
                id="classification-pattern"
                name="fileNamePattern"
                value={draft.fileNamePattern}
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    fileNamePattern: event.target.value
                  }))
                }
                placeholder="留空表示不限"
                autoComplete="off"
                disabled={busy}
              />
            </label>
          </div>

          <div className="field-row">
            <label className="field" htmlFor="classification-type">
              <span>文件类型</span>
              <select
                id="classification-type"
                name="fileType"
                value={draft.fileType ?? ""}
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    fileType: event.target.value || null
                  }))
                }
                disabled={busy}
              >
                <option value="">不限</option>
                {importableDocumentTypes.map((displayType) => (
                  <option key={displayType} value={displayType}>
                    {displayType}
                  </option>
                ))}
              </select>
            </label>
            <label className="field" htmlFor="classification-directory">
              <span>来源目录</span>
              <input
                id="classification-directory"
                name="sourceDirectory"
                value={draft.sourceDirectory ?? ""}
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    sourceDirectory: event.target.value || null
                  }))
                }
                placeholder="留空表示不限"
                autoComplete="off"
                disabled={busy}
              />
            </label>
          </div>

          <label className="field" htmlFor="classification-collection">
            <span>目标集合</span>
            <select
              id="classification-collection"
              name="collectionId"
              value={draft.collectionId ?? ""}
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  collectionId: event.target.value || null
                }))
              }
              disabled={busy}
            >
              <option value="">不指定（仅应用标签）</option>
              {collections.map((collection) => (
                <option key={collection.id} value={collection.id}>
                  {collection.name}
                </option>
              ))}
            </select>
          </label>

          <fieldset className="tag-picker" disabled={busy}>
            <legend>标签</legend>
            {tags.length === 0 ? (
              <p className="tag-picker-empty">暂无标签，请先在侧栏创建。</p>
            ) : (
              <div className="tag-picker-options">
                {tags.map((tag) => (
                  <label className="tag-picker-option" key={tag.id}>
                    <input
                      type="checkbox"
                      checked={draft.tagIds.includes(tag.id)}
                      onChange={() => toggleDraftTag(tag.id)}
                    />
                    <span>{tag.name}</span>
                  </label>
                ))}
              </div>
            )}
          </fieldset>

          <label className="classification-rule-toggle">
            <input
              type="checkbox"
              checked={draft.enabled}
              disabled={busy}
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  enabled: event.target.checked
                }))
              }
            />
            <span>启用这条规则</span>
          </label>

          {formError ? (
            <p className="dialog-error" role="alert">
              {formError}
            </p>
          ) : null}

          <div className="dialog-actions">
            <button
              className="button secondary"
              type="button"
              onClick={closeEditor}
              disabled={busy}
            >
              取消
            </button>
            <button className="button primary" type="submit" disabled={busy}>
              {busy ? (
                <LoaderCircle className="spin" size={15} aria-hidden="true" />
              ) : (
                <Plus size={15} aria-hidden="true" />
              )}
              保存规则
            </button>
          </div>
        </form>
      ) : null}
    </section>
  );
}
