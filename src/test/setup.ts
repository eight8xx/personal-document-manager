import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

import { clearPreviewRenderCache } from "../components/previewRenderCache";

afterEach(() => {
  cleanup();
  clearPreviewRenderCache();
});
