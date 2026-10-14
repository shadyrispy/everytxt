/**
 * P3-B：PDF 页视图 —— 页位图 + 命中 bbox 叠加 + 翻页。
 *
 * 数据流：
 *   pdfPageCount(path)           → 页数（失败即隐藏/提示，非 PDF 或 PDFium 缺失）
 *   renderPdfPage(path, page, w) → PNG 字节 → Blob URL
 *   pdfPageBoxes(path, page)     → 归一化 bbox（0..1，左上原点）→ 命中框叠加
 *
 * 设计：完全自包含（不依赖 PreviewPane 内部状态），PreviewPane 仅需按 Tab 渲染本组件。
 */
import { useEffect, useMemo, useRef, useState } from 'react'

import { pdfPageBoxes, pdfPageCount, renderPdfPage } from '../../api/preview'
import type { PageTextBox, SearchMode } from '../../types'

interface Props {
  path: string
  query: string
  mode: SearchMode
}

/**
 * 查询词项（与后端 `terms_of` 同口径的简化版）：
 * fuzzy 按空白切分；exact 整串；dsl 去运算符与取反项、剥成对引号
 */
function termsOf(query: string, mode: SearchMode): string[] {
  const q = query.trim()
  if (!q) return []
  if (mode === 'exact') return [q]
  if (mode === 'dsl') {
    return q
      .split(/[&|()]/)
      .map((t) => t.trim())
      .filter((t) => t && !t.startsWith('!'))
      .map((t) => (t.startsWith('"') && t.endsWith('"') && t.length >= 2 ? t.slice(1, -1) : t))
      .filter(Boolean)
  }
  return q.split(/\s+/).filter(Boolean)
}

export default function PdfPageView({ path, query, mode }: Props) {
  const [pageCount, setPageCount] = useState(0)
  const [page, setPage] = useState(0)
  const [url, setUrl] = useState<string | null>(null)
  const [boxes, setBoxes] = useState<PageTextBox[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const urlRef = useRef<string | null>(null)

  const terms = useMemo(
    () => termsOf(query, mode).map((t) => t.toLowerCase()),
    [query, mode],
  )

  // 页数（切换文档时复位到第 1 页）
  useEffect(() => {
    let alive = true
    setPage(0)
    setError(null)
    setBoxes([])
    pdfPageCount(path)
      .then((n) => {
        if (alive) setPageCount(n)
      })
      .catch((e) => {
        if (alive) {
          setPageCount(0)
          setError(String(e))
        }
      })
    return () => {
      alive = false
    }
  }, [path])

  // 当前页渲染 + bbox
  useEffect(() => {
    if (!pageCount) return
    let alive = true
    setLoading(true)
    Promise.all([renderPdfPage(path, page, 1000), pdfPageBoxes(path, page)])
      .then(([bytes, bs]) => {
        if (!alive) return
        const blob = new Blob([new Uint8Array(bytes)], { type: 'image/png' })
        const next = URL.createObjectURL(blob)
        if (urlRef.current) URL.revokeObjectURL(urlRef.current)
        urlRef.current = next
        setUrl(next)
        setBoxes(bs)
        setError(null)
        setLoading(false)
      })
      .catch((e) => {
        if (alive) {
          setError(String(e))
          setLoading(false)
        }
      })
    return () => {
      alive = false
    }
  }, [path, page, pageCount])

  // 卸载时释放 Blob URL
  useEffect(
    () => () => {
      if (urlRef.current) URL.revokeObjectURL(urlRef.current)
    },
    [],
  )

  const hits = useMemo(
    () => (terms.length ? boxes.filter((b) => {
      const t = b.text.toLowerCase()
      return terms.some((term) => t.includes(term))
    }) : []),
    [boxes, terms],
  )

  if (error) {
    return <div className="pdf-view-note">页视图不可用：{error}</div>
  }
  if (!pageCount) {
    return <div className="pdf-view-note">正在读取 PDF…</div>
  }

  return (
    <div className="pdf-view">
      <div className="pdf-view-bar">
        <button
          className="pdf-view-nav"
          disabled={page <= 0 || loading}
          onClick={() => setPage((p) => Math.max(0, p - 1))}
        >
          上一页
        </button>
        <span className="pdf-view-page">
          第 {page + 1} / {pageCount} 页
        </span>
        <button
          className="pdf-view-nav"
          disabled={page >= pageCount - 1 || loading}
          onClick={() => setPage((p) => Math.min(pageCount - 1, p + 1))}
        >
          下一页
        </button>
        {terms.length > 0 && <span className="pdf-view-hits">本页命中 {hits.length} 处</span>}
        {loading && <span className="pdf-view-hits">渲染中…</span>}
      </div>
      <div className="pdf-view-canvas">
        {url && (
          <div className="pdf-view-pagewrap">
            <img className="pdf-view-img" src={url} alt={`第 ${page + 1} 页`} />
            {hits.map((b, i) => (
              <span
                key={`${i}-${b.x}-${b.y}`}
                className="pdf-hit-box"
                title={b.text}
                style={{
                  left: `${b.x * 100}%`,
                  top: `${b.y * 100}%`,
                  width: `${b.w * 100}%`,
                  height: `${b.h * 100}%`,
                }}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
