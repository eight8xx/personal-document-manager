//! 表格解析层（工作单 08/09）的真实字节集成测试。
//!
//! CSV 与 XLSX 样本都在测试里手工构造：CSV 直接给字节，XLSX 用最小 OOXML 包
//! （与 `library_service.rs` 构造 docx/pptx 的做法一致）。所有断言只看公开行为：
//! 解析结果、上限拒绝、拒绝原因与文本提取，不依赖私有函数。

use std::io::Write;

use flate2::write::DeflateEncoder;
use flate2::Compression;
use personal_document_manager_lib::library::table::{
    extract_table_text, read_csv_table, read_xlsx_table, validate_table_document,
    MAX_TABLE_INDEX_CELLS, MAX_XLSX_COLUMN_INDEX, UNCACHED_FORMULA_TEXT,
};
use personal_document_manager_lib::library::{
    ArchiveLimits, MAX_EXTRACTED_TEXT_CHARS, MAX_TABLE_PREVIEW_COLUMNS, MAX_TABLE_PREVIEW_ROWS,
};

/// 与 `library::limits::MAX_TABLE_CELL_CHARS` 一致；该常量未从 `library` 重导出。
const TABLE_CELL_CHAR_LIMIT: usize = 4096;

const WORKBOOK_MAIN_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml";
const WORKSHEET_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml";
const SHARED_STRINGS_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml";
const WORKSHEET_RELATIONSHIP: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet";
const SHARED_STRINGS_RELATIONSHIP: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings";

fn preview_limits() -> ArchiveLimits {
    ArchiveLimits::for_table_preview()
}

// ---------------------------------------------------------------------------
// CSV 样本
// ---------------------------------------------------------------------------

fn csv_bytes(text: &str) -> Vec<u8> {
    text.as_bytes().to_vec()
}

#[test]
fn csv_reads_quoted_separators_newlines_and_empty_cells() {
    let csv = "名称,备注,数量\n\"含,逗号\",\"第一行\n第二行\",\n\"引号\"\"内容\",,";
    let range = read_csv_table(&csv_bytes(csv), 0, 50, 10, preview_limits()).unwrap();

    assert_eq!(range.sheet_index, 0);
    assert_eq!(range.sheets.len(), 1);
    assert_eq!(range.sheets[0].name, "CSV");
    assert_eq!(range.sheets[0].row_count, Some(3));
    assert_eq!(range.sheets[0].column_count, Some(3));
    assert_eq!(range.cells[0], vec!["名称", "备注", "数量"]);
    assert_eq!(range.cells[1], vec!["含,逗号", "第一行\n第二行", ""]);
    assert_eq!(range.cells[2], vec!["引号\"内容", "", ""]);
    assert_eq!(range.column_count, 10);
    assert!(!range.has_more_rows);
    assert!(range.notice.is_none());
    assert!(range.degraded_features.is_empty());
}

#[test]
fn csv_ignores_bom_and_keeps_chinese_and_english_cells() {
    let mut bytes = vec![0xef, 0xbb, 0xbf];
    bytes.extend_from_slice("名称,Name\n笔记本,Notebook 16\n".as_bytes());

    let range = read_csv_table(&bytes, 0, 10, 5, preview_limits()).unwrap();

    assert_eq!(range.notice.as_deref(), Some("已忽略文件开头的 UTF-8 BOM。"));
    assert_eq!(range.cells[0], vec!["名称", "Name"]);
    assert_eq!(range.cells[1], vec!["笔记本", "Notebook 16"]);
    assert_eq!(range.sheets[0].row_count, Some(2));
    assert!(!range.cells[0][0].contains('\u{feff}'));
}

#[test]
fn csv_skips_blank_lines_and_keeps_rows_of_only_separators() {
    let csv = "a,b\n\n\n,,c\n";
    let range = read_csv_table(&csv_bytes(csv), 0, 10, 5, preview_limits()).unwrap();

    assert_eq!(range.sheets[0].row_count, Some(2));
    assert_eq!(range.cells[0], vec!["a", "b"]);
    assert_eq!(range.cells[1], vec!["", "", "c"]);
}

#[test]
fn csv_rejects_invalid_utf8_utf16_binary_and_archives() {
    let gbk = [0xc3, 0xe6, 0xb3, 0xc6, b'\n'];
    let error = read_csv_table(&gbk, 0, 10, 5, preview_limits()).unwrap_err();
    assert!(
        error.to_string().contains("UTF-8"),
        "unexpected error: {error}"
    );

    let utf16 = [0xff, 0xfe, b'a', 0x00, b',', 0x00];
    let error = read_csv_table(&utf16, 0, 10, 5, preview_limits()).unwrap_err();
    assert!(error.to_string().contains("UTF-16"), "unexpected: {error}");

    let binary = b"a,b\nc,\x00d\n";
    let error = read_csv_table(binary, 0, 10, 5, preview_limits()).unwrap_err();
    assert!(
        error.to_string().contains("二进制"),
        "unexpected: {error}"
    );

    let renamed_archive = b"PK\x03\x04\x14\x00\x00\x00";
    let error = read_csv_table(renamed_archive, 0, 10, 5, preview_limits()).unwrap_err();
    assert!(error.to_string().contains("压缩包"), "unexpected: {error}");
}

#[test]
fn csv_rejects_files_over_the_input_limit() {
    let limits = ArchiveLimits {
        max_input_bytes: 16,
        max_entry_bytes: 64,
        max_expanded_bytes: 128,
        max_compression_ratio: 10,
    };
    let csv = "a,b,c\n1,2,3\n4,5,6\n";
    assert!(csv.len() > 16);

    let error = read_csv_table(&csv_bytes(csv), 0, 10, 5, limits).unwrap_err();
    assert!(error.to_string().contains("文档过大"), "unexpected: {error}");
}

#[test]
fn csv_truncates_over_long_cells_and_reports_the_degradation() {
    let long = "中".repeat(TABLE_CELL_CHAR_LIMIT + 25);
    let csv = format!("短,{long}\n");
    let range = read_csv_table(&csv_bytes(&csv), 0, 10, 5, preview_limits()).unwrap();

    assert_eq!(range.cells[0][0], "短");
    assert_eq!(range.cells[0][1].chars().count(), TABLE_CELL_CHAR_LIMIT);
    assert!(
        range
            .degraded_features
            .iter()
            .any(|feature| feature.contains("字符")),
        "unexpected degradations: {:?}",
        range.degraded_features
    );
}

#[test]
fn csv_pages_long_files_and_clamps_row_and_column_requests() {
    let mut csv = String::new();
    for index in 1..=600 {
        csv.push_str(&format!("行{index},值{index}\n"));
    }

    let first = read_csv_table(&csv_bytes(&csv), 0, 100_000, 1_000, preview_limits()).unwrap();
    assert_eq!(first.sheets[0].row_count, Some(600));
    assert_eq!(first.cells.len(), MAX_TABLE_PREVIEW_ROWS);
    assert_eq!(first.column_count, MAX_TABLE_PREVIEW_COLUMNS);
    assert!(first.has_more_rows);
    assert_eq!(first.cells[0][0], "行1");
    assert_eq!(first.cells[MAX_TABLE_PREVIEW_ROWS - 1][0], "行500");
    assert!(
        first.cells.iter().all(|row| row.len() <= first.column_count),
        "返回范围不得超过请求的列上限"
    );

    let second = read_csv_table(&csv_bytes(&csv), 500, 100, 2, preview_limits()).unwrap();
    assert_eq!(second.start_row, 500);
    assert_eq!(second.cells.len(), 100);
    assert!(!second.has_more_rows);
    assert_eq!(second.cells[0], vec!["行501", "值501"]);
    assert_eq!(second.cells[99], vec!["行600", "值600"]);
}

#[test]
fn csv_counts_rows_consistently_when_a_skipped_row_contains_a_quote() {
    // 第 3 行含裸引号（字段中间的引号不是引号语法的一部分）。窗口之外的行也必须
    // 用同样的引号规则扫描，否则分页得到的总行数会与整表读取不一致。
    let csv = "a,b\nc,d\ne\"f,g\nh,i\n";
    let whole = read_csv_table(&csv_bytes(csv), 0, 10, 5, preview_limits()).unwrap();
    assert_eq!(whole.sheets[0].row_count, Some(4));
    assert_eq!(whole.cells[2], vec!["e\"f", "g"]);

    let last_page = read_csv_table(&csv_bytes(csv), 3, 10, 5, preview_limits()).unwrap();
    assert_eq!(
        last_page.sheets[0].row_count,
        Some(4),
        "窗口之外的行必须按同样的引号规则计数"
    );
    assert_eq!(last_page.cells[0], vec!["h", "i"]);
    assert!(!last_page.has_more_rows);
}

#[test]
fn csv_extracts_bounded_visible_text_for_indexing() {
    let csv = "\u{feff}名称,数量\n笔记本,2\n含,逗号\n";
    let text = extract_table_text(&csv_bytes(csv), "CSV").unwrap();

    assert!(text.contains("笔记本"), "unexpected text: {text}");
    assert!(text.contains("含 逗号"), "unexpected text: {text}");
    assert!(!text.contains('\u{feff}'));
    assert!(text.chars().count() <= MAX_EXTRACTED_TEXT_CHARS);

    let error = extract_table_text(b"a,b\n", "PDF").unwrap_err();
    assert!(error.to_string().contains("不是表格文档"));
}

#[test]
fn csv_validation_accepts_text_and_rejects_misnamed_or_undecodable_files() {
    assert!(validate_table_document("a,b\n1,2\n".as_bytes(), "csv").is_ok());
    assert!(validate_table_document("".as_bytes(), "CSV").is_ok());

    let error = validate_table_document(&[0xc3, 0xe6], "CSV").unwrap_err();
    assert!(error.contains("UTF-8"), "unexpected: {error}");

    let error = validate_table_document(b"PK\x03\x04xxxx", "csv").unwrap_err();
    assert!(error.contains("压缩包"), "unexpected: {error}");
}

// ---------------------------------------------------------------------------
// XLSX 样本
// ---------------------------------------------------------------------------

fn content_types_xml(sheet_count: usize, with_shared_strings: bool) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>",
    );
    xml.push_str(&format!(
        "<Override PartName=\"/xl/workbook.xml\" ContentType=\"{WORKBOOK_MAIN_CONTENT_TYPE}\"/>"
    ));
    if with_shared_strings {
        xml.push_str(&format!(
            "<Override PartName=\"/xl/sharedStrings.xml\" ContentType=\"{SHARED_STRINGS_CONTENT_TYPE}\"/>"
        ));
    }
    for number in 1..=sheet_count {
        xml.push_str(&format!(
            "<Override PartName=\"/xl/worksheets/sheet{number}.xml\" ContentType=\"{WORKSHEET_CONTENT_TYPE}\"/>"
        ));
    }
    xml.push_str("</Types>");
    xml
}

fn workbook_xml(sheet_names: &[&str]) -> String {
    let mut sheets = String::new();
    for (index, name) in sheet_names.iter().enumerate() {
        let number = index + 1;
        sheets.push_str(&format!(
            "<sheet name=\"{name}\" sheetId=\"{number}\" r:id=\"rId{number}\"/>"
        ));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
         <sheets>{sheets}</sheets></workbook>"
    )
}

fn workbook_rels_xml(sheet_count: usize, with_shared_strings: bool) -> String {
    let mut relationships = String::new();
    for number in 1..=sheet_count {
        relationships.push_str(&format!(
            "<Relationship Id=\"rId{number}\" Type=\"{WORKSHEET_RELATIONSHIP}\" Target=\"worksheets/sheet{number}.xml\"/>"
        ));
    }
    if with_shared_strings {
        let id = sheet_count + 1;
        relationships.push_str(&format!(
            "<Relationship Id=\"rId{id}\" Type=\"{SHARED_STRINGS_RELATIONSHIP}\" Target=\"sharedStrings.xml\"/>"
        ));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{relationships}</Relationships>"
    )
}

fn worksheet_xml(sheet_data: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
         <sheetData>{sheet_data}</sheetData></worksheet>"
    )
}

/// 生成 `row_count` 行 `columns` 列的内联字符串单元格。
fn inline_rows(prefix: &str, row_count: usize, columns: usize) -> String {
    let mut xml = String::new();
    for row in 1..=row_count {
        xml.push_str(&format!("<row r=\"{row}\">"));
        for column in 0..columns {
            let letter = column_letter(column);
            xml.push_str(&format!(
                "<c r=\"{letter}{row}\" t=\"inlineStr\"><is><t>{prefix}{row}列{}</t></is></c>",
                column + 1
            ));
        }
        xml.push_str("</row>");
    }
    xml
}

fn column_letter(column: usize) -> String {
    let mut index = column;
    let mut letters = Vec::new();
    loop {
        letters.push((b'A' + (index % 26) as u8) as char);
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    letters.iter().rev().collect()
}

fn xlsx_fixture(
    sheet_names: &[&str],
    sheet_parts: &[String],
    shared_strings: Option<&str>,
    extras: &[(&str, &[u8])],
) -> Vec<u8> {
    assert_eq!(sheet_names.len(), sheet_parts.len());
    let mut entries: Vec<(String, Vec<u8>)> = vec![
        (
            "[Content_Types].xml".to_string(),
            content_types_xml(sheet_names.len(), shared_strings.is_some()).into_bytes(),
        ),
        (
            "xl/workbook.xml".to_string(),
            workbook_xml(sheet_names).into_bytes(),
        ),
        (
            "xl/_rels/workbook.xml.rels".to_string(),
            workbook_rels_xml(sheet_names.len(), shared_strings.is_some()).into_bytes(),
        ),
    ];
    for (index, part) in sheet_parts.iter().enumerate() {
        entries.push((
            format!("xl/worksheets/sheet{}.xml", index + 1),
            part.as_bytes().to_vec(),
        ));
    }
    if let Some(shared) = shared_strings {
        entries.push(("xl/sharedStrings.xml".to_string(), shared.as_bytes().to_vec()));
    }
    for (name, contents) in extras {
        entries.push(((*name).to_string(), contents.to_vec()));
    }
    stored_zip(&entries)
}

#[test]
fn xlsx_lists_worksheets_and_only_reads_the_selected_one() {
    let summary = worksheet_xml(
        "<row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>汇总甲</t></is></c></row>",
    );
    let detail = worksheet_xml(
        "<row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>明细甲</t></is></c>\
         <c r=\"B1\" t=\"inlineStr\"><is><t>明细乙</t></is></c></row>\
         <row r=\"2\"><c r=\"A2\" t=\"inlineStr\"><is><t>明细丙</t></is></c></row>",
    );
    let package = xlsx_fixture(&["汇总", "明细"], &[summary, detail], None, &[]);

    let range = read_xlsx_table(&package, 1, 0, 50, 10, preview_limits()).unwrap();
    assert_eq!(range.sheet_index, 1);
    assert_eq!(
        range
            .sheets
            .iter()
            .map(|sheet| sheet.name.as_str())
            .collect::<Vec<_>>(),
        vec!["汇总", "明细"]
    );
    assert_eq!(range.sheets[0].row_count, None, "未读取的工作表不给出统计");
    assert_eq!(range.sheets[1].row_count, Some(2));
    assert_eq!(range.sheets[1].column_count, Some(2));
    assert_eq!(range.cells[0], vec!["明细甲", "明细乙"]);
    assert_eq!(range.cells[1], vec!["明细丙"]);
    assert!(!range.has_more_rows);

    let error = read_xlsx_table(&package, 5, 0, 10, 5, preview_limits()).unwrap_err();
    assert!(
        error.to_string().contains("工作表 5 不存在"),
        "unexpected: {error}"
    );
}

#[test]
fn xlsx_resolves_shared_strings_including_rich_text_and_empty_entries() {
    let shared = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
        <sst xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" count=\"4\">\
        <si><t>合计</t></si>\
        <si><r><rPr><b/></rPr><t>中</t></r><r><t>文</t></r></si>\
        <si><t xml:space=\"preserve\"> 保留空格 </t></si>\
        <si/></sst>";
    let sheet = worksheet_xml(
        "<row r=\"1\">\
         <c r=\"A1\" t=\"s\"><v>0</v></c>\
         <c r=\"B1\" t=\"s\"><v>1</v></c>\
         <c r=\"C1\" t=\"s\"><v>2</v></c>\
         <c r=\"D1\" t=\"s\"><v>3</v></c></row>\
         <row r=\"2\"><c r=\"B2\" t=\"s\"><v>1</v></c></row>",
    );
    let package = xlsx_fixture(&["数据"], &[sheet], Some(shared), &[]);

    let range = read_xlsx_table(&package, 0, 0, 50, 10, preview_limits()).unwrap();
    assert_eq!(range.cells[0], vec!["合计", "中文", " 保留空格 "]);
    assert_eq!(range.cells[1], vec!["", "中文"]);
    // 空字符串单元格不占位，行尾不补齐；列范围由有值的单元格决定。
    assert_eq!(range.sheets[0].column_count, Some(3));

    let broken_shared =
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><sst xmlns=\"x\"><si><t>只有一项</t></si></sst>";
    let broken = worksheet_xml("<row r=\"1\"><c r=\"A1\" t=\"s\"><v>7</v></c></row>");
    let package = xlsx_fixture(
        &["数据"],
        std::slice::from_ref(&broken),
        Some(broken_shared),
        &[],
    );
    let error = read_xlsx_table(&package, 0, 0, 10, 5, preview_limits()).unwrap_err();
    assert!(error.to_string().contains("越界"), "unexpected: {error}");

    let missing = xlsx_fixture(&["数据"], std::slice::from_ref(&broken), None, &[]);
    let error = read_xlsx_table(&missing, 0, 0, 10, 5, preview_limits()).unwrap_err();
    assert!(
        error.to_string().contains("sharedStrings"),
        "unexpected: {error}"
    );
}

#[test]
fn xlsx_positions_sparse_rows_and_cells_by_reference() {
    let sheet = worksheet_xml(
        "<row r=\"1\">\
         <c r=\"A1\" t=\"inlineStr\"><is><t>甲</t></is></c>\
         <c r=\"C1\" t=\"inlineStr\"><is><t>丙</t></is></c></row>\
         <row r=\"100\"><c r=\"B100\" t=\"inlineStr\"><is><t>乙</t></is></c></row>",
    );
    let package = xlsx_fixture(&["稀疏"], &[sheet], None, &[]);

    let first = read_xlsx_table(&package, 0, 0, 50, 10, preview_limits()).unwrap();
    assert_eq!(first.cells.len(), 1, "只有第一行带值");
    assert_eq!(first.cells[0], vec!["甲", "", "丙"], "内部空洞按列索引定位");
    assert_eq!(first.sheets[0].row_count, Some(100));
    assert_eq!(first.sheets[0].column_count, Some(3));
    assert!(first.has_more_rows);

    let second = read_xlsx_table(&package, 0, 50, 50, 10, preview_limits()).unwrap();
    assert_eq!(second.cells, vec![vec!["", "乙"]]);
    assert_eq!(second.start_row, 50);
    assert!(!second.has_more_rows);
}

#[test]
fn xlsx_reads_formula_cached_values_without_evaluating() {
    let sheet = worksheet_xml(
        "<row r=\"1\">\
         <c r=\"A1\"><f>1/0</f><v>7</v></c>\
         <c r=\"B1\" t=\"str\"><f>CONCAT(\"a\",\"b\")</f><v>ab</v></c>\
         <c r=\"C1\" t=\"e\"><v>#DIV/0!</v></c>\
         <c r=\"D1\" t=\"b\"><v>1</v></c></row>\
         <row r=\"2\"><c r=\"A2\"><f>SUM(A1:A1)</f><v>16</v></c><c r=\"D2\" t=\"b\"><v>0</v></c></row>",
    );
    let package = xlsx_fixture(&["公式"], &[sheet], None, &[]);

    let range = read_xlsx_table(&package, 0, 0, 50, 10, preview_limits()).unwrap();
    assert_eq!(range.cells[0], vec!["7", "ab", "#DIV/0!", "TRUE"]);
    assert_eq!(range.cells[1], vec!["16", "", "", "FALSE"]);
    assert!(range.degraded_features.is_empty());

    let text = extract_table_text(&package, "XLSX").unwrap();
    assert!(text.contains('7') && text.contains("ab"));
    assert!(
        !text.contains("1/0") && !text.contains("CONCAT") && !text.contains("SUM"),
        "公式文本不得进入索引：{text}"
    );
}

#[test]
fn xlsx_reports_uncached_formulas_without_evaluating() {
    let sheet = worksheet_xml(
        "<row r=\"1\">\
         <c r=\"A1\"><f>SUM(B1:B2)</f></c>\
         <c r=\"B1\" t=\"inlineStr\"><is><t>有缓存</t></is></c></row>",
    );
    let package = xlsx_fixture(&["公式"], &[sheet], None, &[]);

    let range = read_xlsx_table(&package, 0, 0, 50, 10, preview_limits()).unwrap();
    assert_eq!(range.cells[0], vec![UNCACHED_FORMULA_TEXT, "有缓存"]);
    assert!(
        range
            .degraded_features
            .iter()
            .any(|feature| feature.contains("缓存值")),
        "unexpected degradations: {:?}",
        range.degraded_features
    );

    let text = extract_table_text(&package, "XLSX").unwrap();
    assert!(text.contains("有缓存"));
    assert!(
        !text.contains(UNCACHED_FORMULA_TEXT) && !text.contains("SUM"),
        "未缓存公式不得进入索引：{text}"
    );
}

#[test]
fn xlsx_pages_ranges_and_reports_has_more_rows() {
    let sheet = worksheet_xml(&inline_rows("数据", 120, 2));
    let package = xlsx_fixture(&["长表"], &[sheet], None, &[]);

    let first = read_xlsx_table(&package, 0, 0, 50, 10, preview_limits()).unwrap();
    assert_eq!(first.cells.len(), 50);
    assert_eq!(first.sheets[0].row_count, Some(120));
    assert!(first.has_more_rows);
    assert_eq!(first.cells[0][0], "数据1列1");
    assert_eq!(first.cells[49][0], "数据50列1");

    let last = read_xlsx_table(&package, 0, 100, 50, 10, preview_limits()).unwrap();
    assert_eq!(last.cells.len(), 20);
    assert!(!last.has_more_rows);
    assert_eq!(last.start_row, 100);
    assert_eq!(last.cells[0][0], "数据101列1");
    assert_eq!(last.cells[19][0], "数据120列1");
}

/// 超过 Excel 上限（XFD，第 16384 列）的引用是畸形文件：必须报错，而不是按列号补空单元格。
/// 合法上限 XFD 本身仍然接受。
#[test]
fn xlsx_rejects_column_references_beyond_the_excel_limit() {
    let beyond = worksheet_xml(r#"<row r="1"><c r="ZZZZZ1" t="str"><v>x</v></c></row>"#);
    let package = xlsx_fixture(&["越界"], &[beyond], None, &[]);
    let error = read_xlsx_table(&package, 0, 0, 10, 8, preview_limits()).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("XFD") || message.contains("列号"),
        "畸形列引用要给出可理解原因：{message}"
    );

    let legal = worksheet_xml(r#"<row r="1"><c r="XFD1" t="str"><v>x</v></c></row>"#);
    let package = xlsx_fixture(&["上限列"], &[legal], None, &[]);
    let range = read_xlsx_table(&package, 0, 0, 10, 8, preview_limits()).unwrap();
    assert_eq!(range.sheets[0].column_count, Some(MAX_XLSX_COLUMN_INDEX + 1));
}

/// 索引路径：稀疏远列不放大工作量（由独立复现用例覆盖），这里补「单元格预算」的有界性——
/// 有值单元格超过预算后只统计不保留，索引文本不会随单元格总数线性膨胀。
#[test]
fn xlsx_index_path_bounds_retained_cells() {
    // 3000 行 × 80 列 = 240,000 个有值单元格，超过 MAX_TABLE_INDEX_CELLS（200,000）。
    let mut sheet_data = String::new();
    for row in 1..=3000 {
        sheet_data.push_str(&format!("<row r=\"{row}\">"));
        for _ in 0..80 {
            sheet_data.push_str("<c><v>m</v></c>");
        }
        sheet_data.push_str("</row>");
    }
    let total_cells = 3000 * 80;
    let package = xlsx_fixture(&["大表"], &[worksheet_xml(&sheet_data)], None, &[]);

    let started = std::time::Instant::now();
    let text = extract_table_text(&package, "XLSX").unwrap();
    let elapsed = started.elapsed();
    let retained = text.matches('m').count();
    assert!(retained > 0, "预算内的单元格必须照常进索引");
    assert!(
        retained < total_cells,
        "超过预算后应停止保留：保留 {retained} / 共 {total_cells}"
    );
    assert!(
        retained <= MAX_TABLE_INDEX_CELLS + 256,
        "保留量应受预算约束，实际 {retained}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "有界样本不应慢到异常：{elapsed:?}"
    );
}

/// 稀疏表的翻页提示：首屏为空时给出「下一个含数据的行」，界面可以直接跳过去。
#[test]
fn xlsx_preview_reports_the_next_data_row_for_sparse_sheets() {
    // 数据从 Excel 第 1000 行（0 基索引 999）开始，前 999 行都是空的。
    let sparse = worksheet_xml(
        r#"<row r="1000"><c r="A1000" t="inlineStr"><is><t>稀疏数据</t></is></c></row>
           <row r="1001"><c r="A1001" t="inlineStr"><is><t>第二行数据</t></is></c></row>"#,
    );
    let package = xlsx_fixture(&["稀疏"], &[sparse], None, &[]);

    let first = read_xlsx_table(&package, 0, 0, 50, 8, preview_limits()).unwrap();
    assert!(first.cells.is_empty(), "首屏应当是空的：{:?}", first.cells);
    assert_eq!(first.next_data_row, Some(999), "应直接指向第一个含数据的行");
    assert!(first.has_more_rows);

    // 跳过去就能看到数据（界面「下一页」的行为）。
    let jumped = read_xlsx_table(&package, 0, 999, 50, 8, preview_limits()).unwrap();
    assert_eq!(jumped.row_numbers, vec![999, 1000]);
    assert_eq!(jumped.cells[0][0], "稀疏数据");
    assert_eq!(jumped.cells[1][0], "第二行数据");
    assert_eq!(jumped.next_data_row, None, "没有更多数据时不再提示");

    // 稠密表：提示就是下一页的起点，旧行为不变。
    let dense = worksheet_xml(&inline_rows("稠密", 120, 2));
    let package = xlsx_fixture(&["稠密"], &[dense], None, &[]);
    let page = read_xlsx_table(&package, 0, 0, 50, 8, preview_limits()).unwrap();
    assert_eq!(page.row_numbers.first(), Some(&0));
    assert_eq!(page.next_data_row, Some(50));
}

#[test]
fn xlsx_clamps_requested_rows_and_columns_to_the_preview_limits() {
    let sheet = worksheet_xml(&inline_rows("宽表", 600, 80));
    let package = xlsx_fixture(&["宽表"], &[sheet], None, &[]);

    let range = read_xlsx_table(&package, 0, 0, 100_000, 10_000, preview_limits()).unwrap();
    assert_eq!(range.cells.len(), MAX_TABLE_PREVIEW_ROWS);
    assert_eq!(range.column_count, MAX_TABLE_PREVIEW_COLUMNS);
    assert!(
        range
            .cells
            .iter()
            .all(|row| row.len() <= MAX_TABLE_PREVIEW_COLUMNS),
        "每行不得超过列上限"
    );
    assert!(range.has_more_rows);
}

#[test]
fn xlsx_extracts_visible_text_from_all_worksheets() {
    let shared = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
        <sst xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
        <si><t>共享值</t></si><si><t>笔记本</t></si></sst>";
    let first = worksheet_xml("<row r=\"1\"><c r=\"A1\" t=\"s\"><v>0</v></c></row>");
    let second = worksheet_xml(
        "<row r=\"1\"><c r=\"A1\" t=\"s\"><v>1</v></c>\
         <c r=\"B1\" t=\"inlineStr\"><is><t>明细文本</t></is></c></row>",
    );
    let package = xlsx_fixture(&["汇总", "明细"], &[first, second], Some(shared), &[]);

    let text = extract_table_text(&package, "XLSX").unwrap();
    assert!(text.contains("汇总") && text.contains("明细"), "text: {text}");
    assert!(text.contains("共享值"), "text: {text}");
    assert!(text.contains("笔记本"), "text: {text}");
    assert!(text.contains("明细文本"), "text: {text}");
    assert!(text.chars().count() <= MAX_EXTRACTED_TEXT_CHARS);
}

#[test]
fn xlsx_skips_external_links_and_embedded_objects_without_reading_them() {
    let sheet = worksheet_xml("<row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>本地值</t></is></c></row>");
    let external_link = b"<externalLink xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><externalBook r:id=\"rId1\"/></externalLink>".as_slice();
    let external_rels = "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/externalLink\" Target=\"https://example.com/远程.xlsx\" TargetMode=\"External\"/></Relationships>";
    let extras: &[(&str, &[u8])] = &[
        ("xl/externalLinks/externalLink1.xml", external_link),
        (
            "xl/externalLinks/_rels/externalLink1.xml.rels",
            external_rels.as_bytes(),
        ),
        ("xl/embeddings/oleObject1.bin", b"\x00\x01binary".as_slice()),
    ];
    let package = xlsx_fixture(&["链接"], &[sheet], None, extras);

    let range = read_xlsx_table(&package, 0, 0, 50, 10, preview_limits()).unwrap();
    assert_eq!(range.cells[0], vec!["本地值"]);
    assert!(
        range
            .degraded_features
            .iter()
            .any(|feature| feature.contains("外部链接")),
        "unexpected degradations: {:?}",
        range.degraded_features
    );
    assert!(
        range
            .degraded_features
            .iter()
            .any(|feature| feature.contains("嵌入对象")),
        "unexpected degradations: {:?}",
        range.degraded_features
    );

    let text = extract_table_text(&package, "XLSX").unwrap();
    assert!(text.contains("本地值"));
    assert!(
        !text.contains("example.com"),
        "不得读取外部链接目标：{text}"
    );
}

#[test]
fn xlsx_rejects_truncated_and_structurally_broken_packages() {
    let sheet = worksheet_xml("<row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>值</t></is></c></row>");
    let package = xlsx_fixture(&["表"], &[sheet], None, &[]);

    let truncated = &package[..package.len() - 60];
    let error = read_xlsx_table(truncated, 0, 0, 10, 5, preview_limits()).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("中央目录") || message.contains("意外结束"),
        "unexpected: {message}"
    );

    // 工作簿根节点不是 workbook。
    let entries = vec![
        (
            "[Content_Types].xml".to_string(),
            content_types_xml(1, false).into_bytes(),
        ),
        (
            "xl/workbook.xml".to_string(),
            b"<?xml version=\"1.0\"?><notWorkbook/>".to_vec(),
        ),
        (
            "xl/_rels/workbook.xml.rels".to_string(),
            workbook_rels_xml(1, false).into_bytes(),
        ),
        (
            "xl/worksheets/sheet1.xml".to_string(),
            worksheet_xml("").into_bytes(),
        ),
    ];
    let error = read_xlsx_table(&stored_zip(&entries), 0, 0, 10, 5, preview_limits()).unwrap_err();
    assert!(
        error.to_string().contains("工作簿定义"),
        "unexpected: {error}"
    );

    // 缺少工作簿关系文件。
    let entries = vec![
        (
            "[Content_Types].xml".to_string(),
            content_types_xml(1, false).into_bytes(),
        ),
        (
            "xl/workbook.xml".to_string(),
            workbook_xml(&["表"]).into_bytes(),
        ),
        (
            "xl/worksheets/sheet1.xml".to_string(),
            worksheet_xml("").into_bytes(),
        ),
    ];
    let error = read_xlsx_table(&stored_zip(&entries), 0, 0, 10, 5, preview_limits()).unwrap_err();
    assert!(
        error.to_string().contains("workbook.xml.rels"),
        "unexpected: {error}"
    );

    // 内容类型没有声明工作簿主部件（连 xml 默认类型也去掉）。
    let content_types = content_types_xml(1, false)
        .replace(
            &format!(
                "<Override PartName=\"/xl/workbook.xml\" ContentType=\"{WORKBOOK_MAIN_CONTENT_TYPE}\"/>"
            ),
            "",
        )
        .replace(
            "<Default Extension=\"xml\" ContentType=\"application/xml\"/>",
            "",
        );
    let entries = vec![
        ("[Content_Types].xml".to_string(), content_types.into_bytes()),
        (
            "xl/workbook.xml".to_string(),
            workbook_xml(&["表"]).into_bytes(),
        ),
        (
            "xl/_rels/workbook.xml.rels".to_string(),
            workbook_rels_xml(1, false).into_bytes(),
        ),
        (
            "xl/worksheets/sheet1.xml".to_string(),
            worksheet_xml("").into_bytes(),
        ),
    ];
    let error = read_xlsx_table(&stored_zip(&entries), 0, 0, 10, 5, preview_limits()).unwrap_err();
    assert!(
        error.to_string().contains("未声明"),
        "unexpected: {error}"
    );

    // 工作表部件缺失。
    let entries = vec![
        (
            "[Content_Types].xml".to_string(),
            content_types_xml(1, false).into_bytes(),
        ),
        (
            "xl/workbook.xml".to_string(),
            workbook_xml(&["表"]).into_bytes(),
        ),
        (
            "xl/_rels/workbook.xml.rels".to_string(),
            workbook_rels_xml(1, false).into_bytes(),
        ),
    ];
    let error = read_xlsx_table(&stored_zip(&entries), 0, 0, 10, 5, preview_limits()).unwrap_err();
    assert!(
        error.to_string().contains("不存在"),
        "unexpected: {error}"
    );
}

#[test]
fn xlsx_rejects_encrypted_legacy_macro_binary_and_ods_containers() {
    let sheet = worksheet_xml("<row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>值</t></is></c></row>");
    let limits = preview_limits();

    let cfb = [0xd0u8, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1, 0x00, 0x00];
    let error = read_xlsx_table(&cfb, 0, 0, 10, 5, limits).unwrap_err();
    assert!(error.to_string().contains("OLE"), "unexpected: {error}");

    let plain_text = b"id,name\n1,legacy\n";
    let error = read_xlsx_table(plain_text, 0, 0, 10, 5, limits).unwrap_err();
    assert!(
        error.to_string().contains("旧版") || error.to_string().contains("压缩包"),
        "unexpected: {error}"
    );

    let macro_package = xlsx_fixture(
        &["表"],
        &[sheet.clone()],
        None,
        &[("xl/vbaProject.bin", b"\x00macro".as_slice())],
    );
    let error = read_xlsx_table(&macro_package, 0, 0, 10, 5, limits).unwrap_err();
    assert!(error.to_string().contains("XLSM"), "unexpected: {error}");

    let binary_package = xlsx_fixture(
        &["表"],
        &[sheet.clone()],
        None,
        &[("xl/workbook.bin", b"\x00binary".as_slice())],
    );
    let error = read_xlsx_table(&binary_package, 0, 0, 10, 5, limits).unwrap_err();
    assert!(error.to_string().contains("XLSB"), "unexpected: {error}");

    let ods_package = xlsx_fixture(
        &["表"],
        &[sheet.clone()],
        None,
        &[(
            "mimetype",
            b"application/vnd.oasis.opendocument.spreadsheet".as_slice(),
        )],
    );
    let error = read_xlsx_table(&ods_package, 0, 0, 10, 5, limits).unwrap_err();
    assert!(error.to_string().contains("ODS"), "unexpected: {error}");

    // ZIP 条目声明了加密标志位。
    let content_types = content_types_xml(1, false);
    let workbook = workbook_xml(&["表"]);
    let encrypted = zip_with_flags(&[
        ("[Content_Types].xml", 0x0001, content_types.as_bytes()),
        ("xl/workbook.xml", 0x0001, workbook.as_bytes()),
    ]);
    let error = read_xlsx_table(&encrypted, 0, 0, 10, 5, limits).unwrap_err();
    assert!(error.to_string().contains("已加密"), "unexpected: {error}");
}

#[test]
fn xlsx_rejects_packages_over_the_resource_limits() {
    let sheet = worksheet_xml("<row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>值</t></is></c></row>");
    let package = xlsx_fixture(&["表"], &[sheet], None, &[]);

    let input_limited = ArchiveLimits {
        max_input_bytes: 64,
        ..preview_limits()
    };
    let error = read_xlsx_table(&package, 0, 0, 10, 5, input_limited).unwrap_err();
    assert!(error.to_string().contains("文档过大"), "unexpected: {error}");

    let entry_limited = ArchiveLimits {
        max_entry_bytes: 32,
        ..preview_limits()
    };
    let error = read_xlsx_table(&package, 0, 0, 10, 5, entry_limited).unwrap_err();
    assert!(
        error.to_string().contains("单条目上限"),
        "unexpected: {error}"
    );

    let expansion_limited = ArchiveLimits {
        max_expanded_bytes: 40,
        ..preview_limits()
    };
    let error = read_xlsx_table(&package, 0, 0, 10, 5, expansion_limited).unwrap_err();
    assert!(
        error.to_string().contains("累计解压量"),
        "unexpected: {error}"
    );

    // 声明解压量远超实际压缩数据的「压缩炸弹」在解压之前就被拒绝。
    let workbook = workbook_xml(&["表"]);
    let compressed = deflate(workbook.as_bytes());
    assert!(
        compressed.len() < 4096,
        "样本应保持高压缩比：{} 字节",
        compressed.len()
    );
    let bomb = zip_with_entries(&[
        RawEntry {
            name: "[Content_Types].xml",
            flags: 0,
            compression: 0,
            crc: 0,
            uncompressed_size: content_types_xml(1, false).len() as u32,
            data: content_types_xml(1, false).into_bytes(),
        },
        RawEntry {
            name: "xl/workbook.xml",
            flags: 0,
            compression: 8,
            crc: 0,
            uncompressed_size: 8 * 1024 * 1024,
            data: compressed,
        },
        RawEntry {
            name: "xl/_rels/workbook.xml.rels",
            flags: 0,
            compression: 0,
            crc: 0,
            uncompressed_size: workbook_rels_xml(1, false).len() as u32,
            data: workbook_rels_xml(1, false).into_bytes(),
        },
    ]);
    let error = read_xlsx_table(&bomb, 0, 0, 10, 5, preview_limits()).unwrap_err();
    assert!(error.to_string().contains("压缩比"), "unexpected: {error}");
}

#[test]
fn table_validation_accepts_real_csv_and_xlsx_and_rejects_other_documents() {
    assert!(validate_table_document("a,b\n1,2\n".as_bytes(), "CSV").is_ok());

    let sheet = worksheet_xml("<row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>值</t></is></c></row>");
    let package = xlsx_fixture(&["表"], &[sheet], None, &[]);
    assert!(validate_table_document(&package, "XLSX").is_ok());
    assert!(validate_table_document(&package, "xlsx").is_ok());

    let error = validate_table_document(b"a,b\n", "PDF").unwrap_err();
    assert!(error.contains("不是表格文档"), "unexpected: {error}");

    let error = validate_table_document(b"a,b\n", "unknown-format").unwrap_err();
    assert!(error.contains("不支持的文档格式"), "unexpected: {error}");

    // 工作表的根节点不是 worksheet。
    let entries = vec![
        (
            "[Content_Types].xml".to_string(),
            content_types_xml(1, false).into_bytes(),
        ),
        (
            "xl/workbook.xml".to_string(),
            workbook_xml(&["表"]).into_bytes(),
        ),
        (
            "xl/_rels/workbook.xml.rels".to_string(),
            workbook_rels_xml(1, false).into_bytes(),
        ),
        (
            "xl/worksheets/sheet1.xml".to_string(),
            b"<?xml version=\"1.0\"?><chartSpace/>".to_vec(),
        ),
    ];
    let error = validate_table_document(&stored_zip(&entries), "XLSX").unwrap_err();
    assert!(
        error.contains("worksheet") && error.contains("结构校验失败"),
        "unexpected: {error}"
    );
}

// ---------------------------------------------------------------------------
// ZIP 构造工具（与 library_service.rs 的 stored_zip 做法一致）
// ---------------------------------------------------------------------------

struct RawEntry<'a> {
    name: &'a str,
    flags: u16,
    compression: u16,
    crc: u32,
    uncompressed_size: u32,
    data: Vec<u8>,
}

fn stored_zip(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let raw: Vec<RawEntry<'_>> = entries
        .iter()
        .map(|(name, contents)| RawEntry {
            name: name.as_str(),
            flags: 0x0800,
            compression: 0,
            crc: 0,
            uncompressed_size: contents.len() as u32,
            data: contents.clone(),
        })
        .collect();
    zip_with_entries(&raw)
}

fn zip_with_flags(entries: &[(&str, u16, &[u8])]) -> Vec<u8> {
    let raw: Vec<RawEntry<'_>> = entries
        .iter()
        .map(|(name, flags, contents)| RawEntry {
            name,
            flags: *flags,
            compression: 0,
            crc: 0,
            uncompressed_size: contents.len() as u32,
            data: contents.to_vec(),
        })
        .collect();
    zip_with_entries(&raw)
}

fn zip_with_entries(entries: &[RawEntry<'_>]) -> Vec<u8> {
    let mut archive = Vec::new();
    let mut central_entries = Vec::new();

    for entry in entries {
        let local_offset = archive.len() as u32;
        push_u32(&mut archive, 0x0403_4b50);
        push_u16(&mut archive, 20);
        push_u16(&mut archive, entry.flags);
        push_u16(&mut archive, entry.compression);
        push_u16(&mut archive, 0);
        push_u16(&mut archive, 0);
        push_u32(&mut archive, entry.crc);
        push_u32(&mut archive, entry.data.len() as u32);
        push_u32(&mut archive, entry.uncompressed_size);
        push_u16(&mut archive, entry.name.len() as u16);
        push_u16(&mut archive, 0);
        archive.extend_from_slice(entry.name.as_bytes());
        archive.extend_from_slice(&entry.data);

        let mut central = Vec::new();
        push_u32(&mut central, 0x0201_4b50);
        push_u16(&mut central, 20);
        push_u16(&mut central, 20);
        push_u16(&mut central, entry.flags);
        push_u16(&mut central, entry.compression);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u32(&mut central, entry.crc);
        push_u32(&mut central, entry.data.len() as u32);
        push_u32(&mut central, entry.uncompressed_size);
        push_u16(&mut central, entry.name.len() as u16);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u32(&mut central, 0);
        push_u32(&mut central, local_offset);
        central.extend_from_slice(entry.name.as_bytes());
        central_entries.push(central);
    }

    let central_offset = archive.len() as u32;
    for entry in &central_entries {
        archive.extend_from_slice(entry);
    }
    let central_size = archive.len() as u32 - central_offset;

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

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data).expect("deflate sample");
    encoder.finish().expect("deflate sample")
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
