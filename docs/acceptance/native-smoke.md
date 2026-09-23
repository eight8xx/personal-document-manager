# Windows 原生窗口冒烟测试

运行 `npm run test:native-smoke`。脚本先以 `src-tauri/tauri.smoke.conf.json` 构建独立的 Tauri debug 程序，然后由 `tauri-driver` 在真实 Windows 窗口中执行操作。测试直接使用原生 Tauri IPC 和真实资料库，不使用前端假客户端。Windows 原生文件选择器与向导、导入按钮不在此冒烟覆盖范围内；脚本通过窗口内的真实 Tauri IPC 命令建立隔离资料库并导入样本，再驱动其余 UI 流程。

## 一次性准备

需要项目的 Node.js、Rust/MSVC 构建环境、Microsoft Edge 和 WebView2。驱动版本固定为 `tauri-driver 2.0.6`：

```powershell
cargo install tauri-driver --version 2.0.6 --locked
```

也可以把它安装到项目的忽略目录：

```powershell
cargo install tauri-driver --version 2.0.6 --locked --root .scratch/.tmp/native-smoke-tools
```

从 [Microsoft Edge WebDriver](https://developer.microsoft.com/en-us/microsoft-edge/tools/webdriver/) 下载与当前 Edge **完整版本号相同**的 Windows x64 驱动，将 `msedgedriver.exe` 放入 `.scratch/.tmp/native-smoke-tools/`，或设置 `MSEDGEDRIVER_PATH` 为它的完整路径。脚本会核对两个驱动的版本，缺失或不匹配时给出失败原因。`TAURI_DRIVER_PATH` 可指定另一个通过 Cargo 安装的 `tauri-driver.exe`。

## 执行内容和证据

测试需要已登录的 Windows 桌面。它创建独立的 `%TEMP%\pdm-native-smoke-*` 目录，并覆盖子进程的 `%APPDATA%`、`%LOCALAPPDATA%` 与 WebView2 用户数据目录。每次构建使用随机的专用应用标识，避免与日常应用的单实例进程或最近使用资料库冲突。Windows 已知文件夹 API 可能不遵守这些环境变量，但随机应用标识仍把测试状态与日常应用分开；因此受限沙箱若禁止访问用户 AppData，原生应用会以“拒绝访问”失败，需要在普通交互式 PowerShell 中运行。测试结束时仅终止由测试启动的驱动进程与本次随机命名的测试程序。

冒烟流程：用真实 Tauri IPC 创建临时资料库并导入 TXT、在真实窗口创建集合并移动文档、搜索样本文字、查看预览、移入回收站、恢复，再次确认预览可读。运行报告中的 `nativeFilePickerCovered`、`libraryWizardCovered`、`importButtonCovered` 都为 `false`，不可据此签收这些交互。

每次运行保留 `report.json`（每步结果、环境、构建类型和覆盖边界）与 `tauri-driver.log`；成功时保存 `success.png`，失败时保存 `failure.png`、`failure.html`（若 WebDriver 会话仍可用）。测试进程清理失败会使整次运行返回失败，并在报告中记录 `cleanupError`。控制台最后会打印证据目录。测试不会删除该目录，便于定位失败。这个冒烟只覆盖一份 TXT 和 debug 窗口；原生文件选择器、release、复杂文档、万份数据和长期内存仍需单独验收。

当前机器上，`tauri-driver` 能启动真实 Tauri 窗口。WebDriver 点击路径选择按钮后，原生对话框未出现在 Windows UI Automation 的顶层窗口中；而 Tauri 把 `__TAURI_INTERNALS__.invoke` 定义为不可写、不可配置，无法在测试中只替换路径选择调用。因此此测试改用真实 IPC 初始化资料库和样本；文件选择器、首次向导与导入按钮仍需独立人工或桌面 UI 自动化验收。

## 当前运行结果

2026-09-23 在 Windows、Edge/WebView2 `153.0.4234.48`、`tauri-driver 2.0.6`、debug 构建下运行。真实窗口与 IPC、集合移动、正文搜索、TXT 预览、回收站与恢复的测试步骤通过；结束后未发现残留的测试应用和驱动进程。`start_import` 使用当前资料库摘要的新契约通过验证。该结果只覆盖上述子集，**不代表议题 07 全部验收完成**。本次记录位于 `%TEMP%\pdm-native-smoke-3dbf618c\report.json`，最终窗口截图为同目录的 `success.png`。
