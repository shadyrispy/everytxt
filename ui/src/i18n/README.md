# i18n 骨架（架构 v2 §4-D9 / §8-R5）

框架定稿（本目录），**非全量迁移**。已接线：`MenuBar.tsx`、`WelcomePage.tsx`；
`StatusBar.tsx` 自身无硬编码文案（`text`/`right` 由 App.tsx 组合传入，见下）。

## 结构

- `index.ts`：`t(key, vars?)`（`{1}`/`{name}` 占位替换）、`I18nProvider`（lang ← AppConfig.language，
  读失败默认 zh-CN）、`useI18n()`、`translate/interpolate`（可脱离 React 用于测试）。
- `zh-CN.ts` / `en-US.ts`：字典，`Record<string, string>`，**双语言严格同 key**。

## 集成方（App.tsx）待办

1. 根部包一层：`import { I18nProvider } from './i18n'` → `<I18nProvider>…现有树…</I18nProvider>`。
2. 挂载 CLI 唤起监听：
   `<ExternalSearchListener onSearch={(query, drive, ext) => { setQuery(query); if (ext) setFileType(ext); if (drive) setDir(drive); setSubmitTick((t) => t + 1) }} />`
3. StatusBar 文案组合改 t()（key 已在两份字典预留）：
   - `statusText`：`t('status.searching')` / `t('status.ready')` /
     `t('status.results', [results.length, (elapsedMs / 1000).toFixed(3)])`
   - `right`：`stats.available ? t('status.indexDocs', [stats.docs, stats.size]) : t('status.indexUnavailable')`
     ；stats 为 null 时 `t('status.indexLoading')`

## 其余组件迁移清单（未迁移，按优先级）

| 文件 | 含中文字符串的行数（grep 估算） | 主要 key 前缀建议 |
|---|---|---|
| ui/src/components/index/IndexAdminDialog.tsx | 128 | `idxAdmin.*` |
| ui/src/components/preview/PreviewPane.tsx | 102 | `preview.*` |
| ui/src/App.tsx（错误条/状态栏/快捷键提示） | 80 | `app.*` / `status.*` |
| ui/src/components/shell/SettingsDialog.tsx | 39 | `settings.*` |
| ui/src/components/results/ResultList.tsx | 44 | `results.*` |
| ui/src/components/results/SelectionBar.tsx | 28 | `selection.*` |
| ui/src/components/shell/AppToolbar.tsx | 25 | `toolbar.*` |
| ui/src/components/preview/PdfPageView.tsx | 19 | `pdf.*` |
| ui/src/components/search/SearchPanel.tsx | 17 | `search.*` |
| ui/src/components/preview/OoxmlView.tsx | 17 | `ooxml.*` |
| ui/src/components/preview/Minimap.tsx | 14 | `minimap.*` |
| ui/src/components/search/FilterRow.tsx | 11 | `filter.*` |

## 约定

- key 命名：`组件.区块.条目`（如 `menu.file.export`、`status.results`）；列表用 `*.1..n` 或 `*1..n` 后缀。
- **新增字符串必须 zh-CN / en-US 双语言同增**；PR 里两文件 diff 行数应一致。
- 组件内取文案一律 `const { t } = useI18n()`；勿直接 import 字典。
- 运行时构造的文案（如 MenuBar `buildMenus(view)`）把 `t` 作为参数传入函数，保持结构不变。
- en-US 缺 key 时运行时回退 zh-CN，再缺回退 key 本身（开发期可见，勿依赖）。
- 语言切换：设置页写 AppConfig.language 后提示重启（或重挂 Provider）即生效。
