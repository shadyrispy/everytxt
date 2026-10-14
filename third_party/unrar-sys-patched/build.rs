fn main() {
    // PATCHED: 上游 `cfg!(windows)` / `#[cfg(windows)]` 是 build script 自身
    // （host）语义，交叉编译（host≠target，如 Linux→Windows）时漏判：
    // 1) isnt.cpp 不被编译，但 pathfn/timefn 引用其定义的 WinNT() /
    //    IsWindows11OrGreater() → 链接期 undefined reference；
    // 2) -lpowrprof / shell32 链接标志不注入。
    // 改为按 cargo TARGET 判断（CARGO_CFG_* 由 cargo 按目标平台注入 build script）。
    let target_windows = std::env::var_os("CARGO_CFG_WINDOWS").is_some();
    if target_windows {
        println!("cargo:rustc-flags=-lpowrprof");
        println!("cargo:rustc-link-lib=shell32");
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
            println!("cargo:rustc-link-lib=pthread");
        }
    } else {
        println!("cargo:rustc-link-lib=pthread");
    }
    let mut names: Vec<&str> = vec![
        "strlist",
        "strfn",
        "pathfn",
        "smallfn",
        "global",
        "file",
        "filefn",
        "filcreat",
        "archive",
        "arcread",
        "unicode",
        "system",
        "crypt",
        "crc",
        "rawread",
        "encname",
        "match",
        "timefn",
        "rdwrfn",
        "consio",
        "options",
        "errhnd",
        "rarvm",
        "secpassword",
        "rijndael",
        "getbits",
        "sha1",
        "sha256",
        "blake2s",
        "hash",
        "extinfo",
        "extract",
        "volume",
        "list",
        "find",
        "unpack",
        "headers",
        "threadpool",
        "rs16",
        "cmddata",
        "ui",
        "filestr",
        "scantree",
        "dll",
        "qopen",
    ];
    // isnt.cpp 定义 WinNT()/IsWindows11OrGreater()（pathfn/timefn 引用），
    // Windows 目标必须编入（原生 Windows 构建上游已编，交叉编译补上）。
    if target_windows {
        names.push("isnt");
    }
    let files: Vec<String> = names
        .iter()
        .map(|&s| format!("vendor/unrar/{s}.cpp"))
        .collect();
    cc::Build::new()
        .cpp(true) // Switch to C++ library compilation.
        .opt_level(2)
        .std("c++14")
        // by default cc crate tries to link against dynamic stdlib, which causes problems on windows-gnu target
        .cpp_link_stdlib(None)
        .warnings(false)
        .extra_warnings(false)
        .flag_if_supported("-stdlib=libc++")
        .flag_if_supported("-fPIC")
        .flag_if_supported("-Wno-switch")
        .flag_if_supported("-Wno-parentheses")
        .flag_if_supported("-Wno-macro-redefined")
        .flag_if_supported("-Wno-dangling-else")
        .flag_if_supported("-Wno-logical-op-parentheses")
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-unused-variable")
        .flag_if_supported("-Wno-unused-function")
        .flag_if_supported("-Wno-missing-braces")
        .flag_if_supported("-Wno-unknown-pragmas")
        .flag_if_supported("-Wno-deprecated-declarations")
        .define("_FILE_OFFSET_BITS", Some("64"))
        .define("_LARGEFILE_SOURCE", None)
        .define("RAR_SMP", None)
        .define("RARDLL", None)
        .files(&files)
        .compile("libunrar.a");
}
