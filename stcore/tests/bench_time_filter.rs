//! D7 量化基准：schema v2（mtime 仅 STORED）→ v3（INDEXED|FAST|STORED）
//! 时间区间过滤的收益实测。
//!
//! - 变体 A（v3 现状）：`search_page(..., mtime_from, mtime_to, ...)`
//!   → tantivy RangeQuery 下推，仅 limit 条入堆
//! - 变体 B（v2 语义模拟）：不带时间条件全量召回（limit=N），Rust 侧
//!   `retain(|h| h.mtime in range)` 后过滤——STORED-only schema 下
//!   mtime 不可查询，只能如此
//! - 变体 C（基线）：不带时间条件的普通分页查询（limit=20），
//!   用于分离"词项查询本身成本"与"过滤成本"
//!
//! 手动运行（`#[ignore]` 不进默认门禁）：
//! ```bash
//! cargo test -p stcore --test bench_time_filter -- --ignored --nocapture
//! ```

use std::path::Path;
use std::time::Instant;

use stcore::index::{IndexDoc, IndexManager, Writer};
use stcore::search::{search, search_page, SearchFilter, SearchMode};

const N: usize = 20_000;
const ROUNDS: usize = 100;
const WARMUP: usize = 5;
/// mtime 跨度：730 天（秒）
const SPAN: i64 = 730 * 86_400;
const BASE_MTIME: i64 = 1_700_000_000;

/// xorshift64：免 rand 依赖的确定性伪随机
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn range(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn build_index(dir: &Path) -> IndexManager {
    let mut w = Writer::open(dir).expect("Writer::open");
    let mut rng = Lcg(0x5EED_1234);
    for i in 0..N {
        // 每篇都含查询词"基准"（全库命中），另加每篇唯一 token 防退化
        let body = format!("基准文档 第 {i} 号 通用词 内容填充 唯一标识 u{i} 结束");
        w.upsert(&IndexDoc {
            path: format!("/data/docs/doc-{i:05}.txt"),
            title: format!("文档 {i}"),
            body,
            size: 64,
            mtime: BASE_MTIME + rng.range(SPAN as u64) as i64,
        })
        .expect("upsert");
    }
    w.commit().expect("commit");
    IndexManager::open(dir).expect("IndexManager::open")
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    let idx = ((sorted.len() as f64 * p).floor() as usize).min(sorted.len() - 1);
    sorted[idx]
}

fn bench<F: FnMut() -> usize>(name: &str, mut f: F) -> (f64, f64, f64, usize) {
    for _ in 0..WARMUP {
        std::hint::black_box(f());
    }
    let mut times = Vec::with_capacity(ROUNDS);
    let mut last = 0;
    for _ in 0..ROUNDS {
        let t = Instant::now();
        last = std::hint::black_box(f());
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    let (p50, p99) = (pct(&times, 0.50), pct(&times, 0.99));
    println!(
        "{name:<28} P50 {p50:>7.3} ms | P99 {p99:>7.3} ms | mean {mean:>7.3} ms | 结果 {last}"
    );
    (p50, p99, mean, last)
}

#[test]
#[ignore = "基准测试：手动运行，约 1-2 分钟"]
fn time_filter_fast_vs_post_filter() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = build_index(dir.path());
    println!("\n== D7 基准：{N} 篇，mtime 跨度 {SPAN} 秒（730 天），每轮 {ROUNDS} 次 ==\n");

    for (label, frac) in [("窗口 10%（73 天）", 0.10), ("窗口 1%（7.3 天）", 0.01)] {
        let width = (SPAN as f64 * frac) as i64;
        let t0 = BASE_MTIME + SPAN / 3;
        let t1 = t0 + width;
        println!("-- {label}: [{t0}, {t1}] --");

        // C：无时间条件的普通分页（词项查询本身成本）
        let (c_p50, ..) = bench("C 无过滤 limit=20", || {
            search(
                &mgr,
                "基准",
                SearchMode::Fuzzy,
                false,
                20,
                &SearchFilter::default(),
            )
            .unwrap()
            .len()
        });

        // A：v3 RangeQuery 下推
        let (a_p50, _, _, a_total) = bench("A v3 RangeQuery limit=20", || {
            let r = search_page(
                &mgr,
                "基准",
                SearchMode::Fuzzy,
                false,
                0,
                20,
                Some(t0),
                Some(t1),
                &SearchFilter::default(),
            )
            .unwrap();
            assert_eq!(r.hits.len(), 20.min(r.total as usize));
            r.total as usize
        });

        // B：v2 语义——全量召回 + Rust 后过滤
        let (b_p50, _, _, b_count) = bench("B v2 后过滤 limit=N", || {
            let hits = search(
                &mgr,
                "基准",
                SearchMode::Fuzzy,
                false,
                N,
                &SearchFilter::default(),
            )
            .unwrap();
            hits.into_iter()
                .filter(|h| h.mtime >= t0 && h.mtime <= t1)
                .count()
        });

        // 一致性：RangeQuery 下推与后过滤两种口径命中篇数必须相等
        assert_eq!(a_total, b_count, "A/B 命中数不一致");
        println!(
            "   加速比 P50: {:.1}×（B {:.3} ms → A {:.3} ms）｜ 基线 C {:.3} ms\n",
            b_p50 / a_p50,
            b_p50,
            a_p50,
            c_p50
        );
    }
}
