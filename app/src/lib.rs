//! search-tool lib 入口（Tauri GUI 模式）
//!
//! 注意：headless CLI 走 app/src/main.rs 的 cli 子命令，不经此模块。
//!
//! P0+ 五子项并行开发的文件所有权边界见 commands/mod.rs 头注释；
//! UI 布局规格见 docs/ui-redesign.md。
//!
//! 架构 v2 R2：scanner 引擎迁入 `stcore::indexer`（此处 re-export 供 CLI 复用）；
//! 索引任务统一走 [`indexer_queue`]（单工作线程串行 = writer 单实例）。

pub mod commands;
pub mod indexer_queue;
pub mod state;

/// CLI（main.rs）经此使用统一索引引擎（原 `search_tool_lib::scanner` 路径迁移）
pub use stcore::indexer;

/// 启动 Tauri GUI
///
/// `open_dir`：由 CLI `--open-dir <dir>` 传入（右击搜索入口），
/// 前端首次拉取后作为初始目录过滤。
pub fn run(open_dir: Option<String>) {
    use tauri::Manager;

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        // 子项 5：应用外壳插件（全局热键 / 开机启动）
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(move |app| {
            // R2：先起索引任务队列（须早于任何 submit）
            indexer_queue::init(app.handle().clone());

            let index_dir = state::default_index_dir();
            std::fs::create_dir_all(&index_dir)?;
            let st = state::AppState::new(index_dir);
            if let Some(d) = open_dir.clone() {
                st.set_pending_open_dir(Some(d));
            }
            app.manage(st);

            // 子项 5.3：系统托盘（显示/隐藏/退出）+ 关闭到托盘行为
            if let Err(e) = commands::shell::setup_tray(app.handle()) {
                eprintln!("系统托盘初始化失败（不影响 GUI）: {e}");
            }
            commands::shell::setup_close_to_tray(app.handle());

            // 子项 5.1：从配置恢复全局热键（唤醒主窗口）
            commands::shell::register_hotkeys_from_config(app.handle());

            // 子项 3.4：按已存配置拉起定时更新调度（间隔/定点/从不 三态）
            commands::index_admin::restart_scheduler(app.handle());

            //实时更新（文件监听 → 去抖后提交队列任务）
            commands::index_admin::restart_watcher(app.handle());

            // 子项 5.6：CLI 单实例通信（Unix socket 监听，查询转发给本主实例）
            #[cfg(unix)]
            {
                if let Err(e) = commands::shell::setup_single_instance_socket(app.handle()) {
                    eprintln!("单实例 socket 监听启动失败（不影响 GUI）: {e}");
                }
            }

            Ok(())
        })
        // 汇总登记全部 invoke 命令（新增命令在 commands::handler() 内追加）
        .invoke_handler(commands::handler())
        .run(tauri::generate_context!())
        .expect("启动 GUI 失败");
}

/// 测试用临时目录（scanner 迁入 stcore 后的 app 侧替身；原 `scanner::testutil`）
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
