/**
 * 结果列表（子项 2 所有权）
 * 按规格）；
 * 行内文件类型着色图标；悬停 1.2s 上浮预览片断；Ctrl/Shift 多选；右键七操作。
 *
 * 说明（R9 / 契约）：
 * - 排序方向 `desc` 为本地视图态：`sort` 契约只有 relevance/name/mtime 无方向位，
 *   故方向仅切换本地展示顺序；行始终携带「原始下标」回调 onActivate，保证预览不错位。
 * - 「复制到…/移动到…」目标目录由后端弹框获取（前端未装 @tauri-apps/plugin-dialog），
 *   故这里 `transferFiles(..., dest='')`。
 * - 删除/移动成功后调 App 的 onRefresh(成功路径)：本地过滤结果 + 重发搜索，
 *   选择清空与激活位错峰一并由 App 收尾（见 App.onRefreshAfterOp）。
 */
import { useEffect, useMemo, useRef, useState } from 'react'
import type { MouseEvent as ReactMouseEvent, KeyboardEvent as ReactKeyboardEvent } from 'react'
import type { FileOpResult, SearchResult, SearchMode, SortKey } from '../../types'
import { baseName, errMsg, formatDate, formatSize } from '../../util'
import * as files from '../../api/files'
import './results.css'

export interface ResultListProps {
  results: SearchResult[]
  activeIdx: number
  selected: ReadonlySet<string>
  sort: SortKey
  query: string
  mode: SearchMode
  /** R3：分页态（page 从 0 计） */
  page: number
  pageSize: number
  total: number
  partial: boolean
  onSortChange: (s: SortKey) => void
  onPageChange: (page: number) => void
  /** R3：右键菜单"添加书签"（取菜单目标第一项） */
  onAddBookmark: (path: string) => void
  onActivate: (idx: number) => void
  onSelectionChange: (next: Set<string>) => void
  onOpenFile: (path: string) => void
  /** 删除/移动成功后的收尾（传入实际失效的路径） */
  onRefresh: (removed: string[]) => void
}

/** 结果列显隐（R3：表头右侧下拉控制；本地视图态，不入后端契约） */
interface ColFlags {
  name: boolean
  mtime: boolean
  path: boolean
  size: boolean
  hits: boolean
}

const COL_LABELS: Array<{ key: keyof ColFlags; label: string }> = [
  { key: 'name', label: '名称' },
  { key: 'mtime', label: '修改日期' },
  { key: 'path', label: '路径' },
  { key: 'size', label: '大小' },
  { key: 'hits', label: '命中' },
]

type SortDir = 'asc' | 'desc'

/* --------------------------- 扩展名 → 图标 --------------------------- */

const EBOOK = new Set(['mobi', 'epub', 'azw3', 'azw', 'fb2', 'djvu']) // 左栏以电子书为主
const DOCTYPE = new Set(['doc', 'docx', 'wps', 'odt', 'rtf'])
const SHEET = new Set(['xls', 'xlsx', 'csv', 'ods'])
const SLIDE = new Set(['ppt', 'pptx', 'odp'])
const ARCHIVE = new Set(['zip', 'rar', '7z', 'tar', 'gz', 'bz2'])
const MARKUP = new Set(['html', 'htm', 'mht', 'eml', 'xml'])
const PLAIN = new Set(['md', 'log', 'json', 'yml', 'yaml', 'ini', 'conf'])

function extOf(name: string): string {
  const i = name.lastIndexOf('.')
  return i >= 0 ? name.slice(i + 1).toLowerCase() : ''
}

function ftypeKind(ext: string): string {
  if (ext === 'pdf') return 'pdf'
  if (ext === 'txt') return 'text'
  if (EBOOK.has(ext)) return 'ebook'
  if (DOCTYPE.has(ext)) return 'doc'
  if (SHEET.has(ext)) return 'sheet'
  if (SLIDE.has(ext)) return 'slide'
  if (ARCHIVE.has(ext)) return 'zip'
  if (MARKUP.has(ext)) return 'markup'
  if (PLAIN.has(ext)) return 'plain'
  return 'other'
}

function ftypeLabel(ext: string): string {
  return ext ? ext.slice(0, 4).toUpperCase() : 'FILE'
}

/** 路径省略中段（完整路径由 title 呈现）；keep 下限 4 字符防极窄时不可读 */
function middleEllipsis(s: string, max: number): string {
  if (s.length <= max) return s
  const keep = Math.max(4, max - 3)
  const head = Math.ceil(keep / 2)
  const tail = keep - head
  return `${s.slice(0, head)}...${s.slice(s.length - tail)}`
}

/* ------------------------------ 组件主体 ------------------------------ */

export default function ResultList(props: ResultListProps) {
  const {
    results,
    activeIdx,
    selected,
    sort,
    query,
    page,
    pageSize,
    total,
    partial,
    onSortChange,
    onPageChange,
    onAddBookmark,
    onActivate,
    onSelectionChange,
    onOpenFile,
    onRefresh,
  } = props

  const containerRef = useRef<HTMLDivElement>(null)
  const anchorRef = useRef<number | null>(null) // Shift 区间锚点（展示序位）
  const hoverTimer = useRef<number | null>(null)

  const [dir, setDir] = useState<SortDir>('asc')
  const [hover, setHover] = useState<{ idx: number; top: number; left: number } | null>(null)
  const [menu, setMenu] = useState<{ x: number; y: number; targets: string[] } | null>(null)
  const [cols, setCols] = useState<ColFlags>({ name: true, mtime: true, path: true, size: true, hits: true })
  const [colOpen, setColOpen] = useState(false)

  /* 列显隐下拉：点外部 / Esc 关闭 */
  useEffect(() => {
    if (!colOpen) return
    const close = () => setColOpen(false)
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') close()
    }
    window.addEventListener('mousedown', close)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('mousedown', close)
      window.removeEventListener('keydown', onKey)
    }
  }, [colOpen])

  /* 展示顺序：relevance 保持后端序；name/mtime 本地排序以支持方向切换 */
  const order = useMemo(() => {
    const idx = results.map((_, i) => i)
    if (sort === 'relevance') return idx
    idx.sort((a, b) => {
      let cmp: number
      if (sort === 'name') {
        // 与名称列展示保持一致：按 basename 排序（后端 title 在 Windows 上含目录段）
        const va = baseName(results[a].name).toLowerCase()
        const vb = baseName(results[b].name).toLowerCase()
        cmp = va < vb ? -1 : va > vb ? 1 : 0
      } else {
        cmp = results[a].mtime - results[b].mtime
      }
      return dir === 'asc' ? cmp : -cmp
    })
    return idx
  }, [results, sort, dir])

  /* 活动行滚动进视野 */
  useEffect(() => {
    if (activeIdx < 0) return
    containerRef.current?.querySelector('.result-row.selected')?.scrollIntoView({ block: 'nearest' })
  }, [activeIdx, order])

  /* 悬停计时器清理 */
  useEffect(
    () => () => {
      if (hoverTimer.current !== null) window.clearTimeout(hoverTimer.current)
    },
    [],
  )

  /* 右键菜单：点外部 / 滚动 / Esc 关闭 */
  useEffect(() => {
    if (!menu) return
    const close = () => setMenu(null)
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') close()
    }
    window.addEventListener('mousedown', close)
    window.addEventListener('scroll', close, true)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('mousedown', close)
      window.removeEventListener('scroll', close, true)
      window.removeEventListener('keydown', onKey)
    }
  }, [menu])

  function clickSort(k: 'name' | 'mtime') {
    if (sort === k) {
      setDir((d) => (d === 'asc' ? 'desc' : 'asc'))
    } else {
      onSortChange(k)
      setDir('asc')
    }
  }

  function handleRowClick(e: ReactMouseEvent, origIdx: number, pos: number, path: string) {
    containerRef.current?.focus()
    if (e.shiftKey && anchorRef.current !== null) {
      const lo = Math.min(anchorRef.current, pos)
      const hi = Math.max(anchorRef.current, pos)
      const next = new Set<string>()
      for (let i = lo; i <= hi; i++) next.add(results[order[i]].path)
      onSelectionChange(next)
      onActivate(origIdx)
      return
    }
    if (e.ctrlKey || e.metaKey) {
      const next = new Set(selected)
      if (next.has(path)) next.delete(path)
      else next.add(path)
      onSelectionChange(next)
      onActivate(origIdx)
      anchorRef.current = pos
      return
    }
    onSelectionChange(new Set())
    onActivate(origIdx)
    anchorRef.current = pos
  }

  function handleContextMenu(e: ReactMouseEvent, origIdx: number, path: string) {
    e.preventDefault()
    onActivate(origIdx)
    const targets = selected.size > 0 ? Array.from(selected) : [path]
    setMenu({ x: e.clientX, y: e.clientY, targets })
  }

  function toggleCol(key: keyof ColFlags) {
    setCols((c) => {
      const next = { ...c, [key]: !c[key] }
      // 至少保留一列
      return Object.values(next).some(Boolean) ? next : c
    })
  }

  function handleKeyDown(e: ReactKeyboardEvent) {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return
    e.preventDefault()
    if (order.length === 0) return
    const cur = order.indexOf(activeIdx)
    let pos: number
    if (cur === -1) pos = e.key === 'ArrowDown' ? 0 : order.length - 1
    else pos = e.key === 'ArrowDown' ? Math.min(order.length - 1, cur + 1) : Math.max(0, cur - 1)
    onActivate(order[pos])
  }

  function onRowEnter(e: ReactMouseEvent, idx: number) {
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect()
    if (hoverTimer.current !== null) window.clearTimeout(hoverTimer.current)
    hoverTimer.current = window.setTimeout(() => {
      hoverTimer.current = null
      /* U5：卡片 420px 宽 / ~110px 高，贴近视口右/下缘时向内夹取防裁切 */
      setHover({
        idx,
        top: Math.min(rect.bottom + 4, Math.max(0, window.innerHeight - 130)),
        left: Math.min(rect.left, Math.max(0, window.innerWidth - 430)),
      })
    }, 1200)
  }

  function clearHover() {
    if (hoverTimer.current !== null) {
      window.clearTimeout(hoverTimer.current)
      hoverTimer.current = null
    }
    setHover(null)
  }

  /* ------------------------------ 文件操作 ------------------------------ */

  function notify(r: FileOpResult, action: string) {
    if (r.failed.length > 0) {
      const f = r.failed[0]
      window.alert(`${action}：成功 ${r.ok} 项，失败 ${r.failed.length} 项。\n首个错误：${f.path}\n${f.error}`)
    }
  }

  /** 部分失败不中断：从 targets 中剔除失败项，返回实际失效的路径 */
  function removedPaths(r: FileOpResult, targets: string[]): string[] {
    const failed = new Set(r.failed.map((f) => f.path))
    return targets.filter((p) => !failed.has(p))
  }

  async function doOp(kind: string, targets: string[]) {
    setMenu(null)
    if (targets.length === 0) return
    try {
      switch (kind) {
        case 'open':
          await files.openFile(targets[0])
          break
        case 'openWith':
          await files.openWith(targets[0])
          break
        case 'reveal':
          await files.revealInFolder(targets[0])
          break
        case 'copy':
          await files.copyPaths(targets)
          break
        case 'copyName':
          await files.copyFileNames(targets)
          break
        case 'trash': {
          const r = await files.moveToTrash(targets)
          notify(r, '移入回收站')
          onRefresh(removedPaths(r, targets))
          break
        }
        case 'delete':
          if (!window.confirm(`确定彻底删除 ${targets.length} 项？此操作不可恢复。`)) return
          {
            const r = await files.deletePermanently(targets)
            notify(r, '彻底删除')
            onRefresh(removedPaths(r, targets))
          }
          break
        case 'copyTo':
          notify(await files.transferFiles(targets, '', false), '复制')
          break
        case 'moveTo': {
          const r = await files.transferFiles(targets, '', true)
          notify(r, '移动')
          onRefresh(removedPaths(r, targets))
          break
        }
      }
    } catch (e) {
      window.alert(`操作失败：${errMsg(e)}`)
    }
  }

  /* -------------------------------- 渲染 -------------------------------- */

  const sortTh = (label: string, k: 'name' | 'mtime', cls: string) => {
    const active = sort === k
    return (
      <th className={`th-sort ${cls}${active ? ' active' : ''}`} onClick={() => clickSort(k)} title={`按${label}排序`}>
        {label}
        <span className="sort-arrow">{active ? (dir === 'asc' ? '▲' : '▼') : ''}</span>
      </th>
    )
  }

  const emptyText = query.trim() ? '未找到匹配结果' : '输入关键词开始搜索'

  const menuStyle = menu
    ? {
        /* U3：菜单 10 项 + 3 分隔线 ≈ 285px 高，防溢出夹取按 310 计 */
        left: Math.min(menu.x, Math.max(0, window.innerWidth - 176)),
        top: Math.min(menu.y, Math.max(0, window.innerHeight - 310)),
      }
    : undefined

  return (
    <div className="result-list" ref={containerRef} tabIndex={0} onKeyDown={handleKeyDown}>
      <table className="result-table">
        <thead>
          <tr>
            {cols.name && sortTh('名称', 'name', 'th-name')}
            {cols.mtime && sortTh('修改日期', 'mtime', 'th-mtime')}
            {cols.path && <th className="th-plain">路径</th>}
            {cols.size && <th className="th-plain th-size">大小</th>}
            {cols.hits && <th className="th-plain th-hits">命中</th>}
            <th className="th-plain th-cols">
              <button
                type="button"
                className="col-toggle-btn"
                title="显示 / 隐藏列"
                onClick={(e) => {
                  e.stopPropagation()
                  setColOpen((v) => !v)
                }}
              >
                ⚙
              </button>
              {colOpen && (
                <div className="col-dropdown" onMouseDown={(e) => e.stopPropagation()}>
                  {COL_LABELS.map((c) => (
                    <label key={c.key} className="col-option">
                      <input
                        type="checkbox"
                        checked={cols[c.key]}
                        onChange={() => toggleCol(c.key)}
                      />
                      {c.label}
                    </label>
                  ))}
                </div>
              )}
            </th>
          </tr>
        </thead>
        {results.length > 0 && (
          <tbody>
            {order.map((origIdx, pos) => {
              const r = results[origIdx]
              const ext = extOf(r.name)
              const active = origIdx === activeIdx
              const multi = selected.has(r.path)
              return (
                <tr
                  key={r.path}
                  className={`result-row${active ? ' selected' : ''}${multi ? ' multi' : ''}`}
                  onClick={(e) => handleRowClick(e, origIdx, pos, r.path)}
                  onContextMenu={(e) => handleContextMenu(e, origIdx, r.path)}
                  onDoubleClick={() => onOpenFile(r.path)}
                  onMouseEnter={(e) => onRowEnter(e, origIdx)}
                  onMouseLeave={clearHover}
                >
                  {cols.name && (
                    <td className="col-name">
                      <span className="ftype" data-kind={ftypeKind(ext)}>
                        {ftypeLabel(ext)}
                      </span>
                      <span className="fname">{baseName(r.name)}</span>
                    </td>
                  )}
                  {cols.mtime && <td className="col-mtime" title={formatDate(r.mtime)}>{formatDate(r.mtime)}</td>}
                  {cols.path && (
                    <td className="col-path" title={r.path}>
                      {middleEllipsis(r.path, 56)}
                    </td>
                  )}
                  {cols.size && <td className="col-size">{formatSize(r.size)}</td>}
                  {cols.hits && (
                    <td className="col-hits">{r.hits > 1 ? <span className="hit-badge">{r.hits}</span> : null}</td>
                  )}
                </tr>
              )
            })}
          </tbody>
        )}
      </table>

      {results.length === 0 && <div className="empty">{emptyText}</div>}

      {/* R3：底部分页条 */}
      {total > 0 && (
        <div className="pager">
          <button
            type="button"
            className="btn"
            disabled={page === 0}
            onClick={() => onPageChange(page - 1)}
          >
            上一页
          </button>
          <span className="pager-info">
            第 {page + 1} / {Math.max(1, Math.ceil(total / pageSize))} 页 · 共 {total} 条
          </span>
          <button
            type="button"
            className="btn"
            disabled={(page + 1) * pageSize >= total}
            onClick={() => onPageChange(page + 1)}
          >
            下一页
          </button>
          {partial && <span className="pager-partial">结果可能不完整</span>}
        </div>
      )}

      {hover && results[hover.idx] && (
        <div className="hover-preview" style={{ left: hover.left, top: hover.top }}>
          <div className="hover-preview-name">{baseName(results[hover.idx].name)}</div>
          <div className="hover-preview-snippet">{results[hover.idx].snippet || '（无预览片段）'}</div>
        </div>
      )}

      {menu && (
        <div className="result-menu" style={menuStyle} role="menu" onMouseDown={(e) => e.stopPropagation()}>
          <button className="menu-item" onClick={() => void doOp('open', menu.targets)}>
            打开
          </button>
          <button className="menu-item" onClick={() => void doOp('openWith', menu.targets)}>
            打开方式…
          </button>
          <button className="menu-item" onClick={() => void doOp('reveal', menu.targets)}>
            定位到文件
          </button>
          <button className="menu-item" onClick={() => void doOp('copy', menu.targets)}>
            复制路径
          </button>
          <button className="menu-item" onClick={() => void doOp('copyName', menu.targets)}>
            复制文件名称
          </button>
          <div className="menu-sep" />
          <button
            className="menu-item"
            onClick={() => {
              setMenu(null)
              onAddBookmark(menu.targets[0])
            }}
          >
            添加书签
          </button>
          <div className="menu-sep" />
          <button className="menu-item" onClick={() => void doOp('trash', menu.targets)}>
            移入回收站
          </button>
          <button className="menu-item danger" onClick={() => void doOp('delete', menu.targets)}>
            彻底删除
          </button>
          <div className="menu-sep" />
          <button className="menu-item" onClick={() => void doOp('copyTo', menu.targets)}>
            复制到…
          </button>
          <button className="menu-item" onClick={() => void doOp('moveTo', menu.targets)}>
            移动到…
          </button>
        </div>
      )}
    </div>
  )
}
