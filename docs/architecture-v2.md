# 架构设计 v2（整体优化，避免返工）

**日期**：2026-10-14
**输入**：桌面全文搜索工具的功能需求全集 × 现状代码（P0–P6 各批次交付）
**性质**：设计定稿稿 —— 确认后按 §8 演进路线批量执行

---

## 1. 设计驱动因子（功能需求 → 架构约束）

| 功能域 | 需求（含缺口） | 架构约束 |
|---|---|---|
| 搜索 | 4 模式（模糊/精准/DSL/**正则**）、反向、拼音/同义词、**时间区间**、分页、目录/类型筛选、排序、历史、书签 | schema 必须**一次定到位**；查询层可扩展 |
| 索引 | 全量/重建/手动/定时/实时监听；per-format 三态规则；格式全集（含 7z/rar/chm/OFD/WPS…）；压缩包递归；OCR | 提取是 CPU/IO 密集 → 必须与 UI **线程隔离**；FFI 库需**进程隔离** |
| 预览 | 文本虚拟化 + 高亮/minimap/词组统计；PDF 页视图 + bbox；OOXML Canvas；多 Tab | 渲染不能卡 UI；PDFium(C++) 崩溃不得带走主程序 |
| 外壳 | 托盘/热键/右击/单实例/CLI `/s /d /e`/i18n/**HTTP 搜索(暂缓)** | 单实例语义；CLI 与 GUI 同能力 → 需要**服务层抽象** |
| 非功能 | UI 永不冻结；崩溃影响面可控；体积与内存 | Tauri 主线程只做派发；长任务全部落线程池 |

## 2. 原版架构参照（一句话版）

6 进程：ATGUI(Qt+Wt Web) ⇄ ATService(索引引擎, QJsonRpc/HTTP) + ATOHelper(NCNN+Vulkan OCR) + ATDHelper2(DjVu/PDF) + ATDHelper(静态解析) + ATNativeHelper(划词)。**GUI 与引擎分离、解析与 OCR 进程隔离、一切能力 API 化。**

取其神（隔离边界 + 服务化 + API 面），不取其形（不必 6 进程——Rust 无 UB，进程数量砍半）。

---

## 3. 目标架构

```text
┌──────────── WebView 进程（系统管理） ────────────┐
│ React UI：搜索 / 结果 / 预览 / 设置 / 索引管理     │
└───────────────▲─────────────────────────────────┘
                │ Tauri IPC（全部 async 命令 + 事件）
┌───────────────┴─────────────────────────────────┐
│ app（Rust 主进程）                                │
│  commands/（薄封装，一律 async → 不阻塞主线程）      │
│  ┌─────────────────────────────────────────────┐ │
│  │ service 层（§5-D5 transport 抽象）            │ │
│  │  SearchService / IndexService / HistoryStore │ │
│  └───────┬──────────────┬──────────────┬───────┘ │
│          ▼              ▼              ▼         │
│   ┌────────────┐ ┌───────────┐ ┌────────────┐   │
│   │indexer 线程 │ │watcher 线程│ │scheduler线程│   │
│   │writer 独占  │ │(notify)   │ │(定时三态)   │   │
│   └─────┬──────┘ └─────┬─────┘ └─────┬──────┘   │
│         ▼              └──────┬──────┘          │
│   rayon 提取池(CPU 密集, N-1)  任务队列(去抖合并)    │
│   reader pool（搜索/预览读侧，commit 后 reload）    │
└──┬──────────────┬──────────────────┬────────────┘
   │ 子进程族（崩溃/内存/工具链隔离，统一 stdio JSON 协议）
┌──▼──────────┐ ┌──▼──────────┐ ┌────▼─────────────────┐
│ ocr-helper  │ │ pdf-helper  │ │ CLI（search-tool cli │
│ PP-OCRv6    │ │ PDFium 渲染/ │ │ + /s /d /e → socket  │
│ (rustc1.95) │ │ 文字层/bbox  │ │ 转发给主实例）         │
└─────────────┘ └─────────────┘ └──────────────────────┘
```

**进程职责表**

| 进程 | 职责 | 隔离理由 | 对标原版 |
|---|---|---|---|
| app（主） | UI 宿主 + service 层 + 全部业务线程 | — | ATGUI+ATService 合并 |
| ocr-helper | OCR 推理 + bbox | C++ 运行时 + rustc 1.95 工具链 + 内存峰值 | ATOHelper |
| **pdf-helper（新增，R5 评估）** | PDFium 渲染 PNG / 文字层 / bbox | C++ 崩溃隔离 + 大页内存峰值 | ATDHelper2(libPdfCore) |
| CLI | 唤起/查询/索引（经 socket 与主实例互通） | 独立入口 | atgui /s |

---

## 4. 关键架构决策（含否决项）

| # | 决策 | 理由 / 否决项 |
|---|---|---|
| D1 | **不照抄 6 进程**：单主进程 + helper 进程族 | Rust 内存安全使业务逻辑无需进程隔离；只在 **FFI 库**（OCR/PDFium）与**工具链**（rustc 1.95）处拆进程。否决"ATService 常驻独立进程"（IPC 序列化 + 部署复杂度，收益仅在多前端时出现——见 D5 留口） |
| D2 | **命令层全面 async 化**：40+ 命令改 `#[tauri::command(async)]`（sync_threadpool），重计算内部再 `spawn_blocking` | 现状全部 Blocking → 主线程冻结（已实证 tauri-macros 默认 `Blocking` 原地调用）。**这是返工风险最大的一处，必须先做** |
| D3 | **索引引擎统一**：`IndexWriter` 单线程独占（indexer 线程），读侧 `ReaderPool`（commit 后 reload）；scanner 从 app 并入 stcore（引擎与宿主分离） | 现状 scanner 自开 `IndexManager`，与 state 缓存的 mgr 是两个实例 → writer 文件锁竞争 + reader 不同步。对标原版 `IndexMgr` 单例 |
| D4 | **任务统一**：watcher / scheduler / 手动索引 → 单一**索引任务队列**（去抖合并 + `ctl.running` 互斥 + 统一事件 `{phase,done,total,current}`） | 现状三入口各自为政，靠 swap 互斥；去抖合并可防 watcher 风暴 |
| D5 | **transport 抽象**：service 层定义 trait（`Search/Index/Preview/...`），先 Inproc 直调实现；预留 `unix-socket`/`http` transport | **避免返工核心**：未来 CLI 全功能、HTTP 搜索(Web UI)、对外 API 只需换 transport，业务零改动。对标原版 QJsonRpc |
| D6 | **helper 协议统一**：stdio JSON 行协议（请求/响应/错误三段式），ocr-helper 已有，pdf-helper 同构复用 | 一套 helper 骨架（spawn/stdin/stdout/互斥/超时/catch_unwind）两个进程共用 |
| D7 | **schema v3 一次到位**：`mtime/size` → `INDEXED \| FAST`（RangeQuery + 排序必需）；其余字段不动 | 现状仅 `STORED`，时间筛选/按大小筛选做不了。**每次加字段都要全量重建** → 现在定死。用户需知晓：v3 上线一次重建 |
| D8 | **搜索层扩展**：`SearchMode::Regex`（docstore 候选集上跑正则，依托目录/类型/时间先收窄）；`SearchRequest` 加 `mtimeFrom/mtimeTo` + `page/pageSize`；历史与书签入 **SQLite**（rusqlite 已有） | 正则语义对标原版 = 对**正文文本**正则，非对分词 term；全库裸跑成本不可控，必须先过滤收窄（原版也如此提示） |
| D9 | **i18n 框架**：UI 字符串集中 `t(key)` + 资源文件（json），先 zh-CN/en-US | 原版 17 语言 ini 证明字符串全集中可行；后补语言零成本 |
| D10 | **否决/暂缓**（登记）：Web UI(HTTP 搜索)、网盘索引、划词助手、DjVu、GPU OCR、全语种 i18n、授权体系 | 价值/成本比不足；transport/http 留口后随时可加 |

---

## 5. 模块重构映射（现状 → 目标）

| 现状 | 目标 | 动作 |
|---|---|---|
| `app/src/commands/*.rs` 40+ sync 命令 | 全部 async 薄封装 | `#[tauri::command(async)]` 逐个标注；阻塞体包 `spawn_blocking` |
| `app/src/scanner.rs`（引擎逻辑在宿主层） | `stcore/src/indexer/`（枚举+管线+规则） | 移动 + `app` 只留接线；CLI 复用同一引擎 |
| `stcore/src/index/mod.rs`（单结构既写又读） | `indexer::Writer`（独占）+ `reader::Pool` | 拆分；`with_mgr` 改为 `with_reader` |
| 三入口（watch/sched/manual）各自跑 | `indexer::queue`（去抖 + 互斥 + 事件） | 新建统一队列 |
| `stcore/src/ocr.rs`（同步等 helper） | async 化 + 超时 + helper 骨架抽 `stcore/src/helper.rs` | ocr/pdf 共用 |
| 无 history/书签 | `stcore/src/store.rs`（SQLite：history/bookmarks） | rusqlite 已有 |
| `types.ts SearchRequest` | 加 `mtimeFrom/mtimeTo/page/pageSize`；`SearchMode` 加 `'regex'` | 契约一次定稿（v3 语义） |
| `config.rs` 类型表 | 与提取层能力**对齐**（删除无提取器的声明或补实现） | 消除静默 Unsupported |

---

## 6. 数据流（目标态）

**索引**：`roots → 任务队列(去抖250ms合并) → indexer 线程(独占 writer)：枚举(globset) → rayon 提取池(含 OCR/PDF 调 helper，N-1 限流) → mpsc 候选 → upsert → 周期 commit → reload reader → emit {phase,done,total,current}`

**搜索**：`UI debounce 150ms → search(async) → reader 查询(RangeQuery 时间 + facet + 分页 offset) → docstore 片段 → HitRow[]`；regex 模式 = 先过滤收窄 → 候选正文流式正则（上限熔断）

**预览**：`get_document(async, spawn_blocking) → 解码/截断`；PDF → `pdf-helper`（PNG + bbox）；OCR bbox → `ocr-helper`；OOXML → 前端 WASM（不变）

---

## 7. Schema v3（定稿）

| 字段 | 现状 | v3 |
|---|---|---|
| path | STRING\|STORED | 不变 |
| title/body | TEXT(seg)\|STORED | 不变 |
| pinyin | TEXT(plain) | 不变 |
| **size** | STORED | **INDEXED\|FAST\|STORED** |
| **mtime** | STORED | **INDEXED\|FAST\|STORED** |
| ext | STRING\|STORED | 不变 |
| schema 版本号 | 2 | **3**（open 校验，旧索引提示重建） |

---

## 8. 演进路线（每阶段独立可验收，避免半成品）

| 阶段 | 内容 | 验收 | 估时 |
|---|---|---|---|
| **R1 线程模型** | 40+ 命令 async 化；OCR 链路 spawn_blocking；rayon 池限流 N-1 | 编译+测试+**真机 GUI 冒烟清单**（索引/预览/搜索时 UI 可交互） | 1d |
| **R2 索引引擎统一** | writer/reader 拆分、scanner 入 stcore、任务队列、**schema v3 + 重建**、事件协议统一 | 三入口互斥正确；watcher 风暴合并；CLI 不回归 | 2-3d |
| **R3 搜索层补齐** | regex 模式、时间区间、分页、结果列控制、历史、书签 | 功能规格对照通过 | 2d |
| **R4 格式补齐** | 7z/rar/bz2、chm 提取器；类型表对齐；WPS/永中可行性评估；二进制 strings 提取 | 格式矩阵与官方「关于」全集对齐（除否决项） | 2d |
| **R5 服务化与外壳** | transport trait 落地（unix-socket）；CLI `/s /d /e` 完整；i18n zh/en；pdf-helper 评估 | CLI 唤起 GUI 呈现结果；外部进程可查 | 2-3d |

**总估时**：9–11 人日。R1/R2 是**不可逆决策**（先做）；R3–R5 顺序可按需调整。

## 9. 风险与回退

| 风险 | 缓解 |
|---|---|
| schema v3 全量重建（一次性） | CLI 一键重建已有；迁移提示已具备（v2→v3 沿用机制） |
| async 化后隐藏的锁竞争显性化 | R1 中逐命令审视持锁跨度；`with_reader` 只做快照 |
| helper 进程增多（ocr/pdf） | 统一骨架 + 懒启动 + 空闲退出；资源占用可控 |
| 正则搜索 DoS（灾难性回溯） | 候选集上限 + 单文件超时 + 复杂度熔断（沿用现有正则护栏） |

## 10. 决策记录（Decision Log）

| # | 日期 | 决策 | 依据 |
|---|---|---|---|
| D9 | 2026-10-14 | **pdf-helper 暂不拆进程**：维持方案 C 过渡护栏（渲染宽度上限、单 Tab 单页、渲染结果落盘复用），登记三条触发条件（真机 PDFium segfault ≥1 次 / 主进程 RSS >1.5GB / 引入不可信 PDF 场景），满足任一即启动方案 A（常驻 helper 子进程，与 ocr-helper 同构 stdio JSON 协议，方案要点见 docs/pdf-helper-assessment.md §4） | C++ FFI segfault 不可 catch_unwind；但当前无不可信 PDF 入口且真机未复现，拆进程成本（2 人日）暂不投入 |
| D10 | 2026-10-14 | **schema v3 FAST 属性收益已量化**：20k 篇 / 730 天 mtime 跨度下，时间区间查询 RangeQuery 下推 vs STORED-only 后过滤，P50 加速 **33–40×**（401ms → 10-12ms），基线（无时间条件）6.3ms。详见 docs/bench-d7-time-filter.md | 定量证据支持 v3 升级为不可逆决策；窗口收窄至 1% 时 A 仅 12ms，仍远低于后过滤 |

| D11 | 2026-10-14 | **Windows 平台适配定稿**：确认三平台（Linux/macOS/Windows）可构建运行。修复 8 项真缺陷（chmlib E0599、dirs 数据目录、unrar 头文件大小写 + build.rs host/target 混淆、chmlib-sys ffs 守卫、tauri-build icon.ico、OCR helper .exe 定位、两处 cfg 死代码），vendored 三个上游 crate（chmlib / chmlib-sys / unrar-sys，`[patch.crates-io]` + `third_party/*-patched`），随仓库提供 Windows pdfium.dll（Chromium 157.0.8100.0）。证据：mingw 交叉编译 check/test 链接全绿 + 主机门禁 237/0。详见 docs/cross-platform-assessment.md | 用户仅有 Windows 机器可测 UI；沙箱无 Windows/wine，运行时冒烟降级为 docs/windows-runbook.md §6 清单，交由真机执行 |
| D12 | 2026-10-14 | **单实例 socket 维持 Unix-only**：Windows 命名管道单实例不投入实现；CLI `/s` 在 Windows 自动回退本地 headless（main.rs 现有回退路径），`SocketClient` 保留跨平台类型 + `#[cfg(not(unix))]` 占位 impl 恒 Err | 单实例转发是体验优化非正确性需求；回退路径已存在且测试覆盖；命名管道实现约 1 人日且无当期收益。触发条件：Windows 端 CLI 转发需求出现 |
