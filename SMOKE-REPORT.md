# search-tool 冒烟验收报告（P0 闭环）

日期：2026-10-13 · 构建：`cargo build --workspace` debug · 状态：**全部通过**

## 1. 质量门禁

| 门禁 | 结果 |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --all-targets` | 0 warning |
| `cargo test --workspace` | **54 passed / 0 failed**（stcore lib） |
| `cargo build --workspace` | 通过（search-tool 二进制 + search_tool_lib） |

## 2. 冒烟路径（headless CLI，无 GUI 环境可跑）

```bash
./target/debug/search-tool cli testdata --index-dir /tmp/smoke-index
```

- 演示语料 `/tmp/search-tool-demo`：5 个样本（txt/md/html/eml）+ 1 个 zip（内层 2 条目）
- 扫描 6 文件 → **索引 7 篇文档**（含 `资料打包.zip#说明.txt`、`#笔记.md` 两条虚拟文档）

## 3. 三模式搜索 + 反向实测

| 用例 | 命令 | 结果 | 判定 |
|---|---|---|---|
| 模糊 AND | `search 中华人民共和国 --mode fuzzy` | 2 篇：简史.txt（5 命中）+ 生活随笔.md（4 命中，"中华烟/人民/共和国"**分散出现也命中**） | ✅ spike 语义落地 |
| 精准子串 | `search 人民共和国 --mode exact` | 仅 1 篇：简史.txt（连续子串）；生活随笔.md 的"朝鲜共和国"不命中 | ✅ |
| DSL 组合 | `search '中华人民共和国 & !崛起' --mode dsl` | 2 篇，hits 与 fuzzy 同源口径（5/4） | ✅ |
| 反向搜索 | `search 中华人民共和国 --mode fuzzy --reverse` | 5 篇 = 全库 7 篇差集（不含全部查询词者） | ✅ |
| 统计 | `cli stats` | 7 篇 / 14.8KB | ✅ |

单次搜索耗时 ~0.35s（含 jieba 词典冷启动 OnceLock 初始化）。

## 4. 本轮修复实录（接管提取层 + 22+10+7 编译错误清零）

- **DSL parser**：`word` 用 `many0` 可空匹配 → `many0(pair(...))` 零长度迭代报 Many0 错误；改 many1 语义
- **mail-parser 0.10 实证**：`body_text` 对 html part 返回**不剔 script 的脏文本**（"Visible text\ntrack()"）→ 正文策略反转为 **HTML 优先**（拿原文走自家 scraper 转换）；非邮件文本 parse=Some 但空 → 回退条件改为"提不出任何东西就退原文"
- **测试数据错误**：base64 `5L2g5aW95LiW6Ze0` 实际编码"你好**世间**"而非"你好**世界**"，mail-parser 解码正确，是测试写错
- **encoding_rs 陷阱**：`encode_from_utf8_to_vec` 写入 `dst.spare_capacity_mut()`，空 Vec 容量 0 → 静默 0 输出；必须预分配
- **zip 4.x**：`ZipWriter::new` 要求 W: Seek，Vec 无 Seek → `Cursor<Vec<u8>>` 承接；`tar::Builder` 用 `into_inner()` 而非 `finish()`
- **tantivy 0.26.2**：`add_field_value` 需 `&OwnedValue`；`searcher.doc()` 收 `DocAddress` 非 u32；`as_i64` 需 `Value` trait in scope；`TopDocs` 须 `.order_by_score()`
- **crate 命名地雷**：包名 `core` 与 std `core` 冲突，thiserror 展开 `core::fmt` 在 doctest 解析失败 → 重命名 `core` → `stcore`
- **架构分层**：app 不再直接依赖 tantivy，压缩包预览改走 `IndexManager::doc_body_by_path`
- **DSL hits 口径**：词面未按查询侧同款切分导致分散命中文文档 hits=0 → 新增 `collect_terms_tagged`，fuzzy 词面走 `seg_query_tokens`、exact 引号词用整串

## 5. 交付物

| 产物 | 路径 |
|---|---|
| headless CLI 二进制 | `search-tool/target/debug/search-tool` |
| Tauri GUI 库 | `search-tool/target/debug/libsearch_tool_lib.*`（配 `ui/dist` 前端） |
| 核心库源码 | `search-tool/stcore/src/`（extract/seg/dsl/index/search/text） |
| 冒烟语料 | `/tmp/search-tool-demo`（`cli testdata` 可随时重建） |

## 6. 下一步（P0+）

1. GUI 实机验证（需显示环境）：`cargo run` 起 Tauri 壳，验证搜索/预览/索引进度事件
2. 大规模回归：>10 万文档规模下复测 S3 分词命中率（spike 残留项）
3. OCR/office 提取层（P0+ 排期已列 27 任务卡）
