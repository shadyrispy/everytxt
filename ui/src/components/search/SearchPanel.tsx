/**
 * 搜索面板（子项 1 所有权）
 * 按规格）+ 反向开关
 *
 * 即输即搜的 debounce 由 App.tsx 统一处理，本组件只负责输入、模式切换与手动提交。
 * R3：模式加"正则"；输入框聚焦且为空时下拉最近 10 条历史（点击回填并提交）。
 */
import { useEffect, useRef, useState } from 'react'
import type { ChangeEvent, KeyboardEvent } from 'react'
import type { HistoryEntry, IndexRunState, SearchMode } from '../../types'
import * as storeApi from '../../api/store'
import { formatDate } from '../../util'
import './search.css'

export interface SearchPanelProps {
  query: string
  mode: SearchMode
  reverse: boolean
  loading: boolean
  indexState: IndexRunState
  onQueryChange: (v: string) => void
  onModeChange: (m: SearchMode) => void
  onReverseChange: (v: boolean) => void
  onSubmit: () => void
  onArrowNav: (dir: 1 | -1) => void
}

/** 各模式的输入提示文案（随模式切换） */
const PLACEHOLDERS: Record<SearchMode, string> = {
  fuzzy: '输入关键词，多个词以空格分隔，需全部命中',
  exact: '输入完整关键词，完全匹配',
  dsl: '支持 & | ! () 语法，如：test & (hello|"this is") !that',
  regex: '输入正则表达式对正文匹配，如：验证码\\d{4,6}',
}

/** 模式四选项：值 → 显示名 */
const MODE_OPTIONS: Array<{ value: SearchMode; label: string }> = [
  { value: 'fuzzy', label: '模糊' },
  { value: 'exact', label: '精准' },
  { value: 'dsl', label: '高级' },
  { value: 'regex', label: '正则' },
]

/** 索引运行态 → 指示灯语义类名 */
const DOT_CLASS: Record<IndexRunState, string> = {
  idle: 'on',
  indexing: 'busy',
  paused: 'warn',
}

export default function SearchPanel({
  query,
  mode,
  reverse,
  loading,
  indexState,
  onQueryChange,
  onModeChange,
  onReverseChange,
  onSubmit,
  onArrowNav,
}: SearchPanelProps) {
  const [histOpen, setHistOpen] = useState(false)
  const [hist, setHist] = useState<HistoryEntry[]>([])
  const histCloseTimer = useRef<number | null>(null)

  useEffect(
    () => () => {
      if (histCloseTimer.current !== null) window.clearTimeout(histCloseTimer.current)
    },
    [],
  )

  function loadHistory() {
    void storeApi
      .listHistory(10)
      .then(setHist)
      .catch(() => setHist([]))
  }

  const onInputFocus = () => {
    if (!query.trim()) {
      loadHistory()
      setHistOpen(true)
    }
  }

  const onInputBlur = () => {
    // 延迟关闭让点击历史项先落地
    if (histCloseTimer.current !== null) window.clearTimeout(histCloseTimer.current)
    histCloseTimer.current = window.setTimeout(() => setHistOpen(false), 150)
  }

  const onQueryInput = (e: ChangeEvent<HTMLInputElement>) => {
    onQueryChange(e.target.value)
    if (e.target.value.trim()) setHistOpen(false)
  }

  function pickHistory(entry: HistoryEntry) {
    setHistOpen(false)
    if (MODE_OPTIONS.some((m) => m.value === entry.mode)) onModeChange(entry.mode)
    onQueryChange(entry.query)
    onSubmit()
  }

  function clearHistory() {
    void storeApi
      .clearHistory()
      .then(() => setHist([]))
      .catch(() => {})
  }

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'ArrowUp') {
      e.preventDefault()
      setHistOpen(false)
      onArrowNav(-1)
    } else if (e.key === 'ArrowDown') {
      e.preventDefault()
      setHistOpen(false)
      onArrowNav(1)
    } else if (e.key === 'Enter') {
      e.preventDefault()
      setHistOpen(false)
      onSubmit()
    } else if (e.key === 'Escape') {
      setHistOpen(false)
    }
  }

  const onModeSelect = (e: ChangeEvent<HTMLSelectElement>) => {
    onModeChange(e.target.value as SearchMode)
  }

  return (
    <div className="search-panel">
      <select
        className="mode-select"
        value={mode}
        onChange={onModeSelect}
        title="搜索模式"
      >
        {MODE_OPTIONS.map((m) => (
          <option key={m.value} value={m.value}>
            {m.label}
          </option>
        ))}
      </select>

      <span className="search-icon" aria-hidden="true">
        🔍
      </span>

      <div className="search-input-wrap">
        <input
          className="search-input"
          type="text"
          value={query}
          placeholder={PLACEHOLDERS[mode]}
          onChange={onQueryInput}
          onFocus={onInputFocus}
          onBlur={onInputBlur}
          onKeyDown={onKeyDown}
          spellCheck={false}
          autoComplete="off"
        />
        {histOpen && (
          <div className="history-dropdown" role="listbox">
            {hist.length === 0 && <div className="history-empty">暂无搜索历史</div>}
            {hist.map((h) => (
              <button
                key={h.id}
                type="button"
                className="history-item"
                title={`${h.query}（${h.resultCount} 条结果）`}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => pickHistory(h)}
              >
                <span className="history-mode">{h.mode}</span>
                <span className="history-query">{h.query}</span>
                <span className="history-meta">{formatDate(h.ts)}</span>
              </button>
            ))}
            {hist.length > 0 && (
              <button
                type="button"
                className="history-clear"
                onMouseDown={(e) => e.preventDefault()}
                onClick={clearHistory}
              >
                清空记录
              </button>
            )}
          </div>
        )}
      </div>

      <button className="btn primary" onClick={onSubmit} disabled={loading}>
        {loading ? '搜索中…' : '开始'}
      </button>

      <label className="reverse-toggle" title="反向结果：搜不含该关键词的文件">
        <input
          type="checkbox"
          checked={reverse}
          onChange={(e) => onReverseChange(e.target.checked)}
        />
        反向结果
      </label>

      <span
        className={'dot ' + DOT_CLASS[indexState]}
        title={`索引状态：${indexState}`}
      />
    </div>
  )
}
