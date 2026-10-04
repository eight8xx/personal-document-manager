import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { constrainLayout, useWorkspaceResize, WorkspaceDivider } from "./components/WorkspaceResize";

afterEach(() => { localStorage.clear(); vi.unstubAllGlobals(); });
function Workspace() {
  const layout = useWorkspaceResize();
  return <div ref={layout.ref} style={layout.style}>
    <WorkspaceDivider index={0} size={layout.sizes[0]} onChange={layout.change} onReset={layout.reset} />
    <WorkspaceDivider index={1} size={layout.sizes[1]} onChange={layout.change} onReset={layout.reset} />
  </div>;
}
it("最小桌面窗口仍保留三个面板的可用空间", () => {
  for (const width of [860, 1024, 1280, 1920]) {
    const [left, right] = constrainLayout(width, width * 0.6, width * 0.6);
    expect(left).toBeGreaterThanOrEqual(160);
    expect(right).toBeGreaterThanOrEqual(220);
    expect(width - 12 - left - right).toBeGreaterThanOrEqual(280);
  }
});
it("键盘调节保存比例，重新挂载保留，双击重置", () => {
  const view = render(<Workspace />);
  const left = screen.getByRole("separator", { name: "调整集合与标签宽度" });
  const original = Number(left.getAttribute("aria-valuenow"));
  fireEvent.keyDown(left, { key: "ArrowRight" });
  expect(Number(left.getAttribute("aria-valuenow"))).toBe(original + 16);
  view.unmount();
  render(<Workspace />);
  const restored = screen.getByRole("separator", { name: "调整集合与标签宽度" });
  expect(Number(restored.getAttribute("aria-valuenow"))).toBe(original + 16);
  fireEvent.doubleClick(restored);
  expect(Number(restored.getAttribute("aria-valuenow"))).toBe(original);
});
it("主窗口改变宽度时按比例变化，并保留列表最小宽度", () => {
  let resize: ResizeObserverCallback | undefined;
  vi.stubGlobal("ResizeObserver", class { constructor(callback: ResizeObserverCallback) { resize = callback; } observe() {} disconnect() {} });
  render(<Workspace />);
  act(() => resize!([{ contentRect: { width: 1280 } } as ResizeObserverEntry], {} as ResizeObserver));
  expect(screen.getAllByRole("separator")[0]).toHaveAttribute("aria-valuenow", "237");
  expect(screen.getAllByRole("separator")[1]).toHaveAttribute("aria-valuenow", "320");
  act(() => resize!([{ contentRect: { width: 860 } } as ResizeObserverEntry], {} as ResizeObserver));
  expect(screen.getAllByRole("separator")[0]).toHaveAttribute("aria-valuenow", "160");
  expect(screen.getAllByRole("separator")[1]).toHaveAttribute("aria-valuenow", "220");
});
it("拖动分隔线时调整面板，释放指针后停止调整", () => {
  vi.stubGlobal("PointerEvent", MouseEvent);
  render(<Workspace />);
  const left = screen.getByRole("separator", { name: "调整集合与标签宽度" });
  left.setPointerCapture = vi.fn();
  left.hasPointerCapture = vi.fn().mockReturnValue(true);
  left.releasePointerCapture = vi.fn();
  const original = Number(left.getAttribute("aria-valuenow"));
  fireEvent.pointerDown(left, { button: 0, clientX: 200 });
  fireEvent.pointerMove(left, { clientX: 240 });
  expect(Number(left.getAttribute("aria-valuenow"))).toBe(original + 40);
  fireEvent.pointerUp(left);
  fireEvent.pointerMove(left, { clientX: 280 });
  expect(Number(left.getAttribute("aria-valuenow"))).toBe(original + 40);
});
