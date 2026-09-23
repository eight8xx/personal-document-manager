//! 表格文档（CSV/XLSX）的解析层：只做「字节进、结构化数据出」。
//!
//! 本模块不接触文件系统、数据库与资料库状态：调用方（`service`）负责读取资料库
//! 副本的字节并决定请求的行列范围，这里只接收字节与资源预算。
//!
//! 资源上限全部走 [`super::limits`]：输入大小、单条目声明大小、压缩比、累计解压量
//! 与预览行列上限都由 [`ArchiveLimits`] / [`ExpansionBudget`] 决定，模块内不另设
//! 上限。CSV 与 XLSX 都不会把整份表格交给界面：只保留请求范围内的单元格，
//! 超出的行列只用于统计，不进入返回值。
//!
//! 安全边界：XLSX 只读取包内部件，不跟随外部链接、不执行宏与嵌入对象、不计算
//! 公式（只读公式的缓存值）；旧版 .xls、加密容器、ODS 与启用宏的工作簿直接拒绝。

use std::collections::HashMap;
use std::io::Cursor;

use flate2::read::DeflateDecoder;
use quick_xml::events::{BytesStart, BytesText, Event};
use quick_xml::Reader;

use super::error::{LibraryError, LibraryResult};
use super::formats::DocumentFormatId;
use super::limits::{
    bound_extracted_text, clamp_table_range, read_stream_limited, ArchiveLimits, ExpansionBudget,
    MAX_TABLE_CELL_CHARS,
};

/// CSV 逻辑上只有一张工作表，名称固定。
pub const CSV_SHEET_NAME: &str = "CSV";

/// 公式缺少缓存值时的可理解表示。该文本只用于显示，不进入文本索引。
pub const UNCACHED_FORMULA_TEXT: &str = "未缓存";

const CONTENT_TYPES_PART: &str = "[Content_Types].xml";
const WORKBOOK_PART: &str = "xl/workbook.xml";
const WORKBOOK_RELS_PART: &str = "xl/_rels/workbook.xml.rels";
const SHARED_STRINGS_PART: &str = "xl/sharedStrings.xml";
const MACRO_PART: &str = "xl/vbaProject.bin";
const BINARY_WORKBOOK_PART: &str = "xl/workbook.bin";
const WORKBOOK_MAIN_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml";
const WORKBOOK_TEMPLATE_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.template.main+xml";
const WORKSHEET_RELATIONSHIP_SUFFIX: &str = "/worksheet";
const SHARED_STRINGS_RELATIONSHIP_SUFFIX: &str = "/sharedStrings";

/// 工作表元数据；`row_count` / `column_count` 只在读取过该工作表时有值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSheetInfo {
    pub index: usize,
    pub name: String,
    pub row_count: Option<usize>,
    pub column_count: Option<usize>,
}

/// 一次表格预览请求的结果范围，对应 `DocumentPreview::Table` 的字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRange {
    pub sheets: Vec<TableSheetInfo>,
    pub sheet_index: usize,
    pub start_row: usize,
    /// 请求范围内的单元格；行内按列索引定位（内部空洞为空字符串），行尾不补齐。
    pub cells: Vec<Vec<String>>,
    /// 与 `cells` 一一对应的行号（从 0 开始）。
    ///
    /// CSV 是连续行号；XLSX 稀疏表用真实的 Excel 行索引，因此可能出现跳号，
    /// 界面据此显示与表格软件一致的行号。
    pub row_numbers: Vec<usize>,
    pub column_count: usize,
    pub has_more_rows: bool,
    pub degraded_features: Vec<String>,
    pub notice: Option<String>,
}

/// 读取 CSV 的有限行列范围。
///
/// 逻辑上只有一张工作表（名称 [`CSV_SHEET_NAME`]）。整个文件都会被解码与扫描以得到
/// 总行数，但只有请求范围内的单元格会被保留。
pub fn read_csv_table(
    bytes: &[u8],
    start_row: usize,
    row_count: usize,
    column_count: usize,
    limits: ArchiveLimits,
) -> LibraryResult<TableRange> {
    let mut budget = ExpansionBudget::new(limits);
    budget.check_input_size(bytes.len() as u64)?;
    let (text, had_bom) = decode_csv_text(bytes)?;
    budget.charge(CSV_SHEET_NAME, text.len())?;

    let (start_row, rows_requested, columns) =
        clamp_table_range(start_row, row_count, column_count);
    let parsed = parse_csv_rows(&text, Some((start_row, rows_requested)), columns);
    let degraded = csv_degradations(&parsed);
    let total_rows = parsed.total_rows;
    let max_columns = parsed.max_columns;

    Ok(TableRange {
        sheets: vec![TableSheetInfo {
            index: 0,
            name: CSV_SHEET_NAME.to_string(),
            row_count: Some(total_rows),
            column_count: Some(max_columns),
        }],
        sheet_index: 0,
        start_row,
        has_more_rows: start_row.saturating_add(rows_requested) < total_rows,
        cells: parsed.rows,
        row_numbers: parsed.row_numbers,
        column_count: columns,
        degraded_features: degraded,
        notice: had_bom.then(|| "已忽略文件开头的 UTF-8 BOM。".to_string()),
    })
}

/// 读取 XLSX 某张工作表的有限行列范围。
///
/// 工作表按工作簿中 `<sheets>` 的顺序编号（只统计 worksheet 部件），只读取被请求
/// 的那张工作表，其余工作表只给出名称。
pub fn read_xlsx_table(
    bytes: &[u8],
    sheet_index: usize,
    start_row: usize,
    row_count: usize,
    column_count: usize,
    limits: ArchiveLimits,
) -> LibraryResult<TableRange> {
    let mut budget = ExpansionBudget::new(limits);
    budget.check_input_size(bytes.len() as u64)?;
    let package = XlsxPackage::open(bytes, &mut budget)?;
    let sheets = package.sheets(&mut budget)?;

    if sheet_index >= sheets.len() {
        return Err(LibraryError::Preview(format!(
            "工作表 {sheet_index} 不存在：工作簿共有 {} 张工作表。",
            sheets.len()
        )));
    }

    let (start_row, rows_requested, columns) =
        clamp_table_range(start_row, row_count, column_count);
    let shared = package.shared_strings(&mut budget)?;
    let sheet_xml = package.read_part(&sheets[sheet_index].part, &mut budget)?;
    let parsed = parse_sheet_xml(
        &sheet_xml,
        &shared,
        (start_row, rows_requested),
        columns,
        "XLSX",
    )?;

    let mut degraded = Vec::new();
    if parsed.uncached_formulas > 0 {
        degraded.push("部分公式缺少缓存值，已显示为「未缓存」。".to_string());
    }
    if parsed.truncated_cells > 0 {
        degraded.push(format!(
            "部分单元格文本超过 {MAX_TABLE_CELL_CHARS} 字符已截断。"
        ));
    }
    if package.has_part_prefix("xl/externalLinks/") {
        degraded.push("工作簿包含外部链接，已跳过其内容。".to_string());
    }
    if package.has_part_prefix("xl/embeddings/") {
        degraded.push("工作簿包含嵌入对象，已跳过。".to_string());
    }

    Ok(TableRange {
        sheets: sheets
            .iter()
            .enumerate()
            .map(|(index, sheet)| TableSheetInfo {
                index,
                name: sheet.name.clone(),
                row_count: (index == sheet_index).then_some(parsed.row_count),
                column_count: (index == sheet_index).then_some(parsed.column_count),
            })
            .collect(),
        sheet_index,
        start_row,
        has_more_rows: start_row.saturating_add(rows_requested) < parsed.row_count,
        cells: parsed.rows,
        row_numbers: parsed.row_numbers,
        column_count: columns,
        degraded_features: degraded,
        notice: None,
    })
}

/// 提取表格文档的可见文字，供搜索索引使用。
///
/// 返回的文本按 [`super::limits::MAX_EXTRACTED_TEXT_CHARS`] 截断；XLSX 的公式只取
/// 缓存值，缺少缓存值的公式不贡献文字（显示层的「未缓存」标记不进入索引）。
pub fn extract_table_text(bytes: &[u8], file_type: &str) -> LibraryResult<String> {
    match table_format_id(file_type)? {
        DocumentFormatId::Csv => {
            let mut budget = ExpansionBudget::new(ArchiveLimits::for_index());
            budget.check_input_size(bytes.len() as u64)?;
            let (text, _) = decode_csv_text(bytes)?;
            budget.charge(CSV_SHEET_NAME, text.len())?;
            let parsed = parse_csv_rows(&text, None, usize::MAX);
            let mut joined = String::new();
            for row in &parsed.rows {
                append_row_text(&mut joined, row);
            }
            Ok(bound_extracted_text(joined).text)
        }
        DocumentFormatId::Xlsx => {
            let mut budget = ExpansionBudget::new(ArchiveLimits::for_index());
            budget.check_input_size(bytes.len() as u64)?;
            let package = XlsxPackage::open(bytes, &mut budget)?;
            let sheets = package.sheets(&mut budget)?;
            let shared = package.shared_strings(&mut budget)?;
            let mut joined = String::new();
            for sheet in &sheets {
                if !joined.is_empty() {
                    joined.push('\n');
                }
                joined.push_str(&sheet.name);
                joined.push('\n');
                let sheet_xml = package.read_part(&sheet.part, &mut budget)?;
                let parsed = parse_sheet_xml(
                    &sheet_xml,
                    &shared,
                    (0, usize::MAX),
                    usize::MAX,
                    "XLSX",
                )?;
                for row in &parsed.rows {
                    append_row_text(&mut joined, row);
                }
            }
            Ok(bound_extracted_text(joined).text)
        }
        _ => Err(LibraryError::UnsupportedFile(format!(
            "表格文本提取不支持 {file_type} 文档。"
        ))),
    }
}

/// 真实结构校验：CSV 必须是可安全解码的 UTF-8 文本，XLSX 必须是结构完整的工作簿包。
///
/// 这里的判断不依赖扩展名，也不只看 ZIP 文件头：CSV 会做全文解码，XLSX 会解析内容
/// 类型、工作簿关系与每张工作表的根节点。
pub fn validate_table_document(bytes: &[u8], file_type: &str) -> Result<(), String> {
    match table_format_id(file_type) {
        Ok(DocumentFormatId::Csv) => decode_csv_text(bytes).map(|_| ()).map_err(|error| {
            format!("CSV 结构校验失败：{error}")
        }),
        Ok(DocumentFormatId::Xlsx) => {
            let mut budget = ExpansionBudget::new(ArchiveLimits::for_index());
            budget
                .check_input_size(bytes.len() as u64)
                .map_err(|error| format!("XLSX 结构校验失败：{error}"))?;
            let package = XlsxPackage::open(bytes, &mut budget)
                .map_err(|error| format!("XLSX 结构校验失败：{error}"))?;
            let sheets = package
                .sheets(&mut budget)
                .map_err(|error| format!("XLSX 结构校验失败：{error}"))?;
            for sheet in &sheets {
                let xml = package
                    .read_part(&sheet.part, &mut budget)
                    .map_err(|error| format!("XLSX 结构校验失败：{error}"))?;
                let root = xml_root_local_name(&xml, "XLSX")
                    .map_err(|error| format!("XLSX 结构校验失败：{error}"))?;
                if root != "worksheet" {
                    return Err(format!(
                        "XLSX 结构校验失败：工作表「{}」的根节点是 {root}，不是 worksheet。",
                        sheet.name
                    ));
                }
            }
            Ok(())
        }
        Ok(other) => Err(format!(
            "{} 不是表格文档，无法按表格结构校验。",
            other.as_str()
        )),
        Err(error) => Err(error.to_string()),
    }
}

fn table_format_id(file_type: &str) -> LibraryResult<DocumentFormatId> {
    let capability = super::formats::capability_for_file_type(file_type)
        .ok_or_else(|| LibraryError::UnsupportedFile(format!("不支持的文档格式：{file_type}")))?;
    match capability.id {
        DocumentFormatId::Csv | DocumentFormatId::Xlsx => Ok(capability.id),
        other => Err(LibraryError::UnsupportedFile(format!(
            "{} 不是表格文档。",
            other.as_str()
        ))),
    }
}

fn append_row_text(target: &mut String, row: &[String]) {
    let visible = row
        .iter()
        // 「未缓存」只是显示层的占位，不作为索引文字。
        .filter(|cell| !cell.is_empty() && cell.as_str() != UNCACHED_FORMULA_TEXT)
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    if visible.is_empty() {
        return;
    }
    if !target.is_empty() && !target.ends_with('\n') {
        target.push('\n');
    }
    target.push_str(&visible);
}

// ---------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------

/// 把 CSV 字节解码为文本，返回 `(文本, 是否忽略过 BOM)`。
///
/// 只接受 UTF-8（可带 BOM）：UTF-16、非法 UTF-8（如 GBK）与二进制内容都返回具体
/// 错误，绝不静默生成乱码。
fn decode_csv_text(bytes: &[u8]) -> LibraryResult<(String, bool)> {
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        return Err(LibraryError::ImportFile(
            "CSV 使用 UTF-16 编码，当前只支持 UTF-8；请先转换为 UTF-8。".to_string(),
        ));
    }
    if bytes.starts_with(b"PK\x03\x04") {
        return Err(LibraryError::ImportFile(
            "文件是压缩包（可能是 Office 文档），不是 CSV 文本。".to_string(),
        ));
    }
    let (contents, had_bom) = match bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        Some(rest) => (rest, true),
        None => (bytes, false),
    };
    if contents.contains(&0) {
        return Err(LibraryError::ImportFile(
            "CSV 包含二进制内容（NUL 字节），不是有效的文本表格。".to_string(),
        ));
    }
    match std::str::from_utf8(contents) {
        Ok(text) => Ok((text.to_string(), had_bom)),
        Err(error) => Err(LibraryError::ImportFile(format!(
            "CSV 解码失败：第 {} 字节起不是有效的 UTF-8（可能是 GBK/ANSI 编码），请先转换为 UTF-8。",
            error.valid_up_to()
        ))),
    }
}

struct CsvParseOutcome {
    /// 只有请求范围内的行；`window` 为 `None` 时是全部行。
    rows: Vec<Vec<String>>,
    /// 与 `rows` 一一对应的行号。
    row_numbers: Vec<usize>,
    total_rows: usize,
    max_columns: usize,
    truncated_cells: usize,
}

fn csv_degradations(parsed: &CsvParseOutcome) -> Vec<String> {
    if parsed.truncated_cells == 0 {
        return Vec::new();
    }
    vec![format!(
        "部分单元格文本超过 {MAX_TABLE_CELL_CHARS} 字符已截断。"
    )]
}

fn parse_csv_rows(
    text: &str,
    window: Option<(usize, usize)>,
    column_limit: usize,
) -> CsvParseOutcome {
    let mut builder = CsvBuilder::new(window, column_limit);
    let mut characters = text.chars().peekable();

    while let Some(character) = characters.next() {
        if builder.in_quotes() {
            match character {
                '"' => {
                    if characters.peek() == Some(&'"') {
                        characters.next();
                        builder.push_character('"');
                    } else {
                        builder.close_quotes();
                    }
                }
                // 引号内的换行属于单元格内容；CRLF 统一为 LF 便于按行呈现。
                '\r' => {
                    if characters.peek() == Some(&'\n') {
                        characters.next();
                    }
                    builder.push_character('\n');
                }
                other => builder.push_character(other),
            }
            continue;
        }

        match character {
            '"' if builder.field_is_empty() => builder.open_quotes(),
            ',' => builder.end_field(),
            '\r' => {
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }
                builder.end_row();
            }
            '\n' => builder.end_row(),
            other => builder.push_character(other),
        }
    }

    // 文件末尾没有换行时同样收尾；只有确实有内容时才产生一行。
    if builder.has_content() {
        builder.end_row();
    }
    builder.finish()
}

struct CsvBuilder {
    rows: Vec<Vec<String>>,
    row_numbers: Vec<usize>,
    total_rows: usize,
    max_columns: usize,
    truncated_cells: usize,
    row: Vec<String>,
    field: String,
    /// 当前字段是否已有内容；范围外的行不保留文本，但仍要区分
    /// `"` 是开始引号还是字段中间的普通字符，否则引号内的换行会被误判成行尾。
    field_started: bool,
    fields_in_row: usize,
    row_has_content: bool,
    in_quotes: bool,
    store_all: bool,
    window_start: usize,
    window_rows: usize,
    column_limit: usize,
}

impl CsvBuilder {
    fn new(window: Option<(usize, usize)>, column_limit: usize) -> Self {
        let (store_all, window_start, window_rows) = match window {
            Some((start, rows)) => (false, start, rows),
            None => (true, 0, usize::MAX),
        };
        Self {
            rows: Vec::new(),
            row_numbers: Vec::new(),
            total_rows: 0,
            max_columns: 0,
            truncated_cells: 0,
            row: Vec::new(),
            field: String::new(),
            field_started: false,
            fields_in_row: 0,
            row_has_content: false,
            in_quotes: false,
            store_all,
            window_start,
            window_rows,
            column_limit,
        }
    }

    fn in_quotes(&self) -> bool {
        self.in_quotes
    }

    fn field_is_empty(&self) -> bool {
        !self.field_started
    }

    fn has_content(&self) -> bool {
        self.row_has_content
    }

    /// 当前行是否落在请求范围内；范围外的行只统计不保留。
    fn storing(&self) -> bool {
        self.store_all
            || (self.total_rows >= self.window_start
                && self.total_rows - self.window_start < self.window_rows)
    }

    fn open_quotes(&mut self) {
        self.in_quotes = true;
        self.row_has_content = true;
        self.field_started = true;
    }

    fn close_quotes(&mut self) {
        self.in_quotes = false;
    }

    fn push_character(&mut self, character: char) {
        self.row_has_content = true;
        self.field_started = true;
        if self.storing() {
            self.field.push(character);
        }
    }

    fn end_field(&mut self) {
        self.row_has_content = true;
        self.fields_in_row += 1;
        self.max_columns = self.max_columns.max(self.fields_in_row);
        if self.storing() && self.row.len() < self.column_limit {
            let value = std::mem::take(&mut self.field);
            let value = self.truncate_cell(value);
            self.row.push(value);
        } else {
            self.field.clear();
        }
        self.field_started = false;
    }

    fn end_row(&mut self) {
        // 完全空行（没有任何字段、分隔符或引号）不产生记录，也不占行号。
        if !self.row_has_content && !self.in_quotes && self.fields_in_row == 0 {
            self.reset_row();
            return;
        }
        self.end_field();
        let index = self.total_rows;
        self.total_rows += 1;
        if (self.store_all
            || (index >= self.window_start && index - self.window_start < self.window_rows))
            && !self.row.is_empty()
        {
            self.row_numbers.push(index);
            self.rows.push(std::mem::take(&mut self.row));
        } else {
            self.row.clear();
        }
        self.reset_row();
    }

    fn reset_row(&mut self) {
        self.row.clear();
        self.field.clear();
        self.field_started = false;
        self.fields_in_row = 0;
        self.row_has_content = false;
        self.in_quotes = false;
    }

    fn truncate_cell(&mut self, value: String) -> String {
        // 字节长度不超过上限时字符数必然不超过上限，先做便宜的判断。
        if value.len() > MAX_TABLE_CELL_CHARS && value.chars().count() > MAX_TABLE_CELL_CHARS {
            self.truncated_cells += 1;
            return value.chars().take(MAX_TABLE_CELL_CHARS).collect();
        }
        value
    }

    fn finish(self) -> CsvParseOutcome {
        CsvParseOutcome {
            rows: self.rows,
            row_numbers: self.row_numbers,
            total_rows: self.total_rows,
            max_columns: self.max_columns,
            truncated_cells: self.truncated_cells,
        }
    }
}

// ---------------------------------------------------------------------------
// XLSX
// ---------------------------------------------------------------------------

struct WorkbookSheet {
    name: String,
    part: String,
}

#[derive(Debug, Clone)]
struct ZipEntryMeta {
    name: String,
    compression: u16,
    crc32: u32,
    compressed_size: usize,
    uncompressed_size: u64,
    data_start: usize,
}

/// 只解析 ZIP 中央目录的阅读器：先取得条目清单，再按需解压需要的部件。
///
/// 这样表格预览不会为了读一张工作表而展开整个工作簿；每个条目的声明大小、压缩比
/// 与累计解压量都在解压前交给 [`ExpansionBudget`] 校验。
struct ZipDirectory {
    entries: Vec<ZipEntryMeta>,
}

impl ZipDirectory {
    fn parse(archive: &[u8], format: &str) -> LibraryResult<Self> {
        if !archive.starts_with(b"PK\x03\x04") {
            return Err(LibraryError::ImportFile(format!(
                "不是有效的 {format} 压缩包结构（可能是旧版 .xls、.ods 或已加密工作簿）。"
            )));
        }
        let eocd = find_zip_eocd(archive).ok_or_else(|| {
            LibraryError::ImportFile(format!("{format} 文件结构无效：找不到 ZIP 中央目录。"))
        })?;
        let entry_count = read_u16(archive, eocd + 10)? as usize;
        let mut cursor = read_u32(archive, eocd + 16)? as usize;
        let mut entries = Vec::with_capacity(entry_count);
        let mut names = std::collections::HashSet::new();

        for _ in 0..entry_count {
            if read_u32(archive, cursor)? != 0x0201_4b50 {
                return Err(LibraryError::ImportFile(format!(
                    "{format} 文件结构无效：中央目录项损坏。"
                )));
            }
            let flags = read_u16(archive, cursor + 8)?;
            if flags & 0x0001 != 0 {
                return Err(LibraryError::ImportFile(format!(
                    "{format} 已加密，无法读取。"
                )));
            }
            let compression = read_u16(archive, cursor + 10)?;
            if !matches!(compression, 0 | 8) {
                return Err(LibraryError::ImportFile(format!(
                    "{format} 使用了不支持的压缩方式：{compression}。"
                )));
            }
            let crc32 = read_u32(archive, cursor + 16)?;
            let compressed_size = read_u32(archive, cursor + 20)? as usize;
            let uncompressed_size = read_u32(archive, cursor + 24)?;
            if uncompressed_size == u32::MAX {
                return Err(LibraryError::ImportFile(format!(
                    "暂不支持 ZIP64 格式的 {format} 文件。"
                )));
            }
            let name_length = read_u16(archive, cursor + 28)? as usize;
            let extra_length = read_u16(archive, cursor + 30)? as usize;
            let comment_length = read_u16(archive, cursor + 32)? as usize;
            let local_header_offset = read_u32(archive, cursor + 42)? as usize;

            let name_start = cursor + 46;
            let name_end = name_start.checked_add(name_length).ok_or_else(|| {
                LibraryError::ImportFile(format!("{format} 文件结构无效：文件名长度溢出。"))
            })?;
            let name_bytes = archive.get(name_start..name_end).ok_or_else(|| {
                LibraryError::ImportFile(format!("{format} 文件结构无效：文件名超出范围。"))
            })?;
            let name = String::from_utf8(name_bytes.to_vec()).map_err(|_| {
                LibraryError::ImportFile(format!("{format} 文件结构无效：文件名不是有效 UTF-8。"))
            })?;
            let normalized = normalize_part_name(&name, format)?;
            if !names.insert(normalized.clone()) {
                return Err(LibraryError::ImportFile(format!(
                    "{format} 文件结构无效：部件名称重复：{normalized}。"
                )));
            }

            let data_start = zip_local_data_start(archive, local_header_offset, format)?;
            let data_end = data_start.checked_add(compressed_size).ok_or_else(|| {
                LibraryError::ImportFile(format!("{format} 文件结构无效：正文长度溢出。"))
            })?;
            if archive.get(data_start..data_end).is_none() {
                return Err(LibraryError::ImportFile(format!(
                    "{format} 文件结构无效：部件 {normalized} 的正文超出文件范围。"
                )));
            }

            entries.push(ZipEntryMeta {
                name: normalized,
                compression,
                crc32,
                compressed_size,
                uncompressed_size: uncompressed_size as u64,
                data_start,
            });
            cursor = name_end
                .checked_add(extra_length)
                .and_then(|value| value.checked_add(comment_length))
                .ok_or_else(|| {
                    LibraryError::ImportFile(format!("{format} 文件结构无效：目录项长度溢出。"))
                })?;
        }
        Ok(Self { entries })
    }

    fn find(&self, name: &str) -> Option<&ZipEntryMeta> {
        self.entries.iter().find(|entry| entry.name == name)
    }

    fn has_prefix(&self, prefix: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.name.starts_with(prefix))
    }

    /// 按需解压一个部件：声明大小、压缩比、实际解压量与累计量都经过预算校验。
    fn read(
        &self,
        archive: &[u8],
        name: &str,
        budget: &mut ExpansionBudget,
        format: &str,
    ) -> LibraryResult<Vec<u8>> {
        let entry = self.find(name).ok_or_else(|| {
            LibraryError::ImportFile(format!("{format} 文件缺少 {name}。"))
        })?;
        self.read_entry(archive, entry, budget, format)
    }

    fn read_optional(
        &self,
        archive: &[u8],
        name: &str,
        budget: &mut ExpansionBudget,
        format: &str,
    ) -> LibraryResult<Option<Vec<u8>>> {
        match self.find(name) {
            Some(entry) => self.read_entry(archive, entry, budget, format).map(Some),
            None => Ok(None),
        }
    }

    fn read_entry(
        &self,
        archive: &[u8],
        entry: &ZipEntryMeta,
        budget: &mut ExpansionBudget,
        format: &str,
    ) -> LibraryResult<Vec<u8>> {
        budget.check_entry_size(&entry.name, entry.uncompressed_size)?;
        budget.check_compression_ratio(
            &entry.name,
            entry.compressed_size as u64,
            entry.uncompressed_size,
        )?;
        let compressed = archive
            .get(entry.data_start..entry.data_start + entry.compressed_size)
            .ok_or_else(|| {
                LibraryError::ImportFile(format!(
                    "{format} 文件结构无效：部件 {} 的正文超出文件范围。",
                    entry.name
                ))
            })?;
        let contents = match entry.compression {
            0 => read_stream_limited(
                Cursor::new(compressed),
                &entry.name,
                entry.uncompressed_size,
                budget,
            )?,
            8 => read_stream_limited(
                DeflateDecoder::new(Cursor::new(compressed)),
                &entry.name,
                entry.uncompressed_size,
                budget,
            )?,
            method => {
                return Err(LibraryError::ImportFile(format!(
                    "{format} 使用了不支持的压缩方式：{method}。"
                )))
            }
        };
        if contents.len() as u64 != entry.uncompressed_size {
            return Err(LibraryError::ImportFile(format!(
                "{format} 文件结构无效：部件 {} 的解压长度不一致。",
                entry.name
            )));
        }
        if entry.crc32 != 0 && crc32(&contents) != entry.crc32 {
            return Err(LibraryError::ImportFile(format!(
                "{format} 文件结构无效：部件 {} 的 CRC 不匹配。",
                entry.name
            )));
        }
        Ok(contents)
    }
}

struct XlsxPackage<'a> {
    bytes: &'a [u8],
    directory: ZipDirectory,
}

impl<'a> XlsxPackage<'a> {
    fn open(bytes: &'a [u8], budget: &mut ExpansionBudget) -> LibraryResult<Self> {
        if bytes.starts_with(&[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]) {
            return Err(LibraryError::ImportFile(
                "文件是 OLE 复合文档（旧版 .xls 或加密工作簿），不是有效的 XLSX。".to_string(),
            ));
        }
        let directory = ZipDirectory::parse(bytes, "XLSX")?;
        let package = Self { bytes, directory };

        if let Some(mimetype) = package.directory.read_optional(
            package.bytes,
            "mimetype",
            budget,
            "XLSX",
        )? {
            if mimetype.starts_with(b"application/vnd.oasis.opendocument") {
                return Err(LibraryError::ImportFile(
                    "这是 OpenDocument 表格（ODS），暂不支持；请另存为 XLSX。".to_string(),
                ));
            }
        }
        if package.directory.find(MACRO_PART).is_some() {
            return Err(LibraryError::ImportFile(
                "这是启用宏的工作簿（XLSM），出于安全考虑已拒绝读取。".to_string(),
            ));
        }
        if package.directory.find(BINARY_WORKBOOK_PART).is_some() {
            return Err(LibraryError::ImportFile(
                "这是二进制工作簿（XLSB），暂不支持；请另存为 XLSX。".to_string(),
            ));
        }
        package.require_spreadsheet_content_type(budget)?;
        Ok(package)
    }

    /// 内容类型必须把 `xl/workbook.xml` 声明为电子表格主部件。
    fn require_spreadsheet_content_type(&self, budget: &mut ExpansionBudget) -> LibraryResult<()> {
        let xml = self
            .directory
            .read(self.bytes, CONTENT_TYPES_PART, budget, "XLSX")?;
        if xml_root_local_name(&xml, "XLSX")? != "Types" {
            return Err(LibraryError::ImportFile(
                "XLSX 结构无效：[Content_Types].xml 根节点不是 Types。".to_string(),
            ));
        }
        let content_types = ContentTypes::parse(&xml)?;
        match content_types.declared_type(WORKBOOK_PART) {
            Some(WORKBOOK_MAIN_CONTENT_TYPE) | Some(WORKBOOK_TEMPLATE_CONTENT_TYPE) => Ok(()),
            Some(other) => Err(LibraryError::ImportFile(format!(
                "XLSX 结构无效：{WORKBOOK_PART} 的声明内容类型是 {other}，不是电子表格工作簿。"
            ))),
            None => Err(LibraryError::ImportFile(format!(
                "XLSX 结构无效：[Content_Types].xml 未声明 {WORKBOOK_PART} 的内容类型。"
            ))),
        }
    }

    /// 工作簿里可读取的工作表（只统计 worksheet 部件，图表工作表没有单元格）。
    fn sheets(&self, budget: &mut ExpansionBudget) -> LibraryResult<Vec<WorkbookSheet>> {
        let workbook = self
            .directory
            .read(self.bytes, WORKBOOK_PART, budget, "XLSX")?;
        if xml_root_local_name(&workbook, "XLSX")? != "workbook" {
            return Err(LibraryError::ImportFile(
                "XLSX 结构无效：xl/workbook.xml 不是工作簿定义。".to_string(),
            ));
        }
        let relationships = self
            .directory
            .read_optional(self.bytes, WORKBOOK_RELS_PART, budget, "XLSX")?
            .ok_or_else(|| {
                LibraryError::ImportFile(
                    "XLSX 结构无效：缺少 xl/_rels/workbook.xml.rels。".to_string(),
                )
            })?;
        let relationship_map = parse_relationships(&relationships, "XLSX")?;

        let mut sheets = Vec::new();
        for sheet in workbook_sheet_references(&workbook)? {
            let relationship = relationship_map.get(&sheet.relationship_id).ok_or_else(|| {
                LibraryError::ImportFile(format!(
                    "XLSX 结构无效：工作表「{}」引用了不存在的关系 {}。",
                    sheet.name, sheet.relationship_id
                ))
            })?;
            if relationship.external
                || !relationship
                    .relationship_type
                    .to_ascii_lowercase()
                    .ends_with(WORKSHEET_RELATIONSHIP_SUFFIX)
            {
                // 图表工作表等没有可预览的单元格，跳过而不是报错。
                continue;
            }
            let part = resolve_internal_target(WORKBOOK_PART, &relationship.target, "XLSX")?;
            if self.directory.find(&part).is_none() {
                return Err(LibraryError::ImportFile(format!(
                    "XLSX 结构无效：工作表「{}」的部件不存在：{part}。",
                    sheet.name
                )));
            }
            sheets.push(WorkbookSheet {
                name: sheet.name,
                part,
            });
        }
        if sheets.is_empty() {
            return Err(LibraryError::ImportFile(
                "XLSX 结构无效：工作簿没有可读取的工作表。".to_string(),
            ));
        }
        Ok(sheets)
    }

    /// 共享字符串表；部件不存在时返回空表（只在遇到 `t="s"` 时报错）。
    fn shared_strings(&self, budget: &mut ExpansionBudget) -> LibraryResult<SharedStrings> {
        let part = self
            .shared_strings_part(budget)?
            .unwrap_or_else(|| SHARED_STRINGS_PART.to_string());
        match self
            .directory
            .read_optional(self.bytes, &part, budget, "XLSX")?
        {
            Some(xml) => parse_shared_strings(&xml, "XLSX").map(|values| SharedStrings {
                values,
                present: true,
            }),
            None => Ok(SharedStrings {
                values: Vec::new(),
                present: false,
            }),
        }
    }

    fn shared_strings_part(&self, budget: &mut ExpansionBudget) -> LibraryResult<Option<String>> {
        let Some(relationships) = self
            .directory
            .read_optional(self.bytes, WORKBOOK_RELS_PART, budget, "XLSX")?
        else {
            return Ok(None);
        };
        for relationship in parse_relationships_list(&relationships) {
            if relationship
                .relationship_type
                .to_ascii_lowercase()
                .ends_with(SHARED_STRINGS_RELATIONSHIP_SUFFIX)
                && !relationship.external
            {
                return resolve_internal_target(WORKBOOK_PART, &relationship.target, "XLSX")
                    .map(Some);
            }
        }
        Ok(None)
    }

    fn read_part(&self, name: &str, budget: &mut ExpansionBudget) -> LibraryResult<Vec<u8>> {
        self.directory.read(self.bytes, name, budget, "XLSX")
    }

    fn has_part_prefix(&self, prefix: &str) -> bool {
        self.directory.has_prefix(prefix)
    }
}

/// 共享字符串表；`present` 为 false 说明包里没有该部件。
struct SharedStrings {
    values: Vec<String>,
    present: bool,
}

impl SharedStrings {
    fn get(&self, index: usize) -> LibraryResult<&str> {
        if !self.present {
            return Err(LibraryError::ImportFile(
                "XLSX 结构无效：单元格引用了共享字符串，但缺少 xl/sharedStrings.xml。".to_string(),
            ));
        }
        self.values.get(index).map(String::as_str).ok_or_else(|| {
            LibraryError::ImportFile(format!(
                "XLSX 结构无效：共享字符串索引 {index} 越界（共 {} 项）。",
                self.values.len()
            ))
        })
    }
}

struct ContentTypes {
    defaults: HashMap<String, String>,
    overrides: HashMap<String, String>,
}

impl ContentTypes {
    fn parse(xml: &[u8]) -> LibraryResult<Self> {
        let mut defaults = HashMap::new();
        let mut overrides = HashMap::new();
        let mut reader = Reader::from_reader(xml);
        reader.config_mut().trim_text(true);
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer) {
                Ok(Event::Start(element)) | Ok(Event::Empty(element)) => {
                    let local_name = element.local_name();
                    let name = local_name.as_ref();
                    if name == "Default" || name == "Override" {
                        let attributes = read_attributes(&element, "XLSX [Content_Types].xml")?;
                        if name == "Default" {
                            if let (Some(extension), Some(content_type)) =
                                (attributes.get("Extension"), attributes.get("ContentType"))
                            {
                                defaults.insert(
                                    extension.trim_start_matches('.').to_ascii_lowercase(),
                                    content_type.to_string(),
                                );
                            }
                        } else if let (Some(part_name), Some(content_type)) =
                            (attributes.get("PartName"), attributes.get("ContentType"))
                        {
                            overrides.insert(
                                normalize_part_name(part_name, "XLSX")?,
                                content_type.to_string(),
                            );
                        }
                    }
                }
                Ok(Event::Eof) => break,
                Err(error) => {
                    return Err(LibraryError::ImportFile(format!(
                        "无法解析 XLSX [Content_Types].xml：{error}"
                    )))
                }
                _ => {}
            }
            buffer.clear();
        }
        Ok(Self {
            defaults,
            overrides,
        })
    }

    fn declared_type(&self, part_name: &str) -> Option<&str> {
        if let Some(content_type) = self.overrides.get(part_name) {
            return Some(content_type);
        }
        let extension = part_name.rsplit_once('.').map(|(_, extension)| extension)?;
        self.defaults
            .get(&extension.to_ascii_lowercase())
            .map(String::as_str)
    }
}

#[derive(Debug, Clone)]
struct Relationship {
    id: String,
    relationship_type: String,
    target: String,
    external: bool,
}

struct SheetReference {
    name: String,
    relationship_id: String,
}

fn workbook_sheet_references(xml: &[u8]) -> LibraryResult<Vec<SheetReference>> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut sheets = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element)) | Ok(Event::Empty(element)) => {
                if element.local_name().as_ref() == "sheet" {
                    let attributes = read_attributes(&element, "XLSX xl/workbook.xml")?;
                    let name = attributes
                        .get("name")
                        .filter(|value| !value.trim().is_empty())
                        .cloned()
                        .unwrap_or_else(|| format!("工作表 {}", sheets.len() + 1));
                    let relationship_id = attributes
                        .get("r:id")
                        .or_else(|| attributes.get("id"))
                        .cloned()
                        .ok_or_else(|| {
                            LibraryError::ImportFile(format!(
                                "XLSX 结构无效：工作表「{name}」缺少 r:id。"
                            ))
                        })?;
                    sheets.push(SheetReference {
                        name,
                        relationship_id,
                    });
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => {
                return Err(LibraryError::ImportFile(format!(
                    "无法解析 XLSX 工作表清单：{error}"
                )))
            }
            _ => {}
        }
        buffer.clear();
    }
    Ok(sheets)
}

fn parse_relationships(xml: &[u8], format: &str) -> LibraryResult<HashMap<String, Relationship>> {
    let mut map = HashMap::new();
    for relationship in parse_relationships_list(xml) {
        if map
            .insert(relationship.id.clone(), relationship.clone())
            .is_some()
        {
            return Err(LibraryError::ImportFile(format!(
                "{format} 结构无效：关系 Id 重复。"
            )));
        }
    }
    Ok(map)
}

fn parse_relationships_list(xml: &[u8]) -> Vec<Relationship> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut relationships = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element)) | Ok(Event::Empty(element)) => {
                if element.local_name().as_ref() == "Relationship" {
                    if let Ok(attributes) = read_attributes(&element, "XLSX relationship") {
                        if let (Some(id), Some(relationship_type), Some(target)) = (
                            attributes.get("Id"),
                            attributes.get("Type"),
                            attributes.get("Target"),
                        ) {
                            let external = attributes
                                .get("TargetMode")
                                .is_some_and(|mode| mode.eq_ignore_ascii_case("External"))
                                || target_is_external(target);
                            relationships.push(Relationship {
                                id: id.clone(),
                                relationship_type: relationship_type.clone(),
                                target: target.clone(),
                                external,
                            });
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buffer.clear();
    }
    relationships
}

fn parse_shared_strings(xml: &[u8], format: &str) -> LibraryResult<Vec<String>> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut strings = Vec::new();
    let mut current: Option<String> = None;
    let mut in_text = false;
    let mut in_phonetic = false;
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element)) => match element.local_name().as_ref() {
                "si" => current = Some(String::new()),
                "rPh" => in_phonetic = true,
                "t" if current.is_some() && !in_phonetic => in_text = true,
                _ => {}
            },
            Ok(Event::Empty(element)) => {
                if element.local_name().as_ref() == "si" {
                    strings.push(String::new());
                }
            }
            Ok(Event::End(element)) => match element.local_name().as_ref() {
                "t" => in_text = false,
                "rPh" => in_phonetic = false,
                "si" => {
                    if let Some(value) = current.take() {
                        strings.push(value);
                    }
                }
                _ => {}
            },
            Ok(Event::Text(text)) if in_text => {
                let decoded = decode_xml_text(&text, format)?;
                if let Some(value) = current.as_mut() {
                    value.push_str(decoded.as_ref());
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => {
                return Err(LibraryError::ImportFile(format!(
                    "无法解析 {format} sharedStrings.xml：{error}"
                )))
            }
            _ => {}
        }
        buffer.clear();
    }
    Ok(strings)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CellKind {
    SharedString,
    InlineString,
    Boolean,
    Error,
    Text,
    Number,
}

impl CellKind {
    fn from_attribute(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some("s") => Self::SharedString,
            Some("inlineStr") => Self::InlineString,
            Some("b") => Self::Boolean,
            Some("e") => Self::Error,
            Some("str") => Self::Text,
            _ => Self::Number,
        }
    }
}

struct OpenCell {
    column: usize,
    kind: CellKind,
    has_formula: bool,
    value: String,
    inline_text: String,
    in_value: bool,
    in_inline: bool,
    in_text: bool,
}

struct SheetParse {
    rows: Vec<Vec<String>>,
    /// 与 `rows` 一一对应的行索引；稀疏表会跳号。
    row_numbers: Vec<usize>,
    /// 工作表的最大行索引 + 1（用 `r` 属性定位，稀疏行不会被顺序硬填）。
    row_count: usize,
    column_count: usize,
    uncached_formulas: usize,
    truncated_cells: usize,
}

struct SheetParser<'a> {
    shared: &'a SharedStrings,
    window_start: usize,
    window_rows: usize,
    columns: usize,
    current_row: Option<usize>,
    last_column: Option<usize>,
    row_cells: Vec<String>,
    cell: Option<OpenCell>,
    max_row_index: Option<usize>,
    max_column_index: Option<usize>,
    rows: Vec<Vec<String>>,
    row_numbers: Vec<usize>,
    uncached_formulas: usize,
    truncated_cells: usize,
}

impl SheetParser<'_> {
    fn in_window(&self, row_index: usize) -> bool {
        row_index >= self.window_start && row_index - self.window_start < self.window_rows
    }

    fn start_row(&mut self, element: &BytesStart<'_>) -> LibraryResult<()> {
        let attributes = read_attributes(element, "XLSX worksheet row")?;
        let row_index = match attributes.get("r") {
            Some(value) => parse_row_number(value)?.unwrap_or_else(|| {
                self.current_row.map(|row| row + 1).unwrap_or_default()
            }),
            None => self.current_row.map(|row| row + 1).unwrap_or_default(),
        };
        self.current_row = Some(row_index);
        self.last_column = None;
        self.row_cells.clear();
        Ok(())
    }

    fn start_cell(&mut self, element: &BytesStart<'_>) -> LibraryResult<()> {
        let attributes = read_attributes(element, "XLSX worksheet cell")?;
        let column = match attributes.get("r") {
            Some(reference) => cell_reference_column(reference)?.unwrap_or_else(|| {
                self.last_column.map(|column| column + 1).unwrap_or_default()
            }),
            None => self
                .last_column
                .map(|column| column + 1)
                .unwrap_or_default(),
        };
        self.cell = Some(OpenCell {
            column,
            kind: CellKind::from_attribute(attributes.get("t").map(String::as_str)),
            has_formula: false,
            value: String::new(),
            inline_text: String::new(),
            in_value: false,
            in_inline: false,
            in_text: false,
        });
        Ok(())
    }

    fn finish_cell(&mut self) -> LibraryResult<()> {
        let Some(cell) = self.cell.take() else {
            return Ok(());
        };
        let row_index = self.current_row.unwrap_or_default();
        let value = match cell.kind {
            CellKind::SharedString => {
                let raw = cell.value.trim();
                if raw.is_empty() {
                    None
                } else {
                    let index: usize = raw.parse().map_err(|_| {
                        LibraryError::ImportFile(format!(
                            "XLSX 结构无效：共享字符串索引 {raw} 不是有效数字。"
                        ))
                    })?;
                    Some(self.shared.get(index)?.to_string())
                }
            }
            CellKind::InlineString => Some(cell.inline_text),
            CellKind::Boolean => match cell.value.trim() {
                "1" => Some("TRUE".to_string()),
                "0" => Some("FALSE".to_string()),
                other if !other.is_empty() => Some(other.to_string()),
                _ => None,
            },
            CellKind::Error => {
                let text = cell.value.trim();
                (!text.is_empty()).then(|| text.to_string())
            }
            // 公式只读缓存值：`t="str"` 与数字都取 <v> 的原样文本，绝不求值。
            CellKind::Text | CellKind::Number => {
                (!cell.value.is_empty()).then(|| cell.value.clone())
            }
        };
        let value = match value {
            Some(text) if text.is_empty() => None,
            Some(text) => Some(text),
            None if cell.has_formula => {
                self.uncached_formulas += 1;
                Some(UNCACHED_FORMULA_TEXT.to_string())
            }
            None => None,
        };

        self.last_column = Some(cell.column);
        if let Some(value) = value {
            self.max_row_index = Some(
                self.max_row_index
                    .map_or(row_index, |current| current.max(row_index)),
            );
            self.max_column_index = Some(
                self.max_column_index
                    .map_or(cell.column, |current| current.max(cell.column)),
            );
            if self.in_window(row_index) {
                if self.columns > 0 && self.row_cells.len() < self.columns {
                    while self.row_cells.len() < cell.column && self.row_cells.len() < self.columns {
                        self.row_cells.push(String::new());
                    }
                    if cell.column < self.columns {
                        let value = self.truncate_cell(value);
                        self.row_cells.push(value);
                    }
                }
            }
        }
        Ok(())
    }

    fn truncate_cell(&mut self, value: String) -> String {
        if value.len() > MAX_TABLE_CELL_CHARS && value.chars().count() > MAX_TABLE_CELL_CHARS {
            self.truncated_cells += 1;
            return value.chars().take(MAX_TABLE_CELL_CHARS).collect();
        }
        value
    }

    fn finish_row(&mut self) {
        if self
            .current_row
            .is_some_and(|row_index| self.in_window(row_index))
            && !self.row_cells.is_empty()
        {
            self.row_numbers
                .push(self.current_row.unwrap_or_default());
            self.rows.push(std::mem::take(&mut self.row_cells));
        }
        self.row_cells.clear();
        self.cell = None;
        self.last_column = None;
    }

    fn finish(self) -> SheetParse {
        SheetParse {
            rows: self.rows,
            row_numbers: self.row_numbers,
            row_count: self.max_row_index.map_or(0, |index| index + 1),
            column_count: self.max_column_index.map_or(0, |index| index + 1),
            uncached_formulas: self.uncached_formulas,
            truncated_cells: self.truncated_cells,
        }
    }
}

/// 解析工作表 XML，返回请求范围内的单元格与整张表的行列范围。
fn parse_sheet_xml(
    xml: &[u8],
    shared: &SharedStrings,
    window: (usize, usize),
    columns: usize,
    format: &str,
) -> LibraryResult<SheetParse> {
    let mut parser = SheetParser {
        shared,
        window_start: window.0,
        window_rows: window.1,
        columns,
        current_row: None,
        last_column: None,
        row_cells: Vec::new(),
        cell: None,
        max_row_index: None,
        max_column_index: None,
        rows: Vec::new(),
        row_numbers: Vec::new(),
        uncached_formulas: 0,
        truncated_cells: 0,
    };
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element)) => match element.local_name().as_ref() {
                "row" => parser.start_row(&element)?,
                "c" => parser.start_cell(&element)?,
                "v" => {
                    if let Some(cell) = parser.cell.as_mut() {
                        cell.in_value = true;
                    }
                }
                "f" => {
                    if let Some(cell) = parser.cell.as_mut() {
                        cell.has_formula = true;
                    }
                }
                "is" => {
                    if let Some(cell) = parser.cell.as_mut() {
                        cell.in_inline = true;
                    }
                }
                "t" => {
                    if let Some(cell) = parser.cell.as_mut() {
                        if cell.in_inline {
                            cell.in_text = true;
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::Empty(element)) => match element.local_name().as_ref() {
                "row" => parser.start_row(&element)?,
                "c" => {
                    parser.start_cell(&element)?;
                    parser.finish_cell()?;
                }
                "f" => {
                    if let Some(cell) = parser.cell.as_mut() {
                        cell.has_formula = true;
                    }
                }
                _ => {}
            },
            Ok(Event::End(element)) => match element.local_name().as_ref() {
                "v" => {
                    if let Some(cell) = parser.cell.as_mut() {
                        cell.in_value = false;
                    }
                }
                "is" => {
                    if let Some(cell) = parser.cell.as_mut() {
                        cell.in_inline = false;
                    }
                }
                "t" => {
                    if let Some(cell) = parser.cell.as_mut() {
                        cell.in_text = false;
                    }
                }
                "c" => parser.finish_cell()?,
                "row" => parser.finish_row(),
                _ => {}
            },
            Ok(Event::Text(text)) => {
                if let Some(cell) = parser.cell.as_mut() {
                    if cell.in_value || cell.in_text {
                        let decoded = decode_xml_text(&text, format)?;
                        if cell.in_value {
                            cell.value.push_str(decoded.as_ref());
                        }
                        if cell.in_text {
                            cell.inline_text.push_str(decoded.as_ref());
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => {
                return Err(LibraryError::ImportFile(format!(
                    "无法解析 {format} 工作表 XML：{error}"
                )))
            }
            _ => {}
        }
        buffer.clear();
    }
    Ok(parser.finish())
}

// ---------------------------------------------------------------------------
// 通用小工具
// ---------------------------------------------------------------------------

fn xml_root_local_name(xml: &[u8], format: &str) -> LibraryResult<String> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element)) | Ok(Event::Empty(element)) => {
                return Ok(element.local_name().as_ref().to_string())
            }
            Ok(Event::Eof) => {
                return Err(LibraryError::ImportFile(format!(
                    "{format} XML 为空或缺少根节点。"
                )))
            }
            Err(error) => {
                return Err(LibraryError::ImportFile(format!(
                    "无法解析 {format} XML：{error}"
                )))
            }
            _ => {}
        }
        buffer.clear();
    }
}

/// 解码 XML 文本节点：先按 XML 1.0 归一换行，再展开实体引用。
fn decode_xml_text(text: &BytesText<'_>, format: &str) -> LibraryResult<String> {
    let raw = text.xml10_content();
    quick_xml::escape::unescape(raw.as_ref())
        .map(|decoded| decoded.into_owned())
        .map_err(|error| LibraryError::ImportFile(format!("无法解析 {format} XML 文本：{error}")))
}

fn read_attributes(
    element: &BytesStart<'_>,
    context: &str,
) -> LibraryResult<HashMap<String, String>> {
    let mut attributes = HashMap::new();
    for result in element.attributes().with_checks(false) {
        let attribute = result
            .map_err(|error| LibraryError::ImportFile(format!("{context} 属性无效：{error}")))?;
        let key = attribute.key.as_ref().to_string();
        let value = attribute
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|error| LibraryError::ImportFile(format!("{context} 属性编码无效：{error}")))?
            .into_owned();
        attributes.insert(key, value);
    }
    Ok(attributes)
}

/// 解析 `<row r="5">` 的行号（1 基）为 0 基索引。
fn parse_row_number(value: &str) -> LibraryResult<Option<usize>> {
    let digits = value.trim();
    if digits.is_empty() {
        return Ok(None);
    }
    let number: usize = digits.parse().map_err(|_| {
        LibraryError::ImportFile(format!("XLSX 结构无效：行号 {digits} 不是有效数字。"))
    })?;
    Ok(Some(number.saturating_sub(1)))
}

/// 解析 `C12` 形式的单元格引用，返回 0 基列索引。
fn cell_reference_column(reference: &str) -> LibraryResult<Option<usize>> {
    let reference = reference.trim();
    if reference.is_empty() {
        return Ok(None);
    }
    let mut column = 0usize;
    let mut saw_letter = false;
    for character in reference.chars() {
        if character.is_ascii_alphabetic() {
            saw_letter = true;
            let value = (character.to_ascii_uppercase() as u8 - b'A') as usize + 1;
            column = column
                .checked_mul(26)
                .and_then(|current| current.checked_add(value))
                .ok_or_else(|| {
                    LibraryError::ImportFile(format!(
                        "XLSX 结构无效：单元格引用 {reference} 超出范围。"
                    ))
                })?;
        } else {
            break;
        }
    }
    if !saw_letter {
        return Err(LibraryError::ImportFile(format!(
            "XLSX 结构无效：单元格引用 {reference} 缺少列字母。"
        )));
    }
    Ok(Some(column - 1))
}

fn normalize_part_name(part_name: &str, format: &str) -> LibraryResult<String> {
    let normalized = part_name.trim().trim_start_matches('/');
    if normalized.is_empty() || normalized.contains('\\') || normalized.split('/').any(|p| p == "..")
    {
        return Err(LibraryError::ImportFile(format!(
            "{format} 结构无效：部件名称异常：{part_name}。"
        )));
    }
    Ok(normalized.to_string())
}

fn resolve_internal_target(source_part: &str, target: &str, format: &str) -> LibraryResult<String> {
    if target_is_external(target) || target.contains('\\') {
        return Err(LibraryError::ImportFile(format!(
            "{format} 结构无效：关系目标不是包内部部件：{target}。"
        )));
    }
    let target = target.trim().split(['#', '?']).next().unwrap_or_default();
    if target.is_empty() {
        return normalize_part_name(source_part, format);
    }
    let raw = if target.starts_with('/') {
        target.trim_start_matches('/').to_string()
    } else {
        let directory = source_part
            .rsplit_once('/')
            .map(|(directory, _)| directory)
            .unwrap_or("");
        format!("{directory}/{target}")
    };
    let mut parts: Vec<&str> = Vec::new();
    for component in raw.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(LibraryError::ImportFile(format!(
                        "{format} 结构无效：关系目标逃逸包根目录。"
                    )));
                }
            }
            component => parts.push(component),
        }
    }
    if parts.is_empty() {
        return Err(LibraryError::ImportFile(format!(
            "{format} 结构无效：关系目标为空。"
        )));
    }
    Ok(parts.join("/"))
}

fn target_is_external(target: &str) -> bool {
    let target = target.trim().to_ascii_lowercase();
    target.starts_with("//")
        || target.starts_with("\\\\")
        || target.split_once(':').is_some_and(|(scheme, _)| {
            scheme
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || ".-+".contains(character))
        })
}

fn find_zip_eocd(archive: &[u8]) -> Option<usize> {
    let search_start = archive.len().saturating_sub(65_557);
    archive[search_start..]
        .windows(4)
        .rposition(|window| window == b"PK\x05\x06")
        .map(|index| search_start + index)
}

fn zip_local_data_start(
    archive: &[u8],
    local_header_offset: usize,
    format: &str,
) -> LibraryResult<usize> {
    if read_u32(archive, local_header_offset)? != 0x0403_4b50 {
        return Err(LibraryError::ImportFile(format!(
            "{format} 文件结构无效：本地文件头损坏。"
        )));
    }
    let name_length = read_u16(archive, local_header_offset + 26)? as usize;
    let extra_length = read_u16(archive, local_header_offset + 28)? as usize;
    local_header_offset
        .checked_add(30)
        .and_then(|value| value.checked_add(name_length))
        .and_then(|value| value.checked_add(extra_length))
        .ok_or_else(|| LibraryError::ImportFile(format!("{format} 文件结构无效：本地头长度溢出。")))
}

fn read_u16(bytes: &[u8], offset: usize) -> LibraryResult<u16> {
    let value = bytes.get(offset..offset + 2).ok_or_else(|| {
        LibraryError::ImportFile("表格文件结构无效：文件意外结束。".to_string())
    })?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> LibraryResult<u32> {
    let value = bytes.get(offset..offset + 4).ok_or_else(|| {
        LibraryError::ImportFile("表格文件结构无效：文件意外结束。".to_string())
    })?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}
