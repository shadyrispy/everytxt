//! 7z 内存展开：sevenz-rust2（纯 Rust，无系统依赖）。
//!
//! 复用 archive.rs 的同一套压缩炸弹闸门与条目名净化：
//! - 条目数上限 [`MAX_ENTRIES`]，解压总量上限 [`MAX_TOTAL_BYTES`]（逐条目 `take` 限量读取）
//! - 条目名走 [`normalize_entry_name`]：拒 `../` 穿越、绝对路径、`#` 冲突
//! - 目录/反条目跳过；内层文本走既有 `extract_entry` 链路
//!
//! 虚拟路径约定与 zip 一致：上层以 `xxx.7z#inner.md` 寻址。

use std::io::Cursor;

use sevenz_rust2::{ArchiveReader, Password};

use super::archive::{
    collect_entry, normalize_entry_name, read_limited, MAX_ENTRIES, MAX_TOTAL_BYTES,
};
use super::{ExtractError, ExtractedEntry};

/// 展开 7z，返回内层可索引条目（按 name 排序保证确定性）。
pub(super) fn expand(bytes: &[u8]) -> Result<Vec<ExtractedEntry>, ExtractError> {
    let mut sz = ArchiveReader::new(Cursor::new(bytes), Password::empty()).map_err(invalid_data)?;

    // 闸门触发的错误经 gate 带出闭包（闭包只能返回 sevenz::Error），
    // 返回 Ok(false) 终止迭代，展开结束后统一上抛。
    let mut children = Vec::new();
    let mut total: u64 = 0;
    let mut file_count = 0usize;
    let mut gate: Option<ExtractError> = None;

    sz.for_each_entries(|entry, reader| {
        if gate.is_some() {
            return Ok(false);
        }
        // 目录与反条目（删除标记）无正文可索引
        if entry.is_directory || entry.is_anti_item {
            return Ok(true);
        }
        let Some(name) = normalize_entry_name(&entry.name) else {
            return Ok(true);
        };
        file_count += 1;
        if file_count > MAX_ENTRIES {
            gate = Some(ExtractError::TooManyEntries);
            return Ok(false);
        }
        match read_limited(reader, MAX_TOTAL_BYTES.saturating_sub(total)) {
            Ok(buf) => {
                total += buf.len() as u64;
                match collect_entry(&name, &buf, &mut children) {
                    Ok(()) => Ok(true),
                    Err(err) => {
                        gate = Some(err);
                        Ok(false)
                    }
                }
            }
            // 超限是炸弹信号；IO 错误传播会让整个包记失败
            Err(err) => {
                gate = Some(err);
                Ok(false)
            }
        }
    })
    .map_err(invalid_data)?;

    match gate {
        Some(err) => Err(err),
        None => {
            children.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(children)
        }
    }
}

/// sevenz-rust2 的错误不是 io::Error；统一包成 InvalidData。
fn invalid_data(err: sevenz_rust2::Error) -> ExtractError {
    ExtractError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        err.to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter};

    /// 内存里造一个 7z（默认 LZMA 压缩）
    fn build_7z(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = ArchiveWriter::new(Cursor::new(Vec::new())).expect("创建 7z writer 失败");
        for (name, data) in files {
            let entry = ArchiveEntry {
                name: (*name).to_string(),
                has_stream: !data.is_empty(),
                is_directory: false,
                size: data.len() as u64,
                ..ArchiveEntry::default()
            };
            writer
                .push_archive_entry(entry, Some(Cursor::new(data.to_vec())))
                .expect("写入 7z 条目失败");
        }
        writer.finish().expect("收尾 7z 失败").into_inner()
    }

    #[test]
    fn sevenz_roundtrip_extracts_text_entries() {
        let sz = build_7z(&[
            ("docs/readme.txt", "七zip 内容".as_bytes()),
            ("src/main.rs", b"fn main() {}"),
            ("img.png", &[0x89, 0x50]),
        ]);
        let entries = expand(&sz).unwrap();
        assert_eq!(entries.len(), 2, "png 应被跳过: {:?}", entries);
        assert_eq!(
            entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            vec!["docs/readme.txt", "src/main.rs"]
        );
        assert_eq!(entries[0].text, "七zip 内容");
        assert_eq!(entries[0].format, "txt");
    }

    #[test]
    fn sevenz_entry_count_gate_rejects_bomb() {
        let files: Vec<(String, &[u8])> = (0..600)
            .map(|i| (format!("f{i:03}.txt"), &b"x"[..]))
            .collect();
        let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, d)| (n.as_str(), *d)).collect();
        let sz = build_7z(&refs);
        assert!(matches!(expand(&sz), Err(ExtractError::TooManyEntries)));
    }

    #[test]
    fn sevenz_unsafe_entry_names_are_skipped() {
        let sz = build_7z(&[
            ("../evil.txt", b"escape"),
            ("has#hash.txt", b"hash"),
            ("ok.txt", b"fine"),
        ]);
        let entries = expand(&sz).unwrap();
        assert_eq!(
            entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            vec!["ok.txt"]
        );
    }

    #[test]
    fn sevenz_corrupt_is_io_error() {
        let err = expand(b"not a 7z at all").unwrap_err();
        assert!(matches!(err, ExtractError::Io(_)), "实际: {err:?}");
    }
}
