/**
 * P4-D：OOXML 文档视图 —— docx / xlsx / pptx 的 Canvas 高保真渲染 + 命中高亮。
 *
 * 由 `@silurus/ooxml`（Rust→WASM 解析器 + Canvas 渲染器，MIT）驱动：
 *   new DocxViewer(canvas, opts) → load(ArrayBuffer) → findText(query) 高亮
 *
 * 设计：自包含；PreviewPane 仅按 Tab 渲染（docx/xlsx/pptx 才出现该 Tab）。
 * 渲染器按需动态 import（wasm 体积大，不进主 bundle）。
 */
import { useEffect, useRef, useState } from 'react'

import { readFileBytes } from '../../api/preview'

type OoxmlKind = 'docx' | 'xlsx' | 'pptx'

interface Props {
  path: string
  /** 当前搜索词（空则清除高亮） */
  query: string
  kind: OoxmlKind
}

/** 渲染器实例的最小契约（三个格式的 viewer 共享这些方法） */
interface ViewerLike {
  load(source: string | ArrayBuffer): Promise<void>
  findText(query: string, opts?: unknown): Promise<unknown[]>
  findNext(): Promise<unknown>
  findPrev(): Promise<unknown>
  clearFind(): void
  destroy?(): void
}

/** 三个格式的构造签名不同：docx/pptx 收 canvas，xlsx 收容器（自建 canvas） */
async function createViewer(kind: OoxmlKind, canvas: HTMLCanvasElement): Promise<ViewerLike> {
  const host: HTMLElement = canvas.parentElement ?? canvas
  if (kind === 'docx') {
    const mod = await import('@silurus/ooxml/docx')
    return new mod.DocxViewer(canvas, {
      container: host,
      enableTextSelection: true,
    }) as unknown as ViewerLike
  }
  if (kind === 'xlsx') {
    const mod = await import('@silurus/ooxml/xlsx')
    return new mod.XlsxViewer(host, {
      showZoomSlider: false,
    } as never) as unknown as ViewerLike
  }
  const mod = await import('@silurus/ooxml/pptx')
  return new mod.PptxViewer(canvas, {
    enableTextSelection: true,
  }) as unknown as ViewerLike
}

export default function OoxmlView({ path, query, kind }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const viewerRef = useRef<ViewerLike | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [matchCount, setMatchCount] = useState(0)

  // 挂载 / 换文档：创建 viewer 并 load
  useEffect(() => {
    let alive = true
    let viewer: ViewerLike | null = null
    setLoading(true)
    setError(null)
    setMatchCount(0)
    viewerRef.current = null

    const run = async () => {
      try {
        const canvas = canvasRef.current
        if (!canvas) return
        const bytes = await readFileBytes(path)
        if (!alive) return
        viewer = await createViewer(kind, canvas)
        await viewer.load(new Uint8Array(bytes).buffer)
        if (!alive) {
          viewer.destroy?.()
          return
        }
        viewerRef.current = viewer
        setLoading(false)
      } catch (e) {
        if (alive) {
          setError(String(e))
          setLoading(false)
        }
      }
    }
    void run()

    return () => {
      alive = false
      try {
        viewer?.destroy?.()
      } catch {
        /* 忽略销毁异常 */
      }
      viewerRef.current = null
    }
  }, [path, kind])

  // 查询变化 → 高亮全部命中
  useEffect(() => {
    const v = viewerRef.current
    if (!v || loading) return
    const q = query.trim()
    if (!q) {
      v.clearFind()
      setMatchCount(0)
      return
    }
    let alive = true
    v.findText(q)
      .then((ms) => {
        if (alive) setMatchCount(Array.isArray(ms) ? ms.length : 0)
      })
      .catch(() => {
        if (alive) setMatchCount(0)
      })
    return () => {
      alive = false
    }
  }, [query, loading, path])

  return (
    <div className="ooxml-view">
      <div className="ooxml-view-bar">
        <span className="ooxml-view-kind">{kind.toUpperCase()} 文档视图</span>
        {loading && <span className="ooxml-view-hint">加载中…</span>}
        {!loading && query.trim() !== '' && (
          <span className="ooxml-view-hint">命中 {matchCount} 处</span>
        )}
        <span className="ooxml-view-spacer" />
        <button
          type="button"
          className="ooxml-view-nav"
          disabled={loading}
          onClick={() => void viewerRef.current?.findPrev()}
        >
          上一个
        </button>
        <button
          type="button"
          className="ooxml-view-nav"
          disabled={loading}
          onClick={() => void viewerRef.current?.findNext()}
        >
          下一个
        </button>
      </div>
      <div className="ooxml-view-canvas">
        <canvas ref={canvasRef} />
      </div>
      {error && <div className="pdf-view-note">文档视图不可用：{error}</div>}
    </div>
  )
}
