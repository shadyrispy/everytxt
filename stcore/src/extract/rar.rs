//! RAR 内存展开：unrar crate（绑定官方 C++ 解压源码 unrar_sys）。
//!
//! 复用 archive.rs 的同一套压缩炸弹闸门与条目名净化：
//! - 条目数上限 [`MAX_ENTRIES`]，解压总量上限 [`MAX_TOTAL_BYTES`]（按声明 size 预检 + 实际字节数复核）
//! - 条目名走 [`normalize_entry_name`]：拒 `../` 穿越、绝对路径、`#` 冲突
//! - 目录跳过；加密条目无密码无法解，跳过而非失败（一个怪条目不毁掉整个包）
//! - 内层文本走既有 `extract_entry` 链路；虚拟路径 `xxx.rar#inner.md`
//!
//! 文件级入口：RAR 由官方库按磁盘路径打开，走 `extract_path` 传入的真实路径。

use std::path::Path;

use super::archive::{collect_entry, normalize_entry_name, MAX_ENTRIES, MAX_TOTAL_BYTES};
use super::{ExtractError, ExtractedEntry};

/// 展开 RAR，返回内层可索引条目（按 name 排序保证确定性）。
pub(super) fn expand(path: &Path) -> Result<Vec<ExtractedEntry>, ExtractError> {
    let mut next = unrar::Archive::new(path)
        .open_for_processing()
        .map_err(rar_error)?;

    let mut children = Vec::new();
    let mut total: u64 = 0;
    let mut file_count = 0usize;

    loop {
        let Some(header) = next.read_header().map_err(rar_error)? else {
            break;
        };
        let entry = header.entry();
        if entry.is_directory() {
            next = header.skip().map_err(rar_error)?;
            continue;
        }
        // 加密条目：无密码解不开，跳过（与 zip 里怪条目同一宽容语义）
        if entry.is_encrypted() {
            next = header.skip().map_err(rar_error)?;
            continue;
        }
        let Some(name) = normalize_entry_name(&entry.filename.to_string_lossy()) else {
            next = header.skip().map_err(rar_error)?;
            continue;
        };
        file_count += 1;
        if file_count > MAX_ENTRIES {
            return Err(ExtractError::TooManyEntries);
        }
        // 声明 size 预检：超预算直接判炸弹，不去解压
        let remaining = MAX_TOTAL_BYTES.saturating_sub(total);
        if entry.unpacked_size > remaining {
            return Err(ExtractError::TooManyEntries);
        }
        let (data, rest) = header.read().map_err(rar_error)?;
        // 实际字节数复核（声明 size 不可信）
        if data.len() as u64 > remaining {
            return Err(ExtractError::TooManyEntries);
        }
        total += data.len() as u64;
        collect_entry(&name, &data, &mut children)?;
        next = rest;
    }

    children.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(children)
}

/// unrar 的错误不是 io::Error；统一包成 InvalidData。
fn rar_error(err: unrar::error::UnrarError) -> ExtractError {
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
    fn rar_corrupt_is_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("corrupt.rar");
        std::fs::write(&p, b"this is not a rar at all").unwrap();
        let err = expand(&p).unwrap_err();
        assert!(matches!(err, ExtractError::Io(_)), "实际: {err:?}");
    }

    /// 真实 RAR 样例测试：设 SEARCH_TOOL_RAR_SAMPLE 指向 .rar 文件后启用。
    /// CI/本机无样例时忽略（见 R4 报告）。
    #[test]
    #[ignore = "需要真实 RAR 样例：SEARCH_TOOL_RAR_SAMPLE=<path>"]
    fn rar_real_sample_extracts_text_entries() {
        let path = std::env::var("SEARCH_TOOL_RAR_SAMPLE").expect("未设置样例路径");
        let entries = expand(Path::new(&path)).unwrap();
        assert!(!entries.is_empty(), "样例包应至少有一个文本条目");
    }
}
