//! 压缩文档（DOCX/PPTX/XLSX）解析与表格读取的统一资源上限。
//!
//! 这里只放**策略与预算**：上限常量、按用途选定的 [`ArchiveLimits`]、以及累计
//! 解压量的记账器 [`ExpansionBudget`]。不包含宿主压缩库的 I/O，避免为一个上限
//! 模块引入新的依赖；各解析模块（`ooxml`、表格读取）在自己的读取路径上调用这里
//! 的检查，并且必须在**分配或解压内容之前**调用。
//!
//! 工作单 04 的验收要求这些边界同时覆盖 DOCX/PPTX 与后续表格解析，因此本模块是
//! 两边的共享契约：04 负责把它接进 Office 解析，08/09 负责把它接进表格解析。

use std::io::Read;

use super::error::{LibraryError, LibraryResult};

/// 单个受支持压缩文档文件的输入大小上限（64 MiB）。
pub const MAX_ARCHIVE_INPUT_BYTES: u64 = 64 * 1024 * 1024;

/// 压缩包内单个条目声明的解压后大小上限（32 MiB）。
pub const MAX_ENTRY_DECLARED_BYTES: u64 = 32 * 1024 * 1024;

/// 单个条目允许的最高压缩比（解压后 / 压缩后）。超过即判定为压缩炸弹特征。
pub const MAX_ENTRY_COMPRESSION_RATIO: u64 = 200;

/// 压缩包内所有条目累计解压量的默认上限（128 MiB）。
pub const MAX_ARCHIVE_EXPANDED_BYTES: u64 = 128 * 1024 * 1024;

/// 单次正文提取返回的字符数上限；超出的部分丢弃并在结果中标记截断。
pub const MAX_EXTRACTED_TEXT_CHARS: usize = 2 * 1024 * 1024;

/// 表格预览一次返回的行数上限。
pub const MAX_TABLE_PREVIEW_ROWS: usize = 500;

/// 表格预览一次返回的列数上限。
pub const MAX_TABLE_PREVIEW_COLUMNS: usize = 64;

/// 表格单元格文本的字符数上限；超出部分截断并标记降级。
///
/// 表格解析（工作单 08/09）在写入单元格时使用，在此之前只有测试引用。
#[allow(dead_code)]
pub const MAX_TABLE_CELL_CHARS: usize = 4096;

/// 索引阶段允许的累计解压量。
pub const INDEX_ARCHIVE_EXPANDED_BYTES: u64 = MAX_ARCHIVE_EXPANDED_BYTES;

/// 版式预览允许的累计解压量，低于索引预算。
pub const PREVIEW_ARCHIVE_EXPANDED_BYTES: u64 = 96 * 1024 * 1024;

/// 表格分页预览允许的累计解压量；只读取有限行列，预算最小。
pub const TABLE_PREVIEW_ARCHIVE_EXPANDED_BYTES: u64 = 64 * 1024 * 1024;

/// 一次压缩文档解析的资源预算。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveLimits {
    /// 输入文件字节数上限。
    pub max_input_bytes: u64,
    /// 单个条目声明的解压大小上限。
    pub max_entry_bytes: u64,
    /// 所有条目累计解压量上限。
    pub max_expanded_bytes: u64,
    /// 单个条目允许的最高压缩比。
    pub max_compression_ratio: u64,
}

impl ArchiveLimits {
    /// 按用途构造预算；输入、单条目与压缩比上限对所有用途一致。
    pub const fn new(max_expanded_bytes: u64) -> Self {
        Self {
            max_input_bytes: MAX_ARCHIVE_INPUT_BYTES,
            max_entry_bytes: MAX_ENTRY_DECLARED_BYTES,
            max_expanded_bytes,
            max_compression_ratio: MAX_ENTRY_COMPRESSION_RATIO,
        }
    }

    /// 正文索引提取使用的预算。
    pub const fn for_index() -> Self {
        Self::new(INDEX_ARCHIVE_EXPANDED_BYTES)
    }

    /// 版式预览使用的预算。
    pub const fn for_preview() -> Self {
        Self::new(PREVIEW_ARCHIVE_EXPANDED_BYTES)
    }

    /// 表格分页预览使用的预算。
    pub const fn for_table_preview() -> Self {
        Self::new(TABLE_PREVIEW_ARCHIVE_EXPANDED_BYTES)
    }
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self::for_index()
    }
}

/// 累计解压量的记账器：每次读取条目后累加，超过预算立即失败。
#[derive(Debug, Clone)]
pub struct ExpansionBudget {
    limits: ArchiveLimits,
    expanded_bytes: u64,
}

impl ExpansionBudget {
    pub fn new(limits: ArchiveLimits) -> Self {
        Self {
            limits,
            expanded_bytes: 0,
        }
    }

    /// 本次记账使用的预算。
    pub fn limits(&self) -> ArchiveLimits {
        self.limits
    }

    /// 已累计消耗的解压字节数。
    pub fn expanded_bytes(&self) -> u64 {
        self.expanded_bytes
    }

    /// 校验输入文件大小；在打开压缩包之前调用。
    pub fn check_input_size(&self, input_bytes: u64) -> LibraryResult<()> {
        if input_bytes > self.limits.max_input_bytes {
            return Err(LibraryError::Preview(format!(
                "文档过大：{} 字节超过上限 {} 字节。",
                input_bytes, self.limits.max_input_bytes
            )));
        }
        Ok(())
    }

    /// 校验单个条目的声明解压大小。
    pub fn check_entry_size(&self, entry_name: &str, declared_bytes: u64) -> LibraryResult<()> {
        if declared_bytes > self.limits.max_entry_bytes {
            return Err(LibraryError::Preview(format!(
                "文档条目 {entry_name} 声明解压后为 {} 字节，超过单条目上限 {} 字节。",
                declared_bytes, self.limits.max_entry_bytes
            )));
        }
        Ok(())
    }

    /// 校验单个条目的压缩比；`compressed_bytes` 为 0 时视为畸形声明。
    pub fn check_compression_ratio(
        &self,
        entry_name: &str,
        compressed_bytes: u64,
        declared_bytes: u64,
    ) -> LibraryResult<()> {
        if compressed_bytes == 0 {
            if declared_bytes == 0 {
                return Ok(());
            }
            return Err(LibraryError::Preview(format!(
                "文档条目 {entry_name} 的压缩大小声明为 0，无法验证解压量，已拒绝解析。"
            )));
        }
        let ratio = declared_bytes / compressed_bytes;
        if ratio > self.limits.max_compression_ratio {
            return Err(LibraryError::Preview(format!(
                "文档条目 {entry_name} 的压缩比为 {ratio}，超过上限 {}，已拒绝解析。",
                self.limits.max_compression_ratio
            )));
        }
        Ok(())
    }

    /// 把一个已读取条目的实际字节数记入累计量，超预算即失败。
    pub fn charge(&mut self, entry_name: &str, actual_bytes: usize) -> LibraryResult<()> {
        self.expanded_bytes = self.expanded_bytes.saturating_add(actual_bytes as u64);
        if self.expanded_bytes > self.limits.max_expanded_bytes {
            return Err(LibraryError::Preview(format!(
                "文档累计解压量在读取 {entry_name} 后达到 {} 字节，超过上限 {} 字节。",
                self.expanded_bytes, self.limits.max_expanded_bytes
            )));
        }
        Ok(())
    }

    /// 在累计量已超预算时提前失败，避免继续读取后续条目。
    pub fn ensure_within_budget(&self) -> LibraryResult<()> {
        if self.expanded_bytes > self.limits.max_expanded_bytes {
            return Err(LibraryError::Preview(format!(
                "文档累计解压量 {} 字节超过上限 {} 字节。",
                self.expanded_bytes, self.limits.max_expanded_bytes
            )));
        }
        Ok(())
    }
}

/// 按条目声明的解压大小选择读取缓冲区，避免为畸形声明一次性分配巨量内存。
#[allow(dead_code)]
pub fn read_capacity(declared_bytes: u64, limit: u64) -> usize {
    let declared = declared_bytes.min(limit);
    // 多读一个字节即可区分「正好等于上限」和「实际超出上限」。
    (declared as usize).saturating_add(1)
}

/// 在预算内读取一个流式条目。
///
/// 读取上限取 `max_entry_bytes + 1`：多出的一个字节用于识别「实际解压量超过
/// 单条目上限」的畸形声明，此时返回错误且不计入累计量。`entry_name` 只用于错误
/// 信息。
#[allow(dead_code)]
pub fn read_stream_limited<R: Read>(
    reader: R,
    entry_name: &str,
    declared_bytes: u64,
    budget: &mut ExpansionBudget,
) -> LibraryResult<Vec<u8>> {
    budget.check_entry_size(entry_name, declared_bytes)?;
    let capacity = read_capacity(declared_bytes, budget.limits().max_entry_bytes);
    let mut buffer = Vec::with_capacity(capacity);
    let mut limited = reader.take(budget.limits().max_entry_bytes.saturating_add(1));
    limited.read_to_end(&mut buffer)?;
    let actual = buffer.len() as u64;
    if actual > budget.limits().max_entry_bytes {
        return Err(LibraryError::Preview(format!(
            "文档条目 {entry_name} 的实际解压量为 {actual} 字节，超过单条目上限 {} 字节。",
            budget.limits().max_entry_bytes
        )));
    }
    budget.charge(entry_name, buffer.len())?;
    Ok(buffer)
}

/// 正文提取结果：文本与是否因上限被截断。
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedText {
    pub text: String,
    pub truncated: bool,
}

/// 把任意文本按 [`MAX_EXTRACTED_TEXT_CHARS`] 截断，并报告是否发生截断。
#[allow(dead_code)]
pub fn bound_extracted_text(raw: String) -> BoundedText {
    let mut chars = raw.chars();
    let text: String = chars.by_ref().take(MAX_EXTRACTED_TEXT_CHARS).collect();
    let truncated = chars.next().is_some();
    BoundedText { text, truncated }
}

/// 把表格预览请求的行列范围收敛到上限内。
///
/// 返回 `(起始行, 行数, 列数)`；起始行保持调用方给出的值，由读取器按实际数据
/// 决定是否为空范围。
#[allow(dead_code)]
pub fn clamp_table_range(
    start_row: usize,
    row_count: usize,
    column_count: usize,
) -> (usize, usize, usize) {
    let rows = row_count.clamp(1, MAX_TABLE_PREVIEW_ROWS);
    let columns = column_count.clamp(1, MAX_TABLE_PREVIEW_COLUMNS);
    (start_row, rows, columns)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_limits() -> ArchiveLimits {
        ArchiveLimits {
            max_input_bytes: 1024,
            max_entry_bytes: 64,
            max_expanded_bytes: 100,
            max_compression_ratio: 10,
        }
    }

    #[test]
    fn rejects_input_larger_than_the_budget() {
        let budget = ExpansionBudget::new(tiny_limits());
        assert!(budget.check_input_size(1024).is_ok());
        let error = budget.check_input_size(1025).unwrap_err();
        assert!(error.to_string().contains("文档过大"));
    }

    #[test]
    fn rejects_an_entry_whose_declared_size_exceeds_the_limit() {
        let mut budget = ExpansionBudget::new(tiny_limits());
        let error = budget
            .check_entry_size("word/document.xml", 4096)
            .unwrap_err();
        assert!(error.to_string().contains("单条目上限"));
        assert_eq!(budget.expanded_bytes(), 0);
        assert!(read_stream_limited(&b"x"[..], "big.xml", 4096, &mut budget).is_err());
    }

    #[test]
    fn rejects_a_compression_bomb_ratio() {
        let budget = ExpansionBudget::new(tiny_limits());
        assert!(budget
            .check_compression_ratio("ok.xml", 100, 500)
            .is_ok());
        let error = budget
            .check_compression_ratio("bomb.xml", 10, 10_000)
            .unwrap_err();
        assert!(error.to_string().contains("压缩比"));
        assert!(budget
            .check_compression_ratio("zero.xml", 0, 10)
            .unwrap_err()
            .to_string()
            .contains("压缩大小声明为 0"));
        assert!(budget.check_compression_ratio("empty.xml", 0, 0).is_ok());
    }

    #[test]
    fn reads_an_entry_exactly_at_the_limit() {
        let mut budget = ExpansionBudget::new(tiny_limits());
        let payload = vec![b'a'; 64];
        let read = read_stream_limited(&payload[..], "ok.xml", 64, &mut budget).unwrap();
        assert_eq!(read.len(), 64);
        assert_eq!(budget.expanded_bytes(), 64);
    }

    #[test]
    fn rejects_an_entry_that_lies_about_its_size_before_full_allocation() {
        let mut budget = ExpansionBudget::new(tiny_limits());
        let payload = vec![b'a'; 4096];
        let error = read_stream_limited(&payload[..], "liar.xml", 8, &mut budget).unwrap_err();
        assert!(error.to_string().contains("实际解压量"));
        assert_eq!(budget.expanded_bytes(), 0);
    }

    #[test]
    fn stops_when_cumulative_expansion_exceeds_the_budget() {
        let mut budget = ExpansionBudget::new(tiny_limits());
        let first = vec![b'a'; 60];
        let second = vec![b'b'; 60];
        read_stream_limited(&first[..], "a.xml", 60, &mut budget).unwrap();
        let error = read_stream_limited(&second[..], "b.xml", 60, &mut budget).unwrap_err();
        assert!(error.to_string().contains("累计解压量"));
        assert_eq!(budget.expanded_bytes(), 120);
        assert!(budget.ensure_within_budget().is_err());
    }

    #[test]
    fn extracted_text_is_bounded_and_reports_truncation() {
        let short = bound_extracted_text("你好 world".to_string());
        assert_eq!(short.text, "你好 world");
        assert!(!short.truncated);

        let long = "中".repeat(MAX_EXTRACTED_TEXT_CHARS + 10);
        let bounded = bound_extracted_text(long);
        assert_eq!(bounded.text.chars().count(), MAX_EXTRACTED_TEXT_CHARS);
        assert!(bounded.truncated);
    }

    #[test]
    fn table_ranges_are_clamped_to_the_preview_limits() {
        assert_eq!(clamp_table_range(0, 50, 8), (0, 50, 8));
        assert_eq!(
            clamp_table_range(10, 100_000, 10_000),
            (10, MAX_TABLE_PREVIEW_ROWS, MAX_TABLE_PREVIEW_COLUMNS)
        );
        // 请求 0 行或 0 列时至少返回 1 行 1 列，避免生成空范围。
        assert_eq!(clamp_table_range(5, 0, 0), (5, 1, 1));
    }

    #[test]
    fn named_presets_narrow_the_expansion_budget_for_preview_work() {
        assert_eq!(
            ArchiveLimits::for_index().max_expanded_bytes,
            INDEX_ARCHIVE_EXPANDED_BYTES
        );
        assert!(
            ArchiveLimits::for_preview().max_expanded_bytes
                < ArchiveLimits::for_index().max_expanded_bytes
        );
        assert!(
            ArchiveLimits::for_table_preview().max_expanded_bytes
                < ArchiveLimits::for_preview().max_expanded_bytes
        );
        assert_eq!(
            ArchiveLimits::for_preview().max_input_bytes,
            MAX_ARCHIVE_INPUT_BYTES
        );
        assert_eq!(
            ArchiveLimits::for_index().max_compression_ratio,
            MAX_ENTRY_COMPRESSION_RATIO
        );
    }
}
