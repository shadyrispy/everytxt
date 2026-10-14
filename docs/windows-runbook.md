# Windows 构建与 UI 测试 Runbook

> 目标读者：在 Windows 机器上构建、运行、冒烟测试 search-tool 的开发者。
> 配套结论：`docs/cross-platform-assessment.md`（跨平台评估与证据矩阵）。
> 本文所有步骤均在 Windows 11 + PowerShell 验证过路径逻辑；未在沙箱实跑
> （沙箱为 Linux），真机执行时如遇偏差请按 §9 反馈修正。

## 1. 前置依赖（一次性安装）

| 组件 | 要求 | 获取方式 | 说明 |
|---|---|---|---|
| Rust 工具链 | stable，**默认 host 选 `x86_64-pc-windows-msvc`** | https://rustup.rs | Tauri 官方推荐 MSVC；rustup 装完默认即 MSVC |
| VS Build Tools 2022 | 「使用 C++ 的桌面开发」工作负载（含 Windows SDK） | https://visualstudio.microsoft.com/visual-cpp-build-tools/ | unrar/chmlib 的 C/C++ 编译与链接必需；**不要**只装 VS Code |
| Node.js | 22 LTS | https://nodejs.org | 前端构建 |
| pnpm | 10.x（`corepack enable pnpm`） | 随 Node | 仓库使用 pnpm |
| WebView2 运行时 | 常青版 | Win11 自带；Win10: https://developer.microsoft.com/microsoft-edge/webview2/ | Tauri 窗口内核 |
| Git | 任意 | https://git-scm.com | 克隆代码 |

> 验证安装：`rustup show`（Default host 应为 `x86_64-pc-windows-msvc`）、
> `node -v`、`pnpm -v`。

## 2. 获取代码

```powershell
git clone <repo-url> search-tool
cd search-tool
```

仓库含 vendored 二进制（pdfium .so/.dll、OCR 模型等，约 23MB），无需额外下载。

## 3. 安装依赖

```powershell
# 前端
cd ui
pnpm install
cd ..

# Rust 首次构建会自动拉取并编译全部依赖（含 C/C++，约 5-15 分钟）
```

## 4. 构建并运行 UI（两条路径，任选）

### 路径 A：`cargo tauri dev`（推荐，带热重载）

```powershell
cargo install tauri-cli --version "^2"   # 首次
cd app
cargo tauri dev
```

自动完成：启动 Vite dev server（:1420）→ 编译 Rust → 开窗。
改 UI 代码热更新；改 Rust 代码自动重编。

### 路径 B：`cargo run`（无 tauri-cli）

```powershell
cd ui
pnpm build            # 先产出 dist（tauri.conf.json 的 frontendDist 指向它）
cd ../app
cargo run
```

## 5. PDF 预览支持（开箱即用）

- `third_party/pdfium/lib/pdfium.dll`（Chromium 157.0.8100.0）已随仓库提供：
  - **dev 模式**：stcore 按「源码树 third_party/pdfium/lib」回退加载，无需任何操作；
  - **打包模式**：经 `tauri.conf.json` resources glob 映射进 bundle 的 `lib/`。
- 如需指定其他版本：设 `SEARCH_TOOL_PDFIUM_LIB=<pdfium.dll 全路径>`。
- 若 dll 缺失/损坏：PDF 提取降级 pdf-extract（仅西文文本，无 CID 中文/无页渲染），
  其余格式不受影响。

## 6. UI 冒烟清单（真机验证要点）

启动成功后按序过以下场景，任何一项行为与预期不符即记录反馈：

| # | 场景 | 预期 |
|---|---|---|
| 1 | 窗口/菜单/托盘/热键 | 主窗口 1200×760 可缩放；托盘图标存在；13 组全局热键可触发（设置页可见） |
| 2 | 建立索引 | 索引管理页选目录（建议先用一个小目录，如 `C:\Users\<你>\Documents` 子集）→ 开始；进度/暂停/取消三态正常 |
| 3 | 搜索 | 关键词 fuzzy/exact/regex/dsl 四种模式均有结果；结果列表分页滚动正常 |
| 4 | 过滤 | 目录前缀（`/d`）、扩展名（ext）、时间范围过滤生效 |
| 5 | 预览 | txt/md 直接预览；docx/xlsx/pptx 高保真视图；**PDF 中文文本可选中**（验证 pdfium.dll 生效）； Minimap 滚动同步 |
| 6 | 文件操作 | 打开/复制/移动/删除（有确认护栏）；删除后结果列表同步移除 |
| 7 | 压缩包 | zip/7z/rar/chm 内文件可索引可预览（rar 验证 vendored unrar；chm 验证 vendored chmlib） |
| 8 | 右击集成 | 设置页开启后，资源管理器目录/目录空白处右键出现「用 search-tool 搜索」 |
| 9 | CLI 回退 | 未开 GUI 时 `app\target\debug\search-tool.exe /s 关键词` 应打印"主实例未在运行…回退本地搜索"并正常出结果（**这是 Windows 预期行为**：单实例 socket 仅 Unix 实现，见评估报告 §4） |
| 10 | 索引位置 | 默认落在 `%LOCALAPPDATA%\search-tool\index`（可用 `SEARCH_TOOL_INDEX_DIR` 覆盖） |

## 7. OCR（可选，默认关）

扫描件 OCR 为可选通道，默认关闭；不开启时扫描 PDF/图片记 `Unsupported` 跳过，不影响其他格式。

```powershell
# 开启需要两步：
# 1) 构建 helper（需 rustc ≥ 1.95，独立工具链）
cd ocr-helper
rustup toolchain install 1.95.0   # 或按 ocr-helper/rust-toolchain.toml
cargo build --release
cd ..

# 2) 设置环境变量
$env:SEARCH_TOOL_OCR = "1"        # 启用开关
# helper 自动定位顺序：SEARCH_TOOL_OCR_HELPER 环境变量 → exe 同级 → 源码树
# ocr-helper/target/release/ocr-helper.exe（路径③已验证可自动命中）
```

OCR 模型已随仓库位于 `third_party/ocr-models/`（6MB）。

## 8. 打包分发（可选）

```powershell
cd app
cargo tauri build --bundles nsis    # 或 msi（需 WiX Toolset，单独安装）
```

产物在 `app/target/release/bundle/`。pdfium.dll 与 OCR 模型会随 resources
一并打包（`lib/` 下）。

## 9. 故障排查

| 现象 | 原因 | 处理 |
|---|---|---|
| `error: linker 'link.exe' not found` | 未装 VS Build Tools C++ 工作负载 | 装 workload 后重开终端 |
| `SetSuspendState was not declared` / `ffs undefined` | 用了 **gnu** 工具链且未应用 vendored 修复 | 本仓库已 vendored 修复；如手动改过 `[patch.crates-io]` 请恢复；更建议直接用默认 MSVC |
| 窗口白屏 | `ui/dist` 不存在（路径 B）或 Vite 未起来（路径 A） | 路径 B 先 `pnpm build`；路径 A 看 1420 端口 |
| PDF 中文乱码/无文本 | pdfium.dll 未加载 | 确认 `third_party/pdfium/lib/pdfium.dll` 存在；或设 `SEARCH_TOOL_PDFIUM_LIB` |
| 右键菜单不出现 | 未在设置页开启，或注册表写入失败 | 开启后重启资源管理器；手动验证 `reg query "HKCU\Software\Classes\Directory\shell\SearchTool"` |
| OCR 无效果 | 未设 `SEARCH_TOOL_OCR=1` 或 helper 未构建 | 按 §7 两步走 |

## 10. 与 Linux/macOS 的差异备忘

| 项 | Windows | Linux/macOS |
|---|---|---|
| 数据目录 | `%LOCALAPPDATA%\search-tool\index` | `~/.local/share/search-tool/index` |
| 单实例转发 | 不支持（CLI 自动回退） | Unix socket 转发到主实例 |
| 右击集成 | 注册表 HKCU | nautilus 脚本 / .desktop |
| 托盘/热键/自启动 | 插件 Windows 分支 | 插件对应分支 |
