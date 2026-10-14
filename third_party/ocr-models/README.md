# OCR 模型（不随仓库分发）

`pp-ocrv6_tiny_det.onnx` / `pp-ocrv6_tiny_rec.onnx` / `ppocrv6_tiny_dict.txt`
**不在 git 中**，由 `scripts/fetch-binaries.sh` 从 ModelScope
[`greatv/oar-ocr`](https://www.modelscope.cn/models/greatv/oar-ocr) 获取，
并按 oar-ocr-core `download::REGISTRY` 登记的 SHA-256 强校验。

- 本地：`scripts/fetch-binaries.sh`（模型部分三平台通用）
- CI：`.github/workflows/ci.yml` 自动调用
- 运行时：`ocr-helper` 从本目录读取模型（见其 README）；
  缺模型时 oar-ocr 亦可自动从 ModelScope 下载到 `$OAR_HOME`
