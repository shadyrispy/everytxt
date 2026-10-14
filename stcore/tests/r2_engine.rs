//! R2 索引引擎统一：集成验收
//!
//! - schema v3：size/mtime 为 INDEXED|FAST|STORED（RangeQuery + 排序前置条件）
//! - 外部 writer commit → 已打开的读侧 IndexManager ≤2s 内自动可见
//!   （读侧 reader ReloadPolicy::OnCommitWithDelay 监听 meta.json）
//! - 写侧单实例：第二个 writer 被 tantivy 目录锁拒绝

use std::time::{Duration, Instant};

use stcore::index::{IndexDoc, IndexManager, Writer};

fn doc(path: &str, body: &str, mtime: i64) -> IndexDoc {
    IndexDoc {
        path: path.to_string(),
        title: path.rsplit('/').next().unwrap_or(path).to_string(),
        body: body.to_string(),
        size: body.len() as u64,
        mtime,
    }
}

#[test]
fn schema_v3_size_mtime_indexed_fast_stored() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    w.upsert(&doc("/t/a.txt", "x", 1)).unwrap();
    w.commit().unwrap();
    // 经读侧取实际落盘 schema（create_in_dir 使用 build_schema 的 v3 定义）
    let mgr = IndexManager::open(dir.path()).unwrap();
    let schema = mgr.index.schema();
    for name in ["size", "mtime"] {
        let entry = schema.get_field_entry(schema.get_field(name).unwrap());
        assert!(entry.is_indexed(), "{name} 应 INDEXED");
        assert!(entry.is_fast(), "{name} 应 FAST");
        assert!(entry.is_stored(), "{name} 应 STORED");
    }
}

#[test]
fn external_commit_visible_within_2s() {
    let dir = tempfile::tempdir().unwrap();
    // 1. 读侧先打开（常驻 reader）
    let mgr = IndexManager::open(dir.path()).unwrap();
    assert_eq!(mgr.num_docs(), 0);

    // 2. 另一个写侧 writer 实例 commit 一篇文档
    let mut w = Writer::open(dir.path()).unwrap();
    w.upsert(&doc("/t/量子.txt", "量子计算 改变 世界", 42))
        .unwrap();
    w.commit().unwrap();
    drop(w);

    // 3. 读侧 searcher ≤2s 自动可见（不手动 reload，验证 OnCommitWithDelay 生效）
    let deadline = Instant::now() + Duration::from_secs(2);
    while mgr.num_docs() == 0 {
        assert!(
            Instant::now() < deadline,
            "外部 commit 后 2s 内读侧应自动可见"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(mgr.num_docs(), 1);
}

#[test]
fn manual_reload_sees_external_commit_immediately() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = IndexManager::open(dir.path()).unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    w.upsert(&doc("/t/a.txt", "hello 世界", 0)).unwrap();
    w.commit().unwrap();
    drop(w);
    mgr.reload().unwrap();
    assert_eq!(mgr.num_docs(), 1);
}

#[test]
fn second_writer_rejected_by_directory_lock() {
    let dir = tempfile::tempdir().unwrap();
    let mut w1 = Writer::open(dir.path()).unwrap();
    w1.upsert(&doc("/t/a.txt", "x", 0)).unwrap();
    let mut w2 = Writer::open(dir.path()).unwrap();
    assert!(w2.upsert(&doc("/t/b.txt", "y", 0)).is_err());
}

#[test]
fn incremental_upsert_after_reader_open_is_visible() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    w.upsert(&doc("/t/one.txt", "第一篇 量子计算", 1)).unwrap();
    w.commit().unwrap();
    drop(w);

    let mgr = IndexManager::open(dir.path()).unwrap();
    assert_eq!(mgr.num_docs(), 1);

    // 幂等 upsert（同 path 先删后建）+ 新增 → 读侧继续自动可见
    let mut w = Writer::open(dir.path()).unwrap();
    w.upsert(&doc("/t/one.txt", "第一篇 量子计算 更新版", 1))
        .unwrap();
    w.upsert(&doc("/t/two.txt", "第二篇 供应链 金融", 2))
        .unwrap();
    w.commit().unwrap();
    drop(w);

    let deadline = Instant::now() + Duration::from_secs(2);
    while mgr.num_docs() < 2 {
        assert!(Instant::now() < deadline, "增量 commit 后读侧应自动可见");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(mgr.num_docs(), 2, "同 path upsert 应幂等（先删后建）");
}
