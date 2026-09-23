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
    DocumentPreview, DocumentSearchFilters, DocumentSearchQuery, DocumentSearchResult,
    ImportDecision, ImportItemStatus, LibraryService, LocationStatus, SearchMatchKind,
    TablePreviewRequest, MAX_TABLE_PREVIEW_COLUMNS, MAX_TABLE_PREVIEW_ROWS,
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

/// 取搜索结果本身，用于断言片段内容与命中类型。
fn search_results(service: &LibraryService, query: &str) -> Vec<DocumentSearchResult> {
    service
        .search_documents(DocumentSearchQuery {
            query: query.to_string(),
            filters: DocumentSearchFilters::default(),
        })
        .unwrap()
        .results
}

/// 读取一份文档的字符串片段；命中片段缺失时给出可诊断信息。
fn snippet_for(results: &[DocumentSearchResult], document_id: &str) -> String {
    let result = results
        .iter()
        .find(|result| result.document.id == document_id)
        .unwrap_or_else(|| panic!("搜索结果里没有 {document_id}：{results:?}"));
    assert_eq!(result.match_kind, SearchMatchKind::Content);
    result
        .snippet
        .clone()
        .unwrap_or_else(|| panic!("正文命中必须带片段：{result:?}"))
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

// ---------------------------------------------------------------------------
// 补充验收：真实资料库上的编码一致性、命中片段、重复跳过、分页与拒绝路径
// ---------------------------------------------------------------------------

/// 通用最小 OOXML 工作簿：工作表名称、sheetData、共享字符串与额外部件都可指定。
///
/// 与 `sample_workbook` 使用同一套最小包结构，便于构造宏、ODS、加密与损坏样本。
fn workbook_package(
    sheet_names: &[&str],
    sheet_data: &[&str],
    shared_strings: Option<&str>,
    extra_entries: &[(&str, Vec<u8>)],
) -> Vec<u8> {
    assert_eq!(
        sheet_names.len(),
        sheet_data.len(),
        "每张工作表都要有对应的 sheetData"
    );

    let mut overrides = String::new();
    let mut sheet_tags = String::new();
    let mut relationships = String::new();
    for (index, name) in sheet_names.iter().enumerate() {
        let number = index + 1;
        overrides.push_str(&format!(
            r#"<Override PartName="/xl/worksheets/sheet{number}.xml" ContentType="{WORKSHEET_CONTENT_TYPE}"/>"#
        ));
        sheet_tags.push_str(&format!(
            r#"<sheet name="{name}" sheetId="{number}" r:id="rId{number}"/>"#
        ));
        relationships.push_str(&format!(
            r#"<Relationship Id="rId{number}" Type="{WORKSHEET_RELATIONSHIP}" Target="worksheets/sheet{number}.xml"/>"#
        ));
    }
    if shared_strings.is_some() {
        let number = sheet_names.len() + 1;
        overrides.push_str(&format!(
            r#"<Override PartName="/xl/sharedStrings.xml" ContentType="{SHARED_STRINGS_CONTENT_TYPE}"/>"#
        ));
        relationships.push_str(&format!(
            r#"<Relationship Id="rId{number}" Type="{SHARED_STRINGS_RELATIONSHIP}" Target="sharedStrings.xml"/>"#
        ));
    }

    let content_types = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/xl/workbook.xml" ContentType="{WORKBOOK_CONTENT_TYPE}"/>
{overrides}
</Types>"#
    );
    let root_relationships = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#;
    let workbook = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
 xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<sheets>{sheet_tags}</sheets>
</workbook>"#
    );
    let workbook_relationships = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
{relationships}
</Relationships>"#
    );

    let mut entries: Vec<(String, Vec<u8>)> = vec![
        ("[Content_Types].xml".to_string(), content_types.into_bytes()),
        (
            "_rels/.rels".to_string(),
            root_relationships.as_bytes().to_vec(),
        ),
        ("xl/workbook.xml".to_string(), workbook.into_bytes()),
        (
            "xl/_rels/workbook.xml.rels".to_string(),
            workbook_relationships.into_bytes(),
        ),
    ];
    if let Some(shared) = shared_strings {
        entries.push((
            "xl/sharedStrings.xml".to_string(),
            shared.as_bytes().to_vec(),
        ));
    }
    for (index, data) in sheet_data.iter().enumerate() {
        entries.push((
            format!("xl/worksheets/sheet{}.xml", index + 1),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetData>{data}</sheetData>
</worksheet>"#
            )
            .into_bytes(),
        ));
    }
    for (name, bytes) in extra_entries {
        entries.push((name.to_string(), bytes.clone()));
    }

    let borrowed: Vec<(&str, Vec<u8>)> = entries
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.clone()))
        .collect();
    zip_package(&borrowed)
}

#[test]
fn imports_bom_csv_and_keeps_display_and_search_consistent() {
    let root = tempdir().unwrap();
    let mut service = open_service(root.path());
    let source = root.path().join("带BOM账目.csv");
    let mut bytes = vec![0xef, 0xbb, 0xbf];
    bytes.extend_from_slice("项目,Name\n差旅报销,Notebook 16\n\"含,逗号\",300\n".as_bytes());
    fs::write(&source, &bytes).unwrap();
    let original = fs::read(&source).unwrap();

    let document_id = import_path(&mut service, &source);

    let sheets = service.list_document_sheets(&document_id).unwrap();
    assert_eq!(sheets.len(), 1);
    assert_eq!(sheets[0].name, "CSV");
    assert_eq!(sheets[0].row_count, Some(3));

    // 显示：BOM 被忽略并给出提示，中英文单元格原样呈现。
    match service.get_document_preview(&document_id, None).unwrap() {
        DocumentPreview::Table {
            cells, notice, ..
        } => {
            assert_eq!(cells[0], vec!["项目", "Name"]);
            assert!(
                !cells[0][0].contains('\u{feff}'),
                "BOM 不应出现在第一个单元格：{:?}",
                cells[0][0]
            );
            assert_eq!(cells[1], vec!["差旅报销", "Notebook 16"]);
            assert_eq!(cells[2], vec!["含,逗号", "300"]);
            assert_eq!(
                notice.as_deref(),
                Some("已忽略文件开头的 UTF-8 BOM。"),
                "显示层要说明 BOM 已被忽略"
            );
        }
        other => panic!("期望表格预览，实际是 {other:?}"),
    }

    // 搜索：与显示一致，中英文单元格都能命中，片段就是单元格文字且不带 BOM。
    service.index_pending_documents().unwrap();
    let chinese = search_results(&service, "差旅报销");
    assert_eq!(chinese.len(), 1);
    assert_eq!(chinese[0].document.id, document_id);
    let snippet = snippet_for(&chinese, &document_id);
    assert!(snippet.contains("差旅报销"), "片段应包含单元格文字：{snippet}");
    assert!(
        !snippet.contains('\u{feff}'),
        "BOM 不得进入搜索片段：{snippet:?}"
    );
    assert_eq!(search(&service, "Notebook 16"), 1, "英文单元格同样可搜索");
    assert_eq!(search(&service, "含,逗号"), 1, "带引号的逗号单元格可搜索");
    assert_eq!(fs::read(&source).unwrap(), original, "源文件字节不变");
}

#[test]
fn duplicate_csv_import_waits_for_a_decision_and_keeps_one_library_copy() {
    let root = tempdir().unwrap();
    let mut service = open_service(root.path());
    let source = root.path().join("账目.csv");
    fs::write(&source, "项目,金额\n差旅报销,1200\n").unwrap();
    let original = fs::read(&source).unwrap();

    let first = service
        .start_import(vec![source.to_string_lossy().into_owned()])
        .unwrap();
    assert_eq!(first.imported_count, 1);
    let document_id = first.items[0].document_id.clone().unwrap();
    let copy_path = root
        .path()
        .join("Library")
        .join("documents")
        .join(&document_id)
        .join("账目.csv");
    let copy_bytes = fs::read(&copy_path).unwrap();
    assert_eq!(copy_bytes, original, "资料库副本是权威版本，内容与源文件一致");

    // 同一份 CSV 再次导入走既有重复判断，不新建文档。
    let duplicate = service
        .start_import(vec![source.to_string_lossy().into_owned()])
        .unwrap();
    assert_eq!(duplicate.duplicate_count, 1);
    assert_eq!(duplicate.items[0].status, ImportItemStatus::Duplicate);
    assert_eq!(
        duplicate.items[0].duplicate_document_id.as_deref(),
        Some(document_id.as_str())
    );

    let skipped = service
        .resolve_import_item(&duplicate.items[0].item_id, ImportDecision::UseExisting)
        .unwrap();
    assert_eq!(skipped.status, ImportItemStatus::Skipped);
    assert_eq!(skipped.document_id.as_deref(), Some(document_id.as_str()));
    assert_eq!(service.list_documents().unwrap().len(), 1);
    assert_eq!(fs::read(&copy_path).unwrap(), copy_bytes, "重复判定不重写副本");
    assert_eq!(fs::read(&source).unwrap(), original, "源文件字节不变");
}

#[test]
fn xlsx_search_snippets_name_the_worksheet_and_uncached_formulas_stay_explained() {
    let root = tempdir().unwrap();
    let mut service = open_service(root.path());
    let source = root.path().join("季度经营报表.xlsx");
    // 工作表名超过两个字，才能作为正文命中并出现在搜索片段里。
    let workbook = workbook_package(
        &["季度经营汇总表", "差旅明细表"],
        &[
            r#"<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row>
<row r="2"><c r="A2" t="s"><v>2</v></c><c r="B2"><f>SUMPRODUCT(B1:B1)</f></c></row>"#,
            r#"<row r="1"><c r="A1" t="s"><v>3</v></c></row>"#,
        ],
        Some(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="4" uniqueCount="4">
<si><t>项目</t></si><si><t>金额</t></si><si><t>差旅报销</t></si><si><t>住宿发票</t></si>
</sst>"#,
        ),
        &[],
    );
    fs::write(&source, &workbook).unwrap();
    let original = fs::read(&source).unwrap();

    let document_id = import_path(&mut service, &source);
    let sheets = service.list_document_sheets(&document_id).unwrap();
    assert_eq!(sheets.len(), 2);
    assert_eq!(sheets[0].name, "季度经营汇总表");
    assert_eq!(sheets[1].name, "差旅明细表");

    // 缺缓存值的公式给出可读表示并记入降级，而不是空白或求值结果。
    match service.get_document_preview(&document_id, None).unwrap() {
        DocumentPreview::Table {
            cells,
            degraded_features,
            ..
        } => {
            assert_eq!(cells[0], vec!["项目", "金额"]);
            assert_eq!(cells[1][0], "差旅报销");
            assert_eq!(
                cells[1][1],
                personal_document_manager_lib::library::table::UNCACHED_FORMULA_TEXT,
                "缺缓存值的公式要给出可理解的表示"
            );
            assert!(
                degraded_features.iter().any(|feature| feature.contains("缓存值")),
                "应说明降级原因：{degraded_features:?}"
            );
        }
        other => panic!("期望表格预览，实际是 {other:?}"),
    }

    service.index_pending_documents().unwrap();

    // 工作表名称进入索引：搜索结果能说明命中来自哪张表。
    let by_sheet = search_results(&service, "季度经营汇总表");
    assert_eq!(by_sheet.len(), 1);
    let sheet_snippet = snippet_for(&by_sheet, &document_id);
    assert!(
        sheet_snippet.contains("季度经营汇总表"),
        "片段应带工作表名称：{sheet_snippet}"
    );

    // 单元格文字命中，片段就是单元格文本；未读到的第二张表也进入索引。
    let by_cell = search_results(&service, "差旅报销");
    assert_eq!(by_cell.len(), 1);
    assert!(snippet_for(&by_cell, &document_id).contains("差旅报销"));
    assert_eq!(search(&service, "住宿发票"), 1, "第二张工作表的单元格可搜索");

    // 公式文本既不求值也不进入索引或片段。
    assert_eq!(
        search_results(&service, "SUMPRODUCT").len(),
        0,
        "公式文本不得进入索引"
    );
    assert_eq!(fs::read(&source).unwrap(), original, "源文件字节不变");
}

#[test]
fn long_csv_and_xlsx_tables_page_within_the_row_limit() {
    let root = tempdir().unwrap();
    let mut service = open_service(root.path());

    let csv_source = root.path().join("长账目.csv");
    let mut csv = String::from("序号,说明\n");
    for index in 0..1_200 {
        csv.push_str(&format!("{index},长表数据{index}\n"));
    }
    fs::write(&csv_source, csv.as_bytes()).unwrap();

    let xlsx_rows: String = (1..=700)
        .map(|row| {
            // 末行加一个只在工作簿里出现的标记，便于断言 XLSX 的索引而不是 CSV。
            let marker = if row == 700 {
                r#"<c r="B700" t="inlineStr"><is><t>长表末行标记</t></is></c>"#
            } else {
                ""
            };
            format!(
                r#"<row r="{row}"><c r="A{row}" t="inlineStr"><is><t>长表数据{row}</t></is></c>{marker}</row>"#
            )
        })
        .collect();
    let xlsx_source = root.path().join("长表.xlsx");
    fs::write(
        &xlsx_source,
        workbook_package(&["长表"], &[&xlsx_rows], None, &[]),
    )
    .unwrap();

    let csv_id = import_path(&mut service, &csv_source);
    let xlsx_id = import_path(&mut service, &xlsx_source);

    // CSV：首屏只给一行上限内的范围，并说明还有更多行。
    let sheets = service.list_document_sheets(&csv_id).unwrap();
    assert_eq!(sheets[0].row_count, Some(1_201));
    let first = service.get_document_preview(&csv_id, None).unwrap();
    let (first_cells, first_rows) = table_preview(&first);
    assert_eq!(first_cells.len(), MAX_TABLE_PREVIEW_ROWS);
    assert_eq!(first_rows[0], 0);
    match &first {
        DocumentPreview::Table { has_more_rows, .. } => assert!(has_more_rows),
        other => panic!("期望表格预览，实际是 {other:?}"),
    }

    // 一次请求 10 万行 / 1 万列也会被收敛到上限。
    let clamped = service
        .get_table_preview(
            &csv_id,
            TablePreviewRequest {
                sheet_index: 0,
                start_row: 0,
                row_count: 100_000,
                column_count: 10_000,
            },
        )
        .unwrap();
    let (clamped_cells, _) = table_preview(&clamped);
    assert_eq!(clamped_cells.len(), MAX_TABLE_PREVIEW_ROWS);
    match &clamped {
        DocumentPreview::Table { column_count, .. } => {
            assert_eq!(*column_count, MAX_TABLE_PREVIEW_COLUMNS)
        }
        other => panic!("期望表格预览，实际是 {other:?}"),
    }

    // 末尾一页只返回剩余行，不补齐。
    let last = service
        .get_table_preview(
            &csv_id,
            TablePreviewRequest {
                sheet_index: 0,
                start_row: 1_200,
                row_count: 50,
                column_count: 2,
            },
        )
        .unwrap();
    let (last_cells, last_rows) = table_preview(&last);
    assert_eq!(last_cells.len(), 1);
    assert_eq!(last_cells[0], vec!["1199", "长表数据1199"]);
    assert_eq!(last_rows, &vec![1_200]);
    match &last {
        DocumentPreview::Table { has_more_rows, .. } => assert!(!has_more_rows),
        other => panic!("期望表格预览，实际是 {other:?}"),
    }

    // XLSX：700 行的长表同样分页，不一次展开整表。
    let xlsx_first = service.get_document_preview(&xlsx_id, None).unwrap();
    let (xlsx_first_cells, _) = table_preview(&xlsx_first);
    assert_eq!(xlsx_first_cells.len(), MAX_TABLE_PREVIEW_ROWS);
    match &xlsx_first {
        DocumentPreview::Table { has_more_rows, .. } => assert!(has_more_rows),
        other => panic!("期望表格预览，实际是 {other:?}"),
    }
    let xlsx_second = service
        .get_table_preview(
            &xlsx_id,
            TablePreviewRequest {
                sheet_index: 0,
                start_row: 500,
                row_count: 500,
                column_count: 2,
            },
        )
        .unwrap();
    let (xlsx_second_cells, xlsx_second_rows) = table_preview(&xlsx_second);
    assert_eq!(xlsx_second_cells.len(), 200);
    assert_eq!(xlsx_second_cells[0][0], "长表数据501");
    assert_eq!(xlsx_second_rows[0], 500);
    match &xlsx_second {
        DocumentPreview::Table { has_more_rows, .. } => assert!(!has_more_rows),
        other => panic!("期望表格预览，实际是 {other:?}"),
    }

    // 长表正文仍按上限进入索引（两份长表分别命中自己的内容）。
    service.index_pending_documents().unwrap();
    assert_eq!(search(&service, "长表数据1199"), 1);
    assert_eq!(search(&service, "长表末行标记"), 1);
}

#[test]
fn rejects_legacy_encrypted_and_macro_workbooks_without_blocking_the_batch() {
    let root = tempdir().unwrap();
    let mut service = open_service(root.path());

    let sheet_data = r#"<row r="1"><c r="A1" t="inlineStr"><is><t>正常表格</t></is></c></row>"#;

    // 旧 .xls（OLE 复合文档）：扩展名不受支持，识别阶段就拒绝。
    let legacy = root.path().join("旧表.xls");
    fs::write(
        &legacy,
        [0xd0_u8, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1, 0x00, 0x00],
    )
    .unwrap();

    // 加密工作簿（OLE 容器）改名成 .xlsx：按真实结构拒绝，不只看 ZIP 头。
    let encrypted = root.path().join("加密.xlsx");
    fs::write(
        &encrypted,
        [0xd0_u8, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1, 0x00, 0x00],
    )
    .unwrap();

    // 带宏的工作簿内容（xl/vbaProject.bin）改名成 .xlsx。
    let macro_workbook = root.path().join("宏表.xlsx");
    fs::write(
        &macro_workbook,
        workbook_package(
            &["表"],
            &[sheet_data],
            None,
            &[("xl/vbaProject.bin", b"\x00macro".to_vec())],
        ),
    )
    .unwrap();

    // ODS 标记的工作簿改名成 .xlsx。
    let ods_workbook = root.path().join("开源表.xlsx");
    fs::write(
        &ods_workbook,
        workbook_package(
            &["表"],
            &[sheet_data],
            None,
            &[(
                "mimetype",
                b"application/vnd.oasis.opendocument.spreadsheet".to_vec(),
            )],
        ),
    )
    .unwrap();

    // 截断的工作簿：中央目录不完整。
    let truncated = root.path().join("损坏.xlsx");
    let package = workbook_package(&["表"], &[sheet_data], None, &[]);
    fs::write(&truncated, &package[..package.len() - 40]).unwrap();

    // 同批里正常的两份表格文档。
    let good_csv = root.path().join("正常.csv");
    fs::write(&good_csv, "名称\n可导入\n").unwrap();
    let good_xlsx = root.path().join("正常.xlsx");
    fs::write(
        &good_xlsx,
        workbook_package(&["汇总表"], &[sheet_data], None, &[]),
    )
    .unwrap();

    let rejected = [
        &legacy,
        &encrypted,
        &macro_workbook,
        &ods_workbook,
        &truncated,
    ];
    let mut paths: Vec<String> = rejected
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    paths.push(good_csv.to_string_lossy().into_owned());
    paths.push(good_xlsx.to_string_lossy().into_owned());

    let batch = service.start_import(paths).unwrap();
    assert_eq!(batch.items.len(), 7);
    assert_eq!(batch.imported_count, 2, "正常文档仍然完成导入");
    assert_eq!(
        batch.failed_count, 5,
        "旧 .xls 与四种改名后的异常工作簿都单项失败"
    );
    assert_eq!(batch.ignored_count, 0);

    // 旧 .xls 在选择器导入路径上按「不支持该文件格式」单项失败。
    let legacy_failure = batch
        .items
        .iter()
        .find(|item| item.file_name == "旧表.xls")
        .expect("旧 .xls 必须作为单项结果出现");
    assert_eq!(legacy_failure.status, ImportItemStatus::Failed);
    assert_eq!(legacy_failure.error_stage.as_deref(), Some("scan"));
    assert!(
        legacy_failure
            .error_message
            .as_deref()
            .unwrap_or_default()
            .contains("仅支持"),
        "旧 .xls 应说明支持的格式：{:?}",
        legacy_failure.error_message
    );

    // 四种改名成 .xlsx 的异常容器都在真实结构校验阶段失败并给出原因。
    for item in batch
        .items
        .iter()
        .filter(|item| item.status == ImportItemStatus::Failed && item.file_name != "旧表.xls")
    {
        let message = item.error_message.clone().unwrap_or_default();
        assert!(
            !message.trim().is_empty(),
            "失败必须给出具体原因：{item:?}"
        );
        assert_eq!(item.error_stage.as_deref(), Some("validate"));
        assert!(item.retryable);
    }

    // 已导入的文档仍可浏览、可搜索。
    let documents = service.list_documents().unwrap();
    assert_eq!(documents.len(), 2);
    let csv_id = documents
        .iter()
        .find(|document| document.file_name == "正常.csv")
        .unwrap()
        .id
        .clone();
    let xlsx_id = documents
        .iter()
        .find(|document| document.file_name == "正常.xlsx")
        .unwrap()
        .id
        .clone();
    assert_eq!(service.list_document_sheets(&csv_id).unwrap().len(), 1);
    assert_eq!(
        service.list_document_sheets(&xlsx_id).unwrap()[0].name,
        "汇总表"
    );
    match service.get_document_preview(&csv_id, None).unwrap() {
        DocumentPreview::Table { cells, .. } => assert_eq!(cells[0], vec!["名称"]),
        other => panic!("期望表格预览，实际是 {other:?}"),
    }
    service.index_pending_documents().unwrap();
    assert_eq!(search(&service, "可导入"), 1);
    assert_eq!(search(&service, "正常表格"), 1);

    // 被拒绝的源文件一律保持原样。
    for path in rejected {
        assert!(path.is_file(), "源文件保持不变：{}", path.display());
    }
}
