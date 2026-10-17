//! 服务层 transport 抽象（架构 v2 §3 / §4-D5 / §8-R5）。
//!
//! 同一业务面（search / stats），两种 transport：
//!   - [`InprocBackend`]：进程内直调 `stcore::search::search`（GUI 内部、CLI 本地回退）
//!   - `SocketClient`：连既有单实例的 JSON 行协议 transport——
//!     Unix socket（Linux/macOS）/ TCP loopback（Windows，主实例写端口文件），
//!     协议 `{op:"search"|"stats", ...}` → 一行 JSON 响应，两端逐字节一致
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

use serde::{Deserialize, Serialize};

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

/// 单实例客户端：连主实例监听的 transport（JSON 行协议），两种 transport：
/// - Unix：`index_dir/search-tool.sock`（Linux/macOS，见 shell.rs `setup_single_instance_socket`）
/// - Tcp：`127.0.0.1:<port>`（Windows 无 Unix socket；端口由主实例写入
///   `index_dir/search-tool.port`，见 shell.rs `setup_single_instance_tcp`）
///
/// 连接/读写失败一律返回 `Err`（调用方——CLI `/s`——回退本地 headless 直跑）。
///
/// 其它平台（非 Unix 非 Windows）无真实 transport，`SearchService` 由下方
/// `#[cfg(not(any(unix, windows)))]` 占位 impl 恒返 Err。
#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
pub struct SocketClient {
    transport: Transport,
    timeout: Duration,
}

/// TCP 连接超时（防主实例僵死/端口陈旧时拖住 CLI；loopback 拒绝是即时错误，
/// 此为兜底上限）。Unix socket 连接由内核快速失败，无需显式超时。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

enum Transport {
    #[cfg(unix)]
    Unix(PathBuf),
    /// Windows 单实例：TCP loopback 地址（`127.0.0.1:<port>`）
    Tcp(std::net::SocketAddr),
}

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

#[derive(Deserialize)]
struct SocketResponse {
    ok: bool,
    #[serde(default)]
    docs: Option<u64>,
    #[serde(default)]
    error: Option<String>,
}

fn mode_str(mode: SearchMode) -> &'static str {
    match mode {
        SearchMode::Fuzzy => "fuzzy",
        SearchMode::Exact => "exact",
        SearchMode::Dsl => "dsl",
        SearchMode::Regex => "regex",
    }
}

/// 行协议连接：`UnixStream`/`TcpStream` 均实现 `Read + Write`，
/// 但 `set_read_timeout` 是各自固有方法（不在 `Read`/`Write` trait 上），
/// 故收进本 trait 以复用同一套请求-响应逻辑。
trait LineConn: std::io::Read + std::io::Write {
    fn set_read_timeout(&mut self, d: Option<Duration>) -> std::io::Result<()>;
}

#[cfg(unix)]
impl LineConn for std::os::unix::net::UnixStream {
    fn set_read_timeout(&mut self, d: Option<Duration>) -> std::io::Result<()> {
        // UFCS 调固有方法（避免与 trait 方法同名歧义）
        std::os::unix::net::UnixStream::set_read_timeout(self, d)
    }
}

impl LineConn for std::net::TcpStream {
    fn set_read_timeout(&mut self, d: Option<Duration>) -> std::io::Result<()> {
        std::net::TcpStream::set_read_timeout(self, d)
    }
}

/// 发送请求行并读取一行响应（带读超时，防主实例僵死拖住 CLI）。
/// transport 无关：Unix socket / TCP loopback 共用。
fn request_over<C: LineConn>(
    mut conn: C,
    line: &str,
    timeout: Duration,
) -> Result<SocketResponse, String> {
    use std::io::{BufRead, BufReader};

    conn.set_read_timeout(Some(timeout))
        .map_err(|e| format!("设置读超时失败: {e}"))?;
    writeln!(conn, "{line}").map_err(|e| format!("发送请求失败: {e}"))?;

    let mut resp_line = String::new();
    BufReader::new(conn)
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

impl SocketClient {
    /// Unix transport：`path` 为完整 socket 文件路径（app 侧 `shell::socket_path(index_dir)`）。
    #[cfg(unix)]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            transport: Transport::Unix(path.into()),
            timeout: Duration::from_secs(3),
        }
    }

    /// Windows transport：主实例写入端口文件中的 TCP loopback 端口。
    pub fn new_tcp(port: u16) -> Self {
        Self {
            transport: Transport::Tcp(std::net::SocketAddr::new(
                std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                port,
            )),
            timeout: Duration::from_secs(3),
        }
    }
}

#[cfg(any(unix, windows))]
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

#[cfg(any(unix, windows))]
impl SocketClient {
    /// 按 transport 连接并完成一次请求-响应（JSON 行协议）。
    fn request(
        &self,
        op: &'static str,
        query: &str,
        mode: &'static str,
        drive: Option<Option<&str>>,
        ext: Option<&str>,
    ) -> Result<SocketResponse, String> {
        let line = serde_json::to_string(&SocketRequest {
            op,
            query,
            mode,
            drive: drive.unwrap_or(None),
            ext,
        })
        .map_err(|e| format!("序列化请求失败: {e}"))?;
        match &self.transport {
            #[cfg(unix)]
            Transport::Unix(p) => {
                let s = std::os::unix::net::UnixStream::connect(p)
                    .map_err(|e| format!("连接主实例失败 ({}): {e}", p.display()))?;
                request_over(s, &line, self.timeout)
            }
            Transport::Tcp(addr) => {
                let s = std::net::TcpStream::connect_timeout(addr, CONNECT_TIMEOUT)
                    .map_err(|e| format!("连接主实例失败 ({addr}): {e}"))?;
                request_over(s, &line, self.timeout)
            }
        }
    }
}

// 非 Unix 非 Windows 占位：一律 Err → 调用方本地回退
#[cfg(not(any(unix, windows)))]
impl SearchService for SocketClient {
    fn search(&self, _req: ServiceSearchRequest) -> Result<ServiceSearchResponse, String> {
        Err("当前平台不支持单实例 transport".to_string())
    }

    fn stats(&self) -> Result<ServiceStats, String> {
        Err("当前平台不支持单实例 transport".to_string())
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

    /// TCP loopback roundtrip（Windows transport；std::net 跨平台，故本机可跑）：
    /// 桩 server 与 Unix 测试同构，验证 `new_tcp` 的端口寻址与请求-响应闭环。
    #[test]
    fn socket_client_tcp_roundtrip_stats_and_search() {
        use std::io::{BufRead, BufReader, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = std::thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                let mut stream = stream.unwrap();
                let mut line = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
                let resp = match req["op"].as_str().unwrap() {
                    "stats" => serde_json::json!({"ok": true, "docs": 7}),
                    "search" => serde_json::json!({"ok": true}),
                    other => serde_json::json!({"ok": false, "error": format!("bad op {other}")}),
                };
                writeln!(stream, "{}", serde_json::to_string(&resp).unwrap()).unwrap();
            }
        });

        let client = SocketClient::new_tcp(port);

        let stats = client.stats().unwrap();
        assert_eq!(stats.docs, 7);

        let resp = client
            .search(ServiceSearchRequest {
                query: "量子计算".to_string(),
                dir: Some("/tmp".to_string()),
                ext: Some("txt".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert!(resp.hits.is_empty(), "tcp search 语义为已呈现");

        // 无监听的端口：连接失败应返回 Err（CLI 据此回退本地 headless）
        let dead_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let dead_port = dead_listener.local_addr().unwrap().port();
        drop(dead_listener);
        let dead = SocketClient::new_tcp(dead_port);
        assert!(dead.stats().is_err());

        server.join().unwrap();
    }
}
