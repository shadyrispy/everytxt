//! P5：扫描型 PDF 的 OCR bbox —— 与 PDF 文字层 bbox 同口径（归一化 0..1，左上原点）。
//!
//! 依赖 helper 二进制与模型（`#[ignore]`，手动运行）：
//! ```bash
//! SEARCH_TOOL_OCR=1 cargo test -p stcore --test p5_ocr_boxes -- --ignored --nocapture
//! ```
//! 样例路径可用 `P5_SCANNED_PDF` 覆盖（默认 `/tmp/p4-samples/扫描白皮书.pdf`）。

#[test]
#[ignore = "依赖 ocr-helper 二进制与模型：手动运行"]
fn scanned_pdf_page_boxes_are_normalized() {
    let path = std::env::var("P5_SCANNED_PDF")
        .unwrap_or_else(|_| "/tmp/p4-samples/扫描白皮书.pdf".to_string());
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("跳过：样例不可读（{path}）: {e}");
            return;
        }
    };
    if !stcore::ocr::available() {
        eprintln!("跳过：OCR 未启用或 helper 未就绪（设 SEARCH_TOOL_OCR=1 并构建 ocr-helper）");
        return;
    }

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
    println!("[ocr-boxes] {} 块，示例：{:?}", boxes.len(), boxes.first());
    assert!(
        boxes.iter().any(|b| b.text.contains("Inverted")),
        "应识别出样例正文: {:?}",
        boxes.iter().map(|b| &b.text).collect::<Vec<_>>()
    );
}
