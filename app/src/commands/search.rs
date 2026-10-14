//! 搜索命令（子项 1 所有权：搜索模式与 DSL；R3：regex/时间区间/分页/历史/书签）
//!
//! 契约：ui/src/api/search.ts → invoke('search', { req: SearchRequest })
//!       ui/src/api/store.ts   → history / bookmark 命令族

use serde::Serialize;
use tauri::State;

use stcore::store::{BookmarkStore, HistoryStore};

use crate::commands::dto::{HitRow, SearchRequest};
use crate::state::AppState;

/// 分页搜索响应（与前端 SearchResponse 对齐）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponseDto {
    pub hits: Vec<HitRow>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
    pub partial: bool,
}

/// 历史库 / 书签库共用 DB 文件：索引目录下 search-tool.db
fn db_path(state: &AppState) -> std::path::PathBuf {
    state.index_dir().join("search-tool.db")
}

/// 搜索（模糊/精准/高级/正则 + 反向 + 指定目录/类型 + 时间区间 + 排序 + 分页）
///
/// 成功后自动记一条搜索历史（result_count=total；记录失败静默，不影响搜索）。
#[tauri::command(async)]
pub fn search(req: SearchRequest, state: State<'_, AppState>) -> Result<SearchResponseDto, String> {
    let cfg = stcore::config::Config::load(&state.index_dir()).map_err(|e| e.to_string())?;
    let filter = req.to_filter(&cfg);
    let resp = state
        .with_mgr(|mgr| {
            stcore::search::search_page(
                mgr,
                &req.query,
                req.mode,
                req.reverse,
                req.page,
                req.page_size as usize,
                req.mtime_from,
                req.mtime_to,
                &filter,
            )
        })
        .map_err(|e| e.to_string())?;

    // 历史记录（DB 打开失败/写失败均静默：历史是增益功能，不能拖垮搜索）
    if let Ok(hs) = HistoryStore::open(&db_path(&state)) {
        let _ = hs.add(
            &req.query,
            stcore::search::mode_str(req.mode),
            resp.total as i64,
        );
    }

    Ok(SearchResponseDto {
        hits: resp.hits.into_iter().map(HitRow::from).collect(),
        total: resp.total,
        page: resp.page,
        page_size: resp.page_size,
        partial: resp.partial,
    })
}

/* ------------------------------ 历史命令 ------------------------------ */

#[tauri::command]
pub fn list_history(
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> Result<Vec<stcore::store::HistoryEntry>, String> {
    HistoryStore::open(&db_path(&state))
        .and_then(|hs| hs.list(limit.unwrap_or(10)))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clear_history(state: State<'_, AppState>) -> Result<(), String> {
    HistoryStore::open(&db_path(&state))
        .and_then(|hs| hs.clear())
        .map_err(|e| e.to_string())
}

/* ------------------------------ 书签命令 ------------------------------ */

#[tauri::command]
pub fn add_bookmark(path: String, name: String, state: State<'_, AppState>) -> Result<(), String> {
    BookmarkStore::open(&db_path(&state))
        .and_then(|bs| bs.add(&path, &name))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_bookmarks(state: State<'_, AppState>) -> Result<Vec<stcore::store::Bookmark>, String> {
    BookmarkStore::open(&db_path(&state))
        .and_then(|bs| bs.list())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn remove_bookmark(path: String, state: State<'_, AppState>) -> Result<(), String> {
    BookmarkStore::open(&db_path(&state))
        .and_then(|bs| bs.remove(&path).map(|_| ()))
        .map_err(|e| e.to_string())
}
