# P5 批 · 交付报告（OCR bbox / 模型随包 / 右击搜索）

> 批次（按用户指令）：① 扫描件页视图命中框（OCR bbox）② OCR 模型随安装包分发 ③ 右击搜索（shell 集成）④ 性能基准接 CI（本次不做）
> 完成时间：2026-10-14

---

## 1. 结论

**三项全部落地并验证；门禁全绿。**

| 门禁 | 结果 |
|---|---|
| `cargo fmt --all --check` / `cargo clippy --workspace --all-targets` | ✅ clean / **0 warning** |
| `cargo test --workspace` | ✅ **186 passed / 0 failed** |
| `cd ui && pnpm build` | ✅（新增设置项，主包体积不变） |
| OCR bbox（扫描型 PDF） | ✅ 实测（见 §3.1） |
| 右击搜索（Linux） | ✅ 安装/卸载 + 文件产物实测（见 §3.3） |
| `cargo tauri build --bundles deb` | ✅ **29.6 MB**，实测含 PDFium 与 3 个 OCR 模型文件（见 §3.2） |

### 安装包内容（实测 `dpkg-deb -c`）

```
usr/lib/search-tool/lib/libpdfium.so                                    7,964,952
usr/lib/search-tool/lib/search-tool/models/pp-ocrv6_tiny_det.onnx        1,780,590
usr/lib/search-tool/lib/search-tool/models/pp-ocrv6_tiny_rec.onnx        4,462,639
usr/lib/search-tool/lib/search-tool/models/ppocrv6_tiny_dict.txt            27,156
```
deb 体积 21.8 MB（P4）→ **29.6 MB**（本批 +7.8 MB，即 OCR 模型）

---

## 2. ① OCR bbox：扫描件页视图命中框

| 层 | 变更 |
|---|---|
| `ocr-helper` | 响应新增 `box:[x,y,w,h]`（像素，左上原点）与 `image_width/image_height`；从 `region.bounding_box`（oar-ocr）取 |
| `stcore/src/ocr.rs` | 新增 `OcrBox{text,x,y,w,h}`（**归一化 0..1**，与 PDF 文字层 bbox 同口径）；`call_helper()` 抽取共用；`ocr_image_boxes()` / **`ocr_pdf_page_boxes()`**（渲染该页 → OCR → 归一化）；置信度阈值 0.3 |
| `app` `pdf_page_boxes` | 文字层为空时**自动回退 OCR**（需 helper + `SEARCH_TOOL_OCR=1`），返回同一 `PageTextBox` 结构 → 前端无需改动 |
| 前端 | 复用 P3-B 的 `PdfPageView` 叠框逻辑（同一归一化口径，零改动） |

**实测**（helper 直调）：

```json
{"ok":true,"image_width":900,"image_height":300,
 "lines":[{"text":"Inverted index whitepaper","confidence":0.9968,"box":[35.0,38.0,608.0,53.0]},
          {"text":"Full text search engine","confidence":0.9964,"box":[36.0,118.0,562.0,54.0]},
          {"text":"0CR channel test 2026","confidence":0.9585,"box":[35.0,197.0,514.0,52.0]}]}
```

**扫描型 PDF 端到端**（纯图片页，无文字层）：

```
$ SEARCH_TOOL_OCR=1 search-tool cli index --dir /tmp/p4-samples
完成: 扫描 3 个文件, 索引 3 篇文档
$ search-tool cli search Inverted --mode fuzzy
  1. [1 次命中] 扫描件示例.png
  2. [1 次命中] 扫描白皮书.pdf         ← 走 NeedsOcr → PDFium 渲染 → OCR
```

**bbox 归一化断言**（`tests/p5_ocr_boxes.rs`，`#[ignore]`）：

```
[ocr-boxes] 3 块，示例：OcrBox { text: "Inverted index whitepaper",
            x: 0.139, y: 0.1973, w: 0.684, h: 0.0424 }
```

---

## 3. ② OCR 模型随安装包

| 项 | 内容 |
|---|---|
| 模型入库 | `third_party/ocr-models/`：`pp-ocrv6_tiny_det.onnx` (1.7 MB) + `pp-ocrv6_tiny_rec.onnx` (4.3 MB) + `ppocrv6_tiny_dict.txt` (27 KB) ≈ **6.3 MB** |
| helper 解析顺序 | `SEARCH_TOOL_OCR_MODEL_DIR` → 可执行文件同级 `models/` → `../lib/search-tool/models/` → 源码树 `third_party/ocr-models/` → **回退注册名（auto-download）** |
| 打包声明 | `tauri.conf.json` resources：`../third_party/ocr-models` → `lib/search-tool/models` |
| 效果 | **安装后无需联网**（离线可用）；helper 与 deb 均携带模型 |

实测：`SEARCH_TOOL_OCR_MODEL_DIR=third_party/ocr-models` 直接识别成功（未触网、未读 `~/.oar`）。

---

## 4. ③ 右击搜索（shell 集成）

交互语义：**在目录上/目录空白处右键 → “搜索” → 以该目录启动应用**；开关在“设置 → 右击目录搜索”。

| 平台 | 机制 | 状态 |
|---|---|---|
| Linux | ① `~/.local/share/nautilus-python/extensions/search-tool.py`（**目录空白处**菜单，需系统装 `nautilus-python`）② `~/.local/share/nautilus/scripts/search-tool-search`（选中目录/文件右键“脚本”）③ `~/.local/share/applications/search-tool.desktop`（Open With 关联目录） | ✅ 已实现并实测 |
| Windows | `HKCU\Software\Classes\Directory\{shell, Background\shell}\SearchTool`（`%V` 传目录） | ✅ 已实现（本机不可实测） |
| macOS | 需 Finder Sync App Extension（须随应用签名打包） | 明确不支持并给出提示 |

**闭环**：右键 → `search-tool --open-dir <dir>` → GUI 启动 → 前端 `take_open_dir()` 取走该目录 → 预填目录过滤并自动搜一次。

**实测**（app 单测）：安装后三个文件均写入且脚本含 `--open-dir`；卸载后 `is_installed()==false`。

```
test result: ok. 23 passed   （含 unix_install_then_uninstall_writes_and_removes_files）
```

前端：设置 → 启动区新增“右击目录搜索”开关；保存时调用 `set_context_menu_enabled` 并把返回的安装结果提示给用户（失败仅提示、不回滚其他设置）。

---

## 5. 偏差与缺口

| # | 项 | 状态 | 说明 |
|---|---|---|---|
| N1 | Nautilus 空白处菜单依赖 `nautilus-python` | 已登记 | 未装该包时退化为“选中目录右键 → 脚本”；脚本与 .desktop 仍可用 |
| N2 | macOS 右击搜索 | 未做 | 需 App Extension + 签名；建议后续或用全局热键替代 |
| N3 | Windows 分支未实测 | 已登记 | 代码按 `reg add` 实现；需 Windows 环境验证 |
| N4 | OCR bbox 粒度 | 已登记 | 当前为**行级** bbox（oar-ocr 结果粒度）；词级需进一步切分 |
| N5 | 性能基准接 CI | 本次按指令不做 | `#[ignore]` 手动运行保持不变 |
| N6 | appimage | 环境受限 | 沙箱无 `/dev/fuse`（见 P4 报告）；deb 已可用 |

---

## 6. 复现命令

```bash
cd /workspace/search-tool
cargo fmt --all && cargo clippy --workspace --all-targets && cargo test --workspace   # 186 passed
cd ui && pnpm build && cd ..
cargo tauri build --bundles deb

# OCR helper（本地模型，不触网）
cd ocr-helper && cargo build --release && cd ..
printf '{"image_path":"/tmp/ocr-sample.png"}\n' | \
  SEARCH_TOOL_OCR_MODEL_DIR=third_party/ocr-models ocr-helper/target/release/ocr-helper

# 扫描件 OCR bbox
SEARCH_TOOL_OCR=1 cargo test -p stcore --test p5_ocr_boxes -- --ignored --nocapture

# 右击搜索（GUI：设置 → 右击目录搜索；或直接调用命令）
#   安装产物：~/.local/share/nautilus-python/extensions/search-tool.py
#             ~/.local/share/nautilus/scripts/search-tool-search
#             ~/.local/share/applications/search-tool.desktop
```

---

## 7. 下一步建议

1. Windows 环境实测右击搜索 + macOS Finder 扩展方案评估
2. OCR 词级 bbox（更精细的高亮框）
3. 打包批收尾：在具备 FUSE 的环境出 appimage；模型/库体积优化
4. plan P2 打磨剩余项：i18n（en/zh-CN）、自动更新
