# 第二版代码审查记录（2026-09-23）

审查范围：`git diff dcc5cd8..e9415c3`（第二版全部改动），只读审查，发现的问题按严重度分组。
审查者：独立代码审查子代理；H1 用仓库外最小 crate（真实 quick-xml 0.42）做了实测复现。

## 高（已派修，见 task-14）

### H1 XLSX 稀疏列引用让索引阶段吃 GB 级内存并 abort（可复现崩溃循环）

- 位置：`src-tauri/src/library/table.rs` 的 `SheetParser::finish_cell`（按列号补空字符串）、`cell_reference_column`（缺 Excel XFD=16383 上界）、`extract_table_text`（把 `columns` 传成 `usize::MAX`）。
- 实测放大倍率（真实 quick-xml，参数与生产一致）：

| 列引用 | 行数 | XML 大小 | 保留槽位 | `Vec<String>` 占用 |
| --- | --- | --- | --- | --- |
| `XFD`（Excel 真实最大列） | 400 | 21,134 B | 6,553,600 | 150.0 MB |
| `ZZZZ` | 50 | 2,732 B | 23,762,700 | 600.0 MB |
| `ZZZZZ` | 3 | 303 B | 37,069,890 | 1,152.0 MB |

- 崩溃链：导入校验不解析单元格 → 文档导入成功 → 打开资料库自动索引 → 分配失败 abort → 文档仍 `pending` → 每次打开资料库都再崩一次。预览路径不受影响（列数被 `clamp_table_range` 收在 64 以内）。

### H2 首次接收清单截断 500 条，补扫按 4000 条自动导入 → 静默导入用户没看到的文件

- 位置：`service.rs` 的 `MAX_RECEIVE_LISTING_ITEMS = 500` 与 `.take(500)`、扫描上限 `500*8`、`receive_pending_files` 只排除 skips 与有日志的文件；前端只在清单内记录跳过，且 `skipReceiveDirectoryFiles` 会置位 `last_scanned_at` 使「首次确认前不导入」闸门失效。
- 影响：500+ 文件的接收目录里，第 501..4000 个文件既未展示也未记入 skips，周期补扫会把它们全部自动导入，违反 PRD 19/36 与冻结契约。
- 覆盖缺口：既有 `receive_sources.rs` 的 helper 传的是目录全量而非截断清单，因此掩盖了这个问题。

## 中（已记录，未派单）

- **M1 CSV 导入校验无输入上限**：`validate_file_content` 的预检不覆盖 `CsvText`，`validate_table_file` 直接 `fs::read` 整份文件，`decode_csv_text` 再复制一份 String，峰值约为文件大小 2 倍。（注：`861f7a2` 已把 `CsvText` 纳入预检，此项的**主要部分已闭合**；剩余的「二次整体复制」仍存在。）
- **M2 CSV/XLSX 索引期解析产物不在预算内**：字符级截断发生在解析之后，`Vec<Vec<String>>` 的单元格内存不记账；64 MiB 的 `a,a,a,…` CSV 可产出约 3200 万单元格。
- **M3 接收来源变化待决项在界面上无路可走**：日志没有 `item_id`，前端无法调用 `resolveImportItem`，`pendingCount` 永不清零，用户无法按 PRD 25/34 选择「新建/替换」。
- **M4 中断恢复对单个文件操作失败是致命的**：`documents/<id>/` 下一个被占用的残留就让 `open_library` 整体失败，而不是「那一份文档恢复失败、其余照常」；同文件已有 `RecoveryFailure` 机制但只有哈希不匹配会走到。

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
