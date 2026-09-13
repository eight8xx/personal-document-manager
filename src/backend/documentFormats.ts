import capabilityData from "../../shared/document-format-capabilities.json";
import type {
  DocumentFormatCapability,
  DocumentFormatId,
  DocumentSummary
} from "./types";

export const documentFormatCapabilities =
  capabilityData as DocumentFormatCapability[];

export const importableDocumentExtensions = documentFormatCapabilities
  .filter((capability) => capability.importEnabled)
  .flatMap((capability) => capability.extensions);

export const importableDocumentTypes = documentFormatCapabilities
  .filter((capability) => capability.importEnabled)
  .map((capability) => capability.displayType);

const capabilityByType = new Map(
  documentFormatCapabilities.map((capability) => [
    capability.displayType.toLocaleLowerCase(),
    capability
  ])
);

const capabilityById = new Map(
  documentFormatCapabilities.map((capability) => [
    capability.id,
    capability
  ])
);

const capabilityByExtension = new Map(
  documentFormatCapabilities.flatMap((capability) =>
    capability.extensions.map((extension) => [extension, capability] as const)
  )
);

export function documentFormatForType(
  fileType: string
): DocumentFormatCapability | null {
  const normalized = fileType.trim().toLocaleLowerCase();
  return (
    capabilityById.get(normalized as DocumentFormatId) ??
    capabilityByType.get(normalized) ??
    null
  );
}

export function documentFormatIdForType(
  fileType: string
): DocumentFormatId | null {
  return documentFormatForType(fileType)?.id ?? null;
}

export function documentFormatForPath(
  path: string
): DocumentFormatCapability | null {
  const extension = path.split(".").at(-1)?.toLocaleLowerCase() ?? "";
  const capability = capabilityByExtension.get(extension) ?? null;
  return capability?.importEnabled ? capability : null;
}

export function fileTypeForPath(path: string): string | null {
  return documentFormatForPath(path)?.displayType ?? null;
}

export function documentUsesLocalImage(
  document: Pick<DocumentSummary, "fileType">
) {
  return documentFormatForType(document.fileType)?.preview === "localImage";
}

export function documentUsesPdfPreview(
  document: Pick<DocumentSummary, "fileType">
) {
  return documentFormatForType(document.fileType)?.preview === "pdfPages";
}

export function documentSupportsGeneratedThumbnail(
  document: Pick<DocumentSummary, "fileType">
) {
  const strategy = documentFormatForType(document.fileType)?.thumbnail;
  return (
    strategy === "localImage" ||
    strategy === "pdfFirstPage" ||
    strategy === "pptxFirstPage"
  );
}

export function documentUsesPptxPreview(
  document: Pick<DocumentSummary, "fileType">
) {
  return documentFormatForType(document.fileType)?.preview === "pptxPages";
}

export function unsupportedDocumentMessage() {
  const types = [...importableDocumentTypes];
  const last = types.pop();
  const supported = last ? `${types.join("、")} 和 ${last}` : "";
  return `不支持该文件格式。仅支持 ${supported} 文件。`;
}
