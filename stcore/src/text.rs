//! 编码检测与文本解码。
//!
//! 策略：BOM 优先 → chardetng 统计检测（只取前 4KB 样本，控制大文件开销）
//! → UTF-8 strict 兜底修正。解码一律 lossy：非法字节替换为 U+FFFD，不返回 Err，
//! 因为提取层的职责是"尽量拿到可索引文本"，编码瑕疵不应阻断索引。

use chardetng::EncodingDetector;
use encoding_rs::{Encoding, GB18030, GBK, UTF_16BE, UTF_16LE, UTF_8, WINDOWS_1252};

/// chardetng 样本长度：前 4KB 足够统计判定，且避免整读大文件。
const SAMPLE_LEN: usize = 4096;

/// 检测编码并解码为 String；返回 (文本, 编码名如 "UTF-8"/"GBK"/"GB18030"/"Big5"/"windows-1252")。
///
/// 规则：BOM 优先 → chardetng 检测（取前 4KB 样本）→ UTF-8 strict 兜底。
/// 解码失败字符用 U+FFFD 替换，不返回 Err。
pub fn decode_bytes(bytes: &[u8]) -> (String, &'static str) {
    if bytes.is_empty() {
        return (String::new(), "UTF-8");
    }

    // 1. BOM 优先：带 BOM 时编码已确定，无需统计检测
    if let Some((encoding, body)) = strip_bom(bytes) {
        return (decode_with(encoding, body), encoding_name(encoding));
    }

    // 2. chardetng 统计检测（前 4KB 样本）
    let sample = &bytes[..bytes.len().min(SAMPLE_LEN)];
    let mut detector = EncodingDetector::new();
    detector.feed(sample, true);
    let guess = detector.guess(None, true);

    // 3. UTF-8 strict 兜底：chardetng 对纯 ASCII/英文常猜 windows-1252，
    // 也可能把 UTF-8 文本误判成 GB18030；只要样本本身是合法 UTF-8 就按 UTF-8 处理，
    // 避免 mojibake。真正的 GBK/Big5 中文几乎不可能是合法 UTF-8，不会误伤。
    let encoding = if guess == UTF_8 || std::str::from_utf8(sample).is_ok() {
        UTF_8
    } else {
        guess
    };

    (decode_with(encoding, bytes), encoding_name(encoding))
}

/// 剥离 BOM 并返回对应编码；无 BOM 返回 None。
fn strip_bom(bytes: &[u8]) -> Option<(&'static Encoding, &[u8])> {
    if bytes.starts_with(b"\xEF\xBB\xBF") {
        Some((UTF_8, &bytes[3..]))
    } else if bytes.starts_with(b"\xFF\xFE") {
        Some((UTF_16LE, &bytes[2..]))
    } else if bytes.starts_with(b"\xFE\xFF") {
        Some((UTF_16BE, &bytes[2..]))
    } else {
        None
    }
}

/// lossy 解码：非法字节序列替换为 U+FFFD。
fn decode_with(encoding: &'static Encoding, bytes: &[u8]) -> String {
    // decode_without_bom_handling 返回 (Cow<str>, had_replacements)
    let (cow, _replaced) = encoding.decode_without_bom_handling(bytes);
    cow.into_owned()
}

/// 统一编码命名：encoding_rs 的部分名字是小写（如 "gb18030"），
/// 这里归一对上层的固定字面量，其余走 encoding_rs 原名。
/// 注意：encoding_rs 的编码常量是 `&'static Encoding` 静态，不能直接做
/// match arm 模式（E0530 bindings cannot shadow statics），故用 if 链比较。
fn encoding_name(encoding: &'static Encoding) -> &'static str {
    if encoding == UTF_8 {
        "UTF-8"
    } else if encoding == GBK {
        "GBK"
    } else if encoding == GB18030 {
        "GB18030"
    } else if encoding == WINDOWS_1252 {
        "windows-1252"
    } else if encoding == UTF_16LE {
        "UTF-16LE"
    } else if encoding == UTF_16BE {
        "UTF-16BE"
    } else {
        encoding.name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_chinese() {
        let bytes = "你好，世界。这是 UTF-8 中文测试。".as_bytes();
        let (text, enc) = decode_bytes(bytes);
        assert_eq!(enc, "UTF-8");
        assert_eq!(text, "你好，世界。这是 UTF-8 中文测试。");
    }

    #[test]
    fn gbk_chinese() {
        // GBK 编码的中文：非法 UTF-8，chardetng 应检出 GBK/GB18030 系
        let raw = "你好，世界。这是 GBK 中文测试。";
        // str → GBK 字节流：Encoding::encode 返回的是 Cow<str>（output encoding），
        // 拿字节必须走 Encoder::encode_from_utf8_to_vec（last=true 表示输入到此为止）。
        // 坑：该方法写入 dst.spare_capacity_mut()，空 Vec 容量为 0 会静默写出
        // 0 字节——必须预分配（最坏 4 字节/字，GB18030 全角）
        let mut encoder = encoding_rs::GBK.new_encoder();
        let mut bytes = Vec::with_capacity(raw.len() * 4);
        let _ = encoder.encode_from_utf8_to_vec(raw, &mut bytes, true);
        assert!(!bytes.is_empty(), "编码输出为空（容量未预分配？）");
        assert!(std::str::from_utf8(&bytes).is_err(), "样本应不是合法 UTF-8");
        let (text, enc) = decode_bytes(&bytes);
        assert!(enc == "GBK" || enc == "GB18030", "实际检出: {enc}");
        assert_eq!(text, raw);
    }

    #[test]
    fn utf8_with_bom() {
        let mut bytes = b"\xEF\xBB\xBF".to_vec();
        bytes.extend_from_slice("你好，世界。".as_bytes());
        let (text, enc) = decode_bytes(&bytes);
        assert_eq!(enc, "UTF-8");
        assert_eq!(text, "你好，世界。");
    }

    #[test]
    fn empty_input() {
        let (text, enc) = decode_bytes(b"");
        assert_eq!(enc, "UTF-8");
        assert!(text.is_empty());
    }

    #[test]
    fn pure_ascii_is_utf8() {
        // chardetng 对纯英文常猜 windows-1252；UTF-8 strict 兜底应修正为 UTF-8
        let (text, enc) = decode_bytes(b"Hello world, plain ascii log line.\n");
        assert_eq!(enc, "UTF-8");
        assert_eq!(text, "Hello world, plain ascii log line.\n");
    }

    #[test]
    fn invalid_bytes_replaced_not_error() {
        // 直接验证 lossy 语义：非法 UTF-8 序列替换为 U+FFFD，不得 panic/Err
        assert_eq!(decode_with(UTF_8, b"abc\xFF"), "abc\u{FFFD}");
        assert_eq!(decode_with(UTF_8, b"\xE4\xBD"), "\u{FFFD}"); // 截断的三字节序列
                                                                 // 公开入口对二进制垃圾也不得 panic
        let (text, _) = decode_bytes(&[0x80, 0x81, 0x82, 0x83]);
        assert!(!text.is_empty());
    }
}
