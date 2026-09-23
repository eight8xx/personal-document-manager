import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { configure } from "@testing-library/dom";
import { afterEach } from "vitest";

import { clearPreviewRenderCache } from "../components/previewRenderCache";

// findBy*/waitFor 默认只等 1 秒，本机渲染首次挂载（含 docx/pptx 渲染器）在并行
// 构建负载下会超过 1 秒而误报；放宽等待窗口，断言本身不放宽。
configure({ asyncUtilTimeout: 3000 });

afterEach(() => {
  cleanup();
  clearPreviewRenderCache();
});
