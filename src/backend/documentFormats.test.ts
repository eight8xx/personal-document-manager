import { describe, expect, it } from "vitest";

import {
  documentFormatCapabilities,
  documentFormatForPath,
  documentFormatForType,
  importableDocumentExtensions,
  importableDocumentTypes,
  unsupportedDocumentMessage
} from "./documentFormats";

describe("文档格式能力表", () => {
  it("uses one complete capability definition for import, preview and safety", () => {
    expect(
      documentFormatCapabilities.map((capability) => capability.displayType)
    ).toEqual(["PDF", "DOCX", "TXT", "Markdown", "JPG", "PNG", "PPTX"]);
    expect(importableDocumentTypes).toEqual([
      "PDF",
      "DOCX",
      "TXT",
      "Markdown",
      "JPG",
      "PNG"
    ]);
    expect(importableDocumentExtensions).toEqual([
      "pdf",
      "docx",
      "txt",
      "md",
      "markdown",
      "jpg",
      "jpeg",
      "png"
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
    expect(documentFormatForPath("C:\\Docs\\README.MD")?.id).toBe("markdown");
    expect(documentFormatForPath("C:\\Docs\\photo.jpeg")?.displayType).toBe(
      "JPG"
    );
    expect(documentFormatForPath("C:\\Docs\\slides.pptx")).toBeNull();
    expect(documentFormatForPath("C:\\Docs\\macro.docm")).toBeNull();
    expect(unsupportedDocumentMessage()).not.toContain("PPTX");
  });
});
