use std::{path::Path, sync::OnceLock};

use serde::{Deserialize, Serialize};

use super::error::{LibraryError, LibraryResult};

const CAPABILITIES_JSON: &str = include_str!("../../../shared/document-format-capabilities.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DocumentFormatId {
    Pdf,
    Docx,
    Txt,
    Markdown,
    Jpg,
    Png,
    Pptx,
    Csv,
    Xlsx,
}

impl DocumentFormatId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Docx => "docx",
            Self::Txt => "txt",
            Self::Markdown => "markdown",
            Self::Jpg => "jpg",
            Self::Png => "png",
            Self::Pptx => "pptx",
            Self::Csv => "csv",
            Self::Xlsx => "xlsx",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "pdf" => Some(Self::Pdf),
            "docx" => Some(Self::Docx),
            "txt" => Some(Self::Txt),
            "markdown" | "md" => Some(Self::Markdown),
            "jpg" | "jpeg" => Some(Self::Jpg),
            "png" => Some(Self::Png),
            "pptx" => Some(Self::Pptx),
            "csv" => Some(Self::Csv),
            "xlsx" => Some(Self::Xlsx),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ValidationStrategy {
    PdfSignature,
    DocxPackage,
    PlainText,
    JpegSignature,
    PngSignature,
    PptxPackage,
    CsvText,
    XlsxPackage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PreviewStrategy {
    PdfPages,
    DocxLayout,
    PlainText,
    SafeMarkdown,
    LocalImage,
    PptxPages,
    TablePaged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThumbnailStrategy {
    PdfFirstPage,
    LocalImage,
    TypeIcon,
    PptxFirstPage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TextExtractionStrategy {
    PdfText,
    DocxText,
    PlainText,
    None,
    PptxText,
    TableText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SecurityPolicy {
    Blocked,
    UserInitiated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatSecurity {
    pub macros: SecurityPolicy,
    pub scripts: SecurityPolicy,
    pub embedded_objects: SecurityPolicy,
    pub remote_resources: SecurityPolicy,
    pub media_autoplay: SecurityPolicy,
    pub source_mutation: SecurityPolicy,
    pub external_navigation: SecurityPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentFormatCapability {
    pub id: DocumentFormatId,
    pub display_type: String,
    pub extensions: Vec<String>,
    pub import_enabled: bool,
    pub validation: ValidationStrategy,
    pub preview: PreviewStrategy,
    pub thumbnail: ThumbnailStrategy,
    pub text_extraction: TextExtractionStrategy,
    pub searchable: bool,
    pub media_type: Option<String>,
    pub renderer: String,
    pub security: FormatSecurity,
}

static CAPABILITIES: OnceLock<Vec<DocumentFormatCapability>> = OnceLock::new();

pub fn document_format_capabilities() -> &'static [DocumentFormatCapability] {
    CAPABILITIES.get_or_init(|| {
        serde_json::from_str(CAPABILITIES_JSON)
            .expect("shared document format capability table must be valid")
    })
}

pub fn capability_for_path(path: &Path) -> Option<&'static DocumentFormatCapability> {
    let extension = path.extension()?.to_string_lossy().to_ascii_lowercase();
    document_format_capabilities().iter().find(|capability| {
        capability.import_enabled
            && capability
                .extensions
                .iter()
                .any(|candidate| candidate == &extension)
    })
}

pub fn capability_for_file_type(file_type: &str) -> Option<&'static DocumentFormatCapability> {
    if let Some(id) = DocumentFormatId::parse(file_type) {
        return capability_for_id(id);
    }
    document_format_capabilities().iter().find(|capability| {
        capability
            .display_type
            .eq_ignore_ascii_case(file_type.trim())
    })
}

pub fn capability_for_id(id: DocumentFormatId) -> Option<&'static DocumentFormatCapability> {
    document_format_capabilities()
        .iter()
        .find(|capability| capability.id == id)
}

pub fn canonical_file_type(file_type: &str) -> Option<&'static str> {
    capability_for_file_type(file_type).map(|capability| capability.display_type.as_str())
}

pub fn importable_display_types() -> Vec<&'static str> {
    document_format_capabilities()
        .iter()
        .filter(|capability| capability.import_enabled)
        .map(|capability| capability.display_type.as_str())
        .collect()
}

pub fn unsupported_message() -> String {
    let display_types = importable_display_types();
    let supported = match display_types.as_slice() {
        [] => String::new(),
        [only] => (*only).to_string(),
        [prefix @ .., last] => format!("{} 和 {last}", prefix.join("、")),
    };
    format!("不支持该文件格式。仅支持 {supported} 文件。")
}

pub fn require_capability_for_file_type(
    file_type: &str,
) -> LibraryResult<&'static DocumentFormatCapability> {
    capability_for_file_type(file_type)
        .ok_or_else(|| LibraryError::UnsupportedFile(format!("不支持的文档格式：{file_type}")))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn capability_table_is_complete_unique_and_fully_enabled() {
        let capabilities = document_format_capabilities();
        assert_eq!(
            capabilities
                .iter()
                .map(|capability| capability.display_type.as_str())
                .collect::<Vec<_>>(),
            [
                "PDF", "DOCX", "TXT", "Markdown", "JPG", "PNG", "PPTX", "CSV", "XLSX"
            ]
        );

        let mut extensions = HashSet::new();
        for capability in capabilities {
            assert!(!capability.extensions.is_empty());
            for extension in &capability.extensions {
                assert!(
                    extensions.insert(extension),
                    "duplicate extension: {extension}"
                );
            }
            assert_eq!(capability.security.macros, SecurityPolicy::Blocked);
            assert_eq!(capability.security.scripts, SecurityPolicy::Blocked);
            assert_eq!(
                capability.security.remote_resources,
                SecurityPolicy::Blocked
            );
            assert_eq!(capability.security.media_autoplay, SecurityPolicy::Blocked);
            assert_eq!(capability.security.source_mutation, SecurityPolicy::Blocked);
        }

        let pptx = capability_for_file_type("PPTX").unwrap();
        assert_eq!(
            capability_for_file_type("pptx").unwrap().id,
            DocumentFormatId::Pptx
        );
        assert!(pptx.import_enabled);
        assert_eq!(pptx.validation, ValidationStrategy::PptxPackage);
        assert_eq!(pptx.preview, PreviewStrategy::PptxPages);
        assert_eq!(pptx.thumbnail, ThumbnailStrategy::PptxFirstPage);
        assert_eq!(pptx.text_extraction, TextExtractionStrategy::PptxText);

        let csv = capability_for_file_type("csv").unwrap();
        assert_eq!(csv.id, DocumentFormatId::Csv);
        assert_eq!(csv.validation, ValidationStrategy::CsvText);
        assert_eq!(csv.preview, PreviewStrategy::TablePaged);
        assert_eq!(csv.text_extraction, TextExtractionStrategy::TableText);
        assert!(csv.searchable);

        let xlsx = capability_for_path(Path::new("C:/Docs/账目.XLSX")).unwrap();
        assert_eq!(xlsx.id, DocumentFormatId::Xlsx);
        assert_eq!(xlsx.validation, ValidationStrategy::XlsxPackage);
        assert_eq!(xlsx.preview, PreviewStrategy::TablePaged);
        assert_eq!(xlsx.text_extraction, TextExtractionStrategy::TableText);

        assert_eq!(importable_display_types().len(), 9);
        assert!(unsupported_message().contains("PPTX"));
        assert!(unsupported_message().contains("CSV"));
        assert!(unsupported_message().contains("XLSX"));
    }
}
