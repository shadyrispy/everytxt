#!/usr/bin/env bash
# 第三方二进制获取脚本 —— 本仓库不内置任何第三方二进制（PDFium / OCR 模型），
# 构建或测试前必须先运行本脚本。CI 中由 .github/workflows/ci.yml 调用。
#
# 来源与校验：
#   PDFium   bblanchon/pdfium-binaries GitHub Releases（latest）
#            linux x64 → pdfium-linux-x64.tgz   → third_party/pdfium/lib/libpdfium.so
#            linux arm → pdfium-linux-arm64.tgz → third_party/pdfium/lib/libpdfium.so
#            windows   → pdfium-win-x64.tgz     → third_party/pdfium/lib/pdfium.dll
#   OCR 模型 ModelScope greatv/oar-ocr（oar-ocr-core download::REGISTRY 登记件）
#            pp-ocrv6_tiny_det.onnx / pp-ocrv6_tiny_rec.onnx / ppocrv6_tiny_dict.txt
#            下载后按 registry 中的 SHA-256 强校验，不一致即失败。
#
# 用法：
#   scripts/fetch-binaries.sh                    # 按当前平台取所需文件
#   scripts/fetch-binaries.sh --platform linux   # 仅 Linux（libpdfium.so）
#   scripts/fetch-binaries.sh --platform windows # 仅 Windows（pdfium.dll）
#   scripts/fetch-binaries.sh --platform all     # 两平台 + 模型全取
#   scripts/fetch-binaries.sh --force            # 忽略已存在文件，强制重取
#
# 国内网络 GitHub 直连失败时，走代理前缀（CI 中不需要）：
#   GH_PROXY=https://gh-proxy.com scripts/fetch-binaries.sh

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PDFIUM_LIB_DIR="$ROOT/third_party/pdfium/lib"
MODELS_DIR="$ROOT/third_party/ocr-models"
mkdir -p "$PDFIUM_LIB_DIR" "$MODELS_DIR"

PLATFORM="auto"
FORCE=0
while [ $# -gt 0 ]; do
  case "$1" in
    --platform) PLATFORM="${2:?--platform 需要 linux|windows|all}"; shift 2 ;;
    --force) FORCE=1; shift ;;
    *) echo "未知参数: $1" >&2; exit 2 ;;
  esac
done

if [ "$PLATFORM" = "auto" ]; then
  case "$(uname -s)" in
    Linux*) PLATFORM="linux" ;;
    MINGW*|MSYS*|CYGWIN*) PLATFORM="windows" ;;
    *) echo "无法识别平台，请显式指定 --platform linux|windows|all" >&2; exit 2 ;;
  esac
fi

# GitHub URL；设置了 GH_PROXY 则走代理前缀
gh_url() {  # gh_url <path-after-github.com>
  local p="$1"
  if [ -n "${GH_PROXY:-}" ]; then
    echo "${GH_PROXY%/}/https://github.com/$p"
  else
    echo "https://github.com/$p"
  fi
}

PDFIUM_REPO="bblanchon/pdfium-binaries"
MODELSCOPE_BASE="https://www.modelscope.cn/models/greatv/oar-ocr/resolve/master"

# name → sha256（与 oar-ocr-core 0.9.x download::REGISTRY 一致）
model_sha256() {
  case "$1" in
    pp-ocrv6_tiny_det.onnx) echo "193bab7a04fca699a6c82e6abb5b81bdb28177f0abd4062552b04908dafb19f8" ;;
    pp-ocrv6_tiny_rec.onnx) echo "9ef676d6ed3c88256a2d92c640c44f25b0c40947e111b14b8be8f594091563e6" ;;
    ppocrv6_tiny_dict.txt)  echo "c5cbe34ef40c29c4df07ed012bf96569cb69a2d2a01a07027e9f13cb832bd9cd" ;;
    *) echo "未知模型件: $1" >&2; return 1 ;;
  esac
}

fetch_file() {  # fetch_file <url> <out>
  local url="$1" out="$2"
  echo "→ $url"
  curl -fSL --retry 3 --connect-timeout 20 --max-time 600 -o "$out" "$url"
}

# 已存在且哈希匹配则跳过
have_ok() {  # have_ok <file> <sha256>
  [ -f "$1" ] && [ "$(sha256sum "$1" | cut -d' ' -f1)" = "$2" ]
}

fetch_models() {
  for name in pp-ocrv6_tiny_det.onnx pp-ocrv6_tiny_rec.onnx ppocrv6_tiny_dict.txt; do
    local out="$MODELS_DIR/$name" want
    want="$(model_sha256 "$name")"
    if [ "$FORCE" = "0" ] && have_ok "$out" "$want"; then
      echo "✅ $name 已存在且校验通过，跳过"
      continue
    fi
    fetch_file "$MODELSCOPE_BASE/$name" "$out"
    local got
    got="$(sha256sum "$out" | cut -d' ' -f1)"
    if [ "$got" != "$want" ]; then
      echo "✗ $name SHA-256 不匹配: got=$got want=$want" >&2
      exit 1
    fi
    echo "✅ $name 校验通过 ($(du -h "$out" | cut -f1))"
  done
}

fetch_pdfium() {  # fetch_pdfium <linux|windows>
  local plat="$1" asset out_name
  if [ "$plat" = "linux" ]; then
    case "$(uname -m)" in
      arm64|aarch64) asset="pdfium-linux-arm64.tgz" ;;
      *)              asset="pdfium-linux-x64.tgz" ;;
    esac
    out_name="libpdfium.so"
  else
    asset="pdfium-win-x64.tgz"; out_name="pdfium.dll"
  fi
  local out="$PDFIUM_LIB_DIR/$out_name"
  # 无强哈希源，已存在即跳过（--force 重取）；版本登记见 PDFIUM-VERSION.txt
  if [ "$FORCE" = "0" ] && [ -s "$out" ]; then
    echo "✅ $out_name 已存在，跳过（--force 可强制刷新）"
    return
  fi
  local tmp
  tmp="$(mktemp -d)"
  fetch_file "$(gh_url "$PDFIUM_REPO/releases/latest/download/$asset")" "$tmp/$asset"
  tar xzf "$tmp/$asset" -C "$tmp"
  local found
  found="$(find "$tmp" -name "$out_name" -type f | head -1)"
  if [ -z "$found" ]; then
    echo "✗ 归档内未找到 $out_name" >&2
    rm -rf "$tmp"
    exit 1
  fi
  cp "$found" "$out"
  rm -rf "$tmp"
  echo "✅ $out_name 已就位 ($(du -h "$out" | cut -f1))"
}

write_version_note() {
  # 记录 release tag（best-effort，失败不影响主流程）
  local tag="unknown"
  tag="$(curl -fsSL --max-time 30 -o /dev/null -w '%{url_effective}' \
    "$(gh_url "$PDFIUM_REPO/releases/latest")" 2>/dev/null \
    | sed 's#.*/tag/##' || true)"
  {
    echo "# 由 scripts/fetch-binaries.sh 生成 —— 请勿手工编辑"
    echo "# 生成时间: $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
    echo "# pdfium release: ${tag:-unknown} (bblanchon/pdfium-binaries)"
    echo "# OCR 模型: ModelScope greatv/oar-ocr (SHA-256 校验通过)"
    echo "libpdfium.so  sha256: $(sha256sum "$PDFIUM_LIB_DIR/libpdfium.so" 2>/dev/null | cut -d' ' -f1 || echo '-')"
    echo "pdfium.dll    sha256: $(sha256sum "$PDFIUM_LIB_DIR/pdfium.dll" 2>/dev/null | cut -d' ' -f1 || echo '-')"
  } > "$ROOT/third_party/pdfium/PDFIUM-VERSION.txt"
}

echo "平台: $PLATFORM (force=$FORCE)"
case "$PLATFORM" in
  linux)   fetch_pdfium linux;   fetch_models ;;
  windows) fetch_pdfium windows; fetch_models ;;
  all)     fetch_pdfium linux;   fetch_pdfium windows; fetch_models ;;
  *) echo "未知平台: $PLATFORM" >&2; exit 2 ;;
esac
write_version_note
echo "✅ 全部就位: $PDFIUM_LIB_DIR / $MODELS_DIR"
