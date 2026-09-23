//! 分类规则与接收目录的资料库级持久化骨架（工作单 10/11/12/13）。
//!
//! 这里定义**存储契约与表结构**：分类规则、接收来源、来源扫描状态（含用户明确
//! 跳过的既有文件）与接收导入日志。规则与来源都属于单个资料库，随资料库目录
//! 迁移，不写入应用级状态。
//!
//! 匹配与扫描语义由 `service.rs` 调用这里的读写原语实现；本模块只负责数据。

use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use super::error::LibraryResult;
use super::models::{
    ClassificationRule, ClassificationRuleInput, ClassificationRuleOperation, ImportItemStatus,
    ReceiveImportLogEntry, ReceiveSource, ReceiveSourceInput, ReceiveSourceKind,
    ReceiveSourceStatus,
};
// 只在测试里构造「更新规则」的输入用到的类型。
#[cfg(test)]
use super::models::ClassificationRuleUpdate;

/// 建立分类规则与接收目录相关的表；由 `initialize_schema` 调用。
pub(super) fn initialize_store_schema(connection: &Connection) -> LibraryResult<()> {
    connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS classification_rules (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
            position INTEGER NOT NULL,
            file_name_pattern TEXT NOT NULL DEFAULT '',
            file_type TEXT,
            source_directory TEXT,
            collection_id TEXT REFERENCES collections(id) ON DELETE SET NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS classification_rules_position_idx
            ON classification_rules(position, id);

        CREATE TABLE IF NOT EXISTS classification_rule_tags (
            rule_id TEXT NOT NULL REFERENCES classification_rules(id) ON DELETE CASCADE,
            tag_id TEXT NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
            PRIMARY KEY (rule_id, tag_id)
        );

        CREATE TABLE IF NOT EXISTS receive_sources (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            display_name TEXT NOT NULL,
            path TEXT,
            enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
            status TEXT NOT NULL,
            status_message TEXT,
            last_scanned_at TEXT,
            first_scan_confirmed_at INTEGER,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE UNIQUE INDEX IF NOT EXISTS receive_sources_kind_path
            ON receive_sources(kind, path)
            WHERE path IS NOT NULL;

        -- 用户明确未选择的既有源文件：补扫与实时导入都不会自动导入它们。
        CREATE TABLE IF NOT EXISTS receive_source_skips (
            source_id TEXT NOT NULL REFERENCES receive_sources(id) ON DELETE CASCADE,
            source_path TEXT NOT NULL,
            skipped_at TEXT NOT NULL,
            PRIMARY KEY (source_id, source_path)
        );

        CREATE TABLE IF NOT EXISTS receive_import_log (
            id TEXT PRIMARY KEY,
            source_id TEXT NOT NULL REFERENCES receive_sources(id) ON DELETE CASCADE,
            source_path TEXT NOT NULL,
            file_name TEXT NOT NULL,
            status TEXT NOT NULL,
            document_id TEXT,
            collection_id TEXT,
            tag_ids TEXT NOT NULL DEFAULT '[]',
            matched_rule_ids TEXT NOT NULL DEFAULT '[]',
            error_message TEXT,
            created_at TEXT NOT NULL,
            item_id TEXT,
            resolved_at TEXT
        );

        CREATE INDEX IF NOT EXISTS receive_import_log_created_idx
            ON receive_import_log(created_at DESC, id DESC);
        ",
    )?;
    // 旧资料库的 receive_sources 建表时还没有「首次确认水位」列，这里补一次迁移。
    ensure_column(
        connection,
        "receive_sources",
        "first_scan_confirmed_at",
        "INTEGER",
    )?;
    // 来源变化待决项要能被界面处理：日志行需要记录待决项 id 与处理时间。
    ensure_column(connection, "receive_import_log", "item_id", "TEXT")?;
    ensure_column(connection, "receive_import_log", "resolved_at", "TEXT")?;
    Ok(())
}

/// 幂等补列：`CREATE TABLE IF NOT EXISTS` 不会给已存在的表加列。
fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    kind: &str,
) -> LibraryResult<()> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if names.iter().any(|name| name == column) {
        return Ok(());
    }
    connection.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {kind}"))?;
    Ok(())
}

/// 记录「首次确认水位」与最近扫描时间。
///
/// 水位只写一次（`COALESCE`）：它标记用户在首次清单里做完选择的那一刻，单位是 Unix 纳秒
/// （与 `modified_at` 相同）。之后的补扫只自动导入**水位之后**新增或修改过的文件；
/// 清单被截断没展示到的既有文件因此不会被静默导入。
pub(super) fn confirm_receive_source_first_scan(
    connection: &Connection,
    source_id: &str,
    timestamp: &str,
    watermark: i64,
) -> LibraryResult<()> {
    connection.execute(
        "UPDATE receive_sources
            SET last_scanned_at = ?2,
                first_scan_confirmed_at = COALESCE(first_scan_confirmed_at, ?3),
                updated_at = ?2
          WHERE id = ?1",
        params![source_id, timestamp, watermark],
    )?;
    Ok(())
}

/// 读取首次确认水位（Unix 纳秒，与 `modified_at` 同单位）；`None` 表示用户还没做完首次选择。
pub(super) fn receive_source_first_scan_watermark(
    connection: &Connection,
    source_id: &str,
) -> LibraryResult<Option<i64>> {
    let watermark: Option<Option<i64>> = connection
        .query_row(
            "SELECT first_scan_confirmed_at FROM receive_sources WHERE id = ?1",
            params![source_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(watermark.flatten())
}

/// 来源重新定位后清空扫描状态：新目录必须重新走一次「首次清单」。
pub(super) fn clear_receive_source_scan_state(
    connection: &Connection,
    source_id: &str,
) -> LibraryResult<()> {
    connection.execute(
        "UPDATE receive_sources
            SET last_scanned_at = NULL, first_scan_confirmed_at = NULL, updated_at = ?2
          WHERE id = ?1",
        params![source_id, now_timestamp()],
    )?;
    Ok(())
}

fn now_timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// 读取当前资料库的分类规则，按用户顺序返回。
pub(super) fn list_classification_rules(
    connection: &Connection,
) -> LibraryResult<Vec<ClassificationRule>> {
    let mut statement = connection.prepare(
        "SELECT id, name, enabled, position, file_name_pattern, file_type,
                source_directory, collection_id
         FROM classification_rules
         ORDER BY position ASC, id ASC",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(ClassificationRule {
            id: row.get(0)?,
            name: row.get(1)?,
            enabled: row.get::<_, i64>(2)? != 0,
            position: row.get(3)?,
            file_name_pattern: row.get(4)?,
            file_type: row.get(5)?,
            source_directory: row.get(6)?,
            collection_id: row.get(7)?,
            tag_ids: Vec::new(),
        })
    })?;

    let mut rules = Vec::new();
    for row in rows {
        let mut rule = row?;
        rule.tag_ids = rule_tag_ids(connection, &rule.id)?;
        rules.push(rule);
    }
    Ok(rules)
}

fn rule_tag_ids(connection: &Connection, rule_id: &str) -> LibraryResult<Vec<String>> {
    let mut statement = connection.prepare(
        "SELECT tag_id FROM classification_rule_tags WHERE rule_id = ?1 ORDER BY tag_id ASC",
    )?;
    let rows = statement.query_map(params![rule_id], |row| row.get::<_, String>(0))?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row?);
    }
    Ok(ids)
}

/// 应用一次规则编辑操作并返回最新规则列表。
pub(super) fn apply_classification_rule_operation(
    connection: &Connection,
    operation: &ClassificationRuleOperation,
    timestamp: &str,
) -> LibraryResult<Vec<ClassificationRule>> {
    match operation {
        ClassificationRuleOperation::Create { rule } => {
            let id = Uuid::new_v4().to_string();
            let position = next_rule_position(connection)?;
            connection.execute(
                "INSERT INTO classification_rules
                    (id, name, enabled, position, file_name_pattern, file_type,
                     source_directory, collection_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
                params![
                    id,
                    rule.name,
                    i64::from(rule.enabled),
                    position,
                    rule.file_name_pattern,
                    rule.file_type,
                    rule.source_directory,
                    rule.collection_id,
                    timestamp,
                ],
            )?;
            replace_rule_tags(connection, &id, &rule.tag_ids)?;
        }
        ClassificationRuleOperation::Update { rule } => {
            write_rule(connection, &rule.id, &rule.input, timestamp)?;
        }
        ClassificationRuleOperation::Delete { rule_id } => {
            connection.execute("DELETE FROM classification_rules WHERE id = ?1", params![rule_id])?;
        }
        ClassificationRuleOperation::SetEnabled { rule_id, enabled } => {
            connection.execute(
                "UPDATE classification_rules SET enabled = ?2, updated_at = ?3 WHERE id = ?1",
                params![rule_id, i64::from(*enabled), timestamp],
            )?;
        }
        ClassificationRuleOperation::Reorder { ordered_rule_ids } => {
            for (index, rule_id) in ordered_rule_ids.iter().enumerate() {
                connection.execute(
                    "UPDATE classification_rules SET position = ?2, updated_at = ?3 WHERE id = ?1",
                    params![rule_id, index as i64 + 1, timestamp],
                )?;
            }
        }
    }
    list_classification_rules(connection)
}

fn write_rule(
    connection: &Connection,
    rule_id: &str,
    rule: &ClassificationRuleInput,
    timestamp: &str,
) -> LibraryResult<()> {
    connection.execute(
        "UPDATE classification_rules
            SET name = ?2, enabled = ?3, file_name_pattern = ?4, file_type = ?5,
                source_directory = ?6, collection_id = ?7, updated_at = ?8
          WHERE id = ?1",
        params![
            rule_id,
            rule.name,
            i64::from(rule.enabled),
            rule.file_name_pattern,
            rule.file_type,
            rule.source_directory,
            rule.collection_id,
            timestamp,
        ],
    )?;
    replace_rule_tags(connection, rule_id, &rule.tag_ids)
}

fn replace_rule_tags(
    connection: &Connection,
    rule_id: &str,
    tag_ids: &[String],
) -> LibraryResult<()> {
    connection.execute(
        "DELETE FROM classification_rule_tags WHERE rule_id = ?1",
        params![rule_id],
    )?;
    let mut seen = std::collections::HashSet::new();
    for tag_id in tag_ids {
        if !seen.insert(tag_id) {
            continue;
        }
        connection.execute(
            "INSERT OR IGNORE INTO classification_rule_tags (rule_id, tag_id) VALUES (?1, ?2)",
            params![rule_id, tag_id],
        )?;
    }
    Ok(())
}

fn next_rule_position(connection: &Connection) -> LibraryResult<i64> {
    let next: Option<i64> =
        connection.query_row("SELECT MAX(position) FROM classification_rules", [], |row| {
            row.get(0)
        })?;
    Ok(next.unwrap_or(0) + 1)
}

/// 读取当前资料库的接收来源，按来源类型与创建时间稳定排序。
pub(super) fn list_receive_sources(connection: &Connection) -> LibraryResult<Vec<ReceiveSource>> {
    let mut statement = connection.prepare(
        "SELECT id, kind, display_name, path, enabled, status, status_message,
                last_scanned_at,
                (SELECT COUNT(*) FROM receive_import_log log
                  WHERE log.source_id = receive_sources.id
                    AND log.status = 'sourceChanged'
                    AND log.resolved_at IS NULL)
         FROM receive_sources
         ORDER BY kind ASC, created_at ASC, id ASC",
    )?;
    let rows = statement.query_map([], |row| {
        let kind: String = row.get(1)?;
        let status: String = row.get(5)?;
        Ok(ReceiveSource {
            id: row.get(0)?,
            kind: receive_kind_from_str(&kind),
            display_name: row.get(2)?,
            path: row.get(3)?,
            enabled: row.get::<_, i64>(4)? != 0,
            status: receive_status_from_str(&status),
            status_message: row.get(6)?,
            // 待处理数由扫描流程写入状态表；这里先用导入日志里的 pending 项计。
            pending_count: row.get(8)?,
            last_scanned_at: row.get(7)?,
        })
    })?;
    let mut sources = Vec::new();
    for row in rows {
        sources.push(row?);
    }
    Ok(sources)
}

/// 新增或更新一个接收来源；`source_id` 为 `None` 时新建。
pub(super) fn upsert_receive_source(
    connection: &Connection,
    source_id: Option<&str>,
    input: &ReceiveSourceInput,
    timestamp: &str,
) -> LibraryResult<Vec<ReceiveSource>> {
    let kind = receive_kind_to_str(input.kind);
    match source_id {
        Some(id) => {
            connection.execute(
                "UPDATE receive_sources
                    SET kind = ?2, display_name = ?3, path = ?4, enabled = ?5, updated_at = ?6
                  WHERE id = ?1",
                params![
                    id,
                    kind,
                    input.display_name,
                    input.path,
                    i64::from(input.enabled),
                    timestamp
                ],
            )?;
        }
        None => {
            connection.execute(
                "INSERT INTO receive_sources
                    (id, kind, display_name, path, enabled, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'ready', ?6, ?6)",
                params![
                    Uuid::new_v4().to_string(),
                    kind,
                    input.display_name,
                    input.path,
                    i64::from(input.enabled),
                    timestamp
                ],
            )?;
        }
    }
    list_receive_sources(connection)
}

/// 删除一个接收来源及其跳过记录与日志。
pub(super) fn remove_receive_source(
    connection: &Connection,
    source_id: &str,
) -> LibraryResult<Vec<ReceiveSource>> {
    connection.execute("DELETE FROM receive_sources WHERE id = ?1", params![source_id])?;
    list_receive_sources(connection)
}

/// 记录用户明确跳过的既有源文件；重复跳过一次即可。
pub(super) fn record_skipped_sources(
    connection: &Connection,
    source_id: &str,
    paths: &[String],
    timestamp: &str,
) -> LibraryResult<()> {
    for path in paths {
        connection.execute(
            "INSERT OR REPLACE INTO receive_source_skips (source_id, source_path, skipped_at)
             VALUES (?1, ?2, ?3)",
            params![source_id, path, timestamp],
        )?;
    }
    Ok(())
}

/// 该源文件是否被用户明确跳过（补扫与实时导入都会跳过它）。
pub(super) fn is_source_skipped(
    connection: &Connection,
    source_id: &str,
    path: &str,
) -> LibraryResult<bool> {
    let found: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM receive_source_skips WHERE source_id = ?1 AND source_path = ?2",
            params![source_id, path],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

/// 记录一次接收导入结果，供界面事后查看与改正。
#[allow(clippy::too_many_arguments)]
pub(super) fn record_import_log(
    connection: &Connection,
    source_id: &str,
    source_path: &str,
    file_name: &str,
    status: ImportItemStatus,
    document_id: Option<&str>,
    collection_id: Option<&str>,
    tag_ids: &[String],
    matched_rule_ids: &[String],
    error_message: Option<&str>,
    item_id: Option<&str>,
    timestamp: &str,
) -> LibraryResult<()> {
    connection.execute(
        "INSERT INTO receive_import_log
            (id, source_id, source_path, file_name, status, document_id, collection_id,
             tag_ids, matched_rule_ids, error_message, created_at, item_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            Uuid::new_v4().to_string(),
            source_id,
            source_path,
            file_name,
            import_status_to_str(status),
            document_id,
            collection_id,
            serde_json::to_string(tag_ids).unwrap_or_else(|_| "[]".to_string()),
            serde_json::to_string(matched_rule_ids).unwrap_or_else(|_| "[]".to_string()),
            error_message,
            timestamp,
            item_id,
        ],
    )?;
    Ok(())
}

/// 按导入项 id 把对应的待决日志行标记为已决（写入 `resolved_at`）。
///
/// 只匹配 `item_id` 相同且尚未结算的行：待决项也可能来自人工导入、没有日志行，
/// 那时这里一行都不改（返回 0），不会误标别的待决。行本身保留，供用户回看。
pub(super) fn resolve_import_log_by_item(
    connection: &Connection,
    item_id: &str,
    timestamp: &str,
) -> LibraryResult<usize> {
    let updated = connection.execute(
        "UPDATE receive_import_log
            SET resolved_at = ?2
          WHERE item_id = ?1 AND resolved_at IS NULL",
        params![item_id, timestamp],
    )?;
    Ok(updated)
}

/// 读取最近的接收导入日志。
pub(super) fn list_import_log(
    connection: &Connection,
    limit: usize,
) -> LibraryResult<Vec<ReceiveImportLogEntry>> {
    let mut statement = connection.prepare(
        "SELECT source_id, source_path, file_name, status, document_id, collection_id,
                tag_ids, matched_rule_ids, error_message, created_at, item_id, resolved_at
           FROM receive_import_log
          ORDER BY created_at DESC, id DESC
          LIMIT ?1",
    )?;
    let rows = statement.query_map(params![limit as i64], |row| {
        let status: String = row.get(3)?;
        let tag_ids: String = row.get(6)?;
        let matched: String = row.get(7)?;
        Ok(ReceiveImportLogEntry {
            source_id: row.get(0)?,
            source_path: row.get(1)?,
            file_name: row.get(2)?,
            status: import_status_from_str(&status),
            document_id: row.get(4)?,
            collection_id: row.get(5)?,
            tag_ids: serde_json::from_str(&tag_ids).unwrap_or_default(),
            matched_rule_ids: serde_json::from_str(&matched).unwrap_or_default(),
            error_message: row.get(8)?,
            created_at: row.get(9)?,
            // 已结算的待决项不再提供 item_id，避免界面重复处理同一条。
            item_id: match row.get::<_, Option<String>>(11)? {
                Some(_) => None,
                None => row.get(10)?,
            },
            resolved_at: row.get(11)?,
        })
    })?;
    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }
    Ok(entries)
}

fn receive_kind_to_str(kind: ReceiveSourceKind) -> &'static str {
    match kind {
        ReceiveSourceKind::Qq => "qq",
        ReceiveSourceKind::Wechat => "wechat",
        ReceiveSourceKind::Other => "other",
    }
}

fn receive_kind_from_str(value: &str) -> ReceiveSourceKind {
    match value {
        "qq" => ReceiveSourceKind::Qq,
        "wechat" => ReceiveSourceKind::Wechat,
        _ => ReceiveSourceKind::Other,
    }
}

fn receive_status_from_str(value: &str) -> ReceiveSourceStatus {
    match value {
        "ready" => ReceiveSourceStatus::Ready,
        "missing" => ReceiveSourceStatus::Missing,
        "unreadable" => ReceiveSourceStatus::Unreadable,
        _ => ReceiveSourceStatus::Unconfigured,
    }
}

fn import_status_to_str(status: ImportItemStatus) -> &'static str {
    match status {
        ImportItemStatus::Imported => "imported",
        ImportItemStatus::Duplicate => "duplicate",
        ImportItemStatus::SourceChanged => "sourceChanged",
        ImportItemStatus::Failed => "failed",
        ImportItemStatus::Ignored => "ignored",
        ImportItemStatus::Skipped => "skipped",
    }
}

fn import_status_from_str(value: &str) -> ImportItemStatus {
    match value {
        "imported" => ImportItemStatus::Imported,
        "duplicate" => ImportItemStatus::Duplicate,
        "sourceChanged" => ImportItemStatus::SourceChanged,
        "failed" => ImportItemStatus::Failed,
        "ignored" => ImportItemStatus::Ignored,
        _ => ImportItemStatus::Skipped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::service::LibraryService;
    use tempfile::tempdir;

    fn open_library() -> (tempfile::TempDir, LibraryService) {
        let root = tempdir().unwrap();
        let mut service = LibraryService::new(root.path().join("app-state")).unwrap();
        service
            .create_library(root.path().join("Library"))
            .unwrap();
        (root, service)
    }

    /// 建库实例 + 已建好的存储表，测试主体只关心存储行为。
    fn with_store<T>(f: impl FnOnce(&Connection) -> T) -> T {
        let (_root, service) = open_library();
        service.with_test_connection(|connection| {
            initialize_store_schema(connection).unwrap();
            f(connection)
        })
    }

    #[test]
    fn store_schema_is_created_with_the_library() {
        with_store(|connection| {
            assert!(list_classification_rules(connection).unwrap().is_empty());
            assert!(list_receive_sources(connection).unwrap().is_empty());
            assert!(list_import_log(connection, 10).unwrap().is_empty());
        });
    }

    #[test]
    fn rules_keep_user_order_and_merged_tag_ids() {
        let (_root, mut service) = open_library();
        // 规则标签有外键约束，先建立真实标签。
        let tag_a = service.create_tag("发票".to_string()).unwrap().id;
        let tag_b = service.create_tag("票据".to_string()).unwrap().id;
        service.with_test_connection(|connection| {
            initialize_store_schema(connection).unwrap();
            let first = apply_classification_rule_operation(
                connection,
                &ClassificationRuleOperation::Create {
                    rule: ClassificationRuleInput {
                        name: "发票".to_string(),
                        enabled: true,
                        file_name_pattern: "发票".to_string(),
                        file_type: Some("PDF".to_string()),
                        source_directory: Some("C:\\QQ".to_string()),
                        collection_id: None,
                        // 重复标签只保留一次。
                        tag_ids: vec![tag_a.clone(), tag_a.clone()],
                    },
                },
                "2026-09-23T08:00:00Z",
            )
            .unwrap();
            assert_eq!(first.len(), 1);
            assert_eq!(first[0].position, 1);
            assert_eq!(first[0].tag_ids, vec![tag_a.clone()]);

            let second = apply_classification_rule_operation(
                connection,
                &ClassificationRuleOperation::Create {
                    rule: ClassificationRuleInput {
                        name: "归档".to_string(),
                        enabled: false,
                        file_name_pattern: String::new(),
                        file_type: None,
                        source_directory: None,
                        collection_id: None,
                        tag_ids: Vec::new(),
                    },
                },
                "2026-09-23T08:00:01Z",
            )
            .unwrap();
            assert_eq!(second.len(), 2);
            assert_eq!(second[1].position, 2);
            assert!(!second[1].enabled);

            let reordered = apply_classification_rule_operation(
                connection,
                &ClassificationRuleOperation::Reorder {
                    ordered_rule_ids: vec![second[1].id.clone(), second[0].id.clone()],
                },
                "2026-09-23T08:00:02Z",
            )
            .unwrap();
            assert_eq!(reordered[0].id, second[1].id);
            assert_eq!(reordered[0].position, 1);
            assert_eq!(reordered[1].id, second[0].id);
            assert_eq!(reordered[1].position, 2);

            let disabled = apply_classification_rule_operation(
                connection,
                &ClassificationRuleOperation::SetEnabled {
                    rule_id: reordered[0].id.clone(),
                    enabled: true,
                },
                "2026-09-23T08:00:03Z",
            )
            .unwrap();
            assert!(disabled[0].enabled);

            let updated = apply_classification_rule_operation(
                connection,
                &ClassificationRuleOperation::Update {
                    rule: ClassificationRuleUpdate {
                        id: disabled[0].id.clone(),
                        input: ClassificationRuleInput {
                            name: "发票与票据".to_string(),
                            enabled: true,
                            file_name_pattern: "票据".to_string(),
                            file_type: Some("PDF".to_string()),
                            source_directory: None,
                            collection_id: None,
                            tag_ids: vec![tag_b.clone()],
                        },
                    },
                },
                "2026-09-23T08:00:04Z",
            )
            .unwrap();
            assert_eq!(updated[0].name, "发票与票据");
            assert_eq!(updated[0].tag_ids, vec![tag_b.clone()]);
            assert!(updated[0].source_directory.is_none());

            let deleted = apply_classification_rule_operation(
                connection,
                &ClassificationRuleOperation::Delete {
                    rule_id: updated[0].id.clone(),
                },
                "2026-09-23T08:00:05Z",
            )
            .unwrap();
            assert_eq!(deleted.len(), 1);
            assert_eq!(deleted[0].position, 2, "删除后不重排既有位置");
        });
    }

    #[test]
    fn skipped_sources_stay_skipped_until_the_source_is_removed() {
        with_store(|connection| {
            let sources = upsert_receive_source(
                connection,
                None,
                &ReceiveSourceInput {
                    kind: ReceiveSourceKind::Qq,
                    display_name: "QQ".to_string(),
                    path: "C:\\QQ\\Files".to_string(),
                    enabled: true,
                },
                "2026-09-23T08:00:00Z",
            )
            .unwrap();
            assert_eq!(sources.len(), 1);
            assert_eq!(sources[0].kind, ReceiveSourceKind::Qq);
            assert_eq!(sources[0].status, ReceiveSourceStatus::Ready);
            let source_id = sources[0].id.clone();

            assert!(!is_source_skipped(connection, &source_id, "C:\\QQ\\Files\\a.pdf").unwrap());
            record_skipped_sources(
                connection,
                &source_id,
                &["C:\\QQ\\Files\\a.pdf".to_string()],
                "2026-09-23T08:00:01Z",
            )
            .unwrap();
            assert!(is_source_skipped(connection, &source_id, "C:\\QQ\\Files\\a.pdf").unwrap());
            assert!(!is_source_skipped(connection, &source_id, "C:\\QQ\\Files\\b.pdf").unwrap());

            let updated = upsert_receive_source(
                connection,
                Some(&source_id),
                &ReceiveSourceInput {
                    kind: ReceiveSourceKind::Qq,
                    display_name: "QQ 文件".to_string(),
                    path: "D:\\QQ\\Files".to_string(),
                    enabled: false,
                },
                "2026-09-23T08:00:02Z",
            )
            .unwrap();
            assert_eq!(updated.len(), 1);
            assert_eq!(updated[0].display_name, "QQ 文件");
            assert!(!updated[0].enabled);

            let removed = remove_receive_source(connection, &source_id).unwrap();
            assert!(removed.is_empty());
            let remaining: i64 = connection
                .query_row("SELECT COUNT(*) FROM receive_source_skips", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(remaining, 0, "删除来源时跳过记录一并清理");
        });
    }

    #[test]
    fn import_log_round_trips_tags_status_and_library_scope() {
        with_store(|connection| {
            let sources = upsert_receive_source(
                connection,
                None,
                &ReceiveSourceInput {
                    kind: ReceiveSourceKind::Wechat,
                    display_name: "微信".to_string(),
                    path: "C:\\WeChat\\Files".to_string(),
                    enabled: true,
                },
                "2026-09-23T08:00:00Z",
            )
            .unwrap();
            let source_id = sources[0].id.clone();

            record_import_log(
                connection,
                &source_id,
                "C:\\WeChat\\Files\\b.csv",
                "b.csv",
                ImportItemStatus::Imported,
                Some("document-1"),
                Some("inbox"),
                &["tag-a".to_string()],
                &["rule-1".to_string()],
                None,
                Some("item-1"),
                "2026-09-23T08:00:01Z",
            )
            .unwrap();
            record_import_log(
                connection,
                &source_id,
                "C:\\WeChat\\Files\\c.pdf",
                "c.pdf",
                ImportItemStatus::Failed,
                None,
                None,
                &[],
                &[],
                Some("文件损坏"),
                None,
                "2026-09-23T08:00:02Z",
            )
            .unwrap();

            let log = list_import_log(connection, 10).unwrap();
            assert_eq!(log.len(), 2);
            assert_eq!(log[0].file_name, "c.pdf", "最新记录在前");
            assert_eq!(log[0].status, ImportItemStatus::Failed);
            assert_eq!(log[0].error_message.as_deref(), Some("文件损坏"));
            assert_eq!(log[1].status, ImportItemStatus::Imported);
            assert_eq!(log[1].tag_ids, vec!["tag-a".to_string()]);
            assert_eq!(log[1].matched_rule_ids, vec!["rule-1".to_string()]);

            let limited = list_import_log(connection, 1).unwrap();
            assert_eq!(limited.len(), 1);
            assert_eq!(limited[0].file_name, "c.pdf");
        });
    }

    #[test]
    fn rules_and_sources_are_scoped_to_their_own_library() {
        let first_root = tempdir().unwrap();
        let second_root = tempdir().unwrap();
        let mut first = LibraryService::new(first_root.path().join("state")).unwrap();
        first
            .create_library(first_root.path().join("Library A"))
            .unwrap();
        let mut second = LibraryService::new(second_root.path().join("state")).unwrap();
        second
            .create_library(second_root.path().join("Library B"))
            .unwrap();

        first.with_test_connection(|connection| {
            initialize_store_schema(connection).unwrap();
            apply_classification_rule_operation(
                connection,
                &ClassificationRuleOperation::Create {
                    rule: ClassificationRuleInput {
                        name: "只属于 A".to_string(),
                        enabled: true,
                        file_name_pattern: String::new(),
                        file_type: None,
                        source_directory: None,
                        collection_id: None,
                        tag_ids: Vec::new(),
                    },
                },
                "2026-09-23T08:00:00Z",
            )
            .unwrap();
        });
        second.with_test_connection(|connection| {
            initialize_store_schema(connection).unwrap();
            assert!(
                list_classification_rules(connection).unwrap().is_empty(),
                "另一个资料库不应看到 A 的规则"
            );
        });
    }
}

