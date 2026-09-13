---
status: accepted
---

# Windows 优先的 Tauri 技术栈

第一版目标是 Windows 桌面应用，采用 Tauri 2、React/TypeScript、Rust 和 SQLite。Tauri 用于降低安装包和运行资源占用并保留跨平台可能；Rust 负责本地文件操作、哈希与索引；SQLite 保存资料库数据和搜索索引。代价是技术栈更复杂，并需要维护 Rust 代码。
