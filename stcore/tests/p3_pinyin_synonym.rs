//! P3-C 集成测试：拼音检索通道 + 同义词查询扩展。
//!
//! 验证真实索引 → 搜索链路（含 schema v2 的 pinyin 字段与 synonyms 加载）。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use stcore::index::{IndexDoc, IndexManager, Writer};
use stcore::search::{search, SearchFilter, SearchMode};

static SEQ: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("st-p3-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn doc(path: &str, body: &str) -> IndexDoc {
    IndexDoc {
        path: path.to_string(),
        title: path.rsplit('/').next().unwrap_or(path).to_string(),
        body: body.to_string(),
        size: body.len() as u64,
        mtime: 0,
    }
}

fn index_of(docs: &[IndexDoc]) -> (TempDir, IndexManager) {
    let dir = TempDir::new("idx");
    {
        let mut w = Writer::open(dir.path()).unwrap();
        for d in docs {
            w.upsert(d).unwrap();
        }
        w.commit().unwrap();
    }
    let mgr = IndexManager::open(dir.path()).unwrap();
    (dir, mgr)
}

fn hit_paths(mgr: &IndexManager, q: &str, mode: SearchMode) -> Vec<String> {
    let filter = SearchFilter::default();
    search(mgr, q, mode, false, 20, &filter)
        .unwrap()
        .into_iter()
        .map(|h| h.path)
        .collect()
}

#[test]
fn pinyin_query_hits_chinese_body() {
    let (_d, mgr) = index_of(&[
        doc("/a/国史.txt", "中华人民共和国成立于一九四九年"),
        doc("/b/english.txt", "hello world"),
    ]);
    // 全文拼音
    let hits = hit_paths(&mgr, "renmingongheguo", SearchMode::Fuzzy);
    assert!(hits.contains(&"/a/国史.txt".to_string()), "{hits:?}");
    // 词级拼音
    let hits = hit_paths(&mgr, "zhonghua", SearchMode::Fuzzy);
    assert!(hits.contains(&"/a/国史.txt".to_string()), "{hits:?}");
    // 不相关的拼音不应命中
    let hits = hit_paths(&mgr, "zhonghua", SearchMode::Fuzzy);
    assert!(!hits.contains(&"/b/english.txt".to_string()), "{hits:?}");
}

#[test]
fn pinyin_query_hits_title() {
    let (_d, mgr) = index_of(&[doc("/a/人民报告.txt", "正文与标题无关")]);
    let hits = hit_paths(&mgr, "renmin", SearchMode::Fuzzy);
    assert!(hits.contains(&"/a/人民报告.txt".to_string()), "{hits:?}");
}

#[test]
fn pinyin_disabled_in_exact_mode() {
    let (_d, mgr) = index_of(&[doc("/a/国史.txt", "中华人民共和国成立于一九四九年")]);
    // exact 模式不启用拼音扩展 → 拼音查询不应命中
    let hits = hit_paths(&mgr, "renmingongheguo", SearchMode::Exact);
    assert!(hits.is_empty(), "{hits:?}");
}

#[test]
fn synonym_expansion_in_fuzzy_mode() {
    let (_d, mgr) = index_of(&[doc("/a/机器.txt", "这台计算机运行良好")]);
    // "电脑" 未出现在正文，但同义词组含 "计算机" → fuzzy 应命中
    let hits = hit_paths(&mgr, "电脑", SearchMode::Fuzzy);
    assert!(hits.contains(&"/a/机器.txt".to_string()), "{hits:?}");
}

#[test]
fn synonym_not_applied_in_exact_mode() {
    let (_d, mgr) = index_of(&[doc("/a/机器.txt", "这台计算机运行良好")]);
    let hits = hit_paths(&mgr, "电脑", SearchMode::Exact);
    assert!(hits.is_empty(), "exact 不应做同义扩展: {hits:?}");
}

#[test]
fn custom_synonym_file_takes_effect() {
    let dir = TempDir::new("syn");
    std::fs::write(dir.path().join("synonyms.json"), r#"[["夸克","层子"]]"#).unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    w.upsert(&doc("/a/物理.txt", "层子模型是早期假说")).unwrap();
    w.commit().unwrap();
    drop(w);
    let mgr = IndexManager::open(dir.path()).unwrap();
    let hits = hit_paths(&mgr, "夸克", SearchMode::Fuzzy);
    assert!(hits.contains(&"/a/物理.txt".to_string()), "{hits:?}");
}

#[test]
fn schema_version_bumped_to_v3() {
    let dir = TempDir::new("ver");
    let mut w = Writer::open(dir.path()).unwrap();
    w.upsert(&doc("/a/x.txt", "内容")).unwrap();
    w.commit().unwrap();
    drop(w);
    let mgr = IndexManager::open(dir.path()).unwrap();
    drop(mgr);
    let raw = std::fs::read_to_string(dir.path().join("meta.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        v["stcore_schema_version"], 3,
        "meta.json 应记录 schema v3: {raw}"
    );
}
