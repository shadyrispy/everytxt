/**
 * 批量操作条（子项 2 所有权）
 * 有 ≥1 项选中时出现：打开 / 定位 / 复制路径 / 移入回收站 / 彻底删除 / 复制到… / 移动到… / 清除选择
 *
 * 说明（R9 / 契约）：
 * - 危险操作二次确认：仅「彻底删除」不可恢复，故只有它弹 window.confirm
 *   （移入回收站可恢复，不拦截）。与 ResultList 右键菜单口径一致。
 * - 「复制到…/移动到…」目标目录由后端弹框获取（前端未装 @tauri-apps/plugin-dialog），
 *   transferFiles 的 dest 传空串触发。
 * - 删除/移动成功后调 App 的 onRefresh(成功路径)：本地过滤结果 + 重发搜索，
 *   选择清收由 App 统一收尾（见 App.onRefreshAfterOp）。
 */
import { useState } from 'react'
import type { FileOpResult } from '../../types'
import { errMsg } from '../../util'
import * as files from '../../api/files'
import './results.css'

export interface SelectionBarProps {
  paths: string[]
  onClear: () => void
  /** 删除/移动成功后的收尾（传入实际失效的路径） */
  onRefresh: (removed: string[]) => void
}

export default function SelectionBar({ paths, onClear, onRefresh }: SelectionBarProps) {
  const [busy, setBusy] = useState(false)

  if (paths.length === 0) return null

  function notify(r: FileOpResult, action: string) {
    if (r.failed.length > 0) {
      const f = r.failed[0]
      window.alert(`${action}：成功 ${r.ok} 项，失败 ${r.failed.length} 项。\n首个错误：${f.path}\n${f.error}`)
    }
  }

  /** 部分失败不中断：从 paths 中剔除失败项，返回实际失效的路径 */
  function removedPaths(r: FileOpResult, targets: string[]): string[] {
    const failed = new Set(r.failed.map((f) => f.path))
    return targets.filter((p) => !failed.has(p))
  }

  async function guard(fn: () => Promise<void>) {
    if (busy) return
    setBusy(true)
    try {
      await fn()
    } catch (e) {
      window.alert(`操作失败：${errMsg(e)}`)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="selection-bar">
      <span className="sel-count">已选择 {paths.length} 项</span>
      <div className="sel-actions">
        <button className="btn" disabled={busy} onClick={() => void guard(() => files.openFile(paths[0]))}>
          打开
        </button>
        <button className="btn" disabled={busy} onClick={() => void guard(() => files.revealInFolder(paths[0]))}>
          定位
        </button>
        <button className="btn" disabled={busy} onClick={() => void guard(() => files.copyPaths(paths))}>
          复制路径
        </button>
        <button className="btn" disabled={busy} onClick={() => void guard(() => files.copyFileNames(paths))}>
          复制名称
        </button>
        <button
          className="btn"
          disabled={busy}
          onClick={() =>
            void guard(async () => {
              const r = await files.moveToTrash(paths)
              notify(r, '移入回收站')
              onRefresh(removedPaths(r, paths))
            })
          }
        >
          移入回收站
        </button>
        <button
          className="btn danger"
          disabled={busy}
          onClick={() =>
            void guard(async () => {
              if (!window.confirm(`确定彻底删除 ${paths.length} 项？此操作不可恢复。`)) return
              const r = await files.deletePermanently(paths)
              notify(r, '彻底删除')
              onRefresh(removedPaths(r, paths))
            })
          }
        >
          彻底删除
        </button>
        <button
          className="btn"
          disabled={busy}
          onClick={() => void guard(async () => notify(await files.transferFiles(paths, '', false), '复制'))}
        >
          复制到…
        </button>
        <button
          className="btn"
          disabled={busy}
          onClick={() =>
            void guard(async () => {
              const r = await files.transferFiles(paths, '', true)
              notify(r, '移动')
              onRefresh(removedPaths(r, paths))
            })
          }
        >
          移动到…
        </button>
        <button className="btn" disabled={busy} onClick={onClear}>
          清除选择
        </button>
      </div>
    </div>
  )
}
