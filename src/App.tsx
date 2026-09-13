import { AlertCircle, LoaderCircle, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import { tauriBackendClient, toBackendError } from "./backend/client";
import type {
  BackendClient,
  LibrarySummary,
  RecentLibrary
} from "./backend/types";
import { LibraryWizard } from "./components/LibraryWizard";
import { LibraryWorkspace } from "./components/LibraryWorkspace";
import { SettingsDialog } from "./components/SettingsDialog";

interface AppProps {
  client?: BackendClient;
}

export function App({ client = tauriBackendClient }: AppProps) {
  const [loading, setLoading] = useState(true);
  const [library, setLibrary] = useState<LibrarySummary | null>(null);
  const [recentLibraries, setRecentLibraries] = useState<RecentLibrary[]>([]);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [error, setError] = useState("");
  const [busyPath, setBusyPath] = useState<string | null>(null);
  const settingsTriggerRef = useRef<HTMLElement | null>(null);

  const openSettings = useCallback(() => {
    if (document.activeElement instanceof HTMLElement) {
      settingsTriggerRef.current = document.activeElement;
    }
    setSettingsOpen(true);
  }, []);

  const closeSettings = useCallback(() => {
    setSettingsOpen(false);
    window.setTimeout(() => settingsTriggerRef.current?.focus(), 0);
  }, []);

  useEffect(() => {
    let active = true;

    async function load() {
      try {
        const state = await client.bootstrap();
        if (!active) {
          return;
        }
        setLibrary(state.currentLibrary);
        setRecentLibraries(state.recentLibraries);
      } catch (caught) {
        if (active) {
          setError(toBackendError(caught).message);
        }
      } finally {
        if (active) {
          setLoading(false);
        }
      }
    }

    void load();
    return () => {
      active = false;
    };
  }, [client]);

  async function openLibrary(path: string) {
    setBusyPath(path);
    setError("");

    try {
      const nextLibrary = await client.openLibrary(path);
      setLibrary(nextLibrary);
      setRecentLibraries(await client.listRecentLibraries());
    } catch (caught) {
      setError(toBackendError(caught).message);
    } finally {
      setBusyPath(null);
    }
  }

  async function forgetLibrary(path: string) {
    setBusyPath(path);
    setError("");

    try {
      setRecentLibraries(await client.forgetRecentLibrary(path));
    } catch (caught) {
      setError(toBackendError(caught).message);
    } finally {
      setBusyPath(null);
    }
  }

  async function openDirectory() {
    if (!library) {
      return;
    }

    setError("");
    try {
      await client.openLibraryDirectory(library.path);
    } catch (caught) {
      setError(toBackendError(caught).message);
    }
  }

  if (loading) {
    return (
      <main className="loading-screen" aria-label="正在加载">
        <LoaderCircle className="spin" size={26} aria-hidden="true" />
      </main>
    );
  }

  return (
    <>
      {error ? (
        <div className="global-error" role="alert">
          <AlertCircle size={18} aria-hidden="true" />
          <span>{error}</span>
          <button
            className="icon-button"
            type="button"
            onClick={() => setError("")}
            aria-label="关闭错误提示"
            title="关闭"
          >
            <X size={17} aria-hidden="true" />
          </button>
        </div>
      ) : null}

      {library ? (
        <>
          <LibraryWorkspace
            library={library}
            onOpenSettings={openSettings}
          />
          {settingsOpen ? (
            <SettingsDialog
              library={library}
              recentLibraries={recentLibraries}
              busyPath={busyPath}
              onClose={closeSettings}
              onOpenDirectory={() => void openDirectory()}
              onOpenLibrary={(path) => void openLibrary(path)}
              onForgetLibrary={(path) => void forgetLibrary(path)}
            />
          ) : null}
        </>
      ) : (
        <LibraryWizard
          client={client}
          onCreated={(created) => {
            setLibrary(created);
            void client
              .listRecentLibraries()
              .then(setRecentLibraries)
              .catch((caught) =>
                setError(toBackendError(caught).message)
              );
          }}
          onOpened={(opened) => {
            setLibrary(opened);
            void client
              .listRecentLibraries()
              .then(setRecentLibraries)
              .catch((caught) =>
                setError(toBackendError(caught).message)
              );
          }}
        />
      )}
    </>
  );
}
