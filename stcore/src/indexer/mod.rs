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

use std::collections::{HashMap, HashSet};
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

/* ------------------------------ 增量同步（O2 / F-1） ------------------------------ */

/// 提取函数抽象（F-1 可测性桩注入用，R7）。
///
/// `sync_root` 内部经本 trait 调用提取层；生产恒为 [`extract::extract_path`]，
/// `#[cfg(test)]` 可注入计数桩，精确断言「跳过时零 extract」。
/// 不改变任何公开 API。
trait ExtractFn: Sync {
    fn extract(&self, path: &Path) -> Result<extract::ExtractResult, extract::ExtractError>;
}

/// 生产提取实现（唯一调用点即 `extract::extract_path`）。
struct RealExtract;

impl ExtractFn for RealExtract {
    fn extract(&self, path: &Path) -> Result<extract::ExtractResult, extract::ExtractError> {
        extract::extract_path(path)
    }
}

/// 增量同步一个 root（O2 核心性能修复；旧全量路径一行不动）。
///
/// 语义（v2 修订 R1/R8）：
/// 1. 轻量 walk（只取 path + fs mtime + size，**不 extract**；过滤口径与
///    [`index_directory_controlled`] 逐项对齐：隐藏文件/gitignore/默认排除/
///    三态规则/仅文件/大小上限/受支持扩展名）；
/// 2. 读侧枚举索引现有 `path -> (mtime, size)` 映射（限本 root 范围，
///    [`crate::index::list_doc_meta`]）；
/// 3. 三态比对：
///    - mtime 与 size 均相同 → **跳过**（省掉最贵的 extract）；
///    - 不同 → extract + upsert（upsert 按 path 先删后建幂等；压缩包内层
///      `path#entry` 旧条目先删后建，覆盖条目增减）；
///    - 索引有、walk 清单无（文件已删/被规则排除/超限/不再是文件）→ delete；
/// 4. 并行提取（专用池 N-1 限流）+ [`ScanControl`] 取消/暂停；单 writer
///    串行写、一次 commit；零变更则完全不碰 writer（不争目录锁）。
///
/// ScanStats 语义（R8）：`scanned` = walk 清单文件数；`indexed` = 实际
/// extract+upsert 的文件数；`skipped` = 未变跳过数；`failed` 预留 0
/// （提取失败与全量路径同语义：打日志跳过，索引不动）；`cancelled` = 取消
/// 提前结束（**取消不写任何变更**，见原子性说明）。
///
/// 原子性：要么全部变更落库，要么一点都不写——删除阶段与提取阶段之间
/// 不允许部分提交（否则取消/失败会把「已删旧 doc、未建新 doc」的半态落库，
/// 造成文档丢失）。提取失败的文件保留旧 doc（陈旧优于丢失）。
///
/// 已知限制（R2，如实标注）：fs mtime 以**秒**入索引（与
/// [`index_directory_controlled`] 同源），同秒内两次保存且 size 不变会漏更新
/// （连按 Ctrl+S/编辑器自动保存）；size 参与判定已覆盖大半窗口，残余由
/// 「手动重建」与「实时失败回退全量」（F-3）兜底。压缩包内层-only 文档
/// （外层无主 doc，`text` 为空的压缩包即此形态）size 存的是正文长度、
/// 与文件 size 不可比，退化为 mtime 单判——同受秒级限制约束。
pub fn sync_root(
    index_dir: &Path,
    root: &Path,
    rules: &RuleSet,
    cfg: &Config,
    ctrl: Option<&ScanControl>,
) -> anyhow::Result<ScanStats> {
    sync_root_with(index_dir, root, rules, cfg, ctrl, &RealExtract)
}

/// 内部实现：提取器可注入（测试计数桩，R7）。
fn sync_root_with(
    index_dir: &Path,
    root: &Path,
    rules: &RuleSet,
    cfg: &Config,
    ctrl: Option<&ScanControl>,
    extractor: &dyn ExtractFn,
) -> anyhow::Result<ScanStats> {
    // 取消前置检查：取消即整体不动作（不 walk、不写索引）
    if ctrl.is_some_and(|c| c.is_cancelled()) {
        return Ok(ScanStats {
            scanned: 0,
            indexed: 0,
            skipped: 0,
            failed: 0,
            cancelled: true,
        });
    }

    let exts = supported_exts(cfg);

    // 1. 轻量 walk（仅元数据，不 extract）
    let files = walk_meta_only(root, rules, &exts, ctrl);
    let total = files.len();
    if ctrl.is_some_and(|c| c.is_cancelled()) {
        // walk 期间收到取消：整体放弃。绝不敢把「取消导致的空/半截清单」
        // 当作「磁盘已空」执行删除（误删全 root 文档）。
        return Ok(ScanStats {
            scanned: 0,
            indexed: 0,
            skipped: 0,
            failed: 0,
            cancelled: true,
        });
    }

    // 2. 读侧枚举索引现有映射（限本 root；一次性 Manual reader）
    let listed = crate::index::list_doc_meta(index_dir, Some(root))?;
    let mut index_paths: Vec<String> = listed.iter().map(|(p, _, _)| p.clone()).collect();
    index_paths.sort();
    index_paths.dedup(); // path 唯一（upsert 幂等），防御性去重
    let meta_of: HashMap<&str, (i64, u64)> = listed
        .iter()
        .map(|(p, m, s)| (p.as_str(), (*m, *s)))
        .collect();

    // walk 文件路径集合：字面含 '#' 的文件名与 `path#entry` 内层命名冲突时
    // 的防误删护栏（其 doc 不得被他文件的 own-docs 删除带走）
    let walk_set: HashSet<String> = files
        .iter()
        .map(|(p, _, _)| p.to_string_lossy().to_string())
        .collect();

    // 3. 三态判定
    let mut to_extract: Vec<(PathBuf, i64, u64)> = Vec::new();
    let mut to_delete: Vec<String> = Vec::new();
    let mut skipped = 0usize;
    // 每个待提取文件的旧 doc 清单（主 doc + 内层 path#*），提取成功后先删后建
    let mut old_docs: HashMap<String, Vec<String>> = HashMap::new();
    // 被 walk 清单覆盖的索引 doc（未变的 + 将重建的）；其余即「索引有、磁盘无」
    let mut covered: HashSet<String> = walk_set.clone();

    for (path, mtime, size) in &files {
        let key = path.to_string_lossy().to_string();
        // 本文件的索引 doc 集合：主 doc `key` + 内层 `key#*`。
        // index_paths 已排序，'#'(0x23) 低于字母数字，own docs 在 `key` 后连续排布。
        let inner_prefix = format!("{key}#");
        let lo = index_paths.partition_point(|e| e.as_str() < key.as_str());
        let mut own: Vec<String> = Vec::new();
        let mut i = lo;
        while i < index_paths.len() {
            let e = &index_paths[i];
            if e == &key || e.starts_with(&inner_prefix) {
                own.push(e.clone());
                covered.insert(e.clone());
                i += 1;
            } else {
                break;
            }
        }
        // 未变判定：主 doc 在 → mtime+size 双比；主 doc 不在（内层-only 压缩包）
        // → 取任一条内层 doc 的 mtime（均等于文件 mtime）单比（size 不可比，已知限制）
        let inner_mtime = own
            .iter()
            .filter(|e| *e != &key)
            .find_map(|e| meta_of.get(e.as_str()).map(|(m, _)| *m));
        let unchanged = match meta_of.get(key.as_str()) {
            Some((im, is)) => *im == *mtime && *is == *size,
            None => inner_mtime.is_some_and(|im| im == *mtime),
        };
        if unchanged {
            skipped += 1;
        } else {
            to_extract.push((path.clone(), *mtime, *size));
            old_docs.insert(key, own);
        }
    }

    // 索引有、walk 清单无 → 删除（文件已删/被规则排除/超限/不再是文件）
    for q in &index_paths {
        if !covered.contains(q) {
            to_delete.push(q.clone());
        }
    }

    // 4a. 并行提取（取消/暂停语义同全量路径；取消则整体放弃，不写索引）
    let extracted: Vec<(PathBuf, i64, u64, Vec<IndexDoc>)> = extract_pool().install(|| {
        to_extract
            .par_iter()
            .filter_map(|(path, mtime, size)| {
                if let Some(c) = ctrl {
                    if c.is_cancelled() || !c.wait_if_paused() {
                        return None;
                    }
                }
                let result = match extractor.extract(path) {
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
                                size: *size,
                                mtime: *mtime,
                            });
                        }
                        // 压缩包内层条目：每条作为独立虚拟文档（path#entry）
                        for child in r.children {
                            if !child.text.is_empty() {
                                let body_len = child.text.len() as u64;
                                docs.push(IndexDoc {
                                    path: format!("{}#{}", path.to_string_lossy(), child.name),
                                    title: child.name,
                                    body: child.text,
                                    size: body_len,
                                    mtime: *mtime,
                                });
                            }
                        }
                        Some(docs)
                    }
                    Err(extract::ExtractError::Unsupported(_)) => None,
                    Err(e) => {
                        // 与全量路径同语义：打日志跳过，旧 doc 保留（陈旧优于丢失）
                        eprintln!("提取失败，跳过 {}: {e}", path.display());
                        None
                    }
                };
                result.map(|docs| (path.clone(), *mtime, *size, docs))
            })
            .collect()
    });
    if ctrl.is_some_and(|c| c.is_cancelled()) {
        // 提取期间取消：整体放弃（原子性——不写任何变更）
        return Ok(ScanStats {
            scanned: 0,
            indexed: 0,
            skipped: 0,
            failed: 0,
            cancelled: true,
        });
    }

    // 4b. 提取成功的文件：旧 doc 全删后重建（覆盖内层条目增减与「变空文件」）
    let mut indexed = 0usize;
    for (path, _, _, _docs) in &extracted {
        if let Some(old) = old_docs.get(path.to_string_lossy().as_ref()) {
            for q in old {
                if !walk_set.contains(q) {
                    to_delete.push(q.clone());
                }
            }
        }
        indexed += 1;
    }

    // 4c. 写侧：先删后建，一次 commit；零变更不碰 writer（不争目录锁）
    let has_upserts = extracted.iter().any(|(_, _, _, docs)| !docs.is_empty());
    if !to_delete.is_empty() || has_upserts {
        let mut writer = Writer::open(index_dir)?;
        for q in &to_delete {
            writer.delete(q)?;
        }
        for (_, _, _, docs) in &extracted {
            for d in docs {
                writer.upsert(d)?;
            }
        }
        writer.commit()?;
    }

    Ok(ScanStats {
        scanned: total,
        indexed,
        skipped,
        failed: 0,
        cancelled: false,
    })
}

/// 轻量 walk（F-1）：与 `index_directory_controlled` 同款过滤配置，但只收集
/// `(path, mtime, size)` 元组，不 extract——元数据级操作，相对 extract 廉价。
///
/// 串行遍历（全量路径的并行遍历是为 extract 吞吐服务；元数据 walk 是 IO
/// 密集型轻操作，串行更简单且顺序确定）。
/// 取消语义：walk 期间收到取消立即停止收集，由调用方整体放弃本次同步。
fn walk_meta_only(
    root: &Path,
    rules: &RuleSet,
    exts: &[String],
    ctrl: Option<&ScanControl>,
) -> Vec<(PathBuf, i64, u64)> {
    // filter_entry 要求 'static，规则集 clone 进闭包（GlobMatcher 内部为 Arc，克隆廉价）
    let rules_owned = rules.clone();
    let mut out = Vec::new();
    for entry in WalkBuilder::new(root)
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
    {
        if ctrl.is_some_and(|c| c.is_cancelled()) {
            break;
        }
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        // 过滤口径与全量路径 L235-239 逐项对齐
        if meta.len() > MAX_FILE_SIZE || !ext_allowed(entry.path(), exts) {
            continue;
        }
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        out.push((entry.into_path(), mtime, meta.len()));
    }
    out
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

    /* ------------------------------ F-1 sync_root 门禁 ------------------------------ */

    /// 计数桩（R7）：仍走真实提取，只计数调用次数
    #[derive(Default)]
    struct CountingExtract {
        calls: AtomicUsize,
    }

    impl ExtractFn for CountingExtract {
        fn extract(&self, path: &Path) -> Result<extract::ExtractResult, extract::ExtractError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            extract::extract_path(path)
        }
    }

    /// 建 zip fixture（Stored 压缩，与 archive.rs 测试同款写法）
    fn make_zip(dir: &Path, name: &str, entries: &[(&str, &str)]) -> PathBuf {
        use std::io::Write;
        let p = dir.join(name);
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (n, c) in entries {
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            writer.start_file(*n, opts).unwrap();
            writer.write_all(c.as_bytes()).unwrap();
        }
        let bytes = writer.finish().unwrap().into_inner();
        std::fs::write(&p, bytes).unwrap();
        p
    }

    /// 索引中位于 root 之下的 doc path 清单
    fn doc_paths(idx: &Path, root: &Path) -> Vec<String> {
        crate::index::list_doc_meta(idx, Some(root))
            .unwrap()
            .into_iter()
            .map(|(p, _, _)| p)
            .collect()
    }

    /// R7 门禁：全未变 → 零 extract+upsert，且 extract 计数桩调用数为 0
    #[test]
    fn sync_skips_unchanged() {
        let src = TempDir::new("sync-src");
        let idx = TempDir::new("sync-idx");
        write(src.path(), "a.txt", "内容甲 世界");
        write(src.path(), "b.txt", "内容乙 世界");
        write(src.path(), "sub/c.md", "内容丙 世界");
        index_directory(src.path(), idx.path(), |_, _| {}).unwrap();
        assert_eq!(doc_paths(idx.path(), src.path()).len(), 3);

        let stub = CountingExtract::default();
        let stats = sync_root_with(
            idx.path(),
            src.path(),
            &RuleSet::empty(),
            &Config::default(),
            None,
            &stub,
        )
        .unwrap();
        assert_eq!(stats.indexed, 0, "全部未变 → 零 extract+upsert");
        assert_eq!(stats.skipped, 3);
        assert_eq!(
            stub.calls.load(Ordering::SeqCst),
            0,
            "R7 门禁：跳过时零 extract"
        );
        assert_eq!(doc_paths(idx.path(), src.path()).len(), 3);
    }

    /// 内容变更 → 重提取入库（更长内容：同秒保存也靠 size 判定捕获，R2 缓解组合 a）
    #[test]
    fn sync_picks_up_edit() {
        let src = TempDir::new("sync-src");
        let idx = TempDir::new("sync-idx");
        let f = src.path().join("a.txt");
        std::fs::write(&f, "旧内容 世界").unwrap();
        index_directory(src.path(), idx.path(), |_, _| {}).unwrap();

        std::fs::write(&f, "新内容 世界，比原来明显更长一些").unwrap();
        let stats = sync_root(
            idx.path(),
            src.path(),
            &RuleSet::empty(),
            &Config::default(),
            None,
        )
        .unwrap();
        assert_eq!(stats.indexed, 1);
        assert_eq!(stats.skipped, 0);

        let mgr = crate::index::IndexManager::open(idx.path()).unwrap();
        let body = mgr.doc_body_by_path(&f.to_string_lossy()).unwrap();
        assert!(body.contains("新内容"), "索引应更新为新内容: {body}");
        assert!(body.contains("比原来"), "索引应更新为新内容: {body}");
        assert!(!body.contains("旧内容"));
    }

    /// 新增文件 → 入库
    #[test]
    fn sync_picks_up_new_file() {
        let src = TempDir::new("sync-src");
        let idx = TempDir::new("sync-idx");
        write(src.path(), "a.txt", "内容甲 世界");
        index_directory(src.path(), idx.path(), |_, _| {}).unwrap();

        write(src.path(), "b.txt", "内容乙 世界");
        let stats = sync_root(
            idx.path(),
            src.path(),
            &RuleSet::empty(),
            &Config::default(),
            None,
        )
        .unwrap();
        assert_eq!(stats.indexed, 1);
        assert_eq!(stats.skipped, 1);
        let paths = doc_paths(idx.path(), src.path());
        assert_eq!(paths.len(), 2);
    }

    /// R1 误删回归：删一个文件 → 仅其 doc 消失，其余文档不动
    #[test]
    fn sync_removes_deleted() {
        let src = TempDir::new("sync-src");
        let idx = TempDir::new("sync-idx");
        write(src.path(), "a.txt", "内容甲 世界");
        write(src.path(), "b.txt", "内容乙 世界");
        write(src.path(), "c.txt", "内容丙 世界");
        index_directory(src.path(), idx.path(), |_, _| {}).unwrap();

        std::fs::remove_file(src.path().join("b.txt")).unwrap();
        let stats = sync_root(
            idx.path(),
            src.path(),
            &RuleSet::empty(),
            &Config::default(),
            None,
        )
        .unwrap();
        assert_eq!(stats.indexed, 0, "纯删除场景无 extract");
        let paths = doc_paths(idx.path(), src.path());
        assert_eq!(paths.len(), 2, "仅删 b.txt 的 doc");
        assert!(!paths.iter().any(|p| p.ends_with("b.txt")));
        assert!(paths.iter().any(|p| p.ends_with("a.txt")));
        assert!(paths.iter().any(|p| p.ends_with("c.txt")));
    }

    /// zip 内容变（mtime/size 变）→ 内层重索引，消失的内层条目被删除
    #[test]
    fn sync_handles_archive_mtime() {
        let src = TempDir::new("sync-src");
        let idx = TempDir::new("sync-idx");
        let zip_path = make_zip(
            src.path(),
            "arch.zip",
            &[
                ("one.txt", "第一条目内容 世界"),
                ("two.txt", "第二条目内容 世界"),
            ],
        );
        index_directory(src.path(), idx.path(), |_, _| {}).unwrap();
        let before = doc_paths(idx.path(), src.path());
        assert!(before.iter().any(|p| p.ends_with("arch.zip#one.txt")));
        assert!(before.iter().any(|p| p.ends_with("arch.zip#two.txt")));
        assert!(
            !before.iter().any(|p| p.ends_with("arch.zip")),
            "压缩包外层 text 为空，不应有主 doc"
        );

        // 重写 zip：two.txt 消失，one.txt 内容变
        make_zip(
            src.path(),
            "arch.zip",
            &[("one.txt", "第一条目新内容 世界")],
        );
        // 显式推进 mtime（内层-only 文档无 size 可比，走 mtime 单判；同秒重写
        // 会被秒级精度判为未变——R2 已知限制，这里显式设不同 mtime 命中判定路径）
        {
            use std::fs::FileTimes;
            let f = std::fs::OpenOptions::new()
                .write(true)
                .open(&zip_path)
                .unwrap();
            f.set_times(FileTimes::new().set_modified(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(2_000_000_000),
            ))
            .unwrap();
        }
        let stats = sync_root(
            idx.path(),
            src.path(),
            &RuleSet::empty(),
            &Config::default(),
            None,
        )
        .unwrap();
        assert_eq!(stats.indexed, 1, "zip 变更 → 重提取");
        let after = doc_paths(idx.path(), src.path());
        assert!(after.iter().any(|p| p.ends_with("arch.zip#one.txt")));
        assert!(
            !after.iter().any(|p| p.ends_with("arch.zip#two.txt")),
            "消失的内层条目应被删除: {after:?}"
        );
        let mgr = crate::index::IndexManager::open(idx.path()).unwrap();
        let inner = format!("{}#one.txt", zip_path.to_string_lossy());
        let body = mgr.doc_body_by_path(&inner).unwrap();
        assert!(body.contains("第一条目新内容"), "内层应更新: {body}");
    }

    /// zip 未变 → 整个包跳过（内层-only 形态的 mtime 单判路径）
    #[test]
    fn sync_skips_unchanged_archive() {
        let src = TempDir::new("sync-src");
        let idx = TempDir::new("sync-idx");
        make_zip(src.path(), "arch.zip", &[("one.txt", "第一条目内容 世界")]);
        index_directory(src.path(), idx.path(), |_, _| {}).unwrap();

        let stub = CountingExtract::default();
        let stats = sync_root_with(
            idx.path(),
            src.path(),
            &RuleSet::empty(),
            &Config::default(),
            None,
            &stub,
        )
        .unwrap();
        assert_eq!(stats.indexed, 0);
        assert_eq!(stats.skipped, 1);
        assert_eq!(stub.calls.load(Ordering::SeqCst), 0);
    }

    /// 缺根清理：root 被整体删除 → 仅该 root 文档被清除，其他 root 不动
    #[test]
    fn sync_missing_root_clears_own_docs() {
        let root_a = TempDir::new("sync-a");
        let root_b = TempDir::new("sync-b");
        let idx = TempDir::new("sync-idx");
        write(root_a.path(), "a1.txt", "甲 世界");
        write(root_b.path(), "b1.txt", "乙 世界");
        index_directory(root_a.path(), idx.path(), |_, _| {}).unwrap();
        index_directory(root_b.path(), idx.path(), |_, _| {}).unwrap();
        assert_eq!(
            crate::index::list_doc_meta(idx.path(), None).unwrap().len(),
            2
        );

        std::fs::remove_dir_all(root_a.path()).unwrap();
        let stats = sync_root(
            idx.path(),
            root_a.path(),
            &RuleSet::empty(),
            &Config::default(),
            None,
        )
        .unwrap();
        assert_eq!(stats.scanned, 0, "root 不存在 → walk 空清单");
        assert_eq!(stats.indexed, 0);

        let remaining = crate::index::list_doc_meta(idx.path(), None).unwrap();
        assert_eq!(remaining.len(), 1, "仅 A 的文档被清除");
        assert!(remaining[0].0.contains("b1.txt"));
        assert_eq!(doc_paths(idx.path(), root_b.path()).len(), 1, "B 不受牵连");
    }

    /// 原子性：取消 → 不写任何变更（绝不把空/半截清单当删除依据）
    #[test]
    fn sync_cancel_makes_no_changes() {
        let src = TempDir::new("sync-src");
        let idx = TempDir::new("sync-idx");
        write(src.path(), "a.txt", "内容甲 世界");
        index_directory(src.path(), idx.path(), |_, _| {}).unwrap();

        let ctrl = ScanControl::new();
        ctrl.cancel();
        let stats = sync_root(
            idx.path(),
            src.path(),
            &RuleSet::empty(),
            &Config::default(),
            Some(&ctrl),
        )
        .unwrap();
        assert!(stats.cancelled);
        assert_eq!(stats.indexed, 0);
        assert_eq!(
            doc_paths(idx.path(), src.path()).len(),
            1,
            "取消不写任何变更"
        );
    }

    /// 枚举性能门禁（方案 §4 步 2）：2 万篇合成索引，`list_doc_meta` 带 root
    /// 前缀过滤全量枚举，宽松上限 2s 防劣化（目标量级 <200ms/10 万篇的外推）。
    #[test]
    fn list_doc_meta_scales_to_20k_docs() {
        let idx = TempDir::new("sync-perf");
        {
            let mut w = crate::index::Writer::open(idx.path()).unwrap();
            for i in 0..20_000 {
                let d = IndexDoc {
                    path: format!("/perf/root/f{i:05}.txt"),
                    title: format!("f{i:05}.txt"),
                    body: format!("性能枚举测试内容 {i} 世界"),
                    size: 32,
                    mtime: 1_700_000_000,
                };
                w.upsert(&d).unwrap();
            }
            w.commit().unwrap();
        }
        let start = std::time::Instant::now();
        let listed =
            crate::index::list_doc_meta(idx.path(), Some(Path::new("/perf/root"))).unwrap();
        let elapsed = start.elapsed();
        assert_eq!(listed.len(), 20_000, "root 前缀过滤应保留全部 2 万篇");
        assert!(
            elapsed.as_secs() < 2,
            "枚举 2 万篇超过 2s 宽松上限: {elapsed:?}"
        );
        // 前缀隔离：换一个不存在的 root 应零命中
        assert!(
            crate::index::list_doc_meta(idx.path(), Some(Path::new("/perf/other")))
                .unwrap()
                .is_empty()
        );
    }
}
