import { useEffect } from "react";

/** 浏览器原生拖动会写入像素尺寸，将它转换为视口比例，主窗口变化后仍保持比例。 */
export function useResizableDialogs() {
  useEffect(() => {
    if (typeof ResizeObserver === "undefined") return;
    const observed = new Set<HTMLElement>();
    const resize = new ResizeObserver(entries => {
      for (const entry of entries) {
        const element = entry.target as HTMLElement;
        if (element.style.width.endsWith("px")) {
          element.style.width = element.matches('[role="dialog"], [role="alertdialog"], .update-notice')
            ? `${Number.parseFloat(element.style.width) / window.innerWidth * 100}vw`
            : "";
        }
        if (element.style.height.endsWith("px")) {
          element.style.height = `${Number.parseFloat(element.style.height) / window.innerHeight * 100}vh`;
        }
      }
    });
    const scan = () => {
      for (const element of observed) {
        if (!element.isConnected) { resize.unobserve(element); observed.delete(element); }
      }
      document.querySelectorAll<HTMLElement>('[role="dialog"], [role="alertdialog"], .update-notice, .detail-preview, .import-panel').forEach(element => {
        if (observed.has(element)) return;
        const width = element.getBoundingClientRect().width;
        if (width > 0 && element.matches('[role="dialog"], [role="alertdialog"], .update-notice')) {
          element.style.width = `${width / window.innerWidth * 100}vw`;
        }
        observed.add(element);
        resize.observe(element);
      });
    };
    scan();
    const mutations = new MutationObserver(scan);
    mutations.observe(document.body, { childList: true, subtree: true });
    return () => { mutations.disconnect(); resize.disconnect(); observed.clear(); };
  }, []);
}
