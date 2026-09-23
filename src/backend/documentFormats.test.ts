import { describe, expect, it } from "vitest";

import {
  documentFormatCapabilities,
  documentFormatForPath,
  documentFormatForType,
  documentFormatIdForType,
  importableDocumentExtensions,
  importableDocumentTypes,
  unsupportedDocumentMessage
} from "./documentFormats";

describe("文档格式能力表", () => {
  it("uses one complete capability definition for import, preview and safety", () => {
    expect(
      documentFormatCapabilities.map((capability) => capability.displayType)
    ).toEqual([
      "PDF",
      "DOCX",
      "TXT",
      "Markdown",
      "JPG",
      "PNG",
      "PPTX",
      "CSV",
      "XLSX"
    ]);
    expect(importableDocumentTypes).toEqual([
      "PDF",
      "DOCX",
      "TXT",
      "Markdown",
      "JPG",
      "PNG",
      "PPTX",
      "CSV",
      "XLSX"
    ]);
    expect(importableDocumentExtensions).toEqual([
      "pdf",
      "docx",
      "txt",
      "md",
      "markdown",
      "jpg",
      "jpeg",
      "png",
      "pptx",
      "csv",
      "xlsx"
    ]);

    for (const capability of documentFormatCapabilities) {
      expect(capability.security.macros).toBe("blocked");
      expect(capability.security.scripts).toBe("blocked");
      expect(capability.security.embeddedObjects).toBe("blocked");
      expect(capability.security.remoteResources).toBe("blocked");
      expect(capability.security.mediaAutoplay).toBe("blocked");
      expect(capability.security.sourceMutation).toBe("blocked");
      expect(capability.security.externalNavigation).toBe("userInitiated");
    }

    expect(documentFormatForType("MARKDOWN")?.id).toBe("markdown");
    expect(documentFormatForType("pptx")?.id).toBe("pptx");
    expect(documentFormatIdForType("PPTX")).toBe("pptx");
    expect(documentFormatForPath("C:\\Docs\\README.MD")?.id).toBe("markdown");
    expect(documentFormatForPath("C:\\Docs\\photo.jpeg")?.displayType).toBe(
      "JPG"
    );
    expect(documentFormatForPath("C:\\Docs\\slides.pptx")?.id).toBe("pptx");
    expect(documentFormatForPath("C:\\Docs\\legacy.ppt")).toBeNull();
    expect(documentFormatForPath("C:\\Docs\\macro.pptm")).toBeNull();
    expect(documentFormatForPath("C:\\Docs\\macro.docm")).toBeNull();
    expect(unsupportedDocumentMessage()).toContain("PPTX");
  });
});
