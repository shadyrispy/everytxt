/**
 * 书签管理面板（R3 新增）
 * 由 FilterRow 右侧 ★ 按钮唤起：名称/路径列表 + 打开 + 删除。
 */
import { useEffect, useState } from 'react'
import type { Bookmark } from '../../types'
import * as storeApi from '../../api/store'
import { errMsg } from '../../util'
import * as files from '../../api/files'
import './results.css'

export interface BookmarkPanelProps {
  open: boolean
  onClose: () => void
}

export default function BookmarkPanel({ open, onClose }: BookmarkPanelProps) {
  const [items, setItems] = useState<Bookmark[]>([])
  const [error, setError] = useState<string | null>(null)

  /* 每次打开拉一次全量 */
  useEffect(() => {
    if (!open) return
    setItems([])
    setError(null)
    void storeApi
      .listBookmarks()
      .then(setItems)
      .catch((e) => setError(errMsg(e)))
  }, [open])

  /* Esc 关闭 */
  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, onClose])

  if (!open) return null

  function remove(path: string) {
    void storeApi
      .removeBookmark(path)
      .then(() => setItems((prev) => prev.filter((b) => b.path !== path)))
      .catch((e) => setError(errMsg(e)))
  }

  return (
    <div className="bookmark-overlay" onMouseDown={onClose}>
      <div className="bookmark-panel" role="dialog" aria-label="书签管理" onMouseDown={(e) => e.stopPropagation()}>
        <div className="bookmark-head">
          <span>书签</span>
          <button type="button" className="bookmark-close" title="关闭" onClick={onClose}>
            ✕
          </button>
        </div>
        <div className="bookmark-list">
          {error && <div className="bookmark-error">{error}</div>}
          {!error && items.length === 0 && <div className="bookmark-empty">暂无书签（在结果行右键"添加书签"）</div>}
          {items.map((b) => (
            <div key={b.id} className="bookmark-row" title={b.path}>
              <div className="bookmark-text">
                <div className="bookmark-name">{b.name}</div>
                <div className="bookmark-path">{b.path}</div>
              </div>
              <div className="bookmark-actions">
                <button type="button" onClick={() => void files.openFile(b.path).catch(() => {})}>
                  打开
                </button>
                <button type="button" className="danger" onClick={() => remove(b.path)}>
                  删除
                </button>
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}
