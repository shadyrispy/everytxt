//! 同义词查询扩展（P3-C）。
//!
//! ## 语义
//! **仅 fuzzy 模式扩展**：查询词命中同义组 → 组内其它词作为 `Should` 备选
//! （召回扩展，不改变 AND 主语义）。`exact` 为"完全匹配"语义、`dsl` 由用户
//! 显式控制运算符，均不自动扩展。
//!
//! ## 数据源（优先级）
//! 1. `<index_dir>/synonyms.json`：`[["计算机","电脑","PC"], ...]`（用户自定义）
//! 2. 内置表 [`builtin_groups`]（常见技术/办公词对）
//!
//! 文件缺失/格式错 → 静默回落内置表（不阻塞搜索）。

use std::collections::HashMap;
use std::path::Path;

/// 同义词表：词（小写）→ 同组其它词
#[derive(Debug, Clone, Default)]
pub struct SynonymTable {
    map: HashMap<String, Vec<String>>,
}

/// 内置同义组（保守选取：语义等价、无歧义）
pub fn builtin_groups() -> Vec<Vec<&'static str>> {
    vec![
        vec!["计算机", "电脑", "pc", "computer"],
        vec!["搜索", "检索", "查找", "search"],
        vec!["文档", "文件", "文稿", "document"],
        vec!["数据库", "db", "database"],
        vec!["服务器", "服务端", "server"],
        vec!["网络", "互联网", "network"],
        vec!["局域网", "内网", "lan"],
        vec!["程序", "软件", "应用", "app"],
        vec!["代码", "源码", "源程序", "code"],
        vec!["错误", "故障", "异常", "error"],
        vec!["配置", "设置", "选项", "config"],
        vec!["用户", "使用者", "账户", "user"],
        vec!["密码", "口令", "password"],
        vec!["图片", "图像", "照片", "image"],
        vec!["视频", "影片", "video"],
        vec!["音频", "语音", "audio"],
        vec!["压缩包", "归档", "压缩文件", "archive"],
        vec!["目录", "文件夹", "folder", "directory"],
        vec!["表格", "电子表格", "excel"],
        vec!["统计", "汇总", "报表", "report"],
        vec!["性能", "效率", "performance"],
        vec!["安全", "防护", "security"],
        vec!["测试", "验证", "test"],
        vec!["安装", "部署", "install", "deploy"],
    ]
}

const SYNONYM_FILE: &str = "synonyms.json";

impl SynonymTable {
    /// 内置表
    pub fn builtin() -> Self {
        Self::from_groups(builtin_groups())
    }

    fn from_groups(groups: Vec<Vec<&str>>) -> Self {
        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for g in groups {
            let lower: Vec<String> = g.iter().map(|s| s.to_lowercase()).collect();
            for w in &lower {
                let others: Vec<String> = lower.iter().filter(|o| *o != w).cloned().collect();
                if !others.is_empty() {
                    map.entry(w.clone()).or_default().extend(others);
                }
            }
        }
        // 去重
        for v in map.values_mut() {
            v.sort();
            v.dedup();
        }
        Self { map }
    }

    /// 从索引目录加载：`synonyms.json` 存在且可解析时**覆盖**内置表
    pub fn load(index_dir: &Path) -> Self {
        let p = index_dir.join(SYNONYM_FILE);
        let Ok(raw) = std::fs::read_to_string(&p) else {
            return Self::builtin();
        };
        match serde_json::from_str::<Vec<Vec<String>>>(&raw) {
            Ok(groups) if !groups.is_empty() => {
                let mut map: HashMap<String, Vec<String>> = HashMap::new();
                for g in groups {
                    let lower: Vec<String> = g.iter().map(|s| s.to_lowercase()).collect();
                    for w in &lower {
                        let others: Vec<String> =
                            lower.iter().filter(|o| *o != w).cloned().collect();
                        if !others.is_empty() {
                            map.entry(w.clone()).or_default().extend(others);
                        }
                    }
                }
                for v in map.values_mut() {
                    v.sort();
                    v.dedup();
                }
                Self { map }
            }
            _ => Self::builtin(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// 查询词的同义词（不含原词；无同义组返回空）
    pub fn synonyms_of(&self, term: &str) -> Vec<String> {
        self.map
            .get(&term.to_lowercase())
            .cloned()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_expands_symmetric() {
        let t = SynonymTable::builtin();
        let a = t.synonyms_of("电脑");
        assert!(a.contains(&"计算机".to_string()), "{a:?}");
        // 对称：计算机也能扩出电脑
        assert!(t.synonyms_of("计算机").contains(&"电脑".to_string()));
        // 大小写不敏感
        assert!(t.synonyms_of("PC").contains(&"电脑".to_string()));
    }

    #[test]
    fn unknown_term_has_no_synonyms() {
        let t = SynonymTable::builtin();
        assert!(t.synonyms_of("绝不存在的词xyz").is_empty());
    }

    #[test]
    fn custom_file_overrides_builtin() {
        let dir = std::env::temp_dir().join(format!("st-syn-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SYNONYM_FILE), r#"[["甲","乙"],["丙","丁"]]"#).unwrap();
        let t = SynonymTable::load(&dir);
        assert_eq!(t.synonyms_of("甲"), vec!["乙"]);
        // 内置词在自定义表下不生效（覆盖语义）
        assert!(t.synonyms_of("电脑").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_file_falls_back_to_builtin() {
        let dir = std::env::temp_dir().join(format!("st-syn-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SYNONYM_FILE), "{ not json").unwrap();
        let t = SynonymTable::load(&dir);
        assert!(t.synonyms_of("电脑").contains(&"计算机".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
