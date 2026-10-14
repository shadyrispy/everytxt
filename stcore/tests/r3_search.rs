//! R3 搜索层补齐集成测试（架构 v2 §8-R3）
//!
//! 覆盖：regex 模式（命中/不命中/非法正则/熔断注入）、时间区间 RangeQuery
//!（命中/边界含端点）、分页翻页正确性。

use std::time::Duration;

use stcore::index::{IndexDoc, IndexManager, Writer};
use stcore::search::{regex_search_with_limits, search_page, SearchFilter, SearchMode, SortKey};

fn doc(path: &str, body: &str, mtime: i64) -> IndexDoc {
    IndexDoc {
        path: path.to_string(),
        title: path.rsplit('/').next().unwrap_or(path).to_string(),
        body: body.to_string(),
        size: body.len() as u64,
        mtime,
    }
}

/// 种子库：
///   /t/a.txt  mtime=100  含 "error 404 code found here"
///   /t/b.txt  mtime=200  含 "error 500 internal"
///   /t/c.txt  mtime=300  含 "all systems nominal"
///   /t/d.txt  mtime=400  含 "error 502 bad gateway"
fn seed() -> (tempfile::TempDir, IndexManager) {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    for d in [
        doc("/t/a.txt", "error 404 code found here", 100),
        doc("/t/b.txt", "error 500 internal", 200),
        doc("/t/c.txt", "all systems nominal", 300),
        doc("/t/d.txt", "error 502 bad gateway", 400),
    ] {
        w.upsert(&d).unwrap();
    }
    w.commit().unwrap();
    drop(w);
    let mgr = IndexManager::open(dir.path()).unwrap();
    (dir, mgr)
}

/* ------------------------------ regex 模式 ------------------------------ */

#[test]
fn regex_hit_miss_and_snippet() {
    let (_d, mgr) = seed();
    // 命中：error \d{3} → a/b/d
    let resp = search_page(
        &mgr,
        "error \\d{3}",
        SearchMode::Regex,
        false,
        0,
        10,
        None,
        None,
        &SearchFilter::default(),
    )
    .unwrap();
    let mut names: Vec<&str> = resp.hits.iter().map(|h| h.title.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["a.txt", "b.txt", "d.txt"]);
    assert_eq!(resp.total, 3);
    assert!(!resp.partial);
    // snippet 取首个匹配上下文（±80 字符内含原文匹配段）
    let a = resp.hits.iter().find(|h| h.title == "a.txt").unwrap();
    assert!(a.snippet.contains("error 404"));
    assert!(a.hits >= 1);

    // 不命中：正则合法但无匹配
    let resp = search_page(
        &mgr,
        "xyzzy\\d+",
        SearchMode::Regex,
        false,
        0,
        10,
        None,
        None,
        &SearchFilter::default(),
    )
    .unwrap();
    assert!(resp.hits.is_empty());
    assert_eq!(resp.total, 0);
}

#[test]
fn regex_invalid_pattern_is_rejected() {
    let (_d, mgr) = seed();
    let err = search_page(
        &mgr,
        "(unclosed",
        SearchMode::Regex,
        false,
        0,
        10,
        None,
        None,
        &SearchFilter::default(),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("正则表达式无效"), "实际: {err}");
}

#[test]
fn regex_candidate_cap_fuse() {
    let (_d, mgr) = seed();
    // 注入候选上限 2：4 篇候选只扫 2 篇 → 提前返回 partial=true
    let resp = regex_search_with_limits(
        &mgr,
        ".*",
        0,
        10,
        None,
        None,
        &SearchFilter::default(),
        2,
        1_000_000,
        Duration::from_secs(60),
    )
    .unwrap();
    assert_eq!(resp.hits.len(), 2, "候选上限 2 → 至多命中 2 篇");
    assert!(resp.partial, "候选触顶应记 partial");
}

#[test]
fn regex_scan_bytes_fuse() {
    let (_d, mgr) = seed();
    // 注入扫描字节上限：第一篇（a.txt，27B）扫完即触顶
    let resp = regex_search_with_limits(
        &mgr,
        ".*",
        0,
        10,
        None,
        None,
        &SearchFilter::default(),
        1_000_000,
        30,
        Duration::from_secs(60),
    )
    .unwrap();
    assert!(resp.partial, "字节触顶应记 partial");
    assert_eq!(resp.hits.len(), 1);
}

#[test]
fn regex_time_budget_fuse() {
    let (_d, mgr) = seed();
    // 注入 0 时长预算：第一篇前即超时
    let resp = regex_search_with_limits(
        &mgr,
        ".*",
        0,
        10,
        None,
        None,
        &SearchFilter::default(),
        1_000_000,
        1_000_000,
        Duration::ZERO,
    )
    .unwrap();
    assert!(resp.partial, "时长触顶应记 partial");
    assert!(resp.hits.is_empty());
}

/* ------------------------------ 时间区间 ------------------------------ */

#[test]
fn mtime_range_hit_and_boundaries() {
    let (_d, mgr) = seed();

    // 全区间 [200, 400] 闭区间含端点 → b/c/d
    let resp = search_page(
        &mgr,
        "",
        SearchMode::Fuzzy,
        false,
        0,
        10,
        Some(200),
        Some(400),
        &SearchFilter::default(),
    )
    .unwrap();
    let mut names: Vec<&str> = resp.hits.iter().map(|h| h.title.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["b.txt", "c.txt", "d.txt"]);
    assert_eq!(resp.total, 3);

    // 端点边界：[100, 100] 仅 a；[101, 199] 空
    let resp = search_page(
        &mgr,
        "",
        SearchMode::Fuzzy,
        false,
        0,
        10,
        Some(100),
        Some(100),
        &SearchFilter::default(),
    )
    .unwrap();
    assert_eq!(resp.hits.len(), 1);
    assert_eq!(resp.hits[0].title, "a.txt");

    let resp = search_page(
        &mgr,
        "",
        SearchMode::Fuzzy,
        false,
        0,
        10,
        Some(101),
        Some(199),
        &SearchFilter::default(),
    )
    .unwrap();
    assert_eq!(resp.total, 0);

    // 单边开区间：from=250 → c/d；to=250 → a/b
    let resp = search_page(
        &mgr,
        "",
        SearchMode::Fuzzy,
        false,
        0,
        10,
        Some(250),
        None,
        &SearchFilter::default(),
    )
    .unwrap();
    assert_eq!(resp.total, 2);
    let resp = search_page(
        &mgr,
        "",
        SearchMode::Fuzzy,
        false,
        0,
        10,
        None,
        Some(250),
        &SearchFilter::default(),
    )
    .unwrap();
    assert_eq!(resp.total, 2);

    // 时间区间 × 反向组合：不含 "error" 且 mtime∈[300,400] → c
    let resp = search_page(
        &mgr,
        "error",
        SearchMode::Fuzzy,
        true,
        0,
        10,
        Some(300),
        Some(400),
        &SearchFilter::default(),
    )
    .unwrap();
    assert_eq!(resp.hits.len(), 1);
    assert_eq!(resp.hits[0].title, "c.txt");
}

/* -------------------------------- 分页 -------------------------------- */

#[test]
fn pagination_pages_and_total() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    for i in 0..25 {
        w.upsert(&doc(
            &format!("/p/{i:02}.txt"),
            &format!("内容 {i:02} 号文件"),
            1000 + i,
        ))
        .unwrap();
    }
    w.commit().unwrap();
    drop(w);
    let mgr = IndexManager::open(dir.path()).unwrap();

    // 按名称排序保证稳定次序；每页 10 条 → 10/10/5，total=25
    let page = |p: u32| {
        search_page(
            &mgr,
            "",
            SearchMode::Fuzzy,
            false,
            p,
            10,
            None,
            None,
            &SearchFilter {
                sort: SortKey::Name,
                ..SearchFilter::default()
            },
        )
        .unwrap()
    };
    let r0 = page(0);
    assert_eq!(
        (r0.hits.len(), r0.total, r0.page, r0.page_size),
        (10, 25, 0, 10)
    );
    assert_eq!(r0.hits[0].title, "00.txt");
    assert_eq!(r0.hits[9].title, "09.txt");

    let r1 = page(1);
    assert_eq!(r1.hits.len(), 10);
    assert_eq!(r1.hits[0].title, "10.txt");

    let r2 = page(2);
    assert_eq!(r2.hits.len(), 5);
    assert_eq!(r2.hits[0].title, "20.txt");

    // 越界页：空
    let r3 = page(3);
    assert!(r3.hits.is_empty());
    assert_eq!(r3.total, 25);

    // 翻页拼接 = 全量（无重复无遗漏）
    let mut all: Vec<String> = Vec::new();
    for p in 0..3 {
        all.extend(page(p).hits.into_iter().map(|h| h.title));
    }
    let mut sorted = all.clone();
    sorted.sort();
    let expect: Vec<String> = (0..25).map(|i| format!("{i:02}.txt")).collect();
    assert_eq!(sorted, expect);
}

/* ------------------------- regex × 分页 × 排序 ------------------------- */

#[test]
fn regex_mode_pagination_and_sort() {
    let (_d, mgr) = seed();
    let f = SearchFilter {
        sort: SortKey::Mtime,
        ..SearchFilter::default()
    };
    let resp = search_page(
        &mgr,
        "error|nominal",
        SearchMode::Regex,
        false,
        1,
        2,
        None,
        None,
        &f,
    )
    .unwrap();
    // total=4（a/b/c/d 全部命中 alternation），mtime 倒序 → d,c,b,a；第 1 页（0 起）取 b,a
    assert_eq!(resp.total, 4);
    let names: Vec<&str> = resp.hits.iter().map(|h| h.title.as_str()).collect();
    assert_eq!(names, ["b.txt", "a.txt"]);
}
