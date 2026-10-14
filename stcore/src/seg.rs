//! 分词内核（S3/S8 spike 定型，生产版）
//!
//! 策略（spike 双门禁验证：S8 37/37 + S3 hit@5 100%）：
//!   查询侧 = 无缝切分 DP（jieba cut_for_search 词边，丢弃 ≥4 字边，最少段数无缝覆盖；
//!              无解回退精确模式）
//!   索引侧 = 每空白块所有 1-3 字连续子串（char n-gram）+ 整块本身
//!   >3 字查询 token（未登录回退整词）→ 拆相邻 bigram AND
//!
//! 第一性原理：≤3 字 token 的"子串存在性" ⟺ "n-gram token 存在性"，
//! 故 tantivy AND 与 oracle 子串语义严格对齐；AND 语义下索引 token
//! 超集化不改变已有查询结果。
//!
//! 性能：Jieba::new() 解析 350 万词条词典以秒计，必须 OnceLock 缓存。

use std::sync::OnceLock;

fn jieba() -> &'static jieba_rs::Jieba {
    static JIEBA: OnceLock<jieba_rs::Jieba> = OnceLock::new();
    JIEBA.get_or_init(jieba_rs::Jieba::new)
}

/// jieba 精确模式分词（回退参考用）
pub fn jieba_cut_exact(text: &str) -> Vec<String> {
    jieba()
        .cut(text, false)
        .into_iter()
        .map(|t| t.word.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 查询侧分词：纯净无缝切分（spike S8 定型）
pub fn segment_query(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for chunk in text.split_whitespace() {
        out.extend(segment_chunk(chunk));
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|t| seen.insert(t.clone()));
    out
}

fn segment_chunk(chunk: &str) -> Vec<String> {
    if chunk.is_empty() {
        return Vec::new();
    }
    let n = chunk.chars().count();

    // 收集词边（char 区间），丢弃 ≥4 字长词
    let edges: Vec<(usize, usize)> = jieba()
        .cut_for_search(chunk, false)
        .into_iter()
        .filter(|t| t.word.chars().count() < 4)
        .map(|t| (t.start, t.end))
        .collect();

    if edges.is_empty() {
        return jieba_cut_exact(chunk);
    }

    // DP 最少段数无缝覆盖 [0, n)
    let mut best: Vec<Option<(usize, usize)>> = vec![None; n + 1];
    best[0] = Some((0, usize::MAX));
    for i in 0..=n {
        let Some((seg, _)) = best[i] else { continue };
        for &(s, e) in &edges {
            if s == i && e <= n {
                let cand = seg + 1;
                if best[e].is_none_or(|(bs, _)| cand < bs) {
                    best[e] = Some((cand, i));
                }
            }
        }
    }

    let Some(_) = best[n] else {
        return jieba_cut_exact(chunk);
    };
    let mut pieces: Vec<(usize, usize)> = Vec::new();
    let mut cur = n;
    while cur != 0 {
        let (_, prev) = best[cur].unwrap();
        pieces.push((prev, cur));
        cur = prev;
    }
    pieces.reverse();
    let chars: Vec<char> = chunk.chars().collect();
    pieces
        .into_iter()
        .map(|(s, e)| chars[s..e].iter().collect::<String>().to_lowercase())
        .collect()
}

/// 索引侧分词：每块所有 1-3 字连续子串 + 整块本身
pub fn segment_index(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for chunk in text.split_whitespace() {
        let chars: Vec<char> = chunk.chars().collect();
        let n = chars.len();
        if n > 0 {
            out.push(chunk.to_lowercase());
        }
        for len in 1..=3.min(n) {
            for s in 0..=n - len {
                out.push(chars[s..s + len].iter().collect::<String>().to_lowercase());
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|t| seen.insert(t.clone()));
    out
}

/// 查询 token → term 列表：≤3 字原样；>3 字拆相邻 bigram
pub fn expand_token_to_terms(token: &str) -> Vec<String> {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() <= 3 {
        return vec![token.to_string()];
    }
    (0..chars.len() - 1)
        .map(|i| chars[i..i + 2].iter().collect::<String>())
        .collect()
}

/// 去重排序的查询 token 列表（供 search 层使用）
pub fn seg_query_tokens(text: &str) -> Vec<String> {
    let mut t = segment_query(text);
    t.sort();
    t.dedup();
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_3_1_segmentation() {
        //中华人民共和国 → 中华/人民/共和国
        // seg_query_tokens 语义 = 去重排序（Unicode 码点序：中华 < 人民 < 共和国）
        assert_eq!(
            seg_query_tokens("中华人民共和国"),
            vec!["中华", "人民", "共和国"]
        );
    }

    #[test]
    fn index_side_has_single_chars() {
        // 索引侧必须有单字粒度（S3 单字查询命中依据）
        let toks = segment_index("中华人民共和国");
        assert!(toks.contains(&"共".to_string()));
        assert!(toks.contains(&"华人".to_string()));
        assert!(toks.contains(&"中华人民共和国".to_string()));
    }

    #[test]
    fn expand_long_token_to_bigrams() {
        assert_eq!(expand_token_to_terms("中华"), vec!["中华"]);
        assert_eq!(
            expand_token_to_terms("华人民共"),
            vec!["华人", "人民", "民共"]
        );
    }

    #[test]
    fn english_chunk_passthrough() {
        assert_eq!(seg_query_tokens("hello world"), vec!["hello", "world"]);
    }
}
