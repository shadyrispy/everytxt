/**
 * 搜索条件行（子项 1 所有权）
 * 按规格
 *
 * 数据源来自 api/indexAdmin.ts 的 listIndexedRoots / listFileTypes（由子项 3 提供）。
 * 本组件只消费；拉取失败静默，不打扰主搜索流程。
 * R3：时间区间九档下拉（换档经 onTimeRangeChange 上抛，App 触发重搜）+ 书签入口。
 */
import { useEffect, useState } from 'react'
import type { FileType } from '../../types'
import { listFileTypes, listIndexedRoots } from '../../api/indexAdmin'
import './search.css'

export interface FilterRowProps {
  dir: string | null
  fileType: string | null
  /** R3：时间区间档位 key（TIME_RANGES 之一；'all' = 不限） */
  timeRange: string
  onDirChange: (v: string | null) => void
  onFileTypeChange: (v: string | null) => void
  onTimeRangeChange: (key: string) => void
  /** R3：打开书签管理面板 */
  onOpenBookmarks: () => void
}

/** 下拉空值哨兵：<select> 无法表达 null，用空串占位并在回流时还原 */
const ALL = ''

const DAY = 86_400

function startOfDay(unixSec: number): number {
  const d = new Date(unixSec * 1000)
  d.setHours(0, 0, 0, 0)
  return Math.floor(d.getTime() / 1000)
}

export interface TimeBounds {
  from: number | null
  to: number | null
}

/** 时间区间九档 → unix 秒闭区间边界（'all' → 双 null） */
export function timeRangeBounds(key: string): TimeBounds {
  const now = Math.floor(Date.now() / 1000)
  const todayStart = startOfDay(now)
  switch (key) {
    case 'today':
      return { from: todayStart, to: null }
    case 'yesterday': {
      const s = todayStart - DAY
      return { from: s, to: todayStart - 1 }
    }
    case 'week': {
      // 本周（周一起算）
      const dow = (new Date(now * 1000).getDay() + 6) % 7
      return { from: todayStart - dow * DAY, to: null }
    }
    case 'month': {
      const d = new Date(now * 1000)
      return { from: todayStart - (d.getDate() - 1) * DAY, to: null }
    }
    case 'quarter':
      return { from: now - 90 * DAY, to: null }
    case 'year': {
      const y = new Date(now * 1000).getFullYear()
      return { from: startOfDay(Math.floor(new Date(y, 0, 1).getTime() / 1000)), to: null }
    }
    case 'years3': {
      const d = new Date(now * 1000)
      const s = Math.floor(new Date(d.getFullYear() - 3, d.getMonth(), d.getDate()).getTime() / 1000)
      return { from: s, to: null }
    }
    case 'before3y': {
      const d = new Date(now * 1000)
      const s = Math.floor(new Date(d.getFullYear() - 3, d.getMonth(), d.getDate()).getTime() / 1000)
      return { from: null, to: s - 1 }
    }
    default:
      return { from: null, to: null }
  }
}

/** 九档：key → 显示名 */
export const TIME_RANGES: Array<{ key: string; label: string }> = [
  { key: 'all', label: '全部时间' },
  { key: 'today', label: '今天' },
  { key: 'yesterday', label: '昨天' },
  { key: 'week', label: '本周' },
  { key: 'month', label: '本月' },
  { key: 'quarter', label: '近三月' },
  { key: 'year', label: '今年' },
  { key: 'years3', label: '近三年' },
  { key: 'before3y', label: '三年以前' },
]

export default function FilterRow({
  dir,
  fileType,
  timeRange,
  onDirChange,
  onFileTypeChange,
  onTimeRangeChange,
  onOpenBookmarks,
}: FilterRowProps) {
  const [roots, setRoots] = useState<string[]>([])
  const [types, setTypes] = useState<FileType[]>([])

  /* 挂载时各拉一次（失败静默） */
  useEffect(() => {
    void listIndexedRoots()
      .then(setRoots)
      .catch(() => {})
    void listFileTypes()
      .then(setTypes)
      .catch(() => {})
  }, [])

  /* 当前 fileType 已不在列表里 → 回落 null（如类型被删除/改名） */
  useEffect(() => {
    if (fileType !== null && !types.some((t) => t.name === fileType)) {
      onFileTypeChange(null)
    }
  }, [fileType, types, onFileTypeChange])

  return (
    <div className="filter-row">
      <label className="filter-label">
        指定目录：
        <span className="filter-icon" aria-hidden="true">
          📁
        </span>
        <select
          className="filter-select"
          value={dir ?? ALL}
          onChange={(e) => onDirChange(e.target.value === ALL ? null : e.target.value)}
        >
          <option value={ALL}>全部目录</option>
          {roots.map((r) => (
            <option key={r} value={r}>
              {r}
            </option>
          ))}
        </select>
      </label>

      <label className="filter-label">
        指定类型：
        <span className="filter-icon" aria-hidden="true">
          📄
        </span>
        <select
          className="filter-select"
          value={fileType ?? ALL}
          onChange={(e) => onFileTypeChange(e.target.value === ALL ? null : e.target.value)}
        >
          <option value={ALL}>全部文件类型</option>
          {types.map((t) => (
            <option key={t.id} value={t.name}>
              {t.name}
            </option>
          ))}
        </select>
      </label>

      <label className="filter-label">
        时间：
        <span className="filter-icon" aria-hidden="true">
          🕐
        </span>
        <select
          className="filter-select"
          value={timeRange}
          onChange={(e) => onTimeRangeChange(e.target.value)}
        >
          {TIME_RANGES.map((t) => (
            <option key={t.key} value={t.key}>
              {t.label}
            </option>
          ))}
        </select>
      </label>

      <button
        type="button"
        className="filter-bookmark-btn"
        title="书签管理"
        style={{ marginLeft: 'auto' }}
        onClick={onOpenBookmarks}
      >
        ★ 书签
      </button>
    </div>
  )
}
