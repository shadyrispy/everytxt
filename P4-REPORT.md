# P4 批 · 交付报告（OCR 接入 / 打包 / 旧格式 / OOXML 视图）

> 批次：P4 四线并行
> - **P4-A** OCR 主程序接入（子进程封装 + 开关 + 扫描型 PDF 兜底）
> - **P4-B** 打包交付（tauri build + PDFium 资源声明）
> - **P4-C** 旧格式扩展（OLE 二进制与 ODF：doc/xls/ppt/odt/ods/odp/rtf）
> - **P4-D** OOXML 文档视图（`@silurus/ooxml` WASM Canvas 渲染 + 命中高亮）
> 完成时间：2026-10-14

---

## 1. 结论

**四线全部落地，端到端验证通过；首次产出可安装包。**

| 质量门禁 | 结果 |
|---|---|
| `cargo fmt --all --check` / `cargo clippy --workspace --all-targets` | ✅ clean / **0 warning** |
| `cargo test --workspace` | ✅ **185 passed / 0 failed**（+7） |
| `cd ui && pnpm build` | ✅ 主包 289 KB + 按需分块（docx 815 KB / xlsx 286 KB / pptx 290 KB + 3 份 wasm） |
| `cargo tauri build` | ✅ **deb 21.8 MB**（含 `usr/lib/search-tool/lib/libpdfium.so` 7.96 MB）；appimage 受沙箱环境限制（见 §5 N5） |
| OCR 端到端 | ✅ 开关两态对比通过（见 §3.1） |
| 旧格式端到端 | ✅ `.xls` 命中（见 §3.3） |

---

## 2. P4-A · OCR 主程序接入

| 组件 | 内容 |
|---|---|
| `stcore/src/ocr.rs` | helper 定位（env → exe 同级 → 源码树）、stdio JSON 调用、**全局互斥串行化**（plan R2：避免 OCR 内存峰值与索引叠加）、低置信度行（<0.3）过滤 |
| 开关 | `SEARCH_TOOL_OCR=1` 启用（默认关，符合 plan §5.2 `ocr_images=false`）；未启用或无 helper → 记 `Unsupported` **跳过**（不算失败） |
| 图片通道 | `IMAGE_EXTS`（png/jpg/jpeg/webp/bmp/tiff/gif）→ helper OCR；内置类型表新增 "图片"(id=10) 使扫描层收录 |
| 扫描型 PDF 通道 | `pdf.rs` 无文字层时返回 **`ExtractError::NeedsOcr{pages, page_count}`** → dispatch 在 OCR 可用时改为 **逐页渲染(PDFium) → OCR** 拼接；页数闸门 200 页 |
| 内置类型表 | 新增 "旧格式"(id=11)（doc/xls/ppt/odt/ods/odp/rtf） |

---

## 3. 端到端证据

### 3.1 OCR（图片）

```
$ SEARCH_TOOL_OCR=1 search-tool cli index --dir /tmp/p4-samples
完成: 扫描 1 个文件, 索引 1 篇文档
$ search-tool cli search Inverted --mode fuzzy
  1. [1 次命中] 扫描件示例.png
      Inverted index whitepaper Full text search engine 0CR channe…

# 关闭 OCR（未设 SEARCH_TOOL_OCR）
完成: 扫描 1 个文件, 索引 0 篇文档        ← 正确跳过
```

helper 直调（模型首次自动下载后）：

```json
{"ok":true,"lines":[
  {"text":"Inverted index whitepaper","confidence":0.9968},
  {"text":"Full text search engine","confidence":0.9964},
  {"text":"0CR channel test 2026","confidence":0.9585}]}
```

### 3.2 旧格式（OLE 二进制 .xls）

```
$ search-tool cli search 人民好酒 --mode fuzzy
  1. [3 次命中] 旧版台账.xls
      | 产品 | 销量 | | --- | --- | | 人民好酒 | 860 | | 中华烟 | 1200 |
```

anydoc 把 `.xls` 还原为 Markdown 表格（`odt/ods/odp/doc/ppt/rtf` 同链路）。

### 3.3 打包（deb）

```
search-tool_0.1.0_amd64.deb  21.8 MB
  usr/bin/search-tool                        50.7 MB
  usr/lib/search-tool/lib/libpdfium.so        7.96 MB   ← resources 声明生效
Installed-Size: 57261
Depends: libayatana-appindicator3-1, libwebkit2gtk-4.1-0, libgtk-3-0
```

前端资源（含 3 份 `*_parser_bg.wasm`）由 Tauri 嵌入二进制，无需额外文件。

---

## 4. 关键决策与证据

| 决策 | 证据 |
|---|---|
| OCR 走**独立进程**而非链接进主程序 | `oar-ocr 0.6.3` 因 `ort 2.0.0-rc.14` 的 `Session::builder(&Environment)` 新签名编译失败；`0.9.1` 修复但要求 **rustc ≥ 1.95**（主工程 stable 1.93）。故隔离工具链 + 隔离重依赖 |
| 旧格式用 **anydoc 0.2.4** | 其 `Format` 枚举覆盖 Doc/Ppt/Excel（含 `.xls`/`.xlsb`）/Odt/Ods/Odp/Rtf；且对扫描型 PDF 返回 **`NeedsOcr{pages,page_count}`**，与 P4-A 天然衔接 |
| docx/xlsx/pptx 仍走**自建** `ooxml.rs` | 避免与 anydoc 双源（提取层单一事实源原则） |
| OOXML 视图用 **`@silurus/ooxml 0.88.0`** | 提供 `findText/findNext/findPrev` + `findHighlightColors`（命中高亮与跳转原生支持）；三格式构造签名不同（docx/pptx 收 canvas、xlsx 收容器），已在 `OoxmlView.tsx` 分别适配 |
| appimage 需 AppRun/linuxdeploy | tauri 内置下载走 GitHub 直连超时；已用 `gh-proxy` 预置 `AppRun-x86_64`、`linuxdeploy-x86_64.AppImage`、`linuxdeploy-plugin-appimage-x86_64.AppImage` 到 `~/.cache/tauri/`。**但仍失败**：沙箱无 `/dev/fuse`，AppImage 无法挂载执行；改用 `--appimage-extract` + shell wrapper 绕过时，tauri-bundler 对工具做体积/校验并回退到内置版本，仍报 `failed to run linuxdeploy`。属**环境限制而非代码问题**（deb 路径不依赖 FUSE，已成功） |

---

## 5. 偏差与缺口

| # | 项 | 状态 | 说明 |
|---|---|---|---|
| N1 | OCR 模型分发 | 待决策 | 当前首次运行自动下载（ModelScope → `~/.oar`，实测可用）。若要求离线安装，需把模型随包分发（体积 +数十 MB） |
| N2 | `.doc`/`.ppt` 端到端 | 未验证 | 本环境无 LibreOffice 且公开 `.doc` 样例源 403；`.xls` 已实测。三者共用同一 anydoc 链路，风险低但**未实测** |
| N3 | OCR 结果无 bbox | 已登记 | helper 当前只回文本 + 置信度；扫描型 PDF 的命中框叠层需 helper 扩展 box 字段 |
| N4 | OCR 与索引的线程限流 | 部分 | 已做**进程内互斥**（同一时刻一次 OCR）；跨进程/全局配额未做 |
| N5 | appimage | **环境受限** | 根因：沙箱无 `/dev/fuse`（容器未挂载 FUSE）→ linuxdeploy 的 AppImage 无法挂载执行；tauri-bundler 会校验工具文件并回退内置版本，wrapper 绕过无效。**解法**：在具备 FUSE 的宿主/CI 上执行 `cargo tauri build --bundles appimage`（或 `apt install libfuse2` + 宿主开放 `/dev/fuse`）。**deb 已可用**，Linux 分发不受阻塞 |
| N6 | 模型/依赖体积 | 已登记 | deb 21.8 MB（含 PDFium）；OCR 模型独立于安装包（首启下载） |
| N7 | i18n / 自动更新 / 右击搜索 | 未做 | plan P2 打磨项 |

---

## 6. 复现命令

```bash
cd /workspace/search-tool
cargo fmt --all && cargo clippy --workspace --all-targets && cargo test --workspace   # 185 passed
cd ui && pnpm build && cd ..
cargo tauri build                                                                     # deb（+ appimage）

# OCR（先构建 helper，首次运行会下载模型）
cd ocr-helper && cargo build --release && cd ..
echo '{"image_path": "/tmp/scan.png"}' | ocr-helper/target/release/ocr-helper
SEARCH_TOOL_OCR=1 SEARCH_TOOL_INDEX_DIR=/tmp/idx ./target/debug/search-tool cli index --dir <含图片的目录>

# 旧格式
SEARCH_TOOL_INDEX_DIR=/tmp/idx ./target/debug/search-tool cli index --dir <含 xls/doc 的目录>
```

---

## 7. 下一步建议

1. **OCR bbox 扩展**：helper 回 box → 扫描型 PDF 页视图命中框（与 P3-B 统一）
2. **模型分发决策**：随包 vs 首启下载（含离线安装方案）
3. **旧格式补测**：`.doc`/`.ppt` 真实样例（CI 内用 LibreOffice 转换生成）
4. **plan P2 打磨**：i18n（en/zh-CN）、自动更新、右击搜索、大文本虚拟化
5. **性能回归**：把 P3-D 基准接入 CI（当前 `#[ignore]` 手动）
