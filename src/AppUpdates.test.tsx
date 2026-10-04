import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AppUpdateProvider, AppUpdateSettings } from "./components/AppUpdates";
import type { UpdateClient, UpdateResult } from "./components/AppUpdates";

const available: UpdateResult = { currentVersion: "0.1.0", update: {
  version: "0.2.0", notes: "改进布局", releaseUrl: "https://github.com/example/releases/tag/v0.2.0",
  downloadUrl: "https://github.com/example/releases/download/v0.2.0/setup.exe"
} };
function mount(check = vi.fn().mockResolvedValue(available), open = vi.fn().mockResolvedValue(undefined)) {
  const client: UpdateClient = { enabled: true, check, open };
  render(<AppUpdateProvider client={client}><AppUpdateSettings /></AppUpdateProvider>);
  return { check, open };
}
describe("应用更新", () => {
  it("启动时询问，稍后不打开下载，手动检查可再次询问，确认才打开", async () => {
    const { check, open } = mount();
    await screen.findByRole("region", { name: "发现应用更新" });
    expect(open).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "稍后再说" }));
    expect(screen.queryByRole("region", { name: "发现应用更新" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "检查更新" }));
    await screen.findByRole("region", { name: "发现应用更新" });
    expect(check).toHaveBeenCalledTimes(2);
    fireEvent.click(screen.getAllByRole("button", { name: "下载更新" }).at(-1)!);
    await waitFor(() => expect(open).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.queryByRole("region", { name: "发现应用更新" })).not.toBeInTheDocument());
  });
  it("网络失败可重试，同版本显示最新，不询问下载", async () => {
    const check = vi.fn().mockRejectedValueOnce(new Error("网络不可用"))
      .mockResolvedValue({ currentVersion: "0.1.0", update: null });
    mount(check);
    await screen.findByRole("alert");
    fireEvent.click(screen.getByRole("button", { name: "检查更新" }));
    await screen.findByText("当前已是最新正式版本。");
    expect(screen.queryByRole("region", { name: "发现应用更新" })).not.toBeInTheDocument();
  });
  it("打开失败保留提示并允许重试", async () => {
    const open = vi.fn().mockRejectedValueOnce("浏览器无法打开").mockResolvedValue(undefined);
    mount(undefined, open);
    await screen.findByRole("region", { name: "发现应用更新" });
    fireEvent.click(screen.getAllByRole("button", { name: "下载更新" }).at(-1)!);
    await screen.findAllByText("浏览器无法打开");
    expect(screen.getByRole("region", { name: "发现应用更新" })).toBeInTheDocument();
    fireEvent.click(screen.getAllByRole("button", { name: "下载更新" }).at(-1)!);
    await waitFor(() => expect(open).toHaveBeenCalledTimes(2));
  });
  it("自动检查失败不在工作区弹出错误", async () => {
    const check = vi.fn().mockRejectedValue("无法连接 GitHub");
    render(<AppUpdateProvider client={{ enabled: true, check, open: vi.fn() }}><div>工作区</div></AppUpdateProvider>);
    await act(async () => { await Promise.resolve(); });
    expect(check).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});
