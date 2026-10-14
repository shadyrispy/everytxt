/**
 * 状态栏（子项 5 所有权）
 * 按规格
 *   左：搜索状态文案（"返回 107 个结果，耗时 0.235 秒，搜索完成。"）
 *   中：当前文件全路径（超长省略，title 显示完整）
 *   右：索引统计（"1,234 篇 · 12.3 MB"）
 * 高度由 --h-status 控制（见 shell.css）。
 */
import './shell.css'

export interface StatusBarProps {
  /** 左：搜索状态文案 */
  text: string
  /** 中：当前文件全路径（可空） */
  path: string | null
  /** 右：索引统计，如 "1234 篇 · 12.3 MB" */
  right: string
}

export default function StatusBar({ text, path, right }: StatusBarProps) {
  return (
    <footer className="statusbar">
      <span className="status-text" title={text}>
        {text}
      </span>
      <span className="status-path" title={path ?? ''}>
        {path ?? ''}
      </span>
      <span className="status-right" title={right}>
        {right}
      </span>
    </footer>
  )
}
