//! 图形界面专用入口：**Windows 子系统**，双击不弹黑框。
//!
//! CLI 用 `zhs-brush.exe`（控制台子系统），GUI 用 `zhs-brush-gui.exe`。
//! 两者共用 `zhs-brush` 库，数据目录规则一致。

// 只在 Windows 上生效；其它平台保持普通入口
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::path::PathBuf;

fn main() {
    let mut data_dir: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data-dir" | "-d" => data_dir = args.next().map(PathBuf::from),
            other if other.starts_with("--data-dir=") => {
                data_dir = Some(PathBuf::from(other.trim_start_matches("--data-dir=")));
            }
            _ => {}
        }
    }

    let dir = data_dir
        .map(|d| zhs_brush::util::normalize_dir(&d))
        .unwrap_or_else(zhs_brush::util::data_dir);
    std::env::set_var("ZHS_HOME", &dir);

    // 没有控制台，出错只能写日志，别让窗口一闪就没了查不到原因
    if let Err(e) = zhs_brush::gui::run(dir.clone()) {
        zhs_brush::util::write_log(&dir, "ERROR", &format!("界面启动失败: {e}"));
        // 再给个可见提示（无控制台时用系统弹窗）
        #[cfg(windows)]
        unsafe {
            show_error_box(&format!("界面启动失败：\n{e}\n\n详细日志见 {}\\logs\\app.log", dir.display()));
        }
    }
}

/// 没有控制台时，用系统消息框把错误显示出来。
#[cfg(windows)]
unsafe fn show_error_box(text: &str) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(hwnd: *mut core::ffi::c_void, text: *const u16, caption: *const u16, utype: u32) -> i32;
    }

    let wide = |s: &str| -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    };
    let msg = wide(text);
    let cap = wide("ZHS-Brush-RUST");
    // MB_OK | MB_ICONERROR = 0x10
    MessageBoxW(std::ptr::null_mut(), msg.as_ptr(), cap.as_ptr(), 0x10);
}
