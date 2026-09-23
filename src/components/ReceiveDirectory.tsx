import {
  FolderSearch,
  LoaderCircle,
  Play,
  RotateCcw,
  ShieldAlert,
  Upload
} from "lucide-react";
import { useContext, useEffect, useState } from "react";

import { toBackendError } from "../backend/error";
import { LibraryContext } from "../backend/libraryContext";
import type {
  BackendClient,
  ReceiveDirectoryListingItem,
  ReceiveSource,
  ReceiveSourceCandidate,
  ReceiveSourceKind,
  ReceiveSourceScanResult
} from "../backend/types";

/** 界面按来源分别配置；QQ 与微信互相独立。 */
export const RECEIVE_SOURCE_KINDS: ReceiveSourceKind[] = ["qq", "wechat"];

export const RECEIVE_SOURCE_LABELS: Record<ReceiveSourceKind, string> = {
  qq: "QQ",
  wechat: "微信",
  other: "其他"
};

const STATUS_PRESENTATION: Record<
  ReceiveSource["status"],
  { label: string; tone: string }
> = {
  unconfigured: { label: "未配置", tone: "" },
  ready: { label: "目录可用", tone: "success" },
  missing: { label: "目录不存在", tone: "danger" },
  unreadable: { label: "目录无法访问", tone: "danger" }
};

interface CandidatePanel {
  kind: ReceiveSourceKind;
  /** 首次启用或目录失效后重新确认。 */
  intent: "enable" | "relocate";
  candidates: ReceiveSourceCandidate[];
}

interface DirectoryListingState {
  kind: ReceiveSourceKind;
  sourceId: string;
  path: string;
  items: ReceiveDirectoryListingItem[];
  selected: string[];
}

interface ReceiveDirectoryPanelProps {
  client: BackendClient;
}

function formatTimestamp(value: string | null) {
  if (!value) {
    return "尚未扫描";
  }
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return date.toLocaleString("zh-CN", { hour12: false });
}

/**
 * 每库接收目录配置（工作单 11/12）。
 *
 * QQ 与微信两个来源各自显示候选目录、识别依据、启用开关与扫描状态；首次启用
 * 或重新定位后必须由用户确认目录，才登记目录里已有的文件清单；用户未勾选的
 * 文件会记为“已跳过”，后续补扫不会自动导入。目录失效时只提示重新确认，绝不
 * 静默换目录。
 */
export function ReceiveDirectoryPanel({ client }: ReceiveDirectoryPanelProps) {
  const library = useContext(LibraryContext);
  const [sources, setSources] = useState<ReceiveSource[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [busyKind, setBusyKind] = useState<ReceiveSourceKind | null>(null);
  const [candidatePanel, setCandidatePanel] = useState<CandidatePanel | null>(
    null
  );
  const [manualPath, setManualPath] = useState("");
  const [listing, setListing] = useState<DirectoryListingState | null>(null);
  const [scanResults, setScanResults] = useState<
    Partial<Record<ReceiveSourceKind, ReceiveSourceScanResult>>
  >({});
  const [reloadToken, setReloadToken] = useState(0);

  useEffect(() => {
    let active = true;
    if (!library) {
      setSources([]);
      setLoading(false);
      return () => {
        active = false;
      };
    }
    setLoading(true);
    setError("");
    void client
      .listReceiveSources(library)
      .then((loaded) => {
        if (active) {
          setSources(loaded);
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
  }, [client, library, reloadToken]);

  function sourceFor(kind: ReceiveSourceKind) {
    return sources.find((source) => source.kind === kind) ?? null;
  }

  async function openCandidates(
    kind: ReceiveSourceKind,
    intent: CandidatePanel["intent"]
  ) {
    if (!library) {
      return;
    }
    setBusyKind(kind);
    setError("");
    setManualPath("");
    try {
      const response = await client.listReceiveSourceCandidates(library, kind);
      setCandidatePanel({ kind, intent, candidates: response.candidates });
    } catch (caught) {
      setError(toBackendError(caught).message);
    } finally {
      setBusyKind(null);
    }
  }

  function closeCandidates() {
    setCandidatePanel(null);
    setManualPath("");
  }

  async function openListingFor(
    kind: ReceiveSourceKind,
    source: ReceiveSource | null
  ) {
    if (!library || !source?.path) {
      return;
    }
    setBusyKind(kind);
    setError("");
    try {
      const items = await client.listReceiveDirectoryFiles(library, source.id);
      setListing({
        kind,
        sourceId: source.id,
        path: items.path,
        items: items.items,
        // 默认全不勾选：用户未选择的文件会在确认时记为“已跳过”。
        selected: []
      });
    } catch (caught) {
      setError(toBackendError(caught).message);
    } finally {
      setBusyKind(null);
    }
  }

  async function openListing(kind: ReceiveSourceKind) {
    await openListingFor(kind, sourceFor(kind));
  }

  async function saveSource(
    kind: ReceiveSourceKind,
    path: string,
    enabled: boolean,
    options: { openListingAfter?: boolean } = {}
  ) {
    if (!library) {
      return null;
    }
    const source = sourceFor(kind);
    setBusyKind(kind);
    setError("");
    try {
      const next = await client.upsertReceiveSource(
        library,
        source?.id ?? null,
        {
          kind,
          displayName: source?.displayName ?? RECEIVE_SOURCE_LABELS[kind],
          path,
          enabled
        }
      );
      setSources(next);
      closeCandidates();
      return next;
    } catch (caught) {
      setError(toBackendError(caught).message);
      return null;
    } finally {
      setBusyKind(null);
    }
  }

  async function confirmCandidate(kind: ReceiveSourceKind, path: string) {
    if (!path.trim()) {
      setError("请先填写接收目录路径。");
      return;
    }
    const saved = await saveSource(kind, path.trim(), true);
    if (saved) {
      // 首次确认目录后立即列出目录里已有的受支持文件。
      await openListingFor(
        kind,
        saved.find((source) => source.kind === kind) ?? null
      );
    }
  }

  async function toggleSource(kind: ReceiveSourceKind, enabled: boolean) {
    const source = sourceFor(kind);
    if (!source?.path) {
      // 还没确认过目录：先让用户确认候选目录，不静默使用任何默认路径。
      await openCandidates(kind, "enable");
      return;
    }
    const saved = await saveSource(kind, source.path, enabled);
    if (saved && enabled && !source.lastScannedAt) {
      // 首次启用：列出目录里已有的受支持文件，由用户决定导入哪些。
      await openListingFor(
        kind,
        saved.find((candidate) => candidate.kind === kind) ?? null
      );
    }
  }

  async function scan(kind: ReceiveSourceKind) {
    const source = sourceFor(kind);
    if (!library || !source) {
      return;
    }
    setBusyKind(kind);
    setError("");
    try {
      const results = await client.scanReceiveSources(library);
      const result = results.find((entry) => entry.sourceId === source.id);
      if (result) {
        setScanResults((current) => ({ ...current, [kind]: result }));
      }
      setSources(await client.listReceiveSources(library));
    } catch (caught) {
      setError(toBackendError(caught).message);
    } finally {
      setBusyKind(null);
    }
  }

  function toggleSelection(path: string) {
    setListing((current) => {
      if (!current) {
        return current;
      }
      return {
        ...current,
        selected: current.selected.includes(path)
          ? current.selected.filter((candidate) => candidate !== path)
          : [...current.selected, path]
      };
    });
  }

  async function applySelection(kind: ReceiveSourceKind) {
    if (!library || !listing) {
      return;
    }
    const selected = new Set(listing.selected);
    const unselected = listing.items
      .filter((item) => !selected.has(item.path))
      .map((item) => item.path);
    setBusyKind(kind);
    setError("");
    try {
      if (unselected.length > 0) {
        // 未勾选的文件必须记为“已跳过”，后续补扫才不会自动导入。
        await client.skipReceiveDirectoryFiles(library, {
          sourceId: listing.sourceId,
          paths: unselected
        });
      }
      const result = await client.applyReceiveDirectorySelection(library, {
        sourceId: listing.sourceId,
        paths: listing.selected
      });
      setScanResults((current) => ({ ...current, [kind]: result }));
      setListing(null);
      setSources(await client.listReceiveSources(library));
    } catch (caught) {
      setError(toBackendError(caught).message);
    } finally {
      setBusyKind(null);
    }
  }

  async function skipAll(kind: ReceiveSourceKind) {
    if (!library || !listing) {
      return;
    }
    setBusyKind(kind);
    setError("");
    try {
      const next = await client.skipReceiveDirectoryFiles(library, {
        sourceId: listing.sourceId,
        paths: listing.items.map((item) => item.path)
      });
      setSources(next);
      setListing(null);
    } catch (caught) {
      setError(toBackendError(caught).message);
    } finally {
      setBusyKind(null);
    }
  }

  return (
    <section className="receive-directory" aria-labelledby="receive-title">
      <div className="section-title-row">
        <div>
          <p className="eyebrow">自动接收</p>
          <h3 id="receive-title">接收目录</h3>
        </div>
      </div>
      <p className="muted-copy">
        每个资料库分别配置 QQ 与微信的接收目录。只有已启用的来源会在打开资料库时补扫；
        关闭应用不会后台监视。
      </p>

      {loading ? (
        <p className="rules-status" role="status">
          <LoaderCircle className="spin" size={14} aria-hidden="true" />
          正在加载接收来源
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
            重试
          </button>
        </p>
      ) : null}

      {RECEIVE_SOURCE_KINDS.map((kind) => {
        const label = RECEIVE_SOURCE_LABELS[kind];
        const source = sourceFor(kind);
        const status = STATUS_PRESENTATION[source?.status ?? "unconfigured"];
        const busy = busyKind === kind;
        const invalid = source?.status === "missing" || source?.status === "unreadable";
        const candidatesOpen =
          candidatePanel?.kind === kind ? candidatePanel : null;
        const activeListing = listing?.kind === kind ? listing : null;
        const scanResult = scanResults[kind];

        return (
          <div
            className="receive-source"
            key={kind}
            role="group"
            aria-labelledby={`receive-${kind}-title`}
          >
            <div className="section-title-row">
              <h4 id={`receive-${kind}-title`}>{label}</h4>
              <span className={`status-badge ${status.tone}`}>
                {status.label}
              </span>
            </div>
            <p className="receive-source-path" title={source?.path ?? undefined}>
              {source?.path ?? "尚未配置接收目录"}
            </p>
            {source?.statusMessage ? (
              <p className="receive-source-message">{source.statusMessage}</p>
            ) : null}
            {invalid ? (
              <p className="receive-source-warning" role="alert">
                <ShieldAlert size={14} aria-hidden="true" />
                目录失效：{source?.statusMessage ?? "目录不存在或无法访问"}
                。请重新确认目录，确认前不会改用其他目录。
              </p>
            ) : null}

            <label className="receive-source-toggle">
              <input
                type="checkbox"
                checked={source?.enabled ?? false}
                disabled={busy}
                aria-label={`启用 ${label} 接收目录`}
                onChange={(event) =>
                  void toggleSource(kind, event.target.checked)
                }
              />
              <span>启用自动接收</span>
            </label>

            <p className="muted-copy">
              待处理 {source?.pendingCount ?? 0} 个文件 ·{" "}
              {formatTimestamp(source?.lastScannedAt ?? null)}
            </p>

            <div className="row-actions">
              <button
                className="button quiet"
                type="button"
                onClick={() => void scan(kind)}
                disabled={busy || !source?.path || !source.enabled}
                aria-label={`扫描 ${label} 接收目录`}
              >
                {busy ? (
                  <LoaderCircle className="spin" size={14} aria-hidden="true" />
                ) : (
                  <Play size={14} aria-hidden="true" />
                )}
                立即扫描
              </button>
              <button
                className="button secondary"
                type="button"
                onClick={() => void openListing(kind)}
                disabled={busy || !source?.path}
                aria-label={`打开 ${label} 文件清单`}
              >
                <FolderSearch size={14} aria-hidden="true" />
                {source?.lastScannedAt ? "补选文件" : "确认目录内文件"}
              </button>
              {invalid ? (
                <button
                  className="button danger-quiet"
                  type="button"
                  onClick={() => void openCandidates(kind, "relocate")}
                  disabled={busy}
                  aria-label={`重新确认 ${label} 接收目录`}
                >
                  重新确认目录
                </button>
              ) : null}
            </div>

            {candidatesOpen ? (
              <div
                className="receive-candidates"
                role="group"
                aria-label={`${label} 候选目录`}
              >
                <p className="muted-copy">
                  {candidatesOpen.intent === "relocate"
                    ? "请选择新的接收目录；确认前仍保留原目录，不会自动切换。"
                    : "请先确认接收目录，确认后才会列出目录内的文件。"}
                </p>
                {candidatesOpen.candidates.length === 0 ? (
                  <p className="muted-copy">
                    没有自动识别到候选目录，请手动填写路径。
                  </p>
                ) : (
                  <ul className="receive-candidate-list">
                    {candidatesOpen.candidates.map((candidate) => (
                      <li key={candidate.path}>
                        <div>
                          <p className="receive-candidate-path">
                            {candidate.path}
                          </p>
                          <p className="receive-candidate-evidence">
                            识别依据：{candidate.evidence}
                          </p>
                        </div>
                        <button
                          className="button secondary"
                          type="button"
                          onClick={() =>
                            void confirmCandidate(kind, candidate.path)
                          }
                          disabled={busy}
                          aria-label={`使用候选目录 ${candidate.path}`}
                        >
                          使用此目录
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
                <div className="receive-manual-path">
                  <label className="field" htmlFor={`receive-${kind}-manual`}>
                    <span>手动填写目录</span>
                    <input
                      id={`receive-${kind}-manual`}
                      value={manualPath}
                      onChange={(event) => setManualPath(event.target.value)}
                      autoComplete="off"
                      disabled={busy}
                    />
                  </label>
                  <button
                    className="button quiet"
                    type="button"
                    onClick={() => void confirmCandidate(kind, manualPath)}
                    disabled={busy || !manualPath.trim()}
                  >
                    使用该目录
                  </button>
                  <button
                    className="button quiet"
                    type="button"
                    onClick={closeCandidates}
                    disabled={busy}
                  >
                    取消
                  </button>
                </div>
              </div>
            ) : null}

            {activeListing ? (
              <div
                className="receive-listing"
                role="group"
                aria-label={`${label} 目录文件清单`}
              >
                <p className="muted-copy">
                  目录 {activeListing.path} 下有 {activeListing.items.length}{" "}
                  个受支持文件。未勾选的文件会记为“已跳过”，后续补扫不会自动导入。
                </p>
                {activeListing.items.length === 0 ? (
                  <p className="muted-copy" role="status">
                    目录里没有可导入的文件。
                  </p>
                ) : (
                  <>
                    <div className="row-actions">
                      <button
                        className="button quiet"
                        type="button"
                        onClick={() =>
                          setListing((current) =>
                            current
                              ? {
                                  ...current,
                                  selected: current.items.map(
                                    (item) => item.path
                                  )
                                }
                              : current
                          )
                        }
                        disabled={busy}
                      >
                        全选
                      </button>
                      <button
                        className="button quiet"
                        type="button"
                        onClick={() =>
                          setListing((current) =>
                            current ? { ...current, selected: [] } : current
                          )
                        }
                        disabled={busy}
                      >
                        全不选
                      </button>
                    </div>
                    <ul className="receive-listing-items">
                      {activeListing.items.map((item) => (
                        <li key={item.path}>
                          <label>
                            <input
                              type="checkbox"
                              checked={activeListing.selected.includes(
                                item.path
                              )}
                              disabled={busy}
                              aria-label={`选择 ${item.fileName}`}
                              onChange={() => toggleSelection(item.path)}
                            />
                            <span className="receive-listing-name">
                              {item.fileName}
                            </span>
                            <span className="muted-copy">
                              {item.fileType ?? "未知"}
                            </span>
                            {item.previouslySkipped ? (
                              <span className="status-badge warning">
                                此前已跳过
                              </span>
                            ) : null}
                          </label>
                        </li>
                      ))}
                    </ul>
                  </>
                )}
                <div className="dialog-actions">
                  {activeListing.items.length > 0 ? (
                    <>
                      <button
                        className="button secondary"
                        type="button"
                        onClick={() => void skipAll(kind)}
                        disabled={busy}
                      >
                        全部跳过
                      </button>
                      <button
                        className="button primary"
                        type="button"
                        onClick={() => void applySelection(kind)}
                        disabled={busy || activeListing.selected.length === 0}
                      >
                        <Upload size={14} aria-hidden="true" />
                        导入所选（{activeListing.selected.length}）
                      </button>
                    </>
                  ) : (
                    <button
                      className="button secondary"
                      type="button"
                      onClick={() => setListing(null)}
                      disabled={busy}
                    >
                      关闭清单
                    </button>
                  )}
                </div>
              </div>
            ) : null}

            {scanResult ? (
              <p className="receive-scan-result" role="status">
                扫描完成：检查 {scanResult.scannedCount} 个，导入{" "}
                {scanResult.importedCount} 个，跳过 {scanResult.skippedCount}{" "}
                个，待处理 {scanResult.pendingCount} 个，失败{" "}
                {scanResult.failedCount} 个。
              </p>
            ) : null}
          </div>
        );
      })}
    </section>
  );
}
