//! tantivy 索引管理（生产版，架构 v2 R2：读/写分离）
//!
//! tantivy 0.26.2 API 注记（spike S8 实测）：
//!   - Tokenizer trait 为 GAT：type TokenStream<'a>，自实现 advance/token/token_mut
//!     后用 BoxTokenStream::new 装箱
//!   - TopDocs 须 .order_by_score() 才是 Collector
//!   - IndexRecordOption 三档：Basic / WithFreqs / WithFreqsAndPositions（多词查询需最后者）
//!
//! 引擎拆分（D3）：
//!   - [`Writer`]：写侧独占 `IndexWriter`（indexer 线程 / scanner / CLI 使用），
//!     同一目录同一时刻只有一个 writer（tantivy 目录锁保证）
//!   - [`IndexManager`]：读侧只读（搜索/预览/统计），不持有 writer，
//!     reader 以 `ReloadPolicy::OnCommitWithDelay` 构建（tantivy 0.26 无 `OnCommit`
//!     变体；该策略监听 meta.json，外部 writer commit 后毫秒级自动 reload）

use std::path::{Path, PathBuf};
use tantivy::schema::{
    Field, IndexRecordOption, OwnedValue, Schema, Value as _, FAST, INDEXED, STORED, STRING, TEXT,
};
use tantivy::tokenizer::{BoxTokenStream, Token, TokenStream, Tokenizer};
use tantivy::{DocAddress, Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument};

use crate::seg;

/// tantivy Tokenizer：包装 seg::segment_index（索引侧 n-gram）
#[derive(Clone)]
pub struct SegIndexTokenizer;

struct VecTokenStream {
    tokens: Vec<Token>,
    index: usize,
    current: Token,
}

impl TokenStream for VecTokenStream {
    fn advance(&mut self) -> bool {
        if self.index < self.tokens.len() {
            self.current = Token {
                offset_from: self.tokens[self.index].offset_from,
                offset_to: self.tokens[self.index].offset_to,
                position: self.tokens[self.index].position,
                text: self.tokens[self.index].text.clone(),
                position_length: 1,
            };
            self.index += 1;
            true
        } else {
            false
        }
    }
    fn token(&self) -> &Token {
        &self.current
    }
    fn token_mut(&mut self) -> &mut Token {
        &mut self.current
    }
}

impl Tokenizer for SegIndexTokenizer {
    type TokenStream<'a> = BoxTokenStream<'a>;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> Self::TokenStream<'a> {
        let tokens: Vec<Token> = seg::segment_index(text)
            .into_iter()
            .enumerate()
            .map(|(pos, t)| Token {
                text: t,
                offset_from: 0,
                offset_to: 0,
                position: pos,
                position_length: 1,
            })
            .collect();
        BoxTokenStream::new(VecTokenStream {
            tokens,
            index: 0,
            current: Token::default(),
        })
    }
}

/// 索引文档（提取层产出 → 索引）
#[derive(Debug, Clone)]
pub struct IndexDoc {
    pub path: String,
    pub title: String,
    pub body: String,
    pub size: u64,
    pub mtime: i64,
}

/// schema 版本号：索引结构（字段 / tokenizer / 语义）变更时递增。
/// open 时校验 meta.json 记录的版本，不匹配返回带指引的错误（不静默失败）。
///
/// 变更记录：
/// - v1：path/title/body/size/mtime/ext
/// - v2：新增 `pinyin` 字段（拼音检索通道，P3-C）
/// - v3：`size`/`mtime` 由 STORED 升级为 `INDEXED|FAST|STORED`
///   （RangeQuery 时间/大小筛选与排序必需，架构 v2 D7；上线需一次全量重建）
pub const SCHEMA_VERSION: u32 = 3;

/// meta.json 中记录 schema 版本的键（tantivy 自身不识别该键，读时忽略）
const SCHEMA_VERSION_KEY: &str = "stcore_schema_version";

/// schema 各字段的 Field 句柄（读写两侧共用）
#[derive(Debug, Clone, Copy)]
struct SchemaFields {
    f_path: Field,
    f_title: Field,
    f_body: Field,
    f_pinyin: Field,
    f_size: Field,
    f_mtime: Field,
    f_ext: Field,
}

fn build_schema() -> Schema {
    let mut sb = Schema::builder();
    let indexing = || {
        TEXT.set_indexing_options(
            tantivy::schema::TextFieldIndexing::default()
                .set_tokenizer("seg")
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        )
    };
    let text_stored = indexing().set_stored();
    // 拼音字段只索引不存储：拼音串体积约为正文的数倍，存下来无检索价值
    let text_plain = indexing();
    sb.add_text_field("path", STRING | STORED);
    sb.add_text_field("title", text_stored.clone());
    sb.add_text_field("body", text_stored);
    sb.add_text_field("pinyin", text_plain);
    // v3（D7）：INDEXED（RangeQuery 过滤）+ FAST（按值排序）+ STORED（结果列展示）
    sb.add_i64_field("size", INDEXED | FAST | STORED);
    sb.add_i64_field("mtime", INDEXED | FAST | STORED);
    sb.add_text_field("ext", STRING | STORED);
    sb.build()
}

fn fields_of(index: &Index) -> anyhow::Result<SchemaFields> {
    Ok(SchemaFields {
        f_path: index.schema().get_field("path")?,
        f_title: index.schema().get_field("title")?,
        f_body: index.schema().get_field("body")?,
        f_pinyin: index.schema().get_field("pinyin")?,
        f_size: index.schema().get_field("size")?,
        f_mtime: index.schema().get_field("mtime")?,
        f_ext: index.schema().get_field("ext")?,
    })
}

/// 打开（或创建）索引目录：schema 版本校验 + tokenizer 注册（读写两侧共用）
fn open_index(dir: &Path) -> anyhow::Result<Index> {
    std::fs::create_dir_all(dir)?;
    let schema = build_schema();
    let index = if dir.join("meta.json").exists() {
        // schema 演进迁移：版本不匹配 → 带指引报错，不静默失败
        check_schema_version(dir)?;
        Index::open_in_dir(dir)?
    } else {
        let idx = Index::create_in_dir(dir, schema.clone())?;
        persist_schema_version(dir)?;
        idx
    };
    index.tokenizers().register("seg", SegIndexTokenizer);
    Ok(index)
}

/* ------------------------------- 写侧（独占 writer） ------------------------------- */

/// 索引写入端：独占一个 `IndexWriter`（indexer 线程 / scanner / CLI 使用）。
///
/// tantivy 以目录锁保证同一索引目录同时只有一个 writer；
/// 写入端与读侧 [`IndexManager`] 完全分离，互不持锁。
pub struct Writer {
    index: Index,
    fields: SchemaFields,
    /// 索引目录（commit 后回写 schema 版本号用）
    dir: PathBuf,
    writer: Option<IndexWriter>,
}

impl Writer {
    /// 打开（或创建）写侧上下文（此刻不建 writer，首次写入时懒创建）
    pub fn open(dir: &Path) -> anyhow::Result<Self> {
        let index = open_index(dir)?;
        let fields = fields_of(&index)?;
        Ok(Self {
            index,
            fields,
            dir: dir.to_path_buf(),
            writer: None,
        })
    }

    /// 获取 writer（50MB 堆，够 P0；批量导入时足够）
    fn writer(&mut self) -> anyhow::Result<&mut IndexWriter> {
        if self.writer.is_none() {
            self.writer = Some(self.index.writer(50_000_000)?);
        }
        Ok(self.writer.as_mut().unwrap())
    }

    /// 写入/更新一篇文档（按 path 先删后建，保证幂等）
    pub fn upsert(&mut self, doc: &IndexDoc) -> anyhow::Result<()> {
        let f_path = self.fields.f_path;
        let ext = Path::new(&doc.path)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        // 拼音通道：标题与正文的汉字转无声调拼音连写串（供 "renmin" → "人民"）
        let pinyin_text = {
            let mut s = crate::pinyin::text_to_pinyin_text(&doc.title);
            s.push(' ');
            s.push_str(&crate::pinyin::text_to_pinyin_text(&doc.body));
            s
        };
        // 手工构造文档（tantivy::doc! 宏在 lib 目标下路径解析有坑，直接 add_field_value）
        let mut d = TantivyDocument::default();
        d.add_field_value(f_path, &doc.path);
        d.add_field_value(self.fields.f_title, &doc.title);
        d.add_field_value(self.fields.f_body, &doc.body);
        d.add_field_value(self.fields.f_pinyin, &pinyin_text);
        d.add_field_value(self.fields.f_size, &OwnedValue::I64(doc.size as i64));
        d.add_field_value(self.fields.f_mtime, &OwnedValue::I64(doc.mtime));
        d.add_field_value(self.fields.f_ext, &ext);
        let w = self.writer()?;
        w.delete_term(tantivy::Term::from_field_text(f_path, &doc.path));
        w.add_document(d)?;
        Ok(())
    }

    /// 删除一篇文档
    pub fn delete(&mut self, path: &str) -> anyhow::Result<()> {
        let term = tantivy::Term::from_field_text(self.fields.f_path, path);
        self.writer()?.delete_term(term);
        Ok(())
    }

    pub fn commit(&mut self) -> anyhow::Result<()> {
        if let Some(w) = self.writer.as_mut() {
            w.commit()?;
        }
        // tantivy 每次 commit 都会覆写 meta.json（丢失自定义键），需回写版本号
        persist_schema_version(&self.dir)?;
        Ok(())
    }

    /// 清空索引（重建用）
    pub fn clear(&mut self) -> anyhow::Result<()> {
        self.writer()?.delete_all_documents()?;
        self.commit()
    }
}

/* ------------------------------- 读侧（只读 + 自动 reload） ------------------------------- */

/// 索引读取端（搜索/预览/统计）：不持有 writer，绝不与写侧争目录锁。
///
/// reader 以 `ReloadPolicy::OnCommitWithDelay` 构建（tantivy 0.26 无 `OnCommit`）：
/// 监听 meta.json，**外部** writer commit 后毫秒级自动 reload，搜索无需手动刷新。
pub struct IndexManager {
    pub index: Index,
    pub f_path: Field,
    pub f_title: Field,
    pub f_body: Field,
    /// 拼音字段（仅索引不存储）：中文词语拼音连写串，供拼音查询命中
    pub f_pinyin: Field,
    pub f_size: Field,
    pub f_mtime: Field,
    pub f_ext: Field,
    /// 同义词表（open 时从 `<index_dir>/synonyms.json` 或内置表加载）
    pub synonyms: crate::synonym::SynonymTable,
    /// 常驻 reader：OnCommitWithDelay 自动 reload（读侧正确性核心）
    reader: IndexReader,
}

impl IndexManager {
    /// 打开或创建索引目录（读侧）
    pub fn open(dir: &Path) -> anyhow::Result<Self> {
        let index = open_index(dir)?;
        let fields = fields_of(&index)?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;
        Ok(Self {
            index,
            f_path: fields.f_path,
            f_title: fields.f_title,
            f_body: fields.f_body,
            f_pinyin: fields.f_pinyin,
            f_size: fields.f_size,
            f_mtime: fields.f_mtime,
            f_ext: fields.f_ext,
            synonyms: crate::synonym::SynonymTable::load(dir),
            reader,
        })
    }

    /// 立即 reload reader（测试/确定性场景；正常路径 OnCommitWithDelay 自动完成）
    pub fn reload(&self) -> anyhow::Result<()> {
        Ok(self.reader.reload()?)
    }

    pub fn num_docs(&self) -> u64 {
        self.reader.searcher().num_docs()
    }

    /// 读取文档正文（预览/子串复核用）
    pub fn doc_body(&self, addr: DocAddress) -> Option<(String, String)> {
        let searcher = self.reader.searcher();
        let doc: TantivyDocument = searcher.doc(addr).ok()?;
        let body = doc
            .get_first(self.f_body)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let path = doc
            .get_first(self.f_path)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        Some((path, body))
    }

    /// 按 path（含 `archive.zip#inner.txt` 虚拟路径）精确取正文。
    ///
    /// 供上层预览压缩包内层条目： tantivy 细节不外溢到 app 层。
    pub fn doc_body_by_path(&self, path: &str) -> Option<String> {
        let searcher = self.reader.searcher();
        let term = tantivy::Term::from_field_text(self.f_path, path);
        let q = tantivy::query::TermQuery::new(term, tantivy::schema::IndexRecordOption::Basic);
        let top = searcher
            .search(
                &q,
                &tantivy::collector::TopDocs::with_limit(1).order_by_score(),
            )
            .ok()?;
        let (_, addr) = top.first()?;
        self.doc_body(*addr).map(|(_, body)| body)
    }
}

/* --------------------------- 一次性计数（迁移校验专用） --------------------------- */

/// 一次性文档计数（迁移校验用）：打开索引取数后**即弃**，不建常驻 reader/watcher。
///
/// 与 [`IndexManager::open`] 的差别仅在 reload 策略：
/// - `IndexManager` 用 `OnCommitWithDelay`，tantivy 会 spawn
///   `thread-tantivy-meta-file-watcher` 轮询线程；该线程在目录对象 drop 后仍最多
///   存活一个 poll 周期（500ms），期间 transient 持有 meta.json 打开句柄——
///   Windows 上 `remove_dir_all` 与该轮询竞态即 sharing violation（删除失败）。
/// - 本函数用 `ReloadPolicy::Manual`：零线程、零 WatchHandle，reader 与 `Index`
///   drop 时全部句柄**同步**关闭，满足 Windows 删除前置条件。
///
/// 生产读侧路径（搜索/预览）不受影响，仍走 `IndexManager` 的自动 reload。
pub fn count_docs(dir: &Path) -> anyhow::Result<u64> {
    let index = open_index(dir)?;
    let reader: IndexReader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::Manual)
        .try_into()?;
    // InnerIndexReader::new 已 eager load searcher（tantivy 0.26.2 reader/mod.rs L172-178 实证），
    // 直接取数即可，无需 reload
    let num_docs = reader.searcher().num_docs();
    // 显式 drop：句柄随 drop 同步关闭（Windows 上删除目录的前置条件）
    drop(reader);
    drop(index);
    Ok(num_docs)
}

/* --------------------------- 增量同步枚举（F-1 专用） --------------------------- */

/// 一次性枚举索引文档元数据 `(path, mtime, size)`（增量同步 `indexer::sync_root` 用）。
///
/// 与 [`count_docs`] 同款 `ReloadPolicy::Manual`：零 watcher 线程、句柄随 drop
/// 同步关闭（实时同步每事件调用一次，不为单次枚举 spawn/join watcher 线程；
/// Windows 删除前置条件同样满足）。
///
/// `root` 为 `Some` 时只保留位于该 root 之下（含 `path#entry` 压缩包内层）的
/// 文档——多 root 索引 per-root 增量互不牵连的核心原语。
pub fn list_doc_meta(dir: &Path, root: Option<&Path>) -> anyhow::Result<Vec<(String, i64, u64)>> {
    let index = open_index(dir)?;
    let reader: IndexReader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::Manual)
        .try_into()?;
    let searcher = reader.searcher();
    let fields = fields_of(&index)?;
    let addrs = searcher.search(
        &tantivy::query::AllQuery,
        &tantivy::collector::DocSetCollector,
    )?;
    let mut out = Vec::with_capacity(addrs.len());
    for addr in &addrs {
        let doc: TantivyDocument = searcher.doc(*addr)?;
        let path = doc
            .get_first(fields.f_path)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if let Some(r) = root {
            if !Path::new(&path).starts_with(r) {
                continue;
            }
        }
        let mtime = doc
            .get_first(fields.f_mtime)
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let size = doc
            .get_first(fields.f_size)
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            .max(0) as u64;
        out.push((path, mtime, size));
    }
    // 显式 drop：句柄同步关闭（同 count_docs 的 Windows 删除前置条件）
    drop(searcher);
    drop(reader);
    drop(index);
    Ok(out)
}

/* --------------------------------- 版本持久化 --------------------------------- */

/// 校验 meta.json 记录的 schema 版本；缺失 / 损坏 / 不符 → 指引重建
fn check_schema_version(dir: &Path) -> anyhow::Result<()> {
    let raw = std::fs::read_to_string(dir.join("meta.json"))?;
    let meta: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| anyhow::anyhow!("索引 meta.json 损坏: {e}"))?;
    let found = meta.get(SCHEMA_VERSION_KEY).and_then(|v| v.as_u64());
    match found {
        Some(v) if v == SCHEMA_VERSION as u64 => Ok(()),
        _ => anyhow::bail!(
            "索引由旧版本创建，请重建（schema 版本不匹配：期望 {SCHEMA_VERSION}，实际 {found:?}；删除索引目录后重新扫描即可）"
        ),
    }
}

/// 把 schema 版本号写回 meta.json（原子：临时文件 + rename）
fn persist_schema_version(dir: &Path) -> anyhow::Result<()> {
    let p = dir.join("meta.json");
    let mut meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&p)?)
        .map_err(|e| anyhow::anyhow!("索引 meta.json 损坏: {e}"))?;
    if let Some(obj) = meta.as_object_mut() {
        obj.insert(
            SCHEMA_VERSION_KEY.to_string(),
            serde_json::json!(SCHEMA_VERSION),
        );
    }
    let tmp = dir.join("meta.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&meta)?)?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(path: &str, body: &str) -> IndexDoc {
        IndexDoc {
            path: path.to_string(),
            title: path.rsplit('/').next().unwrap_or(path).to_string(),
            body: body.to_string(),
            size: body.len() as u64,
            mtime: 0,
        }
    }

    /// 写侧 seed：一批文档入库并 commit
    fn seed(dir: &Path, docs: &[IndexDoc]) {
        let mut w = Writer::open(dir).unwrap();
        for d in docs {
            w.upsert(d).unwrap();
        }
        w.commit().unwrap();
    }

    #[test]
    fn schema_v3_size_mtime_are_indexed_and_fast() {
        let schema = build_schema();
        for name in ["size", "mtime"] {
            let entry = schema.get_field_entry(schema.get_field(name).unwrap());
            assert!(entry.is_indexed(), "{name} 应可索引（RangeQuery）");
            assert!(entry.is_fast(), "{name} 应为 FAST（排序）");
            assert!(entry.is_stored(), "{name} 应 STORED（结果列）");
        }
    }

    #[test]
    fn schema_version_recorded_and_survives_commit() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), &[doc("/t/a.txt", "中华人民共和国")]);
        // commit 后版本键仍在（tantivy 覆写 meta.json 不丢失自定义键）
        let raw = std::fs::read_to_string(dir.path().join("meta.json")).unwrap();
        assert!(
            raw.contains(SCHEMA_VERSION_KEY),
            "meta.json 应记录 schema 版本: {raw}"
        );
        // 重新 open 正常（版本匹配）
        let mgr = IndexManager::open(dir.path()).unwrap();
        assert_eq!(mgr.num_docs(), 1);
    }

    #[test]
    fn legacy_index_without_version_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        // 旧版索引：meta.json 存在但无版本键
        std::fs::write(
            dir.path().join("meta.json"),
            r#"{"segments":[],"schema":[],"opstamp":0}"#,
        )
        .unwrap();
        let err = match IndexManager::open(dir.path()) {
            Ok(_) => panic!("旧版索引应被拒绝"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("旧版本"), "应提示重建，实际: {err}");
    }

    #[test]
    fn mismatched_schema_version_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), &[doc("/t/a.txt", "x")]);
        // 人为写入更高版本号 → 不匹配
        let mut meta: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("meta.json")).unwrap())
                .unwrap();
        meta.as_object_mut()
            .unwrap()
            .insert(SCHEMA_VERSION_KEY.to_string(), serde_json::json!(99));
        std::fs::write(
            dir.path().join("meta.json"),
            serde_json::to_string_pretty(&meta).unwrap(),
        )
        .unwrap();
        assert!(IndexManager::open(dir.path()).is_err());
    }

    #[test]
    fn clear_keeps_schema_version() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), &[doc("/t/a.txt", "x")]);
        let mut w = Writer::open(dir.path()).unwrap();
        w.clear().unwrap();
        drop(w);
        let mgr = IndexManager::open(dir.path()).unwrap();
        assert_eq!(mgr.num_docs(), 0);
        let raw = std::fs::read_to_string(dir.path().join("meta.json")).unwrap();
        assert!(raw.contains(SCHEMA_VERSION_KEY));
        // clear 后仍可正常重开
        drop(mgr);
        let mgr = IndexManager::open(dir.path()).unwrap();
        assert_eq!(mgr.num_docs(), 0);
    }

    /// R2 正确性核心：读侧 IndexManager 先打开，**另一个** Writer 实例 commit 后，
    /// 读侧 searcher 在 ≤2s 内自动可见（OnCommitWithDelay 监听 meta.json）。
    #[test]
    fn external_commit_becomes_visible_to_open_reader() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = IndexManager::open(dir.path()).unwrap();
        assert_eq!(mgr.num_docs(), 0);

        // 另一个（写侧）writer 实例：独立 Index 对象 + 独立 IndexWriter
        let mut w = Writer::open(dir.path()).unwrap();
        w.upsert(&doc("/t/x.txt", "量子计算 世界")).unwrap();
        w.commit().unwrap();
        drop(w);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while mgr.num_docs() == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "外部 commit 后 2s 内读侧应自动可见（OnCommitWithDelay）"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(mgr.num_docs(), 1);
        // 手动 reload 兜底也应立即可见且一致
        mgr.reload().unwrap();
        assert_eq!(mgr.num_docs(), 1);
    }

    /// `count_docs` 与 `IndexManager::num_docs` 两条计数通道互证一致
    /// （迁移校验走前者：Manual reader 无 watcher 线程，Windows 回滚删除无竞态）
    #[test]
    fn count_docs_matches_manager() {
        let dir = tempfile::tempdir().unwrap();
        seed(
            dir.path(),
            &[
                doc("/t/a.txt", "x"),
                doc("/t/b.txt", "y"),
                doc("/t/c.txt", "z"),
            ],
        );
        assert_eq!(count_docs(dir.path()).unwrap(), 3);
        let mgr = IndexManager::open(dir.path()).unwrap();
        assert_eq!(mgr.num_docs(), 3);
        // 空目录（新建索引）计数为 0
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(count_docs(empty.path()).unwrap(), 0);
    }

    #[test]
    fn writer_lock_excludes_second_writer() {
        // 单实例语义：持 writer 的目录上，第二个 writer 无法创建
        let dir = tempfile::tempdir().unwrap();
        let mut w1 = Writer::open(dir.path()).unwrap();
        w1.upsert(&doc("/t/a.txt", "x")).unwrap();
        let mut w2 = Writer::open(dir.path()).unwrap();
        assert!(
            w2.upsert(&doc("/t/b.txt", "y")).is_err(),
            "第二个 writer 应被 tantivy 目录锁拒绝"
        );
    }
}
