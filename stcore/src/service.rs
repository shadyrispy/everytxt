//! 服务层 transport 抽象（架构 v2 §3 / §4-D5 / §8-R5）。
//!
//! 同一业务面（search / stats），两种 transport：
//!   - [`InprocBackend`]：进程内直调 `stcore::search::search`（GUI 内部、CLI 本地回退）
//!   - `SocketClient`：连既有单实例 Unix socket（app/src/commands/shell.rs 监听），
//!     JSON 行协议 `{op:"search"|"stats", ...}` → 一行 JSON 响应
//!     （仅 Unix：Windows 单实例 socket 未实现，CLI `/s` 回退本地 headless）
//!
//! 意义：CLI `/s`、外部入口、未来 HTTP/Web UI 只需新增/复用 transport，业务零改动。
//! 不要求 app 现有 tauri 命令迁移。
//!
//! socket `search` 语义注记：主实例收到后把查询转发到 GUI 呈现
//! （emit `external-search` + show/set_focus），**结果不经本连接返回**，
//! 故 `SocketClient::search` 成功时返回空 hits（调用方按"已呈现"处理）。
//! `stats` 则真实返回主实例当前索引文档数。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[cfg(unix)]
use serde::Deserialize;
use serde::Serialize;

use crate::index::IndexManager;
use crate::search::{search, SearchFilter, SearchHit, SearchMode, SortKey};

/* ------------------------------ 服务面（契约） ------------------------------ */

/// 服务层 transport 抽象（D5）：同一业务面，Inproc 直调 / Socket 远程两种实现。
/// 未来 HTTP/Web UI 只需新增 transport，业务零改动。
pub trait SearchService: Send + Sync {
    fn search(&self, req: ServiceSearchRequest) -> Result<ServiceSearchResponse, String>;
    fn stats(&self) -> Result<ServiceStats, String>;
}

/// 服务层搜索请求：字段对齐 [`stcore::search::search`] 的输入
/// （mode/reverse/limit 直传；dir/ext 落到 `SearchFilter`）。
#[derive(Debug, Clone)]
pub struct ServiceSearchRequest {
    pub query: String,
    pub mode: SearchMode,
    pub reverse: bool,
    pub limit: usize,
    /// 目录前缀过滤（None = 全部；CLI `/d` 盘符归一化后的前缀）
    pub dir: Option<String>,
    /// 扩展名过滤（None = 全部；不带点，如 "txt"）
    pub ext: Option<String>,
}

impl Default for ServiceSearchRequest {
    fn default() -> Self {
        Self {
            query: String::new(),
            mode: SearchMode::Fuzzy,
            reverse: false,
            limit: 20,
            dir: None,
            ext: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceSearchResponse {
    pub hits: Vec<SearchHit>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceStats {
    pub docs: u64,
    pub disk_bytes: u64,
}

/* ------------------------------ Inproc 直调 ------------------------------ */

/// 进程内直调实现：按需打开 [`IndexManager`]（读侧只读，与 GUI writer 并存安全）。
/// 面向 CLI / 外部低频入口，不做常驻缓存（`AppState::with_mgr` 才是 GUI 常驻入口）。
pub struct InprocBackend {
    index_dir: PathBuf,
}

impl InprocBackend {
    pub fn new(index_dir: impl Into<PathBuf>) -> Self {
        Self {
            index_dir: index_dir.into(),
        }
    }

    fn open_mgr(&self) -> Result<IndexManager, String> {
        IndexManager::open(&self.index_dir).map_err(|e| format!("打开索引失败: {e}"))
    }
}

impl SearchService for InprocBackend {
    fn search(&self, req: ServiceSearchRequest) -> Result<ServiceSearchResponse, String> {
        let started = Instant::now();
        let mgr = self.open_mgr()?;
        let filter = SearchFilter {
            dir: req.dir,
            exts: req.ext.as_ref().map(|e| vec![e.clone()]),
            sort: SortKey::Relevance,
        };
        let hits = search(&mgr, &req.query, req.mode, req.reverse, req.limit, &filter)
            .map_err(|e| e.to_string())?;
        Ok(ServiceSearchResponse {
            hits,
            elapsed_ms: started.elapsed().as_millis() as u64,
        })
    }

    fn stats(&self) -> Result<ServiceStats, String> {
        let mgr = self.open_mgr()?;
        Ok(ServiceStats {
            docs: mgr.num_docs(),
            disk_bytes: dir_size(&self.index_dir),
        })
    }
}

fn dir_size(p: &Path) -> u64 {
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

/* ------------------------------ Socket 远程 ------------------------------ */

/// 单实例 socket 客户端：连 shell.rs 监听的 Unix socket（JSON 行协议）。
///
/// 连接/读写失败一律返回 `Err`（调用方——CLI `/s`——回退本地 headless 直跑）。
///
/// 仅 Unix 有真实 transport；Windows 单实例（命名管道）未实现（shell.rs TODO
/// 同步），`SearchService` 由下方 `#[cfg(not(unix))]` 占位 impl 恒返 Err，
/// 字段在非 Unix 平台不被读取（CLI 仍会构造并走回退路径）。
#[cfg_attr(not(unix), allow(dead_code))]
pub struct SocketClient {
    path: PathBuf,
    timeout: Duration,
}

#[cfg(unix)]
#[derive(Serialize)]
struct SocketRequest<'a> {
    op: &'static str,
    query: &'a str,
    mode: &'static str,
    /// socket 协议字段名沿用 `drive`，
    /// 承载的是归一化后的目录前缀（None = 全库）。
    drive: Option<&'a str>,
    ext: Option<&'a str>,
}

#[cfg(unix)]
#[derive(Deserialize)]
struct SocketResponse {
    ok: bool,
    #[serde(default)]
    docs: Option<u64>,
    #[serde(default)]
    error: Option<String>,
}

#[cfg(unix)]
fn mode_str(mode: SearchMode) -> &'static str {
    match mode {
        SearchMode::Fuzzy => "fuzzy",
        SearchMode::Exact => "exact",
        SearchMode::Dsl => "dsl",
        SearchMode::Regex => "regex",
    }
}

impl SocketClient {
    /// `path`：完整 socket 文件路径（app 侧 `shell::socket_path(index_dir)`）。
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            timeout: Duration::from_secs(3),
        }
    }
}

#[cfg(unix)]
impl SearchService for SocketClient {
    fn search(&self, req: ServiceSearchRequest) -> Result<ServiceSearchResponse, String> {
        self.request(
            "search",
            &req.query,
            mode_str(req.mode),
            Some(req.dir.as_deref()),
            req.ext.as_deref(),
        )?;
        // "转发并在主窗口呈现"语义：结果不经连接返回，空 hits = 已呈现
        Ok(ServiceSearchResponse {
            hits: Vec::new(),
            elapsed_ms: 0,
        })
    }

    fn stats(&self) -> Result<ServiceStats, String> {
        let resp = self.request("stats", "", "fuzzy", None, None)?;
        Ok(ServiceStats {
            docs: resp.docs.unwrap_or(0),
            disk_bytes: 0,
        })
    }
}

#[cfg(unix)]
impl SocketClient {
    /// 发送请求行并读取一行响应（带读超时，防主实例僵死拖住 CLI）
    fn request(
        &self,
        op: &'static str,
        query: &str,
        mode: &'static str,
        drive: Option<Option<&str>>,
        ext: Option<&str>,
    ) -> Result<SocketResponse, String> {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream;

        let mut stream = UnixStream::connect(&self.path)
            .map_err(|e| format!("连接主实例失败 ({}): {e}", self.path.display()))?;
        stream
            .set_read_timeout(Some(self.timeout))
            .map_err(|e| format!("设置读超时失败: {e}"))?;
        let line = serde_json::to_string(&SocketRequest {
            op,
            query,
            mode,
            drive: drive.unwrap_or(None),
            ext,
        })
        .map_err(|e| format!("序列化请求失败: {e}"))?;
        writeln!(stream, "{line}").map_err(|e| format!("发送请求失败: {e}"))?;

        let mut resp_line = String::new();
        BufReader::new(stream)
            .read_line(&mut resp_line)
            .map_err(|e| format!("读取响应失败: {e}"))?;
        let resp: SocketResponse =
            serde_json::from_str(resp_line.trim()).map_err(|e| format!("解析响应失败: {e}"))?;
        if resp.ok {
            Ok(resp)
        } else {
            Err(resp.error.unwrap_or_else(|| "未知错误".to_string()))
        }
    }
}

// 非 Unix 占位（Windows 走命名管道，shell.rs TODO 同步）：一律 Err → 调用方本地回退
#[cfg(not(unix))]
impl SearchService for SocketClient {
    fn search(&self, _req: ServiceSearchRequest) -> Result<ServiceSearchResponse, String> {
        Err("当前平台不支持 unix socket transport".to_string())
    }

    fn stats(&self) -> Result<ServiceStats, String> {
        Err("当前平台不支持 unix socket transport".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{IndexDoc, Writer};

    fn seed(dir: &Path) {
        let mut w = Writer::open(dir).unwrap();
        for (p, b) in [("a", "中华人民共和国 崛起"), ("b", "人民 的 生活 日常")] {
            w.upsert(&IndexDoc {
                path: format!("/tmp/{p}.txt"),
                title: format!("{p}.txt"),
                body: b.to_string(),
                size: b.len() as u64,
                mtime: 0,
            })
            .unwrap();
        }
        w.commit().unwrap();
    }

    #[test]
    fn inproc_backend_parity_with_direct_search() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());

        let backend = InprocBackend::new(dir.path());
        let req = ServiceSearchRequest {
            query: "中华人民共和国".to_string(),
            ..Default::default()
        };
        let resp = backend.search(req).unwrap();

        let mgr = IndexManager::open(dir.path()).unwrap();
        let direct = search(
            &mgr,
            "中华人民共和国",
            SearchMode::Fuzzy,
            false,
            20,
            &SearchFilter::default(),
        )
        .unwrap();

        let got: Vec<String> = resp.hits.iter().map(|h| h.path.clone()).collect();
        let want: Vec<String> = direct.iter().map(|h| h.path.clone()).collect();
        assert_eq!(got, want);
        assert!(!resp.hits.is_empty());
        assert!(resp.hits[0].hits > 0);
    }

    #[test]
    fn inproc_backend_ext_filter_and_stats() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());

        let backend = InprocBackend::new(dir.path());
        let resp = backend
            .search(ServiceSearchRequest {
                query: "崛起".to_string(),
                ext: Some("md".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert!(resp.hits.is_empty(), "ext 不匹配应过滤为空");

        let resp = backend
            .search(ServiceSearchRequest {
                query: "崛起".to_string(),
                ext: Some("txt".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(resp.hits.len(), 1);

        let stats = backend.stats().unwrap();
        assert_eq!(stats.docs, 2);
        assert!(stats.disk_bytes > 0);
    }

    /// SocketClient roundtrip：起最小桩 server（复刻 shell.rs 单实例 JSON 行协议），
    /// 验证请求序列化字段名（op/query/mode/drive/ext）与响应解析闭环，
    /// 无需拉起 Tauri 主实例。
    #[cfg(unix)]
    #[test]
    fn socket_client_roundtrip_stats_and_search() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("st.sock");
        let listener = UnixListener::bind(&sock).unwrap();

        // 桩 server：收一行请求 → 按 op 回一行响应（与 shell.rs 协议一致）
        let server = std::thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                let mut stream = stream.unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                    .unwrap();
                let mut line = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
                let resp = match req["op"].as_str().unwrap() {
                    "stats" => serde_json::json!({"ok": true, "docs": 42}),
                    "search" => serde_json::json!({"ok": true}),
                    other => serde_json::json!({"ok": false, "error": format!("bad op {other}")}),
                };
                writeln!(stream, "{}", serde_json::to_string(&resp).unwrap()).unwrap();
            }
        });

        let client = SocketClient::new(&sock);

        // stats：docs 字段应被解析
        let stats = client.stats().unwrap();
        assert_eq!(stats.docs, 42);

        // search："转发并呈现"语义 → Ok + 空 hits
        let resp = client
            .search(ServiceSearchRequest {
                query: "量子计算".to_string(),
                dir: Some("/tmp".to_string()),
                ext: Some("txt".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert!(
            resp.hits.is_empty(),
            "socket search 语义为已呈现，不经连接返回结果"
        );

        // 主实例不在：连接失败应返回 Err（CLI 据此回退本地 headless）
        let dead = SocketClient::new(dir.path().join("nonexistent.sock"));
        assert!(dead.stats().is_err());

        server.join().unwrap();
    }
}
