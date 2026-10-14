# PDFium 二进制（不随仓库分发）

`lib/libpdfium.so`（Linux）与 `lib/pdfium.dll`（Windows）**不在 git 中**，
由 `scripts/fetch-binaries.sh` 从上游 Releases 获取：

- 上游：https://github.com/bblanchon/pdfium-binaries/releases（latest）
  - Linux x64：`pdfium-linux-x64.tgz` → `lib/libpdfium.so`
  - Windows x64：`pdfium-win-x64.tgz` → `lib/pdfium.dll`
- CI：`.github/workflows/ci.yml` 在构建/测试前自动调用该脚本
- 本地：`scripts/fetch-binaries.sh --platform all`

每次获取后，脚本会把 release tag 与两个文件的 SHA-256 写入
`PDFIUM-VERSION.txt`（该文件 gitignored，仅本地留痕）。

## 运行时搜索链

运行时（`stcore/src/extract/pdf.rs`）按以下顺序定位 PDFium：

1. 环境变量 `SEARCH_TOOL_PDFIUM_LIB`（显式指定 .so/.dll 全路径）
2. 可执行文件同级目录、`lib/`、`../lib/`
3. 源码树 `third_party/pdfium/lib/`（dev 模式自动命中）
4. 系统库搜索路径

打包时 `tauri.conf.json` 的 `bundle.resources` 将
`../third_party/pdfium/lib/*` 映射进 bundle 的 `lib/` 目录 —— 因此
**打包前也必须先跑 fetch 脚本**，否则 glob 空匹配。
