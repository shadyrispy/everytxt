//! P1 格式扩展集成测试：`extract_path` 端到端分发 + 真实 PDF 夹具。
//!
//! 单测已在各模块内覆盖抽取逻辑；这里验证**路由接线**（mod.rs 的 dispatch）
//! 与真实文件的受控失败路径。

use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn english_pdf_dispatches_and_extracts() {
    let r = stcore::extract::extract_path(&fixture("en.pdf")).expect("标准编码 PDF 应可提取");
    assert_eq!(r.format, "pdf");
    assert!(r.text.contains("Inverted"), "文字层应抽出: {:?}", r.text);
    assert!(r.children.is_empty());
}

#[test]
fn cid_chinese_pdf_extracts_or_fails_controlled_without_panic() {
    // 中文 CID（UniGB-UCS2-H）PDF：
    // - PDFium 通道可用 → 应成功抽出中文正文（本批升级目标）
    // - 库缺失（纯 pdf-extract 兜底）→ 须为受控错误，绝不 panic
    let r = stcore::extract::extract_path(&fixture("cid-cn.pdf"));
    match r {
        Ok(ok) => {
            assert_eq!(ok.format, "pdf");
            assert!(
                ok.text.contains("全文索引")
                    || ok.text.contains("倒排索引")
                    || ok.text.contains("白皮书"),
                "PDFium 已提取但未见中文正文: {:?}",
                ok.text
            );
        }
        Err(stcore::extract::ExtractError::Unsupported(_))
        | Err(stcore::extract::ExtractError::Io(_)) => {}
        Err(e) => panic!("非受控错误: {e}"),
    }
}

/// 临时目录写入字节后按路径提取（模拟真实文件）
fn extract_bytes_as(name: &str, bytes: &[u8]) -> Result<stcore::extract::ExtractResult, String> {
    let dir = std::env::temp_dir().join(format!("st-p1-{}-{}", std::process::id(), name));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let p = dir.join(name);
    std::fs::write(&p, bytes).map_err(|e| e.to_string())?;
    let r = stcore::extract::extract_path(&p).map_err(|e| e.to_string());
    let _ = std::fs::remove_dir_all(&dir);
    r
}

#[test]
fn docx_dispatches_via_extract_path() {
    use std::io::Write;
    let mut buf = Vec::new();
    {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default();
        z.start_file("word/document.xml", opts).unwrap();
        z.write_all(
            r#"<w:document xmlns:w="x"><w:body><w:p><w:r><w:t>季度报告 量子计算</w:t></w:r></w:p></w:body></w:document>"#
                .as_bytes(),
        )
        .unwrap();
        z.finish().unwrap();
    }
    let r = extract_bytes_as("report.docx", &buf).expect("docx 应可提取");
    assert_eq!(r.format, "docx");
    assert!(r.text.contains("量子计算"), "{:?}", r.text);
}

#[test]
fn xlsx_dispatches_via_extract_path() {
    use std::io::Write;
    let mut buf = Vec::new();
    {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default();
        z.start_file("xl/sharedStrings.xml", opts).unwrap();
        z.write_all(r#"<sst><si><t>华北</t></si></sst>"#.as_bytes())
            .unwrap();
        z.start_file("xl/worksheets/sheet1.xml", opts).unwrap();
        z.write_all(
            r#"<worksheet><sheetData><row><c t="s"><v>0</v></c></row></sheetData></worksheet>"#
                .as_bytes(),
        )
        .unwrap();
        z.finish().unwrap();
    }
    let r = extract_bytes_as("ledger.xlsx", &buf).expect("xlsx 应可提取");
    assert_eq!(r.format, "xlsx");
    assert!(r.text.contains("华北"), "{:?}", r.text);
}

#[test]
fn epub_dispatches_via_extract_path() {
    use std::io::Write;
    let mut buf = Vec::new();
    {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default();
        let parts: [(&str, &str); 3] = [
            (
                "META-INF/container.xml",
                r#"<container><rootfiles><rootfile full-path="OEBPS/c.opf"/></rootfiles></container>"#,
            ),
            (
                "OEBPS/c.opf",
                r#"<package><manifest><item id="c1" href="ch1.xhtml"/></manifest><spine><itemref idref="c1"/></spine></package>"#,
            ),
            (
                "OEBPS/ch1.xhtml",
                "<html><body><p>分布式 存储 架构</p></body></html>",
            ),
        ];
        for (n, b) in parts {
            z.start_file(n, opts).unwrap();
            z.write_all(b.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }
    let r = extract_bytes_as("book.epub", &buf).expect("epub 应可提取");
    assert_eq!(r.format, "epub");
    assert!(r.text.contains("分布式"), "{:?}", r.text);
}

#[test]
fn unknown_extension_stays_unsupported() {
    let r = extract_bytes_as("file.unknownext", b"whatever");
    assert!(r.is_err());
    assert!(r.unwrap_err().contains("不支持"), "应报 Unsupported");
}
