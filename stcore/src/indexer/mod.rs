//! 扫描-提取-索引管线（架构 v2 R2：引擎入 stcore，宿主只留接线）
//!
//! 原 `app/src/scanner.rs` 迁入（D3：引擎与宿主分离，GUI/CLI 共用同一引擎）：
//!
//! 子项 3 接线（R10 单一事实源）：
//!   - 目录三态规则经 `RuleSet::allows` 接入 `filter_entry`，与查询层共用同一判定函数；
//!   - 受支持扩展名由 `Config::all_exts()`（config.rs 文件类型表）派生，不再硬编码；
//!   - 取消 / 暂停经 `ScanControl`（进程级原子标志）接入，取消后**正常 commit 已写入的部分**，
//!     保证索引不损坏、可续。
//!
//! 线程模型（R1）：
//!   - 提取走**专用 rayon 池**（N-1 限流），不再用全局默认池，避免与 UI 抢满核；
//!   - 写入走 `index::Writer`（写侧独占 IndexWriter），与读侧 IndexManager 分离。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use ignore::WalkBuilder;
use rayon::prelude::*;

use crate::config::{Config, RuleSet};
use crate::extract;
use crate::index::{IndexDoc, Writer};

/// 支持的压缩包扩展名（递归展开内层文本）。
///
/// 与 config 内置"压缩包"类型重叠，此处显式并集保留压缩链路语义：
/// 即使类型表被调整，压缩包内层展开行为不变。
pub const ARCHIVE_EXTS: &[&str] = &["zip", "tar", "gz", "tgz"];

/// 路径扩展名（小写）是否在预计算的受支持集合中
fn ext_allowed(path: &Path, exts: &[String]) -> bool {
    let Some(ext) = path.extension().map(|e| e.to_string_lossy().to_lowercase()) else {
        return false;
    };
    exts.iter().any(|s| s == &ext)
}

/// 从配置派生受支持扩展名（R10 单一事实源：`config.rs` 文件类型表 + 内置压缩包）。
///
/// 归一化：去点前缀、转小写、去重、排序（`Config::all_exts` 已排序去重，这里再兜底）。
pub fn supported_exts(cfg: &Config) -> Vec<String> {
    let mut v: Vec<String> = cfg
        .all_exts()
        .into_iter()
        .map(|e| e.trim_start_matches('.').to_lowercase())
        .filter(|e| !e.is_empty())
        .collect();
    for a in ARCHIVE_EXTS {
        let s = (*a).to_string();
        if !v.contains(&s) {
            v.push(s);
        }
    }
    v.sort();
    v.dedup();
    v
}

/// 路径是否为受支持类型（判定口径与原硬编码表等价：小写扩展名 ∈ 派生集合）
pub fn is_supported(path: &Path, cfg: &Config) -> bool {
    let Some(ext) = path.extension().map(|e| e.to_string_lossy().to_lowercase()) else {
        return false;
    };
    cfg.all_exts().iter().any(|e| e == &ext) || ARCHIVE_EXTS.contains(&ext.as_str())
}

/// 单文件大小上限（P0：50MB；与提取层共用常量，单一真源）
pub use crate::extract::MAX_FILE_SIZE;

/// 默认排除目录（与 ripgrep 惯例一致）
const DEFAULT_EXCLUDES: &[&str] = &[".git", "node_modules", "target", ".cache"];

/// 暂停时轮询粒度
const PAUSE_POLL: Duration = Duration::from_millis(50);

/* ------------------------------ 提取线程池（N-1 限流） ------------------------------ */

/// 专用提取池：`available_parallelism() - 1`（至少 1），给 UI 线程留一核。
///
/// 不用 rayon 全局默认池：GUI 进程里搜索/预览等其它 rayon 用户会与本管线抢满核。
static EXTRACT_POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();

fn extract_pool() -> &'static rayon::ThreadPool {
    EXTRACT_POOL.get_or_init(|| {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        rayon::ThreadPoolBuilder::new()
            .num_threads(cores.saturating_sub(1).max(1))
            .thread_name(|i| format!("st-extract-{i}"))
            .build()
            .expect("构建提取线程池失败")
    })
}

/// 扫描进度（供 `index-progress` 事件与 CLI 复用）
#[derive(Debug, Clone)]
pub struct ScanProgress {
    pub done: usize,
    pub total: usize,
    /// 当前正在处理的文件路径（重建 UI 显示用）
    pub current: String,
}

/// 取消 / 暂停控制（进程级共享；由 index_admin 的 rebuild 使用）
#[derive(Debug, Default)]
pub struct ScanControl {
    cancel: AtomicBool,
    paused: AtomicBool,
}

impl ScanControl {
    pub fn new() -> Self {
        Self::default()
    }

    /// 请求取消：不再启动新文件，已提取部分正常入库
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// 清除取消标志（开始新一轮重建前调用）
    pub fn reset(&self) {
        self.cancel.store(false, Ordering::SeqCst);
        self.paused.store(false, Ordering::SeqCst);
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// 阻塞直至恢复或被取消；返回 false 表示期间收到取消
    fn wait_if_paused(&self) -> bool {
        while self.is_paused() {
            if self.is_cancelled() {
                return false;
            }
            std::thread::sleep(PAUSE_POLL);
        }
        !self.is_cancelled()
    }
}

pub struct ScanStats {
    pub scanned: usize,
    pub indexed: usize,
    pub skipped: usize,
    pub failed: usize,
    /// 是否因取消而提前结束（已写入部分已 commit）
    pub cancelled: bool,
}

/// 全量索引一个目录（并行提取 → 串行写索引）——旧签名薄封装，保留 CLI（main.rs）兼容。
///
/// 扩展名集合从 `index_dir` 的 config.json 派生（文件不存在则用内置默认）。
pub fn index_directory(
    root: &Path,
    index_dir: &Path,
    on_progress: impl Fn(usize, usize) + Sync,
) -> anyhow::Result<ScanStats> {
    let cfg = Config::load(index_dir).unwrap_or_default();
    index_directory_controlled(
        root,
        index_dir,
        &RuleSet::empty(),
        &cfg,
        None,
        |p: ScanProgress| on_progress(p.done, p.total),
    )
}

/// 带目录三态规则的全量索引（GUI「索引目录」入口；任务卡 3.2）
///
/// 扩展名集合同样从 `index_dir` 的 config.json 派生（单一事实源）。
pub fn index_directory_with_rules(
    root: &Path,
    index_dir: &Path,
    rules: &RuleSet,
    on_progress: impl Fn(usize, usize) + Sync,
) -> anyhow::Result<ScanStats> {
    let cfg = Config::load(index_dir).unwrap_or_default();
    index_directory_controlled(root, index_dir, rules, &cfg, None, |p: ScanProgress| {
        on_progress(p.done, p.total)
    })
}

/// 全功能入口：规则过滤 + 取消/暂停 + 带当前文件名的进度回调。
///
/// `cfg` 提供受支持扩展名（R10：与查询层共用 config 文件类型表）。
///
/// 取消语义：命中取消后 `filter_map` 跳过剩余文件，随后照常 `commit` 已提取的文档，
/// 返回 `cancelled = true`。写入端幂等（按 path 先删后建），故可续跑。
pub fn index_directory_controlled(
    root: &Path,
    index_dir: &Path,
    rules: &RuleSet,
    cfg: &Config,
    ctrl: Option<&ScanControl>,
    on_progress: impl Fn(ScanProgress) + Sync,
) -> anyhow::Result<ScanStats> {
    // 写侧独占 writer（tantivy 目录锁保证单实例；读侧 IndexManager 不受影响）
    let mut writer = Writer::open(index_dir)?;

    // 受支持扩展名预计算一次（避免遍历中逐文件派生）
    let exts = supported_exts(cfg);

    // 1. 收集候选文件（并行遍历）；filter_entry 同时应用默认排除 + 三态规则
    //    注：filter_entry 要求 'static，规则集 clone 进闭包（GlobMatcher 内部为 Arc，克隆廉价）
    let rules_owned = rules.clone();
    let files: Vec<PathBuf> = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(move |e| {
            !DEFAULT_EXCLUDES
                .iter()
                .any(|x| e.file_name().to_string_lossy() == *x)
                && rules_owned.allows(e.path())
        })
        .build()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter(|e| {
            e.metadata()
                .map(|m| m.len() <= MAX_FILE_SIZE && ext_allowed(e.path(), &exts))
                .unwrap_or(false)
        })
        .map(|e| e.into_path())
        .collect();

    let total = files.len();
    let done = AtomicUsize::new(0);

    // 2. 并行提取（CPU 密集，专用池 N-1 限流）；取消/暂停在每文件入口检查
    let docs: Vec<IndexDoc> = extract_pool().install(|| {
        files
            .par_iter()
            .filter_map(|path| {
                if let Some(c) = ctrl {
                    if c.is_cancelled() || !c.wait_if_paused() {
                        return None;
                    }
                }
                let meta = std::fs::metadata(path).ok()?;
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let size = meta.len();

                let result = match extract::extract_path(path) {
                    Ok(r) => {
                        let mut docs = Vec::new();
                        if !r.text.is_empty() {
                            docs.push(IndexDoc {
                                path: path.to_string_lossy().to_string(),
                                title: path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_default(),
                                body: r.text,
                                size,
                                mtime,
                            });
                        }
                        // 压缩包内层条目：每条作为独立虚拟文档（path#entry）
                        for child in r.children {
                            if !child.text.is_empty() {
                                // 先取长度再 move，避免 borrow-after-move
                                let body_len = child.text.len() as u64;
                                docs.push(IndexDoc {
                                    path: format!("{}#{}", path.to_string_lossy(), child.name),
                                    title: child.name,
                                    body: child.text,
                                    size: body_len,
                                    mtime,
                                });
                            }
                        }
                        Some(docs)
                    }
                    Err(crate::extract::ExtractError::Unsupported(_)) => None,
                    Err(e) => {
                        // 可观测性：提取/OCR 失败不再静默丢弃（曾经 Err(_) => None
                        // 致 OCR 竞态丢文档无任何线索）。Unsupported 是正常跳过不打日志。
                        eprintln!("提取失败，跳过 {}: {e}", path.display());
                        None
                    }
                };
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                on_progress(ScanProgress {
                    done: n,
                    total,
                    current: path.to_string_lossy().to_string(),
                });
                result
            })
            .flatten()
            .collect()
    });

    // 3. 串行写索引（写侧独占 writer）；取消也照常提交已写部分
    let mut indexed = 0usize;
    for doc in &docs {
        writer.upsert(doc)?;
        indexed += 1;
    }
    writer.commit()?;

    let cancelled = ctrl.map(|c| c.is_cancelled()).unwrap_or(false);
    Ok(ScanStats {
        scanned: total,
        indexed,
        skipped: total.saturating_sub(indexed),
        failed: 0,
        cancelled,
    })
}

/// 测试用临时目录（避免为单测给 stcore 引入 tempfile 主依赖）
#[cfg(test)]
pub(crate) mod testutil {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "st-{}-{}-{}",
                tag,
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 写入文件（自动建父目录）
    pub fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, body).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::{write, TempDir};
    use super::*;
    use crate::config::{Config, RuleState};

    #[test]
    fn rules_prune_excluded_subtree() {
        let src = TempDir::new("scan-src");
        let idx = TempDir::new("scan-idx");
        write(src.path(), "keep/a.txt", "hello 世界");
        write(src.path(), "dist/b.txt", "should be excluded 世界");

        let mut cfg = Config::default();
        cfg.set_rule(None, "**/dist/**", RuleState::Exclude);
        let rules = cfg.compile_rules(None).unwrap();

        let stats = index_directory_with_rules(src.path(), idx.path(), &rules, |_, _| {}).unwrap();
        assert_eq!(stats.indexed, 1, "dist/ 应被三态规则排除");
    }

    #[test]
    fn supported_exts_derived_from_config() {
        let cfg = Config::default();
        let exts = supported_exts(&cfg);
        // 内置类型表提供文本类扩展名
        assert!(exts.iter().any(|e| e == "txt"), "config 类型表应提供 txt");
        assert!(exts.iter().any(|e| e == "md"));
        // 压缩包扩展名显式并集保留
        assert!(exts.iter().any(|e| e == "zip"), "压缩包链路保留");
        // 排序去重（all_exts 语义 + 本地兜底）
        let mut sorted = exts.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(exts, sorted);

        // 用户新增类型即时进入受支持集合（单一事实源，无需改代码）
        let mut cfg2 = Config::default();
        cfg2.add_type("自定义", vec![".FOO".into(), "bar".into()]);
        let exts2 = supported_exts(&cfg2);
        assert!(exts2.iter().any(|e| e == "foo"), "归一化去点小写");
        assert!(exts2.iter().any(|e| e == "bar"));
    }

    #[test]
    fn is_supported_uses_config_table() {
        let cfg = Config::default();
        assert!(is_supported(Path::new("/a/b.txt"), &cfg));
        assert!(is_supported(Path::new("/a/b.TXT"), &cfg), "大小写不敏感");
        assert!(is_supported(Path::new("/a/包.zip"), &cfg), "压缩包仍支持");
        assert!(
            !is_supported(Path::new("/a/b.notsupportedext"), &cfg),
            "未配置扩展名不支持"
        );
        assert!(!is_supported(Path::new("/a/noext"), &cfg), "无扩展名不支持");

        let mut cfg2 = Config::default();
        cfg2.add_type("自定义", vec!["foo".into()]);
        assert!(
            is_supported(Path::new("/a/x.foo"), &cfg2),
            "新增类型即时生效"
        );
        assert!(!is_supported(Path::new("/a/x.foo"), &cfg));
    }

    #[test]
    fn cancel_commits_partial_and_reports_flag() {
        let src = TempDir::new("scan-src");
        let idx = TempDir::new("scan-idx");
        for i in 0..5 {
            write(src.path(), &format!("f{i}.txt"), "取消测试 世界");
        }
        let ctrl = ScanControl::new();
        ctrl.cancel(); // 一开始就取消 → 不启动任何文件，但不应报错
        let stats = index_directory_controlled(
            src.path(),
            idx.path(),
            &RuleSet::empty(),
            &Config::default(),
            Some(&ctrl),
            |_| {},
        )
        .unwrap();
        assert!(stats.cancelled);
        assert_eq!(stats.indexed, 0);
        // 索引目录仍可正常打开（未损坏）
        let mgr = crate::index::IndexManager::open(idx.path()).unwrap();
        assert_eq!(mgr.num_docs(), 0);
    }

    #[test]
    fn paused_flag_roundtrip() {
        let ctrl = ScanControl::new();
        assert!(!ctrl.is_paused());
        ctrl.set_paused(true);
        assert!(ctrl.is_paused());
        ctrl.set_paused(false);
        ctrl.cancel();
        ctrl.reset();
        assert!(!ctrl.is_cancelled());
    }
}
