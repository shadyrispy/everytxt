# 审计批 + P1 格式扩展批 · 交付报告

> 批次：全量代码审计（4 路并行只读）→ 缺陷修复迭代 → P1 提取层扩展
> 完成时间：2026-10-14
> 前置：`P0+-REPORT.md`（P0+ 五子项）

---

## 1. 结论

**审计 4 路回收 P0×1 / P1×9 / P2×40，全部 P0/P1 与选定 P2 已修复；P1 格式扩展（Office/PDF/EPUB）落地并端到端验证。工作区全绿。**

| 质量门禁 | 修复前 | 修复后 |
|---|---|---|
| `cargo fmt --all --check` | clean | ✅ clean |
| `cargo clippy --workspace --all-targets` | 0 warning | ✅ **0 warning** |
| `cargo test --workspace` | 120 passed | ✅ **160 passed / 0 failed**（+40） |
| `cargo build -p search-tool --bin search-tool` | ✅ | ✅ `target/debug/search-tool` |
| `cd ui && pnpm build` | ✅ | ✅ `tsc -b` + vite（283.8 KB JS / 23.1 KB CSS） |
| headless 冒烟（8 格式） | 不适用 | ✅ 8 扫描 / 8 索引，7/7 关键词命中 |

> 后续增补（同日）：**L1 PDFium 通道**（解中文 CID PDF）、**L2 电子书扩展**（mobi/fb2/lit/cbz/rtf/odt）、**L0 国内下载通道固化**。见 §8。

---

## 2. 审计发现与修复

### 2.1 P0（1/1 已修）

| # | 位置 | 现象 | 修复 |
|---|---|---|---|
| P0-1 | `preview.rs` `find_in_doc` | 大小写不敏感时 `text.to_lowercase()` 生成的 haystack 偏移被用于切**原文**，含 İ/K/ẞ 时非字符边界 → IPC 命令 panic | 折叠改为**仅 ASCII**（hay 与原文等长同坐标系），全路径偏移安全 |

### 2.2 P1（9/9 已修）

| # | 位置 | 现象 | 修复 |
|---|---|---|---|
| P1-1 | `search/mod.rs` `count_and_snip` | 同源的 lower 偏移切 body → 搜索主路径 panic 风险 | 同上 ASCII 折叠 |
| P1-2 | `search/mod.rs` 反向 + exact | reverse 差集结果被正向 exacts 复核全部剔除 → 恒空 | reverse 时跳过 exacts 复核 |
| P1-3 | `search/mod.rs` `Expr::Not` | 否定词面污染 positive exacts → DSL `"a" & !"b"` 恒空 | Not 返回空 exacts |
| P1-4 | `extract/mod.rs` `expand_gz` | 单文件 gz 回退 `read_to_end` 无上限 → 压缩炸弹绕过全部闸门 OOM | 限量读取（`MAX_FILE_SIZE + 1` 探测）+ 炸弹测试 |
| P1-5 | `app/state.rs` `with_mgr` | 锁外快照 dir + 迁移热切换 TOCTOU → 搜索静默命中旧索引 | 缓存改 `(PathBuf, IndexManager)`，锁内比对目录不匹配即重开 |
| P1-6 | `index_admin.rs` `index_directory` | 未参与 `ctl.running` 互斥 → 与重建并发双 writer 锁冲突；传 `None` 致取消/暂停按钮失灵 | 入口 `running.swap` 占用 + finally 复位 + 传 `Some(&ctl.control)` |
| P1-7 | `App.tsx` 前进/后退 | 只改 `navPos` 无 effect 应用 → 按钮空转 | 加 effect 应用 navStack 条目 + ref 防回写 |
| P1-8 | `App.tsx` 索引暂停态 | 双源失真；`indexState` 恒 'idle'（无 index-progress 监听）→ 指示灯永不亮 | App 级单一状态源，监听 index-progress 驱动 idle/indexing/paused；工具栏暂停真调后端 |
| P1-9 | 删除后结果不同步 | 删除/移动后 `results` 残留、`selected` 未清 | App 提供 onRefresh，统一三条删除入口收尾 |

### 2.3 P2（选定项已修，其余登记）

**已修（按域）**：

| 域 | 项 |
|---|---|
| stcore 安全/正确性 | 目录前缀过滤补路径分量边界；exact 复核大小写口径统一（ASCII 折叠）；`static_ext` 消除 `Box::leak`；三态规则**类型级优先于全局**；`next_id` 回填防 id 碰撞；内置类型禁改 name/exts；DSL 嵌套深度上限 64；`interval_minutes * 60` 溢出钳制；regex 模式 whole_word 自动包 `\b`；词项含换行拒绝；match `count` 独立计数不受收集上限截断；预览三命令统一 `truncate_preview`；schema 版本校验（旧索引明确报"请重建"）；压缩条目名拒 `../`/绝对路径/`#`；zip/tar 符号链接条目跳过 |
| app 层 | `migrate_storage` 参与互斥；`restart_scheduler` 先读配置再停旧句柄；重建取消返回 `Err("已取消")`；`get_index_stats`/`get_storage_info` 加 `available` 字段（索引不可用可见）；锁毒化改 `into_inner()` 恢复；`delete_permanently` 加危险路径护栏（拒索引目录/根）；`transfer_into` 拒目标嵌套源；`copy_paths` 删除误导注释；`get_preview`/`get_file_info` 统一大小上限；`load_text` 先查磁盘存在性再判 `#` 虚拟路径；CLI 全部 unwrap/expect → `die()` 优雅退出；socket 先 connect 探测再清理残留；热键**逐键独立注册**（不再 all-or-nothing）+ **全局白名单制**（仅 `showWindow` 系统级注册，Ctrl+A/Enter/Ctrl+S 等改为窗口内键）；扫描层扩展名改读 `Config::all_exts()`（单一事实源） |
| 前端 | find-in-doc effect 卸载守卫；docCache 过期清理 + 上限；minimap 截断比例修正；`window.open` 补 `noopener`；死代码清理（getPreview/showMainWindow/hideMainWindow/collectTerms 等）；`nav.refresh` 补实现、`edit.favorite` 死按钮移除；IndexAdminDialog 文案改正（热切换已实现）；`indexDirectory` 契约改回 `ScanReport` 并展示"扫描 N / 索引 M"；注释 `rebuild://progress` → `index-progress`；`IndexRunState` 统一引用；find-bar 跨组件 DOM 查询改 prop；`available` 不可用态提示 |

**明确登记不修（理由）**：错误链扁平化（需引入 `AppError` 序列化契约，属独立批次）；Tauri 同步命令改 async（需重测进度事件时序）；CLI 单实例完整 socket 转发（现为探测+提示）；Windows 命名管道。

---

## 3. P1 格式扩展交付

| 模块 | 规模 | 能力 | 关键设计 |
|---|---|---|---|
| `extract/ooxml.rs` | 394 行 | docx/docm、xlsx/xlsm/xlsb、pptx/pptm/pps/ppsx/pot | zip + quick-xml 流式抽 `<w:t>`/`<v>`/`<a:t>`；段落换行；共享串表索引解析；**GeneralRef 实体显式解析**（0.42 把 `&amp;` 拆为独立事件）；part 数/字节双上限 |
| `extract/pdf.rs` | 205 行 | PDF 文字层（**中文 CID 可解**） | **PDFium 主路径**（CMap/CID 完整映射）+ `pdf-extract` 兜底；库定位 4 级候选（env / exe 同级 / `third_party/pdfium/lib` / 系统）；页级异常跳过；两解析器互补 |
| `extract/ebook.rs` | 111 行 | mobi/azw/azw3/fb2/lit/kfx/cbz/rtf/odt | `ebook-rs` 0.17；`Book::sections().plain_text`；DRM 判 `Unsupported`；`catch_unwind` 兜底；**不接管 epub/docx**（自有链路优先，避免双源） |
| `third_party/pdfium/lib/libpdfium.so` | 7.6 MB | PDFium 运行时（Chromium PDF 引擎，BSD-3） | 随包分发；`scripts/fetch-gh.sh` 记录获取方式 |
| `extract/epub.rs` | 217 行 | EPUB2/3 | container.xml → OPF → spine 顺序读 XHTML，复用 html 层转文本；子目录 href 拼接 |
| `tests/p1_formats.rs` | 126 行 | 路由接线 + 真实夹具 | 6 集成测试（含 `tests/fixtures/en.pdf`、`cid-cn.pdf`） |

**依赖新增**：`pdf-extract = "0.12.1"`、`quick-xml = "0.42.0"`、`pdfium-render = "0.9.4"`（no-default + `pdfium_latest` + `thread_safe`）、`ebook-rs = "0.17.1"`（Office/EPUB/PDFium 之外复用已有 `zip`/`flate2`）。

**路由接线**：`extract_path` 与 `extract_entry` 双入口均接入新格式（压缩包内层 docx/xlsx/epub 同样可提取）；`static_ext` 查表补三组扩展名；扫描层经 `Config::all_exts()` 覆盖（内置类型表已含 docx/xlsx/pptx/pdf/epub）。

---

## 4. 端到端冒烟（headless CLI，真实样例，8 格式）

```
$ search-tool cli index --dir /tmp/p1-samples
完成: 扫描 8 个文件, 索引 8 篇文档              ← 8/8 全索引（含中文 CID PDF、真实 mobi）

格式         关键词         结果
docx        量子计算        1 命中  报告正文
xlsx        华北            1 命中  单元格（共享串表解析）
pdf(英文)   Inverted        1 命中  文字层
pdf(中文CID) 倒排索引        3 命中  文字层 ← 升级前为 0 结果/panic
epub        分布式          1 命中  spine 章节
fb2         拜占庭          1 命中  XML 正文
mobi        rabbit         53 命中  Alice in Wonderland（真实 Mobipocket v6）
```

---

## 5. 偏差与缺口（如实登记）

| # | 项 | 状态 | 说明 |
|---|---|---|---|
| N1 | ~~中文 CID PDF~~ | **已解决（L1）** | PDFium 通道接入后中文 CID PDF 端到端命中（`倒排索引` 3 次）；`pdf-extract` 降为兜底（无 PDFium 库时仍受控失败） |
| N2 | 扫描型 PDF / 图片 OCR | 未做 | 需模型分发 + 推理运行时（PP-OCRv6 / ort）。**anydoc-ocr 源码已获取**（`gh-proxy` 通道），依赖链 `anydoc 0.2.4 / oar-ocr / ofd-core / pdf-inspector` 在 crates.io 全部可拉，待接入（需 ONNX Runtime 预编译库 + 模型下载） |
| N3 | `@silurus/ooxml` 高保真预览 | 未做 | **npm 镜像可达**（`registry.npmmirror.com` 200）；属预览保真层（WASM + Canvas），与提取层不同层 |
| N4 | ~~mobi/azw3/fb2/chm/djvu~~ → **mobi/azw3/fb2/lit/kfx/cbz/rtf/odt 已解决（L2）**；chm/djvu 未做 | 部分解决 | `ebook-rs` 0.17 接入，mobi 实测 53 命中；chm/djvu 需 `libchm`/`djvu-rs`（plan spike S11） |
| N5 | 同义词 / 拼音 | 未做 | plan §9 P1 余项 |
| N6 | xlsb / doc（BIFF/CFB 旧格式） | 未做 | 现仅 OOXML（xlsx/docx/pptx）；旧二进制格式需 anydoc 或专用解析 |
| N7 | 错误链扁平化 / 同步命令 async 化 / Windows 命名管道 / CLI socket 完整转发 | 登记 | 见 §2.3 末 |
| N8 | PDFium 分发 | 已就位 | `third_party/pdfium/lib/libpdfium.so`（7.6 MB，v157）；打包时经 `tauri.conf.json` resources 随应用分发（待打包批确认） |

---

## 6. 复现命令

```bash
cd /workspace/search-tool
cargo fmt --all && cargo clippy --workspace --all-targets && cargo test --workspace   # 156 passed
cd ui && pnpm build && cd ..
cargo build -p search-tool --bin search-tool

# P1 格式冒烟（样例可用 reportlab/openpyxl/zipfile 生成）
SEARCH_TOOL_INDEX_DIR=/tmp/p1-index ./target/debug/search-tool cli index --dir /tmp/p1-samples
SEARCH_TOOL_INDEX_DIR=/tmp/p1-index ./target/debug/search-tool cli search 量子计算 --mode fuzzy
```

---

## 7. 下一步建议（按价值排序）

1. **OCR 通道**：anydoc-ocr 源码已到手（`gh-proxy` 可下），依赖链 crates.io 全通；缺 ONNX Runtime 预编译库 + 模型（ModelScope）→ 扫描件/图片 OCR
2. **预览保真**：`@silurus/ooxml`（npm 镜像可达）→ docx/xlsx/pptx Canvas 渲染 + 命中高亮
3. **旧格式**：`doc/xls/ppt`（CFB）与 `chm/djvu`
4. **PDF 页视图**：PDFium 已接入 → 页渲染 PNG + bbox 叠层（命中定位到页）
5. **GUI 真机走查**：对照 docs/ui-redesign.md 核验视觉还原度

---

## 8. 增补批（同日）：L0/L1/L2

### 8.1 L0 · 国内下载通道固化

实测（2026-10-14）：

| 通道 | 状态 | 用途 |
|---|---|---|
| `https://gh-proxy.com/<github-url>` | ✅ 200 | GitHub 网页/raw/releases/archive 全类型 |
| `https://codeload.github.com/<owner>/<repo>/zip/refs/heads/<br>` | ✅ 200（直连） | 仓库 zip（无需代理） |
| `https://registry.npmmirror.com` | ✅ 200 | npm 包（`@silurus/ooxml` 等） |
| ustc crates 镜像（Cargo 已配） | ✅ | Rust 依赖；`cargo add --dry-run` 可查可用性 |
| `ghfast.top` / `ghproxy.net` | ❌ 403 | — |
| `mirror.ghproxy.com` / `kkgithub` / `bgithub` / `hub.gitmirror` | ❌ 不可达 | — |
| `gitclone.com` | ❌ 502 | — |

固化脚本：`scripts/fetch-gh.sh`（`archive` / `raw` / `release` / `url` 四子命令，自动代理→直连回退）。

### 8.2 L1 · PDF 通道升级（PDFium）

- **根因**：`pdf-extract` 0.12 不支持 CID CMap（`UniGB-UCS2-H`）→ 中文 PDF 无法提取且内部 panic
- **修复**：`pdfium-render 0.9.4`（Chromium PDFium，BSD-3）为主路径；libpdfium 四级定位（env `SEARCH_TOOL_PDFIUM_LIB` → exe 同级 → `third_party/pdfium/lib` → 系统库）；`pdf-extract` 保留为兜底
- **效果**：中文 CID PDF 从 **0 结果 / panic** → **3 次命中、正文正确**；页级异常跳过不整体失败
- 新增 `extract/pdf.rs` 205 行、`third_party/pdfium/lib/libpdfium.so`（7.6 MB）

### 8.3 L2 · 电子书扩展（ebook-rs）

- 接入 `ebook-rs 0.17.1`：mobi / azw / azw3 / fb2 / lit / kfx / cbz / rtf / odt
- 边界设计：**epub 与 docx 仍走自有链路**（不交给 ebook-rs），避免同格式双源；DRM 判 `Unsupported`（跳过而非失败）；`catch_unwind` 兜底第三方解析器 panic
- 实测：真实 Mobipocket v6（Alice in Wonderland）`rabbit` 53 次命中；自建 fb2 `拜占庭` 1 次命中
- 新增 `extract/ebook.rs` 111 行

### 8.4 增补批门禁

| 门禁 | 结果 |
|---|---|
| `cargo fmt --all --check` | ✅ clean |
| `cargo clippy --workspace --all-targets` | ✅ **0 warning** |
| `cargo test --workspace` | ✅ **160 passed / 0 failed** |
| headless 冒烟 | ✅ 8 扫描 / 8 索引，7/7 关键词命中 |
| 二进制 | `target/debug/search-tool` 459 MB（debug，含 PDFium 绑定与 ebook-rs） |

### 8.5 L3 准备（OCR 通道，下一批）

| 项 | 状态 |
|---|---|
| anydoc-ocr 源码 | ✅ 已获取（`gh-proxy` 通道，1.28 MB zip；`src/` 1038 行，含 pdf/ofd/ocr/gfm_adapter） |
| 依赖链可拉性 | ✅ `anydoc 0.2.4` / `oar-ocr` / `ofd-core 0.3.0` / `pdf-inspector` / `pdfium-render` 均在 crates.io 可用 |
| ONNX Runtime 预编译库 | ✅ `onnxruntime-linux-x64-1.20.1`（anydoc-ocr README 指定版本）已下载（`gh-proxy`）；待落入 `third_party/ort/x64/` |
| OCR 模型分发 | ⏳ 需验证 ModelScope 具体模型 URL 可达性（根 API 404，不代表模型地址不可达）；不行则用 `oar-ocr` 的 `auto-download` 或手工外置模型目录 |
| 阻塞点 | OCR 属**重依赖**（ORT 动态库 + 模型 ~数十 MB）：需确认是否随包分发或首次运行下载；另需线程池限流（plan 风险 R2） |
