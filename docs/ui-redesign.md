# UI 设计规格（v1）

> 设计目标：对标主流桌面全文搜索工具的交互范式（主界面 / 预览 / 查找 / 统计）。
> 关键界面要素：`image3`（主界面 + 欢迎页）、`image5`（单文档预览 + 词组统计）、
> `image19`（多 Tab 预览 + 文档内查找 + 匹配列表 + minimap）、`image21`（帮助菜单 + 高级语法）

---

## 1. 主窗口布局（照 image3 逐行还原）

```text
┌──────────────────────────────────────────────────────────────────────────────────────┐
│ 文件名 - search-tool 全文搜索                                              ─ □ ✕    │ 标题栏（OS）
├──────────────────────────────────────────────────────────────────────────────────────┤
│ 文件(F)   编辑(E)   查看(V)   设置(O)   帮助(H)                                      │ ① MenuBar
├──────────────────────────────────────────────────────────────────────────────────────┤
│ ← → ▼  ↻  ✕  🗑  ★  ⚙  ↑  🎨  ℹ  ?          [ 模糊 ▼ ]                       ●    │ ② AppToolbar
├──────────────────────────────────────────────────────────────────────────────────────┤
│ [ 中华人民共和国                                    ] 🔍  [ 开始 ]  □反向结果        │ ③ SearchPanel
│ 指定目录：[📁] 全部目录 ▼                                                            │ ④ FilterRow
│ 指定类型：[📄] 全部文件类型 ▼                                                        │
├──────────────────────────────────────────┬───────────────────────────────────────────┤
│ 名称 ▲▼ │ 修改日期 ▲▼ │ 路径 │ 命中      │  [ 文件A.txt ✕ ][ 文件B.md ✕ ]            │ ⑤ ResultList
│──────────────────────────────────────────│ ┌──────────────────────────────┬──┐      │ ⑥ PreviewPane
│ 📄 故事会2018年17期.mobi  2021/12/31 ... │ │1839 中云  中雲                │▐ │      │
│ 📄 1.epub                 2021/12/31 ... │ │1850 中华苏维埃共和国 中華蘇維埃│▐ │      │ ← Minimap
│ 📄 2022海外日历.pdf       2022/04/11 ... │ │...                            │  │      │
│ ...                                       │ └──────────────────────────────┴──┘      │
│                                           │ [输入搜索关键词    ] Aa ( * ) ↑上一个 2/53 ↓下一个 │
│                                           │ ▾ 中华人民共和国（53 个结果）              │
│                                           │   1 ...上下文...中华人民共和国...          │
│                                           │ [搜索结果] [文件信息] [词组统计]           │
├──────────────────────────────────────────┴───────────────────────────────────────────┤
│ 返回 107 个结果，耗时 0.235 秒，搜索完成。    D:\path\to\selected.txt   1,234 篇 · 12.3 MB │ ⑦ StatusBar
└──────────────────────────────────────────────────────────────────────────────────────┘
```

**与原版的有意偏差**（需在交付说明中标注）：

| 原版 | 本实现 | 理由 |
|---|---|---|
| 预览是带独立标题栏的**浮层窗口** | 右侧**停靠面板**（可调宽） | 单窗口实现成本低、Tauri 下体验更一致 |
| 结果表无"命中"列，命中数在名称后堆叠小字 | 独立"命中"列 + 徽标 | 任务卡 1.5 明确要求"命中数徽标" |
| 工具栏模式下拉写死"模糊搜索" | 三态下拉（模糊/精准/高级） | 任务卡 1.4 |

---

## 2. 组件与文件所有权（并行零冲突的关键）

| # | 组件 | 文件 | 归属子项 | 允许改动的文件 |
|---|---|---|---|---|
| ② | AppToolbar | `ui/src/components/shell/AppToolbar.tsx` | 5 | 同目录 `shell.css` |
| ② | MenuBar | `ui/src/components/shell/MenuBar.tsx` | 5 | `shell.css` |
| ③ | SearchPanel | `ui/src/components/search/SearchPanel.tsx` | 1 | `search/search.css` |
| ④ | FilterRow | `ui/src/components/search/FilterRow.tsx` | 1 | `search/search.css` |
| ⑤ | ResultList | `ui/src/components/results/ResultList.tsx` | 2 | `results/results.css` |
| ⑤ | SelectionBar | `ui/src/components/results/SelectionBar.tsx` | 2 | `results/results.css` |
| ⑥ | PreviewPane | `ui/src/components/preview/PreviewPane.tsx` | 4 | `preview/preview.css` |
| ⑥ | Minimap | `ui/src/components/preview/Minimap.tsx` | 4 | `preview/preview.css` |
| — | IndexAdminDialog | `ui/src/components/index/IndexAdminDialog.tsx` | 3 | `index/index.css` |
| — | SettingsDialog | `ui/src/components/shell/SettingsDialog.tsx` | 5 | `shell.css` |
| — | StatusBar | `ui/src/components/shell/StatusBar.tsx` | 5 | `shell.css` |
| — | WelcomePage | `ui/src/components/shell/WelcomePage.tsx` | 5 | `shell.css` |
| — | ThemeProvider | `ui/src/components/shell/ThemeProvider.tsx` | 5 | `shell.css` |

**共享文件（只读，勿改）**：

- `ui/src/types.ts` — 全部接口契约（改需同步 `app/src/commands/dto.rs`）
- `ui/src/util.ts` — `collectTerms` / `highlightRanges` / `highlightHtml` / `formatDate` / `formatSize`
- `ui/src/App.tsx` — 组装器（状态机 + 事件分发）；组件 props 若需变更，**在任务书上提出**，由集成负责人改
- `ui/src/api/*.ts` — 每个子项一份，可改自己那份
- `app/src/commands/dto.rs` — 共享 DTO
- `app/src/commands/mod.rs` — `handler()` 命令登记表

**子组件不建新文件**：一个子项内部的子组件写在同文件内（如 PreviewPane 内含 DocTabs/DocBody/FindBar/MatchPanel/FileInfoPanel/WordStatsPanel）。

---

## 3. 子项验收细则

### 子项 1：搜索模式与 DSL（任务卡 1.4 / 1.5）

**SearchPanel**
- 输入框 `class="search-input"`（App.tsx 的 Ctrl+L 与 `edit.findInDoc` 依赖此类名）
- 左侧模式下拉：`模糊` / `精准` / `高级`，`class="mode-select"`
- 右侧"开始"按钮（`class="btn primary"`）→ `onSubmit()`（手动提交走 0 延迟）
- 反向结果开关 `□反向结果`
- 输入框内 ↑/↓ → `onArrowNav(±1)`；Enter → `onSubmit()`
- 占位文案随模式变化：
  - fuzzy：`输入关键词，多个词以空格分隔，需全部命中`
  - exact：`输入完整关键词，完全匹配`
  - dsl：`支持 & | ! () 语法，如：test & (hello|"this is") !that`
- 索引态指示灯 `●`（`dot on/busy/warn` 由 `indexState` 决定）

**FilterRow**
- 两个下拉，数据源 `api.listIndexedRoots()` / `api.listFileTypes()`（子项 3 提供）
- 首项为 `全部目录` / `全部文件类型`，值为 `null`
- 类型下拉显示 `name`，回传 `name`（后端按名解析扩展名）

**title^2 boost（Rust 侧，`stcore/src/search/mod.rs`）**
- `map_to_tantivy` 中 title 字段子句权重 ×2.0（tantivy `BoostQuery`）
- 验收：文件名命中排在正文命中之前（构造 title 命中 + body 命中的对拍用例）

### 子项 2：结果交互与文件操作（任务卡 2.1–2.5）
- 表头三列可点排序（`名称`/`修改日期`↑↓ 切换 `name`/`mtime`；默认 `relevance`）
- 行：`<tr class="selected">` 为活动行；Ctrl+click 切换、Shift+click 区间选择
- 悬停 1.2s 出浮层显示 `snippet`（`class="hover-preview"`），移出即消
- 右键菜单六项：打开 / 定位目录 / 复制路径 / 回收站删除 / 彻底删除 / 复制到… / 移动到…
  - 危险操作（彻底删除）必须二次确认
- `SelectionBar` 仅当 `paths.length >= 1` 时显示
- 本子项需实现 `app/src/commands/files.rs` 全部 7 个命令
- 导出 xlsx：`dest` 为空时由前端用 `tauri-plugin-dialog` 的 save 对话框取路径

### 子项 3：索引管理（任务卡 3.1–3.5）
- 类型表 CRUD（内置类型标 `builtin`，删除按钮禁用）
- 目录规则：三态 `include/exclude/inherit` + glob 模式输入 + 实时预览匹配结果
- 存储目录迁移：迁移中禁用搜索；完成后刷新 `getStorageInfo`
- 定时更新三态：`从不 / 每 N 分钟 / 每天 HH:mm`
- 重建：订阅 `index-progress` 事件（`{done, total, current}`），显示进度条 + 取消按钮
- 本子项需补齐 `migrate_storage` / `cancel_rebuild` / `set_index_paused` 与 `schedule.rs` 接线

### 子项 4：预览交互全家桶（任务卡 4.1–4.7）
- **Tab 栏**：每个结果点击生成一个 Tab；`✕` 关闭；关闭活动 Tab 后激活相邻
- **正文区**：行号 + 命中高亮（`<mark>`）；`line-height` 与行号严格对齐
- **minimap**：右侧 12px 窄条，按 `matchOffsets` 比例画刻度；点击定位
- **文档内查找栏**：输入框 `class="preview-find-input"`（App.tsx 的 `edit.findInDoc` 依赖）+ `Aa` 大小写 + `( * )` 正则 + `↑上一个 n/N ↓下一个`
- **匹配列表面板**：按词项分组（`matchGroups`），每条显示行号 + 上下文（命中词高亮）
- **底部三 Tab**：`搜索结果`（匹配列表）/ `文件信息`（`getFileInfo`）/ `词组统计`（`wordStats`，词 + 次数）
- **三段 offset 铁律**：高亮、minimap、匹配列表必须来自同一份 `matchGroups` 结果
- 性能门禁（S9）：1MB 文本 minimap <50ms、词组统计 <200ms
- `stcore/src/preview.rs` 的 `match_groups` 升级 aho-corasick 单遍扫描

### 子项 5：应用外壳（任务卡 5.1–5.6）
- MenuBar 五组菜单，动作经 `onAction(MenuAction)` 派发（枚举见 `types.ts`）
- 帮助菜单含高级搜索语法速查（照 image21）
- AppToolbar 图标组 + 索引态指示灯
- SettingsDialog：皮肤三态 / 开机启动 / 关闭到托盘 / 全量热键表（可改绑）
- 托盘三态（显示/隐藏/退出）；全局热键唤醒
- 皮肤：`ThemeProvider` 落 `document.documentElement.dataset.theme`；`base.css` 已定义 `--c-*` 令牌与 `:root[data-theme='dark']`
- 单实例：`app/src/commands/shell.rs::setup_single_instance_socket`（Unix socket 查询转发）
- 本子项需补齐 `apply_global_hotkeys` 的真实注册与冲突回滚

---

## 4. CSS 约定

- 设计令牌全部来自 `ui/src/styles/base.css` 的 `--c-*` / `--h-*` / `--font-*`，**禁止硬编码颜色**
- 应用外壳栅格：`.app`（5 行）+ `.app-main`（2 列，`single` 时 1 列）
- 通用类：`.btn` / `.btn.primary` / `.empty` / `.dot` / `.scroll-y` / `.error-bar`
- 各子项样式写自己的 `*.css`，由组件直接 `import`；不建全局样式

---

## 5. 验收命令（每个子项自测必须通过）

```bash
cd /workspace/search-tool
cargo fmt --all && cargo clippy --workspace --all-targets   # 必须 0 warning
cargo test --workspace                                       # 必须全绿
cd ui && pnpm build                                          # tsc -b + vite build 必须通过
```
