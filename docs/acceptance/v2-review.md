# 第二版代码审查记录（2026-09-23）

审查范围：`git diff dcc5cd8..e9415c3`（第二版全部改动），只读审查，发现的问题按严重度分组。
审查者：独立代码审查子代理；H1 用仓库外最小 crate（真实 quick-xml 0.42）做了实测复现。
**后续状态**：H1、H2 已由 `b8c5268` 修复并经 Lead 独立复现验证；M1 的主要部分已由 `861f7a2` 闭合；其余条目为待办。

## 高（已修复）

### H1 XLSX 稀疏列引用让索引路径按列号放大工作量

- 位置：`src-tauri/src/library/table.rs` 的 `SheetParser::finish_cell`（`while row_cells.len() < cell.column` 逐列补空字符串）、`cell_reference_column`（缺 Excel XFD=16383 上界）、`extract_table_text`（把 `columns` 传成 `usize::MAX`，导致补空循环无界）。
- **Lead 独立实测（修复前的 `codex/v2-development`）**：单工作表、行内只有一个远列单元格、XML 仅 1.5～7.5 KB 时，`extract_table_text` 的耗时随列索引放大：

| 列引用（列索引） | 行数 | XML | 索引提取耗时 |
| --- | --- | --- | --- |
| `XFD`（16,383，Excel 真实最大列） | 400 | 7,534 B | 0.14 s |
| `ZZZZ`（475,253） | 50 | 2,289 B | 0.82 s |
| `ZZZZZ`（12,356,630） | 3 | 1,597 B | **3.59 s** |

- **与最初审查结论的差异**：审查报告称会「按列号保留 GB 级内存」，Lead 复核**未观察到**该现象——预览路径的列数被 `clamp_table_range` 收住（3 行样本只保留 24 个槽位），索引路径的结果文本也一直很小。真实机制是**逐列补空字符串的无界工作量**（每个空 `String` 24 字节栈内数据 + 分配/回收开销），列号越远越慢，足以让一次索引长时间占住服务锁、放大小样本的破坏力。
- 崩溃链（审查推断，未实测到 abort）：导入校验不解析单元格 → 导入成功 → 打开资料库自动索引 → 长时间/失败 → 文档仍 `pending` → 每次打开资料库重跑。
- **修复（`b8c5268`）**：`row_cells` 改为稀疏的 `Vec<(usize, String)>`，删除补空循环；`cell_reference_column` 拒绝 >16383；索引路径改为「只取值不补位」并加 `MAX_TABLE_INDEX_CELLS = 200_000` 预算（CSV 索引另用 `MAX_TABLE_INDEX_COLUMNS = 256`）。修复后 Lead 的独立复现用例从 3.59 s → **0.01 s**。

### H2 首次接收清单截断 500 条，补扫按 4000 条自动导入 → 静默导入用户没看到的文件

- 位置：`service.rs` 的 `MAX_RECEIVE_LISTING_ITEMS = 500` 与 `.take(500)`、扫描上限 `500*8`、`receive_pending_files` 只排除 skips 与有日志的文件；前端只在清单内记录跳过，且 `skipReceiveDirectoryFiles` 会置位 `last_scanned_at` 使「首次确认前不导入」闸门失效。
- 影响：500+ 文件的接收目录里，第 501..4000 个文件既未展示也未记入 skips，周期补扫会把它们全部自动导入，违反 PRD 19/36 与冻结契约。
- **修复（`b8c5268`）**：新增 `receive_sources.first_scan_confirmed_at` 作为明确的「已确认」闸门（含旧库幂等补列），并新增 `begin_receive_selection_batch`——用户完成选择时把目录内**所有未勾选的受支持文件**在一个事务里记为明确跳过；补扫路径不写跳过记录。新增用例按界面真实顺序在 520 个文件上断言补扫零导入、只留勾选的一份、之后新到的文件照常自动导入。
- 实现者还纠正了 Lead 建议的「纯 mtime 水位」方案：实测会让「确认前写入临时名、确认后改名」的合法新文件永远不被导入（改名不改 mtime），既有用例当场变红。

## 中（待办）

- **M1 CSV 导入校验的内存峰值**：`validate_file_content` 的预检已由 `861f7a2` 纳入 `CsvText`（超限在导入阶段即拒绝），但 `validate_table_file` 仍对允许范围内的文件整份 `fs::read`，`decode_csv_text` 再复制一份 `String`，峰值约为文件大小的 2 倍。**部分闭合**。
- **M2 CSV 索引期解析产物不在预算内**：`861f7a2`/`b8c5268` 已让 XLSX/CSV 索引路径共享单元格预算，但 `parse_csv_rows(&text, None, usize::MAX)` 的整表物化与 `bound_extracted_text` 的截断时机仍需复核。**部分闭合**。
- **M3 接收来源变化待决项在界面上无路可走**：日志没有 `item_id`，前端无法调用 `resolveImportItem`，`pendingCount` 永不清零，用户无法按 PRD 25/34 选择「新建/替换」。**未修**。
- **M4 中断恢复对单个文件操作失败是致命的**：`documents/<id>/` 下一个被占用的残留就让 `open_library` 整体失败，而不是「那一份文档恢复失败、其余照常」；同文件已有 `RecoveryFailure` 机制但只有哈希不匹配会走到。**未修**。

## 低（已记录）

- **L1** 接收目录监视线程 Drop 时不等同「立刻退出」：`join` 会等到本轮补扫结束。
- **L2** 中断在「副本已落盘、数据库未写入」之间会留下永不清理的孤儿副本目录。
- **L3** XLSX 稀疏表翻页固定 ±50 行且每页重读整文件；数据在很靠后的行时体验差（建议后端返回「下一个含数据的行」）。
- **L4** 固定压缩比阈值 200 对内容分布敏感，有误拒合法文档的风险（无样本证实）。

## 已检查、确认没问题的方向

1. 契约一致性：全部新类型的字段名与 camelCase、12 个命令名与参数名在 `client.ts` 与 `commands.rs`/`lib.rs` 之间逐条对得上；internally-tagged 判别联合实测 round-trip 一致。
2. 并发正确性：分步导入在锁内先核库身份再写库，切库以 `invalidLibrary` 中止且新库零写入；批次状态在 finish/abort 两条路径都清理；同批两线程不重复处理同一项；进度事件序列与单次调用逐字一致。
3. 替换/永久删除的崩溃恢复：6 个中断点逐推演可覆盖，内容不匹配时保留现场；源文件与副本从不原地修改。
4. 资源上限（解压前校验部分）：输入/单条目声明/压缩比在解压前校验，解压经限流读取并累计记账，ZIP 范围有溢出校验，拒绝 ZIP64/加密/不支持压缩方式。
5. 安全预览：XLSX 拒绝 XLSM/XLSB/加密/ODS；公式只取缓存值不求值；外部关系被拦截且外部部件从不打开；预览载荷经 React 默认转义，无注入面。
6. 接收目录其余语义：只处理当前库已启用来源、无常驻进程、目录失效不静默换目录、跳过临时/聊天库/链接/重解析点、拒绝资料库自身、事件按库身份过滤。
7. panic 面：`table.rs` 的字节/字符串操作均走 `get()`/`checked_*`，UTF-8 只按字符处理；未发现可 panic 的输入。

## 次要观察（非缺陷）

- `limits.rs` 若干 helper 挂着 `#[allow(dead_code)]` 但已被使用，历史标注可清理。
- `store.rs` 的 `pending_count` 统计 `status='pending'`（写入端从不写这个值），随后总被 service 层覆盖，语义不一致。
- `fakeClient.ts` 的接收语义与真实后端有差异（`pendingCount` 自增、不置 `lastScannedAt`、清单不截断），相关前端断言只验证渲染。
- 缺少「前端 JSON → Rust 反序列化」的 JSON 级契约测试（审查者已实测形状正确，建议补测试防回归）。
- `SettingsDialog` 的页签缺 `aria-controls` 与方向键支持（可访问性完善项）。
