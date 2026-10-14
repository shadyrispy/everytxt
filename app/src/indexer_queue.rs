//! 索引任务队列（架构 v2 R2-D4：任务统一）
//!
//! 单一工作线程串行消费 → 同一时刻至多一个索引任务在跑（天然串行 = writer 单实例），
//! watcher / scheduler / 手动索引三入口统一经此提交。
//!
//! 实现约束：只用 std（`std::sync::mpsc`），不引新依赖；
//! oneshot 用 std mpsc 双通道模拟（`submit_and_wait`）。

use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;

use tauri::AppHandle;

use crate::commands::index_admin::ScanReport;

/// 索引任务（统一三入口：手动目录索引 / 定时与 watch 全根刷新 / 重建）
pub enum IndexTask {
    /// GUI「索引目录」：登记根目录 + 按类型规则全量索引（带回执）
    Directory {
        root: String,
        file_type_id: Option<i64>,
        reply: Option<Sender<Result<ScanReport, String>>>,
    },
    /// 定时 / watch 触发：全部已索引根目录幂等刷新（无回执，不等结果）
    Roots(Vec<PathBuf>),
    /// 清空 + 遍历全部根目录重建（带回执）
    Rebuild {
        reply: Option<Sender<Result<(), String>>>,
    },
}

struct IndexQueue {
    tx: Sender<IndexTask>,
}

static QUEUE: OnceLock<IndexQueue> = OnceLock::new();

/// 初始化队列并 spawn 工作线程（GUI setup 首行调用，必须早于任何 submit）。
///
/// 工作线程持有 AppHandle：任务执行需要 emit 进度事件 / 读配置 / 重启调度与监听。
pub fn init(app: AppHandle) {
    let _ = QUEUE.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<IndexTask>();
        std::thread::Builder::new()
            .name("indexer-queue".into())
            .spawn(move || {
                while let Ok(task) = rx.recv() {
                    match task {
                        IndexTask::Directory {
                            root,
                            file_type_id,
                            reply,
                        } => {
                            let r = crate::commands::index_admin::index_directory_job(
                                &app,
                                &root,
                                file_type_id,
                            );
                            if let Some(s) = reply {
                                let _ = s.send(r);
                            }
                        }
                        IndexTask::Roots(roots) => {
                            crate::commands::index_admin::run_roots_job(&app, &roots);
                        }
                        IndexTask::Rebuild { reply } => {
                            let r = crate::commands::index_admin::rebuild_job(&app);
                            if let Some(s) = reply {
                                let _ = s.send(r);
                            }
                        }
                    }
                }
            })
            .expect("索引队列线程启动失败");
        IndexQueue { tx }
    });
}

/// 提交任务（不等待；`init` 未调用即 panic——命令层只可能运行于 GUI 进程）
pub fn submit(task: IndexTask) {
    let _ = QUEUE
        .get()
        .expect("索引队列未初始化（应在 GUI setup 中先调用 init）")
        .tx
        .send(task);
}

/// 提交并阻塞等待回执（oneshot 经 std mpsc 模拟）。
///
/// 调用方须处于 `#[tauri::command(async)]` 的线程池上下文（不阻塞 UI 主线程）。
pub fn submit_and_wait<T>(
    make: impl FnOnce(Sender<Result<T, String>>) -> IndexTask,
) -> Result<T, String> {
    let (tx, rx) = mpsc::channel();
    submit(make(tx));
    rx.recv().map_err(|_| "索引队列已关闭".to_string())?
}
