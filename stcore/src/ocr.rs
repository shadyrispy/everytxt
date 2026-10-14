//! OCR 通道（P4-A）：调用独立 `ocr-helper` 进程，把图片/扫描件转成文本。
//!
//! ## 架构
//! 主程序**不链接** ONNX Runtime 与 OCR 模型，而是以子进程方式调用
//! `ocr-helper`（独立 workspace + rustc 1.95，见 `ocr-helper/README.md`）。
//!
//! ## helper 定位（按优先级）
//! 1. 环境变量 `SEARCH_TOOL_OCR_HELPER`（显式路径）
//! 2. 可执行文件同级 `ocr-helper`（打包态）
//! 3. 源码树 `../ocr-helper/target/release/ocr-helper`（开发态）
//!
//! ## 开关
//! 图片与扫描型 PDF 的 OCR **默认关闭**（plan §5.2 `ocr_images=false`），
//! 设 `SEARCH_TOOL_OCR=1` 启用。未启用或无 helper 时，相关文件记 `Unsupported` 跳过
//! （不记为失败，也不阻塞索引）。
//!
//! ## 并发
//! 单 helper 进程按请求串行处理，故全局互斥串行化 OCR 调用（plan 风险 R2：
//! 避免 OCR 内存峰值与索引线程叠加）。

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::Deserialize;

use crate::extract::ExtractError;

/// OCR 是否启用（默认关；`SEARCH_TOOL_OCR=1` 开）
pub fn enabled() -> bool {
    matches!(
        std::env::var("SEARCH_TOOL_OCR").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

/// helper 可执行文件名（Windows 需 .exe 后缀，否则同级/源码树自动定位失效）
#[cfg(windows)]
const HELPER_NAME: &str = "ocr-helper.exe";
#[cfg(not(windows))]
const HELPER_NAME: &str = "ocr-helper";

/// 定位 helper 可执行文件
pub fn helper_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("SEARCH_TOOL_OCR_HELPER") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for cand in [
                dir.join(HELPER_NAME),
                dir.join("..").join("ocr-helper").join(HELPER_NAME),
            ] {
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    // 开发态：源码树
    let dev = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("ocr-helper")
        .join("target")
        .join("release")
        .join(HELPER_NAME);
    dev.is_file().then_some(dev)
}

/// helper 是否可用（启用开关 + 二进制存在）
pub fn available() -> bool {
    enabled() && helper_path().is_some()
}

#[derive(Debug, Deserialize)]
struct HelperResponse {
    ok: bool,
    #[serde(default)]
    lines: Vec<HelperLine>,
    #[serde(default)]
    image_width: u32,
    #[serde(default)]
    image_height: u32,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HelperLine {
    text: String,
    #[serde(default)]
    confidence: f32,
    /// 像素坐标 [x, y, w, h]（左上原点）
    #[serde(default, rename = "box")]
    bbox: [f32; 4],
}

/// OCR 文本块（**归一化**坐标 0..1，左上原点）——与 PDF 文字层 bbox 同口径，
/// 供预览页视图统一叠框（P5）
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrBox {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// 低置信度阈值：低于此值的行视为噪声（既不计入文本，也不画框）
const MIN_CONFIDENCE: f32 = 0.3;

fn io_err(msg: String) -> ExtractError {
    ExtractError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, msg))
}

/// 全局串行化：helper 一次只处理一个请求
fn lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

/// 进程内唯一的临时文件路径。
///
/// 原子序号保证并发唯一：曾经仅靠「进程 id + 毫秒时间戳 + idx」，
/// 索引器 `par_iter` 下两张图同一毫秒落同一临时文件，先完成方删除文件
/// 致后到方读到空/不存在文件（OCR 偶发丢文档，R35 联调实测复现）。
/// 序号与锁外写入段叠加后路径必然唯一。
fn tmp_image_path(idx: u32, ext: &str) -> PathBuf {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "st-ocr-{}-{}-{idx}-{seq}.{ext}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ))
}

/// 调用 helper 处理**单个图像文件**，返回原始响应（供文本/bbox 两条路径共用）
fn call_helper(path: &Path) -> Result<HelperResponse, ExtractError> {
    let helper = helper_path().ok_or_else(|| {
        ExtractError::Unsupported("OCR helper 未就绪（见 ocr-helper/README.md）".to_string())
    })?;

    let _guard = lock().lock().unwrap_or_else(|e| e.into_inner());

    let req = serde_json::json!({ "image_path": path.to_string_lossy() }).to_string();
    let mut child = Command::new(&helper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| io_err(format!("启动 OCR helper 失败: {e}")))?;

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| io_err("无法写入 OCR helper stdin".to_string()))?;
        stdin
            .write_all(req.as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .map_err(|e| io_err(format!("写入 OCR 请求失败: {e}")))?;
    } // drop stdin → helper 读到 EOF

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io_err("无法读取 OCR helper stdout".to_string()))?;
    let mut line = String::new();
    BufReader::new(stdout)
        .read_line(&mut line)
        .map_err(|e| io_err(format!("读取 OCR 响应失败: {e}")))?;
    let _ = child.wait();

    if line.trim().is_empty() {
        return Err(io_err("OCR helper 无响应".to_string()));
    }
    let resp: HelperResponse = serde_json::from_str(line.trim())
        .map_err(|e| io_err(format!("OCR 响应解析失败: {e}; 原文: {}", line.trim())))?;
    if !resp.ok {
        return Err(io_err(format!(
            "OCR 失败: {}",
            resp.error.unwrap_or_else(|| "未知错误".to_string())
        )));
    }
    Ok(resp)
}

/// 调用 helper 对**单个图像文件**做 OCR，返回识别文本（行以 \n 连接）。
///
/// 失败语义：helper 缺失 → `Unsupported`（可跳过）；helper 报错 → `Io`。
pub fn ocr_image_file(path: &Path) -> Result<String, ExtractError> {
    let resp = call_helper(path)?;
    Ok(resp
        .lines
        .iter()
        .filter(|l| l.confidence >= MIN_CONFIDENCE)
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n"))
}

/// 图像文件 → **归一化文本块**（坐标 0..1，左上原点）
fn ocr_image_file_boxes(path: &Path) -> Result<Vec<OcrBox>, ExtractError> {
    let resp = call_helper(path)?;
    let (iw, ih) = (
        resp.image_width.max(1) as f32,
        resp.image_height.max(1) as f32,
    );
    Ok(resp
        .lines
        .iter()
        .filter(|l| l.confidence >= MIN_CONFIDENCE && !l.text.trim().is_empty())
        .map(|l| OcrBox {
            text: l.text.clone(),
            x: (l.bbox[0] / iw).clamp(0.0, 1.0),
            y: (l.bbox[1] / ih).clamp(0.0, 1.0),
            w: (l.bbox[2] / iw).clamp(0.0, 1.0),
            h: (l.bbox[3] / ih).clamp(0.0, 1.0),
        })
        .collect())
}

/// 图片字节 → 落临时文件 → OCR → 文本（临时文件随即删除）
pub fn ocr_image_bytes(bytes: &[u8], ext: &str) -> Result<String, ExtractError> {
    let tmp = tmp_image_path(
        std::process::id() % 10_000,
        if ext.is_empty() { "png" } else { ext },
    );
    std::fs::write(&tmp, bytes)?;
    let r = ocr_image_file(&tmp);
    let _ = std::fs::remove_file(&tmp);
    let text = r?;
    if text.trim().is_empty() {
        return Err(ExtractError::Unsupported(
            "OCR 未识别出文本（空白或纯图形图片）".to_string(),
        ));
    }
    Ok(text)
}

/// 扫描型 PDF → 逐页渲染（PDFium）→ OCR → 拼接文本。
///
/// 依赖 PDFium（页渲染）与 helper（识别）；任一缺失返回 `Unsupported`。
pub fn ocr_pdf_bytes(bytes: &[u8]) -> Result<String, ExtractError> {
    if !available() {
        return Err(ExtractError::Unsupported(
            "扫描型 PDF 需 OCR（helper 未就绪或未启用 SEARCH_TOOL_OCR=1）".to_string(),
        ));
    }
    let pages = crate::extract::pdf::page_count(bytes)?;
    if pages == 0 {
        return Err(ExtractError::Unsupported("PDF 无页面".to_string()));
    }
    // 页数闸门：避免超大扫描件的 OCR 时间失控（plan：单文件熔断）
    const MAX_OCR_PAGES: usize = 200;
    if pages > MAX_OCR_PAGES {
        return Err(ExtractError::Unsupported(format!(
            "扫描型 PDF 页数 {pages} 超 OCR 上限 {MAX_OCR_PAGES}"
        )));
    }

    let mut out = String::new();
    for i in 0..pages {
        let png = crate::extract::pdf::render_page_png(bytes, i, 1000)?;
        let text = ocr_image_bytes(&png, "png")?;
        if !text.trim().is_empty() {
            out.push_str(&format!("===== 第 {} 页（OCR）=====\n", i + 1));
            out.push_str(&text);
            out.push('\n');
        }
    }
    if out.trim().is_empty() {
        return Err(ExtractError::Unsupported(
            "扫描型 PDF OCR 未得到文本".to_string(),
        ));
    }
    Ok(out)
}

/// 图片字节 → 归一化文本块（落临时文件 → helper → 归一化 → 删除临时文件）
pub fn ocr_image_boxes(bytes: &[u8], ext: &str) -> Result<Vec<OcrBox>, ExtractError> {
    let tmp = tmp_image_path(
        std::process::id() % 10_000,
        if ext.is_empty() { "png" } else { ext },
    );
    std::fs::write(&tmp, bytes)?;
    let r = ocr_image_file_boxes(&tmp);
    let _ = std::fs::remove_file(&tmp);
    r
}

/// **扫描型 PDF** 指定页 → 归一化文本块（PDFium 渲染该页 → OCR → 归一化）
///
/// 供预览页视图在"无文字层"时叠框（与 PDF 文字层 bbox 同口径）。
pub fn ocr_pdf_page_boxes(
    bytes: &[u8],
    page_index: usize,
    target_width: u32,
) -> Result<Vec<OcrBox>, ExtractError> {
    if !available() {
        return Err(ExtractError::Unsupported(
            "扫描型 PDF 需 OCR（helper 未就绪或未启用 SEARCH_TOOL_OCR=1）".to_string(),
        ));
    }
    let png = crate::extract::pdf::render_page_png(bytes, page_index, target_width)?;
    ocr_image_boxes(&png, "png")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_path_resolution_does_not_panic() {
        // 无论 helper 是否就绪都不得 panic；未就绪时为 None
        let _ = helper_path();
        let _ = available();
    }

    #[test]
    fn disabled_by_default() {
        // 未显式设置 SEARCH_TOOL_OCR 时应为关闭（测试进程环境变量未设）
        if std::env::var("SEARCH_TOOL_OCR").is_err() {
            assert!(!enabled(), "默认应关闭");
        }
    }

    #[test]
    fn ocr_without_helper_is_unsupported() {
        // 强制走无 helper 分支（临时改环境变量指向不存在路径）
        // 注意：进程内环境变量影响并行测试，这里用直接调用 helper_path 的判定代替
        if helper_path().is_none() {
            let e = ocr_image_file(Path::new("/tmp/not-exist.png")).unwrap_err();
            assert!(matches!(e, ExtractError::Unsupported(_)), "{e}");
        }
    }
}
