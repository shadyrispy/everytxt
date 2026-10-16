# search-tool GUI + OCR 冒烟验收报告

日期：2026-10-16 · 构建：debug + `--features custom-protocol` · 状态：**GUI 冒烟 15/15 通过，质量门禁全绿**

## 1. 结论

| 验收项 | 结果 |
|---|---|
| GUI 冒烟（CDP 驱动，15 项断言） | **15/15 通过**，5 张截图取证 |
| `cargo test --workspace` | **236 passed / 0 failed**，exit 0（19m42s 含 504 crate 全量重建） |
| `cargo test -p stcore --test ocr_e2e -- --ignored` | 3/3 通过（13.77s，含 8×3 并发无临时文件碰撞） |
| `cargo fmt --all --check` | clean |

## 2. 本轮破解的三个根因（全部证据级）

### 2.1 wry 0.57 不读环境变量 → 代码桥接 browser args

- 实锤：wry-0.57.0/src/webview2/mod.rs:300 `create_environment` 只读
  `pl_attrs.additional_browser_args.unwrap_or_else(...)`，**无 env var 读取路径**；
  cmdwatch.ps1 抓 WebView2 子进程命令行双证 → 此前全部 flag 实验作废的根因。
- 修复：`app/src/lib.rs` setup() 代码建窗（conf 不再声明 windows），经
  `WebviewWindowBuilder::additional_browser_args()` 桥接
  `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`（复刻 wry 默认 args + 追加 env 值）；
  marker flag 实证到达 browser 命令行。
- 坑：`WebviewWindowBuilder::new` 需第三参 `tauri::WebviewUrl::App("index.html".into())`（E0061）。

### 2.2 dev 构建纯黑屏 → custom-protocol 空资产

- `generate_context!` 嵌入条件 `dev = cfg!(not(feature="custom-protocol"))`；dev + devUrl
  且未启用该 feature → 嵌入**空资产** → `tauri://localhost` 404 空响应 + wry 透明黑背景
  = 纯黑屏。必须 `cargo build -p search-tool --features custom-protocol`（custom-protocol
  不在 default features，已在 app/Cargo.toml 注释修正）。

### 2.3 WebView2 GPU 进程 FATAL → `--no-sandbox`

- dump 铁证 `GPU process crashed too many times with software GL`；
  `--disable-gpu` 只把软件 GL 崩溃循环从秒杀拖到 ~30-45s，最终仍
  `GPU process isn't usable. Goodbye.`（FATAL）。
- **`--no-sandbox` 使 GPU 崩溃变为非致命**：browser/renderer/CDP 全部存活，
  GPU 进程仍崩溃循环（47 次/30s）但不再判死。
- 附带坑：PowerShell 会话结束时的 job 对象清理会杀 browser 孙进程
  （app 因 Start-Process 脱离而幸存，无转储静默死亡）→ GUI 冒烟必须收敛在
  同一个 PowerShell 会话内完成。

## 3. 冒烟环境

| 项 | 值 |
|---|---|
| 二进制 | `target/debug/search-tool.exe`（debug + custom-protocol） |
| 启动 flags | `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--no-sandbox --remote-debugging-port=9222"`（经 lib.rs 桥接） |
| 驱动 | `target/spawnprobe/cdp-smoke.js`（node + ws，CDP page target = 点分 `http://tauri.localhost/`） |
| React 交互 | 受控输入 = native value setter + `input` 事件；`HTMLElement.click()` 触发 onClick |
| 截图 | CDP `Page.captureScreenshot`（Add-Type/COM 被安全策略拦截，屏幕捕获不可用） |

**冒烟语料**（`target/spawnprobe/smoke-corpus/`，4 文件）：

| 文件 | 大小 | 文本来源 |
|---|---|---|
| readme.txt | 98 B | 纯文本（含"量子计算"） |
| clean.png | 48 KB | 干净图片，OCR 文本含"量子计算"与"42.5%" |
| scanned.pdf | 46.7 KB | 扫描 PDF 无文字层 → OCR（4 块，含"42.5%"） |
| scanned.png | 340 KB | 扫描图片 → OCR（4 块，含"42.5%"） |

**索引**：`target/spawnprobe/smoke-index/` 以**绝对路径** + `SEARCH_TOOL_OCR=1` 重建
（3 个 tantivy 段 + search-tool.db），修复此前相对路径导致预览"加载失败 os error 3"。

## 4. 15 项检查明细

| # | 检查 | 断言 | 结果 | 证据 |
|---|---|---|---|---|
| 1 | initial-render | React 挂载、欢迎页渲染 | ✅ | gui-cdp-1 |
| 2-5 | search-quantum | 搜"量子计算" 4 行 = readme.txt + clean.png + scanned.png + scanned.pdf | ✅×4 | gui-cdp-2 |
| 6-9 | search-ocr-only | 搜"42.5%" 3 行 = clean.png + scanned.png + scanned.pdf（**全部来自 OCR 文本**，纯文本源不命中） | ✅×4 | gui-cdp-3 |
| 10-13 | preview | 点 scanned.pdf 行 → 预览打开、无加载错误、有内容 | ✅×4 | gui-cdp-4 |
| 14-15 | pdf-page-view | 切「页视图」→ canvas 页位图 + OCR bbox 命中框（hitBoxes=1，"本页命中 1 处"） | ✅×2 | gui-cdp-5 |

底部 tab label 是「**页视图**」非「预览视图」（截图误读导致过一轮假失败，选择器改
`trim() === '页视图'` 后实锤）。

## 5. 质量门禁明细（`cargo test --workspace`，19m42s）

| 测试二进制 | passed | ignored |
|---|---|---|
| stcore lib 单测 | 166 | 3 |
| app lib 单测（search_tool_lib） | 20 | 0 |
| app bin 单测（main.rs） | 6 | 0 |
| p0plus_search | 9 | 0 |
| p1_formats | 6 | 0 |
| p3_pinyin_synonym | 7 | 0 |
| r2_engine | 5 | 0 |
| r3_search | 8 | 0 |
| r4_formats | 6 | 0 |
| r5_service | 3 | 0 |
| ocr_e2e / p3_perf / bench_time_filter / p5_ocr_boxes | 0 | 8（OCR 重用例 3 个已单独跑通 3/3；其余为性能/基准，默认忽略） |
| **合计** | **236** | 11 |

说明：本轮以 `CARGO_INCREMENTAL=0` 跑门禁（规避沙箱对 target/debug/incremental
`\\?\` 扩展路径写入的 os error 5 干扰），触发 504 crate 全量重建——相当于一次
干净环境下的完整验证。

## 6. 代码改动清单（本轮）

| 文件 | 改动 |
|---|---|
| `app/src/lib.rs` | setup() 代码建窗 + `webview_browser_args()` 桥接函数（未设 env var 时行为零变化） |
| `app/tauri.conf.json` | `"windows": [...]` → `"windows": []` |
| `app/Cargo.toml` | 修正 features 注释：custom-protocol 不在 default，必须显式启用 |

## 7. 复现路径

```bash
# 构建（必须带 custom-protocol，否则黑屏）
cargo build -p search-tool --features custom-protocol
# 重建 OCR 索引（绝对路径语料 + OCR 通道）
SEARCH_TOOL_OCR=1 SEARCH_TOOL_OCR_HELPER=<ocr-helper exe> \
  ./target/debug/search-tool.exe cli <corpus> --index-dir <abs-index>
# 启动（bridge 透传 flags）
WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--no-sandbox --remote-debugging-port=9222" \
  ./target/debug/search-tool.exe
# 驱动（同一 PowerShell 会话内）
node target/spawnprobe/cdp-smoke.js
```

## 8. 下一步

1. 大规模回归：>10 万文档复测分词命中率（spike 残留项）
2. OCR 精度基准化：以 smoke-corpus 4 文件为固定基准，跟踪 helper 升级前后的置信度/bbox 漂移
3. PaddleOCR 3.x `use_doc_unwarping=True`（UVDoc）致 bbox 偏移问题已列修复候选，待验证后纳入
