//! EML → 纯文本：Subject/From/Date 头 + 正文。
//!
//! 正文策略（mail-parser 0.10 实证修正）：**HTML 优先**——
//! mail-parser 自带的 html→text 不剔除 script/style
//! （实测 html-only 邮件 `body_text` 返回 "Visible text\ntrack()"，
//! script 内容混入正文），因此有 text/html 部分时必须拿原文走
//! 我们自己的 html_fragment_to_text（与 .html 文件同一套转换）。
//! 无 HTML 部分时才用 text/plain。
//! mail-parser 自身处理 MIME 编码/quoted-printable/base64 与 RFC2047 头解码。

use mail_parser::{Message, MessageParser};

use super::html::html_fragment_to_text;
use crate::text::decode_bytes;

/// 解析 eml 字节流，拼装为可索引文本。
///
/// 布局：`Subject: ..\nFrom: ..\nDate: ..\n\n{body}`，缺失的头跳过。
/// 连头部都提不出东西时（如误把非邮件文本当 eml）退回原文解码：
/// 邮件格式千奇百怪，不因解析失败丢掉可索引内容。
pub fn eml_to_text(bytes: &[u8]) -> String {
    let mut out = String::new();
    if let Some(message) = MessageParser::default().parse(bytes) {
        if let Some(subject) = message.subject() {
            out.push_str("Subject: ");
            out.push_str(subject);
            out.push('\n');
        }
        if let Some(from) = message.from() {
            out.push_str("From: ");
            out.push_str(&format_address(from));
            out.push('\n');
        }
        if let Some(date) = message.date() {
            out.push_str("Date: ");
            out.push_str(&date.to_rfc822());
            out.push('\n');
        }
        let body = extract_body(&message);
        if !body.is_empty() {
            out.push('\n');
            out.push_str(&body);
        }
    }
    // 什么都提不出（无头无正文的纯文本被误判为邮件）：退回原文解码
    if out.is_empty() {
        return decode_bytes(bytes).0;
    }
    out
}

/// 地址列表格式化为 "Name <addr>"，多地址用逗号连接。
fn format_address(address: &mail_parser::Address) -> String {
    address
        .iter()
        .map(|addr| match (addr.name(), addr.address()) {
            (Some(name), Some(email)) => format!("{name} <{email}>"),
            (None, Some(email)) => email.to_string(),
            (Some(name), None) => name.to_string(),
            (None, None) => String::new(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// 取正文：**HTML 优先**——mail-parser 的 body_text 对 html part
/// 返回不剔 script 的脏文本（0.10 实证），必须拿 html 原文自转换。
/// multipart/alternative（plain+html 等价内容）也用 html 版，
/// 保证 script/style 一律剔除；只有纯 plain 邮件才走 body_text。
fn extract_body(message: &Message) -> String {
    let html_count = message.html_body_count();
    if html_count > 0 {
        return (0..html_count)
            .filter_map(|i| message.body_html(i))
            .map(|html| html_fragment_to_text(&html))
            .collect::<Vec<_>>()
            .join("\n");
    }
    let text_count = message.text_body_count();
    if text_count > 0 {
        return (0..text_count)
            .filter_map(|i| message.body_text(i))
            .map(|cow| cow.into_owned())
            .collect::<Vec<_>>()
            .join("\n");
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_email() {
        let raw = b"From: Alice <alice@example.com>\r\n\
            To: Bob <bob@example.com>\r\n\
            Subject: =?UTF-8?B?5L2g5aW95LiW55WM?=\r\n\
            Date: Tue, 13 Oct 2026 10:00:00 +0800\r\n\
            Content-Type: text/plain; charset=UTF-8\r\n\
            \r\n\
            Hello Bob,\r\n\
            this is the plain body.\r\n";
        let text = eml_to_text(raw);
        assert!(text.starts_with("Subject: 你好世界\n"), "实际: {text}");
        assert!(
            text.contains("From: Alice <alice@example.com>"),
            "实际: {text}"
        );
        assert!(
            text.contains("Date: Tue, 13 Oct 2026 10:00:00 +0800"),
            "实际: {text}"
        );
        // 头部之后空一行接正文（不假设行尾是 \n 还是 \r\n）
        assert!(text.contains("\n\nHello Bob,"), "实际: {text}");
        assert!(text.contains("this is the plain body."), "实际: {text}");
    }

    #[test]
    fn html_only_email_uses_html_converter() {
        let raw = b"Subject: html mail\r\n\
            Content-Type: text/html; charset=UTF-8\r\n\
            \r\n\
            <html><body><p>Visible <b>text</b></p>\
            <script>track()</script></body></html>\r\n";
        let text = eml_to_text(raw);
        assert!(text.contains("Subject: html mail"), "实际: {text}");
        assert!(text.contains("Visible text"), "实际: {text}");
        assert!(!text.contains("track()"), "script 未剔除: {text}");
    }

    #[test]
    fn missing_headers_are_skipped() {
        // 无 Subject/From/Date：不产生空头行，直接进入正文分隔
        let raw = b"Content-Type: text/plain\r\n\r\nbody only\r\n";
        let text = eml_to_text(raw);
        assert!(text.starts_with('\n'), "应无头部直接空行: {text:?}");
        assert_eq!(text.trim(), "body only");
    }

    #[test]
    fn multipart_alternative_uses_html_branch() {
        // plain + html 等价内容：html 优先，script 必须剔除
        let raw = b"Subject: mixed\r\nContent-Type: multipart/alternative; boundary=BB\r\n\r\n\
            --BB\r\nContent-Type: text/plain\r\n\r\nplain part\r\n\
            --BB\r\nContent-Type: text/html\r\n\r\n<p>html part</p><script>x()</script>\r\n\
            --BB--\r\n";
        let text = eml_to_text(raw);
        assert!(text.contains("html part"), "实际: {text}");
        assert!(!text.contains("x()"), "script 未剔除: {text}");
    }

    #[test]
    fn unparsable_falls_back_to_raw_text() {
        // 完全不是邮件格式：退回原文解码而非返回空
        let raw = "这不是邮件，只是纯文本内容。".as_bytes();
        let text = eml_to_text(raw);
        assert_eq!(text, "这不是邮件，只是纯文本内容。");
    }
}
