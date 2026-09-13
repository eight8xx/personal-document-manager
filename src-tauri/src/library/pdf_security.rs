use std::io::Read;

use flate2::read::ZlibDecoder;

use super::error::{LibraryError, LibraryResult};

const MAX_OBJECT_STREAM_BYTES: u64 = 64 * 1024 * 1024;
const DANGEROUS_PDF_NAMES: &[&str] = &[
    "openaction",
    "aa",
    "javascript",
    "js",
    "launch",
    "uri",
    "embeddedfile",
    "richmedia",
    "xfa",
    "gotoR",
    "gotoE",
    "submitform",
    "importdata",
    "rendition",
    "movie",
    "sound",
    "3d",
    "encrypt",
];

pub(super) fn ensure_safe_pdf_preview(bytes: &[u8]) -> LibraryResult<()> {
    let streams = scan_pdf_names(bytes)?;
    for stream in streams {
        if !dictionary_is_object_stream(stream.dictionary) {
            continue;
        }
        let decoded = decode_object_stream(stream.dictionary, stream.data)?;
        scan_pdf_names(&decoded)?;
    }
    Ok(())
}

struct PdfStream<'a> {
    dictionary: &'a [u8],
    data: &'a [u8],
}

fn scan_pdf_names(bytes: &[u8]) -> LibraryResult<Vec<PdfStream<'_>>> {
    let mut cursor = 0;
    let mut streams = Vec::new();
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'%' => {
                cursor = bytes[cursor..]
                    .iter()
                    .position(|byte| matches!(*byte, b'\r' | b'\n'))
                    .map(|offset| cursor + offset + 1)
                    .unwrap_or(bytes.len());
            }
            b'(' => {
                cursor = skip_literal_string(bytes, cursor)?;
            }
            b'<' if bytes.get(cursor + 1) == Some(&b'<') => {
                cursor += 2;
            }
            b'<' => {
                cursor = skip_hex_string(bytes, cursor)?;
            }
            b'/' => {
                let (name, next) = parse_pdf_name(bytes, cursor)?;
                if dangerous_name(&name) {
                    return Err(LibraryError::UnsafePreview(format!(
                        "PDF 包含会在预览时被禁止的对象：/{}。",
                        display_pdf_name(&name)
                    )));
                }
                cursor = next;
            }
            byte if is_pdf_regular(byte) => {
                let (keyword, next) = parse_pdf_keyword(bytes, cursor);
                if keyword.eq_ignore_ascii_case(b"stream") {
                    let dictionary_start = dictionary_start_before(bytes, cursor);
                    let mut data_start = next;
                    if bytes.get(data_start) == Some(&b'\r') {
                        data_start += 1;
                    }
                    if bytes.get(data_start) == Some(&b'\n') {
                        data_start += 1;
                    }
                    let data_end = find_bytes(&bytes[data_start..], b"endstream")
                        .map(|offset| data_start + offset)
                        .ok_or_else(|| {
                            LibraryError::UnsafePreview(
                                "PDF 对象流缺少结束标记，无法验证安全性。".to_string(),
                            )
                        })?;
                    streams.push(PdfStream {
                        dictionary: &bytes[dictionary_start..cursor],
                        data: &bytes[data_start..data_end],
                    });
                    cursor = data_end + b"endstream".len();
                } else {
                    cursor = next;
                }
            }
            _ => cursor += 1,
        }
    }
    Ok(streams)
}

fn skip_literal_string(bytes: &[u8], start: usize) -> LibraryResult<usize> {
    let mut cursor = start + 1;
    let mut depth = 1_u32;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => {
                cursor = (cursor + 2).min(bytes.len());
            }
            b'(' => {
                depth += 1;
                cursor += 1;
            }
            b')' => {
                depth -= 1;
                cursor += 1;
                if depth == 0 {
                    return Ok(cursor);
                }
            }
            _ => cursor += 1,
        }
    }
    Err(LibraryError::UnsafePreview(
        "PDF 字符串未闭合，无法验证安全性。".to_string(),
    ))
}

fn skip_hex_string(bytes: &[u8], start: usize) -> LibraryResult<usize> {
    let end = find_bytes(&bytes[start + 1..], b">")
        .map(|offset| start + 1 + offset + 1)
        .ok_or_else(|| {
            LibraryError::UnsafePreview("PDF 十六进制字符串未闭合，无法验证安全性。".to_string())
        })?;
    Ok(end)
}

fn parse_pdf_name(bytes: &[u8], start: usize) -> LibraryResult<(Vec<u8>, usize)> {
    let mut cursor = start + 1;
    let mut name = Vec::new();
    while cursor < bytes.len()
        && !bytes[cursor].is_ascii_whitespace()
        && !is_pdf_delimiter(bytes[cursor])
    {
        if bytes[cursor] == b'#' {
            let high = bytes.get(cursor + 1).and_then(|byte| hex_value(*byte));
            let low = bytes.get(cursor + 2).and_then(|byte| hex_value(*byte));
            let (Some(high), Some(low)) = (high, low) else {
                return Err(LibraryError::UnsafePreview(
                    "PDF 名称包含无效转义，无法验证安全性。".to_string(),
                ));
            };
            name.push((high << 4) | low);
            cursor += 3;
        } else {
            name.push(bytes[cursor].to_ascii_lowercase());
            cursor += 1;
        }
    }
    if name.is_empty() {
        return Err(LibraryError::UnsafePreview(
            "PDF 名称 token 为空，无法验证安全性。".to_string(),
        ));
    }
    Ok((name, cursor))
}

fn parse_pdf_keyword(bytes: &[u8], start: usize) -> (&[u8], usize) {
    let mut cursor = start;
    while cursor < bytes.len() && is_pdf_regular(bytes[cursor]) {
        cursor += 1;
    }
    (&bytes[start..cursor], cursor)
}

fn dangerous_name(name: &[u8]) -> bool {
    DANGEROUS_PDF_NAMES
        .iter()
        .any(|dangerous| name.eq_ignore_ascii_case(dangerous.as_bytes()))
}

fn display_pdf_name(name: &[u8]) -> String {
    String::from_utf8_lossy(name).into_owned()
}

fn is_pdf_regular(byte: u8) -> bool {
    !is_pdf_delimiter(byte) && !byte.is_ascii_whitespace()
}

fn is_pdf_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn dictionary_start_before(bytes: &[u8], stream_keyword: usize) -> usize {
    bytes[..stream_keyword]
        .windows(2)
        .rposition(|window| window == b"<<")
        .unwrap_or(0)
}

fn dictionary_is_object_stream(dictionary: &[u8]) -> bool {
    contains_pdf_name(dictionary, b"objstm") && contains_pdf_name(dictionary, b"type")
}

fn contains_pdf_name(bytes: &[u8], expected: &[u8]) -> bool {
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] == b'/' {
            if let Ok((name, next)) = parse_pdf_name(bytes, cursor) {
                if name.eq_ignore_ascii_case(expected) {
                    return true;
                }
                cursor = next;
                continue;
            }
        }
        cursor += 1;
    }
    false
}

fn decode_object_stream(dictionary: &[u8], compressed: &[u8]) -> LibraryResult<Vec<u8>> {
    if !contains_pdf_name(dictionary, b"flatedecode") && !contains_pdf_name(dictionary, b"fl") {
        return Err(LibraryError::UnsafePreview(
            "PDF 对象流使用了无法安全解析的编码。".to_string(),
        ));
    }
    let mut decoder = ZlibDecoder::new(compressed).take(MAX_OBJECT_STREAM_BYTES + 1);
    let mut decoded = Vec::new();
    decoder
        .read_to_end(&mut decoded)
        .map_err(|error| LibraryError::UnsafePreview(format!("无法解压 PDF 对象流：{error}")))?;
    if decoded.len() as u64 > MAX_OBJECT_STREAM_BYTES {
        return Err(LibraryError::UnsafePreview(
            "PDF 对象流过大，无法安全验证。".to_string(),
        ));
    }
    Ok(decoded)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::write::ZlibEncoder;

    use super::*;

    #[test]
    fn allows_a_safe_pdf_dictionary_and_ignores_stream_text() {
        let pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Length 34 >> stream\n(/JavaScript /OpenAction) Tj\nendstream endobj\n%%EOF";
        assert!(ensure_safe_pdf_preview(pdf).is_ok());
    }

    #[test]
    fn rejects_direct_and_indirect_active_objects() {
        assert_eq!(
            parse_pdf_name(b"/OpenAction 2 0 R", 0).unwrap().0,
            b"openaction"
        );
        let open_action = b"%PDF-1.7\n1 0 obj << /Type /Catalog /OpenAction 2 0 R >> endobj\n%%EOF";
        assert!(matches!(
            ensure_safe_pdf_preview(open_action),
            Err(LibraryError::UnsafePreview(_))
        ));

        let javascript = b"%PDF-1.7\n1 0 obj << /S /JavaScript /JS (app.alert(1)) >> endobj\n%%EOF";
        assert!(matches!(
            ensure_safe_pdf_preview(javascript),
            Err(LibraryError::UnsafePreview(_))
        ));
    }

    #[test]
    fn rejects_active_objects_hidden_in_a_flate_compressed_object_stream() {
        let pdf = object_stream_pdf(b"5 0 << /S /JavaScript /JS (alert(1)) >>");
        assert!(matches!(
            ensure_safe_pdf_preview(&pdf),
            Err(LibraryError::UnsafePreview(_))
        ));
    }

    #[test]
    fn escapes_in_names_cannot_hide_active_objects() {
        let pdf = b"%PDF-1.7\n1 0 obj << /Open#41ction 2 0 R >> endobj\n%%EOF";
        assert!(matches!(
            ensure_safe_pdf_preview(pdf),
            Err(LibraryError::UnsafePreview(_))
        ));
    }

    fn object_stream_pdf(contents: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(contents).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut pdf =
            b"%PDF-1.7\n6 0 obj << /Type /ObjStm /N 1 /First 4 /Filter /FlateDecode /Length "
                .to_vec();
        pdf.extend_from_slice(compressed.len().to_string().as_bytes());
        pdf.extend_from_slice(b" >>\nstream\n");
        pdf.extend_from_slice(&compressed);
        pdf.extend_from_slice(b"\nendstream\nendobj\n%%EOF\n");
        pdf
    }
}
