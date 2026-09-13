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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ValidationStrategy {
    PdfSignature,
    DocxPackage,
    PlainText,
    JpegSignature,
    PngSignature,
    OfficeOpenXmlReserved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PreviewStrategy {
    PdfPages,
    ExtractedOfficeText,
    PlainText,
    SafeMarkdown,
    LocalImage,
    PptxPagesReserved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThumbnailStrategy {
    PdfFirstPage,
    LocalImage,
    TypeIcon,
    PptxFirstPageReserved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TextExtractionStrategy {
    PdfText,
    DocxText,
    PlainText,
    None,
    PptxTextReserved,
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
    document_format_capabilities()
        .iter()
        .find(|capability| capability.display_type.eq_ignore_ascii_case(file_type))
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
    fn capability_table_is_complete_unique_and_future_ready() {
        let capabilities = document_format_capabilities();
        assert_eq!(
            capabilities
                .iter()
                .map(|capability| capability.display_type.as_str())
                .collect::<Vec<_>>(),
            ["PDF", "DOCX", "TXT", "Markdown", "JPG", "PNG", "PPTX"]
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
        assert!(!pptx.import_enabled);
        assert_eq!(importable_display_types().len(), 6);
        assert!(!unsupported_message().contains("PPTX"));
    }
}
