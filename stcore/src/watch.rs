//! 文件监听 → 实时更新
//!
//! 递归监听索引根目录；变化事件去抖 250ms 后回调一次（串行、合并突发写入）。
//! 回调只负责「触发一轮索引」，互斥/进度/错误处理由调用方（app 层 index_admin）负责。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// 去抖窗口：静默满该时长后回调一次
const DEBOUNCE: Duration = Duration::from_millis(250);

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

/// 启动监听；`on_change` 在每次去抖窗口结束后被调用（同一时刻至多一次）。
///
/// 返回 `None` 表示底层监听器初始化失败（环境不支持等），调用方应仅记日志、不 panic。
pub fn start(roots: &[PathBuf], on_change: Arc<dyn Fn() + Send + Sync>) -> Option<RootWatcher> {
    let (tx, rx) = mpsc::channel::<()>();

    let watcher = match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        match res {
            Ok(ev) => {
                // 纯读取（打开/关闭/执行）不算变更，忽略
                if matches!(ev.kind, notify::EventKind::Access(_)) {
                    return;
                }
                let _ = tx.send(());
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
        // 每轮 recv_timeout 至多阻塞 DEBOUNCE，兼作停止标志的轮询粒度
        while !stop_thread.load(Ordering::Relaxed) {
            match rx.recv_timeout(DEBOUNCE) {
                Ok(()) => dirty = true,
                Err(RecvTimeoutError::Timeout) => {
                    if dirty {
                        dirty = false;
                        on_change();
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
}
