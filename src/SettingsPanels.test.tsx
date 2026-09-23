import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { FakeBackendClient } from "./backend/fakeClient";
import { LibraryContext } from "./backend/libraryContext";
import type { LibrarySummary } from "./backend/types";
import { SettingsDialog } from "./components/SettingsDialog";

const library: LibrarySummary = {
  id: "library-current",
  name: "个人资料",
  path: "C:\\Documents\\个人资料",
  createdAt: "2026-09-13T08:00:00Z"
};

const key = `${library.id}:${library.path}`;

function renderDialog() {
  const client = new FakeBackendClient({
    classificationRules: {
      [key]: [
        {
          id: "rule-1",
          name: "发票归档",
          enabled: true,
          position: 1,
          fileNamePattern: "发票",
          fileType: null,
          sourceDirectory: null,
          collectionId: "finance",
          tagIds: []
        }
      ]
    },
    receiveSources: {
      [key]: [
        {
          id: "source-1",
          kind: "qq",
          displayName: "QQ",
          path: "C:\\QQ\\Files",
          enabled: true,
          status: "ready",
          statusMessage: null,
          pendingCount: 0,
          lastScannedAt: null
        }
      ]
    },
    collections: [
      { id: "inbox", name: "收件箱", parentId: null, isInbox: true, documentCount: 0 },
      { id: "finance", name: "财务", parentId: null, isInbox: false, documentCount: 0 }
    ]
  });
  render(
    <LibraryContext.Provider value={library}>
      <SettingsDialog
        library={library}
        recentLibraries={[]}
        busyPath={null}
        client={client}
        collections={[
          { id: "inbox", name: "收件箱", parentId: null, isInbox: true, documentCount: 0 },
          { id: "finance", name: "财务", parentId: null, isInbox: false, documentCount: 0 }
        ]}
        onClose={() => {}}
        onOpenDirectory={() => {}}
        onOpenLibrary={() => {}}
        onForgetLibrary={() => {}}
      />
    </LibraryContext.Provider>
  );
  return client;
}

describe("SettingsDialog 面板接线", () => {
  it("shows rules and receive directory panels when their tabs are selected", async () => {
    const user = userEvent.setup();
    const client = renderDialog();
    const dialog = screen.getByRole("dialog", { name: "资料库" });

    await user.click(within(dialog).getByRole("tab", { name: "分类规则" }));
    expect(await within(dialog).findByText("发票归档")).toBeInTheDocument();
    expect(client.calls).toContain("listClassificationRules");

    await user.click(within(dialog).getByRole("tab", { name: "接收目录" }));
    expect(await within(dialog).findByText("QQ")).toBeInTheDocument();
    expect(within(dialog).getByText("C:\\QQ\\Files")).toBeInTheDocument();
  });
});
