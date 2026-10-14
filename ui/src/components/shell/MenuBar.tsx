/**
 * 菜单栏（子项 5 所有权）
 * 按规格
 * 帮助菜单内含「高级搜索语法速查」子面板（照 image21：& 与 / | 或 / ! 非 / () 分组 / "" 精确）。
 *
 * 交互：
 *   - 点击菜单标题打开下拉；再次点击或点外部关闭；
 *   - Escape 关闭；打开状态下 ↑/↓ 在项间移动、Enter 触发；
 *   - 菜单项显示中文文本 + 右侧快捷键提示。
 * 高度由 --h-menu 控制（见 shell.css）。
 */
import { useEffect, useRef, useState } from 'react'
import type { MenuAction, PreviewViewOpts } from '../../types'
import { useI18n, type Vars } from '../../i18n'
import './shell.css'

export interface MenuBarProps {
  onAction: (a: MenuAction) => void
  /** 预览显示选项；用于查看菜单显示行号/自动换行的勾选态 */
  view?: PreviewViewOpts
}

type TFn = (key: string, vars?: Vars) => string

interface MenuItemDef {
  label: string
  accel?: string
  action?: MenuAction
  /** 勾选态（查看菜单显示行号/自动换行） */
  checked?: boolean
  /** 特殊项：切换「高级搜索语法速查」子面板 */
  syntax?: boolean
}

interface MenuDef {
  title: string
  items: MenuItemDef[]
}

/** 菜单结构；查看菜单的勾选态依赖运行时 view，故用函数构建（label 走 t()） */
function buildMenus(t: TFn, view?: PreviewViewOpts): MenuDef[] {
  return [
    {
      title: t('menu.file'),
      items: [
        { label: t('menu.file.export'), accel: 'Ctrl+S', action: 'file.export' },
        { label: t('menu.file.exit'), accel: 'Alt+F4', action: 'file.exit' },
      ],
    },
    {
      title: t('menu.edit'),
      items: [
        { label: t('menu.edit.copyPath'), accel: 'Ctrl+Shift+C', action: 'edit.copyPath' },
        { label: t('menu.edit.copyName'), action: 'edit.copyName' },
        { label: t('menu.edit.selectAll'), accel: 'Ctrl+A', action: 'edit.selectAll' },
        { label: t('menu.edit.findInDoc'), accel: 'Ctrl+F', action: 'edit.findInDoc' },
      ],
    },
    {
      title: t('menu.view'),
      items: [
        { label: t('menu.view.themeLight'), action: 'view.themeLight' },
        { label: t('menu.view.themeDark'), action: 'view.themeDark' },
        { label: t('menu.view.themeSystem'), action: 'view.themeSystem' },
        { label: t('menu.view.prevPage'), accel: 'Shift+F3', action: 'view.prevPage' },
        { label: t('menu.view.nextPage'), accel: 'F3', action: 'view.nextPage' },
        { label: t('menu.view.lineNumbers'), checked: view?.lineNumbers, action: 'view.lineNumbers' },
        { label: t('menu.view.wrap'), checked: view?.wrap, action: 'view.wrap' },
        { label: t('menu.view.zoomIn'), accel: 'Ctrl+↑', action: 'view.zoomIn' },
        { label: t('menu.view.zoomOut'), accel: 'Ctrl+↓', action: 'view.zoomOut' },
        { label: t('menu.view.zoomReset'), accel: 'Ctrl+0', action: 'view.zoomReset' },
      ],
    },
    {
      title: t('menu.settings'),
      items: [
        { label: t('menu.settings.options'), action: 'settings.options' },
        { label: t('menu.settings.indexAdmin'), action: 'settings.indexAdmin' },
      ],
    },
    {
      title: t('menu.help'),
      items: [
        { label: t('menu.help.syntax'), accel: 'F1', syntax: true },
        { label: t('menu.help.homepage'), action: 'help.homepage' },
        { label: t('menu.help.about'), accel: 'Shift+F1', action: 'help.about' },
      ],
    },
  ]
}

/** 高级搜索语法速查（照 image21）；文案走 t() */
function syntaxRules(t: TFn): Array<{ op: string; name: string; desc: string }> {
  return [
    { op: '&', name: t('syntax.and.name'), desc: t('syntax.and.desc') },
    { op: '|', name: t('syntax.or.name'), desc: t('syntax.or.desc') },
    { op: '!', name: t('syntax.not.name'), desc: t('syntax.not.desc') },
    { op: '()', name: t('syntax.group.name'), desc: t('syntax.group.desc') },
    { op: '""', name: t('syntax.exact.name'), desc: t('syntax.exact.desc') },
  ]
}

export default function MenuBar({ onAction, view }: MenuBarProps) {
  const { t } = useI18n()
  const [open, setOpen] = useState<string | null>(null)
  const [showSyntax, setShowSyntax] = useState(false)
  const [hi, setHi] = useState(-1)
  const rootRef = useRef<HTMLElement | null>(null)

  const menus = buildMenus(t, view)
  const rules = syntaxRules(t)

  const close = () => {
    setOpen(null)
    setShowSyntax(false)
    setHi(-1)
  }

  const toggle = (title: string) => {
    setOpen((cur) => (cur === title ? null : title))
    setShowSyntax(false)
    setHi(-1)
  }

  /* 点击菜单外部关闭 */
  useEffect(() => {
    if (!open) return
    const onDocClick = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) close()
    }
    document.addEventListener('mousedown', onDocClick)
    return () => document.removeEventListener('mousedown', onDocClick)
  }, [open])

  /* Escape 关闭 / ↑↓ 移动 / Enter 触发 */
  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      const menu = menus.find((m) => m.title === open)
      if (!menu) return
      if (e.key === 'Escape') {
        e.preventDefault()
        close()
      } else if (e.key === 'ArrowDown') {
        e.preventDefault()
        setHi((i) => (i + 1) % menu.items.length)
      } else if (e.key === 'ArrowUp') {
        e.preventDefault()
        setHi((i) => (i <= 0 ? menu.items.length - 1 : i - 1))
      } else if (e.key === 'Enter' && hi >= 0) {
        e.preventDefault()
        const it = menu.items[hi]
        if (it.syntax) setShowSyntax((v) => !v)
        else if (it.action) {
          onAction(it.action)
          close()
        }
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, hi, onAction, view, menus])

  return (
    <nav className="menubar" ref={rootRef} role="menubar" aria-label={t('menu.menubar')}>
      {menus.map((m) => {
        const isHelp = m.items.some((it) => it.syntax)
        return (
          <div className="menu" key={m.title}>
            <button
              type="button"
              className={`menu-title${open === m.title ? ' active' : ''}`}
              onClick={() => toggle(m.title)}
            >
              {m.title}
            </button>

            {open === m.title && (
              <div className="menu-dropdown" role="menu">
                {m.items.map((it, idx) => (
                  <button
                    type="button"
                    key={it.label}
                    className={`menu-item${hi === idx ? ' hi' : ''}`}
                    role="menuitem"
                    onMouseEnter={() => setHi(idx)}
                    onClick={() => {
                      if (it.syntax) {
                        setShowSyntax((v) => !v)
                      } else if (it.action) {
                        onAction(it.action)
                        close()
                      }
                    }}
                  >
                    <span className="menu-label">
                      {it.checked && <span className="menu-check">✓</span>}
                      {it.label}
                    </span>
                    {it.accel && <span className="menu-accel">{it.accel}</span>}
                  </button>
                ))}

                {isHelp && showSyntax && (
                  <div className="menu-syntax" role="dialog" aria-label={t('menu.help.syntax')}>
                    <div className="syntax-title">{t('syntax.title')}</div>
                    <table className="syntax-table">
                      <thead>
                        <tr>
                          <th>{t('syntax.header.op')}</th>
                          <th>{t('syntax.header.meaning')}</th>
                          <th>{t('syntax.header.example')}</th>
                        </tr>
                      </thead>
                      <tbody>
                        {rules.map((r) => (
                          <tr key={r.op}>
                            <td className="syntax-op">{r.op}</td>
                            <td className="syntax-name">{r.name}</td>
                            <td className="syntax-desc">{r.desc}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                    <div className="syntax-hint">{t('syntax.hint')}</div>
                  </div>
                )}
              </div>
            )}
          </div>
        )
      })}
    </nav>
  )
}
