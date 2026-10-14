//! 二进制文件 strings 提取：exe/dll/so/dylib/bin 等无可读结构格式。
//!
//! 策略照 Unix `strings`：提取
//! - 可打印 ASCII 连续序列（≥ [`MIN_RUN`] 字节，含 Tab）
//! - UTF-16LE 文本序列（≥ [`MIN_RUN`] 个码元，覆盖 ASCII/CJK/全角）
//!
//! 拼接为纯文本供索引；总输出截断在 [`MAX_OUTPUT`]（256KB），
//! 上游仍有单文件 50MB 闸门兜底。无依赖、纯内存。

/// 最小序列长度（连续判定阈值，与经典 strings 的默认值一致）
const MIN_RUN: usize = 5;

/// 输出总字节上限
pub(super) const MAX_OUTPUT: usize = 256 * 1024;

/// 提取二进制文件中的可打印字符串（ASCII + UTF-16LE），按序列逐行拼接。
pub(super) fn strings_to_text(bytes: &[u8]) -> String {
    let mut out = String::new();
    for run in ascii_runs(bytes) {
        if out.len() >= MAX_OUTPUT {
            break;
        }
        push_run(&mut out, &run);
    }
    for run in utf16le_runs(bytes) {
        if out.len() >= MAX_OUTPUT {
            break;
        }
        push_run(&mut out, &run);
    }
    if out.len() > MAX_OUTPUT {
        // 按字符边界截断，避免劈开 UTF-8 序列
        let mut end = MAX_OUTPUT;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
    }
    out
}

fn push_run(out: &mut String, run: &str) {
    out.push_str(run);
    out.push('\n');
}

/// 扫描可打印 ASCII（0x20..=0x7E 与 Tab）连续序列，≥ MIN_RUN 才收录。
fn ascii_runs(bytes: &[u8]) -> Vec<String> {
    let mut runs = Vec::new();
    let mut start: Option<usize> = None;
    for (i, b) in bytes.iter().chain(std::iter::once(&0)).enumerate() {
        if is_printable(*b) {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(s) = start.take() {
            if i - s >= MIN_RUN {
                runs.push(String::from_utf8_lossy(&bytes[s..i]).into_owned());
            }
        }
    }
    runs
}

fn is_printable(b: u8) -> bool {
    (0x20..=0x7E).contains(&b) || b == b'\t'
}

/// 扫描 UTF-16LE 文本序列：连续码元（低字节在前），≥ MIN_RUN 个码元才收录。
/// 覆盖 ASCII、CJK 统一表意、CJK 标点与全角形式。
///
/// 对齐策略：只在偶数偏移取码元（Windows 文件的 UTF-16 串几乎都偶对齐）；
/// 码元不可信时跳过 2 字节保持奇偶对齐，避免错位解码出伪文本。
fn utf16le_runs(bytes: &[u8]) -> Vec<String> {
    let mut runs = Vec::new();
    let mut units: Vec<u16> = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        let unit = u16::from_le_bytes([bytes[i], bytes[i + 1]]);
        if is_plausible_utf16(unit) {
            units.push(unit);
            i += 2;
        } else {
            flush_utf16(&mut units, &mut runs);
            i += 2;
        }
    }
    flush_utf16(&mut units, &mut runs);
    runs
}

fn flush_utf16(units: &mut Vec<u16>, runs: &mut Vec<String>) {
    // 阈值：ASCII 序列沿用经典 strings 的 5；含 CJK 时降到 4
    //（中文实词常见 4 字，如"系统设置"）
    let has_cjk = units.iter().any(|&u| u > 0x2E7F);
    let need = if has_cjk { MIN_RUN - 1 } else { MIN_RUN };
    if units.len() >= need {
        runs.push(
            units
                .iter()
                .flat_map(|&u| char::from_u32(u as u32))
                .collect(),
        );
    }
    units.clear();
}

/// 码元可信判定：可见 ASCII / CJK 标点 / CJK 统一表意 / 全角形式。
/// 高位非 0 的码元只认这几个连续区，避免把指针/浮点噪声当文本。
fn is_plausible_utf16(u: u16) -> bool {
    matches!(u, 0x20..=0x7E | 0x3000..=0x303F | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_runs_extracted() {
        // 二进制噪声 + 两段 ≥5 的 ASCII + 一段 4 字节不足阈值
        let mut bytes = vec![0xDE, 0xAD, 0xBE, 0xEF];
        bytes.extend_from_slice(b"hello_world"); // 11 连续
        bytes.extend_from_slice(&[0x00, 0xFF]);
        bytes.extend_from_slice(b"func"); // 4 连续，不收录
        bytes.extend_from_slice(&[0x01]);
        bytes.extend_from_slice(b"  path=C:\\bin"); // 含空格与路径
        let text = strings_to_text(&bytes);
        assert!(text.contains("hello_world"), "实际: {text:?}");
        assert!(text.contains("path=C:\\bin"), "实际: {text:?}");
        assert!(!text.contains("func\n"), "4 字节序列不应收录: {text:?}");
        assert!(text.ends_with('\n'), "序列按行分隔");
    }

    #[test]
    fn utf16le_runs_extracted() {
        // PE 头噪声：偶数长度且不会误配成可信码元（保持后续 UTF-16 偶对齐）
        let mut bytes = vec![0x4D, 0xFF, 0xFF, 0x00];
        for unit in "系统设置".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes.extend_from_slice(&[0x00, 0x01]); // 打断
        for unit in "System".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let text = strings_to_text(&bytes);
        assert!(text.contains("系统设置"), "实际: {text:?}");
        assert!(text.contains("System"), "实际: {text:?}");
    }

    #[test]
    fn output_truncated_at_max() {
        // 512KB 连续 ASCII：超过 256KB 上限应被截断
        let bytes = vec![b'A'; MAX_OUTPUT * 2];
        let text = strings_to_text(&bytes);
        assert!(text.len() <= MAX_OUTPUT, "实际: {}", text.len());
        assert!(text.chars().all(|c| c == 'A' || c == '\n'));
    }
}
