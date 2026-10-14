#!/usr/bin/env bash
# GitHub 资源下载（国内可达通道）
#
# 实测（2026-10-14，本机网络）：
#   ✅ https://gh-proxy.com/https://github.com/...   （GitHub 网页/raw/releases/archive）
#   ✅ https://codeload.github.com/<owner>/<repo>/zip/refs/heads/<branch>  （zip 直连）
#   ❌ ghfast.top / ghproxy.net（403）、mirror.ghproxy.com、kkgithub、bgithub（不可达）
#
# 用法：
#   ./scripts/fetch-gh.sh archive <owner>/<repo> [branch] [out.zip]
#   ./scripts/fetch-gh.sh raw <owner>/<repo>/<branch>/<path> [out.file]
#   ./scripts/fetch-gh.sh release <owner>/<repo>/<asset-file> [out.file]   # latest release 附件
#   ./scripts/fetch-gh.sh url <任意 github URL> [out.file]
#
# 说明：npm 包走 https://registry.npmmirror.com；Rust crate 走 ustc 镜像（Cargo 已配）。

set -euo pipefail

GH_PROXY="https://gh-proxy.com"
try() {  # try <url> <out>
  local url="$1" out="$2"
  echo "→ $url"
  if curl -fsSL --max-time 300 -o "$out" "$url"; then
    echo "✅ 已保存 $out ($(du -h "$out" | cut -f1))"
    return 0
  fi
  echo "✗ 失败"
  return 1
}

cmd="${1:-}"; shift || true

case "$cmd" in
  archive)
    repo="${1:?owner/repo}"; branch="${2:-main}"; out="${3:-$(basename "$repo")-$branch.zip}"
    try "$GH_PROXY/https://github.com/$repo/archive/refs/heads/$branch.zip" "$out" \
      || try "https://codeload.github.com/$repo/zip/refs/heads/$branch" "$out"
    ;;
  raw)
    path="${1:?owner/repo/branch/path}"; out="${2:-$(basename "$path")}"
    try "$GH_PROXY/https://raw.githubusercontent.com/$path" "$out" \
      || try "https://raw.githubusercontent.com/$path" "$out"
    ;;
  release)
    asset="${1:?owner/repo/asset}"; out="${2:-$(basename "$asset")}"
    try "$GH_PROXY/https://github.com/$asset" "$out" \
      || try "https://github.com/$asset" "$out"
    ;;
  url)
    url="${1:?github url}"; out="${2:-$(basename "${url%%\?*}")}"
    case "$url" in
      https://github.com/*) try "$GH_PROXY/$url" "$out" || try "$url" "$out" ;;
      *) try "$url" "$out" ;;
    esac
    ;;
  *)
    sed -n '2,20p' "$0"
    exit 2
    ;;
esac
