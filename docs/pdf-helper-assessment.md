# pdf-helper 评估（架构 v2 §3 / §8-R5，只评估不实施）

日期：2026-10-14 ｜ 结论先行：**暂不拆进程，登记触发条件**——真机出现一次 PDFium segfault，或大页渲染内存峰值实测 OOM，即启动实施（预计 2 人日内落地，方案见 §4）。

## 1. 现状与风险

- PDF 链路（R4/P3-B）：提取侧 `pdf-extract` 纯 Rust；渲染/文字层/bbox 侧 `pdfium-render`（FFI → libpdfium.so，C++）跑在 app 主进程内。
- 风险 A（崩溃传染）：PDFium 是 C++，段错误（野指针/越界/损坏 PDF 触发的内部 UB）**不可被 Rust `catch_unwind` 捕获**——FFI 边界外 panic 传播本身是 UB，且 segfault 直接杀进程。后果：一次恶意/损坏 PDF 预览 = 整个应用退出，且丢失索引队列未 commit 状态。
- 风险 B（内存峰值）：整页位图渲染 `4096 宽 × 页高 × 4B/px`。A4 纵向页高 ≈ 5784 → 单页 ≈ 90 MB；工程图纸（A0，高 ≈ 16384）≈ 254 MB/页。叠加多 Tab 预览可轻易破 GB。Rust 侧 OOM abort 同样带走主进程。
- 风险 C（线程安全）：pdfium-render 已开 `thread_safe` feature，但 PDFium 全局锁串行化渲染，大页渲染阻塞其它 Tab 的渲染请求。

## 2. 隔离方案对比

| 方案 | 崩溃隔离 | 内存隔离 | 复杂度 | 备注 |
|---|---|---|---|---|
| A. 常驻 helper 子进程（stdio JSON 行协议） | ✅ segfault 只杀 helper，主进程重启 helper + 重试一次 | ✅ 峰值独占子进程地址空间，OS 可回收 | 中 | **推荐**。与 ocr-helper 同构（§6-D6 统一协议），骨架可复用 `stcore::helper` |
| B. 每页 fork/exec 子进程 | ✅ 最强 | ✅ | 中高 | spawn 开销 10–50ms/页 × 连续翻页可感；进程表抖动 |
| C. 保持现状 + 前端定期保存/降级渲染 | ❌ | ❌ | 零 | 仅缓解症状：渲染上限降到 2048 宽、单 Tab 单页、渲染结果即时落盘复用。作为过渡已部分具备 |

## 3. 内存峰值分析（方案 A 下的收益）

- 位图公式：`宽 × 高 × 4B`。4096 宽 A4 ≈ 90 MB、A0 ≈ 254 MB。
- helper 独占地址空间 → 峰值不影响主进程 RSS；页渲染完 PNG 编码后立即 free 位图，子进程 RSS 回落。
- 主进程只持有压缩 PNG（几百 KB–几 MB）与文字层数组，多 Tab 内存可控。
- 可加护栏：helper 单请求超时（如 10s）+ 位图尺寸上限（宽/高 ≤ 8192，超出拒绝并降采样请求）。

## 4. 方案 A 实施要点清单

1. **协议**（与 ocr-helper 同构，stdio JSON 行，三段式）：
   - 请求：`{"id":1,"op":"render_page"|"text_layer"|"bbox","pdf":"<path>","page":3,"width":4096}`（大文件不进制内联，传路径）
   - 成功响应：`{"id":1,"ok":true,"png_base64":"…","width":…,"height":…}` / `{"id":1,"ok":true,"pages":[{"text":"…","bbox":[x0,y0,x1,y1]}]}`
   - 错误响应：`{"id":1,"ok":false,"error":"…"}`；helper 崩溃 = stdout EOF → 主进程重启 helper 后重试一次，再失败则前端展示降级占位。
2. **生命周期**：懒启动（首个 PDF 预览时 spawn）＋ 空闲退出（60s 无请求）＋ 互斥（同一 helper 串行，`catch_unwind` 包请求处理防 Rust 侧 panic 污染）。
3. **binary**：独立 crate `pdf-helper`（对标 `ocr-helper`），依赖 pdfium-render；主程序不再直接依赖 pdfium-render（提取侧 pdf-extract 保留在主进程，纯 Rust 无风险）。
4. **打包 resources**：libpdfium.so 路径解析改为两处探测（helper 自身目录 → 系统路径）；deb/AppImage 的 resources 清单加入 `pdf-helper` 二进制 + libpdfium；Windows 侧 dll 同目录。
5. **stcore 接口**：`stcore::preview` 的 PDF 渲染函数改调 helper（`spawn_blocking` + 超时），调用方签名不变——transport 思路（D5/D6）在此复用。

## 5. 触发条件（何时启动实施）

满足任一即启动：
1. 真机出现 ≥1 次 PDFium segfault / PDF 预览导致整个应用退出；
2. 实测 ≥2 个大页 Tab 并发渲染，主进程 RSS 峰值 > 1.5 GB 或触发系统 OOM；
3. 引入用户可上传/打开不可信 PDF 的场景（恶意构造 PDF 概率上升）。
在此之前维持方案 C 的过渡护栏：渲染宽度上限、渲染结果缓存复用。
