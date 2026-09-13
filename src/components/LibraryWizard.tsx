import {
  ArrowLeft,
  Cloud,
  FolderOpen,
  HardDrive,
  LoaderCircle,
  ShieldAlert
} from "lucide-react";
import { useState } from "react";

import { toBackendError } from "../backend/client";
import type {
  BackendClient,
  LibraryLocationInspection,
  LibrarySummary
} from "../backend/types";

interface LibraryWizardProps {
  client: BackendClient;
  onCreated: (library: LibrarySummary) => void;
  onOpened: (library: LibrarySummary) => void;
}

type WizardStep = "choose" | "review" | "cloudWarning";

export function LibraryWizard({
  client,
  onCreated,
  onOpened
}: LibraryWizardProps) {
  const [step, setStep] = useState<WizardStep>("choose");
  const [selectedPath, setSelectedPath] = useState("");
  const [inspection, setInspection] =
    useState<LibraryLocationInspection | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  async function chooseDirectory() {
    setError("");

    try {
      const path = await client.pickLibraryDirectory();
      if (!path) {
        return;
      }

      setBusy(true);
      setSelectedPath(path);
      const result = await client.inspectLibraryLocation(path);
      setInspection(result);

      if (result.status === "blocked") {
        setError(result.reason ?? "该位置不能用作资料库。");
        setStep("choose");
        return;
      }

      if (result.cloudSyncWarning) {
        setStep("cloudWarning");
        return;
      }

      setStep("review");
    } catch (caught) {
      setError(toBackendError(caught).message);
    } finally {
      setBusy(false);
    }
  }

  async function commit(action: "create" | "open") {
    if (!inspection) {
      return;
    }

    setBusy(true);
    setError("");

    try {
      const library =
        action === "create"
          ? await client.createLibrary(inspection.path)
          : await client.openLibrary(inspection.path);

      if (action === "create") {
        onCreated(library);
      } else {
        onOpened(library);
      }
    } catch (caught) {
      setError(toBackendError(caught).message);
    } finally {
      setBusy(false);
    }
  }

  if (step === "cloudWarning" && inspection?.cloudSyncWarning) {
    return (
      <main className="onboarding-shell">
        <section className="onboarding-panel" aria-labelledby="cloud-title">
          <div className="wizard-icon warning-icon">
            <Cloud aria-hidden="true" />
          </div>
          <p className="eyebrow">同步目录提醒</p>
          <h1 id="cloud-title">这个位置可能由云盘同步</h1>
          <p className="wizard-lead">
            {inspection.cloudSyncWarning.message}
          </p>

          <div className="warning-summary">
            <ShieldAlert aria-hidden="true" />
            <div>
              <strong>{inspection.cloudSyncWarning.provider}</strong>
              <span>{inspection.path}</span>
            </div>
          </div>

          {error ? <p className="error-text">{error}</p> : null}

          <div className="wizard-actions">
            <button
              className="button secondary"
              type="button"
              onClick={() => {
                setStep("choose");
                setInspection(null);
              }}
              disabled={busy}
            >
              <ArrowLeft size={17} aria-hidden="true" />
              返回修改位置
            </button>
            <button
              className="button primary"
              type="button"
              onClick={() =>
                void commit(
                  inspection.isExistingLibrary ? "open" : "create"
                )
              }
              disabled={busy}
            >
              {busy ? (
                <LoaderCircle className="spin" size={17} aria-hidden="true" />
              ) : (
                <HardDrive size={17} aria-hidden="true" />
              )}
              {inspection.isExistingLibrary ? "仍然打开" : "仍然在此创建"}
            </button>
          </div>
        </section>
      </main>
    );
  }

  if (step === "review" && inspection) {
    return (
      <main className="onboarding-shell">
        <section className="onboarding-panel" aria-labelledby="review-title">
          <div className="wizard-icon">
            {inspection.isExistingLibrary ? (
              <FolderOpen aria-hidden="true" />
            ) : (
              <HardDrive aria-hidden="true" />
            )}
          </div>
          <p className="eyebrow">
            {inspection.isExistingLibrary ? "打开资料库" : "创建资料库"}
          </p>
          <h1 id="review-title">
            {inspection.isExistingLibrary
              ? "找到现有资料库"
              : "在这里建立资料库"}
          </h1>
          <p className="wizard-lead">
            {inspection.isExistingLibrary
              ? "应用将打开该资料库，并把它加入最近使用列表。"
              : "应用会创建资料库元数据、搜索索引和文件存储目录。"}
          </p>

          <div className="path-summary">
            <FolderOpen aria-hidden="true" />
            <span>{inspection.path}</span>
          </div>

          {error ? <p className="error-text">{error}</p> : null}

          <div className="wizard-actions">
            <button
              className="button secondary"
              type="button"
              onClick={() => {
                setStep("choose");
                setInspection(null);
              }}
              disabled={busy}
            >
              <ArrowLeft size={17} aria-hidden="true" />
              返回修改位置
            </button>
            <button
              className="button primary"
              type="button"
              onClick={() =>
                void commit(
                  inspection.isExistingLibrary ? "open" : "create"
                )
              }
              disabled={busy}
            >
              {busy ? (
                <LoaderCircle className="spin" size={17} aria-hidden="true" />
              ) : (
                <FolderOpen size={17} aria-hidden="true" />
              )}
              {inspection.isExistingLibrary ? "打开资料库" : "创建资料库"}
            </button>
          </div>
        </section>
      </main>
    );
  }

  return (
    <main className="onboarding-shell">
      <section className="onboarding-panel" aria-labelledby="welcome-title">
        <div className="brand-mark large" aria-hidden="true">
          <span>文</span>
        </div>
        <p className="eyebrow">本地优先的文档资料库</p>
        <h1 id="welcome-title">选择资料库目录</h1>
        <p className="wizard-lead">
          资料库目录保存文档副本、元数据和搜索索引。你可以随时移动或备份它。
        </p>

        <button
          className="button primary wide"
          type="button"
          onClick={() => void chooseDirectory()}
          disabled={busy}
        >
          {busy ? (
            <LoaderCircle className="spin" size={18} aria-hidden="true" />
          ) : (
            <FolderOpen size={18} aria-hidden="true" />
          )}
          选择资料库目录
        </button>

        {error ? <p className="error-text">{error}</p> : null}

        <div className="privacy-note">
          <HardDrive aria-hidden="true" />
          <span>无需账户，数据只保存在你选择的目录中。</span>
        </div>
      </section>
    </main>
  );
}
