import { act, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { useResizableDialogs } from "./components/useResizableDialogs";

afterEach(() => vi.unstubAllGlobals());
function Host() {
  useResizableDialogs();
  return <><section role="dialog" aria-label="编辑" /><section className="detail-preview" aria-label="预览" /></>;
}
it("拖动后的弹窗尺寸转换为视口比例，预览宽度仍随详情栏变化", () => {
  let callback: ResizeObserverCallback | undefined;
  vi.stubGlobal("ResizeObserver", class {
    constructor(resize: ResizeObserverCallback) { callback = resize; }
    observe() {} unobserve() {} disconnect() {}
  });
  const view = render(<Host />);
  const dialog = screen.getByRole("dialog");
  const preview = screen.getByRole("region", { name: "预览" });
  dialog.style.width = `${window.innerWidth / 2}px`;
  dialog.style.height = `${window.innerHeight / 2}px`;
  preview.style.height = `${window.innerHeight / 4}px`;
  preview.style.width = "250px";
  const entries: ResizeObserverEntry[] = [dialog, preview].map(target => ({
    target, contentRect: target.getBoundingClientRect(), borderBoxSize: [], contentBoxSize: [], devicePixelContentBoxSize: []
  }));
  act(() => callback!(entries, {} as ResizeObserver));
  expect(dialog.style.width).toBe("50vw");
  expect(dialog.style.height).toBe("50vh");
  expect(preview.style.width).toBe("");
  expect(preview.style.height).toBe("25vh");
  view.unmount();
});
