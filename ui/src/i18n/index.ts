/**
 * i18n 骨架（架构 v2 §4-D9）：字典集中 + t(key) + React Context。
 *
 * 约定（详见 README.md）：
 *   - key 命名 `menu.file.export` 风格（组件.区块.条目）；
 *   - 新增字符串必须 zh-CN / en-US 双语言同增；
 *   - 组件经 `useI18n()` 取 t；App 根部挂 `I18nProvider`
 *     （lang 来自 AppConfig.language；Provider 外默认 zh-CN）。
 */
import { createContext, createElement, useContext, useEffect, useState, type ReactNode } from 'react'
import { getAppConfig } from '../api/shell'
import zhCN from './zh-CN'
import zhTW from './zh-TW'
import enUS from './en-US'
import ja from './ja'

export type Lang = 'zh-CN' | 'zh-TW' | 'en-US' | 'ja'
export type Dict = Record<string, string>
/** 插值变量：对象 `{name}` 或数组 `{1}`（1 起始） */
export type Vars = Record<string, string | number> | (string | number)[]

export const DICTS: Record<Lang, Dict> = { 'zh-CN': zhCN, 'zh-TW': zhTW, 'en-US': enUS, ja }
export const DEFAULT_LANG: Lang = 'zh-CN'

/** 语言显示名（设置页下拉用，不受当前语言影响，恒以各自语言呈现） */
export const LANG_LABELS: Array<{ value: Lang; label: string }> = [
  { value: 'zh-CN', label: '简体中文' },
  { value: 'zh-TW', label: '繁體中文' },
  { value: 'en-US', label: 'English' },
  { value: 'ja', label: '日本語' },
]

export function normalizeLang(v: string | null | undefined): Lang {
  return v === 'en-US' || v === 'ja' || v === 'zh-TW' ? v : DEFAULT_LANG
}

/** `{1}` / `{name}` 占位替换；缺参保留原占位符 */
export function interpolate(tpl: string, vars?: Vars): string {
  if (!vars) return tpl
  return tpl.replace(/\{(\w+)\}/g, (m, k: string) => {
    const v = Array.isArray(vars) ? vars[Number(k) - 1] : vars[k]
    return v === undefined ? m : String(v)
  })
}

export function translate(lang: Lang, key: string, vars?: Vars): string {
  const s = DICTS[lang][key] ?? DICTS[DEFAULT_LANG][key]
  // 目标语言与兜底语言均缺 key：原样返回 key（开发期可见，避免静默空白）
  return interpolate(s ?? key, vars)
}

const LangContext = createContext<Lang>(DEFAULT_LANG)

/**
 * 语言 Provider：lang 来自 AppConfig.language（读失败保持默认 zh-CN）。
 * 集成方在 App 根部挂载一次；设置页切换语言后重挂/刷新即生效。
 */
export function I18nProvider({ children }: { children: ReactNode }) {
  const [lang, setLang] = useState<Lang>(DEFAULT_LANG)
  useEffect(() => {
    let alive = true
    getAppConfig()
      .then((cfg) => {
        if (alive) setLang(normalizeLang(cfg.language))
      })
      .catch(() => {
        /* 配置读取失败保持默认语言 */
      })
    return () => {
      alive = false
    }
  }, [])
  return createElement(LangContext.Provider, { value: lang }, children)
}

export function useI18n(): { lang: Lang; t: (key: string, vars?: Vars) => string } {
  const lang = useContext(LangContext)
  return { lang, t: (key: string, vars?: Vars) => translate(lang, key, vars) }
}
