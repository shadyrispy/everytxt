//! 前端 invoke 契约 DTO（集成负责人所有权；与 `ui/src/types.ts` 严格一一对应）
//!
//! 改动本文件 = 改前端契约，必须同步 ui/src/types.ts。

use serde::{Deserialize, Serialize};

use stcore::config::Config;
use stcore::search::{SearchFilter, SearchMode, SortKey};

fn default_page() -> u32 {
    0
}

fn default_page_size() -> u32 {
    200
}

/* ------------------------------ 子项 1 ------------------------------ */

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    pub query: String,
    pub mode: SearchMode,
    #[serde(default)]
    pub reverse: bool,
    #[serde(default)]
    pub dir: Option<String>,
    /// 指定文件类型**名**（由 config 解析为扩展名白名单）
    #[serde(default)]
    pub file_type: Option<String>,
    #[serde(default)]
    pub sort: SortKey,
    /// 页码（0 起，R3 分页）
    #[serde(default = "default_page")]
    pub page: u32,
    /// 每页条数（旧 `limit` 语义，R3 更名）
    #[serde(default = "default_page_size")]
    pub page_size: u32,
    /// 修改时间区间下界（unix 秒，闭区间）；None = 不限（R3）
    #[serde(default)]
    pub mtime_from: Option<i64>,
    /// 修改时间区间上界（unix 秒，闭区间）；None = 不限（R3）
    #[serde(default)]
    pub mtime_to: Option<i64>,
}

impl SearchRequest {
    /// 转 stcore 过滤口径
    pub fn to_filter(&self, cfg: &Config) -> SearchFilter {
        SearchFilter {
            dir: self.dir.clone().filter(|d| !d.is_empty()),
            exts: self
                .file_type
                .as_deref()
                .filter(|n| !n.is_empty())
                .and_then(|n| cfg.exts_of(n)),
            sort: self.sort,
        }
    }
}

/// 结果行（与前端 SearchResult 对齐：name/path/mtime/size/hits/snippet）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HitRow {
    pub name: String,
    pub path: String,
    pub mtime: i64,
    pub size: u64,
    pub hits: usize,
    pub snippet: String,
}

impl From<stcore::search::SearchHit> for HitRow {
    fn from(h: stcore::search::SearchHit) -> Self {
        Self {
            name: h.title,
            path: h.path,
            mtime: h.mtime,
            size: h.size,
            hits: h.hits,
            snippet: h.snippet,
        }
    }
}

/* ------------------------------ 子项 3 ------------------------------ */

/// 索引统计（`available=false` 表示索引当前不可用：docs/size 归零，前端展示不可用态）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStats {
    pub docs: u64,
    pub size: String,
    pub available: bool,
}

/// 存储信息（同上）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageInfo {
    pub current: String,
    pub docs: u64,
    pub size: String,
    pub available: bool,
}

/* ------------------------------ 子项 2 ------------------------------ */

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileOpFailure {
    pub path: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileOpResult {
    pub ok: usize,
    pub failed: Vec<FileOpFailure>,
}

impl FileOpResult {
    pub fn new() -> Self {
        Self {
            ok: 0,
            failed: Vec::new(),
        }
    }

    pub fn push_ok(&mut self) {
        self.ok += 1;
    }

    pub fn push_err(&mut self, path: impl Into<String>, error: impl Into<String>) {
        self.failed.push(FileOpFailure {
            path: path.into(),
            error: error.into(),
        });
    }
}

impl Default for FileOpResult {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    pub rows: Vec<HitRow>,
    /// 目标文件绝对路径；空串表示由前端先用对话框取到路径再传
    #[serde(default)]
    pub dest: String,
}
