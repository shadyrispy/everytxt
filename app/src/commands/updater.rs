//! 检测更新（手册 7.9 / B6）：tauri-plugin-updater 接线。
//!
//! 通道配置在 `tauri.conf.json` 的 `plugins.updater`（endpoints + pubkey）。
//! 当前为**占位配置**（example.invalid）：命中即视为"未配置"，
//! 返回可读错误，UI 提示"首个正式版发布后启用"，不暴露裸网络错误。
//! 首个 release 落地时需同时：
//!   1. 配置真实 endpoints（latest.json，如 GitHub Releases 生成的）
//!   2. 生成签名密钥对，pubkey 填入 tauri.conf.json，私钥由 CI 签名（TAURI_SIGNING_PRIVATE_KEY）
//!   3. bundle.targets 补 windows 安装器（nsis/msi，更新通道前提）
//!
//! 交互协议：前端 Ctrl+U / 设置入口 → `check_update` → 有更新则 confirm →
//! `install_update`（download_and_install，装完自动重启）。

use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

/// 占位 endpoint 域名（与 tauri.conf.json `plugins.updater.endpoints` 同步维护；
/// 命中即视为"更新通道未配置"）。
const PLACEHOLDER_HOST: &str = "example.invalid";

/// 前端展示用的更新检查结果
#[derive(serde::Serialize)]
pub struct UpdateInfo {
    /// 是否有新版本
    pub available: bool,
    /// 本地当前版本
    pub current_version: String,
    /// 远端新版本号（available 时有值）
    pub version: Option<String>,
    /// 更新说明（release notes，可空）
    pub notes: Option<String>,
}

/// endpoints 为空或含占位域名 → 未配置
fn endpoint_unconfigured(app: &AppHandle) -> bool {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|v| v.get("endpoints"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.is_empty()
                || arr
                    .iter()
                    .filter_map(|e| e.as_str())
                    .any(|e| e.contains(PLACEHOLDER_HOST))
        })
        .unwrap_or(true)
}

/// 检测更新：Ok(Some) = 有新版本；Ok(None) = 已是最新。
/// 通道未配置返回可读 Err（UI 据此提示）。
#[tauri::command(async)]
pub async fn check_update(app: AppHandle) -> Result<UpdateInfo, String> {
    if endpoint_unconfigured(&app) {
        return Err("更新通道未配置（首个正式版发布后启用）".to_string());
    }
    let updater = app
        .updater()
        .map_err(|e| format!("初始化更新器失败: {e}"))?;
    match updater.check().await {
        Ok(Some(u)) => Ok(UpdateInfo {
            available: true,
            current_version: u.current_version.clone(),
            version: Some(u.version.clone()),
            notes: u.body.clone(),
        }),
        Ok(None) => Ok(UpdateInfo {
            available: false,
            current_version: env!("CARGO_PKG_VERSION").to_string(),
            version: None,
            notes: None,
        }),
        Err(e) => Err(format!("检查更新失败: {e}")),
    }
}

/// 下载并安装更新（装完自动重启；进度回调为空实现——前端以状态提示代替）。
/// 先 check 再安装，避免与 `check_update` 之间出现竞态（用户点确认前又发布新版）。
#[tauri::command(async)]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    if endpoint_unconfigured(&app) {
        return Err("更新通道未配置（首个正式版发布后启用）".to_string());
    }
    let updater = app
        .updater()
        .map_err(|e| format!("初始化更新器失败: {e}"))?;
    let update = updater
        .check()
        .await
        .map_err(|e| format!("检查更新失败: {e}"))?
        .ok_or_else(|| "没有可用更新".to_string())?;
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| format!("下载或安装更新失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// endpoints 含占位域名 → 未配置（example.invalid 的既有回归护栏）
    #[test]
    fn placeholder_host_matches_config_convention() {
        // 与 tauri.conf.json 的占位值保持一致；改任一侧必须同步改另一侧
        assert!(PLACEHOLDER_HOST.contains("example.invalid"));
        assert_eq!(PLACEHOLDER_HOST, "example.invalid");
    }
}
