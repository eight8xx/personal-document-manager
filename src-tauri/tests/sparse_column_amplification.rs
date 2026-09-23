//! 独立复现：XLSX 稀疏列引用在索引路径上的放大（审查发现 H1）。
//!
//! 实测结论（修复前，本文件所在分支）：单张工作表、3 行、行内只有一个远列单元格
//! （`ZZZZZ` = 列索引 12,356,630）、XML 仅 1,597 字节时，`extract_table_text`
//! 耗时 **3.59 秒**；`ZZZZ` 50 行 0.82 秒；`XFD`（Excel 真实最大列）400 行 0.14 秒。
//! 放大来自 `SheetParser::finish_cell` 的 `while row_cells.len() < cell.column`
//! 逐列补空字符串，而索引路径把列上限传成 `usize::MAX`（预览路径被收敛在上限内，
//! 所以不受影响）。
//!
//! 因此这里断言的是**有界性**：小文件里的远列引用必须让解析快速完成，且不得因为
//! 列号很远就按列号放大工作量。断言可观察量（耗时量级、保留的单元格槽位数），不依赖实现细节。

use std::io::Write;
use std::time::{Duration, Instant};

use flate2::write::DeflateEncoder;
use flate2::Compression;
use personal_document_manager_lib::library::table::{extract_table_text, read_xlsx_table};
use personal_document_manager_lib::library::ArchiveLimits;

/// 小文件里的远列引用应当在远小于此的时间内完成；修复前实测 3.59 秒。
const BOUNDED_PARSE_BUDGET: Duration = Duration::from_millis(500);

const WORKBOOK_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml";
const WORKSHEET_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml";

fn deflate(raw: &[u8]) -> Vec<u8> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(raw).unwrap();
    encoder.finish().unwrap()
}

fn push_u16(target: &mut Vec<u8>, value: u16) {
    target.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(target: &mut Vec<u8>, value: u32) {
    target.extend_from_slice(&value.to_le_bytes());
}

fn zip_package(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut archive = Vec::new();
    let mut central = Vec::new();
    for (name, raw) in entries {
        let compressed = deflate(raw);
        let offset = archive.len() as u32;
        push_u32(&mut archive, 0x0403_4b50);
        push_u16(&mut archive, 20);
        push_u16(&mut archive, 0);
        push_u16(&mut archive, 8);
        push_u16(&mut archive, 0);
        push_u16(&mut archive, 0);
        push_u32(&mut archive, 0);
        push_u32(&mut archive, compressed.len() as u32);
        push_u32(&mut archive, raw.len() as u32);
        push_u16(&mut archive, name.len() as u16);
        push_u16(&mut archive, 0);
        archive.extend_from_slice(name.as_bytes());
        archive.extend_from_slice(&compressed);

        push_u32(&mut central, 0x0201_4b50);
        push_u16(&mut central, 20);
        push_u16(&mut central, 20);
        push_u16(&mut central, 0);
        push_u16(&mut central, 8);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u32(&mut central, 0);
        push_u32(&mut central, compressed.len() as u32);
        push_u32(&mut central, raw.len() as u32);
        push_u16(&mut central, name.len() as u16);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u32(&mut central, 0);
        push_u32(&mut central, offset);
        central.extend_from_slice(name.as_bytes());
    }
    let central_offset = archive.len() as u32;
    let central_size = central.len() as u32;
    archive.extend_from_slice(&central);
    push_u32(&mut archive, 0x0605_4b50);
    push_u16(&mut archive, 0);
    push_u16(&mut archive, 0);
    push_u16(&mut archive, entries.len() as u16);
    push_u16(&mut archive, entries.len() as u16);
    push_u32(&mut archive, central_size);
    push_u32(&mut archive, central_offset);
    push_u16(&mut archive, 0);
    archive
}

/// 单张工作表、每行一个单元格；列引用由调用方给出，用来放大列索引。
fn workbook_with_column_reference(column_reference: &str, rows: usize) -> Vec<u8> {
    let content_types = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/xl/workbook.xml" ContentType="{WORKBOOK_CONTENT_TYPE}"/>
<Override PartName="/xl/worksheets/sheet1.xml" ContentType="{WORKSHEET_CONTENT_TYPE}"/>
</Types>"#
    );
    let root_relationships = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#;
    let workbook = r#"<?xml version="1.0" encoding="UTF-8"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
 xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<sheets><sheet name="稀疏" sheetId="1" r:id="rId1"/></sheets>
</workbook>"#;
    let workbook_relationships = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
</Relationships>"#;

    let mut sheet = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#,
    );
    for row in 1..=rows {
        sheet.push_str(&format!(
            r#"<row r="{row}"><c r="{column_reference}{row}" t="str"><v>x</v></c></row>"#
        ));
    }
    sheet.push_str("</sheetData></worksheet>");

    zip_package(&[
        ("[Content_Types].xml", content_types.into_bytes()),
        ("_rels/.rels", root_relationships.as_bytes().to_vec()),
        ("xl/workbook.xml", workbook.as_bytes().to_vec()),
        (
            "xl/_rels/workbook.xml.rels",
            workbook_relationships.as_bytes().to_vec(),
        ),
        ("xl/worksheets/sheet1.xml", sheet.into_bytes()),
    ])
}

fn retained_cell_slots(cells: &[Vec<String>]) -> usize {
    cells.iter().map(|row| row.len()).sum()
}

#[test]
fn sparse_far_column_does_not_amplify_work_in_the_index_path() {
    // Excel 真实最大列 XFD 对应列索引 16383；400 行 XML 只有几 KB。
    let bytes = workbook_with_column_reference("XFD", 400);
    assert!(bytes.len() < 64 * 1024, "样本应当很小：{} 字节", bytes.len());

    let started = Instant::now();
    let extracted = extract_table_text(&bytes, "XLSX").expect("远列引用不应让索引路径失败");
    let elapsed = started.elapsed();
    assert!(
        extracted.contains('x'),
        "有值的单元格文字仍应进入索引：{extracted:?}"
    );
    assert!(
        elapsed < BOUNDED_PARSE_BUDGET,
        "远列引用不得按列号放大工作量：{} 行、{} 字节的样本耗时 {elapsed:?}",
        400,
        bytes.len()
    );

    // 预览路径同样不能被列号放大：列数被收敛在上限内，槽位数必须远小于列索引。
    let preview = read_xlsx_table(&bytes, 0, 0, 10, 8, ArchiveLimits::for_table_preview())
        .expect("预览不应失败");
    let slots = retained_cell_slots(&preview.cells);
    assert!(
        slots <= 10 * 8,
        "预览保留的单元格槽位应与请求范围同量级，实际 {slots}"
    );
}

#[test]
fn extreme_column_reference_stays_bounded() {
    // ZZZZZ = 12,356,630，远超 Excel 的 16383。修复前这个 1.6 KB 样本要跑 3.59 秒。
    let bytes = workbook_with_column_reference("ZZZZZ", 3);
    assert!(bytes.len() < 8 * 1024, "样本应当很小：{} 字节", bytes.len());

    let started = Instant::now();
    let outcome = extract_table_text(&bytes, "XLSX");
    let elapsed = started.elapsed();

    if let Ok(text) = &outcome {
        assert!(
            text.contains('x'),
            "接受该引用时，远列的文字仍应被提取：{text:?}"
        );
    }
    assert!(
        elapsed < BOUNDED_PARSE_BUDGET,
        "极端列引用必须被拒绝或有界处理：{} 字节样本耗时 {elapsed:?}",
        bytes.len()
    );
}
