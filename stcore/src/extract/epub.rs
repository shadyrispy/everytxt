//! EPUB 文本提取（zip 容器 + XHTML 正文）。
//!
//! 流程：`META-INF/container.xml` → OPF 路径 → OPF 的 `manifest`（id→href）
//! 与 `spine`（idref 阅读顺序）→ 逐章读 XHTML 并复用 [`crate::extract::html`] 转文本。
//!
//! 覆盖：EPUB 2/3 的文字型电子书；内嵌样式与脚本按 html 层规则剔除。
//! 不做：DRM 加密包（缺 container/opf 时返回 `Unsupported`）。

use quick_xml::events::Event;
use quick_xml::Reader;

use super::{
    html,
    ooxml::{open_zip, read_part},
    ExtractError,
};
use crate::text::decode_bytes;

/// 从 `META-INF/container.xml` 取第一个 rootfile 的 full-path
fn parse_container(xml: &[u8]) -> Option<String> {
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if e.name().as_ref() == "rootfile" {
                    for a in e.attributes().flatten() {
                        if a.key.as_ref() == "full-path" {
                            return Some(a.value.as_ref().to_string());
                        }
                    }
                }
            }
            Ok(Event::Eof) => return None,
            Err(_) => return None,
            _ => {}
        }
        buf.clear();
    }
}

/// 解析 OPF：返回 (id→href manifest, 阅读顺序目录列表)
fn parse_opf(xml: &[u8]) -> (Vec<(String, String)>, Vec<String>) {
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut manifest: Vec<(String, String)> = Vec::new();
    let mut spine: Vec<String> = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = e.name();
                match name.as_ref() {
                    "item" => {
                        let mut id = None;
                        let mut href = None;
                        for a in e.attributes().flatten() {
                            match a.key.as_ref() {
                                "id" => id = Some(a.value.as_ref().to_string()),
                                "href" => href = Some(a.value.as_ref().to_string()),
                                _ => {}
                            }
                        }
                        if let (Some(id), Some(href)) = (id, href) {
                            manifest.push((id, href));
                        }
                    }
                    "itemref" => {
                        for a in e.attributes().flatten() {
                            if a.key.as_ref() == "idref" {
                                spine.push(a.value.as_ref().to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    (manifest, spine)
}

/// 拼接 base 目录与相对 href（href 已是绝对路径则不拼）
fn join_href(base: &str, href: &str) -> String {
    if base.is_empty() || href.starts_with('/') {
        href.to_string()
    } else {
        format!("{base}/{href}")
    }
}

/// 统一入口：抽取 EPUB 正文（按 spine 顺序拼接各章纯文本）。
pub fn epub_to_text(bytes: &[u8]) -> Result<String, ExtractError> {
    let mut zip = open_zip(bytes)?;

    let Some(container) = read_part(&mut zip, "META-INF/container.xml")? else {
        return Err(ExtractError::Unsupported(
            "epub 缺少 META-INF/container.xml（加密或非 EPUB）".to_string(),
        ));
    };
    let Some(opf_path) = parse_container(&container) else {
        return Err(ExtractError::Unsupported(
            "epub container 无 rootfile".to_string(),
        ));
    };
    let Some(opf) = read_part(&mut zip, &opf_path)? else {
        return Err(ExtractError::Unsupported(format!(
            "epub 缺少 OPF: {opf_path}"
        )));
    };
    let (manifest, spine) = parse_opf(&opf);

    // OPF 所在目录作为相对 href 的基准
    let base = opf_path
        .rsplit_once('/')
        .map(|(d, _)| d.to_string())
        .unwrap_or_default();

    let mut out = String::new();
    for idref in &spine {
        let Some((_, href)) = manifest.iter().find(|(id, _)| id == idref) else {
            continue;
        };
        let path = join_href(&base, href);
        if let Some(raw) = read_part(&mut zip, &path)? {
            let html_text = html::html_to_text(&decode_bytes(&raw).0);
            if !html_text.trim().is_empty() {
                out.push_str(&html_text);
                out.push('\n');
            }
        }
    }

    if out.trim().is_empty() {
        return Err(ExtractError::Unsupported(
            "epub spine 无可提取正文章节".to_string(),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;

    fn build_zip(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut z = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts = SimpleFileOptions::default();
            for (name, body) in parts {
                z.start_file(*name, opts).unwrap();
                z.write_all(body.as_bytes()).unwrap();
            }
            z.finish().unwrap();
        }
        buf
    }

    fn sample_epub() -> Vec<u8> {
        let container = r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;
        let opf = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <manifest>
    <item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="text/ch2.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine><itemref idref="c1"/><itemref idref="c2"/></spine>
</package>"#;
        let ch1 =
            r#"<html><body><h1>第一章</h1><p>正文 一。</p><script>var x=1;</script></body></html>"#;
        let ch2 = r#"<html><body><h1>第二章</h1><p>正文 二。</p></body></html>"#;
        build_zip(&[
            ("META-INF/container.xml", container),
            ("OEBPS/content.opf", opf),
            ("OEBPS/ch1.xhtml", ch1),
            ("OEBPS/text/ch2.xhtml", ch2),
        ])
    }

    #[test]
    fn epub_reads_spine_chapters_in_order() {
        let t = epub_to_text(&sample_epub()).unwrap();
        let i1 = t.find("第一章").expect("第一章缺失");
        let i2 = t.find("第二章").expect("第二章缺失");
        assert!(i1 < i2, "按 spine 顺序: {t}");
        assert!(t.contains("正文 一"), "正文应提取: {t}");
        assert!(!t.contains("var x=1"), "script 应剔除: {t}");
    }

    #[test]
    fn epub_subdir_href_is_resolved() {
        let t = epub_to_text(&sample_epub()).unwrap();
        assert!(t.contains("正文 二"), "子目录 href 应正确拼接: {t}");
    }

    #[test]
    fn epub_without_container_is_unsupported() {
        let zip = build_zip(&[("mimetype", "application/epub+zip")]);
        let err = epub_to_text(&zip).unwrap_err();
        assert!(matches!(err, ExtractError::Unsupported(_)), "{err}");
    }

    #[test]
    fn epub_corrupt_is_io_error() {
        let err = epub_to_text(b"not a zip").unwrap_err();
        assert!(matches!(err, ExtractError::Io(_)), "{err}");
    }
}
