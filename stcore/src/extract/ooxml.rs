//! OOXML（docx / xlsx / pptx）文本提取。
//!
//! 原理：OOXML 文件是 zip 容器 + 一组 XML part。逐 part 流式抽文本节点
//! 即得正文，无需外部 office 库（纯 `zip` + `quick-xml`，零重依赖）。
//!
//! 覆盖矩阵：
//!
//! | 格式 | 抽取 part | 文本标签 | 分段标签 |
//! |---|---|---|---|
//! | docx / docm | `word/document.xml` | `<w:t>` | `<w:p>` |
//! | xlsx / xlsm / xlsb | `xl/sharedStrings.xml` + `xl/worksheets/sheet*.xml` | `<t>` / `<v>` | `<row>` |
//! | pptx / pptm / pps / ppsx / pot / potx | `ppt/slides/slide*.xml` | `<a:t>` | `<a:p>` |
//!
//! 防护：单 part 解压上限 `MAX_PART_BYTES`；参与抽取的 part 数上限 `MAX_PARTS`。
//! 加密 / 非 OOXML 伪装的 zip 包在缺关键 part 时返回 `Unsupported`（正常跳过）。

use std::io::{Cursor, Read};

use quick_xml::events::Event;
use quick_xml::Reader;
use zip::ZipArchive;

use super::{ExtractError, MAX_FILE_SIZE};

/// 单个 XML part 的解压上限（与单文件闸门同源）
const MAX_PART_BYTES: u64 = MAX_FILE_SIZE;
/// 参与抽取的 sheet / slide 数上限（防畸形包塞入海量 part）
const MAX_PARTS: usize = 2000;

/// 打开 zip 容器（错误归一到 Io，便于上层统一记失败）
pub(crate) fn open_zip(bytes: &[u8]) -> Result<ZipArchive<Cursor<&[u8]>>, ExtractError> {
    ZipArchive::new(Cursor::new(bytes)).map_err(|e| {
        ExtractError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e.to_string(),
        ))
    })
}

/// 读取 zip 内一个 part：缺失返回 `Ok(None)`；超限返回 `TooManyEntries`。
pub(crate) fn read_part(
    zip: &mut ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> Result<Option<Vec<u8>>, ExtractError> {
    let mut f = match zip.by_name(name) {
        Ok(f) => f,
        Err(_) => return Ok(None),
    };
    let mut buf = Vec::new();
    f.by_ref().take(MAX_PART_BYTES + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > MAX_PART_BYTES {
        return Err(ExtractError::TooManyEntries);
    }
    Ok(Some(buf))
}

/// 把 `&entity;` 解析为字符（内置 5 实体 + `&#NN;` / `&#xNN;` 字符引用）。
///
/// quick-xml 0.42 把实体引用作为独立 `Event::GeneralRef` 抛出，
/// 不处理会静默丢字符（如 `&amp;` 变空）。
fn push_entity(out: &mut String, r: &quick_xml::events::BytesRef<'_>) {
    if let Ok(Some(c)) = r.resolve_char_ref() {
        out.push(c);
        return;
    }
    match r.as_ref() {
        "amp" => out.push('&'),
        "lt" => out.push('<'),
        "gt" => out.push('>'),
        "quot" => out.push('"'),
        "apos" => out.push('\''),
        _ => {}
    }
}

/// 流式抽取：命中 `text_tag` 的文本累加；遇 `para_tag` 结束补换行。
fn extract_runs(xml: &[u8], text_tag: &str, para_tag: &str) -> String {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut out = String::new();
    let mut in_text = 0usize;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if e.name().as_ref() == text_tag {
                    in_text += 1;
                }
            }
            Ok(Event::End(e)) => {
                let n = e.name();
                if n.as_ref() == text_tag {
                    in_text = in_text.saturating_sub(1);
                } else if n.as_ref() == para_tag && !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            Ok(Event::Text(t)) => {
                if in_text > 0 {
                    out.push_str(&t.xml10_content());
                }
            }
            Ok(Event::GeneralRef(r)) => {
                if in_text > 0 {
                    push_entity(&mut out, &r);
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

/// 解析 `xl/sharedStrings.xml`：每个 `<si>` 内的所有 `<t>` 拼接为一个字符串（保序）。
fn shared_strings(xml: &[u8]) -> Vec<String> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut items = Vec::new();
    let mut cur = String::new();
    let mut in_t = false;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if e.name().as_ref() == "t" {
                    in_t = true;
                }
            }
            Ok(Event::End(e)) => {
                let n = e.name();
                if n.as_ref() == "t" {
                    in_t = false;
                } else if n.as_ref() == "si" {
                    items.push(std::mem::take(&mut cur));
                }
            }
            Ok(Event::Text(t)) => {
                if in_t {
                    cur.push_str(&t.xml10_content());
                }
            }
            Ok(Event::GeneralRef(r)) => {
                if in_t {
                    push_entity(&mut cur, &r);
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    items
}

/// 解析一个 sheet part：`<c>` 单元格取值（`t="s"` 查共享串表；否则取 `<v>` / 内联 `<t>`），
/// 单元格间制表符、`<row>` 结束换行。
fn sheet_to_text(xml: &[u8], shared: &[String]) -> String {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut out = String::new();
    let mut buf = Vec::new();
    let mut cur_shared = false;
    let mut cur = String::new();
    let mut in_v = false;
    let mut in_t = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let n = e.name();
                match n.as_ref() {
                    "c" => {
                        cur_shared = false;
                        cur.clear();
                        for a in e.attributes().flatten() {
                            if a.key.as_ref() == "t" && a.value.as_ref() == "s" {
                                cur_shared = true;
                            }
                        }
                    }
                    "v" => in_v = true,
                    "t" => in_t = true,
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let n = e.name();
                match n.as_ref() {
                    "v" => in_v = false,
                    "t" => in_t = false,
                    "c" => {
                        let cell = if cur_shared {
                            cur.trim()
                                .parse::<usize>()
                                .ok()
                                .and_then(|i| shared.get(i))
                                .cloned()
                                .unwrap_or_default()
                        } else {
                            cur.clone()
                        };
                        if !cell.is_empty() {
                            if !out.is_empty() && !out.ends_with('\n') && !out.ends_with('\t') {
                                out.push('\t');
                            }
                            out.push_str(cell.trim());
                        }
                    }
                    "row" if !out.ends_with('\n') && !out.is_empty() => {
                        out.push('\n');
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(t)) => {
                if in_v || in_t {
                    cur.push_str(&t.xml10_content());
                }
            }
            Ok(Event::GeneralRef(r)) => {
                if in_v || in_t {
                    push_entity(&mut cur, &r);
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

fn is_sheet(name: &str) -> bool {
    name.starts_with("xl/worksheets/") && name.ends_with(".xml")
}

fn is_slide(name: &str) -> bool {
    name.starts_with("ppt/slides/slide") && name.ends_with(".xml")
}

/// 从 `ppt/slides/slide12.xml` 抽序号（排序用；无数字退化为 0）
fn slide_index(name: &str) -> u32 {
    name.trim_start_matches("ppt/slides/slide")
        .trim_end_matches(".xml")
        .parse()
        .unwrap_or(0)
}

/// 统一入口：按扩展名抽取 OOXML 正文（markdown 风格纯文本）。
pub fn to_text(ext: &str, bytes: &[u8]) -> Result<String, ExtractError> {
    match ext {
        "docx" | "docm" => {
            let mut zip = open_zip(bytes)?;
            let Some(xml) = read_part(&mut zip, "word/document.xml")? else {
                return Err(ExtractError::Unsupported(
                    "docx 缺少 word/document.xml（加密或非 OOXML）".to_string(),
                ));
            };
            Ok(extract_runs(&xml, "w:t", "w:p"))
        }
        "xlsx" | "xlsm" | "xlsb" => {
            let mut zip = open_zip(bytes)?;
            let shared = match read_part(&mut zip, "xl/sharedStrings.xml")? {
                Some(x) => shared_strings(&x),
                None => Vec::new(),
            };
            let mut names: Vec<String> = zip
                .file_names()
                .filter(|n| is_sheet(n))
                .map(|s| s.to_string())
                .collect();
            if names.len() > MAX_PARTS {
                return Err(ExtractError::TooManyEntries);
            }
            names.sort();
            let mut out = String::new();
            for n in names {
                if let Some(xml) = read_part(&mut zip, &n)? {
                    out.push_str(&format!("\n===== {n} =====\n"));
                    out.push_str(&sheet_to_text(&xml, &shared));
                }
            }
            Ok(out)
        }
        "pptx" | "pptm" | "pps" | "ppsx" | "pot" | "potx" => {
            let mut zip = open_zip(bytes)?;
            let mut names: Vec<String> = zip
                .file_names()
                .filter(|n| is_slide(n))
                .map(|s| s.to_string())
                .collect();
            if names.len() > MAX_PARTS {
                return Err(ExtractError::TooManyEntries);
            }
            names.sort_by_key(|n| slide_index(n));
            let mut out = String::new();
            for n in names {
                if let Some(xml) = read_part(&mut zip, &n)? {
                    out.push_str(&format!("\n===== {n} =====\n"));
                    out.push_str(&extract_runs(&xml, "a:t", "a:p"));
                }
            }
            Ok(out)
        }
        _ => Err(ExtractError::Unsupported(format!("非 OOXML 扩展名: {ext}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
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

    #[test]
    fn docx_extracts_runs_and_paragraphs() {
        let xml = r#"<?xml version="1.0"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body>
  <w:p><w:r><w:t>第一段</w:t></w:r><w:r><w:t xml:space="preserve"> 续</w:t></w:r></w:p>
  <w:p><w:r><w:t>第二段 &amp; 实体</w:t></w:r></w:p>
</w:body></w:document>"#;
        let zip = build_zip(&[("word/document.xml", xml)]);
        let t = to_text("docx", &zip).unwrap();
        assert!(t.contains("第一段 续"), "run 应拼接: {t}");
        assert!(t.contains("第二段 & 实体"), "实体应解码: {t}");
        // 段落之间换行
        let l1 = t.find("第一段").unwrap();
        let l2 = t.find("第二段").unwrap();
        assert!(t[l1..l2].contains('\n'), "段间应有换行: {t:?}");
    }

    #[test]
    fn xlsx_uses_shared_strings_and_inline_values() {
        let shared = r#"<?xml version="1.0"?><sst><si><t>姓名</t></si><si><t>年龄</t></si><si><t>张三</t></si></sst>"#;
        let sheet = r#"<?xml version="1.0"?><worksheet><sheetData>
<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row>
<row r="2"><c r="A2" t="s"><v>2</v></c><c r="B2"><v>30</v></c></row>
</sheetData></worksheet>"#;
        let zip = build_zip(&[
            ("xl/sharedStrings.xml", shared),
            ("xl/worksheets/sheet1.xml", sheet),
        ]);
        let t = to_text("xlsx", &zip).unwrap();
        assert!(t.contains("姓名"), "共享串应解出: {t}");
        assert!(t.contains("\u{5f20}\u{4e09}"), "字符串单元格应解出: {t}");
        assert!(t.contains("30"), "数值单元格应解出: {t}");
        assert!(t.contains("姓名\t年龄"), "同行单元格制表符分隔: {t:?}");
    }

    #[test]
    fn pptx_extracts_slide_text_in_order() {
        let s1 = r#"<?xml version="1.0"?><p:sld xmlns:a="x" xmlns:p="y"><p:cSld><a:p><a:r><a:t>封面标题</a:t></a:r></a:p></p:cSld></p:sld>"#;
        let s2 = r#"<?xml version="1.0"?><p:sld xmlns:a="x" xmlns:p="y"><p:cSld><a:p><a:r><a:t>第二页</a:t></a:r></a:p></p:cSld></p:sld>"#;
        let zip = build_zip(&[("ppt/slides/slide2.xml", s2), ("ppt/slides/slide1.xml", s1)]);
        let t = to_text("pptx", &zip).unwrap();
        let i1 = t.find("封面标题").unwrap();
        let i2 = t.find("第二页").unwrap();
        assert!(i1 < i2, "slide 应按序号而非字典序: {t}");
    }

    #[test]
    fn missing_document_part_is_unsupported() {
        let zip = build_zip(&[("docProps/app.xml", "<x/>")]);
        let err = to_text("docx", &zip).unwrap_err();
        assert!(matches!(err, ExtractError::Unsupported(_)), "{err}");
    }

    #[test]
    fn corrupt_ooxml_is_io_error() {
        let err = to_text("docx", b"not a zip at all").unwrap_err();
        assert!(matches!(err, ExtractError::Io(_)), "{err}");
    }
}
