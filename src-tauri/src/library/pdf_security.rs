use std::collections::HashSet;

use lopdf::{Dictionary, Document, LoadOptions, Object, ObjectId};

use super::error::{LibraryError, LibraryResult};

const MAX_OBJECT_STREAM_BYTES: usize = 64 * 1024 * 1024;
const DANGEROUS_PDF_NAMES: &[&str] = &[
    "openaction",
    "aa",
    "javascript",
    "js",
    "launch",
    "embeddedfile",
    "embeddedfiles",
    "fileattachment",
    "ef",
    "richmedia",
    "xfa",
    "gotor",
    "gotoe",
    "submitform",
    "importdata",
    "rendition",
    "movie",
    "sound",
    "3d",
    "encrypt",
];

pub(super) fn ensure_safe_pdf_preview(bytes: &[u8]) -> LibraryResult<()> {
    let options = LoadOptions {
        strict: true,
        max_decompressed_size: Some(MAX_OBJECT_STREAM_BYTES),
        ..Default::default()
    };
    let document = Document::load_mem_with_options(bytes, options).map_err(|error| {
        LibraryError::UnsafePreview(format!(
            "无法安全解析 PDF：{error}。为避免绕过安全检查，已拒绝预览。"
        ))
    })?;

    if document.was_encrypted() || document.is_encrypted() {
        return Err(LibraryError::UnsafePreview(
            "PDF 已加密，无法在安全预览中验证完整对象图，已拒绝预览。".to_string(),
        ));
    }

    let mut visited_references = HashSet::new();
    inspect_dictionary(&document.trailer, &document, &mut visited_references)?;
    for object in document.objects.values() {
        inspect_object(object, &document, &mut visited_references)?;
    }
    Ok(())
}

fn inspect_dictionary(
    dictionary: &Dictionary,
    document: &Document,
    visited_references: &mut HashSet<ObjectId>,
) -> LibraryResult<()> {
    for (name, value) in dictionary.iter() {
        if dangerous_name(name) {
            return Err(unsafe_name_error(name));
        }
        inspect_object(value, document, visited_references)?;
    }
    Ok(())
}

fn inspect_object(
    object: &Object,
    document: &Document,
    visited_references: &mut HashSet<ObjectId>,
) -> LibraryResult<()> {
    match object {
        Object::Null
        | Object::Boolean(_)
        | Object::Integer(_)
        | Object::Real(_)
        | Object::String(_, _) => Ok(()),
        Object::Name(name) => {
            if dangerous_name(name) {
                Err(unsafe_name_error(name))
            } else {
                Ok(())
            }
        }
        Object::Array(items) => {
            for item in items {
                inspect_object(item, document, visited_references)?;
            }
            Ok(())
        }
        Object::Dictionary(dictionary) => {
            inspect_dictionary(dictionary, document, visited_references)
        }
        Object::Stream(stream) => inspect_dictionary(&stream.dict, document, visited_references),
        Object::Reference(object_id) => {
            if !visited_references.insert(*object_id) {
                return Ok(());
            }
            let resolved = document.get_object(*object_id).map_err(|error| {
                LibraryError::UnsafePreview(format!(
                    "无法解析 PDF 间接对象 {} {}：{error}。已拒绝预览。",
                    object_id.0, object_id.1
                ))
            })?;
            inspect_object(resolved, document, visited_references)
        }
    }
}

fn dangerous_name(name: &[u8]) -> bool {
    DANGEROUS_PDF_NAMES
        .iter()
        .any(|dangerous| name.eq_ignore_ascii_case(dangerous.as_bytes()))
}

fn unsafe_name_error(name: &[u8]) -> LibraryError {
    LibraryError::UnsafePreview(format!(
        "PDF 包含会在预览时被禁止的对象：/{}。",
        String::from_utf8_lossy(name)
    ))
}

#[cfg(test)]
mod tests {
    use lopdf::SaveOptions;

    use super::*;

    #[test]
    fn rejects_direct_and_indirect_active_objects() {
        let open_action = base_document()
            .with_catalog_entry("OpenAction", Object::Reference((5, 0)))
            .with_object((5, 0), action_dictionary("JavaScript"))
            .into_compressed_pdf();
        assert!(matches!(
            ensure_safe_pdf_preview(&open_action),
            Err(LibraryError::UnsafePreview(_))
        ));

        let javascript = base_document()
            .with_catalog_entry(
                "OpenAction",
                Object::Dictionary(action_dictionary("JavaScript")),
            )
            .into_compressed_pdf();
        assert!(matches!(
            ensure_safe_pdf_preview(&javascript),
            Err(LibraryError::UnsafePreview(_))
        ));
    }

    #[test]
    fn rejects_active_objects_nested_in_a_compressed_object_stream() {
        let nested = Dictionary::from_iter([
            (b"Name".to_vec(), Object::Name(b"JavaScript".to_vec())),
            (
                b"JS".to_vec(),
                Object::string_literal("app.alert('blocked')"),
            ),
        ]);
        let mut wrapper = Dictionary::new();
        wrapper.set("Nested", Object::Dictionary(nested));
        let pdf = base_document()
            .with_catalog_entry("CustomData", Object::Dictionary(wrapper))
            .into_compressed_pdf();

        assert!(find_bytes(&pdf, b"/ObjStm").is_some());
        assert!(matches!(
            ensure_safe_pdf_preview(&pdf),
            Err(LibraryError::UnsafePreview(_))
        ));
    }

    #[test]
    fn allows_a_normal_compressed_pdf_with_nested_dictionaries() {
        let mut nested = Dictionary::new();
        nested.set("Label", "Static metadata");
        nested.set("Values", vec![1.into(), 2.into(), 3.into()]);
        let mut wrapper = Dictionary::new();
        wrapper.set("Nested", Object::Dictionary(nested));
        let pdf = base_document()
            .with_catalog_entry("CustomData", Object::Dictionary(wrapper))
            .into_compressed_pdf();

        assert!(find_bytes(&pdf, b"/ObjStm").is_some());
        assert!(ensure_safe_pdf_preview(&pdf).is_ok());
    }

    #[test]
    fn allows_static_uri_links_in_the_non_interactive_preview() {
        let mut uri_action = Dictionary::new();
        uri_action.set("Type", "Action");
        uri_action.set("S", "URI");
        uri_action.set("URI", Object::string_literal("https://example.com"));
        let pdf = base_document()
            .with_catalog_entry("CustomData", Object::Dictionary(uri_action))
            .into_compressed_pdf();

        assert!(ensure_safe_pdf_preview(&pdf).is_ok());
    }

    #[test]
    fn rejects_unparseable_pdfs_with_a_clear_reason() {
        let error = ensure_safe_pdf_preview(b"not a PDF").unwrap_err();
        assert_eq!(error.code(), "unsafePreview");
        assert!(error.to_string().contains("无法安全解析 PDF"));
    }

    #[test]
    fn rejects_encrypted_pdfs_with_a_clear_reason() {
        let mut document = base_pdf();
        document.trailer.set(
            "ID",
            vec![
                Object::string_literal(b"document-id".to_vec()),
                Object::string_literal(b"document-id".to_vec()),
            ],
        );
        let encryption_version = lopdf::EncryptionVersion::V2 {
            document: &document,
            owner_password: "owner-password",
            user_password: "user-password",
            key_length: 128,
            permissions: lopdf::Permissions::all(),
        };
        let encryption_state = lopdf::EncryptionState::try_from(encryption_version).unwrap();
        document.encrypt(&encryption_state).unwrap();
        let mut bytes = Vec::new();
        document.save_to(&mut bytes).unwrap();

        let error = ensure_safe_pdf_preview(&bytes).unwrap_err();
        assert_eq!(error.code(), "unsafePreview");
        assert!(error.to_string().contains("已加密"));
    }

    fn base_document() -> DocumentBuilder {
        DocumentBuilder {
            document: base_pdf(),
        }
    }

    fn base_pdf() -> Document {
        let mut document = Document::with_version("1.7");
        let mut catalog = Dictionary::new();
        catalog.set("Type", "Catalog");
        catalog.set("Pages", Object::Reference((2, 0)));

        let mut pages = Dictionary::new();
        pages.set("Type", "Pages");
        pages.set("Kids", vec![Object::Reference((3, 0))]);
        pages.set("Count", 1);

        let mut page = Dictionary::new();
        page.set("Type", "Page");
        page.set("Parent", Object::Reference((2, 0)));
        page.set("MediaBox", vec![0.into(), 0.into(), 200.into(), 200.into()]);

        document.objects.insert((1, 0), Object::Dictionary(catalog));
        document.objects.insert((2, 0), Object::Dictionary(pages));
        document.objects.insert((3, 0), Object::Dictionary(page));
        document.trailer.set("Root", Object::Reference((1, 0)));
        document.max_id = 5;
        document
    }

    fn action_dictionary(action: &str) -> Dictionary {
        let mut action_dictionary = Dictionary::new();
        action_dictionary.set("Type", "Action");
        action_dictionary.set("S", action);
        action_dictionary
    }

    struct DocumentBuilder {
        document: Document,
    }

    impl DocumentBuilder {
        fn with_catalog_entry(mut self, key: &str, value: Object) -> Self {
            self.document
                .get_dictionary_mut((1, 0))
                .unwrap()
                .set(key, value);
            self
        }

        fn with_object(mut self, object_id: ObjectId, object: impl Into<Object>) -> Self {
            self.document.objects.insert(object_id, object.into());
            self
        }

        fn into_compressed_pdf(mut self) -> Vec<u8> {
            let mut bytes = Vec::new();
            self.document
                .save_with_options(
                    &mut bytes,
                    SaveOptions {
                        use_object_streams: true,
                        use_xref_streams: true,
                        ..Default::default()
                    },
                )
                .unwrap();
            bytes
        }
    }

    fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }
}
