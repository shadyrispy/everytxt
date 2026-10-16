# ocr-helper（P3-A · OCR 通道）

**独立进程**的 OCR 辅助器：把 ONNX Runtime + PP-OCR 模型的重型依赖与主程序隔离。

## 为什么独立

| 原因 | 说明 |
|---|---|
| 工具链 | `oar-ocr 0.9.1` 要求 **rustc ≥ 1.95**；主工程固定 stable(1.93)。本目录 `rust-toolchain.toml` pin 1.95 并自带 `[workspace]`（脱离父 workspace） |
| 体积/内存 | ONNX Runtime 动态库 + 模型数十 MB，不进主二进制 |
| 稳定性 | 推理崩溃/内存峰值不波及索引进程（plan 风险 R2） |

实测（2026-10-14）：rustc 1.95 下 `oar-ocr 0.9.1` 编译通过（38 s）。

## 构建

```bash
cd ocr-helper
cargo build --release          # 首次会拉 ONNX Runtime 动态库
# 产物：target/release/ocr-helper
```

首次运行会从 ModelScope 自动下载模型（`pp-ocrv6_tiny_*`）到 `$OAR_HOME`（默认 `~/.oar`）。

## 协议（响应恒为 stdout 一行 JSON；请求二选一）

请求：

```json
{"image_path": "/abs/path/page.png"}
```

- **首选 argv**：`ocr-helper '{"image_path":"..."}'`（argv[1] 即请求 JSON）
- 回退 stdin：一行 JSON（手动调试用）

> 为什么首选 argv：Windows 上部分环境对命名管道「OUTBOUND 服务器 +
> GENERIC_READ 客户端」的客户端连接确定性返回 ERROR_PIPE_BUSY(231)
> （rustc 1.99 std `Command::spawn` 的 stdin 管道实现踩中；stdout 方向
> INBOUND + GEN_WRITE 不受影响）。故主程序（stcore）请求走 argv、响应走
> stdout 管道，全平台等价。

成功：

```json
{"ok": true, "lines": [{"text": "识别文本", "confidence": 0.98}]}
```

失败（退出码 1）：

```json
{"ok": false, "error": "..."}
```

## 手动验证

```bash
# argv（主程序同款路径）
./target/release/ocr-helper '{"image_path": "/tmp/scan.png"}'
# stdin 回退
echo '{"image_path": "/tmp/scan.png"}' | ./target/release/ocr-helper
```

## 接入状态

- [x] 独立 crate + 工具链隔离 + 协议定型
- [x] `oar-ocr` 编译验证（1.95）
- [ ] 主程序侧调用封装（`stcore`：扫描型 PDF / 图片 → helper → 文本）
- [ ] 模型分发策略确认（随包 vs 首次运行下载）
- [ ] OCR 线程限流（与索引互斥，plan §5.2 / R2）

## 与 main 的边界

helper **不做**索引、不碰 tantivy；只负责"图片 → 文本行"。命中 bbox 叠层（P3-B）走
PDFium 通道（文字型 PDF 已有 `page_text_boxes`），扫描型 PDF 的 bbox 需本 helper
扩展返回 box 字段（当前版本只回文本 + 置信度）。
