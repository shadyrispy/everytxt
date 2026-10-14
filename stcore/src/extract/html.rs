//! HTML → 纯文本：剔除 script/style/noscript，块级元素之间补换行。
//!
//! 实现说明：scraper 0.24 未暴露节点删除与父节点访问，但 `Html::tree` 是公有字段，
//! 因此直接按文档序遍历树（`tree.nodes()` 为插入序，即文档序）：
//! - 文本节点若位于 script/style/noscript 子树内则跳过（ancestors 判定）
//! - 经过块级元素时记一个待换行标记，下一段非空文本前落一个 \n
//! - 行内相邻文本之间补一个空格，避免 "<b>world</b>" 与前段粘连

use scraper::Html;

/// 不计入正文的标签：脚本、样式，以及脚本关闭时的替代内容。
const SKIP_TAGS: [&str; 3] = ["script", "style", "noscript"];

/// 块级元素：边界处换行，保证段落/列表/表格单元可被分词器区分。
fn is_block_tag(name: &str) -> bool {
    matches!(
        name,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "br"
            | "caption"
            | "center"
            | "col"
            | "colgroup"
            | "dd"
            | "details"
            | "dialog"
            | "dir"
            | "div"
            | "dl"
            | "dt"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hgroup"
            | "hr"
            | "li"
            | "main"
            | "menu"
            | "nav"
            | "ol"
            | "option"
            | "p"
            | "pre"
            | "section"
            | "summary"
            | "table"
            | "tbody"
            | "td"
            | "tfoot"
            | "th"
            | "thead"
            | "tr"
            | "ul"
    )
}

/// 整页 HTML（.html/.htm/.xhtml 文件）转纯文本。
pub fn html_to_text(html: &str) -> String {
    walk_text(Html::parse_document(html))
}

/// HTML 片段（如邮件正文里的 HTML）转纯文本。
///
/// 与整页的区别只在解析模式：fragment 不会包出 html/head/body 外壳，
/// 避免邮件片段被套上文档级结构。
pub fn html_fragment_to_text(html: &str) -> String {
    walk_text(Html::parse_fragment(html))
}

fn walk_text(doc: Html) -> String {
    let mut out = String::new();
    let mut pending_break = false;

    for node in doc.tree.nodes() {
        let value = node.value();
        if value.is_text() {
            // 脚本/样式子树内的文本不算正文
            let in_skipped = node.ancestors().any(|ancestor| {
                ancestor
                    .value()
                    .as_element()
                    .is_some_and(|el| SKIP_TAGS.contains(&el.name()))
            });
            if in_skipped {
                continue;
            }
            // is_text 已保证 as_text 为 Some
            if let Some(text) = value.as_text() {
                push_text(&mut out, &mut pending_break, text);
            }
        } else if let Some(el) = value.as_element() {
            if is_block_tag(el.name()) {
                pending_break = true;
            }
        }
    }

    out
}

/// 追加一段文本：折叠连续空白；跨块级边界补 \n，行内相邻补空格。
fn push_text(out: &mut String, pending_break: &mut bool, raw: &str) {
    // HTML 连续空白渲染为单个空格；<pre> 的缩进会被折叠，对搜索索引影响可忽略
    let piece = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if piece.is_empty() {
        return;
    }
    if *pending_break {
        if !out.is_empty() {
            out.push('\n');
        }
        *pending_break = false;
    } else if !out.is_empty() {
        // 仅当边界两侧都是词字符（字母/数字/中文）时才补空格：
        // "<b>world</b>!" 应得 "world!" 而非 "world !"（标点贴前词）
        if needs_space(
            out.chars().next_back().unwrap(),
            piece.chars().next().unwrap(),
        ) {
            out.push(' ');
        }
    }
    out.push_str(&piece);
}

/// 行内相邻文本边界是否需要空格（两侧皆词字符才需要）。
fn needs_space(prev: char, next: char) -> bool {
    prev.is_alphanumeric() && next.is_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_script_style_noscript() {
        let html = r#"<html><head><style>body{color:red}</style>
        <script>alert("xss")</script></head>
        <body><p>Hello World</p>
        <noscript>请开启脚本</noscript></body></html>"#;
        let text = html_to_text(html);
        assert!(text.contains("Hello World"), "正文丢失: {text}");
        assert!(!text.contains("color:red"), "style 未剔除: {text}");
        assert!(!text.contains("alert"), "script 未剔除: {text}");
        assert!(!text.contains("请开启脚本"), "noscript 未剔除: {text}");
    }

    #[test]
    fn block_elements_get_newlines() {
        let html =
            "<div><h1>Title</h1><p>Para one</p><ul><li>Item A</li><li>Item B</li></ul></div>";
        let text = html_to_text(html);
        assert_eq!(text, "Title\nPara one\nItem A\nItem B");
    }

    #[test]
    fn inline_elements_stay_on_one_line() {
        // 行内元素不应引入换行，相邻文本补空格而非粘连
        let text = html_to_text("<p>Hello <b>world</b>!</p>");
        assert_eq!(text, "Hello world!");
    }

    #[test]
    fn br_forces_newline() {
        assert_eq!(html_to_text("<p>line1<br>line2</p>"), "line1\nline2");
    }

    #[test]
    fn entities_and_nbsp() {
        // html5ever 会解码实体；&nbsp; 为 U+00A0，按空白折叠
        assert_eq!(html_to_text("<p>a&nbsp;&amp;&nbsp;b</p>"), "a & b");
    }

    #[test]
    fn fragment_mode_skips_document_wrapper() {
        // 片段模式：多行内联文本保持同一行，块级换行仍在
        let text = html_fragment_to_text("<p>Hi <b>there</b></p><p>Second</p>");
        assert_eq!(text, "Hi there\nSecond");
    }

    #[test]
    fn empty_html_yields_empty_text() {
        assert_eq!(html_to_text(""), "");
        assert_eq!(html_to_text("<html><body></body></html>"), "");
    }
}
