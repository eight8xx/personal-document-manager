import {
  cleanup,
  render,
  screen,
  waitFor,
  within
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";

import { App } from "./App";
import { FakeBackendClient } from "./backend/fakeClient";
import type { BootstrapState, LibrarySummary } from "./backend/types";

const currentLibrary: LibrarySummary = {
  id: "library-current",
  name: "个人资料",
  path: "C:\\Documents\\个人资料",
  createdAt: "2026-09-13T08:00:00Z"
};

afterEach(() => {
  cleanup();
});

function bootstrapWithLibrary(
  recentLibraries: BootstrapState["recentLibraries"] = []
): BootstrapState {
  return {
    currentLibrary,
    recentLibraries
  };
}

describe("App", () => {
  it("creates a library from the first-run wizard", async () => {
    const user = userEvent.setup();
    const client = new FakeBackendClient({
      selectedDirectory: "C:\\Documents\\我的资料库"
    });

    render(<App client={client} />);

    expect(
      await screen.findByRole("heading", { name: "选择资料库目录" })
    ).toBeInTheDocument();

    await user.click(
      screen.getByRole("button", { name: "选择资料库目录" })
    );

    expect(
      await screen.findByRole("button", { name: "创建资料库" })
    ).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "创建资料库" }));

    expect(
      await screen.findByRole("heading", { name: "空资料库" })
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "导入文档" })).toBeInTheDocument();
    expect(client.calls).toContain(
      "create:C:\\Documents\\我的资料库"
    );
  });

  it("shows a cloud warning and lets the user go back", async () => {
    const user = userEvent.setup();
    const path = "C:\\Users\\Me\\OneDrive\\Documents\\资料库";
    const client = new FakeBackendClient({
      selectedDirectory: path,
      inspections: {
        [path]: {
          path,
          status: "usable",
          isExistingLibrary: false,
          cloudSyncWarning: {
            provider: "OneDrive",
            message: "这个位置看起来位于 OneDrive 同步目录中。"
          },
          reason: null
        }
      }
    });

    render(<App client={client} />);
    await user.click(
      await screen.findByRole("button", { name: "选择资料库目录" })
    );

    expect(
      await screen.findByRole("heading", {
        name: "这个位置可能由云盘同步"
      })
    ).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "返回修改位置" }));
    expect(
      screen.getByRole("heading", { name: "选择资料库目录" })
    ).toBeInTheDocument();
  });

  it("exposes the empty state and settings library controls", async () => {
    const user = userEvent.setup();
    const otherLibrary = {
      path: "D:\\Archive",
      name: "归档",
      lastOpenedAt: "2026-09-12T08:00:00Z",
      isAvailable: true
    };
    const client = new FakeBackendClient({
      bootstrap: bootstrapWithLibrary([
        {
          path: currentLibrary.path,
          name: currentLibrary.name,
          lastOpenedAt: "2026-09-13T08:00:00Z",
          isAvailable: true
        },
        otherLibrary
      ])
    });

    render(<App client={client} />);

    expect(
      await screen.findByRole("heading", { name: "空资料库" })
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "导入文档" })).toBeDisabled();

    const settingsButton = screen.getByRole("button", { name: "设置" });
    await user.click(settingsButton);

    const dialog = screen.getByRole("dialog", { name: "资料库" });
    expect(dialog).toBeInTheDocument();
    const closeSettings = within(dialog).getByRole("button", {
      name: "关闭设置"
    });
    await waitFor(() => expect(closeSettings).toHaveFocus());
    await user.tab({ shift: true });
    expect(
      within(dialog).getAllByRole("button", { name: "移出列表" }).at(-1)
    ).toHaveFocus();
    await user.tab();
    expect(closeSettings).toHaveFocus();
    expect(
      within(dialog).getAllByText(currentLibrary.path).length
    ).toBeGreaterThan(0);
    expect(
      within(dialog).getByText("备份资料库前，请先关闭应用。")
    ).toBeInTheDocument();

    await user.click(within(dialog).getByRole("button", { name: "打开目录" }));
    expect(client.calls).toContain(
      `openDirectory:${currentLibrary.path}`
    );

    await user.click(within(dialog).getByRole("button", { name: "切换" }));
    await waitFor(() => {
      expect(client.calls).toContain("open:D:\\Archive");
    });

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: "资料库" })).not.toBeInTheDocument();
    await waitFor(() => expect(settingsButton).toHaveFocus());
  });
});
