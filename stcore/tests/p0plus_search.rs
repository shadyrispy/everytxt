//! P0+ 集成验收测试：搜索过滤（指定目录 / 指定类型）与排序三档 + title^2 boost
//!
//! 对应任务卡：P0+-1.4（指定目录/类型）、P0+-1.5（title boost 与排序）、P0+-2.5（排序切换）

use stcore::index::{IndexDoc, IndexManager, Writer};
use stcore::search::{search, SearchFilter, SearchHit, SearchMode, SortKey};

struct Fixture {
    _dir: tempfile::TempDir,
    mgr: IndexManager,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut w = Writer::open(dir.path()).unwrap();
        let docs: &[(&str, &str, &str, i64)] = &[
            // (path, title, body, mtime)
            ("/data/docs/a.txt", "a.txt", "中华人民共和国 崛起", 100),
            ("/data/other/b.md", "b.md", "中华人民共和国 崛起", 300),
            ("/data/docs/c.txt", "c.txt", "中华人民共和国 崛起", 200),
            // 仅标题命中（正文无该词）——验证 title^2 boost 的可召回性与排序
            (
                "/data/docs/中华人民共和国纪要.txt",
                "中华人民共和国纪要.txt",
                "无关正文内容",
                400,
            ),
            // 压缩包内层条目
            (
                "/data/docs/pkg.zip#inner.md",
                "inner.md",
                "中华人民共和国 崛起",
                500,
            ),
        ];
        for (path, title, body, mtime) in docs {
            w.upsert(&IndexDoc {
                path: path.to_string(),
                title: title.to_string(),
                body: body.to_string(),
                size: body.len() as u64,
                mtime: *mtime,
            })
            .unwrap();
        }
        w.commit().unwrap();
    }
    let mgr = IndexManager::open(dir.path()).unwrap();
    Fixture { _dir: dir, mgr }
}

fn names(hits: &[SearchHit]) -> Vec<String> {
    hits.iter()
        .map(|h| h.path.rsplit('/').next().unwrap_or(&h.path).to_string())
        .collect()
}

fn run(f: &Fixture, filter: SearchFilter) -> Vec<SearchHit> {
    search(
        &f.mgr,
        "中华人民共和国",
        SearchMode::Fuzzy,
        false,
        50,
        &filter,
    )
    .unwrap()
}

#[test]
fn dir_filter_uses_base_path_for_archive_entries() {
    let f = fixture();
    let hits = run(
        &f,
        SearchFilter {
            dir: Some("/data/docs".into()),
            ..Default::default()
        },
    );
    let got = names(&hits);
    // 目录前缀命中：a.txt / c.txt / 中华人民共和国纪要.txt / pkg.zip#inner.md
    assert!(got.contains(&"a.txt".to_string()));
    assert!(got.contains(&"c.txt".to_string()));
    assert!(
        got.contains(&"pkg.zip#inner.md".to_string()),
        "压缩包内层应随包路径被纳入，实际: {got:?}"
    );
    assert!(!got.contains(&"b.md".to_string()), "/data/other 应被排除");
}

#[test]
fn exts_filter_matches_extension_whitelist() {
    let f = fixture();
    let hits = run(
        &f,
        SearchFilter {
            exts: Some(vec!["md".into()]),
            ..Default::default()
        },
    );
    let got = names(&hits);
    assert_eq!(got, vec!["b.md".to_string()]);
}

#[test]
fn exts_filter_ignores_archive_inner_extension() {
    let f = fixture();
    // 内层条目 `pkg.zip#inner.md` 按包扩展名 zip 判定，故 exts=[txt] 不应命中它
    let hits = run(
        &f,
        SearchFilter {
            exts: Some(vec!["txt".into()]),
            dir: Some("/data/docs".into()),
            ..Default::default()
        },
    );
    let got = names(&hits);
    assert!(got.contains(&"a.txt".to_string()));
    assert!(got.contains(&"c.txt".to_string()));
    assert!(!got.contains(&"pkg.zip#inner.md".to_string()));
}

#[test]
fn sort_by_mtime_desc() {
    let f = fixture();
    let hits = run(
        &f,
        SearchFilter {
            sort: SortKey::Mtime,
            ..Default::default()
        },
    );
    let mtimes: Vec<i64> = hits.iter().map(|h| h.mtime).collect();
    let mut sorted = mtimes.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(mtimes, sorted, "Mtime 应降序");
    assert_eq!(mtimes.first().copied(), Some(500));
}

#[test]
fn sort_by_name_asc() {
    let f = fixture();
    let hits = run(
        &f,
        SearchFilter {
            sort: SortKey::Name,
            ..Default::default()
        },
    );
    let got = names(&hits);
    let mut sorted = got.clone();
    sorted.sort();
    assert_eq!(got, sorted, "Name 应升序");
}

#[test]
fn combined_filter_and_sort() {
    let f = fixture();
    let hits = run(
        &f,
        SearchFilter {
            dir: Some("/data/docs".into()),
            exts: Some(vec!["txt".into()]),
            sort: SortKey::Mtime,
        },
    );
    assert_eq!(
        names(&hits),
        vec![
            "中华人民共和国纪要.txt".to_string(),
            "c.txt".to_string(),
            "a.txt".to_string()
        ]
    );
}

#[test]
fn title_only_match_is_recalled_and_boosted() {
    let f = fixture();
    // 正文不含该词的文档（仅有标题命中）必须能召回（任务卡 1.5）
    let hits = run(&f, SearchFilter::default());
    let got = names(&hits);
    assert!(
        got.contains(&"中华人民共和国纪要.txt".to_string()),
        "仅标题命中的文档应被召回，实际: {got:?}"
    );
    // 且标题命中（boost 2.0）应排在纯正文命中之前
    assert_eq!(hits[0].path, "/data/docs/中华人民共和国纪要.txt");
}

#[test]
fn limit_is_respected_after_filter_and_sort() {
    let f = fixture();
    let hits = search(
        &f.mgr,
        "中华人民共和国",
        SearchMode::Fuzzy,
        false,
        2,
        &SearchFilter {
            sort: SortKey::Mtime,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].mtime, 500);
    assert_eq!(hits[1].mtime, 400);
}

#[test]
fn reverse_respects_dir_filter() {
    let f = fixture();
    let hits = search(
        &f.mgr,
        "中华人民共和国",
        SearchMode::Fuzzy,
        true,
        50,
        &SearchFilter {
            dir: Some("/data/docs".into()),
            ..Default::default()
        },
    )
    .unwrap();
    // 反向：/data/docs 下不含该词的文档 => /data/other/b.md 被目录过滤掉，只剩无匹配者
    for h in &hits {
        let base = h.path.split('#').next().unwrap();
        assert!(base.starts_with("/data/docs"), "反向结果也应受目录过滤约束");
    }
}
