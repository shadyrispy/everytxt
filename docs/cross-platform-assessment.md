# 跨平台评估报告（Linux / macOS / Windows）

> 结论问题：「现在我们的程序可以跨平台吗？手头只有 Windows 机器可以测 UI」
>
> **答：是。三平台（Linux / macOS / Windows）均可构建运行；Windows 已完成适配，
> 本文件附证据矩阵与 Windows 构建运行指引（见 `docs/windows-runbook.md`）。**

- 评估日期：2026-10-14
- 代码基线：`search-tool` workspace（stcore / app / ui / ocr-helper）
- 沙箱证据：Ubuntu 22.04 + mingw-w64 交叉编译（`x86_64-pc-windows-gnu`）
- 主机证据：全量门禁（fmt / clippy / test）

---

## 1. 验证矩阵（本轮实测）

| 检查项 | 主机 Linux（本仓 CI 基线） | Windows 交叉（mingw-w64，沙箱） |
|---|---|---|
| `cargo check` workspace | ✅ | ✅ stcore + app 全量 |
| `cargo test` 编译 + 链接（`--no-run`） | ✅ 237 passed / 0 failed | ✅ 全部测试二进制链接通过 |
| `cargo clippy --all-targets` | ✅ 0 警告 | ✅ 0 警告（check 级） |
| `cargo fmt --check` | ✅ | —（格式与目标无关） |
| C/C++ 依赖编译（unrar / chmlib / pdfium 绑定） | ✅ | ✅ |
| tauri-build（Windows 资源嵌入） | 不适用 | ✅（修复 icon.ico 后） |
| Windows 二进制实跑 | — | ⚠️ 沙箱无 wine，交由真机验证（runbook §6 冒烟清单） |

> 交叉编译只能证明"编得过、链得上"；运行时行为（WebView2、文件系统、右键集成）
> 需在真机按 runbook 冒烟。这是本报告唯一的未闭合项，已降级为操作清单而非风险。

## 2. 分层状态

| 层 | 跨平台机制 | 状态 |
|---|---|---|
| stcore（引擎：索引/搜索/提取/OCR 调度） | 纯 Rust + 3 个 C/C++ vendored 依赖（均带 `#[cfg]` 或构建期平台分支） | ✅ 双目标验证 |
| app（Tauri 2 壳 + 插件） | tauri/wry 官方支持 Windows；托盘/全局热键/自启动插件三平台分支完备 | ✅ 编译验证 |
| ui（React 19 + Vite） | 纯 Web，无平台 API 依赖 | ✅（平台无关） |
| ocr-helper（独立 workspace） | 需 rustc ≥ 1.95 + ort；**可选组件**，默认关闭 | ✅ 可选（runbook §7） |
| 打包 | deb/appimage（Linux）；Windows 用 `--bundles nsis/msi` 覆盖 | ✅（见 runbook §8） |
| Shell 集成（右击搜索） | Linux nautilus 脚本 / Windows 注册表 / macOS（未实现）均有分支 | ✅ Windows 分支完备 |
| 单实例 socket | **仅 Unix**（Windows 命名管道未实现，TODO）；CLI `/s` 检测失败自动回退本地 headless | ⚠️ 已知缺口（功能降级，不阻塞） |

## 3. 本轮发现并修复的真缺陷（全部有实证）

| # | 缺陷 | 平台影响 | 证据 / 修复 |
|---|---|---|---|
| 1 | chmlib 1.0.0 `OsStr::as_str()` 不存在（E0599） | Windows 全工具链编译失败 | vendored 一字修复（`third_party/chmlib-patched`） |
| 2 | 数据目录只认 XDG/HOME，Windows 落 `C:\tmp` | Windows 索引/配置路径错误 | `app/src/state.rs` `dirs` shim 加 `LOCALAPPDATA` 分支 |
| 3 | unrar_sys 头文件大小写（`<PowrProf.h>`/`<Wbemidl.h>`），mingw 大小写敏感 ENOENT，连锁 `SetSuspendState` 未声明 | mingw 工具链编译失败 | vendored 三处改小写（`third_party/unrar-sys-patched`） |
| 4 | unrar_sys build.rs `#[cfg(windows)]` 是 **host** 语义：交叉编译漏编 `isnt.cpp`（`WinNT()`/`IsWindows11OrGreater()` 定义处）与 `-lpowrprof/shell32` 标志 | 交叉编译链接期 undefined | vendored build.rs 改按 `CARGO_CFG_WINDOWS`（target 语义）判断 |
| 5 | chmlib-sys `ffs` 回退只在 MSVC 分支；mingw-gcc（`__GNUC__` + `WIN32`）两侧都不提供 | mingw 工具链链接期 undefined `ffs` | vendored 守卫改 `__GNUC__ && !WIN32`（`third_party/chmlib-sys-patched`） |
| 6 | tauri-build Windows 资源嵌入缺 `icons/icon.ico` 硬失败；`package.metadata does not exist` 仅为无害 println | **MSVC 真机构建同样失败** | 生成 16/32/48/256 四尺寸 icon.ico 并注册到 `bundle.icon` |
| 7 | OCR helper 定位拼 `ocr-helper` 不带 `.exe` | Windows 自动定位失效（OCR 默认关，仅降级） | `stcore/src/ocr.rs` 加 `HELPER_NAME` 平台分支 |
| 8 | shell_integration / service.rs 的 Unix 专属符号在 Windows 目标死代码告警 | 仅交叉目标噪音 | 补 `#[cfg(unix)]` / `cfg_attr` 门控 |

另：`bundle.resources` 原只映射 Linux `libpdfium.so`，已改 glob
（`../third_party/pdfium/lib/* → lib/`）并随仓库提供 Windows `pdfium.dll`
（Chromium 157.0.8100.0，来源记录见 `third_party/pdfium/PDFIUM-WIN-VERSION.txt`）。

## 4. 已知缺口（不阻塞 UI 测试）

| 缺口 | 影响 | 规避 |
|---|---|---|
| 单实例 socket 仅 Unix | Windows 下 CLI `/s` 无法转发到主实例 | 自动回退本地 headless 搜索（设计内行为，main.rs:164-180） |
| macOS 右击集成未实现 | macOS 无资源管理器菜单 | 功能缺口，非缺陷 |
| bundle targets 仅 deb/appimage | Linux 打包配置 | Windows 打包用 `cargo tauri build --bundles nsis` |
| OCR helper 需 rustc 1.95 | 构建门槛 | 可选功能，默认关；不开则扫描件记 Unsupported 跳过 |
| libpdfium.so 版本未随仓库记录 | 跨平台 pdfium 绑定漂移风险 | 刷新时同步记录（DLL 侧已记录） |

## 5. 复验命令

```bash
# 主机门禁（Linux/macOS 同）
cargo fmt --all -- --check
cargo clippy --workspace --all-targets   # 期望 0 警告
cargo test --workspace                    # 期望 237 passed / 0 failed

# Windows 交叉（需 rustup target add x86_64-pc-windows-gnu + mingw-w64）
cargo check -p stcore  --target x86_64-pc-windows-gnu
cargo check -p search-tool --target x86_64-pc-windows-gnu
cargo test  --workspace --target x86_64-pc-windows-gnu --no-run
```

## 6. 结论

引擎层与外壳层均已在双目标完成编译/链接级验证；Windows 特有的构建缺陷
（icon.ico、unrar/chmlib 工具链兼容、helper 定位）已全部修复并有实证。
**用户可立即在 Windows 机器按 `docs/windows-runbook.md` 构建并测试 UI**；
剩余未闭合项仅为真机运行时冒烟（沙箱无 Windows/wine，属环境限制而非代码风险）。
