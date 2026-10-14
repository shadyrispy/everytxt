//! 压缩包内存展开：zip / tar / tar.gz(tgz) / tar.bz2(tbz)。
//!
//! 压缩包炸弹防护（提取层第一道闸门）：
//! - 条目数上限 [`MAX_ENTRIES`]：超限返回 `TooManyEntries`
//! - 解压总字节上限 [`MAX_TOTAL_BYTES`]：跨条目累计，超限返回 `TooManyEntries`
//! - 逐条目用 `take` 限量读取：本地文件头声明的 size 不可信，按实际读到的字节数判定
//!
//! 嵌套压缩包不展开（递归展开是炸弹的经典放大器），由调用方记为跳过。

use std::io::{Cursor, Read};

use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use tar::Archive;

use super::{ExtractError, ExtractedEntry};

/// 单包最大文件条目数（目录条目不占额度）
pub(super) const MAX_ENTRIES: usize = 500;

/// 单包解压总字节上限：50MB
pub(super) const MAX_TOTAL_BYTES: u64 = 50 * 1024 * 1024;

/// 压缩包种类（由调用方按文件名后缀判定后传入）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ArchiveKind {
    Zip,
    Tar,
    Tgz,
    Tbz,
}

/// 展开压缩包，返回内层可索引条目（按 name 排序保证确定性）。
///
/// 内层条目统一走 `extract_entry`：文本格式才收录；
/// 嵌套压缩包/其他格式跳过而非失败——一个怪条目不毁掉整个包。
pub(super) fn expand(kind: ArchiveKind, bytes: &[u8]) -> Result<Vec<ExtractedEntry>, ExtractError> {
    let mut children = match kind {
        ArchiveKind::Zip => expand_zip(bytes)?,
        ArchiveKind::Tar => expand_tar(bytes, Compress::None)?,
        ArchiveKind::Tgz => expand_tar(bytes, Compress::Gz)?,
        ArchiveKind::Tbz => expand_tar(bytes, Compress::Bz)?,
    };
    children.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(children)
}

/// zip 展开：条目数 upfront 已知，按索引顺序读取。
fn expand_zip(bytes: &[u8]) -> Result<Vec<ExtractedEntry>, ExtractError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(zip_error)?;
    let mut children = Vec::new();
    let mut total: u64 = 0;
    let mut file_count = 0usize;

    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(zip_error)?;
        if file.is_dir() {
            continue;
        }
        // 符号链接条目：内容只是目标路径文本，无正文可索引，跳过。
        if file.is_symlink() {
            continue;
        }
        // 路径穿越/绝对路径/虚拟分隔符冲突的条目名：跳过（等价 Unsupported）
        let Some(name) = normalize_entry_name(file.name()) else {
            continue;
        };
        file_count += 1;
        if file_count > MAX_ENTRIES {
            return Err(ExtractError::TooManyEntries);
        }
        let buf = read_limited(&mut file, MAX_TOTAL_BYTES - total)?;
        total += buf.len() as u64;
        collect_entry(&name, &buf, &mut children)?;
    }
    Ok(children)
}

/// tar 流的外层压缩（无 / gzip / bzip2）。
enum Compress {
    None,
    Gz,
    Bz,
}

/// tar / tar.gz / tar.bz2 展开：压缩层由 flate2/bzip2 流式解码，全程内存不落盘。
fn expand_tar(bytes: &[u8], compress: Compress) -> Result<Vec<ExtractedEntry>, ExtractError> {
    let cursor = Cursor::new(bytes);
    // 三种 reader 类型不同，统一装箱后交给 Archive
    let reader: Box<dyn Read> = match compress {
        Compress::None => Box::new(cursor),
        Compress::Gz => Box::new(GzDecoder::new(cursor)),
        Compress::Bz => Box::new(BzDecoder::new(cursor)),
    };
    let mut archive = Archive::new(reader);

    let mut children = Vec::new();
    let mut total: u64 = 0;
    let mut file_count = 0usize;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().into_owned();
        // tar 以路径尾 '/' 与 entry_type 双保险判目录
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() || path.ends_with('/') {
            continue;
        }
        // 符号链接/硬链接条目：内容为目标路径文本，无正文可索引，跳过
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            continue;
        }
        // 路径穿越/绝对路径/虚拟分隔符冲突的条目名：跳过（等价 Unsupported）
        let Some(name) = normalize_entry_name(&path) else {
            continue;
        };
        file_count += 1;
        if file_count > MAX_ENTRIES {
            return Err(ExtractError::TooManyEntries);
        }
        let buf = read_limited(&mut entry, MAX_TOTAL_BYTES - total)?;
        total += buf.len() as u64;
        collect_entry(&name, &buf, &mut children)?;
    }
    Ok(children)
}

/// 提取单个内层条目；不可索引格式（含嵌套压缩包）跳过。
pub(super) fn collect_entry(
    name: &str,
    bytes: &[u8],
    children: &mut Vec<ExtractedEntry>,
) -> Result<(), ExtractError> {
    match super::extract_entry(name, bytes) {
        Ok(entry) => children.push(entry),
        // 不支持的格式是正常跳过，不算失败
        Err(ExtractError::Unsupported(_)) => {}
        // 其余错误（IO/超限）向上传播，让整个包记失败
        Err(err) => return Err(err),
    }
    Ok(())
}

/// 限量读取一个条目：多读 1 字节用于判定是否超出剩余预算。
pub(super) fn read_limited<R: Read>(reader: R, remaining: u64) -> Result<Vec<u8>, ExtractError> {
    let mut buf = Vec::new();
    reader
        .take(remaining.saturating_add(1))
        .read_to_end(&mut buf)?;
    if buf.len() as u64 > remaining {
        return Err(ExtractError::TooManyEntries);
    }
    Ok(buf)
}

/// 统一压缩包内路径：Windows 打的包用 '\' 分隔，归一为 '/' 并去掉开头 "./"。
///
/// 返回 `None` 表示条目名不可信，调用方跳过该条目（等价于记 Unsupported：
/// 一个怪条目不毁掉整个包）：
/// - 含 `..` 段：路径穿越，`path#entry` 虚拟寻址会被解读到包外
/// - 绝对路径：`/etc/passwd`、`C:/Windows/x`（盘符在归一化后仍是绝对形式）
/// - 含 `#`：与上层虚拟文档寻址分隔符 `path#entry` 冲突
pub(super) fn normalize_entry_name(raw: &str) -> Option<String> {
    let unified = raw.replace('\\', "/");
    let stripped = unified.strip_prefix("./").unwrap_or(&unified);
    if stripped.contains('#') || stripped.starts_with('/') || has_drive_prefix(stripped) {
        return None;
    }
    if stripped.split('/').any(|seg| seg == "..") {
        return None;
    }
    Some(stripped.to_string())
}

/// Windows 盘符前缀（归一化后形如 "C:/x"）：单 ASCII 字母 + ':'。
fn has_drive_prefix(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some(drive), Some(':')) if drive.is_ascii_alphabetic()
    )
}

/// zip 的错误不是 io::Error；统一包成 InvalidData，ooxml/epub 复用。
pub(super) fn zip_error(err: zip::result::ZipError) -> ExtractError {
    ExtractError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        err.to_string(),
    ))
}

/// 内存里造一个 zip（Stored，避免依赖压缩特性差异）
///
/// zip 4.x 的 ZipWriter 要求 W: Write + Seek，Vec<u8> 无 Seek，
/// 用 Cursor<Vec<u8>> 承接，finish 后取回内部 Vec。
#[cfg(test)]
pub(super) fn build_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, data) in files {
        writer.start_file(*name, options).unwrap();
        std::io::Write::write_all(&mut writer, data).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

/// 内存里造一个 tar.gz
#[cfg(test)]
pub(super) fn build_tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    for (name, data) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        builder.append_data(&mut header, name, &data[..]).unwrap();
    }
    // into_inner 取回底层 GzEncoder，再 finish 得到压缩字节
    let encoder = builder.into_inner().unwrap();
    encoder.finish().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_roundtrip_extracts_text_entries() {
        let zip = build_zip(&[
            ("docs/readme.txt", "readme 内容".as_bytes()),
            ("src/main.rs", b"fn main() {}"),
            ("notes.md", "# 标题\n正文".as_bytes()),
        ]);
        let entries = expand(ArchiveKind::Zip, &zip).unwrap();
        assert_eq!(entries.len(), 3);
        // 按 name 排序
        assert_eq!(
            entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            vec!["docs/readme.txt", "notes.md", "src/main.rs"]
        );
        assert_eq!(entries[0].text, "readme 内容");
        assert_eq!(entries[0].format, "txt");
        assert_eq!(entries[1].format, "md");
        assert_eq!(entries[1].text, "# 标题\n正文");
    }

    #[test]
    fn tar_gz_roundtrip_extracts_text_entries() {
        let tgz = build_tar_gz(&[("a.txt", b"alpha"), ("dir/b.log", b"beta log")]);
        let entries = expand(ArchiveKind::Tgz, &tgz).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "a.txt");
        assert_eq!(entries[0].text, "alpha");
        assert_eq!(entries[1].name, "dir/b.log");
    }

    #[test]
    fn plain_tar_roundtrip() {
        let encoder = Vec::new();
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(3);
        builder
            .append_data(&mut header, "x.txt", &b"xyz"[..])
            .unwrap();
        let tar_bytes = builder.into_inner().unwrap();
        let entries = expand(ArchiveKind::Tar, &tar_bytes).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].text, "xyz");
    }

    #[test]
    fn nested_archive_is_skipped_not_expanded() {
        // 内层 zip 不再展开：记为跳过，不影响外层其他条目
        let inner = build_zip(&[("deep.txt", b"should not appear")]);
        let outer = build_zip(&[("bundle.zip", &inner), ("real.txt", b"real content")]);
        let entries = expand(ArchiveKind::Zip, &outer).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "real.txt");
    }

    #[test]
    fn non_text_inner_entries_are_skipped() {
        let zip = build_zip(&[
            (
                "image.png",
                &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
            ),
            ("data.json", br#"{"k": 1}"#),
            ("README", b"no extension"),
        ]);
        let entries = expand(ArchiveKind::Zip, &zip).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "data.json");
    }

    #[test]
    fn too_many_entries_rejected() {
        // 600 个文件条目 > 500 上限
        let files: Vec<(String, Vec<u8>)> = (0..600)
            .map(|i| (format!("f{i:03}.txt"), b"x".to_vec()))
            .collect();
        let refs: Vec<(&str, &[u8])> = files
            .iter()
            .map(|(name, data)| (name.as_str(), data.as_slice()))
            .collect();
        let zip = build_zip(&refs);
        assert!(matches!(
            expand(ArchiveKind::Zip, &zip),
            Err(ExtractError::TooManyEntries)
        ));
    }

    #[test]
    fn exactly_max_entries_allowed() {
        let files: Vec<(String, Vec<u8>)> = (0..MAX_ENTRIES)
            .map(|i| (format!("f{i:03}.txt"), b"x".to_vec()))
            .collect();
        let refs: Vec<(&str, &[u8])> = files
            .iter()
            .map(|(name, data)| (name.as_str(), data.as_slice()))
            .collect();
        let zip = build_zip(&refs);
        let entries = expand(ArchiveKind::Zip, &zip).unwrap();
        assert_eq!(entries.len(), MAX_ENTRIES);
    }

    #[test]
    fn corrupt_zip_is_io_error() {
        let err = expand(ArchiveKind::Zip, b"not a zip at all").unwrap_err();
        assert!(matches!(err, ExtractError::Io(_)), "实际: {err:?}");
    }

    #[test]
    fn unsafe_entry_names_are_skipped() {
        // ../ 穿越、绝对路径、盘符绝对路径、'#' 冲突名全部跳过；正常名不受影响
        let zip = build_zip(&[
            ("../evil.txt", b"escape"),
            ("/abs/rooted.txt", b"rooted"),
            ("C:/win/sys.txt", b"drive"),
            ("has#hash.txt", b"hash"),
            ("ok.txt", b"fine"),
        ]);
        let entries = expand(ArchiveKind::Zip, &zip).unwrap();
        assert_eq!(
            entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            vec!["ok.txt"]
        );
    }

    #[test]
    fn zip_symlink_entry_is_skipped() {
        use std::io::Write;
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        // add_symlink 写入 S_IFLNK 权限位，读取端 is_symlink() 为真
        writer
            .add_symlink(
                "link.txt",
                "target.txt",
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        let file_options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("real.txt", file_options).unwrap();
        writer.write_all(b"real content").unwrap();
        let zip = writer.finish().unwrap().into_inner();

        let entries = expand(ArchiveKind::Zip, &zip).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "real.txt");
        assert_eq!(entries[0].text, "real content");
    }

    #[test]
    fn tar_symlink_and_hardlink_entries_are_skipped() {
        let encoder = Vec::new();
        let mut builder = tar::Builder::new(encoder);
        let mut link_header = tar::Header::new_gnu();
        link_header.set_entry_type(tar::EntryType::Symlink);
        link_header.set_size(0);
        link_header.set_mode(0o777);
        builder
            .append_link(&mut link_header, "sym.txt", "target.txt")
            .unwrap();
        let mut hard_header = tar::Header::new_gnu();
        hard_header.set_entry_type(tar::EntryType::hard_link());
        hard_header.set_size(0);
        hard_header.set_mode(0o644);
        builder
            .append_link(&mut hard_header, "hard.txt", "target.txt")
            .unwrap();
        let mut file_header = tar::Header::new_gnu();
        file_header.set_size(4);
        builder
            .append_data(&mut file_header, "real.txt", &b"real"[..])
            .unwrap();
        let tar_bytes = builder.into_inner().unwrap();

        let entries = expand(ArchiveKind::Tar, &tar_bytes).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "real.txt");
    }
}
