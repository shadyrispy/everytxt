//! search-tool 入口：Tauri GUI + headless CLI 双模式
//!
//! CLI 模式让软件在无 GUI 环境（服务器/CI/冒烟测试）完整可运行：
//!   search-tool cli index  --dir <目录> [--index-dir <路径>]
//!   search-tool cli search <查询> --mode fuzzy|exact|dsl [--reverse] [--limit N]
//!   search-tool cli stats
//!   search-tool cli testdata  # 生成演示语料并索引（冒烟用）
//!
//! 错误处理口径（P2 修复）：CLI 失败一律 `eprintln` + `exit(1)`，
//! 不再裸 `unwrap`/`expect`（避免 panic 栈 + 退出码不统一）。

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "search-tool", about = "everytxt —— 本地全文搜索引擎")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// 启动 GUI 并把该目录作为初始过滤条件（右击搜索入口，P5）
    #[arg(long, value_name = "DIR")]
    open_dir: Option<String>,
    /// CLI /s 关键字（atgui /s 关键字 [/d 盘符] [/e 后缀]；
    /// argv 以 `/` 开头时由 preprocess_slash_args 映射而来）
    #[arg(long, value_name = "KW")]
    search: Option<String>,
    /// CLI /d 盘符（如 C 或 C:\）——搜索范围限定该盘
    #[arg(long, value_name = "D")]
    drive: Option<String>,
    /// CLI /e 扩展名（如 txt）——只保留该类型结果
    #[arg(long, value_name = "E")]
    ext: Option<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// headless 模式（GUI 启动时省略子命令）
    Cli {
        #[command(subcommand)]
        sub: CliSub,
    },
}

#[derive(Subcommand)]
enum CliSub {
    /// 全量索引一个目录
    Index {
        #[arg(long)]
        dir: String,
        #[arg(long)]
        index_dir: Option<String>,
    },
    /// 搜索
    Search {
        query: String,
        #[arg(long, default_value = "fuzzy")]
        mode: String,
        #[arg(long)]
        reverse: bool,
        #[arg(long, default_value = "20")]
        limit: usize,
        #[arg(long)]
        index_dir: Option<String>,
    },
    /// 索引统计
    Stats {
        #[arg(long)]
        index_dir: Option<String>,
        /// 附带按扩展名的构成明细（不带时输出与旧版逐字节一致）
        #[arg(long)]
        detail: bool,
    },
    /// 生成演示语料并索引（冒烟验收）
    Testdata {
        #[arg(long)]
        index_dir: Option<String>,
    },
    /// 导出（E-3 O3）：results / history / bookmarks → csv | jsonl（本地运维面）
    Export {
        /// 导出对象：results（搜索结果）/ history（搜索历史）/ bookmarks（书签）
        #[arg(long, value_name = "WHAT")]
        what: String,
        /// results 必需：查询词
        #[arg(long)]
        query: Option<String>,
        /// results：搜索模式（fuzzy/exact/dsl）
        #[arg(long, default_value = "fuzzy")]
        mode: String,
        /// 条数上限（results 默认 100；history 默认全部，上限 500）
        #[arg(long)]
        limit: Option<usize>,
        /// 输出格式：csv（UTF-8 BOM，Excel/WPS 直开）/ jsonl
        #[arg(long, default_value = "csv")]
        format: String,
        /// 输出文件（缺省 stdout）
        #[arg(long)]
        out: Option<String>,
        #[arg(long)]
        index_dir: Option<String>,
    },
}

fn main() {
    //`search-tool /s 关键字 [/d 盘符] [/e 后缀]` 风格
    // 仅当 argv[1] 以 '/' 开头才启用映射，不影响 `cli ...` / `--open-dir` 等正常入口
    let argv: Vec<String> = preprocess_slash_args(&std::env::args().collect::<Vec<_>>());
    let cli = Cli::parse_from(argv);

    if let Some(kw) = cli.search.clone() {
        run_slash_search(kw, cli.drive.clone(), cli.ext.clone());
        return;
    }

    match cli.cmd {
        Some(Cmd::Cli { sub }) => run_cli(sub),
        None => search_tool_lib::run(cli.open_dir),
    }
}

/// argv 预处理：`/s`→`--search`、`/d`→`--drive`、`/e`→`--ext`（大小写不敏感）。
///
/// 仅当 `argv[1]` 以 '/' 开头才启用该模式（避免影响正常子命令）；
/// 模式内只映射**整 token 精确等于** /s、/d、/e 的项，其余（含查询词本身、
/// `/usr/share` 之类路径）原样保留。
fn preprocess_slash_args(argv: &[String]) -> Vec<String> {
    if argv.len() < 2 || !argv[1].starts_with('/') {
        return argv.to_vec();
    }
    argv.iter()
        .map(|a| match a.to_lowercase().as_str() {
            "/s" => "--search".to_string(),
            "/d" => "--drive".to_string(),
            "/e" => "--ext".to_string(),
            _ => a.clone(),
        })
        .collect()
}

/// 盘符 → 目录前缀（`SearchFilter.dir` 用）："C" → "C:/"；已带 `:`/`/` 的原样保留。
fn drive_to_dir_prefix(d: &str) -> String {
    let s = d.trim().trim_end_matches(['/', '\\']);
    if s.chars().count() == 1 {
        format!("{s}:/")
    } else {
        s.to_string()
    }
}

/// CLI 标准输出（忽略 BrokenPipe）：`| head` 等下游提前关闭管道时
/// `println!` 会 panic（`failed printing to stdout`），改用忽略错误的写出
fn out(line: std::fmt::Arguments) {
    use std::io::Write;
    let _ = writeln!(std::io::stdout(), "{line}");
}

/// 安全 println（见 [`out`]）
macro_rules! cli_out {
    ($($arg:tt)*) => {
        out(format_args!($($arg)*))
    };
}

/// CLI 错误统一出口：打印上下文后以退出码 1 结束（不再 panic 裸栈）
fn die<T, E: std::fmt::Display>(r: Result<T, E>, ctx: &str) -> T {
    match r {
        Ok(v) => v,
        Err(e) => {
            eprintln!("错误：{ctx}: {e}");
            std::process::exit(1);
        }
    }
}

/// CLI `/s` 搜索：主实例在 → socket 转发呈现后退出 0；
/// 主实例不在 → 回退本地 headless（复用 cli search 逻辑 + drive/ext 过滤）。
fn run_slash_search(query: String, drive: Option<String>, ext: Option<String>) {
    use stcore::index::IndexManager;
    use stcore::search::{search, SearchFilter, SearchMode, SortKey};
    use stcore::service::SearchService as _;

    let ext = ext
        .map(|e| e.trim_start_matches('.').to_lowercase())
        .filter(|e| !e.is_empty());
    let dir = drive
        .as_deref()
        .map(drive_to_dir_prefix)
        .filter(|d| !d.is_empty());

    // 1) 尝试单实例转发（主实例在 GUI 呈现结果）
    //    transport 按平台：Unix socket 文件 / Windows TCP 端口文件（shell.rs）
    let index_dir = search_tool_lib::state::default_index_dir();
    #[cfg(unix)]
    let client = Some(stcore::service::SocketClient::new(
        search_tool_lib::commands::shell::socket_path(&index_dir),
    ));
    #[cfg(windows)]
    let client = search_tool_lib::commands::shell::read_instance_port(&index_dir)
        .map(stcore::service::SocketClient::new_tcp);
    let req = stcore::service::ServiceSearchRequest {
        query: query.clone(),
        dir: dir.clone(),
        ext: ext.clone(),
        ..Default::default()
    };
    match client {
        Some(client) => match client.search(req) {
            Ok(_) => {
                cli_out!("已在主窗口呈现结果");
                return;
            }
            Err(e) => eprintln!("主实例未在运行（{e}），回退本地搜索"),
        },
        None => eprintln!("主实例未在运行（无端口文件），回退本地搜索"),
    }

    // 2) 本地 headless 回退
    let mgr = die(IndexManager::open(&index_dir), "打开索引失败");
    let filter = SearchFilter {
        dir,
        exts: ext.map(|e| vec![e]),
        sort: SortKey::Relevance,
    };
    let started = std::time::Instant::now();
    let hits = die(
        search(&mgr, &query, SearchMode::Fuzzy, false, 20, &filter),
        "搜索失败",
    );
    let elapsed = started.elapsed();
    cli_out!(
        "返回 {} 个结果，耗时 {:.3} 秒，搜索完成",
        hits.len(),
        elapsed.as_secs_f64()
    );
    for (i, h) in hits.iter().enumerate() {
        cli_out!("{:>3}. [{} 次命中] {}", i + 1, h.hits, h.path);
        if !h.snippet.is_empty() {
            cli_out!("      {}", h.snippet.replace('\n', " "));
        }
    }
}

fn run_cli(sub: CliSub) {
    use stcore::config::Config;
    use stcore::index::IndexManager;
    use stcore::search::{search, SearchMode};

    let resolve_index_dir = |override_dir: &Option<String>| {
        override_dir
            .clone()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(search_tool_lib::state::default_index_dir)
    };

    match sub {
        CliSub::Index { dir, index_dir } => {
            let index_dir = resolve_index_dir(&index_dir);
            cli_out!("索引目录: {} → {}", dir, index_dir.display());
            let stats = die(
                search_tool_lib::indexer::index_directory(
                    std::path::Path::new(&dir),
                    &index_dir,
                    |done, total| {
                        if done == total || done % 100 == 0 {
                            cli_out!("  进度: {}/{}", done, total);
                        }
                    },
                ),
                "索引失败",
            );
            cli_out!(
                "完成: 扫描 {} 个文件, 索引 {} 篇文档",
                stats.scanned,
                stats.indexed
            );
        }
        CliSub::Search {
            query,
            mode,
            reverse,
            limit,
            index_dir,
        } => {
            let index_dir = resolve_index_dir(&index_dir);
            let mode = match mode.as_str() {
                "fuzzy" => SearchMode::Fuzzy,
                "exact" => SearchMode::Exact,
                "dsl" => SearchMode::Dsl,
                other => {
                    eprintln!("未知模式: {}（可选 fuzzy/exact/dsl）", other);
                    std::process::exit(2);
                }
            };
            let mgr = die(IndexManager::open(&index_dir), "打开索引失败");
            let started = std::time::Instant::now();
            let filter = stcore::search::SearchFilter::default();
            let hits = die(
                search(&mgr, &query, mode, reverse, limit, &filter),
                "搜索失败",
            );
            let elapsed = started.elapsed();
            cli_out!(
                "返回 {} 个结果，耗时 {:.3} 秒，搜索完成",
                hits.len(),
                elapsed.as_secs_f64()
            );
            for (i, h) in hits.iter().enumerate() {
                cli_out!("{:>3}. [{} 次命中] {}", i + 1, h.hits, h.path);
                if h.snippets.len() > 1 {
                    // E-2（O4）：多命中文档——轮换输出全部片段（替代单 snippet 行）；
                    // 单命中/空 snippets 时走下面的分支，与旧版逐字节一致
                    for (k, s) in h.snippets.iter().enumerate() {
                        cli_out!("      [{}] {}", k + 1, s.replace('\n', " "));
                    }
                } else if !h.snippet.is_empty() {
                    cli_out!("      {}", h.snippet.replace('\n', " "));
                }
            }
        }
        CliSub::Stats { index_dir, detail } => {
            let index_dir = resolve_index_dir(&index_dir);
            let mgr = die(IndexManager::open(&index_dir), "打开索引失败");
            let size = dir_size(&index_dir);
            cli_out!("索引 {} 篇文档，磁盘占用 {}", mgr.num_docs(), human(size));
            if detail {
                // E-1（O5）：按扩展名构成明细——扩展名集合从 config.json 派生
                let cfg = Config::load(&index_dir).unwrap_or_default();
                let exts = stcore::indexer::supported_exts(&cfg);
                let comp = die(
                    stcore::search::index_composition(&mgr, &exts),
                    "构成统计失败",
                );
                if comp.by_ext.is_empty() {
                    cli_out!("  （空索引，无构成数据）");
                } else {
                    cli_out!("  构成：");
                    for s in &comp.by_ext {
                        let pct = if comp.docs == 0 {
                            0.0
                        } else {
                            s.docs as f64 / comp.docs as f64 * 100.0
                        };
                        let label = if s.ext.is_empty() {
                            "（无扩展名）"
                        } else {
                            s.ext.as_str()
                        };
                        cli_out!("    {:<12} {:>8} 篇 ({pct:.1}%)", label, s.docs);
                    }
                }
            }
        }
        CliSub::Testdata { index_dir } => {
            let index_dir = resolve_index_dir(&index_dir);
            let demo = std::env::temp_dir().join("search-tool-demo");
            let _ = std::fs::remove_dir_all(&demo);
            die(std::fs::create_dir_all(&demo), "创建演示目录失败");
            let samples = [
                ("中华人民共和国简史.txt", "中华人民共和国 成立于 一九四九年。人民 生活 水平 不断 提高，中华 文化 博大 精 深。"),
                ("财经新闻.txt", "人民币 汇率 保持 基本 稳定，外贸 进出口 总额 持续 增长。朝鲜 共和国 的 外交 代表团 访问 中国。"),
                ("生活随笔.md", "# 柴米油盐\n\n人民 的 生活 日常 离 不 开 柴 米 油 盐。\n\n中华烟 与 人民好 的 朝鲜共和国 见闻。"),
                ("科技动态.html", "<html><body><h1>量子计算</h1><p>人工 智能 大模型 引发 行业 变革，算力 需求 快速 增长。</p><script>var x=1;</script></body></html>"),
                ("邮件存档.eml", "From: a@b.c\nSubject: 会议纪要\nContent-Type: text/plain; charset=utf-8\n\n下周 讨论 数字 货币 试点 范围 扩大 事宜。"),
            ];
            for (name, body) in samples {
                die(std::fs::write(demo.join(name), body), "写入演示文件失败");
            }
            // 造一个 zip（内含 2 个文本）
            let zip_path = demo.join("资料打包.zip");
            {
                let f = die(std::fs::File::create(&zip_path), "创建 zip 失败");
                let mut z = zip::ZipWriter::new(f);
                let opts = zip::write::SimpleFileOptions::default();
                die(z.start_file("说明.txt", opts), "写 zip 条目失败");
                die(
                    std::io::Write::write_all(
                        &mut z,
                        "这是 压缩包 内 的 说明 文件：搜索 引擎 改变 了 人 们 获取 信息 的 方式。"
                            .as_bytes(),
                    ),
                    "写 zip 内容失败",
                );
                die(z.start_file("笔记.md", opts), "写 zip 条目失败");
                die(
                    std::io::Write::write_all(
                        &mut z,
                        "# 笔记\n\n区块 链 技术 应用 于 供应链 金融。".as_bytes(),
                    ),
                    "写 zip 内容失败",
                );
                die(z.finish(), "收尾 zip 失败");
            }
            cli_out!("演示语料: {}", demo.display());
            let stats = die(
                search_tool_lib::indexer::index_directory(&demo, &index_dir, |_, _| {}),
                "索引失败",
            );
            cli_out!(
                "完成: 扫描 {} 个文件, 索引 {} 篇文档（含压缩包内层）",
                stats.scanned,
                stats.indexed
            );
            cli_out!("\n试试: search-tool cli search 中华人民共和国 --mode fuzzy");
        }
        CliSub::Export {
            what,
            query,
            mode,
            limit,
            format,
            out,
            index_dir,
        } => {
            let index_dir = resolve_index_dir(&index_dir);
            die(
                run_export(
                    &index_dir,
                    &what,
                    query.as_deref(),
                    &mode,
                    limit,
                    &format,
                    out.as_deref(),
                ),
                "导出失败",
            );
        }
    }
}

/* ------------------------------ E-3（O3）：导出 ------------------------------ */

/// CSV 单元格转义 + 公式注入 sanitize。
///
/// sanitize（OWASP CSV Injection 惯例）：以 `=` `+` `-` `@` 制表符/回车开头的
/// 单元格前置单引号 `'`——防 Excel/WPS 打开导出文件时把 `=HYPERLINK(...)`
/// 之类内容当公式执行（国内用户 Excel/WPS 高频场景）。
/// 转义：含逗号/引号/换行时双引号包裹，内嵌引号翻倍。
fn csv_cell(s: &str) -> String {
    let needs_guard = s.starts_with(['=', '+', '-', '@', '\t', '\r']);
    let safe = if needs_guard {
        format!("'{s}")
    } else {
        s.to_string()
    };
    if safe.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", safe.replace('"', "\"\""))
    } else {
        safe
    }
}

/// 表头 + 行 → CSV 文本（调用方负责 BOM/写出）
fn rows_to_csv(header: &[&str], rows: &[Vec<String>]) -> String {
    let mut out = String::new();
    out.push_str(
        &header
            .iter()
            .map(|h| csv_cell(h))
            .collect::<Vec<_>>()
            .join(","),
    );
    out.push('\n');
    for r in rows {
        out.push_str(&r.iter().map(|c| csv_cell(c)).collect::<Vec<_>>().join(","));
        out.push('\n');
    }
    out
}

/// 统一写出：`out` 为 Some 时写文件（csv 带 UTF-8 BOM，Excel/WPS 直开不乱码），
/// None 时走 stdout（BrokenPipe 安全）。返回产出字节数（日志用）。
fn write_export(text: &str, out: Option<&str>, with_bom: bool) -> anyhow::Result<usize> {
    let body = if with_bom {
        format!("\u{feff}{text}")
    } else {
        text.to_string()
    };
    match out {
        Some(p) => {
            std::fs::write(p, body.as_bytes())?;
            Ok(body.len())
        }
        None => {
            use std::io::Write;
            let mut so = std::io::stdout().lock();
            so.write_all(body.as_bytes())?;
            Ok(body.len())
        }
    }
}

/// `cli export` 主流程（E-3 O3）：results / history / bookmarks × csv | jsonl。
///
/// 全只读：不开 IndexWriter、不动索引；history/bookmarks 走索引目录下
/// `search-tool.db`（与 GUI 同库）。
fn run_export(
    index_dir: &std::path::Path,
    what: &str,
    query: Option<&str>,
    mode: &str,
    limit: Option<usize>,
    format: &str,
    out: Option<&str>,
) -> anyhow::Result<()> {
    use stcore::index::IndexManager;
    let db = index_dir.join("search-tool.db");
    let text = match what {
        "results" => {
            let q = query.ok_or_else(|| anyhow::anyhow!("--what results 需要 --query <查询词>"))?;
            let mode = match mode {
                "fuzzy" => stcore::search::SearchMode::Fuzzy,
                "exact" => stcore::search::SearchMode::Exact,
                "dsl" => stcore::search::SearchMode::Dsl,
                other => anyhow::bail!("未知模式: {other}（可选 fuzzy/exact/dsl）"),
            };
            let cap = limit.unwrap_or(100);
            let mgr = IndexManager::open(index_dir)?;
            let hits = stcore::search::search(
                &mgr,
                q,
                mode,
                false,
                cap,
                &stcore::search::SearchFilter::default(),
            )?;
            match format {
                "csv" => {
                    let rows: Vec<Vec<String>> = hits
                        .iter()
                        .map(|h| {
                            vec![
                                h.path.clone(),
                                h.title.clone(),
                                h.hits.to_string(),
                                h.mtime.to_string(),
                                h.size.to_string(),
                            ]
                        })
                        .collect();
                    rows_to_csv(&["path", "title", "hits", "mtime", "size"], &rows)
                }
                "jsonl" => hits
                    .iter()
                    .map(|h| serde_json::to_string(h).unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join("\n"),
                other => anyhow::bail!("未知格式: {other}（可选 csv/jsonl）"),
            }
        }
        "history" => {
            use stcore::store::HistoryStore;
            let cap = limit.unwrap_or(500);
            let entries = HistoryStore::open(&db)?.list(cap)?;
            match format {
                "csv" => {
                    let rows: Vec<Vec<String>> = entries
                        .iter()
                        .map(|e| {
                            vec![
                                e.id.to_string(),
                                e.query.clone(),
                                e.mode.clone(),
                                e.result_count.to_string(),
                                e.ts.to_string(),
                            ]
                        })
                        .collect();
                    rows_to_csv(&["id", "query", "mode", "result_count", "ts"], &rows)
                }
                "jsonl" => entries
                    .iter()
                    .map(|e| serde_json::to_string(e).unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join("\n"),
                other => anyhow::bail!("未知格式: {other}（可选 csv/jsonl）"),
            }
        }
        "bookmarks" => {
            use stcore::store::BookmarkStore;
            let entries = BookmarkStore::open(&db)?.list()?;
            let entries: Vec<_> = match limit {
                Some(n) => entries.into_iter().take(n).collect(),
                None => entries,
            };
            match format {
                "csv" => {
                    let rows: Vec<Vec<String>> = entries
                        .iter()
                        .map(|b| {
                            vec![
                                b.id.to_string(),
                                b.path.clone(),
                                b.name.clone(),
                                b.created.to_string(),
                            ]
                        })
                        .collect();
                    rows_to_csv(&["id", "path", "name", "created"], &rows)
                }
                "jsonl" => entries
                    .iter()
                    .map(|b| serde_json::to_string(b).unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join("\n"),
                other => anyhow::bail!("未知格式: {other}（可选 csv/jsonl）"),
            }
        }
        other => anyhow::bail!("未知导出对象: {other}（可选 results/history/bookmarks）"),
    };
    let with_bom = format == "csv" && out.is_some();
    let n = write_export(&text, out, with_bom)?;
    if out.is_some() {
        cli_out!("已导出 {} 字节 → {}", n, out.unwrap_or("-"));
    }
    Ok(())
}

fn dir_size(p: &std::path::Path) -> u64 {
    std::fs::read_dir(p)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| {
                    let path = e.path();
                    if path.is_dir() {
                        dir_size(&path)
                    } else {
                        e.metadata().map(|m| m.len()).unwrap_or(0)
                    }
                })
                .sum()
        })
        .unwrap_or(0)
}

fn human(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{:.1}{}", size, UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn slash_mode_maps_flags() {
        let got = preprocess_slash_args(&v(&[
            "search-tool",
            "/s",
            "量子计算",
            "/d",
            "C",
            "/e",
            "txt",
        ]));
        assert_eq!(
            got,
            v(&[
                "search-tool",
                "--search",
                "量子计算",
                "--drive",
                "C",
                "--ext",
                "txt"
            ])
        );
    }

    #[test]
    fn slash_mode_case_insensitive() {
        let got = preprocess_slash_args(&v(&["search-tool", "/S", "kw", "/D", "C:/", "/E", "md"]));
        assert_eq!(
            got,
            v(&[
                "search-tool",
                "--search",
                "kw",
                "--drive",
                "C:/",
                "--ext",
                "md"
            ])
        );
    }

    #[test]
    fn slash_mode_keeps_non_flag_tokens() {
        // 查询词本身以 / 开头（路径）不被误映射
        let got = preprocess_slash_args(&v(&["search-tool", "/s", "/usr/share", "/e", "log"]));
        assert_eq!(
            got,
            v(&["search-tool", "--search", "/usr/share", "--ext", "log"])
        );
    }

    #[test]
    fn normal_subcommand_untouched() {
        // argv[1] 非 '/' 开头：整组参数原样（含组内 /s）
        let args = v(&["search-tool", "cli", "search", "/s"]);
        assert_eq!(preprocess_slash_args(&args), args);
    }

    #[test]
    fn single_arg_untouched() {
        let args = v(&["search-tool"]);
        assert_eq!(preprocess_slash_args(&args), args);
    }

    #[test]
    fn drive_prefix_normalization() {
        assert_eq!(drive_to_dir_prefix("C"), "C:/");
        assert_eq!(drive_to_dir_prefix("c:\\"), "c:");
        assert_eq!(drive_to_dir_prefix("C:/"), "C:");
        assert_eq!(drive_to_dir_prefix(" /d "), "/d");
        assert_eq!(drive_to_dir_prefix(""), "");
    }

    /* ------------------------------ E-3（O3）：导出 ------------------------------ */

    use std::sync::atomic::{AtomicU64, Ordering};

    static EXPORT_SEQ: AtomicU64 = AtomicU64::new(0);

    /// 自建唯一临时目录（沿用 files.rs 模式，不引 tempfile）
    fn export_tmp_dir(tag: &str) -> std::path::PathBuf {
        let n = EXPORT_SEQ.fetch_add(1, Ordering::Relaxed);
        let p =
            std::env::temp_dir().join(format!("st-export-test-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn csv_cell_sanitizes_injection() {
        // 公式注入：危险前缀前置单引号
        assert_eq!(csv_cell("=1+1"), "'=1+1");
        assert_eq!(csv_cell("+cmd"), "'+cmd");
        assert_eq!(csv_cell("-2"), "'-2");
        assert_eq!(csv_cell("@SUM"), "'@SUM");
        assert_eq!(csv_cell("\tTAB"), "'\tTAB");
        // 普通单元格不动
        assert_eq!(csv_cell("普通.txt"), "普通.txt");
        assert_eq!(csv_cell("/tmp/a.txt"), "/tmp/a.txt");
        // 引号/逗号包裹（内嵌引号翻倍）
        assert_eq!(csv_cell("a,b"), "\"a,b\"");
        assert_eq!(csv_cell("他说\"你好\""), "\"他说\"\"你好\"\"\"");
        // 组合：危险前缀 + 逗号 → 先加 guard 再包裹
        assert_eq!(csv_cell("=a,b"), "\"'=a,b\"");
    }

    /// 最小 CSV 行解析（测试专用：支持引号包裹与翻倍引号）
    fn parse_csv_line(line: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut in_q = false;
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            if in_q {
                if c == '"' {
                    if chars.peek() == Some(&'"') {
                        cur.push('"');
                        chars.next();
                    } else {
                        in_q = false;
                    }
                } else {
                    cur.push(c);
                }
            } else if c == '"' {
                in_q = true;
            } else if c == ',' {
                out.push(std::mem::take(&mut cur));
            } else {
                cur.push(c);
            }
        }
        out.push(cur);
        out
    }

    #[test]
    fn export_csv_roundtrip() {
        let header = ["path", "title", "hits", "mtime", "size"];
        let rows = vec![
            vec![
                "/tmp/a.txt".into(),
                "a.txt".into(),
                "3".into(),
                "1700000000".into(),
                "1024".into(),
            ],
            // 逗号/引号/公式注入混合单元格
            vec![
                "/tmp/b,逗号.txt".into(),
                "他说\"你好\"".into(),
                "=1+1".into(),
                "0".into(),
                "0".into(),
            ],
        ];
        let csv = rows_to_csv(&header, &rows);
        let mut lines = csv.lines();
        assert_eq!(parse_csv_line(lines.next().unwrap()), header);
        for (i, r) in rows.iter().enumerate() {
            let got = parse_csv_line(lines.next().unwrap());
            // sanitize 后 '=1+1' → '=1+1 前多一个单引号：回读应含 guard 符
            let expect: Vec<String> = r
                .iter()
                .map(|c| {
                    if c.starts_with(['=', '+', '-', '@', '\t', '\r']) {
                        format!("'{c}")
                    } else {
                        c.clone()
                    }
                })
                .collect();
            assert_eq!(got, expect, "第 {} 行 roundtrip 不一致", i + 1);
        }
    }

    #[test]
    fn export_history_cap() {
        use stcore::store::HistoryStore;
        let dir = export_tmp_dir("hist");
        let db = dir.join("search-tool.db");
        {
            let hs = HistoryStore::open(&db).unwrap();
            for i in 0..5 {
                hs.add(&format!("查询{i}"), "fuzzy", i).unwrap();
            }
        }
        // cap 生效：list(2) 只回 2 条（导出接线透传 limit）
        let entries = HistoryStore::open(&db).unwrap().list(2).unwrap();
        assert_eq!(entries.len(), 2, "--limit 应透传为 list cap");
        // 行构造口径：id/query/mode/result_count/ts 五列
        let rows: Vec<Vec<String>> = entries
            .iter()
            .map(|e| {
                vec![
                    e.id.to_string(),
                    e.query.clone(),
                    e.mode.clone(),
                    e.result_count.to_string(),
                    e.ts.to_string(),
                ]
            })
            .collect();
        assert!(rows.iter().all(|r| r.len() == 5));
        // csv 生成不 panic 且行数 = 1 表头 + 2 数据
        let csv = rows_to_csv(&["id", "query", "mode", "result_count", "ts"], &rows);
        assert_eq!(csv.lines().count(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_results_requires_query() {
        // --what results 缺 --query → 报错（不 panic）
        let dir = export_tmp_dir("noq");
        let r = run_export(&dir, "results", None, "fuzzy", None, "csv", None);
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("query"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_unknown_what_rejected() {
        let dir = export_tmp_dir("what");
        let r = run_export(&dir, "everything", None, "fuzzy", None, "csv", None);
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("未知导出对象"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
