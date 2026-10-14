//! PDF 文本提取：**PDFium 主路径**（支持中文 CID 编码）+ `pdf-extract` 兜底。
//!
//! ## 为什么是双路径
//! - `pdf-extract`（纯 Rust）对标准编码可用，但遇中文 CID（`UniGB-UCS2-H` 等）
//!   直接 panic 且无文本 —— 国内真实中文 PDF 多为此编码（实测结论）。
//! - **PDFium**（Chromium 的 PDF 引擎，BSD-3）完整实现 CMap/CID 映射，
//!   中文/日文/韩文 PDF 文字层均可提取。
//!
//! 故：PDFium 可用时走 PDFium；不可用（库未随包分发）时回退 `pdf-extract`，
//! 保住 ASCII/西文 PDF 的基本可用性。
//!
//! ## 库定位（按优先级）
//! 1. 环境变量 `SEARCH_TOOL_PDFIUM_LIB`
//! 2. 可执行文件同级的 `libpdfium.so` / `lib/` / `../lib/` / macOS `.app Resources/`
//! 3. 源码树 `third_party/pdfium/lib/libpdfium.so`
//! 4. 系统库路径（`bind_to_system_library`）
//!
//! 边界：扫描型 PDF（无文字层）→ `Unsupported`；加密 PDF → `Io`。
//! 页级 bbox / 结构化输出（ContentListV2）不在本模块范围。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use pdfium_render::prelude::{PdfRenderConfig, Pdfium};

use super::ExtractError;

#[cfg(target_os = "linux")]
const LIB_NAME: &str = "libpdfium.so";
#[cfg(target_os = "macos")]
const LIB_NAME: &str = "libpdfium.dylib";
#[cfg(target_os = "windows")]
const LIB_NAME: &str = "pdfium.dll";

/// 进程内一次性初始化（PDFium 绑定是全局函数表）
static PDFIUM: OnceLock<Option<Pdfium>> = OnceLock::new();

fn lib_candidates() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("SEARCH_TOOL_PDFIUM_LIB") {
        v.push(PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            v.push(dir.join(LIB_NAME));
            v.push(dir.join("lib").join(LIB_NAME));
            v.push(dir.join("..").join("lib").join(LIB_NAME));
            // macOS .app bundle
            v.push(dir.join("..").join("Resources").join(LIB_NAME));
        }
    }
    v.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("third_party")
            .join("pdfium")
            .join("lib")
            .join(LIB_NAME),
    );
    v.push(PathBuf::from(format!("/usr/lib/{LIB_NAME}")));
    v.push(PathBuf::from(format!("/usr/local/lib/{LIB_NAME}")));
    v
}

fn init_pdfium() -> Option<Pdfium> {
    for c in lib_candidates() {
        if !c.exists() {
            continue;
        }
        match Pdfium::bind_to_library(&c) {
            Ok(bindings) => return Some(Pdfium::new(bindings)),
            Err(e) => eprintln!("PDFium 绑定失败（{}）: {e}", c.display()),
        }
    }
    Pdfium::bind_to_system_library().ok().map(Pdfium::new)
}

/// 取进程内 PDFium 实例（不可用返回 None）
fn pdfium() -> Option<&'static Pdfium> {
    PDFIUM.get_or_init(init_pdfium).as_ref()
}

/// PDFium 是否可用（供测试/诊断）
pub fn pdfium_available() -> bool {
    pdfium().is_some()
}

/// 统一入口：抽取 PDF 文字层为纯文本。
pub fn pdf_to_text(bytes: &[u8]) -> Result<String, ExtractError> {
    if let Some(p) = pdfium() {
        match extract_with_pdfium(p, bytes) {
            Ok(t) => return Ok(t),
            Err(primary) => {
                // PDFium 失败时再试 pdf-extract（两解析器互补）；
                // 双双失败则回报 PDFium 的错误（信息更准）
                return fallback_pdf_extract(bytes).map_err(|_| primary);
            }
        }
    }
    fallback_pdf_extract(bytes)
}

fn io_err(msg: String) -> ExtractError {
    ExtractError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, msg))
}

fn extract_with_pdfium(p: &Pdfium, bytes: &[u8]) -> Result<String, ExtractError> {
    let doc = p
        .load_pdf_from_byte_slice(bytes, None)
        .map_err(|e| io_err(format!("PDF 打开失败: {e}")))?;

    let mut out = String::new();
    for (i, page) in doc.pages().iter().enumerate() {
        // 单页失败不影响整体（页级异常跳过）
        let Ok(text) = page.text() else { continue };
        let mut page_text = String::new();
        for seg in text.segments().iter() {
            page_text.push_str(&seg.text());
            page_text.push('\n');
        }
        if !page_text.trim().is_empty() {
            out.push_str(&format!("===== 第 {} 页 =====\n", i + 1));
            out.push_str(&page_text);
        }
    }

    if out.trim().is_empty() {
        // 无文字层 = 扫描型/图片型 PDF → 交给 OCR 通道（P4-A）决策
        let page_count = doc.pages().len() as u32;
        return Err(ExtractError::NeedsOcr {
            pages: (1..=page_count).collect(),
            page_count,
        });
    }
    Ok(out)
}

/// `pdf-extract` 兜底（标准编码 PDF 可用；CID 编码返回受控错误）
fn fallback_pdf_extract(bytes: &[u8]) -> Result<String, ExtractError> {
    // pdf-extract 对不支持的字体编码会 panic（非 Result）；catch_unwind 兜底，
    // 归一为 Unsupported —— 提取层其它调用方一律不预期 panic。
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pdf_extract::extract_text_from_mem(bytes)
    }));
    let text = match res {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => return Err(io_err(format!("PDF 文字层提取失败: {e}"))),
        Err(_) => {
            return Err(ExtractError::Unsupported(
                "PDF 字体编码不受支持（如中文 CID / UniGB-UCS2-H），需 PDFium 通道".to_string(),
            ))
        }
    };
    if text.trim().is_empty() {
        return Err(ExtractError::Unsupported(
            "PDF 无文字层（可能是扫描件，需 OCR，属后续批次）".to_string(),
        ));
    }
    Ok(text)
}

/* ---------------------- 页渲染与 bbox（P3-B） ---------------------- */

/// 页面文本块：`text` 为段落文本，矩形为**归一化坐标**（0..1，原点左上）
///
/// 前端拿到页面位图后，用 `x/y/w/h` 直接乘显示尺寸即可叠加命中高亮框。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageTextBox {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

fn pdfium_or_unsupported() -> Result<&'static Pdfium, ExtractError> {
    pdfium().ok_or_else(|| {
        ExtractError::Unsupported(
            "PDFium 库不可用（页视图/bbox 需 PDFium 通道；third_party/pdfium 未就位）".to_string(),
        )
    })
}

/// 页数
pub fn page_count(bytes: &[u8]) -> Result<usize, ExtractError> {
    let p = pdfium_or_unsupported()?;
    let doc = p
        .load_pdf_from_byte_slice(bytes, None)
        .map_err(|e| io_err(format!("PDF 打开失败: {e}")))?;
    Ok(doc.pages().len() as usize)
}

/// 渲染第 `page_index` 页为 PNG 字节（`target_width` 为目标像素宽，高按比例）
pub fn render_page_png(
    bytes: &[u8],
    page_index: usize,
    target_width: u32,
) -> Result<Vec<u8>, ExtractError> {
    let p = pdfium_or_unsupported()?;
    let doc = p
        .load_pdf_from_byte_slice(bytes, None)
        .map_err(|e| io_err(format!("PDF 打开失败: {e}")))?;
    let pages = doc.pages();
    let idx = i32::try_from(page_index)
        .map_err(|_| io_err(format!("页索引 {page_index} 超出可表示范围")))?;
    let page = pages
        .get(idx)
        .map_err(|e| io_err(format!("页索引 {page_index} 越界: {e}")))?;

    // 尺寸闸门（审计收紧）：宽 ≤ 2048；高 ≤ 宽×4；总像素 ≤ 2400 万（≈96 MB RGBA 缓冲）
    // 旧版 `clamp(32,4096)` + 高度 8 倍最坏可分配 ~1.3 亿像素（~536 MB）单页位图
    const MAX_WIDTH: u32 = 2048;
    const MAX_ASPECT: u32 = 4;
    const MAX_PIXELS: u32 = 24_000_000;
    let width = target_width.clamp(32, MAX_WIDTH);
    let mut max_height = width.saturating_mul(MAX_ASPECT);
    if width.saturating_mul(max_height) > MAX_PIXELS {
        max_height = (MAX_PIXELS / width).max(width);
    }
    let cfg = PdfRenderConfig::new()
        .set_target_width(width as i32)
        .set_maximum_height(max_height as i32);
    let bitmap = page
        .render_with_config(&cfg)
        .map_err(|e| io_err(format!("页渲染失败: {e}")))?;

    let w = bitmap.width().max(1) as u32;
    let h = bitmap.height().max(1) as u32;
    let rgba = bitmap.as_rgba_bytes();

    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc
            .write_header()
            .map_err(|e| io_err(format!("PNG 头写入失败: {e}")))?;
        writer
            .write_image_data(&rgba)
            .map_err(|e| io_err(format!("PNG 数据写入失败: {e}")))?;
    }
    Ok(out)
}

/// 第 `page_index` 页的文本块（含归一化 bbox），供"命中 → 页内高亮框"
///
/// 坐标系：PDF 页面空间原点在**左下**、y 向上；本函数输出统一为
/// **左上原点、y 向下**的归一化坐标（前端 `<img>` 叠加可直接用）。
pub fn page_text_boxes(bytes: &[u8], page_index: usize) -> Result<Vec<PageTextBox>, ExtractError> {
    let p = pdfium_or_unsupported()?;
    let doc = p
        .load_pdf_from_byte_slice(bytes, None)
        .map_err(|e| io_err(format!("PDF 打开失败: {e}")))?;
    let pages = doc.pages();
    let idx = i32::try_from(page_index)
        .map_err(|_| io_err(format!("页索引 {page_index} 超出可表示范围")))?;
    let page = pages
        .get(idx)
        .map_err(|e| io_err(format!("页索引 {page_index} 越界: {e}")))?;

    let pw = page.width().value.max(1.0);
    let ph = page.height().value.max(1.0);
    let text = page
        .text()
        .map_err(|e| io_err(format!("页文本读取失败: {e}")))?;

    let mut out = Vec::new();
    for seg in text.segments().iter() {
        let s = seg.text();
        if s.trim().is_empty() {
            continue;
        }
        let r = seg.bounds();
        let (l, t, rr, b) = (
            r.left().value,
            r.top().value,
            r.right().value,
            r.bottom().value,
        );
        out.push(PageTextBox {
            text: s,
            x: (l / pw).clamp(0.0, 1.0),
            y: (1.0 - t / ph).clamp(0.0, 1.0),
            w: ((rr - l) / pw).clamp(0.0, 1.0),
            h: ((t - b) / ph).clamp(0.0, 1.0),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name)
    }

    #[test]
    fn cid_chinese_pdf_extracts_text() {
        // 中文 CID（UniGB-UCS2-H）：pdf-extract panic，PDFium 应可提取。
        let bytes = std::fs::read(fixture("cid-cn.pdf")).expect("夹具存在");
        if !pdfium_available() {
            eprintln!("PDFium 库不可用（third_party 未就位）→ 退化为受控错误路径");
            let e = pdf_to_text(&bytes).unwrap_err();
            assert!(
                matches!(e, ExtractError::Unsupported(_) | ExtractError::Io(_)),
                "无 PDFium 时须为受控错误: {e}"
            );
            return;
        }
        let t = pdf_to_text(&bytes).expect("PDFium 应能提取中文 CID PDF");
        assert!(
            t.contains("全文索引") || t.contains("倒排索引") || t.contains("白皮书"),
            "中文正文应抽出: {t}"
        );
    }

    #[test]
    fn english_pdf_extracts_text() {
        let bytes = std::fs::read(fixture("en.pdf")).expect("夹具存在");
        let t = pdf_to_text(&bytes).expect("英文 PDF 应可提取");
        assert!(t.contains("Inverted"), "{t}");
    }

    #[test]
    fn corrupt_pdf_is_controlled_error() {
        let err = pdf_to_text(b"definitely not a pdf").unwrap_err();
        assert!(
            matches!(err, ExtractError::Io(_) | ExtractError::Unsupported(_)),
            "{err}"
        );
    }

    /// P3-B：页渲染 + bbox（需 PDFium）
    #[test]
    fn render_page_and_text_boxes() {
        let bytes = std::fs::read(fixture("en.pdf")).expect("夹具存在");
        if !pdfium_available() {
            // 无 PDFium 时页视图不可用：须为受控错误
            let e = render_page_png(&bytes, 0, 300).unwrap_err();
            assert!(matches!(e, ExtractError::Unsupported(_)), "{e}");
            return;
        }
        assert_eq!(page_count(&bytes).unwrap(), 1, "单页样例");

        let png = render_page_png(&bytes, 0, 300).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "应是 PNG 魔数");
        assert!(png.len() > 1000, "位图应有内容: {} 字节", png.len());

        let boxes = page_text_boxes(&bytes, 0).unwrap();
        assert!(!boxes.is_empty(), "应取到文本块");
        for b in &boxes {
            assert!(
                (0.0..=1.0).contains(&b.x)
                    && (0.0..=1.0).contains(&b.y)
                    && b.w > 0.0
                    && b.h > 0.0
                    && b.w <= 1.0
                    && b.h <= 1.0,
                "bbox 应归一化且在页内: {b:?}"
            );
        }
        assert!(
            boxes.iter().any(|b| b.text.contains("Inverted")),
            "应含正文文本块: {:?}",
            boxes
        );
    }

    #[test]
    fn page_index_out_of_bounds_is_controlled() {
        let bytes = std::fs::read(fixture("en.pdf")).expect("夹具存在");
        if !pdfium_available() {
            return;
        }
        let e = render_page_png(&bytes, 5, 300).unwrap_err();
        assert!(matches!(e, ExtractError::Io(_)), "{e}");
    }

    #[test]
    fn render_width_is_clamped_to_safe_max() {
        let bytes = std::fs::read(fixture("en.pdf")).expect("夹具存在");
        if !pdfium_available() {
            return;
        }
        // 请求 99999px 宽 → 应被钳到 2048（PNG IHDR 里读回真实宽度）
        let png = render_page_png(&bytes, 0, 99_999).unwrap();
        let w = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
        let h = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
        assert!(w <= 2048, "宽度应被钳制，实际 {w}");
        assert!(w > 0 && h > 0, "尺寸应为正: {w}x{h}");
    }
}
