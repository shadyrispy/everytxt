# P6 批交付报告：A 类偏差修复 + B3 词组统计筛选

**日期**：2026-10-14
**范围**：缺口清单 A1–A6 + B3（7 项），四线并行
**依据**：功能规格清单 × 实现现状（A 类 = 方案声称 ✅ 但实现未落地）

---

## 0. 门禁

| 项 | 结果 |
|---|---|
| `cargo fmt --all` | clean |
| `cargo clippy --workspace --all-targets` | **0 warning** |
| `cargo test --workspace` | **191 passed / 0 failed**（本批前 186；5 ignored 为基准/长测类） |
| `pnpm build` | ✅ 14.3s |
| CLI 端到端冒烟 | 扫描 9 / 索引 9；docx「量子计算」✓、中文 CID pdf「倒排索引」✓、xlsx「人民好酒」✓ |

---

## 1. A1 · 实时更新—— 唯一「依赖已装、功能为零」项

**问题**：`notify = "8"` 已在 `stcore/Cargo.toml:26`、`app/Cargo.toml:27`，但源码零调用（dead dependency），文件改动不更新索引。

**实现**：
- 新增 `stcore/src/watch.rs`（142 行）：`RootWatcher`
  - `notify::RecommendedWatcher` + `RecursiveMode::Recursive`
  - **250ms 自实现去抖**：回调投 mpsc，后台线程 `recv_timeout(DEBOUNCE)` 循环，超时且 dirty 才触发一次（合并突发写入）
  - 过滤 `EventKind::Access(_)`（纯读不触发）；不存在的根 `eprintln` 跳过不 panic
  - `update_roots()` 热切换（不重建线程）；`Drop` 置停止标志
- `app/src/commands/index_admin.rs`
  - `pub fn restart_watcher(app)`：读 `Config.indexed_roots` 启动/热切换；`SEARCH_TOOL_WATCH=0|false` 禁用（启动打印状态）
  - 回调复用既有 `run_scheduled_index`（内含 `ctl.running` 互斥 + `index-progress` 事件），不另写索引逻辑
  - 挂接点：roots 增删 `:325`、存储迁移 `:567`
- `app/src/lib.rs:50`：`setup` 内 `restart_watcher(app.handle())`

**语义（如实登记）**：变化 → 去抖 → 对索引根跑一轮 `index_directory_controlled`。scanner 自带 `mtime/size` 跳过（`skipped = total - indexed`），故**只重提取变化文件**；非逐文件 upsert，未变化文件不重提取。

**实测**：
```
test watch::tests::watch_fires_on_file_create ... ok      ← 沙箱 inotify 可用，非 #[ignore]
test watch::tests::start_and_update_missing_roots_is_safe ... ok
```

---

## 2. A3/A4/A6 · 预览查看三件套（3.6.5 / 3.6.6 / 3.6.10）

- `PreviewPane` 新增可选 props `view: PreviewViewOpts` / `onViewChange`，缺省 `{lineNumbers:true, wrap:false, fontSize:13}`（对外行为与改动前一致）
- **A6 行号开关**：`no-lineno` 修饰类，关闭时正文左移对齐
- **A3 自动换行**：`wrap` 修饰类 → `white-space: pre-wrap; word-break: break-word`；关闭保持 `pre` 横向滚动
- **A4 缩放**：行高由字号派生 `lineHeight = Math.round(fontSize * 1.5)`，经内联 CSS 变量 `--preview-line-h` / `--preview-font-size` 贯通**占用高度 / 行高 / 虚拟化窗口**三者（避免滚动错位）；`Ctrl+滚轮` 用 `useEffect` + `addEventListener('wheel', h, {passive:false})` 绕过 React 被动监听限制；clamp 8..32；无 `onViewChange` 时退化为组件内 state

---

## 3. A2 · 全局快捷键表 13 组

`ui/src/App.tsx` keydown 处理：

| 键 | 行为 | 键 | 行为 |
|---|---|---|---|
| Ctrl+L | 聚焦搜索 | Ctrl+E | 定位当前活动文件 |
| Ctrl+S | 导出 xlsx | Ctrl+T | 复制选中到目录 |
| Ctrl+Shift+C | 复制路径 | Ctrl+M | 移动到目录 |
| Ctrl+A | 全选结果 | Delete | 移入回收站 |
| Ctrl+C / Ctrl+X | 复制/剪切（Linux 降级为复制路径 + 提示，R9） | F3 / Shift+F3 | 结果导航 |
| Ctrl+F | 文档内查找 | Alt+U | 手动更新索引（`dir` 或首个索引根兜底） |
| Alt+S | 搜索本文件 | Alt+G | 搜索全部（聚焦搜索框） |
| Ctrl+↑ / Ctrl+↓ | 缩放 ±1px | Ctrl+0 | 重置字号 |
| F1 | 帮助（打开项目主页） | Escape | 清除选择 |

`inField` 守卫排除输入框冲突（Ctrl+C 在输入框内保留系统文本复制）。

**菜单**（`MenuBar.tsx`）：`MenuItemDef.checked` 支持勾选态；查看菜单加「显示行号(✓) / 自动换行(✓) / 放大 / 缩小 / 重置字号」；编辑菜单加「复制名称」。

**显式未接（注释 TODO）**：`Ctrl+N`（待 B1 多窗口）、`Ctrl+U`（待 B6 检测更新）、`Alt+Q`（后端 `global-shortcut` 已实现，前端不处理）。

---

## 4. A5 · 复制文件名称

- `app/src/commands/files.rs`：`file_name_of()` 纯函数（先按虚拟路径分隔符 `#` 取末段，再取 `file_name`，兼容 `pkg.zip#inner/readme.md` → `readme.md`）+ `copy_file_names` 命令 + 3 单测
- `commands/mod.rs:32` 注册；`SelectionBar.tsx` 加「复制名称」按钮（复用 `guard` 模式）
- 单测：`file_name_of_plain_path` / `file_name_of_archive_virtual_path` / `file_name_of_bare_string`

---

## 5. B3 · 词组统计 2/3/4 字筛选

- 后端 `word_stats` 加 `min_len: Option<usize>` → `preview::word_stats_with(text, top_n, min_len.unwrap_or(2))`（能力早已存在，此前未暴露）
- `api/preview.ts` `wordStats(path, mode, topN?, minLen?)`
- 统计 Tab 四档按钮「自动 / 2 字词 / 3 字词 / 4 字词」：后端按 `min_len` 取数（topN=500），前端用 `[...word].length === n` **精确过滤**（后端 min_len 是下界，非精确长度）

---

## 6. 缺口关闭对照

| # | 项 | 状态 |
|---|---|---|
| A1 | 实时更新 | ✅ 关闭 |
| A2 | 热键表 13 组 | ✅ 关闭（Ctrl+N/U 顺延 B1/B6） |
| A3 | 自动换行 | ✅ 关闭 |
| A4 | 放大缩小 | ✅ 关闭 |
| A5 | 复制文件名称 | ✅ 关闭 |
| A6 | 显示行号开关 | ✅ 关闭 |
| B3 | 词组统计筛选 | ✅ 关闭 |
| B1 / B2 / B4 / B5 / B6 | 多窗口 / CLI 唤起 / 类型选择器 / i18n / 检测更新 | ⬜ 未做 |

**缺口计数：12 → 6**（缺口清单已标注状态更新，无内部矛盾）

---

## 7. 已知限制（登记）

| 项 | 说明 |
|---|---|
| 监听范围 | 仅 `Config.indexed_roots`；未加入索引的目录不监听 |
| 触发粒度 | 去抖后整根重扫（scanner mtime/size 跳过未变文件）；大根首轮开销仍在 |
| GUI 端到端 | 本环境无显示 → watcher 以单测覆盖（实测通过），**GUI 层未真机验证** |
| Linux Ctrl+C/X | 系统剪贴板文件语义降级（R9 既有约束），现以提示如实告知 |
| F1 | 降级为打开项目主页（MenuBar 语法面板 state 未外部暴露） |

---

## 8. 下一步（按既定顺序）

1. **B1 新建搜索多窗口** + **B2 CLI `/s /d /e` 唤起界面呈现结果**（需协议/并发设计，一批）
2. B4 类型选择器（查找/浏览）· B5 i18n · B6 检测更新（方案列为 P2）
