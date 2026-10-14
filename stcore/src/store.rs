//! 搜索历史与书签存储（SQLite，rusqlite bundled；架构 v2 R3-D8）
//!
//! history 与 bookmarks 同库不同表，DB 文件位于索引目录 `search-tool.db`。
//! SQLite 快（毫秒级），命令层同步调用即可；打开失败向上返回错误，由命令层降级。

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};

/// 历史容量上限（FIFO 淘汰）
pub const HISTORY_CAP: usize = 500;

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 搜索历史条目
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: i64,
    pub query: String,
    pub mode: String,
    pub result_count: i64,
    pub ts: i64,
}

/// 书签条目
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bookmark {
    pub id: i64,
    pub path: String,
    pub name: String,
    pub created: i64,
}

/* ------------------------------ 搜索历史 ------------------------------ */

pub struct HistoryStore {
    conn: Connection,
}

impl HistoryStore {
    /// 打开（或创建）历史库；父目录不存在时自动创建
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS history (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                query        TEXT NOT NULL,
                mode         TEXT NOT NULL,
                result_count INTEGER NOT NULL DEFAULT 0,
                ts           INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_history_ts ON history(ts);",
        )?;
        Ok(Self { conn })
    }

    /// 记一条历史，超 `HISTORY_CAP` 按 id FIFO 淘汰最旧
    pub fn add(&self, query: &str, mode: &str, result_count: i64) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO history(query, mode, result_count, ts) VALUES (?1, ?2, ?3, ?4)",
            params![query, mode, result_count, unix_now()],
        )?;
        self.conn.execute(
            "DELETE FROM history WHERE id NOT IN (
                SELECT id FROM history ORDER BY id DESC LIMIT ?1
            )",
            [HISTORY_CAP as i64],
        )?;
        Ok(())
    }

    /// 最近 `limit` 条（新在前）
    pub fn list(&self, limit: usize) -> anyhow::Result<Vec<HistoryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, query, mode, result_count, ts
             FROM history ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok(HistoryEntry {
                id: r.get(0)?,
                query: r.get(1)?,
                mode: r.get(2)?,
                result_count: r.get(3)?,
                ts: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn clear(&self) -> anyhow::Result<()> {
        self.conn.execute("DELETE FROM history", [])?;
        Ok(())
    }
}

/* -------------------------------- 书签 -------------------------------- */

pub struct BookmarkStore {
    conn: Connection,
}

impl BookmarkStore {
    /// 打开（或创建）书签库；与 HistoryStore 同文件不同表
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS bookmarks (
                id      INTEGER PRIMARY KEY AUTOINCREMENT,
                path    TEXT NOT NULL UNIQUE,
                name    TEXT NOT NULL,
                created INTEGER NOT NULL
            );",
        )?;
        Ok(Self { conn })
    }

    /// 添加书签；path 已存在则更新 name/created（幂等 upsert）
    pub fn add(&self, path: &str, name: &str) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO bookmarks(path, name, created) VALUES (?1, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET name = excluded.name, created = excluded.created",
            params![path, name, unix_now()],
        )?;
        Ok(())
    }

    /// 全部书签（新在前）
    pub fn list(&self) -> anyhow::Result<Vec<Bookmark>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, path, name, created FROM bookmarks ORDER BY id DESC")?;
        let rows = stmt.query_map([], |r| {
            Ok(Bookmark {
                id: r.get(0)?,
                path: r.get(1)?,
                name: r.get(2)?,
                created: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// 删除书签；返回是否确有删除
    pub fn remove(&self, path: &str) -> anyhow::Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM bookmarks WHERE path = ?1", [path])?;
        Ok(n > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_db(tag: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(format!("{tag}.db"));
        (dir, p)
    }

    #[test]
    fn history_crud_and_fifo_eviction() {
        let (_d, p) = tmp_db("hist");
        let hs = HistoryStore::open(&p).unwrap();
        assert!(hs.list(10).unwrap().is_empty());

        for i in 0..(HISTORY_CAP + 5) {
            hs.add(&format!("q{i}"), "fuzzy", i as i64).unwrap();
        }
        // 上限 500：最旧 5 条被淘汰，最新在前
        let all = hs.list(HISTORY_CAP + 100).unwrap();
        assert_eq!(all.len(), HISTORY_CAP);
        assert_eq!(all[0].query, format!("q{}", HISTORY_CAP + 4));
        assert!(!all.iter().any(|e| e.query == "q0"));

        // list(limit) 截断
        assert_eq!(hs.list(10).unwrap().len(), 10);

        hs.clear().unwrap();
        assert!(hs.list(10).unwrap().is_empty());
    }

    #[test]
    fn bookmark_upsert_and_remove() {
        let (_d, p) = tmp_db("bm");
        let bs = BookmarkStore::open(&p).unwrap();
        bs.add("/a/b.txt", "b.txt").unwrap();
        bs.add("/c/d.md", "d.md").unwrap();
        let all = bs.list().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "d.md"); // 新在前

        // UNIQUE(path)：重复 add 更新 name 而非新增行
        bs.add("/a/b.txt", "重命名.txt").unwrap();
        let all = bs.list().unwrap();
        assert_eq!(all.len(), 2);
        assert!(all
            .iter()
            .any(|b| b.path == "/a/b.txt" && b.name == "重命名.txt"));

        assert!(bs.remove("/a/b.txt").unwrap());
        assert!(!bs.remove("/a/b.txt").unwrap()); // 二次删除 false
        assert_eq!(bs.list().unwrap().len(), 1);
    }

    #[test]
    fn history_and_bookmark_share_same_db_file() {
        let (_d, p) = tmp_db("share");
        HistoryStore::open(&p).unwrap().add("x", "dsl", 1).unwrap();
        BookmarkStore::open(&p).unwrap().add("/p", "p").unwrap();
        // 两表互不干扰
        assert_eq!(HistoryStore::open(&p).unwrap().list(10).unwrap().len(), 1);
        assert_eq!(BookmarkStore::open(&p).unwrap().list().unwrap().len(), 1);
    }
}
