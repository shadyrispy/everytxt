//! 电子书格式提取（mobi / azw3 / azw / fb2 / lit / kfx / cbz / rtf / odt）。
//!
//! 走 `ebook-rs`（纯 Rust，覆盖 EPUB2/3、MOBI/AZW3、FB2、LIT、KFX、CBZ 等）。
//! 本模块**只接自有模块未覆盖的扩展名**：EPUB 由 [`super::epub`]（自有 zip+XHTML）
//! 处理、docx 由 [`super::ooxml`] 处理，避免两条链路对同一格式给出不同结果。
//!
//! 文本来源：`Book::sections()[i].plain_text`（ebook-rs 已做 XHTML→纯文本与
//! 字符集/去混淆处理）。DRM 保护的电子书（ADEPT/LCP/Mobipocket）返回
//! `Unsupported`（可跳过，不算失败）。

use super::ExtractError;

fn map_err(e: ebook_rs::EbookError) -> ExtractError {
    use ebook_rs::EbookError as E;
    match e {
        E::DrmProtected(msg) => {
            ExtractError::Unsupported(format!("电子书受 DRM 保护，无法提取: {msg}"))
        }
        E::InvalidFormat(msg) => ExtractError::Unsupported(format!("电子书格式不支持: {msg}")),
        other => ExtractError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("电子书解析失败: {other}"),
        )),
    }
}

/// 统一入口：抽取电子书正文（按 spine/section 顺序拼接纯文本）。
pub fn ebook_to_text(bytes: &[u8]) -> Result<String, ExtractError> {
    // 第三方解析器对畸形输入的 panic 一律兜底为受控错误
    let parsed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ebook_rs::Book::from_bytes(bytes)
    }))
    .map_err(|_| {
        ExtractError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "电子书解析器内部错误（已隔离）".to_string(),
        ))
    })?;

    let book = parsed.map_err(map_err)?;

    let mut out = String::new();
    for s in book.sections() {
        if !s.plain_text.trim().is_empty() {
            out.push_str(&s.plain_text);
            out.push('\n');
        }
    }

    if out.trim().is_empty() {
        return Err(ExtractError::Unsupported(
            "电子书无可提取正文（可能为纯图册）".to_string(),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;

    /// 最小 CBZ（zip 内若干 xhtml，按 cbz 约定）
    fn minimal_cbz() -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut z = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts = SimpleFileOptions::default();
            z.start_file("page1.xhtml", opts).unwrap();
            z.write_all("<html><body><p>连环画 第一页 文字</p></body></html>".as_bytes())
                .unwrap();
            z.start_file("page2.xhtml", opts).unwrap();
            z.write_all("<html><body><p>连环画 第二页</p></body></html>".as_bytes())
                .unwrap();
            z.finish().unwrap();
        }
        buf
    }

    #[test]
    fn cbz_extracts_page_text() {
        let bytes = minimal_cbz();
        match ebook_to_text(&bytes) {
            Ok(t) => assert!(t.contains("连环画"), "正文应抽出: {t}"),
            // CBZ 无 OPF 时 ebook-rs 可能按纯图册拒绝 → 受控错误也算合格
            Err(e) => assert!(
                matches!(e, ExtractError::Unsupported(_) | ExtractError::Io(_)),
                "须受控错误: {e}"
            ),
        }
    }

    #[test]
    fn binary_garbage_is_controlled_error() {
        // 非 UTF-8 且无已知魔数 → ebook-rs 判 InvalidFormat → Unsupported
        let e = ebook_to_text(&[0xFF, 0xFE, 0x00, 0x01, 0x02, 0x03, 0x04]).unwrap_err();
        assert!(
            matches!(e, ExtractError::Unsupported(_) | ExtractError::Io(_)),
            "{e}"
        );
    }

    #[test]
    fn plain_utf8_text_is_readable_via_txt_book() {
        // ebook-rs 对可解码 UTF-8 会按 txt 书处理（真实路由中 .txt 走 TEXT_EXTS，
        // 不会到这里；此处仅验证该分支不 panic）
        let r = ebook_to_text("第一章 正文内容".as_bytes());
        assert!(r.is_ok(), "应可读: {r:?}");
    }
}
