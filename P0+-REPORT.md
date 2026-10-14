# P0+ 体验对齐批 · 交付与验收报告

> 批次：P0+ 五子项（27 张任务卡）
> 纪律：5 子项并行开发 → 集成编译 → 全量验收
> 完成时间：2026-10-13

---

## 1. 结论

**P0+ 五子项全部落地，工作区全绿，可执行文件已产出。**

| 质量门禁 | 结果 |
|---|---|
| `cargo fmt --all --check` | ✅ clean |
| `cargo clippy --workspace --all-targets` | ✅ **0 warning** |
| `cargo test --workspace` | ✅ **120 passed / 0 failed**（app 18 + stcore 93 + 集成 9；另 1 个基准测试 `#[ignore]`） |
| `cargo build -p search-tool --bin search-tool` | ✅ 产出 `target/debug/search-tool`（402 MB debug） |
| `cd ui && pnpm build` | ✅ `tsc -b` + vite 通过，dist 23.12 KB CSS / 281 KB JS |
| headless 冒烟（4 路径） | ✅ 模糊 / 精准 / 高级 / 反向 全部符合预期 |

**规模**：Rust 24 文件 6 558 行 · TS/TSX 24 文件 3 943 行 · CSS 1 665 行 · Tauri 命令 **38 个** · 前端组件 **13 个**

---

## 2. 五子项交付内容

### 子项 1：搜索模式与 DSL（+0.2 人月）
- `SearchPanel.tsx`：模式下拉（模糊/精准/高级）、`search-input`、开始按钮（手动提交 0 延迟）、反向结果开关、↑/↓ 结果导航、按模式切换占位文案、索引态指示灯
- `FilterRow.tsx`：**指定目录** / **指定文件类型** 两个下拉，数据源接子项 3
- `stcore/search`：`SortKey` + `SearchFilter`（目录前缀 + 扩展名白名单，压缩包内层按**包路径**判定）+ **title^2 boost**
- **语义变更（重要）**：原实现 `(Must, body) + (Should, title)` 导致"仅文件名命中"的文件无法召回；改为 `(Should, body_AND) + (Should, Boost(title_AND, 2.0))`。**字段内 AND 语义不变**（模糊搜索仍是"所有分词全部命中"），仅新增"标题命中可单独召回并优先排序"。

### 子项 2：结果交互与文件操作（+0.3 人月）
- `files.rs` **7 个命令全部落地**：打开 / 定位目录 / 复制路径 / 回收站删除 / 彻底删除 / 复制移动（`fs_extra`）/ 导出 xlsx（`rust_xlsxwriter`，五列：名称/修改日期/路径/命中数/大小；无路径时弹系统保存对话框）
- `ResultList.tsx`：三列表头点击排序（三档 + 升降序）、**Ctrl/Shift 多选**、**1.2 s 悬停预览浮层**、右键六操作菜单、文件类型着色图标、活动行键盘导航与滚动跟随
- `SelectionBar.tsx`：批量操作条（≥1 项时出现），危险操作二次确认
- 全部批量操作"部分失败不中断"，逐条回收成败

### 子项 3：索引管理（+0.3 人月）
- `stcore/config.rs`：文件类型表（9 个内置类型种子）+ per-format **三态目录规则**（`include/exclude/inherit`）+ 定时配置，持久化 `<index_dir>/config.json`（原子写）；`compile_rules` 合并"全局规则"（`file_type_id=null` 对所有类型生效）
- `stcore/schedule.rs`：`std::thread` 三态调度（从不 / 每 N 分钟 / 每天 HH:mm），可中断 sleep，`Drop` 即停
- `scanner.rs`：规则集接入遍历过滤（`RuleSet::allows`，R10 单一事实源），新增取消/暂停受控扫描，已写部分照常 commit（**取消后索引不损坏可续**）
- `index_admin.rs`：`migrate_storage`（物理迁移 + 校验 + 失败回滚 + **运行期热切换**）、`cancel_rebuild`、`set_index_paused`、`restart_scheduler`
- `IndexAdminDialog.tsx`：五区块（文件类型 / 目录规则 / 存储 / 定时更新 / 索引构建），订阅 `index-progress` 事件显示进度条与当前文件

### 子项 4：预览交互全家桶（+0.5 人月）
- `PreviewPane.tsx` + `Minimap.tsx` 照 `image5`/`image19` 还原：**多文档 Tab**（✕ 关闭、激活相邻）、行号+命中高亮正文、**右侧 minimap 关键词分布**（点击/拖拽定位）、**文档内查找栏**（`Aa` 大小写 / `( * )` 正则 / 全词 / `↑上一个 n/N ↓下一个`）、**匹配列表面板**（按词项分组）、底部三 Tab（搜索结果 / 文件信息 / 词组统计）
- `stcore/preview.rs`：`match_groups` 升级 **aho-corasick 单遍扫描**，实测 **1 MB 文本 4.55 ms**（门禁 <50 ms）；jieba 词组统计；文档内再搜索（字面/全词/正则）
- **三段 offset 铁律**：高亮 / minimap / 匹配列表共用同一份 `MatchGroup`，不重复计算

### 子项 5：应用外壳（+0.4 人月，含整站 UI 重设计）
- 7 个外壳组件：`MenuBar`（五组菜单 + 快捷键提示 + 帮助内高级语法速查，照 `image21`）、`AppToolbar`（12 图标 + 索引指示灯，照 `image3`）、`StatusBar`（三段：状态文案 / 文件全路径 / 索引统计）、`WelcomePage`（搜索步骤/使用技巧/关于 三节，照 `image3`）、`SettingsDialog`（皮肤三态 / 开机启动 / 关闭到托盘 / 全量热键改绑）、`ThemeProvider`
- `shell.rs`：**全局热键真实注册 + 冲突回滚**、**系统托盘三态**（显示/隐藏/退出）、关闭到托盘、开机启动（`tauri-plugin-autostart`）、**CLI 单实例 Unix socket 查询转发**
- 皮肤令牌体系：`base.css` 定义 `--c-*`，`:root[data-theme='dark']` 全量覆盖，切换即时生效无重启

---

## 3. 集成阶段我做的共享文件改造

| 文件 | 改动 | 原因 |
|---|---|---|
| `app/src/state.rs` | `index_dir: PathBuf` → `RwLock<PathBuf>` + `index_dir()` / `set_index_dir()` | 让"存储目录迁移"运行期热切换（子项 3 的阻塞项） |
| `app/src/commands/*.rs` | `state.index_dir` 字段访问 → `state.index_dir()`（12 处） | 随上一条机械化迁移 |
| `app/src/commands/index_admin.rs` | `migrate_storage` 末尾增加 `set_index_dir` + 重开校验 + 失败回退旧目录 + `restart_scheduler` | 落地热切换与回滚保证 |
| `app/src/lib.rs` | `setup()` 增加 `commands::index_admin::restart_scheduler(app.handle())` | 冷启动即按已存配置拉起定时更新 |
| `stcore/tests/p0plus_search.rs` | **新增 9 个集成测试** | 覆盖目录/类型过滤、三档排序、组合过滤、限流、反向过滤、title boost 端到端 |

---

## 4. 冒烟实测（headless CLI，`SEARCH_TOOL_INDEX_DIR=/tmp/p0p-index`）

```
$ search-tool cli testdata
完成: 扫描 6 个文件, 索引 7 篇文档（含压缩包内层）

$ search-tool cli search 中华人民共和国 --mode fuzzy
返回 2 个结果，耗时 0.644 秒，搜索完成     ← 字段内 AND 语义保持
$ search-tool cli search '中华人民共和国 & !崛起' --mode dsl
返回 2 个结果，耗时 0.628 秒，搜索完成
$ search-tool cli search 人民共和国 --mode exact
返回 1 个结果，耗时 0.657 秒，搜索完成     ← 连续子串校验生效
$ search-tool cli search 共和国 --mode fuzzy --reverse
返回 4 个结果，耗时 0.640 秒，搜索完成     ← 反向差集正确
```

---

## 5. 已知偏差与缺口（如实登记）

| # | 项 | 状态 | 说明 |
|---|---|---|---|
| G1 | 预览窗格 | **有意偏差** | 常见方案为带独立标题栏的浮层窗口；本实现为右侧**停靠可调宽面板**（单窗口，Tauri 下体验更一致） |
| G2 | 结果表"命中"列 | **有意偏差** | 需求外新增列；任务卡 1.5 明确要求命中数徽标，故设为独立列 |
| G3 | 单实例 socket | 仅 Unix | Windows 命名管道分支未实现（`#[cfg(not(unix))]` 空实现），已在代码标注 TODO |
| G4 | `reveal_in_folder` | Linux 降级 | Linux 无"定位并选中"标准动作 → 降级为打开父目录（R9 已登记） |
| G5 | OCR / Office / PDF 解析 | 未纳入本批 | 属 P0 及后续批次范围；本批为"体验对齐"，格式面仍是文本类 + 压缩包 |
| G6 | 索引并发写 | 已知边界 | 重建与手动 `index_directory` 同时对同一 tantivy 目录开 writer 会触发锁；日常单流不触发 |
| G7 | `bench_match_groups_1mb` | `#[ignore]` | 性能基准不进默认单测；实测 4.55 ms |

---

## 6. 复现命令

```bash
cd /workspace/search-tool
cargo fmt --all && cargo clippy --workspace --all-targets && cargo test --workspace
cd ui && pnpm build && cd ..
cargo build -p search-tool --bin search-tool        # 产出 target/debug/search-tool

# headless 冒烟
SEARCH_TOOL_INDEX_DIR=/tmp/smoke ./target/debug/search-tool cli testdata
SEARCH_TOOL_INDEX_DIR=/tmp/smoke ./target/debug/search-tool cli search 中华人民共和国 --mode fuzzy

# GUI（需桌面环境）
./target/debug/search-tool
```

---

## 7. 下一步

1. **GUI 真机走查**：按 `docs/ui-redesign.md` §3 逐项核验视觉还原度（尤其 minimap、匹配列表、工具栏图标间距）
2. **补 G3**：Windows 命名管道；**补 G6**：索引 writer 串行化
3. **进入 P1**：Office/PDF/OCR 提取（spike S1/S5/S6 已在 P0 前完成验证，可直接移植）
4. **打包**：`tauri build` 出 deb/appimage（`tauri.conf.json` 的 bundle 配置已就绪）
