//! 表格文档（CSV/XLSX，工作单 08/09）的端到端集成测试。
//!
//! 覆盖真实临时资料库上的完整路径：导入校验 → 复制进资料库 → 正文索引 →
//! `get_document_preview` / `get_table_preview` 分页预览 → 搜索命中 → 源文件不变。
//! 只断言外部可观察行为，不依赖私有函数或表结构。

use std::fs;
use std::io::Write;
use std::path::Path;

use flate2::write::DeflateEncoder;
use flate2::Compression;
use personal_document_manager_lib::library::{
    DocumentPreview, DocumentSearchFilters, DocumentSearchQuery, ImportItemStatus, LibraryService,
    LocationStatus,
};
use tempfile::tempdir;

const WORKBOOK_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml";
const WORKSHEET_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml";
const SHARED_STRINGS_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml";
const WORKSHEET_RELATIONSHIP: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet";
const SHARED_STRINGS_RELATIONSHIP: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings";

fn open_service(root: &Path) -> LibraryService {
    let state_dir = root.join("app-state");
    let library_dir = root.join("Library");
    let mut service = LibraryService::new(&state_dir).unwrap();
    assert_eq!(
        service.inspect_location(&library_dir).unwrap().status,
        LocationStatus::Usable
    );
    service.create_library(&library_dir).unwrap();
    service
}

fn import_path(service: &mut LibraryService, path: &Path) -> String {
    let batch = service.start_import(vec![path.to_string_lossy().into_owned()]).unwrap();
    assert_eq!(
        batch.failed_count, 0,
        "导入不应失败：{:?}",
        batch
            .items
            .iter()
            .map(|item| (&item.status, &item.error_message))
            .collect::<Vec<_>>()
    );
    let item = batch.items.first().unwrap();
    assert_eq!(item.status, ImportItemStatus::Imported);
    item.document_id.clone().unwrap()
}

fn search(service: &LibraryService, query: &str) -> usize {
    service
        .search_documents(DocumentSearchQuery {
            query: query.to_string(),
            filters: DocumentSearchFilters::default(),
        })
        .unwrap()
        .results
        .len()
}

fn table_preview(preview: &DocumentPreview) -> (&Vec<Vec<String>>, &Vec<usize>) {
    match preview {
        DocumentPreview::Table {
            cells, row_numbers, ..
        } => (cells, row_numbers),
        other => panic!("期望表格预览，实际是 {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 最小 OOXML 工作簿构造
// ---------------------------------------------------------------------------

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

/// 组装一个最小的 ZIP（deflate 压缩），供 OOXML 包使用。
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

/// 两张工作表的 XLSX：汇总表含中文共享字符串，明细表含稀疏行与公式缓存值。
fn sample_workbook() -> Vec<u8> {
    let content_types = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/xl/workbook.xml" ContentType="{WORKBOOK_CONTENT_TYPE}"/>
<Override PartName="/xl/worksheets/sheet1.xml" ContentType="{WORKSHEET_CONTENT_TYPE}"/>
<Override PartName="/xl/worksheets/sheet2.xml" ContentType="{WORKSHEET_CONTENT_TYPE}"/>
<Override PartName="/xl/sharedStrings.xml" ContentType="{SHARED_STRINGS_CONTENT_TYPE}"/>
</Types>"#
    );
    let root_relationships = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#;
    let workbook = r#"<?xml version="1.0" encoding="UTF-8"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
 xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<sheets>
<sheet name="汇总" sheetId="1" r:id="rId1"/>
<sheet name="明细" sheetId="2" r:id="rId2"/>
</sheets>
</workbook>"#;
    let workbook_relationships = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="{WORKSHEET_RELATIONSHIP}" Target="worksheets/sheet1.xml"/>
<Relationship Id="rId2" Type="{WORKSHEET_RELATIONSHIP}" Target="worksheets/sheet2.xml"/>
<Relationship Id="rId3" Type="{SHARED_STRINGS_RELATIONSHIP}" Target="sharedStrings.xml"/>
</Relationships>"#
    );
    let shared_strings = r#"<?xml version="1.0" encoding="UTF-8"?>
<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="5" uniqueCount="5">
<si><t>项目</t></si>
<si><t>金额</t></si>
<si><t>差旅报销</t></si>
<si><r><t>季度</t></r><r><t>汇总</t></r></si>
<si><t>备注</t></si>
</sst>"#;
    let sheet1 = r#"<?xml version="1.0" encoding="UTF-8"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetData>
<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row>
<row r="2"><c r="A2" t="s"><v>2</v></c><c r="B2"><v>1200</v></c></row>
<row r="3"><c r="A3" t="s"><v>3</v></c><c r="B3"><f>SUM(B2:B2)</f><v>1200</v></c></row>
</sheetData>
</worksheet>"#;
    let sheet2 = r#"<?xml version="1.0" encoding="UTF-8"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetData>
<row r="1"><c r="A1" t="s"><v>4</v></c></row>
<row r="7"><c r="B7"><v>42</v></c></row>
</sheetData>
</worksheet>"#;

    zip_package(&[
        ("[Content_Types].xml", content_types.into_bytes()),
        ("_rels/.rels", root_relationships.as_bytes().to_vec()),
        ("xl/workbook.xml", workbook.as_bytes().to_vec()),
        (
            "xl/_rels/workbook.xml.rels",
            workbook_relationships.into_bytes(),
        ),
        (
            "xl/sharedStrings.xml",
            shared_strings.as_bytes().to_vec(),
        ),
        ("xl/worksheets/sheet1.xml", sheet1.as_bytes().to_vec()),
        ("xl/worksheets/sheet2.xml", sheet2.as_bytes().to_vec()),
    ])
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[test]
fn imports_csv_and_serves_a_bounded_table_preview() {
    let root = tempdir().unwrap();
    let mut service = open_service(root.path());
    let source = root.path().join("账目.csv");
    let csv = "项目,金额\n差旅报销,1200\n\"含,逗号\",300\n\"跨\n行\",400\n";
    fs::write(&source, csv).unwrap();
    let original = fs::read(&source).unwrap();

    let document_id = import_path(&mut service, &source);

    let documents = service.list_documents().unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].file_type, "CSV");

    let sheets = service.list_document_sheets(&document_id).unwrap();
    assert_eq!(sheets.len(), 1);
    assert_eq!(sheets[0].name, "CSV");
    assert_eq!(sheets[0].row_count, Some(4));

    // 首屏预览即第一段范围，且不超过默认行数上限。
    let preview = service.get_document_preview(&document_id, None).unwrap();
    let (cells, row_numbers) = table_preview(&preview);
    assert_eq!(cells.len(), 4);
    assert_eq!(row_numbers, &vec![0, 1, 2, 3]);
    assert_eq!(cells[0], vec!["项目", "金额"]);
    assert_eq!(cells[2], vec!["含,逗号", "300"]);
    assert_eq!(cells[3], vec!["跨\n行", "400"]);

    // 分页请求只返回请求的范围，并带上真实行号。
    let page = service
        .get_table_preview(
            &document_id,
            personal_document_manager_lib::library::TablePreviewRequest {
                sheet_index: 0,
                start_row: 2,
                row_count: 1,
                column_count: 1,
            },
        )
        .unwrap();
    let (page_cells, page_rows) = table_preview(&page);
    assert_eq!(page_cells.len(), 1);
    assert_eq!(page_cells[0], vec!["含,逗号"]);
    assert_eq!(page_rows, &vec![2]);
    match &page {
        DocumentPreview::Table { has_more_rows, .. } => assert!(has_more_rows),
        other => panic!("期望表格预览，实际是 {other:?}"),
    }

    service.index_pending_documents().unwrap();
    assert_eq!(search(&service, "差旅报销"), 1, "单元格文字进入索引");
    assert_eq!(search(&service, "跨\n行"), 1, "引号内换行按原样索引");

    assert_eq!(fs::read(&source).unwrap(), original, "源文件字节不变");
}

#[test]
fn imports_xlsx_with_multiple_sheets_sheets_switching_and_search() {
    let root = tempdir().unwrap();
    let mut service = open_service(root.path());
    let source = root.path().join("季度.xlsx");
    let workbook = sample_workbook();
    fs::write(&source, &workbook).unwrap();

    let document_id = import_path(&mut service, &source);
    let sheets = service.list_document_sheets(&document_id).unwrap();
    assert_eq!(sheets.len(), 2);
    assert_eq!(sheets[0].name, "汇总");
    assert_eq!(sheets[1].name, "明细");
    assert_eq!(sheets[0].row_count, Some(3), "活动表返回真实行数");
    assert_eq!(
        sheets[1].row_count, None,
        "未读取的工作表不返回行列数，避免为切表扫描整本工作簿"
    );

    let summary = service.get_document_preview(&document_id, None).unwrap();
    let (cells, row_numbers) = table_preview(&summary);
    assert_eq!(row_numbers, &vec![0, 1, 2]);
    assert_eq!(cells[0], vec!["项目", "金额"]);
    assert_eq!(cells[1], vec!["差旅报销", "1200"]);
    assert_eq!(cells[2][0], "季度汇总", "富文本共享字符串拼接");
    assert_eq!(cells[2][1], "1200", "公式只读缓存值");

    // 切到第二张表：稀疏行（第 1 行与第 7 行）保留真实行号。
    let details = service
        .get_table_preview(
            &document_id,
            personal_document_manager_lib::library::TablePreviewRequest {
                sheet_index: 1,
                start_row: 0,
                row_count: 10,
                column_count: 4,
            },
        )
        .unwrap();
    let (detail_cells, detail_rows) = table_preview(&details);
    assert_eq!(detail_rows, &vec![0, 6], "稀疏行保留 Excel 行索引");
    assert_eq!(detail_cells[0], vec!["备注"]);
    assert_eq!(detail_cells[1], vec!["", "42"]);

    service.index_pending_documents().unwrap();
    assert_eq!(search(&service, "差旅报销"), 1, "中文单元格正文可搜索");
    assert_eq!(
        search(&service, "季度汇总"),
        1,
        "富文本共享字符串应可搜索；当前索引文本：{:?}",
        index_text(&service, &document_id)
    );
    assert_eq!(
        search(&service, "1200"),
        1,
        "公式缓存值应可搜索；当前索引文本：{:?}",
        index_text(&service, &document_id)
    );
    assert_eq!(
        search(&service, "备注"),
        0,
        "两字符查询按设计只匹配标题/描述/元数据，不匹配正文"
    );
}

/// 读取索引里的正文，用于断言失败时给出可诊断信息。
fn index_text(service: &LibraryService, document_id: &str) -> Vec<String> {
    service
        .search_documents(DocumentSearchQuery {
            query: String::new(),
            filters: DocumentSearchFilters::default(),
        })
        .unwrap()
        .results
        .iter()
        .filter(|result| result.document.id == document_id)
        .map(|result| result.snippet.clone().unwrap_or_default())
        .collect()
}

#[test]
fn workbook_text_extraction_covers_every_sheet() {
    let extracted =
        personal_document_manager_lib::library::table::extract_table_text(&sample_workbook(), "XLSX")
            .unwrap();
    for expected in ["汇总", "明细", "差旅报销", "季度汇总", "备注", "1200", "42"] {
        assert!(
            extracted.contains(expected),
            "索引文本缺少 {expected}：{extracted}"
        );
    }
}

#[test]
fn rejects_unsafe_or_unreadable_table_sources_individually() {
    let root = tempdir().unwrap();
    let mut service = open_service(root.path());

    // 非法 UTF-8 的 CSV：必须单项失败，不得静默生成乱码文档。
    let broken_csv = root.path().join("乱码.csv");
    fs::write(&broken_csv, [0xE4, 0xB8, 0xAD, 0xFF, 0xFE, 0x00]).unwrap();

    // 伪装成 XLSX 的普通 ZIP：结构校验必须拒绝。
    let fake_xlsx = root.path().join("假.xlsx");
    fs::write(
        &fake_xlsx,
        zip_package(&[("hello.txt", b"not a workbook".to_vec())]),
    )
    .unwrap();

    // 同一批里还有一份正常 CSV，验证失败隔离。
    let good_csv = root.path().join("正常.csv");
    fs::write(&good_csv, "名称\n可导入\n").unwrap();

    let batch = service
        .start_import(vec![
            broken_csv.to_string_lossy().into_owned(),
            fake_xlsx.to_string_lossy().into_owned(),
            good_csv.to_string_lossy().into_owned(),
        ])
        .unwrap();

    assert_eq!(batch.imported_count, 1, "正常文件仍然导入");
    assert_eq!(batch.failed_count, 2, "两个异常文件单项失败");
    for item in batch.items.iter().filter(|item| item.status == ImportItemStatus::Failed) {
        assert_eq!(item.error_stage.as_deref(), Some("validate"));
        let message = item.error_message.clone().unwrap_or_default();
        assert!(!message.trim().is_empty(), "失败必须给出可理解原因");
    }

    let documents = service.list_documents().unwrap();
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].file_name, "正常.csv");
}

#[test]
fn table_preview_is_rejected_for_non_table_documents() {
    let root = tempdir().unwrap();
    let mut service = open_service(root.path());
    let source = root.path().join("说明.md");
    fs::write(&source, "# 标题\n正文\n").unwrap();
    let document_id = import_path(&mut service, &source);

    assert!(service.list_document_sheets(&document_id).unwrap().is_empty());
    match service.get_document_preview(&document_id, None).unwrap() {
        DocumentPreview::Markdown { .. } => {}
        other => panic!("Markdown 仍应走文本预览，实际是 {other:?}"),
    }
    match service
        .get_table_preview(
            &document_id,
            personal_document_manager_lib::library::TablePreviewRequest::default(),
        )
        .unwrap()
    {
        DocumentPreview::Unsupported { message } => {
            assert!(message.contains("表格"), "应说明不是表格文档：{message}")
        }
        other => panic!("非表格文档应返回不支持，实际是 {other:?}"),
    }
}
