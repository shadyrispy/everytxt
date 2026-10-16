//! OCR helper（P3-A / P4-A / P5）：**独立进程**，把重型推理依赖（ONNX Runtime +
//! PP-OCR 模型）与主程序隔离。
//!
//! ## 为什么独立进程
//! - `oar-ocr` 要求 rustc ≥ 1.95（主工程 stable 1.93）→ 工具链隔离（`rust-toolchain.toml`）
//! - ONNX Runtime 动态库与模型不进主二进制，主程序体积/内存不受影响
//! - 推理崩溃/内存峰值（plan 风险 R2）不波及索引进程
//!
//! ## 模型来源（按优先级；P5 起随安装包分发）
//! 1. 环境变量 `SEARCH_TOOL_OCR_MODEL_DIR`
//! 2. 可执行文件同级 `models/`（打包态，见 `tauri.conf.json` resources）
//! 3. 源码树 `third_party/ocr-models/`
//! 4. 以上皆无 → 退回注册名（`auto-download` 从 ModelScope 拉取到 `$OAR_HOME`）
//!
//! ## 协议（响应恒为 stdout 一行 JSON；请求二选一）
//! 请求：`{"image_path": "/abs/path/page.png"}`
//! - **首选 argv**：`ocr-helper '{"image_path":"..."}'`（argv[1] 即请求 JSON）
//! - 回退 stdin：一行 JSON（`echo '{...}' | ocr-helper`，手动调试用）
//!
//! 为什么首选 argv：Windows 上 Rust `Command::spawn` 的 stdin 管道
//! （OUTBOUND 服务器 + GENERIC_READ 客户端）在部分机器上确定性返回
//! ERROR_PIPE_BUSY(231)；stdout 管道（INBOUND + GENERIC_WRITE）不受影响，
//! 故请求走 argv、响应走 stdout。
//! 成功：`{"ok":true,"image_width":900,"image_height":300,
//!        "lines":[{"text":"...","confidence":0.99,"box":[x,y,w,h]}]}`（box 为像素坐标）
//! 失败（退出码 1）：`{"ok":false,"error":"..."}`
//!
//! ## 构建（需 1.95 工具链，本目录已 pin）
//! ```bash
//! cd ocr-helper && cargo build --release
//! ```

use anyhow::{anyhow, Result};
use oar_ocr::prelude::*;
use serde::{Deserialize, Serialize};
use std::io::BufRead;
use std::path::{Path, PathBuf};

/// 模型档位：默认 tiny（plan §5.2 桌面默认档，RSS ~0.5GB）
const DET_MODEL: &str = "pp-ocrv6_tiny_det.onnx";
const REC_MODEL: &str = "pp-ocrv6_tiny_rec.onnx";
const DICT_FILE: &str = "ppocrv6_tiny_dict.txt";

#[derive(Debug, Deserialize)]
struct Request {
    image_path: String,
}

#[derive(Debug, Serialize)]
struct Line {
    text: String,
    confidence: f32,
    /// 像素坐标 [x, y, w, h]（左上原点；stcore 侧归一化）
    #[serde(rename = "box")]
    bbox: [f32; 4],
}

#[derive(Debug, Serialize)]
struct Response {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    lines: Option<Vec<Line>>,
    image_width: u32,
    image_height: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn main() {
    let req = match read_request() {
        Ok(r) => r,
        Err(e) => {
            print_json(&Response {
                ok: false,
                lines: None,
                image_width: 0,
                image_height: 0,
                error: Some(e.to_string()),
            });
            std::process::exit(1);
        }
    };

    match run(&req) {
        Ok((lines, w, h)) => print_json(&Response {
            ok: true,
            lines: Some(lines),
            image_width: w,
            image_height: h,
            error: None,
        }),
        Err(e) => {
            print_json(&Response {
                ok: false,
                lines: None,
                image_width: 0,
                image_height: 0,
                error: Some(e.to_string()),
            });
            std::process::exit(1);
        }
    }
}

fn read_request() -> Result<Request> {
    // 首选 argv[1]：请求 JSON 字符串（规避 Windows stdin 管道 231 问题）
    if let Some(arg) = std::env::args().nth(1) {
        let arg = arg.trim();
        if !arg.is_empty() {
            return serde_json::from_str::<Request>(arg)
                .map_err(|e| anyhow!("argv 请求解析失败: {e}"));
        }
    }
    // 回退 stdin：一行 JSON（手动调试协议）
    let stdin = std::io::stdin();
    let mut buf = String::new();
    stdin.lock().read_line(&mut buf)?;
    let buf = buf.trim();
    if buf.is_empty() {
        return Err(anyhow!("空请求：期望一行 JSON"));
    }
    Ok(serde_json::from_str::<Request>(buf)?)
}

fn print_json(resp: &Response) {
    match serde_json::to_string(resp) {
        Ok(s) => println!("{s}"),
        Err(e) => println!(r#"{{"ok":false,"error":"序列化失败: {e}"}}"#),
    }
}

/// 三个候选目录中第一个"三件套齐全"的；都没有则返回注册名（auto-download 回退）
fn resolve_models() -> (String, String, String) {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(d) = std::env::var("SEARCH_TOOL_OCR_MODEL_DIR") {
        dirs.push(PathBuf::from(d));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(p) = exe.parent() {
            dirs.push(p.join("models"));
            dirs.push(p.join("..").join("lib").join("search-tool").join("models"));
            dirs.push(p.join("..").join("share").join("search-tool").join("models"));
        }
    }
    dirs.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("third_party")
            .join("ocr-models"),
    );

    for d in dirs {
        let (a, b, c) = (d.join(DET_MODEL), d.join(REC_MODEL), d.join(DICT_FILE));
        if a.is_file() && b.is_file() && c.is_file() {
            return (
                a.to_string_lossy().into_owned(),
                b.to_string_lossy().into_owned(),
                c.to_string_lossy().into_owned(),
            );
        }
    }
    (
        DET_MODEL.to_string(),
        REC_MODEL.to_string(),
        DICT_FILE.to_string(),
    )
}

fn run(req: &Request) -> Result<(Vec<Line>, u32, u32)> {
    let path = PathBuf::from(&req.image_path);
    if !path.is_file() {
        return Err(anyhow!("图像不存在: {}", path.display()));
    }

    let (det, rec, dict) = resolve_models();
    let ocr = OAROCRBuilder::new(det, rec, dict).build()?;

    let image = load_image(Path::new(&req.image_path))?;
    let (iw, ih) = (image.width(), image.height());
    let results = ocr.predict(vec![image])?;

    let mut lines = Vec::new();
    for page in &results {
        for region in &page.text_regions {
            let Some((text, confidence)) = region.text_with_confidence() else {
                continue;
            };
            if text.trim().is_empty() {
                continue;
            }
            let bb = &region.bounding_box;
            let (x, y) = (bb.x_min(), bb.y_min());
            lines.push(Line {
                text: text.to_string(),
                confidence,
                bbox: [x, y, bb.x_max() - x, bb.y_max() - y],
            });
        }
    }
    Ok((lines, iw, ih))
}
