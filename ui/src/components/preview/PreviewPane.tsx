/**
 * 预览窗格（子项 4 所有权）
 * 按规格
 *   [文件名 Tab ×N]                          ← ① 多文档 Tab 栏
 *   ┌ 行号 + 正文(<mark> 命中高亮) ┬ minimap ┐ ← ② 正文区 + ③ 关键词分布窄条
 *   [输入搜索关键词] Aa 全词 ( * ) ↑上一个 n/N ↓下一个 ← ④ 文档内查找栏
 *   ▾ 词项（N 个结果）+ 匹配条目             ← ⑤ 匹配列表
 *   [搜索结果] [文件信息] [词组统计]          ← ⑥ 底部三 Tab
 *
 * 三段 offset 铁律：正文高亮、minimap、匹配列表全部来自后端 `matchGroups`
 * 的同一份 MatchItem（行内 UTF-8 字节偏移）。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { CSSProperties, ReactNode } from 'react'
import { listen } from '@tauri-apps/api/event'
import type {
  DocTab,
  DocumentPayload,
  FileInfo,
  FindMatch,
  MatchGroup,
  PreviewViewOpts,
  RebuildProgress,
  SearchMode,
  WordStat,
} from '../../types'
import { errMsg, formatDate, formatSize } from '../../util'
import { findInDoc, getDocument, getFileInfo, matchGroups, wordStats } from '../../api/preview'
import Minimap from './Minimap'
import OoxmlView from './OoxmlView'
import PdfPageView from './PdfPageView'
import './preview.css'

export interface PreviewPaneProps {
  tabs: DocTab[]
  activeTab: string | null
  query: string
  mode: SearchMode
  /** 文档内查找栏聚焦代际（App 菜单 edit.findInDoc 时 +1） */
  findTick?: number
  onTabActivate: (id: string) => void
  onTabClose: (id: string) => void
  /** 预览显示选项（App 持有并下发）；缺省 { lineNumbers: true, wrap: false, fontSize: 13 } */
  view?: PreviewViewOpts
  /** 预览区局部回写（仅 Ctrl+滚轮缩放使用） */
  onViewChange?: (patch: Partial<PreviewViewOpts>) => void
}

/** 视图缺省值（与冻结契约一致；App 未下发 view 时回退到此） */
const DEFAULT_VIEW: PreviewViewOpts = { lineNumbers: true, wrap: false, fontSize: 13 }
/** 字号范围 clamp */
const FONT_MIN = 8
const FONT_MAX = 32
/** 视口上下各预渲染的行数 */
const OVERSCAN = 200
/** 文档缓存上限（LRU 式淘汰，防长会话内存膨胀） */
const DOC_CACHE_MAX = 32

/* ------------------------------ 偏移工具 ------------------------------ */

/** 单字符 UTF-8 字节长度 */
function utf8Len(ch: string): number {
  const cp = ch.codePointAt(0) ?? 0
  if (cp < 0x80) return 1
  if (cp < 0x800) return 2
  if (cp < 0x10000) return 3
  return 4
}

/** UTF-8 字节偏移 → JS 字符串索引（UTF-16 code unit） */
function byteToIndex(s: string, byteOffset: number): number {
  if (byteOffset <= 0) return 0
  let bytes = 0
  let idx = 0
  for (const ch of s) {
    if (bytes >= byteOffset) break
    bytes += utf8Len(ch)
    idx += ch.length
  }
  return idx
}

/**
 * 逐行行首 UTF-8 字节偏移 + 全文字节数（与后端 line_starts 同口径）。
 *
 * 用 charCodeAt 索引循环（避免 for..of 的逐码点字符串分配），1MB 文本单遍 <10ms。
 */
function analyzeText(text: string): { starts: number[]; bytes: number } {
  const starts = [0]
  let bytes = 0
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i)
    if (c === 0x0a) {
      // '\n'：占 1 字节，其后为新行行首
      bytes += 1
      starts.push(bytes)
    } else if (c < 0x80) {
      bytes += 1
    } else if (c < 0x800) {
      bytes += 2
    } else if (c >= 0xd800 && c <= 0xdbff) {
      // 代理对高位：一个 4 字节码点
      bytes += 4
      i++
    } else {
      bytes += 3
    }
  }
  return { starts, bytes }
}

interface Seg {
  /** 行内起始 UTF-8 字节偏移 */
  start: number
  /** 行内结束 UTF-8 字节偏移 */
  end: number
  /** 是否为"当前匹配"（查找栏 n/N） */
  current: boolean
}

/** 把一行文本按命中区间切片，命中包 <mark>，当前命中用 .match-current */
function renderSegments(text: string, segs: Seg[]): ReactNode[] {
  if (segs.length === 0) return [text]
  const idx = segs
    .map((s) => ({ start: byteToIndex(text, s.start), end: byteToIndex(text, s.end), current: s.current }))
    .filter((s) => s.end > s.start)
    .sort((a, b) => a.start - b.start)
  const out: ReactNode[] = []
  let pos = 0
  let key = 0
  for (const s of idx) {
    const a = Math.max(s.start, pos)
    const b = Math.max(s.end, pos)
    if (a > pos) out.push(text.slice(pos, a))
    if (b > a) {
      out.push(
        <mark key={key++} className={s.current ? 'match-current' : undefined}>
          {text.slice(a, b)}
        </mark>,
      )
    }
    pos = Math.max(pos, b)
  }
  if (pos < text.length) out.push(text.slice(pos))
  return out
}

type PanelTab = 'search' | 'info' | 'stats' | 'page' | 'ooxml'

/* ------------------------------ 文档缓存 ------------------------------ */

/** LRU 读：命中后重置为最近使用 */
function cacheGet(c: Map<string, DocumentPayload>, k: string): DocumentPayload | undefined {
  const v = c.get(k)
  if (v === undefined) return undefined
  c.delete(k)
  c.set(k, v)
  return v
}

/** LRU 写：超上限时淘汰最久未使用条目 */
function cacheSet(c: Map<string, DocumentPayload>, k: string, v: DocumentPayload): void {
  c.delete(k)
  c.set(k, v)
  while (c.size > DOC_CACHE_MAX) {
    const oldest = c.keys().next().value
    if (oldest === undefined) break
    c.delete(oldest)
  }
}

export default function PreviewPane({
  tabs,
  activeTab,
  query,
  mode,
  findTick = 0,
  onTabActivate,
  onTabClose,
  view,
  onViewChange,
}: PreviewPaneProps) {
  /* ------------------------------ 视图选项 ------------------------------ */
  const lineNumbers = view?.lineNumbers ?? DEFAULT_VIEW.lineNumbers
  const wrap = view?.wrap ?? DEFAULT_VIEW.wrap
  const [localFontSize, setLocalFontSize] = useState(DEFAULT_VIEW.fontSize)
  /** 生效字号：有 onViewChange（App 受控）时以 props.view 为准 */
  const fontSize = view?.fontSize ?? localFontSize
  /** 派生行高（虚拟化基准 + 行高 + 占位 div 三处共用，必须一致） */
  const lineHeight = Math.round(fontSize * 1.5)

  /* ------------------------------ 文档状态 ------------------------------ */
  const [doc, setDoc] = useState<DocumentPayload | null>(null)
  const [docLoading, setDocLoading] = useState(false)
  const [docError, setDocError] = useState<string | null>(null)
  const docCache = useRef<Map<string, DocumentPayload>>(new Map())
  /** 缓存失效代际：索引变更（重建/索引/迁移结束）时 +1，触发活动文档重拉 */
  const [cacheEpoch, setCacheEpoch] = useState(0)

  /* ------------------------------ 命中状态 ------------------------------ */
  const [groups, setGroups] = useState<MatchGroup[]>([])

  /* ---------------------------- 文档内查找栏 ---------------------------- */
  const [needle, setNeedle] = useState('')
  const [caseSensitive, setCaseSensitive] = useState(false)
  const [wholeWord, setWholeWord] = useState(false)
  const [useRegex, setUseRegex] = useState(false)
  const [findMatches, setFindMatches] = useState<FindMatch[]>([])
  const [findIdx, setFindIdx] = useState(0)

  /* -------------------------------- 面板 -------------------------------- */
  const [panelTab, setPanelTab] = useState<PanelTab>('search')
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set())
  const [activeLine, setActiveLine] = useState<number | null>(null)
  const [info, setInfo] = useState<FileInfo | null>(null)
  const [stats, setStats] = useState<WordStat[] | null>(null)
  /** 词组长度筛选档位：auto 不过滤，2/3/4 精确字数 */
  const [statMode, setStatMode] = useState<'auto' | '2' | '3' | '4'>('auto')

  /* -------------------------------- 视图 -------------------------------- */
  const [scrollTop, setScrollTop] = useState(0)
  const [viewportH, setViewportH] = useState(0)
  const [minimapRatio, setMinimapRatio] = useState(0)
  const bodyRef = useRef<HTMLDivElement | null>(null)
  const findInputRef = useRef<HTMLInputElement | null>(null)
  const rafRef = useRef(0)

  const activePath = activeTab ?? tabs[0]?.path ?? null
  /** PDF 文件：启用"页视图"Tab（PDFium 缺失时组件内自行提示不可用） */
  const isPdf = !!activePath && activePath.toLowerCase().endsWith('.pdf')
  /** OOXML 文件：启用"文档视图"Tab（高保真 Canvas 渲染，P4-D） */
  const ooxmlKind: 'docx' | 'xlsx' | 'pptx' | null = (() => {
    const p = activePath?.toLowerCase() ?? ''
    if (/\.(docx|docm)$/.test(p)) return 'docx'
    if (/\.(xlsx|xlsm|xlsb)$/.test(p)) return 'xlsx'
    if (/\.(pptx|pptm|ppsx)$/.test(p)) return 'pptx'
    return null
  })()

  /* --------- 索引变更 → 文档缓存整体失效（重建/索引/迁移结束事件） --------- */
  useEffect(() => {
    let un: (() => void) | null = null
    let disposed = false
    void (async () => {
      const u = await listen<RebuildProgress>('index-progress', (ev) => {
        const p = ev.payload
        if (p && p.current !== '') return // 进行中，不收尾
        docCache.current.clear()
        setCacheEpoch((e) => e + 1)
      })
      if (disposed) u()
      else un = u
    })()
    return () => {
      disposed = true
      if (un) un()
    }
  }, [])

  /* -------------------- 拉取文档（活动 Tab / 缓存失效时） -------------------- */
  useEffect(() => {
    if (!activePath) {
      setDoc(null)
      return
    }
    const cached = cacheGet(docCache.current, activePath)
    if (cached) {
      setDoc(cached)
      setDocError(null)
      setDocLoading(false)
      return
    }
    let alive = true
    setDocLoading(true)
    setDocError(null)
    setDoc(null)
    getDocument(activePath)
      .then((d) => {
        if (!alive) return
        cacheSet(docCache.current, activePath, d)
        setDoc(d)
      })
      .catch((e: unknown) => {
        if (alive) setDocError(errMsg(e))
      })
      .finally(() => {
        if (alive) setDocLoading(false)
      })
    return () => {
      alive = false
    }
  }, [activePath, cacheEpoch])

  /* 切 Tab：复位滚动、查找命中、面板数据 */
  useEffect(() => {
    if (bodyRef.current) bodyRef.current.scrollTop = 0
    setScrollTop(0)
    setMinimapRatio(0)
    setFindMatches([])
    setFindIdx(0)
    setActiveLine(null)
    setInfo(null)
    setStats(null)
  }, [activePath])

  /* ----------- 拉取匹配列表：query 非空才调；不重拉文档 ----------- */
  useEffect(() => {
    if (!activePath || !query.trim()) {
      setGroups([])
      return
    }
    let alive = true
    matchGroups(activePath, query, mode)
      .then((g) => {
        if (alive) setGroups(g)
      })
      .catch(() => {
        if (alive) setGroups([])
      })
    return () => {
      alive = false
    }
  }, [activePath, query, mode])

  /* -------------------------- 文档内再搜索（debounce） -------------------------- */
  useEffect(() => {
    if (!activePath || !needle) {
      setFindMatches([])
      setFindIdx(0)
      return
    }
    let alive = true
    const h = setTimeout(() => {
      findInDoc(activePath, needle, { caseSensitive, wholeWord, regex: useRegex })
        .then((m) => {
          if (!alive) return
          setFindMatches(m)
          setFindIdx(0)
        })
        .catch(() => {
          if (!alive) return
          setFindMatches([])
          setFindIdx(0)
        })
    }, 200)
    return () => {
      alive = false
      clearTimeout(h)
    }
  }, [activePath, needle, caseSensitive, wholeWord, useRegex])

  /* ------------- 菜单 edit.findInDoc → 打开（聚焦）文档内查找栏 ------------- */
  useEffect(() => {
    if (findTick <= 0) return
    findInputRef.current?.focus()
    findInputRef.current?.select()
  }, [findTick])

  /* ---------------------- 面板数据（按需懒加载） ---------------------- */
  useEffect(() => {
    if (!activePath) return
    let alive = true
    if (panelTab === 'info') {
      getFileInfo(activePath)
        .then((i) => alive && setInfo(i))
        .catch(() => alive && setInfo(null))
    } else if (panelTab === 'stats') {
      // minLen：auto→2（后端最小长度），2/3/4 档同值；精确字数过滤在前端做
      const minLen = statMode === 'auto' ? 2 : Number(statMode)
      wordStats(activePath, mode, 500, minLen)
        .then((s) => alive && setStats(s))
        .catch(() => alive && setStats(null))
    }
    return () => {
      alive = false
    }
  }, [activePath, panelTab, statMode, mode])

  /* ------------------------------ 派生数据 ------------------------------ */
  const analysis = useMemo(
    () => (doc ? analyzeText(doc.text) : { starts: [0], bytes: 0 }),
    [doc],
  )
  const lines = useMemo(() => (doc ? doc.text.split('\n') : []), [doc])
  const totalLines = lines.length

  /* 行号 → 命中区间（行内字节偏移，来自 matchGroups） */
  const lineMarks = useMemo(() => {
    const m = new Map<number, Seg[]>()
    for (const g of groups) {
      for (const it of g.matches) {
        const seg: Seg = { start: it.start, end: it.end, current: false }
        const arr = m.get(it.line)
        if (arr) arr.push(seg)
        else m.set(it.line, [seg])
      }
    }
    return m
  }, [groups])

  /* 当前查找命中（全局字节偏移 → 行内偏移） */
  const curFind = findMatches[findIdx] ?? null

  /* minimap 刻度：与后端 match_offsets 同口径（行首字节 + 行内偏移） */
  const offsets = useMemo(() => {
    if (!doc || groups.length === 0) return []
    const ls = analysis.starts
    const out: number[] = []
    const seen = new Set<number>()
    for (const g of groups) {
      for (const it of g.matches) {
        if (it.line < 1 || it.line > ls.length) continue
        const o = ls[it.line - 1] + it.start
        if (!seen.has(o)) {
          seen.add(o)
          out.push(o)
        }
      }
    }
    out.sort((a, b) => a - b)
    return out
  }, [doc, groups, analysis])

  /* ------------------------------ 交互回调 ------------------------------ */
  const scrollToLine = useCallback(
    (line: number) => {
      const b = bodyRef.current
      if (!b || line < 1) return
      b.scrollTop = Math.max(
        0,
        (line - 1) * lineHeight - b.clientHeight / 2 + lineHeight / 2,
      )
    },
    [lineHeight],
  )

  /* 当前查找命中变化 → 滚动到对应行 */
  useEffect(() => {
    if (curFind) scrollToLine(curFind.line)
  }, [curFind, scrollToLine])

  const onSeek = useCallback((ratio: number) => {
    const b = bodyRef.current
    if (!b) return
    const max = b.scrollHeight - b.clientHeight
    b.scrollTop = Math.max(0, Math.min(1, ratio)) * Math.max(0, max)
  }, [])

  const onBodyScroll = useCallback(() => {
    if (rafRef.current) return
    rafRef.current = requestAnimationFrame(() => {
      rafRef.current = 0
      const b = bodyRef.current
      if (!b) return
      setScrollTop(b.scrollTop)
      const max = b.scrollHeight - b.clientHeight
      setMinimapRatio(max > 0 ? b.scrollTop / max : 0)
    })
  }, [])

  useEffect(() => {
    const b = bodyRef.current
    if (!b || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(() => setViewportH(b.clientHeight))
    ro.observe(b)
    setViewportH(b.clientHeight)
    return () => {
      ro.disconnect()
      if (rafRef.current) cancelAnimationFrame(rafRef.current)
    }
  }, [activePath, doc])

  /* -------- Ctrl+滚轮缩放：wheel 需 passive:false 才能 preventDefault -------- */
  useEffect(() => {
    const b = bodyRef.current
    if (!b) return
    const handler = (e: WheelEvent) => {
      if (!e.ctrlKey) return
      e.preventDefault()
      const step = e.deltaY < 0 ? 1 : -1
      const next = Math.min(FONT_MAX, Math.max(FONT_MIN, fontSize + step))
      if (next === fontSize) return
      if (onViewChange) onViewChange({ fontSize: next })
      else setLocalFontSize(next)
    }
    b.addEventListener('wheel', handler, { passive: false })
    return () => b.removeEventListener('wheel', handler)
  }, [fontSize, onViewChange, doc])

  const gotoFind = (delta: number) => {
    if (findMatches.length === 0) return
    setFindIdx((i) => (i + delta + findMatches.length) % findMatches.length)
  }

  const toggleGroup = (term: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev)
      if (next.has(term)) next.delete(term)
      else next.add(term)
      return next
    })
  }

  /* ------------------------------ 虚拟化窗口 ------------------------------ */
  const first = Math.max(0, Math.floor(scrollTop / lineHeight) - OVERSCAN)
  const last = Math.min(
    totalLines,
    Math.ceil((scrollTop + Math.max(viewportH, 1)) / lineHeight) + OVERSCAN,
  )

  const rows: ReactNode[] = []
  for (let i = first; i < last; i++) {
    const lineNo = i + 1
    const segs = (lineMarks.get(lineNo) ?? []).slice()
    if (curFind && curFind.line === lineNo && lineNo - 1 < analysis.starts.length) {
      const base = analysis.starts[lineNo - 1]
      segs.push({ start: curFind.start - base, end: curFind.end - base, current: true })
    }
    rows.push(
      <div
        key={i}
        className={'preview-line' + (activeLine === lineNo ? ' active' : '')}
      >
        {lineNumbers && <span className="preview-lineno">{lineNo}</span>}
        <span className="preview-linetext">{renderSegments(lines[i] ?? '', segs)}</span>
      </div>,
    )
  }

  /* -------------------------------- 渲染 -------------------------------- */
  return (
    <section className="preview-pane">
      {/* ① 多文档 Tab 栏 */}
      <div className="preview-tabs" role="tablist">
        <div className="preview-tabs-scroll">
          {tabs.map((t) => (
            <div
              key={t.id}
              role="tab"
              tabIndex={0}
              aria-selected={t.id === activePath}
              className={'preview-tab' + (t.id === activePath ? ' active' : '')}
              title={t.path}
              onClick={() => onTabActivate(t.id)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault()
                  onTabActivate(t.id)
                }
              }}
            >
              <span className="preview-tab-title">{t.title}</span>
              <button
                type="button"
                className="preview-tab-close"
                title="关闭"
                aria-label="关闭"
                onClick={(e) => {
                  e.stopPropagation()
                  onTabClose(t.id)
                }}
              >
                ✕
              </button>
            </div>
          ))}
        </div>
      </div>

      {doc?.truncated && (
        <div className="preview-notice">
          文件过大，仅显示前 {formatSize(analysis.bytes)}（原始 {formatSize(doc.totalBytes)}）
        </div>
      )}

      {/* ②③ 正文 + minimap */}
      <div className="preview-main">
        {!activePath ? (
          <div className="empty">点击左侧结果以预览</div>
        ) : docError ? (
          <div className="empty">加载失败：{docError}</div>
        ) : docLoading || !doc ? (
          <div className="empty">加载中…</div>
        ) : (
          <>
            <div
              className={
                'preview-body' + (lineNumbers ? '' : ' no-lineno') + (wrap ? ' wrap' : '')
              }
              ref={bodyRef}
              onScroll={onBodyScroll}
              style={
                {
                  '--preview-line-h': `${lineHeight}px`,
                  '--preview-font-size': `${fontSize}px`,
                } as CSSProperties
              }
            >
              <div style={{ height: first * lineHeight }} />
              {rows}
              <div style={{ height: (totalLines - last) * lineHeight }} />
            </div>
            <Minimap
              totalBytes={doc.truncated ? analysis.bytes : doc.totalBytes}
              offsets={offsets}
              scrollRatio={minimapRatio}
              onSeek={onSeek}
            />
          </>
        )}
      </div>

      {/* ④ 文档内查找栏 */}
      <div className="preview-findbar">
        <input
          ref={findInputRef}
          className="preview-find-input"
          type="text"
          value={needle}
          placeholder="输入搜索关键词。"
          spellCheck={false}
          autoComplete="off"
          onChange={(e) => setNeedle(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault()
              gotoFind(e.shiftKey ? -1 : 1)
            } else if (e.key === 'Escape') {
              e.currentTarget.blur()
            }
          }}
        />
        <button
          type="button"
          className={'find-toggle' + (caseSensitive ? ' on' : '')}
          title="区分大小写"
          onClick={() => setCaseSensitive((v) => !v)}
        >
          Aa
        </button>
        <button
          type="button"
          className={'find-toggle' + (wholeWord ? ' on' : '')}
          title="全词匹配"
          onClick={() => setWholeWord((v) => !v)}
        >
          全词
        </button>
        <button
          type="button"
          className={'find-toggle' + (useRegex ? ' on' : '')}
          title="正则表达式"
          onClick={() => setUseRegex((v) => !v)}
        >
          ( * )
        </button>
        <button type="button" className="find-nav" title="上一个" onClick={() => gotoFind(-1)}>
          ↑ 上一个
        </button>
        <span className="find-count">
          {findMatches.length ? `${findIdx + 1}/${findMatches.length}` : '0/0'}
        </span>
        <button type="button" className="find-nav" title="下一个" onClick={() => gotoFind(1)}>
          ↓ 下一个
        </button>
      </div>

      {/* ⑤ 匹配列表 / 文件信息 / 词组统计 / PDF 页视图（P3-B）/ 文档视图（P4-D） */}
      <div className="preview-panel">
        {panelTab === 'ooxml' && activePath && ooxmlKind && (
          <OoxmlView path={activePath} query={query} kind={ooxmlKind} />
        )}
        {panelTab === 'page' &&
          activePath &&
          (isPdf ? (
            <PdfPageView path={activePath} query={query} mode={mode} />
          ) : (
            <div className="preview-empty">当前文件不是 PDF</div>
          ))}
        {panelTab === 'search' &&
          (groups.length === 0 ? (
            <div className="preview-empty">无匹配</div>
          ) : (
            groups.map((g) => {
              const isCollapsed = collapsed.has(g.term)
              return (
                <div className="match-group" key={g.term}>
                  <div className="match-group-head" onClick={() => toggleGroup(g.term)}>
                    <span className="match-caret">{isCollapsed ? '▸' : '▾'}</span>
                    <span className="match-term">{g.term}</span>
                    <span className="match-count">（{g.count} 个结果）</span>
                  </div>
                  {!isCollapsed && (
                    <div className="match-items">
                      {g.matches.map((it, idx) => (
                        <div
                          className={'match-item' + (activeLine === it.line ? ' active' : '')}
                          key={idx}
                          onClick={() => {
                            setActiveLine(it.line)
                            scrollToLine(it.line)
                          }}
                        >
                          <span className="match-item-no">{idx + 1}</span>
                          <span className="match-item-text">
                            {renderSegments(it.lineText, [
                              { start: it.start, end: it.end, current: false },
                            ])}
                          </span>
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              )
            })
          ))}

        {panelTab === 'info' &&
          (info ? (
            <table className="info-panel">
              <tbody>
                <tr>
                  <th>名称</th>
                  <td>{info.name}</td>
                </tr>
                <tr>
                  <th>路径</th>
                  <td title={info.path}>{info.path}</td>
                </tr>
                <tr>
                  <th>大小</th>
                  <td>{formatSize(info.size)}</td>
                </tr>
                <tr>
                  <th>修改时间</th>
                  <td>{formatDate(info.mtime)}</td>
                </tr>
                <tr>
                  <th>格式</th>
                  <td>
                    {info.kind}
                    {info.ext ? `（${info.ext}）` : ''}
                  </td>
                </tr>
                <tr>
                  <th>行数</th>
                  <td>{info.lines}</td>
                </tr>
                <tr>
                  <th>编码</th>
                  <td>{info.encoding ?? '未知'}</td>
                </tr>
              </tbody>
            </table>
          ) : (
            <div className="preview-empty">加载中…</div>
          ))}

        {panelTab === 'stats' && (
          <div className="stat-filter" role="group" aria-label="词组长度筛选">
            {(['auto', '2', '3', '4'] as const).map((m) => (
              <button
                key={m}
                type="button"
                className={'stat-filter-btn' + (statMode === m ? ' on' : '')}
                onClick={() => setStatMode(m)}
              >
                {m === 'auto' ? '自动' : `${m} 字词`}
              </button>
            ))}
          </div>
        )}

        {panelTab === 'stats' &&
          (stats ? (
            (() => {
              // auto 不过滤；2/3/4 档用码点精确长度过滤（后端 minLen 只是最小值）
              const n = statMode === 'auto' ? 0 : Number(statMode)
              const shown = n === 0 ? stats : stats.filter((w) => [...w.word].length === n)
              return shown.length === 0 ? (
                <div className="preview-empty">无统计数据</div>
              ) : (
                shown.map((w, i) => (
                  <div className="stat-row" key={w.word}>
                    <span className="stat-no">{i + 1}</span>
                    <span className="stat-word">{w.word}</span>
                    <span className="stat-count">{w.count}</span>
                  </div>
                ))
              )
            })()
          ) : (
            <div className="preview-empty">加载中…</div>
          ))}
      </div>

      {/* ⑥ 底部 Tab（PDF 多"页视图"、OOXML 多"文档视图"） */}
      <div className="preview-bottomtabs">
        {ooxmlKind && (
          <button
            type="button"
            className={'preview-bottomtab' + (panelTab === 'ooxml' ? ' active' : '')}
            onClick={() => setPanelTab('ooxml')}
          >
            文档视图
          </button>
        )}
        {isPdf && (
          <button
            type="button"
            className={'preview-bottomtab' + (panelTab === 'page' ? ' active' : '')}
            onClick={() => setPanelTab('page')}
          >
            页视图
          </button>
        )}
        <button
          type="button"
          className={'preview-bottomtab' + (panelTab === 'search' ? ' active' : '')}
          onClick={() => setPanelTab('search')}
        >
          搜索结果
        </button>
        <button
          type="button"
          className={'preview-bottomtab' + (panelTab === 'info' ? ' active' : '')}
          onClick={() => setPanelTab('info')}
        >
          文件信息
        </button>
        <button
          type="button"
          className={'preview-bottomtab' + (panelTab === 'stats' ? ' active' : '')}
          onClick={() => setPanelTab('stats')}
        >
          词组统计
        </button>
      </div>
    </section>
  )
}
