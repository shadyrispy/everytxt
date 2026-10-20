/**
 * 共享纯函数工具（集成负责人所有权，子项 agent 只读复用）
 */

/** unix 秒 → "YYYY-MM-DD HH:mm" */
export function formatDate(unixSeconds: number): string {
  const d = new Date(unixSeconds * 1000)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
}

/** 字节 → 人类可读 */
export function formatSize(bytes: number): string {
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let v = bytes
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return `${i === 0 ? v : v.toFixed(1)} ${units[i]}`
}

/**
 * 展示名取 basename（同时兼容 `/` 与 `\` 分隔符）。
 * 后端 SearchHit.title 由 path.rsplit('/') 切出，Windows 混合分隔符路径下会得到
 * "目录\文件"（反斜杠根目录时甚至是全路径）；目录信息由路径列/title 呈现，
 * 名称列只展示文件名本身，避免浪费列宽。
 */
export function baseName(nameOrPath: string): string {
  if (!nameOrPath) return nameOrPath
  const i = Math.max(nameOrPath.lastIndexOf('/'), nameOrPath.lastIndexOf('\\'))
  return i >= 0 ? nameOrPath.slice(i + 1) : nameOrPath
}

export function errMsg(e: unknown): string {
  if (e instanceof Error) return e.message
  if (typeof e === 'string') return e
  return String(e)
}
