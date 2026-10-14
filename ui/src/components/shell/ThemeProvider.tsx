/**
 * 主题提供器（子项 5 所有权）
 * 把 ThemeMode 落到 <html data-theme="light|dark">；system 跟随 prefers-color-scheme。
 *
 * 注意：App.tsx 自己也做了同样一件事（同一 data-theme 属性名）。
 * 本组件实现为幂等：仅在解析值变化时才写 dataset，重复设置不冲突、不引入第二套属性名。
 */
import { useEffect } from 'react'
import type { ReactNode } from 'react'
import type { ThemeMode } from '../../types'

export interface ThemeProviderProps {
  theme: ThemeMode
  children: ReactNode
}

export default function ThemeProvider({ theme, children }: ThemeProviderProps) {
  useEffect(() => {
    const root = document.documentElement
    const mq = window.matchMedia('(prefers-color-scheme: dark)')
    const apply = () => {
      const resolved = theme === 'system' ? (mq.matches ? 'dark' : 'light') : theme
      // 幂等：值一致不重复写，避免与 App.tsx 的写入相互触发
      if (root.dataset.theme !== resolved) root.dataset.theme = resolved
    }
    apply()
    mq.addEventListener('change', apply)
    return () => mq.removeEventListener('change', apply)
  }, [theme])

  return <>{children}</>
}
