//! CHM 提取：chmlib（绑定 libchm/chmlib C 源码）。
//!
//! CHM 是 HTML 帮助包：枚举内部对象 → 过滤系统元文件（`#`/`$` 开头的
//! SPECIAL/META 条目）→ 内层文本走既有 `extract_entry` 链路。
//!
//! 复用 archive.rs 的同一套闸门与语义：
//! - 条目数上限 [`MAX_ENTRIES`]，解压总量上限 [`MAX_TOTAL_BYTES`]（按 unit.length 预检）
//! - 条目名走 [`normalize_entry_name`]：拒 `../` 穿越、绝对路径、`#` 冲突
//!
//! 文件级入口：libchm 按磁盘路径打开文件，走 `extract_path` 传入的真实路径；
//! 虚拟路径 `xxx.chm#topic.html`。

use std::path::Path;

use chmlib::{ChmFile, Continuation, Filter};

use super::archive::{collect_entry, normalize_entry_name, MAX_ENTRIES, MAX_TOTAL_BYTES};
use super::{ExtractError, ExtractedEntry};

/// 展开 CHM，返回内层可索引条目（按 name 排序保证确定性）。
pub(super) fn expand(path: &Path) -> Result<Vec<ExtractedEntry>, ExtractError> {
    let mut file = ChmFile::open(path).map_err(open_error)?;

    let mut children = Vec::new();
    let mut total: u64 = 0;
    let mut file_count = 0usize;
    // 闸门触发的错误经 gate 带出闭包后统一上抛（闭包经 FFI 边界，
    // 只能返回 Continuation）。
    let mut gate: Option<ExtractError> = None;

    file.for_each(Filter::FILES | Filter::NORMAL, |chm, unit| {
        if gate.is_some() {
            return Continuation::Stop;
        }
        if unit.is_dir() || unit.is_special() || unit.is_meta() {
            return Continuation::Continue;
        }
        // CHM 内部路径以 '/' 开头，先剥掉再走统一净化
        let raw = unit.path().and_then(|p| p.to_str()).unwrap_or_default();
        let Some(name) = normalize_entry_name(raw.trim_start_matches('/')) else {
            return Continuation::Continue;
        };
        file_count += 1;
        if file_count > MAX_ENTRIES {
            gate = Some(ExtractError::TooManyEntries);
            return Continuation::Stop;
        }
        // unit.length() 是对象声明大小；超预算直接判炸弹，不去读取
        let remaining = MAX_TOTAL_BYTES.saturating_sub(total);
        if unit.length() > remaining {
            gate = Some(ExtractError::TooManyEntries);
            return Continuation::Stop;
        }
        let mut buf = vec![0u8; unit.length() as usize];
        match chm.read(&unit, 0, &mut buf) {
            Ok(n) => {
                buf.truncate(n);
                total += n as u64;
                if let Err(err) = collect_entry(&name, &buf, &mut children) {
                    gate = Some(err);
                    return Continuation::Stop;
                }
                Continuation::Continue
            }
            Err(_) => {
                // 单个读不出的条目：跳过而非失败（一个怪条目不毁掉整个包）
                Continuation::Continue
            }
        }
    })
    .map_err(|err| {
        ExtractError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            err.to_string(),
        ))
    })?;

    match gate {
        Some(err) => Err(err),
        None => {
            children.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(children)
        }
    }
}

/// OpenError 统一包成 InvalidData。
fn open_error(err: chmlib::OpenError) -> ExtractError {
    ExtractError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        err.to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 损坏样例：错误路径受控（Io 不 panic）
    #[test]
    fn chm_corrupt_is_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("corrupt.chm");
        std::fs::write(&p, b"this is not a chm at all").unwrap();
        let err = expand(&p).unwrap_err();
        assert!(matches!(err, ExtractError::Io(_)), "实际: {err:?}");
    }

    /// 真实 CHM 样例测试：设 SEARCH_TOOL_CHM_SAMPLE 指向 .chm 文件后启用。
    #[test]
    #[ignore = "需要真实 CHM 样例：SEARCH_TOOL_CHM_SAMPLE=<path>"]
    fn chm_real_sample_extracts_text_entries() {
        let path = std::env::var("SEARCH_TOOL_CHM_SAMPLE").expect("未设置样例路径");
        let entries = expand(Path::new(&path)).unwrap();
        assert!(!entries.is_empty(), "样例包应至少有一个文本条目");
    }
}
