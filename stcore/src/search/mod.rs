//! 搜索层：三种模式 + 查询映射（S8 spike 37/37 定型版移植）
//!
//! 模式语义：
//!   模糊（默认）= 分词 AND：查询侧无缝切分，所有 token 都必须命中
//!   精准 = 完全匹配：token 粗召回 + docstore 子串复核
//!   高级 = 布尔 DSL：nom 8 parser → AST → BooleanQuery
//!
//! 不用 QueryParser（S8 对照组实证其退化）：QueryParser 用字段 tokenizer
//! 分词查询（=索引侧宽分词），AND 语义要求"华人/共和"等重叠子词全中，
//! 模糊搜索退化为精准。必须手写 AST→BooleanQuery 映射。

use std::ops::Bound as RangeBound;
use std::path::Path;
use std::time::{Duration, Instant};

use regex::Regex;
use tantivy::collector::{Count, TopDocs};
use tantivy::query::{BooleanQuery, BoostQuery, EmptyQuery, Occur, Query, RangeQuery, TermQuery};
use tantivy::schema::{Field, IndexRecordOption};
use tantivy::Term;

use crate::dsl::{self, Expr};
use crate::index::IndexManager;
use crate::seg;

#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    Fuzzy,
    Exact,
    Dsl,
    /// 正则模式（架构 v2 D8）：对**正文文本**跑正则（非分词 term），
    /// 先按其余条件（时间区间/目录/类型）收窄候选集，再逐篇 docstore 匹配，
    /// 候选篇数/扫描字节/时长三重熔断（REGEX_* 常量）。
    Regex,
}

/// 结果排序口径（对应前端 SortKey）——契约已定型，勿改变体名
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
    /// BM25 相关度
    #[default]
    Relevance,
    /// 修改时间倒序
    Mtime,
    /// 文件名升序
    Name,
}

/// 搜索附加过滤（指定目录 / 指定文件类型）——契约已定型，勿改字段名
///
/// 注：类型名 → 扩展名列表的解析在 app 层（读 stcore::config）完成，
/// 本层只按扩展名白名单过滤，避免 stcore::search 依赖 stcore::config。
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFilter {
    /// 只保留该目录前缀下的结果（None = 全部目录）
    pub dir: Option<String>,
    /// 只保留这些扩展名的结果（None = 全部文件类型）
    pub exts: Option<Vec<String>>,
    /// 排序口径
    pub sort: SortKey,
}

impl SearchFilter {
    /// 目录 + 扩展名双重过滤（压缩包内层条目 `包.zip#entry` 以包路径判定）
    pub fn accept(&self, path: &str) -> bool {
        if let Some(dir) = &self.dir {
            let base = path.split('#').next().unwrap_or(path);
            // 路径分量边界：/data/docs 命中 /data/docs/a.txt 与 /data/docs 本身，
            // 但不命中 /data/docsx（旧 starts_with 字符串前缀误报）
            if !Path::new(base).starts_with(Path::new(dir.as_str())) {
                return false;
            }
        }
        if let Some(exts) = &self.exts {
            let base = path.split('#').next().unwrap_or(path);
            let ext = base
                .rsplit_once('.')
                .map(|(_, e)| e.to_lowercase())
                .unwrap_or_default();
            if !exts.iter().any(|x| x.to_lowercase() == ext) {
                return false;
            }
        }
        true
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchHit {
    pub path: String,
    pub title: String,
    pub hits: usize,
    /// 首个命中上下文（兼容字段：前端 snippets 为空时回退渲染它）
    pub snippet: String,
    /// 多片段摘要（E-2 O4）：首/中/末至多 3 条，长文档一屏看全部命中分布。
    /// serde default：旧数据/旧消费者缺字段时反序列化为空 → 回退 snippet。
    #[serde(default)]
    pub snippets: Vec<String>,
    pub mtime: i64,
    pub size: u64,
}

/// 搜索响应（R3 分页契约）：hits 为当前页，total 为命中总数
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub hits: Vec<SearchHit>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
    /// true = 因熔断（regex 三上限 / 超额取样封顶）提前终止，结果可能不完整
    pub partial: bool,
}

// ------------------------------------------------------------
// regex 模式熔断上限（架构 v2 D8：候选篇数 / 累计扫描字节 / 总时长）
// ------------------------------------------------------------
pub const REGEX_MAX_CANDIDATES: usize = 5000;
pub const REGEX_MAX_SCAN_BYTES: usize = 256 * 1024 * 1024;
pub const REGEX_TIME_BUDGET: Duration = Duration::from_secs(5);
/// regex 模式下单篇命中计数上限（计满即停，防全篇扫描退化）
const REGEX_HITS_CAP: usize = 20;

/// 模式 → 存储串（history.mode 列 / DTO 复用）
pub fn mode_str(mode: SearchMode) -> &'static str {
    match mode {
        SearchMode::Fuzzy => "fuzzy",
        SearchMode::Exact => "exact",
        SearchMode::Dsl => "dsl",
        SearchMode::Regex => "regex",
    }
}

// ------------------------------------------------------------
// 查询映射（spike s8-dsl map_to_tantivy 移植）
// ------------------------------------------------------------

/// 单个查询 token → tantivy 查询（≤3 字原样；>3 字拆 bigram AND）
fn token_query(field: Field, tok: &str) -> Box<dyn Query> {
    let terms = seg::expand_token_to_terms(tok);
    let clauses: Vec<(Occur, Box<dyn Query>)> = terms
        .iter()
        .map(|t| {
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(field, t),
                    IndexRecordOption::WithFreqsAndPositions,
                )) as Box<dyn Query>,
            )
        })
        .collect();
    if clauses.len() == 1 {
        clauses.into_iter().next().unwrap().1
    } else {
        Box::new(BooleanQuery::new(clauses))
    }
}

/// fuzzy term：所有 token 全 MUST（AND）；token 命中同义组时该 token 变为
/// "原词 OR 同义词组"（召回扩展，AND 主语义不变）
fn fuzzy_term(field: Field, text: &str, synonyms: &crate::synonym::SynonymTable) -> Box<dyn Query> {
    let toks = seg::seg_query_tokens(text);
    if toks.is_empty() {
        return Box::new(EmptyQuery);
    }
    Box::new(BooleanQuery::new(
        toks.iter()
            .map(|t| {
                let syns = synonyms.synonyms_of(t);
                if syns.is_empty() {
                    (Occur::Must, token_query(field, t))
                } else {
                    let mut clauses: Vec<(Occur, Box<dyn Query>)> =
                        vec![(Occur::Should, token_query(field, t))];
                    for s in &syns {
                        clauses.push((Occur::Should, token_query(field, s)));
                    }
                    (
                        Occur::Must,
                        Box::new(BooleanQuery::new(clauses)) as Box<dyn Query>,
                    )
                }
            })
            .collect(),
    ))
}

/// exact term：token OR 粗召回（子串复核由调用方做）
fn exact_term(field: Field, text: &str) -> Box<dyn Query> {
    let toks = seg::seg_query_tokens(text);
    if toks.is_empty() {
        return Box::new(EmptyQuery);
    }
    Box::new(BooleanQuery::new(
        toks.iter()
            .map(|t| (Occur::Should, token_query(field, t)))
            .collect(),
    ))
}

/// 查询映射上下文（字段 + 扩展策略），随 AST 递归传递
struct MapCtx<'a> {
    body: Field,
    title: Field,
    pinyin: Field,
    synonyms: &'a crate::synonym::SynonymTable,
    /// 是否启用查询扩展（拼音 + 同义词）：
    /// 仅 fuzzy 启用；exact 是"完全匹配"语义、dsl 由用户显式控制运算符，均不自动扩展
    expand: bool,
}

/// AST → tantivy 查询 + 需子串复核的 exact 词表
fn map_to_tantivy(expr: &Expr, ctx: &MapCtx<'_>) -> (Box<dyn Query>, Vec<String>) {
    match expr {
        Expr::Empty => (Box::new(EmptyQuery), vec![]),
        Expr::Term { text, exact } => {
            // title^2 boost：标题命中的子句权重 ×2.0，body 保持 1.0。
            // 两字段均用 Occur::Should 组合，"仅标题命中"的文档也能进入结果，
            // 并通过 BoostQuery 加权使标题命中排在正文命中之前（任务卡 1.5）。
            let body_q = if *exact {
                exact_term(ctx.body, text)
            } else {
                fuzzy_term(ctx.body, text, ctx.synonyms)
            };
            let title_q = if *exact {
                exact_term(ctx.title, text)
            } else {
                fuzzy_term(ctx.title, text, ctx.synonyms)
            };
            let title_boost: Box<dyn Query> = Box::new(BoostQuery::new(title_q, 2.0));
            let mut clauses: Vec<(Occur, Box<dyn Query>)> =
                vec![(Occur::Should, body_q), (Occur::Should, title_boost)];
            // 拼音通道（P3-C）：纯 ASCII 查询（如 renmin）额外投到 pinyin 字段
            if ctx.expand && crate::pinyin::is_pinyin_query(text) {
                // 拼音字段不接受同义扩展（同义词是中文语义概念）
                let empty = crate::synonym::SynonymTable::default();
                clauses.push((Occur::Should, fuzzy_term(ctx.pinyin, text, &empty)));
            }
            let exacts = if *exact { vec![text.clone()] } else { vec![] };
            (Box::new(BooleanQuery::new(clauses)), exacts)
        }
        Expr::And(v) => {
            let mut exacts = Vec::new();
            let clauses: Vec<(Occur, Box<dyn Query>)> = v
                .iter()
                .map(|e| {
                    let (q, ex) = map_to_tantivy(e, ctx);
                    exacts.extend(ex);
                    (Occur::Must, q)
                })
                .collect();
            (Box::new(BooleanQuery::new(clauses)), exacts)
        }
        Expr::Or(v) => {
            let mut exacts = Vec::new();
            let clauses: Vec<(Occur, Box<dyn Query>)> = v
                .iter()
                .map(|e| {
                    let (q, ex) = map_to_tantivy(e, ctx);
                    exacts.extend(ex);
                    (Occur::Should, q)
                })
                .collect();
            (Box::new(BooleanQuery::new(clauses)), exacts)
        }
        Expr::Not(e) => {
            let (q, _ex) = map_to_tantivy(e, ctx);
            let clauses: Vec<(Occur, Box<dyn Query>)> = vec![
                (
                    Occur::Must,
                    Box::new(tantivy::query::AllQuery) as Box<dyn Query>,
                ),
                (Occur::MustNot, q),
            ];
            // 否定词的 exact 词面不上抛：反向差集必不含这些词，
            // 参与正向子串复核会恒空（DSL `"a" & !"b"` 历史 Bug）
            (Box::new(BooleanQuery::new(clauses)), Vec::new())
        }
    }
}

// ------------------------------------------------------------
// 搜索入口
// ------------------------------------------------------------

/// 执行搜索（兼容入口，P0+ 契约：返回第一页前 `limit` 条）
///
/// CLI / service 层沿用本签名；GUI 分页走 [`search_page`]。
pub fn search(
    mgr: &IndexManager,
    query: &str,
    mode: SearchMode,
    reverse: bool,
    limit: usize,
    filter: &SearchFilter,
) -> anyhow::Result<Vec<SearchHit>> {
    search_page(mgr, query, mode, reverse, 0, limit, None, None, filter).map(|r| r.hits)
}

/// 分页搜索（R3 契约）：
///   - `page` 从 0 计；`page_size` 为每页条数
///   - `mtime_from`/`mtime_to`（unix 秒，闭区间）为时间区间过滤，None = 不限
///     （注：未并入 SearchFilter 是因 main.rs/service.rs 的字面量构造不可改动）
///   - total：非 regex 模式 = 同 searcher 的 Count collector 计数（含目录/类型
///     过滤前的召回口径，与旧"超额取样"语义一致）；regex 模式 = 命中篇数
#[allow(clippy::too_many_arguments)]
pub fn search_page(
    mgr: &IndexManager,
    query: &str,
    mode: SearchMode,
    reverse: bool,
    page: u32,
    page_size: usize,
    mtime_from: Option<i64>,
    mtime_to: Option<i64>,
    filter: &SearchFilter,
) -> anyhow::Result<SearchResponse> {
    if mode == SearchMode::Regex {
        return regex_search_with_limits(
            mgr,
            query,
            page,
            page_size,
            mtime_from,
            mtime_to,
            filter,
            REGEX_MAX_CANDIDATES,
            REGEX_MAX_SCAN_BYTES,
            REGEX_TIME_BUDGET,
        );
    }

    let expr: Expr = match mode {
        SearchMode::Fuzzy | SearchMode::Exact => {
            if query.trim().is_empty() {
                Expr::Empty
            } else {
                // fuzzy/exact 单模式：整串作为一个 term（fuzzy 走分词 AND；
                // exact 的"完全匹配"由上层加引号处理——CLI/UI 侧 exact 模式
                // 直接把 query 包成 Term{exact:true} 语义）
                match mode {
                    SearchMode::Exact => Expr::Term {
                        text: query.to_string(),
                        exact: true,
                    },
                    _ => Expr::Term {
                        text: query.to_string(),
                        exact: false,
                    },
                }
            }
        }
        SearchMode::Dsl => dsl::parse_query(query).map_err(|e| anyhow::anyhow!(e))?,
        SearchMode::Regex => unreachable!("regex 已在函数入口分流"),
    };

    let ctx = MapCtx {
        body: mgr.f_body,
        title: mgr.f_title,
        pinyin: mgr.f_pinyin,
        synonyms: &mgr.synonyms,
        // 查询扩展（拼音 + 同义词）仅 fuzzy 启用
        expand: matches!(mode, SearchMode::Fuzzy),
    };
    let (query_obj, exacts) = map_to_tantivy(&expr, &ctx);

    // 空查询 = 浏览全库
    let query_obj: Box<dyn Query> = if query.trim().is_empty() && !reverse {
        Box::new(tantivy::query::AllQuery)
    } else if reverse {
        Box::new(BooleanQuery::new(vec![
            (
                Occur::Must,
                Box::new(tantivy::query::AllQuery) as Box<dyn Query>,
            ),
            (Occur::MustNot, query_obj),
        ]))
    } else {
        query_obj
    };

    // 时间区间（RangeQuery on mtime，与反向/其余子句 MUST 同层组合）
    let clauses = mtime_clauses(mgr, mtime_from, mtime_to);
    let query_obj: Box<dyn Query> = if clauses.is_empty() {
        query_obj
    } else {
        let mut all = vec![(Occur::Must, query_obj)];
        all.extend(clauses);
        Box::new(BooleanQuery::new(all))
    };

    let reader = mgr.index.reader()?;
    let searcher = reader.searcher();
    // total：Count collector 单独一查（与 TopDocs 同一 searcher 快照）
    let total: u64 = searcher.search(&*query_obj, &Count)? as u64;

    // 过滤/重排需要超额取样；封顶 2000 条避免大库退化
    let offset = page as usize * page_size;
    let fetch = (offset + page_size)
        .saturating_mul(8)
        .min(2000)
        .max(page_size);
    let top = searcher.search(&*query_obj, &TopDocs::with_limit(fetch).order_by_score())?;

    // 高亮/复核 token：fuzzy 用无缝切分；exact 用整串；dsl 收集全部词面
    // （fuzzy 词面同款切分、exact 引号词用整串——与 tantivy 查询映射同源）
    let mut hl_tokens: Vec<String> = match mode {
        SearchMode::Fuzzy => seg::seg_query_tokens(query),
        SearchMode::Exact => vec![query.to_string()],
        SearchMode::Dsl => {
            let mut tagged = Vec::new();
            expr.collect_terms_tagged(&mut tagged);
            let mut toks: Vec<String> = Vec::new();
            for (t, exact) in &tagged {
                if *exact {
                    toks.push(t.clone());
                } else {
                    toks.extend(seg::seg_query_tokens(t));
                }
            }
            toks.sort();
            toks.dedup();
            toks
        }
        SearchMode::Regex => unreachable!("regex 已在函数入口分流"),
    };

    // fuzzy 下同义词命中也要计入"命中数"与高亮：把同义词并入统计词表。
    // （拼音命中无法反查中文词面，其命中数仍为 0 —— 已在文档中登记）
    if matches!(mode, SearchMode::Fuzzy) && !mgr.synonyms.is_empty() {
        let extra: Vec<String> = hl_tokens
            .iter()
            .flat_map(|t| mgr.synonyms.synonyms_of(t))
            .collect();
        hl_tokens.extend(extra);
        hl_tokens.sort();
        hl_tokens.dedup();
    }

    let mut hits = Vec::new();
    for (_, addr) in top {
        let Some((path, body)) = mgr.doc_body(addr) else {
            continue;
        };
        // exact 复核：索引侧全小写召回，复核同样 ASCII 折叠比较，
        // 保证 "hello" 能命中正文 "Hello"（大小写口径统一）；
        // reverse=true 时跳过——正向 exacts 由正向表达式收集，差集必不含这些词，
        // 复核恒空（历史 Bug：反向 + 含 exact 的查询恒无结果）
        if !reverse && !exacts.is_empty() {
            let body_folded = body.to_ascii_lowercase();
            if !exacts
                .iter()
                .all(|ex| body_folded.contains(&ex.to_ascii_lowercase()))
            {
                continue;
            }
        }
        // 目录 / 类型过滤
        if !filter.accept(&path) {
            continue;
        }
        let title = path.rsplit('/').next().unwrap_or(&path).to_string();
        let (hits_count, snippet, snippets) = count_and_snip(&body, &hl_tokens);
        let mtime = doc_field_i64(&searcher, addr, mgr.f_mtime);
        let size = doc_field_i64(&searcher, addr, mgr.f_size) as u64;
        hits.push(SearchHit {
            path,
            title,
            hits: hits_count,
            snippet,
            snippets,
            mtime,
            size,
        });
    }

    sort_hits(&mut hits, filter.sort);
    // 分页切片：hits 已按最终排序，取 [offset, offset+page_size)
    let hits = hits
        .into_iter()
        .skip(offset)
        .take(page_size)
        .collect::<Vec<_>>();
    Ok(SearchResponse {
        hits,
        total,
        page,
        page_size: page_size as u32,
        partial: false,
    })
}

/// regex 模式（可注入小上限，供熔断测试）：先按时间区间收窄候选集，
/// 枚举候选（TopDocs 上限 = `max_candidates`）→ 逐篇取 docstore 正文匹配。
///
/// 熔断三上限：候选篇数 / 累计扫描字节 / 总时长；任一触顶即 `partial=true` 提前返回。
/// 注：regex 作用于正文文本，query 词面无分词语义，故反向（reverse）在
/// regex 模式下不参与候选查询组合（正则本身决定匹配，详见架构 v2 D8）。
#[allow(clippy::too_many_arguments)]
pub fn regex_search_with_limits(
    mgr: &IndexManager,
    pattern: &str,
    page: u32,
    page_size: usize,
    mtime_from: Option<i64>,
    mtime_to: Option<i64>,
    filter: &SearchFilter,
    max_candidates: usize,
    max_scan_bytes: usize,
    budget: Duration,
) -> anyhow::Result<SearchResponse> {
    let re = Regex::new(pattern).map_err(|e| anyhow::anyhow!("正则表达式无效: {e}"))?;

    let mut clauses: Vec<(Occur, Box<dyn Query>)> = vec![(
        Occur::Must,
        Box::new(tantivy::query::AllQuery) as Box<dyn Query>,
    )];
    clauses.extend(mtime_clauses(mgr, mtime_from, mtime_to));
    let base = BooleanQuery::new(clauses);

    let reader = mgr.index.reader()?;
    let searcher = reader.searcher();
    let top = searcher.search(&base, &TopDocs::with_limit(max_candidates).order_by_score())?;

    let started = Instant::now();
    let mut scanned: usize = 0;
    let mut partial = false;
    let mut matched: Vec<SearchHit> = Vec::new();
    let mut seen = 0usize;
    for (_, addr) in top {
        seen += 1;
        if started.elapsed() > budget {
            partial = true;
            break;
        }
        let Some((path, body)) = mgr.doc_body(addr) else {
            continue;
        };
        // 目录 / 类型过滤（与 regex 同为候选收窄条件）
        if !filter.accept(&path) {
            continue;
        }
        scanned = scanned.saturating_add(body.len());
        if scanned > max_scan_bytes {
            partial = true;
            break;
        }
        if re.is_match(&body) {
            let title = path.rsplit('/').next().unwrap_or(&path).to_string();
            let (hits_count, snippet, snippets) = regex_count_and_snip(&re, &body);
            let mtime = doc_field_i64(&searcher, addr, mgr.f_mtime);
            let size = doc_field_i64(&searcher, addr, mgr.f_size) as u64;
            matched.push(SearchHit {
                path,
                title,
                hits: hits_count,
                snippet,
                snippets,
                mtime,
                size,
            });
        }
    }
    // 候选篇数熔断：TopDocs 恰好取满上限且库中还有更多文档 → 截断，partial=true
    if !partial && seen >= max_candidates && searcher.num_docs() > max_candidates as u64 {
        partial = true;
    }

    sort_hits(&mut matched, filter.sort);
    let total = matched.len() as u64;
    let offset = page as usize * page_size;
    let hits = matched.into_iter().skip(offset).take(page_size).collect();
    Ok(SearchResponse {
        hits,
        total,
        page,
        page_size: page_size as u32,
        partial,
    })
}

/// mtime 时间区间 → RangeQuery 子句（闭区间；两端皆 None 时空）
fn mtime_clauses(
    mgr: &IndexManager,
    mtime_from: Option<i64>,
    mtime_to: Option<i64>,
) -> Vec<(Occur, Box<dyn Query>)> {
    if mtime_from.is_none() && mtime_to.is_none() {
        return Vec::new();
    }
    let lower = mtime_from
        .map(|v| RangeBound::Included(Term::from_field_i64(mgr.f_mtime, v)))
        .unwrap_or(RangeBound::Unbounded);
    let upper = mtime_to
        .map(|v| RangeBound::Included(Term::from_field_i64(mgr.f_mtime, v)))
        .unwrap_or(RangeBound::Unbounded);
    vec![(
        Occur::Must,
        Box::new(RangeQuery::new(lower, upper)) as Box<dyn Query>,
    )]
}

/// 排序：Relevance 保持召回次序；Mtime/Name 重排
fn sort_hits(hits: &mut [SearchHit], sort: SortKey) {
    match sort {
        SortKey::Relevance => {}
        SortKey::Mtime => hits.sort_by_key(|a| std::cmp::Reverse(a.mtime)),
        SortKey::Name => hits.sort_by_key(|a| a.title.to_lowercase()),
    }
}

fn doc_field_i64(searcher: &tantivy::Searcher, addr: tantivy::DocAddress, field: Field) -> i64 {
    searcher
        .doc(addr)
        .ok()
        .and_then(|d: TantivyDocument| d.get_first(field).and_then(|v| v.as_i64()))
        .unwrap_or(0)
}

use tantivy::schema::Value as _;
use tantivy::TantivyDocument;

/// 多片段摘要条数上限（首/中/末）
const MAX_SNIPPETS: usize = 3;

/// 命中位置采样池上限（E-2 R4：防超长文档命中上万时 Vec 内存尖峰；
/// 指数抽稀保持池 ≤ 该值，末位置另存必保）
const SNIP_POS_K: usize = 64;

/// 统计命中次数并生成摘要片段（±60 字）
///
/// ASCII 折叠口径：与索引侧大小写处理一致，且 `to_ascii_lowercase` 保持
/// 字节等长，命中偏移可直接映射回 `body`（历史 `to_lowercase` 偏移错位 Bug）。
///
/// E-2（O4）：除首个片段（`snippet` 兼容字段）外，另从采样位置产出
/// 首/中/末至多 [`MAX_SNIPPETS`] 条（`snippets`）。位置经指数抽稀采样
/// （池 ≤ [`SNIP_POS_K`]），`total` 仍为全篇真实命中总数。
fn count_and_snip(body: &str, tokens: &[String]) -> (usize, String, Vec<String>) {
    if tokens.is_empty() {
        return (0, String::new(), Vec::new());
    }
    let lower = body.to_ascii_lowercase();
    let (total, pool, last) = sample_positions(&lower, tokens);
    let snippet = match pool.first() {
        None => body.chars().take(120).collect(),
        Some(&p) => snippet_around(body, p, 60),
    };
    let snippets = snippets_from_positions(body, &pool, last, 60);
    (total, snippet, snippets)
}

/// 全篇扫描统计命中总数，并以指数抽稀采样命中位置。
///
/// 返回 `(total, pool, last)`：
/// - `total`：全篇真实命中总数（与旧口径一致，不封顶）；
/// - `pool`：采样池，长度恒 ≤ [`SNIP_POS_K`]（池满时步长翻倍 + 保留偶数位，
///   把池压回 K/2 后按新步长继续填）——防超长文档命中上万时 Vec 内存尖峰；
/// - `last`：最后一个命中位置（必保，抽稀不丢尾部）。
///
/// `lower` 须为已 ASCII 折叠的正文（命中偏移与 `body` 字节等长可映射）。
fn sample_positions(lower: &str, tokens: &[String]) -> (usize, Vec<usize>, Option<usize>) {
    let mut total = 0usize;
    let mut pool: Vec<usize> = Vec::new();
    let mut stride = 1usize;
    let mut since = 0usize;
    let mut last: Option<usize> = None;
    for t in tokens {
        if t.is_empty() {
            continue;
        }
        let tl = t.to_ascii_lowercase();
        let mut start = 0usize;
        while let Some(p) = lower[start..].find(&tl) {
            let abs = start + p;
            total += 1;
            last = Some(abs);
            if since == 0 {
                pool.push(abs);
                if pool.len() >= SNIP_POS_K {
                    // 抽稀：步长翻倍 + 池保留偶数位（池回落到 K/2，随后按新步长再填满）
                    stride *= 2;
                    let mut i = 0usize;
                    pool.retain(|_| {
                        let keep = i.is_multiple_of(2);
                        i += 1;
                        keep
                    });
                }
            }
            since += 1;
            if since >= stride {
                since = 0;
            }
            start = abs + tl.len().max(1);
        }
    }
    (total, pool, last)
}

/// regex 命中计数（计满 `REGEX_HITS_CAP` 即停）+ 首个匹配上下文（±80 字）
///
/// E-2（O4）：`snippets` 取已扫描范围内（≤ cap）的首/中/末。注意 regex
/// 熔断语义本就只扫前 cap 个命中，"末条"= 第 cap 个命中而非文档物理末尾，
/// 与 `total` 的封顶口径一致（`partial` 标志已向调用方提示不完整）。
fn regex_count_and_snip(re: &Regex, body: &str) -> (usize, String, Vec<String>) {
    let mut total = 0usize;
    let mut positions: Vec<usize> = Vec::new();
    let mut first_pos: Option<usize> = None;
    for m in re.find_iter(body) {
        if first_pos.is_none() {
            first_pos = Some(m.start());
        }
        total += 1;
        positions.push(m.start());
        if total >= REGEX_HITS_CAP {
            break;
        }
    }
    let snippet = match first_pos {
        None => String::new(),
        Some(p) => snippet_around(body, p, 80),
    };
    let last = positions.last().copied();
    let snippets = snippets_from_positions(body, &positions, last, 80);
    (total, snippet, snippets)
}

/// 采样位置 → 多片段摘要：首 / 中 / 末（≤ [`MAX_SNIPPETS`] 条）
///
/// 按**片段文本**去重（而非仅字节位）：相邻命中（间距 < 2×radius）会产生
/// 大段重叠的相同文本，只保留第一条——避免 ①②③ 角标展示重复内容。
fn snippets_from_positions(
    body: &str,
    pool: &[usize],
    last: Option<usize>,
    radius: usize,
) -> Vec<String> {
    let mut pos: Vec<usize> = Vec::with_capacity(MAX_SNIPPETS);
    if let Some(&f) = pool.first() {
        pos.push(f);
    }
    if pool.len() > 2 {
        pos.push(pool[pool.len() / 2]);
    }
    if let Some(l) = last {
        pos.push(l);
    }
    let mut out: Vec<String> = Vec::with_capacity(MAX_SNIPPETS);
    for p in pos {
        let s = snippet_around(body, p, radius);
        if !out.contains(&s) {
            out.push(s);
        }
        if out.len() >= MAX_SNIPPETS {
            break;
        }
    }
    out
}

/// 字节偏移 → ±`radius` 字符上下文片断（越界加省略号）
fn snippet_around(body: &str, byte_pos: usize, radius: usize) -> String {
    let chars: Vec<char> = body.chars().collect();
    // 字节位置 → char 位置
    let char_pos = body[..byte_pos].chars().count();
    let from = char_pos.saturating_sub(radius);
    let to = (char_pos + radius).min(chars.len());
    let mut s: String = chars[from..to].iter().collect();
    if from > 0 {
        s.insert(0, '…');
    }
    if to < chars.len() {
        s.push('…');
    }
    s
}

/* ------------------------------ 索引构成（O5，E-1） ------------------------------ */

/// 单扩展名计数（GUI 构成面板 / CLI `stats --detail` / socket stats 复用）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtStat {
    pub ext: String,
    pub docs: u64,
}

/// 索引构成：文档总数 + 按扩展名分布（`docs` 降序，同数按 ext 升序）
///
/// 不含 disk_bytes：目录体积计算在 app/CLI 层已有（`dir_size`），
/// stcore 不重复实现（单一事实源）。
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexComposition {
    pub docs: u64,
    pub by_ext: Vec<ExtStat>,
}

/// 按扩展名聚合索引构成。
///
/// ext 字段为 STRING（无 FAST，不能直接 facet 聚合）：对调用方给的扩展名
/// 集合逐个 `TermQuery + Count` collector 计数——词典级 postings 计数，
/// ~40 个 ext × O(词典项)，对话框级交互可接受（<100ms 目标）。
///
/// 归桶口径（与 `Writer::upsert` 的 ext 提取一致，见 index/mod.rs）：
/// - 压缩包内层条目 `包.zip#inner.txt` 的 ext = 内层真实扩展名（txt）；
/// - 外层 zip 自身通常无正文不入索引，故 by_ext 不含 zip 本身；
/// - 无扩展名文档（`Path::extension()` 为 None）归 `ext = ""` 桶。
pub fn index_composition(mgr: &IndexManager, exts: &[String]) -> anyhow::Result<IndexComposition> {
    let reader = mgr.index.reader()?;
    let searcher = reader.searcher();
    let docs = searcher.num_docs();
    let mut by_ext: Vec<ExtStat> = Vec::with_capacity(exts.len() + 1);
    // 派生集合 + ""（无扩展名）逐桶计数；0 篇的桶不列出（面板更干净）
    for e in exts.iter().chain(std::iter::once(&String::new())) {
        let term = Term::from_field_text(mgr.f_ext, e.as_str());
        let q = TermQuery::new(term, IndexRecordOption::Basic);
        let count = searcher.search(&q, &Count)?;
        if count > 0 {
            by_ext.push(ExtStat {
                ext: e.clone(),
                docs: count as u64,
            });
        }
    }
    by_ext.sort_by(|a, b| b.docs.cmp(&a.docs).then_with(|| a.ext.cmp(&b.ext)));
    Ok(IndexComposition { docs, by_ext })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{IndexDoc, Writer};
    use std::collections::BTreeSet;

    fn test_mgr() -> (tempfile::TempDir, IndexManager) {
        let dir = tempfile::tempdir().unwrap();
        // R2 读/写分离：测试种子经写侧 Writer 入库（IndexManager 只读）
        let mut mgr = Writer::open(dir.path()).unwrap();
        let docs = [
            ("d1", "中华人民共和国 崛起"),
            ("d2", "中华烟 与 人民好 的 朝鲜共和国 见闻"),
            ("d4", "人民币 与 朝鲜共和国 的 新闻报道"),
            ("d7", "人民 的 生活 日常 与 柴米油盐"),
        ];
        for (p, b) in docs {
            mgr.upsert(&IndexDoc {
                path: format!("/tmp/{}.txt", p),
                title: format!("{}.txt", p),
                body: b.to_string(),
                size: b.len() as u64,
                mtime: 0,
            })
            .unwrap();
        }
        mgr.commit().unwrap();
        drop(mgr);
        let mgr = IndexManager::open(dir.path()).unwrap();
        (dir, mgr)
    }

    fn paths(hits: &[SearchHit]) -> BTreeSet<String> {
        hits.iter()
            .map(|h| h.path.split('/').next_back().unwrap().replace(".txt", ""))
            .collect()
    }

    #[test]
    fn fuzzy_and_semantics() {
        let (_d, mgr) = test_mgr();
        // 模糊：中华人民共和国 → 中华/人民/共和国 AND（d2 分散出现也应命中）
        let hits = search(
            &mgr,
            "中华人民共和国",
            SearchMode::Fuzzy,
            false,
            10,
            &SearchFilter::default(),
        )
        .unwrap();
        assert_eq!(
            paths(&hits),
            ["d1", "d2"].into_iter().map(String::from).collect()
        );
        assert!(hits[0].hits > 0);
        assert!(!hits[0].snippet.is_empty());
    }

    #[test]
    fn exact_substring() {
        let (_d, mgr) = test_mgr();
        // 精准：人民共和国 → 仅含连续子串的文档（d2 的"人民好 的 朝鲜共和国"不连续，不命中）
        let hits = search(
            &mgr,
            "人民共和国",
            SearchMode::Exact,
            false,
            10,
            &SearchFilter::default(),
        )
        .unwrap();
        assert_eq!(paths(&hits), ["d1"].into_iter().map(String::from).collect());
    }

    #[test]
    fn dsl_and_not() {
        let (_d, mgr) = test_mgr();
        // DSL：中华人民共和国 & !崛起 → d2（d1 含"崛起"被排除）
        let hits = search(
            &mgr,
            "中华人民共和国 & !崛起",
            SearchMode::Dsl,
            false,
            10,
            &SearchFilter::default(),
        )
        .unwrap();
        assert_eq!(paths(&hits), ["d2"].into_iter().map(String::from).collect());
    }

    #[test]
    fn reverse_mode() {
        let (_d, mgr) = test_mgr();
        // 反向：不含"共和国"的文档
        let hits = search(
            &mgr,
            "共和国",
            SearchMode::Fuzzy,
            true,
            10,
            &SearchFilter::default(),
        )
        .unwrap();
        assert_eq!(paths(&hits), ["d7"].into_iter().map(String::from).collect());
    }

    #[test]
    fn title_boost_ranks_title_match_first() {
        // 任务卡 1.5：title^2 使"标题命中"排在"正文命中"之前
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = Writer::open(dir.path()).unwrap();
        // A：title 命中查询词"中华"，body 不含
        mgr.upsert(&IndexDoc {
            path: "/tmp/a.txt".to_string(),
            title: "中华.txt".to_string(),
            body: "完全无关的正文内容".to_string(),
            size: 12,
            mtime: 0,
        })
        .unwrap();
        // B：body 命中"中华"，title 不含
        mgr.upsert(&IndexDoc {
            path: "/tmp/b.txt".to_string(),
            title: "无题.txt".to_string(),
            body: "中华 崛起".to_string(),
            size: 8,
            mtime: 0,
        })
        .unwrap();
        mgr.commit().unwrap();
        drop(mgr);
        let mgr = IndexManager::open(dir.path()).unwrap();

        let hits = search(
            &mgr,
            "中华",
            SearchMode::Fuzzy,
            false,
            10,
            &SearchFilter::default(),
        )
        .unwrap();

        assert_eq!(hits.len(), 2, "标题命中与正文命中都应被召回");
        assert_eq!(
            hits[0].path, "/tmp/a.txt",
            "title 命中（A）应排在 body 命中（B）之前"
        );
    }

    /* ------------------------------ 索引构成（E-1） ------------------------------ */

    /// 构造混合扩展名索引：3 txt + 2 md + 1 无扩展名 + 压缩包内层 1 zip 条目
    fn seed_composition(dir: &std::path::Path) {
        let mut w = Writer::open(dir).unwrap();
        let docs = [
            ("/tmp/a.txt", "a", "txt"),
            ("/tmp/b.txt", "b", "txt"),
            ("/tmp/c.txt", "c", "txt"),
            ("/tmp/readme.md", "m", "md"),
            ("/tmp/notes.md", "n", "md"),
            ("/tmp/noext", "x", ""),
        ];
        for (p, t, _e) in docs {
            w.upsert(&IndexDoc {
                path: p.to_string(),
                title: t.to_string(),
                body: "内容".to_string(),
                size: 6,
                mtime: 0,
            })
            .unwrap();
        }
        // 压缩包内层条目：虚拟路径 ext 取内层真实扩展名（归 txt 桶）
        w.upsert(&IndexDoc {
            path: "/tmp/资料打包.zip#inner.txt".to_string(),
            title: "inner.txt".to_string(),
            body: "包内".to_string(),
            size: 6,
            mtime: 0,
        })
        .unwrap();
        w.commit().unwrap();
    }

    #[test]
    fn composition_sums_to_num_docs() {
        let dir = tempfile::tempdir().unwrap();
        seed_composition(dir.path());
        let mgr = IndexManager::open(dir.path()).unwrap();

        // 派生集合覆盖全部实际扩展名（模拟 app 层 Config::all_exts 归一化产物）
        let exts = vec!["txt".to_string(), "md".to_string()];
        let comp = index_composition(&mgr, &exts).unwrap();

        assert_eq!(comp.docs, 7, "6 文件 + 1 内层条目");
        let get = |e: &str| {
            comp.by_ext
                .iter()
                .find(|s| s.ext == e)
                .map(|s| s.docs)
                .unwrap_or(0)
        };
        assert_eq!(get("txt"), 4, "3 txt + 1 zip 内层");
        assert_eq!(get("md"), 2);
        assert_eq!(get(""), 1, "无扩展名归空桶");

        let sum: u64 = comp.by_ext.iter().map(|s| s.docs).sum();
        assert_eq!(sum, comp.docs, "各桶之和 == num_docs（无遗漏归桶）");

        // docs 降序；同数按 ext 升序（md=2 与 ""=1 不同数，直接查序）
        let order: Vec<&str> = comp.by_ext.iter().map(|s| s.ext.as_str()).collect();
        assert_eq!(order, vec!["txt", "md", ""]);

        // 0 篇桶不列出：zip 不在派生集合中则无桶
        assert!(comp.by_ext.iter().all(|s| s.docs > 0));
    }

    #[test]
    fn composition_empty_index_all_zero() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = IndexManager::open(dir.path()).unwrap();
        let comp = index_composition(&mgr, &["txt".to_string()]).unwrap();
        assert_eq!(comp.docs, 0);
        assert!(comp.by_ext.is_empty(), "空索引无桶");
    }

    /* ------------------------------ 多片段摘要（E-2） ------------------------------ */

    /// 造一篇命中 N 次「量子」的长文：每次命中间隔 200 字节（"量子" + 填充），
    /// 首/尾命中可辨识（分别带 头/尾 标记）。
    fn long_body_with_hits(n: usize) -> String {
        let mut s = String::from("文档头标记 ");
        for i in 0..n {
            s.push_str("量子");
            s.push_str(&format!("{:06}", i));
            // 填充到约 200 字节间隔（ASCII 数字，字节等长）
            s.push_str(&"-".repeat(200 - 2 - 6));
        }
        s.push_str(" 文档尾标记");
        s
    }

    #[test]
    fn snippets_capped_and_spread() {
        let body = long_body_with_hits(40);
        let tokens = vec!["量子".to_string()];
        let (total, snippet, snippets) = count_and_snip(&body, &tokens);
        assert_eq!(total, 40, "total 为全篇真实命中数");
        assert!(!snippet.is_empty(), "兼容 snippet 字段非空");

        assert!(
            snippets.len() <= MAX_SNIPPETS,
            "条数上限 3，实际 {}",
            snippets.len()
        );
        assert!(snippets.len() >= 2, "多命中应产出多条（首/中/末去重后）");
        // 首条覆盖文首命中附近（含头标记）；末条覆盖文末命中附近（含尾标记）
        assert!(
            snippets[0].contains("文档头标记"),
            "首条应含文档头附近内容：{}",
            snippets[0]
        );
        assert!(
            snippets[snippets.len() - 1].contains("000039"),
            "末条应覆盖最后一个命中（#39）：{}",
            snippets[snippets.len() - 1]
        );
        // 三条互不相同（去重生效）
        let uniq: std::collections::BTreeSet<&String> = snippets.iter().collect();
        assert_eq!(uniq.len(), snippets.len(), "片段应按位置去重");

        // 单命中：仅 1 条（首==末去重）
        let one = "只有一处量子出现在文档中".to_string();
        let (_, _, s1) = count_and_snip(&one, &tokens);
        assert_eq!(s1.len(), 1);
        // 相邻命中（间距 < 2×radius）：重叠片段按文本去重，只留 1 条
        let near = "开头量子计算先说一句，紧接着量子计算再出现一次，两处很近".to_string();
        let (total_near, _, s_near) = count_and_snip(&near, &tokens);
        assert_eq!(total_near, 2);
        assert_eq!(s_near.len(), 1, "相邻命中的重叠片段应去重");
        // 无命中：snippets 空、snippet 走 take(120) 兜底
        let (_, s0, sn0) = count_and_snip("没有任何关键词", &tokens);
        assert!(!s0.is_empty() && sn0.is_empty());
    }

    #[test]
    fn snippet_sampling_bounded() {
        // 1 万次命中：采样池必须 ≤ SNIP_POS_K，且 total 仍精确
        let body = long_body_with_hits(10_000);
        let tokens = vec!["量子".to_string()];
        let lower = body.to_ascii_lowercase();
        let (total, pool, last) = sample_positions(&lower, &tokens);
        assert_eq!(total, 10_000, "total 不被采样截断");
        assert!(
            pool.len() <= SNIP_POS_K,
            "采样池上限锁定：{} ≤ {}",
            pool.len(),
            SNIP_POS_K
        );
        assert_eq!(
            *pool.first().unwrap(),
            body.find("量子").unwrap(),
            "首位置必保"
        );
        assert_eq!(last.unwrap(), body.rfind("量子").unwrap(), "末位置必保");
        // 池内位置严格递增（扫描顺序）
        assert!(pool.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn regex_snippets_capped() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = Writer::open(dir.path()).unwrap();
        // 30 次命中 > REGEX_HITS_CAP(20)：snippets 仍 ≤3，total 封顶 20
        let body = long_body_with_hits(30);
        w.upsert(&IndexDoc {
            path: "/tmp/re.txt".to_string(),
            title: "re.txt".to_string(),
            body: body.clone(),
            size: body.len() as u64,
            mtime: 0,
        })
        .unwrap();
        w.commit().unwrap();
        drop(w);
        let mgr = IndexManager::open(dir.path()).unwrap();
        let hits = search(
            &mgr,
            "量子",
            SearchMode::Regex,
            false,
            10,
            &SearchFilter::default(),
        )
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].hits, REGEX_HITS_CAP, "regex 计数封顶");
        assert!(hits[0].snippets.len() <= MAX_SNIPPETS);
    }
}
