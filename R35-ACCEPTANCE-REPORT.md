# R3/R5 合并验收与端到端审计报告

> 日期：2026-10-14 ｜ 范围：架构 v2 路线 R3（搜索层补齐）+ R5（服务化 + CLI/i18n）合并后的全量验收与审计
> 结论：**验收通过**。全量门禁 237 passed / 0 failed / clippy 0 / fmt clean / pnpm build EXIT=0 / CLI 冒烟 5/5 通过。审计发现 2 项问题（1 项已修复，1 项为文档性勘误），均已闭环。

---

## 1. 门禁结果

| 门禁项 | 命令 | 结果 |
|---|---|---|
| 前端构建 | `pnpm build`（ui/） | ✅ EXIT=0，4.45s，产物 dist/assets/（最大 chunk docx 814.79 kB，gzip 245 kB，超 500 kB 告警为既有 code-split 建议，非错误） |
| 格式 | `cargo fmt --all` | ✅ 无 diff |
| Lint | `cargo clippy --workspace --all-targets` | ✅ 0 warning / 0 error |
| 测试 | `cargo test --workspace` | ✅ **237 passed / 0 failed / 4 ignored**（上波基线 214，R3/R5 净增 23 个用例） |

测试增量构成：stcore search（regex 候选集熔断/mtime RangeQuery/分页）+15、store（SQLite history/bookmarks）+5、service（SocketClient roundtrip，本轮新增）+1、app（slash 预处理/store 命令透传）+2。

## 2. CLI 冒烟明细（headless，索引 /tmp/r5-idx，7 篇演示语料）

| # | 命令 | 验证点 | 结果 |
|---|---|---|---|
| T1 | `search-tool '/s' '量子计算'` | `/s` 预处理映射；主实例不在 → 本地 headless 回退 | ✅ 1 命中，0.337s，snippet 正确 |
| T2 | `search-tool cli search '算力｜变革' --mode dsl` | Regex 模式（SearchMode::Regex 经 DSL 入口） | ✅ 1 命中 4 次（alternation 展开正确） |
| T3 | `search-tool cli search '量子计算' --mode exact` | 精确模式 | ✅ 1 命中 1 次 |
| T4 | `search-tool --search '大模型'` | 长参数透传（等价 `/s`） | ✅ 2 命中 |
| T5 | `search-tool cli stats` | 索引统计 | ✅ 7 篇 / 25.4 KB |

T1 的 socket 分支同时验证了失败语义：无主实例时 `Connection refused` → 打印"主实例未在运行…回退本地搜索" → 退出码 0，与 R5 契约一致。

## 3. 前端集成核验（trust-but-verify）

| 核验项 | 方法 | 结果 |
|---|---|---|
| App.tsx 挂载点 | 源码定位 L844 | ✅ `<ExternalSearchListener onSearch={…}>` 已挂载，回调 setQuery/setFileType/setDir/setSubmitTick 四态齐备 |
| i18n key 对账 | node strip-types 解析双侧 TS | ✅ zh-CN 64 keys ↔ en-US 64 keys，**双向零缺失** |
| socket 协议 handler | shell.rs L481-502 | ✅ `op:"search"` → emit `external-search` {query,drive,ext} + show/set_focus |
| i18n 组件接线 | grep useI18n | ✅ MenuBar / StatusBar / WelcomePage 三组件已接入 |

## 4. 审计发现与处置

| # | 发现 | 级别 | 处置 |
|---|---|---|---|
| A1 | **SocketClient 无成功路径测试**：原 service.rs 单测只覆盖 InprocBackend；SocketClient 仅 tests/r5_service.rs 覆盖连接失败回退，成功 roundtrip（请求序列化字段名 op/query/mode/drive/ext ↔ 响应解析）无守卫，协议字段漂移不会被测试发现 | P2 | ✅ **已修复**：新增单测 `socket_client_roundtrip_stats_and_search`（UnixListener 桩 server 复刻 shell.rs JSON 行协议，验证 stats 解析 / search"已呈现"空 hits 语义 / 死连接 Err 三断言）。修复后 237 passed |
| A2 | CLI 子命令层级文档性偏差：R5 交付描述里 `search-tool search <q>` 直白写法与实际 clap 结构（`cli search`，顶层 `--search` 才是 `/s` 等价）不符，首次冒烟按错误写法执行报 `unrecognized subcommand` | P3（文档） | ✅ 已在 T2/T3 复跑确认；正式入口以 `--help` 为准：`/s 关键字` ⇔ `--search KW`；`/d` ⇔ `--drive`；`/e` ⇔ `--ext`；headless 搜索子命令为 `cli search <query> --mode fuzzy｜exact｜dsl` |

## 5. 架构 v2 路线总账（R1–R5）

| 路线 | 内容 | 状态 |
|---|---|---|
| R1 | 命令全面 async（线程模型修正）+ 索引写侧 IndexQueue 单线程串行 / 读侧 IndexManager 只读 + ReloadPolicy::OnCommit + rayon 限流池 | ✅ 已交付并验收 |
| R2 | schema v3（mtime/size INDEXED｜FAST｜STORED）+ scanner 迁 stcore/src/indexer/ + IndexTask 枚举 | ✅ 已交付并验收 |
| R3 | SearchMode::Regex（候选集 5000 篇/256MB/5s 熔断）+ SearchFilter mtime_from/to + 分页 SearchResponse{hits,total,page,page_size,partial} + SQLite 历史/书签 + 前端分页条/历史下拉/九档时间区间/书签面板 | ✅ 本报告验收通过 |
| R4 | 7z/bz2/chm/binary strings 提取器 + 类型表对齐（djvu 登记缺口、wps/永中不加、rar 视 unrar 编译结果） | ✅ 已交付并验收 |
| R5 | transport trait（SearchService/InprocBackend/SocketClient）+ CLI `/s /d /e` 预处理与回退 + shell.rs socket op:"search" → external-search 事件 + i18n 框架与三组件接线 + pdf-helper 评估 | ✅ 本报告验收通过 |

## 6. 剩余事项（下一步候选）

1. **D7 schema v3 的 FAST 属性收益量化**：R2 已落地，但缺 time-range 查询的 P50 基准数字（对照 R3 FilterRow 上线前后），建议补一组 before/after 微基准。
2. **PDF helper 进程化决策**：docs/pdf-helper-assessment.md 已出评估，尚未拍板"拆进程 vs 线程池"的最终实施方案。
3. **OCR helper 端到端联调**：ocr-helper crate 已 pin rustc 1.95 独立构建，尚未跑通 P4-A 主程序接入后的真实扫描件 bbox 全链路（沙箱内缺 GPU 语料）。
4. **chunk >500 kB 告警**：docx/xlsx/pptx 预览组件走动态 import 可进一步瘦身首屏，属优化项非缺陷。
5. **GUI 冒烟**：本环境无显示服务器，App.tsx 集成仅经 build + 源码核验；真机 GUI 冒烟（external-search 事件实际驱动 UI）需在有显示环境补做。

---

**总判定：R3/R5 合并无阻塞问题，可进入下一路线（D7 量化 / PDF helper 拍板 / OCR 全链路联调任选）。**
