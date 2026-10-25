//! 文件监听 → 实时更新
//!
//! 递归监听索引根目录；变化事件去抖 250ms 后回调一次（串行、合并突发写入）。
//! 回调只负责「触发一轮索引」，互斥/进度/错误处理由调用方（app 层 index_admin）负责。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// 去抖窗口：静默满该时长后回调一次
const DEBOUNCE: Duration = Duration::from_millis(250);

/// 聚合上限（R6）：窗口内去重 paths 超过该数 → 丢弃明细，回调给空 Vec
/// （root 桶信号，app 层回落该 root 全量 `sync_root`）。
///
/// git checkout / 解压数万文件场景防 `Vec<PathBuf>` 内存尖峰，且此时全量
/// sync 本就比逐文件判定划算。
pub const MAX_PATHS_PER_BURST: usize = 10_000;

/// 监听句柄：watcher + 已监听集合 + 停止标志 + 后台去抖线程。
///
/// 线程安全（`Send`），可存放于 Tauri `State` / 全局 `OnceLock`。
pub struct RootWatcher {
    watcher: Mutex<RecommendedWatcher>,
    /// 当前已 watch 的路径（update_roots 先全部 unwatch 再 watch）
    watched: Mutex<Vec<PathBuf>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

/// 启动监听；`on_change` 在每次去抖窗口结束后被调用（同一时刻至多一次），
/// 参数为窗口内聚合的变更路径（已去重）。
///
/// 超过 [`MAX_PATHS_PER_BURST`] 时丢弃明细、传空 Vec——调用方按 root 桶
/// 信号回落全量 `sync_root`（见 F-3）。per-root 分组由 app 层完成。
///
/// 返回 `None` 表示底层监听器初始化失败（环境不支持等），调用方应仅记日志、不 panic。
pub fn start_with_paths(
    roots: &[PathBuf],
    on_change: Arc<dyn Fn(Vec<PathBuf>) + Send + Sync>,
) -> Option<RootWatcher> {
    let (tx, rx) = mpsc::channel::<Vec<PathBuf>>();

    let watcher = match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        match res {
            Ok(ev) => {
                // 纯读取（打开/关闭/执行）不算变更，忽略
                if matches!(ev.kind, notify::EventKind::Access(_)) {
                    return;
                }
                let _ = tx.send(ev.paths);
            }
            Err(e) => eprintln!("文件监听事件错误: {e}"),
        }
    }) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("文件监听初始化失败: {e}");
            return None;
        }
    };

    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        let mut dirty = false;
        let mut paths: HashSet<PathBuf> = HashSet::new();
        // 每轮 recv_timeout 至多阻塞 DEBOUNCE，兼作停止标志的轮询粒度
        while !stop_thread.load(Ordering::Relaxed) {
            match rx.recv_timeout(DEBOUNCE) {
                Ok(ev_paths) => {
                    dirty = true;
                    paths.extend(ev_paths);
                }
                Err(RecvTimeoutError::Timeout) => {
                    if dirty {
                        dirty = false;
                        on_change(burst_payload(paths));
                        paths = HashSet::new();
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    let rw = RootWatcher {
        watcher: Mutex::new(watcher),
        watched: Mutex::new(Vec::new()),
        stop,
        handle: Some(handle),
    };
    rw.update_roots(roots);
    Some(rw)
}

/// 窗口聚合结果 → 回调 payload。
///
/// 超上限丢弃明细（空 Vec = root 桶信号）；未超则原样给出去重后的 paths。
/// 独立成函数便于测试锁定上限语义（R6 门禁）。
fn burst_payload(paths: HashSet<PathBuf>) -> Vec<PathBuf> {
    if paths.len() > MAX_PATHS_PER_BURST {
        Vec::new()
    } else {
        paths.into_iter().collect()
    }
}

/// 旧签名薄封装（存量调用方零改动）：内部丢 paths 调 [`start_with_paths`]。
///
/// 返回 `None` 的语义同 `start_with_paths`。
pub fn start(roots: &[PathBuf], on_change: Arc<dyn Fn() + Send + Sync>) -> Option<RootWatcher> {
    start_with_paths(roots, Arc::new(move |_paths| on_change()))
}

impl RootWatcher {
    /// 热切换监听根：先全部 unwatch，再按新集合 watch。
    ///
    /// 不存在或监听失败的路径仅打日志跳过（不 panic）。
    pub fn update_roots(&self, roots: &[PathBuf]) {
        let mut w = self.watcher.lock().unwrap_or_else(|e| e.into_inner());
        let mut watched = self.watched.lock().unwrap_or_else(|e| e.into_inner());

        for p in watched.drain(..) {
            let _ = w.unwatch(&p);
        }
        for r in roots {
            if !r.exists() {
                eprintln!("跳过监听不存在的目录: {}", r.display());
                continue;
            }
            match w.watch(r, RecursiveMode::Recursive) {
                Ok(()) => watched.push(r.clone()),
                Err(e) => eprintln!("监听目录失败 {}: {e}", r.display()),
            }
        }
    }
}

impl Drop for RootWatcher {
    fn drop(&mut self) {
        // 置停止标志 → 后台线程在 <= DEBOUNCE 内自然退出
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    /// 纯逻辑（不依赖 OS 事件）：不存在根目录可启动、update_roots 切换不 panic。
    #[test]
    fn start_and_update_missing_roots_is_safe() {
        let missing = std::env::temp_dir().join("stcore-watch-nonexistent-xyz");
        let cb: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let w = start(std::slice::from_ref(&missing), cb).expect("watcher 应可启动");
        w.update_roots(&[std::env::temp_dir()]);
        w.update_roots(&[missing]);
        drop(w);
    }

    /// 集成：写文件后在 3s 内触发去抖回调。
    ///
    /// 依赖 inotify；沙箱内不可用时会不稳定 → 见报告结论（必要时标 `#[ignore]`）。
    #[test]
    fn watch_fires_on_file_create() {
        let dir = tempfile::tempdir().unwrap();
        let fired = Arc::new(AtomicBool::new(false));
        let f = Arc::clone(&fired);
        let w = start(
            &[dir.path().to_path_buf()],
            Arc::new(move || f.store(true, Ordering::SeqCst)),
        )
        .expect("watcher 应启动");

        // 留时间让 inotify 完成 watch 注册
        thread::sleep(Duration::from_millis(200));
        std::fs::write(dir.path().join("x.txt"), b"hi").unwrap();

        let mut ok = false;
        for _ in 0..30 {
            if fired.load(Ordering::SeqCst) {
                ok = true;
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        drop(w);
        assert!(ok, "创建文件后应在 3s 内触发回调");
    }

    /// R6 门禁：聚合上限锁定——等于上限透传，超上限丢弃明细（root 桶信号）。
    #[test]
    fn burst_payload_caps_at_limit() {
        let at_limit: HashSet<PathBuf> = (0..MAX_PATHS_PER_BURST)
            .map(|i| PathBuf::from(format!("/r/f{i}")))
            .collect();
        assert_eq!(at_limit.len(), MAX_PATHS_PER_BURST);
        assert_eq!(
            burst_payload(at_limit).len(),
            MAX_PATHS_PER_BURST,
            "等于上限应原样透传"
        );

        let over_limit: HashSet<PathBuf> = (0..=MAX_PATHS_PER_BURST)
            .map(|i| PathBuf::from(format!("/r/f{i}")))
            .collect();
        assert_eq!(over_limit.len(), MAX_PATHS_PER_BURST + 1);
        assert!(
            burst_payload(over_limit).is_empty(),
            "超上限应丢弃明细（空 Vec = root 桶信号）"
        );
    }

    /// 集成：`start_with_paths` 回调携带窗口内聚合的变更 paths。
    ///
    /// 依赖 inotify（沙箱不可用时同 `watch_fires_on_file_create` 处理）。
    /// 容错窗口切分：累积多次 burst 的并集再断言。
    #[test]
    fn start_with_paths_fires_with_aggregated_paths() {
        let dir = tempfile::tempdir().unwrap();
        let bursts: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));
        let b = Arc::clone(&bursts);
        let w = start_with_paths(
            &[dir.path().to_path_buf()],
            Arc::new(move |paths| b.lock().unwrap().extend(paths)),
        )
        .expect("watcher 应启动");

        // 留时间让 inotify 完成 watch 注册
        thread::sleep(Duration::from_millis(200));
        // 突发写 3 个文件（同一去抖窗口内聚合）
        for n in ["x1.txt", "x2.txt", "x3.txt"] {
            std::fs::write(dir.path().join(n), b"hi").unwrap();
        }

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if bursts.lock().unwrap().len() >= 3 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "3s 内应收到覆盖 3 个文件的聚合 paths"
            );
            thread::sleep(Duration::from_millis(100));
        }
        drop(w);
        let got = bursts.lock().unwrap();
        for n in ["x1.txt", "x2.txt", "x3.txt"] {
            assert!(
                got.iter().any(|p| p.ends_with(n)),
                "聚合 paths 应含 {n}: {got:?}"
            );
        }
    }

    /// 集成（O2 核心场景）：**保存已存在文件**（编辑器 Ctrl+S）也触发
    /// 带 paths 的回退——inotify Modify 事件路径正确透传，F-3 据此分桶
    /// 增量同步。既有 create 测试不覆盖此路径。
    #[test]
    fn start_with_paths_fires_on_modify() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("doc.txt");
        std::fs::write(&target, b"v1").unwrap();

        let bursts: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));
        let b = Arc::clone(&bursts);
        let w = start_with_paths(
            &[dir.path().to_path_buf()],
            Arc::new(move |paths| b.lock().unwrap().extend(paths)),
        )
        .expect("watcher 应启动");

        // 留时间让 inotify 完成 watch 注册（含已存在文件的监听）
        thread::sleep(Duration::from_millis(200));
        std::fs::write(&target, b"v2 content").unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if bursts
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.ends_with("doc.txt"))
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "3s 内应收到保存事件的 paths"
            );
            thread::sleep(Duration::from_millis(100));
        }
        drop(w);
    }
}
