//! R4 格式补齐集成测试：7z / bz2 / 二进制 strings 的文件级路由。
//!
//! 夹具全部测试内纯内存构造（sevenz-rust2 writer + bzip2 encoder + tar builder），
//! 不依赖外部 CLI。

use std::io::{Cursor, Write};

use stcore::extract::{extract_entry, extract_path};

/// 内存造 7z（sevenz-rust2 writer）
fn build_7z(files: &[(&str, &[u8])]) -> Vec<u8> {
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter};
    let mut writer = ArchiveWriter::new(Cursor::new(Vec::new())).unwrap();
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
            .unwrap();
    }
    writer.finish().unwrap().into_inner()
}

/// 内存造 tar.bz2
fn build_tar_bz2(files: &[(&str, &[u8])]) -> Vec<u8> {
    let encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    for (name, data) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        builder.append_data(&mut header, name, &data[..]).unwrap();
    }
    let encoder = builder.into_inner().unwrap();
    encoder.finish().unwrap()
}

/// 内存造单文件 .bz2
fn build_bz2(data: &[u8]) -> Vec<u8> {
    let mut encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

fn write_tmp(tag: &str, content: &[u8]) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("r4-test-{}-{tag}", std::process::id()));
    std::fs::write(&p, content).unwrap();
    p
}

#[test]
fn path_7z_expands_children() {
    let sz = build_7z(&[
        ("docs/说明.txt", "甲内容".as_bytes()),
        ("notes.md", "# 乙".as_bytes()),
        ("lib.dll", &[0xDE, 0xAD]),
    ]);
    let p = write_tmp("bundle.7z", &sz);
    let r = extract_path(&p).unwrap();
    assert_eq!(r.format, "7z");
    assert!(r.text.is_empty(), "压缩包顶层无正文");
    // dll 现走 strings 通道（R4）：无字符串时 text 为空但仍收录
    assert_eq!(r.children.len(), 3);
    assert_eq!(r.children[0].name, "docs/说明.txt");
    assert_eq!(r.children[0].text, "甲内容");
    let dll = r.children.iter().find(|c| c.name == "lib.dll").unwrap();
    assert_eq!(dll.text, "");
    std::fs::remove_file(&p).ok();
}

#[test]
fn path_tar_bz2_goes_archive_route() {
    let tbz = build_tar_bz2(&[("a.txt", b"alpha"), ("dir/b.log", b"beta log")]);
    let p = write_tmp("archive.tar.bz2", &tbz);
    let r = extract_path(&p).unwrap();
    assert_eq!(r.format, "tbz", "合法 tar.bz2 应走归档路由");
    assert_eq!(r.children.len(), 2);
    assert_eq!(r.children[0].name, "a.txt");
    assert_eq!(r.children[0].text, "alpha");
    std::fs::remove_file(&p).ok();
}

#[test]
fn path_bz2_single_file_falls_back_to_text() {
    let bz = build_bz2("纯文本内容，不是 tar".as_bytes());
    let p = write_tmp("single.txt.bz2", &bz);
    let r = extract_path(&p).unwrap();
    assert_eq!(r.format, "bz2");
    assert_eq!(r.text, "纯文本内容，不是 tar");
    assert!(r.children.is_empty());
    std::fs::remove_file(&p).ok();
}

#[test]
fn path_bz2_corrupt_is_io_error() {
    let p = write_tmp("corrupt.bz2", b"this is not bzip2 at all");
    let err = extract_path(&p).unwrap_err();
    assert!(
        matches!(err, stcore::extract::ExtractError::Io(_)),
        "实际: {err:?}"
    );
    std::fs::remove_file(&p).ok();
}

#[test]
fn path_binary_strings_extracted() {
    // exe 夹具：DOS 头噪声 + UTF-16LE 中文字符串
    let mut bytes = vec![0x4D, 0x5A, 0x90, 0x00];
    for unit in "访问控制列表".encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes.extend_from_slice(&[0x00, 0x00, 0x01, 0x02]);
    bytes.extend_from_slice(b"GetProcAddress");
    let p = write_tmp("sample.dll", &bytes);
    let r = extract_path(&p).unwrap();
    assert_eq!(r.format, "dll");
    assert!(r.text.contains("访问控制列表"), "实际: {:?}", r.text);
    assert!(r.text.contains("GetProcAddress"));
    std::fs::remove_file(&p).ok();
}

#[test]
fn entry_binary_inside_archive_yields_strings() {
    // 压缩包内层二进制条目同样走 strings 通道
    let entry = extract_entry("kernel32.dll", b"LoadLibraryA").unwrap();
    assert_eq!(entry.format, "dll");
    assert!(entry.text.contains("LoadLibraryA"));
    // 无可打印序列 → 输出为空字符串而非报错
    let empty = extract_entry("noise.bin", &[0x00, 0x01, 0xFF]).unwrap();
    assert_eq!(empty.text, "");
}
