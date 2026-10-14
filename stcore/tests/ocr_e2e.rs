//! OCR 全链路端到端联调（fixtures 内置，可重跑；`#[ignore]`，手动运行）：
//!
//! ```bash
//! SEARCH_TOOL_OCR=1 \
//!   SEARCH_TOOL_OCR_HELPER=$PWD/ocr-helper/target/release/ocr-helper \
//!   cargo test -p stcore --test ocr_e2e -- --ignored --nocapture
//! ```
//!
//! fixtures 由 Python3 + PIL 生成（Noto Serif CJK SC，1200px 宽）：
//! - `clean.png`   清晰 4 行中文 + 数字英文混排
//! - `scanned.png` 同文本 + 0.8° 旋转 + 高斯噪声 + 模糊 + 降对比度
//! - `scanned.pdf` PIL 直接 save as PDF（无文字层，走 OCR 通道）

use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

#[test]
#[ignore = "依赖 ocr-helper 二进制与模型：手动运行"]
fn scanned_pdf_page_boxes_normalized_and_keywords() {
    if !stcore::ocr::available() {
        eprintln!("跳过：OCR 未启用或 helper 未就绪（设 SEARCH_TOOL_OCR=1 并构建 ocr-helper）");
        return;
    }
    let bytes =
        std::fs::read(fixture("scanned.pdf")).expect("fixtures/scanned.pdf 应可读（仓库内置）");

    let boxes = stcore::ocr::ocr_pdf_page_boxes(&bytes, 0, 1000).expect("扫描页 OCR bbox");
    assert!(!boxes.is_empty(), "扫描页应识别出文本块");
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
        assert!(!b.text.trim().is_empty(), "文本块不应为空");
    }
    println!(
        "[ocr-e2e] scanned.pdf 第 1 页 {} 块，示例：{:?}",
        boxes.len(),
        boxes.first()
    );
    let all: String = boxes.iter().map(|b| b.text.as_str()).collect();
    assert!(
        all.contains("量子计算") && all.contains("42.5%"),
        "应识别出关键词，实际: {all}"
    );
}

/// 并发回归（竞态修复守护）：索引器 par_iter 会并发触发图片 OCR。
/// 曾经 `tmp_image_path` 仅毫秒精度，同毫秒多图撞同一临时文件导致
/// 偶发读空文件丢文档。多线程同时 OCR 不同 fixture，全部必须成功。
#[test]
#[ignore = "依赖 ocr-helper 二进制与模型：手动运行"]
fn concurrent_image_ocr_no_temp_file_collision() {
    if !stcore::ocr::available() {
        eprintln!("跳过：OCR 未启用或 helper 未就绪");
        return;
    }
    let images = [
        std::fs::read(fixture("clean.png")).expect("fixtures/clean.png 应可读"),
        std::fs::read(fixture("scanned.png")).expect("fixtures/scanned.png 应可读"),
    ];
    let ext = "png";
    let workers = 8;
    let per_worker = 3;

    let handles: Vec<_> = (0..workers)
        .map(|w| {
            let images = images.clone();
            std::thread::spawn(move || {
                for i in 0..per_worker {
                    let bytes = &images[(w + i) % images.len()];
                    let text = stcore::ocr::ocr_image_bytes(bytes, ext)
                        .unwrap_or_else(|e| panic!("worker {w} 第 {i} 次 OCR 失败: {e}"));
                    assert!(
                        text.contains("量子计算"),
                        "worker {w} 第 {i} 次识别文本异常: {text}"
                    );
                }
            })
        })
        .collect();
    for h in handles {
        h.join().expect("OCR 工作线程不应 panic");
    }
    println!("[ocr-e2e] 并发 {workers}×{per_worker} 次 OCR 全部成功（无临时文件碰撞）");
}

#[test]
#[ignore = "依赖 ocr-helper 二进制与模型：手动运行"]
fn image_boxes_normalized_and_keywords() {
    if !stcore::ocr::available() {
        eprintln!("跳过：OCR 未启用或 helper 未就绪（设 SEARCH_TOOL_OCR=1 并构建 ocr-helper）");
        return;
    }
    let bytes =
        std::fs::read(fixture("scanned.png")).expect("fixtures/scanned.png 应可读（仓库内置）");

    let boxes = stcore::ocr::ocr_image_boxes(&bytes, "png").expect("扫描图 OCR bbox");
    assert!(!boxes.is_empty(), "扫描图应识别出文本块");
    for b in &boxes {
        assert!(
            (0.0..=1.0).contains(&b.x)
                && (0.0..=1.0).contains(&b.y)
                && b.w > 0.0
                && b.h > 0.0
                && b.w <= 1.0
                && b.h <= 1.0,
            "bbox 应归一化且在图内: {b:?}"
        );
    }
    println!(
        "[ocr-e2e] scanned.png {} 块，示例：{:?}",
        boxes.len(),
        boxes.first()
    );
    let all: String = boxes.iter().map(|b| b.text.as_str()).collect();
    assert!(
        all.contains("量子计算") && all.contains("42.5%"),
        "应识别出关键词，实际: {all}"
    );
}
