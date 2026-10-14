# P3 收尾批 + P2/P3 代码审计 · 交付报告

> 批次：P3 四线并行（A OCR 通道准备 / B PDF 页视图 / C 拼音+同义词 / D 性能基准）
> 随后按指令对 **P2（L 批）与 P3 全部代码**做一轮审计并修复
> 完成时间：2026-10-14

---

## 1. 结论

**P3 四线全部落地；P2+P3 审计发现 4 项缺陷（全部已修）+ 2 项登记；工作区全绿。**

| 质量门禁 | 结果 |
|---|---|
| `cargo fmt --all --check` | ✅ clean |
| `cargo clippy --workspace --all-targets` | ✅ **0 warning** |
| `cargo test --workspace` | ✅ **178 passed / 0 failed**（+22） |
| `cargo test --release -p stcore --test p3_perf -- --ignored` | ✅ 3/3（见 §4） |
| `cd ui && pnpm build` | ✅ 286.6 KB JS / 24.2 KB CSS |
| `ocr-helper`（rustc 1.95 独立 workspace） | ✅ release 编译通过（2m39s） |
| headless 冒烟（拼音/同义词/schema 提示） | ✅ 全部符合预期 |

---

## 2. P3 四线交付

### P3-A · OCR 通道（独立 helper，工具链隔离）

| 项 | 内容 |
|---|---|
| 架构决策 | **独立进程**：`ocr-helper` 自带 `[workspace]` 脱离父 workspace + `rust-toolchain.toml` pin **rustc 1.95**（`oar-ocr 0.9.1` 硬性要求，主工程 stable 1.93 无法编译） |
| 文件 | `ocr-helper/{Cargo.toml, rust-toolchain.toml, src/main.rs, README.md}` |
| 协议 | stdio 一行 JSON：`{"image_path": ...}` → `{"ok":true,"lines":[{"text","confidence"}]}` |
| 模型 | `pp-ocrv6_tiny_{det,rec}.onnx` + dict，`auto-download`（ModelScope → `$OAR_HOME`） |
| 验证 | 1.95 下 `oar-ocr 0.9.1` 编译通过；`ocr-helper` release 构建成功 |

**为什么必须隔离**（证据）：0.6.3 因 `ort 2.0.0-rc.14` 的 `Session::builder(&Environment)` 新签名编译失败；0.9.1 修复 API 但要求 rustc 1.95。两条路都指向"工具链/依赖隔离"，顺带避免 ONNX Runtime 与模型进入主二进制。

### P3-B · PDF 页视图（渲染 + 命中 bbox）

| 层 | 内容 |
|---|---|
| stcore | `render_page_png(bytes, page, width)` → PNG；`page_text_boxes()` → **归一化 bbox**（左上原点，PDF 左下原点已换算）；`page_count()` |
| app | 3 个命令：`pdf_page_count` / `render_pdf_page` / `pdf_page_boxes`（含 50MB 闸门） |
| ui | `PdfPageView.tsx`（自包含）：页位图 + 命中框叠层 + 上下翻页 + Blob URL 生命周期管理；`PreviewPane` 仅 PDF 时显示"页视图" Tab |
| css | `components/preview/preview.css` 追加 `.pdf-view*` / `.pdf-hit-box` |

### P3-C · 拼音 + 同义词检索

| 项 | 内容 |
|---|---|
| 拼音 | 索引侧附加：汉字 → 无声调拼音**连写**（`人民 共和国` → `renmin gongheguo`）存入 schema **v2** 新字段 `pinyin`（仅索引不存储）；查询侧纯 ASCII 查询（`renmin`）额外投到该字段（OR） |
| 为什么不在查询侧展开 | 拼音→汉字一对多，精确展开需大字典且引入噪声；索引侧附加 + 复用同一 `seg` 分词器（1–3gram / bigram AND）天然对齐 |
| 同义词 | `synonym.rs`：24 组内置（可 `<index_dir>/synonyms.json` 覆盖）；fuzzy 模式下 token → "原词 OR 同义词组"（AND 主语义不变） |
| 语义边界 | 扩展**仅 fuzzy 生效**：exact 是"完全匹配"、dsl 由用户显式控制运算符 |
| 兼容 | schema v1 → **v2**：旧索引打开时明确提示"请重建"（实测输出见 §5） |

### P3-D · 性能基准

`stcore/tests/p3_perf.rs`（`#[ignore]`，`--release --ignored` 手动跑），实测：

| 基准 | 结果 | 参照门禁 |
|---|---|---|
| 8 MB 文本 / 4 词项 `match_groups` | **18.4 ms**（44.6 万处命中） | < 2 s（宽松） |
| 2000 篇 `upsert` | **8.4 ms**（23.7 万篇/秒）+ commit 402.8 ms | — |
| 5000 篇库 / 100 次查询 | **P50 10.2 ms · P99 37.5 ms · max 113.7 ms** | plan：P99 < 100 ms ✅ |

---

## 3. P2 + P3 代码审计（本批执行）

审计范围：P2（L 批：`extract/pdf.rs` PDFium 双路径、`extract/ebook.rs`、`scripts/fetch-gh.sh`、`third_party/pdfium`）+ P3（`pinyin.rs`、`synonym.rs`、`index` schema v2、`search` MapCtx、app 3 命令、`PdfPageView.tsx`、`PreviewPane` 改动、`ocr-helper`）。

### 3.1 发现并修复（4/4）

| # | 级别 | 位置 | 现象（证据） | 修复 |
|---|---|---|---|---|
| A1 | P2 | `index/mod.rs` `upsert` | 对**整篇** body 做拼音转换（无上限）：8 MB 文档 → 800 万次查表 + 十几 MB 临时串，索引吞吐被拖累（D 基准只覆盖短文档，掩盖此点） | `PINYIN_SOURCE_LIMIT = 256 KB` + UTF-8 边界对齐截断；新增 `long_text_is_truncated_at_char_boundary` 测试 |
| A2 | P2 | `extract/pdf.rs` `render_page_png` | 旧闸门 `clamp(32,4096)` × 高度 8 倍 → 最坏 **4096×32768 ≈ 1.34 亿像素 ≈ 536 MB** 单页位图 | 收紧为 宽 ≤ 2048 / 高 ≤ 宽×4 / 总像素 ≤ 2400 万（≈96 MB）；新增 `render_width_is_clamped_to_safe_max`（从 PNG IHDR 读回真实宽度断言） |
| A3 | P3（UX） | `search/mod.rs` hl_tokens | 同义词命中的文档显示 **"0 次命中"**（统计词表只含原词），高亮也不覆盖同义词 | fuzzy 下把同义词并入统计/高亮词表；冒烟复测 `电脑` → `[1 次命中]` + snippet 显示"计算机" |
| A4 | P2（健壮性） | `app/src/main.rs` | `search-tool cli search ... \| head` 触发 **`failed printing to stdout: Broken pipe` panic**（4 处复现） | 新增 `out()` + `cli_out!` 安全输出（忽略 BrokenPipe）；冒烟 `\| head -1` 无 panic、exit=0 |

### 3.2 登记不修（2）

| # | 项 | 说明 |
|---|---|---|
| A5 | 拼音命中的"命中数"仍为 0 | 拼音**无法反查**中文词面，`count_and_snip` 只能在正文里找字面。属计数语义限制而非缺陷（结果召回正确、排序正确）。后续可在结果行标注"（拼音命中）" |
| A6 | `renderPdfPage` 走 JSON `number[]` | 单页 PNG（~100–500 KB）经 JSON 数组膨胀 4–5 倍。当前按需单页加载可接受；后续可改 base64 或二进制 IPC |

### 3.3 审计通过项（无问题）

- **契约**：3 个新命令与 `ui/src/api/preview.ts` 参数名一致（`pageIndex`/`targetWidth` ↔ `page_index`/`target_width` 由 Tauri camelCase 转换），`PageTextBox` 两端字段一致
- **panic 路径**：新增生产代码无 `unwrap`/`expect`（仅测试内）
- **并发**：PDFium 经 `OnceLock` 一次性初始化 + `thread_safe` feature；`pdfium()` 失败路径全部落在 `Unsupported`（受控）
- **前端资源**：`PdfPageView` 的 Blob URL 在换页与卸载时均 `revokeObjectURL`；effect 带 `alive` 守卫
- **P2 通道**：`fetch-gh.sh` 的 `set -euo pipefail` 与 `try A || try B` 组合安全；`ebook.rs` 的 `catch_unwind` + DRM→`Unsupported` 分支正确
- **schema 演进**：v2 变更被 `check_schema_version` 拦截并给出可执行指引（实测见 §5）

---

## 4. 端到端冒烟（headless CLI）

```
完成: 扫描 9 个文件, 索引 9 篇文档

### 拼音：renmin  → 销售台账.xlsx（正文"人民好酒"）      1 结果
### 拼音：jidu    → 季度报告.docx（"季度报告"）          1 结果
### 拼音：suanfa  → 示例小说.fb2（"共识 算法"）          1 结果
### 同义词：电脑  → 计算机笔记.txt（正文只含"计算机"）   [1 次命中] ✅ 修复 A3 后计数正确
### exact：电脑   → 计中文档靠原文命中（不做同义扩展）    语义正确
### | head -1     → 无 panic，exit=0 ✅ 修复 A4 后
```

---

## 5. schema 升级提示（v1 → v2）

```
$ SEARCH_TOOL_INDEX_DIR=<旧索引> search-tool cli stats
错误：打开索引失败: 索引由旧版本创建，请重建（schema 版本不匹配：期望 2，实际 Some(1)；
      删除索引目录后重新扫描即可）
```

> **升级须知**：本批引入拼音字段（schema v2），**旧索引需重建**（UI 会弹出该提示；重建走"索引管理 → 索引构建"）。

---

## 6. 偏差与缺口

| # | 项 | 状态 | 说明 |
|---|---|---|---|
| N1 | OCR 主程序接入 | 未做 | helper 已就绪（协议 + 编译验证），待接：`stcore` 侧子进程调用封装（扫描型 PDF / 图片 → helper）、模型分发策略（随包 vs 首启下载）、OCR 与索引线程池互斥（plan R2） |
| N2 | 拼音覆盖范围 | 已知 | 仅 fuzzy 模式；仅源文本前 256 KB（A1 修复的代价）；命中数显示 0（A5） |
| N3 | 扫描型 PDF 的 bbox | 未做 | 文字型 PDF 已有 bbox（P3-B）；扫描型需 helper 返回 box 字段（当前只回文本 + 置信度） |
| N4 | `@silurus/ooxml` 预览保真 | 未做 | npm 镜像可达，属独立批次（WASM + Canvas 渲染） |
| N5 | 旧二进制格式 | 未做 | `doc/xls/ppt`（CFB，需 anydoc）与 `chm/djvu` |
| N6 | 同义词/拼音的 UI 提示 | 未做 | 结果行未标注"同义词/拼音命中"；设置页未提供同义词编辑入口（`synonyms.json` 可手工放） |
| N7 | 打包 | 未做 | `tauri build` 需同时分发 `libpdfium.so`（7.6 MB），并在 bundle 配置中声明 resources |

---

## 7. 复现命令

```bash
cd /workspace/search-tool
cargo fmt --all && cargo clippy --workspace --all-targets && cargo test --workspace   # 178 passed
cargo test --release -p stcore --test p3_perf -- --ignored --nocapture                # 性能基准
cd ui && pnpm build && cd ..

# 拼音 / 同义词冒烟（schema v2，需先重建索引）
export SEARCH_TOOL_INDEX_DIR=/tmp/p3-index
./target/debug/search-tool cli index --dir /tmp/p1-samples
./target/debug/search-tool cli search renmin --mode fuzzy
./target/debug/search-tool cli search 电脑   --mode fuzzy

# OCR helper（独立工具链 1.95）
cd ocr-helper && cargo build --release
echo '{"image_path": "/tmp/scan.png"}' | ./target/release/ocr-helper
```

---

## 8. 下一步建议

1. **OCR 主程序接入**（helper → stcore 子进程封装 + 线程限流 + 模型分发决策）
2. **扫描型 PDF 端到端**：helper 补 box 字段 → 与 P3-B 的页视图统一叠层
3. **打包批**：`tauri build` + 声明 PDFium 资源 + 体积核对
4. **`@silurus/ooxml` 预览保真**（npm 镜像已验证可达）
5. **旧格式**：`doc/xls/ppt`（CFB）与 `chm/djvu`
