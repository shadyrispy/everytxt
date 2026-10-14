/**
 * minimap 关键词分布（子项 4 所有权）
 * 右侧窄条：按全文比例刻度标出命中位置，点击/拖拽定位。
 *
 * 入参 offsets 必须来自后端 match_groups（与高亮/匹配列表三段同源）。
 * 用 <canvas> 绘制（1MB 文本 + 上千刻度 <50ms，S9 门禁）。
 */
import { useEffect, useRef } from 'react'
import './preview.css'

export interface MinimapProps {
  /** 全文总字节数（截断文档传截断后实际字节数，保证命中刻度比例不失真） */
  totalBytes: number
  /** 命中起始字节偏移（升序） */
  offsets: number[]
  /** 当前滚动位置比例 0..1 */
  scrollRatio: number
  onSeek: (ratio: number) => void
}

/** 读取当前主题下的 CSS 令牌色（canvas 无法直接用 var()） */
function cssVar(name: string, fallback: string): string {
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim()
  return v || fallback
}

export default function Minimap({ totalBytes, offsets, scrollRatio, onSeek }: MinimapProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const boxRef = useRef<HTMLDivElement | null>(null)
  const draggingRef = useRef(false)
  // 用 ref 持有最新数据，供 ResizeObserver / 主题变化触发的重绘读取
  const dataRef = useRef({ totalBytes, offsets, scrollRatio })
  dataRef.current = { totalBytes, offsets, scrollRatio }

  /* 绘制刻度 + 视口指示线 */
  const draw = () => {
    const canvas = canvasRef.current
    if (!canvas) return
    const ctx = canvas.getContext('2d')
    if (!ctx) return
    const cssW = canvas.clientWidth || 12
    const cssH = canvas.clientHeight || 1
    const dpr = window.devicePixelRatio || 1
    const pxW = Math.max(1, Math.round(cssW * dpr))
    const pxH = Math.max(1, Math.round(cssH * dpr))
    if (canvas.width !== pxW || canvas.height !== pxH) {
      canvas.width = pxW
      canvas.height = pxH
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
    ctx.clearRect(0, 0, cssW, cssH)

    const { totalBytes: tb, offsets: offs, scrollRatio: sr } = dataRef.current
    if (tb > 0 && offs.length > 0) {
      ctx.fillStyle = cssVar('--c-mark', '#ffe066')
      const tickH = Math.max(1, Math.min(2, cssH / 400))
      for (const o of offs) {
        const y = Math.max(0, Math.min(cssH, (o / tb) * cssH))
        ctx.fillRect(0, y, cssW, tickH)
      }
    }
    // 视口指示线（当前滚动位置）
    ctx.fillStyle = cssVar('--c-accent', '#2d7ff9')
    const markerY = Math.max(0, Math.min(cssH - 1, sr * cssH))
    ctx.fillRect(0, markerY, cssW, 1)
  }

  useEffect(() => {
    draw()
  })

  /* 尺寸变化 → 重绘 */
  useEffect(() => {
    const el = boxRef.current
    if (!el || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(() => draw())
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  /* 主题切换 → 重绘（读到的令牌色变了） */
  useEffect(() => {
    const obs = new MutationObserver(() => draw())
    obs.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] })
    return () => obs.disconnect()
  }, [])

  const ratioFromEvent = (clientY: number): number => {
    const el = boxRef.current
    if (!el) return 0
    const rect = el.getBoundingClientRect()
    if (rect.height <= 0) return 0
    return Math.max(0, Math.min(1, (clientY - rect.top) / rect.height))
  }

  return (
    <div
      ref={boxRef}
      className="minimap"
      title="关键词分布：点击/拖拽定位"
      onPointerDown={(e) => {
        draggingRef.current = true
        e.currentTarget.setPointerCapture(e.pointerId)
        onSeek(ratioFromEvent(e.clientY))
      }}
      onPointerMove={(e) => {
        if (draggingRef.current) onSeek(ratioFromEvent(e.clientY))
      }}
      onPointerUp={(e) => {
        draggingRef.current = false
        e.currentTarget.releasePointerCapture(e.pointerId)
      }}
      onPointerCancel={() => {
        draggingRef.current = false
      }}
    >
      <canvas ref={canvasRef} className="minimap-canvas" />
    </div>
  )
}
