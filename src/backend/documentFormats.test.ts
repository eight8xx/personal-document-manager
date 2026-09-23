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

    // 表格文档（工作单 08/09）走同一张能力表：文件选择器的过滤器与拖放识别
    // 都依赖这里的扩展名映射，大小写不敏感。
    expect(documentFormatForPath("C:\\Docs\\账目.CSV")?.id).toBe("csv");
    expect(documentFormatForPath("C:\\Docs\\季度报表.xlsx")?.id).toBe("xlsx");
    expect(documentFormatForType("CSV")).toMatchObject({
      validation: "csvText",
      preview: "tablePaged",
      textExtraction: "tableText",
      searchable: true
    });
    expect(documentFormatForType("XLSX")).toMatchObject({
      validation: "xlsxPackage",
      preview: "tablePaged",
      textExtraction: "tableText",
      searchable: true
    });
    // 旧版 XLS、宏工作簿与 ODS 不在能力表内：选择器不会列出，拖放也不会当成表格文档。
    expect(documentFormatForPath("C:\\Docs\\旧账目.xls")).toBeNull();
    expect(documentFormatForPath("C:\\Docs\\宏表.xlsm")).toBeNull();
    expect(documentFormatForPath("C:\\Docs\\表格.ods")).toBeNull();
  });
});
