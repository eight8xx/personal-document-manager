# 问题追踪器：本地 Markdown

本仓库的需求、PRD 和实现任务使用 Markdown 文件保存在 `.scratch/`。

## 约定

- 每个功能使用一个目录：`.scratch/<feature-slug>/`
- PRD：`.scratch/<feature-slug>/PRD.md`
- 实现任务：`.scratch/<feature-slug>/issues/<NN>-<slug>.md`，从 `01` 开始编号
- 每个任务文件顶部使用 `Status:` 记录分类状态
- 讨论历史追加到文件底部的 `## Comments` 下

## 技能要求“发布到问题追踪器”时

创建 `.scratch/<feature-slug>/` 下的对应 Markdown 文件；目录不存在时先创建。

## 技能要求“获取相关任务”时

读取对应路径的文件。用户通常会直接提供路径或任务编号。
