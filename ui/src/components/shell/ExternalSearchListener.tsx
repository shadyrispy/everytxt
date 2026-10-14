/**
 * CLI `/s` 唤起监听（架构 v2 R5，§8）。
 *
 * 后端链路：CLI `search-tool '/s' 关键字 [/d 盘符] [/e 后缀]` → 单实例 socket
 * `{op:"search",...}` → 主实例 shell.rs `handle_socket_op` → emit `external-search`
 * → 本组件 onSearch 回调 → App 填充搜索条件并触发搜索。
 *
 * 挂载（集成方在 App.tsx 完成，本组件不持有业务状态）：
 *   <ExternalSearchListener
 *     onSearch={(query, drive, ext) => {
 *       setQuery(query)
 *       if (ext) setFileType(ext)
 *       if (drive) setDir(drive)
 *       setSubmitTick((t) => t + 1)
 *     }}
 *   />
 */
import { useEffect } from 'react'
import { listen } from '@tauri-apps/api/event'

/** shell.rs ExternalSearchPayload 的镜像 */
export interface ExternalSearchPayload {
  query: string
  /** 目录前缀（CLI /d 盘符归一化后；null = 全库） */
  drive: string | null
  /** 扩展名（不带点；null = 全部类型） */
  ext: string | null
}

export interface ExternalSearchListenerProps {
  onSearch: (query: string, drive: string | null, ext: string | null) => void
}

export default function ExternalSearchListener({ onSearch }: ExternalSearchListenerProps) {
  useEffect(() => {
    let un: (() => void) | null = null
    let disposed = false
    void (async () => {
      const u = await listen<ExternalSearchPayload>('external-search', (ev) => {
        const p = ev.payload
        if (p && p.query) onSearch(p.query, p.drive ?? null, p.ext ?? null)
      })
      if (disposed) u()
      else un = u
    })()
    return () => {
      disposed = true
      if (un) un()
    }
  }, [onSearch])

  return null
}
