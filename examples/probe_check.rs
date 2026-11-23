//! 诊断：等价于界面「读取检查」（只读，不写服务器）。
//! 用法：probe_check.exe <data_dir> <dohomework链接>
use zhs_brush::api::exam::HomeworkIdentity;
use zhs_brush::homework;
use zhs_brush::util;

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let dir = a.next().map(|d| util::normalize_dir(std::path::Path::new(&d))).unwrap_or_else(util::data_dir);
    let url = a.next().expect("需要 dohomework 链接");
    let id = HomeworkIdentity::from_url(&url)?;
    let mut report = |l: String| println!("{l}");
    match homework::run_all(&dir, id, false, false, &mut report) {
        Ok(()) => println!("=== 读取检查通过 ==="),
        Err(e) => println!("[x] {e}"),
    }
    Ok(())
}
