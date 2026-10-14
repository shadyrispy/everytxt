use cc::Build;
use std::{env, path::PathBuf};

fn main() {
    // PATCHED: 去掉 canonicalize() —— Windows 上 canonicalize 会返回
    // `\\?\` 扩展长度路径前缀，MSVC cl.exe 无法打开该类路径
    // （C1083: Cannot open source file: '\chm_lib.c'）。
    // CARGO_MANIFEST_DIR 本身即绝对路径，直接 join 即可；
    // cc-rs 不做内部 canonicalize，传入什么就原样传给编译器。
    let project_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = project_dir.join("vendor").join("CHMLib").join("src");

    let mut build = Build::new();
    // PATCHED: MSVC 只预定义 `_WIN32`，不定义 `WIN32`；CHMLib 的 Windows
    // 分支全部挂在 `#ifdef WIN32` 上（windows.h / malloc.h / ffs 回退），
    // 宏缺失时落入 POSIX 分支 include <unistd.h> → MSVC C1083。
    // MinGW gcc 已预定义 WIN32（-dM 实证），仅 MSVC 需要补；
    // 由 cargo 按 target 注入 CARGO_CFG_TARGET_ENV，交叉编译同样正确。
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        build.define("WIN32", "1");
    }

    build
        .file(src.join("chm_lib.c"))
        .file(src.join("lzx.c"))
        .include(&src)
        .warnings(false)
        .compile("chmlib");
}
