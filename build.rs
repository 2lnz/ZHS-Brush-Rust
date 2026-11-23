//! 构建脚本：给 Windows exe 嵌图标与版本信息。
//!
//! Windows 下用 `windres`（binutils）给 exe 嵌图标与版本信息；
//! `windres.exe` 需要在 PATH 上（MinGW/MSYS2 的 binutils 自带）。
//! 找不到 windres 时只发个警告、不中断编译，保证没有它也能出 exe（只是没图标）。

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    {
        if let Err(e) = embed_icon() {
            println!("cargo:warning=exe 图标嵌入失败（不影响功能）：{e}");
        }
    }
}

#[cfg(windows)]
fn embed_icon() -> Result<(), Box<dyn std::error::Error>> {
    let mut res = winres::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    res.set("ProductName", "ZHS-Brush-RUST");
    res.set("FileDescription", "ZHS-Brush-RUST");
    res.set("LegalCopyright", "Apache-2.0");
    res.set("OriginalFilename", "zhs-brush-gui.exe");
    res.compile()?;
    Ok(())
}
