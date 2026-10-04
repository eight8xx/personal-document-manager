import { useEffect, useRef, useState } from "react";
import type { CSSProperties, PointerEvent } from "react";

const key = "pdm.workspace-layout.v1";
const defaults = [0.185, 0.25];
export function constrainLayout(width: number, sidebar: number, details: number) {
  const available = Math.max(0, width - 12);
  const left = Math.min(Math.max(160, sidebar), Math.max(160, available - 500));
  const right = Math.min(Math.max(220, details), Math.max(220, available - left - 280));
  return [left, right];
}

export function useWorkspaceResize() {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(window.innerWidth);
  const [ratios, setRatios] = useState(() => {
    try {
      const saved: unknown = JSON.parse(localStorage.getItem(key) ?? "null");
      if (Array.isArray(saved) && saved.length === 2 && saved.every(value =>
        typeof value === "number" && Number.isFinite(value) && value >= 0.05 && value <= 0.7)) return saved as number[];
    } catch { /* 使用默认布局。 */ }
    return defaults;
  });
  useEffect(() => {
    if (!ref.current || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(([entry]) => setWidth(entry.contentRect.width));
    observer.observe(ref.current);
    return () => observer.disconnect();
  }, []);
  const sizes = constrainLayout(width, width * ratios[0], width * ratios[1]);
  function change(index: number, size: number) {
    const next = [...sizes];
    next[index] = size;
    // 限制正在调整的面板，不挤掉相邻面板或文档列表。
    next[index] = Math.max(index === 0 ? 160 : 220, Math.min(size, width - 12 - 280 - next[1 - index]));
    const nextRatios = next.map(value => value / Math.max(width, 1));
    setRatios(nextRatios);
    try { localStorage.setItem(key, JSON.stringify(nextRatios)); } catch { /* 布局仍可调整。 */ }
  }
  return { ref, sizes, change, reset: () => {
    setRatios(defaults);
    try { localStorage.removeItem(key); } catch { /* 忽略存储不可用。 */ }
  }, style: { "--sidebar-width": `${sizes[0]}px`, "--details-width": `${sizes[1]}px` } as CSSProperties };
}

export function WorkspaceDivider({ index, size, onChange, onReset }: {
  index: number; size: number;
  onChange: (index: number, size: number) => void; onReset: () => void;
}) {
  const drag = useRef<{ x: number; size: number } | null>(null);
  function move(event: PointerEvent<HTMLDivElement>) {
    if (!drag.current) return;
    // 用拖动起点计算增量，捕获指针后越过分隔线仍可调整。
    onChange(index, drag.current.size + (event.clientX - drag.current.x) * (index === 0 ? 1 : -1));
    event.stopPropagation();
  }
  return <div className={`workspace-divider divider-${index}`} role="separator" tabIndex={0}
    aria-label={index === 0 ? "调整集合与标签宽度" : "调整文档详情宽度"}
    aria-orientation="vertical" aria-valuemin={index === 0 ? 160 : 220} aria-valuenow={Math.round(size)}
    title="拖动调整宽度；方向键微调；双击恢复默认布局"
    onDoubleClick={onReset}
    onPointerDown={event => {
      if (event.button !== 0) return;
      event.stopPropagation();
      drag.current = { x: event.clientX, size };
      event.currentTarget.setPointerCapture(event.pointerId);
    }} onPointerMove={move} onPointerUp={event => {
      drag.current = null; event.stopPropagation();
      if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    }} onPointerCancel={event => { drag.current = null; event.stopPropagation(); }}
    onLostPointerCapture={() => { drag.current = null; }}
    onKeyDown={event => {
      if (event.key === "Home") { event.preventDefault(); onReset(); }
      if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
        event.preventDefault();
        onChange(index, size + (event.key === "ArrowRight" ? 16 : -16) * (index === 0 ? 1 : -1));
      }
    }} />;
}
