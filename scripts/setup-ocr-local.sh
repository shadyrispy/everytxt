#!/usr/bin/env bash
# 本机（Windows + 中国网络）搭建 OCR 通道的一键脚本。
#
# 背景：本机直连 static.rust-lang.org / cdn.pyke.io 均不稳定（超时或长时间卡死，
# cdn.pyke.io 还会中途断连重置），故全部走可用镜像/断点续传。
#
# 用法：bash scripts/setup-ocr-local.sh
#
# 覆盖的坑：
#  1. rustup 1.95 工具链：改走 USTC 镜像（官方源实测 20+ 分钟零进度）
#  2. crates.io：改走 USTC sparse 镜像（见 CARGO_HOME/config.toml）
#  3. ONNX Runtime 原生库：cdn.pyke.io 断点续传 + SHA-256 校验，
#     落盘后用 ORT_LIB_LOCATION 指给 ort-sys，绕过其构建期下载
#  4. rustup show/show 类网络命令会卡住整个 && 链：本脚本不做无谓探测

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUST_BIN="$HOME/.workbuddy/binaries/rust/cargo/bin"
export PATH="$RUST_BIN:$PATH"
export RUSTUP_HOME="$HOME/.workbuddy/binaries/rust/rustup"
export CARGO_HOME="$HOME/.workbuddy/binaries/rust/cargo"
export RUSTUP_DIST_SERVER="https://mirrors.ustc.edu.cn/rust-static"
export RUSTUP_UPDATE_ROOT="https://mirrors.ustc.edu.cn/rust-static/rustup"

ORT_DIR="$ROOT/third_party/onnxruntime"
ORT_VERSION="1.28.0"
ORT_ASSET="x86_64-pc-windows-msvc+directml"
# 与 ort-sys 2.0.0-rc.13 build/download/dist.tsv 登记的 SHA-256 一致
ORT_SHA="f7c654b3729cb9e5ad2a36a0c38e5b48e63bf4eed22968931aed33a0ad0b527d"
ORT_URL="https://cdn.pyke.io/0/pyke:ort-rs/ms@${ORT_VERSION}/${ORT_ASSET}.tar.lzma2"

echo "== [1/3] 确保 rustc 1.95 工具链（USTC 镜像）=="
if rustc +1.95 --version >/dev/null 2>&1; then
  echo "   已就绪：$(rustc +1.95 --version)"
else
  rustup toolchain install 1.95 --profile minimal
fi

echo "== [2/3] 获取 ONNX Runtime 原生库（断点续传，需 ~30MB）=="
mkdir -p "$ORT_DIR"
ARCHIVE="$ORT_DIR/ort-ms.tar.lzma2"
TARGET_SIZE=31076494

if [ -f "$ORT_DIR/lib/onnxruntime.dll" ] && [ "$(sha256sum "$ORT_DIR/.archive.sha256" 2>/dev/null | cut -d' ' -f1)" != "" ]; then
  echo "   已存在 $ORT_DIR/lib/onnxruntime.dll，跳过（删除该目录可强制重取）"
else
  for i in $(seq 1 80); do
    SZ=$(stat -c%s "$ARCHIVE" 2>/dev/null || echo 0)
    if [ "$SZ" -ge "$TARGET_SIZE" ]; then echo "   下载完成（第 $i 次尝试）"; break; fi
    echo "   [尝试 $i] 已 $(( SZ / 1024 / 1024 ))MB，续传…"
    curl -fSL -C - --connect-timeout 20 --max-time 600 \
      --speed-time 30 --speed-limit 1024 -o "$ARCHIVE" "$ORT_URL" 2>/dev/null || true
    sleep 2
  done

  GOT=$(sha256sum "$ARCHIVE" | cut -d' ' -f1)
  if [ "$GOT" != "$ORT_SHA" ]; then
    echo "   ✗ SHA-256 不匹配: got=$GOT want=$ORT_SHA" >&2
    exit 1
  fi
  echo "   ✓ SHA-256 校验通过"

  # lzma2 单文件，用 tar 解；解出的层级不确定，统一 find 定位
  rm -rf "$ORT_DIR/extract" && mkdir -p "$ORT_DIR/extract"
  tar -xf "$ARCHIVE" -C "$ORT_DIR/extract"
  DLL=$(find "$ORT_DIR/extract" -name "onnxruntime.dll" -type f | head -1)
  if [ -z "$DLL" ]; then echo "   ✗ 归档内未找到 onnxruntime.dll" >&2; exit 1; fi
  mkdir -p "$ORT_DIR/lib"
  cp "$DLL" "$ORT_DIR/lib/onnxruntime.dll"
  cp "$(dirname "$DLL")"/onnxruntime.lib "$ORT_DIR/lib/" 2>/dev/null || true
  echo "$ORT_SHA" > "$ORT_DIR/.archive.sha256"
  rm -rf "$ORT_DIR/extract"
  echo "   ✓ 已就位 $ORT_DIR/lib/onnxruntime.dll"
fi

echo "== [3/3] 构建 ocr-helper（release，ORT_LIB_LOCATION 指向本地库）=="
cd "$ROOT/ocr-helper"
export ORT_LIB_LOCATION="$ORT_DIR/lib"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
cargo +1.95 build --release
echo "   ✓ 产物：$ROOT/ocr-helper/target/release/ocr-helper.exe"

echo
echo "全部完成。启用 OCR 通道："
echo "  export SEARCH_TOOL_OCR=1"
echo "  export SEARCH_TOOL_OCR_HELPER=$ROOT/ocr-helper/target/release/ocr-helper.exe"
echo "（helper 亦可自动定位源码树路径，见 stcore/src/ocr.rs）"
