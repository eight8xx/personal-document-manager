import { createContext, useCallback, useContext, useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";

export interface UpdateResult {
  currentVersion: string;
  update: { version: string; notes: string; releaseUrl: string; downloadUrl: string | null } | null;
}
export interface UpdateClient {
  enabled: boolean;
  check: () => Promise<UpdateResult>;
  open: () => Promise<void>;
}
const nativeClient: UpdateClient = {
  get enabled() { return isTauri(); },
  check: () => invoke<UpdateResult>("check_app_update"),
  open: () => invoke<void>("open_app_update")
};
interface UpdateState {
  checking: boolean; opening: boolean; result: UpdateResult | null; error: string;
  check: () => Promise<void>; open: () => Promise<void>;
}
const UpdateContext = createContext<UpdateState | null>(null);
function message(error: unknown) {
  return error instanceof Error ? error.message : typeof error === "string" ? error : "检查更新失败，请稍后重试";
}

export function AppUpdateProvider({ children, client = nativeClient }: { children: ReactNode; client?: UpdateClient }) {
  const [checking, setChecking] = useState(false);
  const [opening, setOpening] = useState(false);
  const [result, setResult] = useState<UpdateResult | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState(false);
  const [noticeError, setNoticeError] = useState("");
  const active = useRef(false);
  const inFlight = useRef(false);
  const openingRef = useRef(false);
  const dismissed = useRef("");
  const check = useCallback(async (manual = true) => {
    if (inFlight.current) return;
    if (!client.enabled) { setError("请在桌面应用中检查更新。"); return; }
    inFlight.current = true;
    setChecking(true); setError("");
    try {
      const next = await client.check();
      if (!active.current) return;
      setResult(next);
      if (next.update && (manual || dismissed.current !== next.update.version)) {
        setNotice(true); setNoticeError("");
      } else if (!next.update) setNotice(false);
    } catch (caught) {
      if (active.current) setError(message(caught));
    } finally {
      inFlight.current = false;
      if (active.current) setChecking(false);
    }
  }, [client]);
  useEffect(() => {
    active.current = true;
    if (client.enabled) void check(false);
    const timer = window.setInterval(() => { if (client.enabled) void check(false); }, 24 * 60 * 60 * 1000);
    return () => { active.current = false; window.clearInterval(timer); };
  }, [check, client]);
  async function open() {
    if (openingRef.current) return;
    openingRef.current = true;
    setOpening(true); setError(""); setNoticeError("");
    try {
      await client.open();
      if (active.current) {
        dismissed.current = result?.update?.version ?? "";
        setNotice(false);
      }
    } catch (caught) {
      if (active.current) { setError(message(caught)); setNoticeError(message(caught)); }
    } finally {
      openingRef.current = false;
      if (active.current) setOpening(false);
    }
  }
  return <UpdateContext.Provider value={{ checking, opening, result, error, check: () => check(true), open }}>
    {children}
    {notice && result?.update ? <section className="update-notice" aria-label="发现应用更新">
      <h2>发现新版本 {result.update.version}</h2>
      <p>当前版本 {result.currentVersion}，是否更新？</p>
      <p className="muted-copy">将在浏览器中打开 GitHub 下载。下载后关闭应用，再运行安装包；使用便携版时替换程序。</p>
      {result.update.notes ? <pre className="update-notes">{result.update.notes}</pre> : null}
      {noticeError ? <p role="alert">{noticeError}</p> : null}
      <div className="row-actions">
        <button className="button quiet" disabled={opening} onClick={() => {
          dismissed.current = result.update!.version; setNotice(false);
        }}>稍后再说</button>
        <button className="button primary" disabled={opening} onClick={() => void open()}>
          {opening ? "正在打开…" : result.update.downloadUrl ? "下载更新" : "查看新版发布"}
        </button>
      </div>
    </section> : null}
  </UpdateContext.Provider>;
}

export function AppUpdateSettings() {
  const state = useContext(UpdateContext);
  if (!state) return null;
  return <div className="settings-section" role="tabpanel">
    <h3>应用更新</h3>
    <p className="muted-copy">启动时及每隔 24 小时自动检查 GitHub 正式发布，发现新版后询问是否下载。</p>
    {state.result ? <p>当前版本：{state.result.currentVersion}</p> : null}
    {state.error ? <p role="alert">{state.error}</p> : null}
    <p role="status">{state.checking ? "正在检查更新…" : state.result ?
      state.result.update ? `可用版本：${state.result.update.version}` : "当前已是最新正式版本。" : ""}</p>
    <button className="button secondary" disabled={state.checking || state.opening} onClick={() => void state.check()}>检查更新</button>
    {state.result?.update ? <button className="button primary" disabled={state.checking || state.opening} onClick={() => void state.open()}>
      {state.opening ? "正在打开…" : "下载更新"}
    </button> : null}
  </div>;
}
