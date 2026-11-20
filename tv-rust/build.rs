fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");

    // 只有 Windows 目标需要 PE 资源；Linux/macOS 构建直接跳过。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let icon = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/icon.ico");
    let mut res = winres::WindowsResource::new();
    res.set_icon(icon.to_str().expect("icon path is valid UTF-8"));
    res.set("FileDescription", "TV Rust - TV-Multiplatform");
    res.set("ProductName", "TV Rust");
    res.set("OriginalFilename", "TVRust.exe");
    res.compile().expect("failed to compile Windows resources");

    // winres 的 `cargo:rustc-link-lib=static=resource` 在 GNU 目标下可能因静态库
    // 没有被任何符号引用而被链接器丢弃（表现为 exe 没有 .rsrc 节、快捷方式仍是默认图标）。
    // 直接把资源对象文件交给最终链接器，确保 .rsrc 一定进入 PE。
    // MSVC 目标由 winres 生成 resource.lib，走它自己的链接参数即可。
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
        let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo");
        let resource_obj = std::path::Path::new(&out_dir).join("resource.o");
        println!("cargo:rustc-link-arg={}", resource_obj.display());
    }
}
