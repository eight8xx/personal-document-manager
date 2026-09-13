import {
  FolderOpen,
  LoaderCircle,
  RotateCcw,
  Settings,
  X
} from "lucide-react";
import { useEffect, useRef } from "react";

import type { LibrarySummary, RecentLibrary } from "../backend/types";

interface SettingsDialogProps {
  library: LibrarySummary;
  recentLibraries: RecentLibrary[];
  busyPath: string | null;
  onClose: () => void;
  onOpenDirectory: () => void;
  onOpenLibrary: (path: string) => void;
  onForgetLibrary: (path: string) => void;
}

export function SettingsDialog({
  library,
  recentLibraries,
  busyPath,
  onClose,
  onOpenDirectory,
  onOpenLibrary,
  onForgetLibrary
}: SettingsDialogProps) {
  const dialogRef = useRef<HTMLElement>(null);
  const closeButtonRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    closeButtonRef.current?.focus();

    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
        return;
      }

      if (event.key !== "Tab" || !dialogRef.current) {
        return;
      }

      const focusable = Array.from(
        dialogRef.current.querySelectorAll<HTMLElement>(
          "button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex='-1'])"
        )
      );

      if (focusable.length === 0) {
        event.preventDefault();
        return;
      }

      const first = focusable[0];
      const last = focusable.at(-1)!;

      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    }

    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  return (
    <div className="dialog-backdrop" role="presentation">
      <section
        ref={dialogRef}
        className="settings-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-title"
      >
        <header className="dialog-header">
          <div>
            <p className="eyebrow">应用设置</p>
            <h2 id="settings-title">资料库</h2>
          </div>
          <button
            ref={closeButtonRef}
            className="icon-button"
            type="button"
            onClick={onClose}
            aria-label="关闭设置"
            title="关闭设置"
          >
            <X size={19} aria-hidden="true" />
          </button>
        </header>

        <div className="settings-section">
          <div className="section-title-row">
            <Settings size={18} aria-hidden="true" />
            <h3>当前资料库</h3>
          </div>
          <div className="current-library-summary">
            <div>
              <strong>{library.name}</strong>
              <span>{library.path}</span>
            </div>
            <button
              className="button secondary"
              type="button"
              onClick={onOpenDirectory}
            >
              <FolderOpen size={17} aria-hidden="true" />
              打开目录
            </button>
          </div>
          <p className="backup-warning">
            备份资料库前，请先关闭应用。
          </p>
        </div>

        <div className="settings-section">
          <div className="section-title-row">
            <RotateCcw size={18} aria-hidden="true" />
            <h3>最近资料库</h3>
          </div>
          <div className="recent-library-list">
            {recentLibraries.length === 0 ? (
              <p className="muted-copy">没有最近使用的资料库。</p>
            ) : (
              recentLibraries.map((item) => {
                const isCurrent = item.path === library.path;
                const isBusy = busyPath === item.path;

                return (
                  <div className="recent-library-row" key={item.path}>
                    <div className="recent-library-copy">
                      <strong>{item.name}</strong>
                      <span>{item.path}</span>
                      {!item.isAvailable ? <em>位置不可用</em> : null}
                    </div>
                    <div className="row-actions">
                      <button
                        className="button quiet"
                        type="button"
                        disabled={
                          isCurrent || !item.isAvailable || Boolean(busyPath)
                        }
                        onClick={() => onOpenLibrary(item.path)}
                      >
                        {isBusy ? (
                          <LoaderCircle
                            className="spin"
                            size={16}
                            aria-hidden="true"
                          />
                        ) : null}
                        {isCurrent ? "当前" : "切换"}
                      </button>
                      <button
                        className="button danger-quiet"
                        type="button"
                        disabled={Boolean(busyPath)}
                        onClick={() => onForgetLibrary(item.path)}
                      >
                        移出列表
                      </button>
                    </div>
                  </div>
                );
              })
            )}
          </div>
          <p className="muted-copy">
            移出最近使用列表不会删除磁盘上的资料库文件。
          </p>
        </div>
      </section>
    </div>
  );
}
