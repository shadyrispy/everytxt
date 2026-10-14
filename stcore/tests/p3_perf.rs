//! P3-D 性能基准（`#[ignore]`，默认不进单测）。
//!
//! 跑法：
//! ```bash
//! cargo test --release -p stcore --test p3_perf -- --ignored --nocapture
//! ```
//!
//! 覆盖三条链路的**回归观测**（数字打印到 stdout 供人工比对；断言仅设宽松上限，
//! 防"性能崩塌式"回归，不做精确门禁以免 CI 抖动误报）：
//!   1. 预览匹配：8 MB 文本 match_groups
//!   2. 索引吞吐：2000 篇文档 upsert + commit
//!   3. 搜索延迟：5000 篇库上 100 次查询的 P50 / P99
//!
//! 参照：性能调优目标 P2（P99 < 100ms）。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use stcore::index::{IndexDoc, IndexManager, Writer};
use stcore::preview::match_groups;
use stcore::search::{search, SearchFilter, SearchMode};

static SEQ: AtomicU64 = AtomicU64::new(0);

fn tmp_dir(tag: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!("st-perf-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn sample_body(i: usize) -> String {
    let unit = "中华人民共和国 成立于 一九四九年。人民 生活 水平 不断 提高，中华 文化 博大 精 深。\
                分布式 系统 与 供应链 金融 是 当前 的 热点 话题。搜索 引擎 索引 文档 内容。";
    format!("第 {i} 篇文档：{unit}{unit}")
}

#[test]
#[ignore = "性能基准：需 --release --ignored 手动运行"]
fn bench_preview_match_8mb() {
    let unit = "中华人民共和国 中华苏维埃共和国 中云 台湾 高山族 图书馆 数据报告\n";
    let mut text = String::with_capacity(8 << 20);
    while text.len() < 8 << 20 {
        text.push_str(unit);
    }
    let terms = vec![
        "共同".to_string(),
        "中华".to_string(),
        "共和国".to_string(),
        "数据".to_string(),
    ];
    let t0 = Instant::now();
    let g = match_groups(&text, &terms);
    let el = t0.elapsed();
    let total: usize = g.iter().map(|x| x.count).sum();
    println!(
        "[preview] 8MB 文本 / {} 词项 → {} 命中组，共 {} 处，耗时 {:.1} ms",
        terms.len(),
        g.len(),
        total,
        el.as_secs_f64() * 1000.0
    );
    // 宽松上限：8MB 文本 > 2 秒说明匹配链路退化
    assert!(el.as_secs_f64() < 2.0, "8MB match_groups 过慢: {el:?}");
}

#[test]
#[ignore = "性能基准：需 --release --ignored 手动运行"]
fn bench_index_2000_docs() {
    let dir = tmp_dir("idx");
    let mut mgr = Writer::open(&dir).unwrap();
    let t0 = Instant::now();
    for i in 0..2000 {
        let body = sample_body(i);
        mgr.upsert(&IndexDoc {
            path: format!("/bench/doc{i}.txt"),
            title: format!("文档{i}"),
            body,
            size: 100,
            mtime: i as i64,
        })
        .unwrap();
    }
    let upsert_el = t0.elapsed();
    let t1 = Instant::now();
    mgr.commit().unwrap();
    let commit_el = t1.elapsed();
    println!(
        "[index] 2000 篇 upsert {:.1} ms（{:.0} 篇/秒）+ commit {:.1} ms",
        upsert_el.as_secs_f64() * 1000.0,
        2000.0 / upsert_el.as_secs_f64().max(0.001),
        commit_el.as_secs_f64() * 1000.0
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "性能基准：需 --release --ignored 手动运行"]
fn bench_search_latency_p50_p99() {
    let dir = tmp_dir("search");
    let mut mgr = Writer::open(&dir).unwrap();
    for i in 0..5000 {
        mgr.upsert(&IndexDoc {
            path: format!("/bench/doc{i}.txt"),
            title: format!("文档{i}"),
            body: sample_body(i),
            size: 100,
            mtime: i as i64,
        })
        .unwrap();
    }
    mgr.commit().unwrap();
    drop(mgr);

    let mgr = IndexManager::open(&dir).unwrap();
    let queries = ["中华人民共和国", "供应链", "分布式", "搜索引擎", "人民生活"];
    let filter = SearchFilter::default();
    let mut lat: Vec<f64> = Vec::with_capacity(100);
    for round in 0..100 {
        let q = queries[round % queries.len()];
        let t = Instant::now();
        let hits = search(&mgr, q, SearchMode::Fuzzy, false, 20, &filter).unwrap();
        lat.push(t.elapsed().as_secs_f64() * 1000.0);
        assert!(!hits.is_empty() || round % 5 == 0, "查询应有结果: {q}");
    }
    lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = lat[lat.len() / 2];
    let p99 = lat[(lat.len() as f64 * 0.99) as usize - 1];
    println!(
        "[search] 5000 篇库 / 100 次查询 → P50 {p50:.1} ms，P99 {p99:.1} ms，最大 {:.1} ms",
        lat.last().unwrap()
    );
    // 计划门禁 P99 < 100ms（宽松 3 倍容差防抖动）
    assert!(p99 < 300.0, "搜索 P99 过慢: {p99:.1} ms");
    let _ = std::fs::remove_dir_all(&dir);
}
