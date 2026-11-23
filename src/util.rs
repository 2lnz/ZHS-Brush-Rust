//! 杂项工具：数据目录定位、HMS 时间格式、日志落盘、打开文件。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// 规范化数据目录路径。
///
/// 主要是防 Windows 命令行的两个坑：
/// 1. 批处理里 `--data-dir "%~dp0"` 的 `%~dp0` 自带结尾反斜杠，
///    拼出 `"...\fuckzhs-rust\"` 后 `\"` 被当成转义引号，目录名里会混进 `"`，
///    随后所有文件操作报 `os error 123（文件名、目录名或卷标语法不正确）`；
/// 2. 结尾多余的 `/` 或 `\`。
pub fn normalize_dir(p: &Path) -> PathBuf {
    let raw = p.to_string_lossy();
    let cleaned = raw
        .trim()
        .trim_matches('"')
        .trim_end_matches(['\\', '/'])
        .trim_matches('"')
        .to_string();
    // "C://path//." → "C://path"（启动.vbs 传的是 "%~dp0."）
    let mut cleaned = cleaned;
    while cleaned.ends_with("\\.\\") || cleaned.ends_with("\\.") {
        cleaned.truncate(cleaned.len() - 2);
        cleaned = cleaned.trim_end_matches(['\\', '/']).to_string();
    }
    let cleaned = cleaned;
    if cleaned.is_empty() {
        return PathBuf::from(".");
    }
    // 只有盘符（如 "C:"）时补回根反斜杠，否则会变成「当前目录」
    if cleaned.len() == 2 && cleaned.ends_with(':') {
        return PathBuf::from(format!("{cleaned}\\"));
    }
    PathBuf::from(cleaned)
}

/// 数据目录（放 config.json / cookies.json / qr / logs）。
///
/// 优先级：环境变量 `ZHS_HOME` → 可执行文件所在目录 → 当前工作目录。
/// 原 Python 版用 `getDir()`，这里保留同样的语义，另外给出环境变量方便开发调试。
pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("ZHS_HOME") {
        let p = normalize_dir(&PathBuf::from(dir));
        let _ = fs::create_dir_all(&p);
        return p;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            // target/debug 下调试时也允许，只要那里有 config.json 就用
            if parent.join("config.json").exists() || parent.join("cookies.json").exists() {
                return parent.to_path_buf();
            }
            // 打包后 exe 与数据同目录
            if parent.file_name().map(|n| n != "debug" && n != "release").unwrap_or(true) {
                return parent.to_path_buf();
            }
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

pub fn ensure_dir(p: &Path) -> std::io::Result<()> {
    if !p.exists() {
        fs::create_dir_all(p)?;
    }
    Ok(())
}

/// 把秒数格式化成 `H:MM:SS`，等价于 Python `str(timedelta(seconds=...))`。
pub fn hms(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as i64;
    let days = total / 86_400;
    let rest = total % 86_400;
    let h = rest / 3_600;
    let m = (rest % 3_600) / 60;
    let s = rest % 60;
    if days > 0 {
        let unit = if days == 1 { "day" } else { "days" };
        format!("{days} {unit}, {h}:{m:02}:{s:02}")
    } else {
        format!("{h}:{m:02}:{s:02}")
    }
}

/// 追加一行到 `logs/app.log`，同时带上时间戳；写失败静默忽略。
pub fn write_log(dir: &Path, level: &str, msg: &str) {
    let log_dir = dir.join("logs");
    let _ = ensure_dir(&log_dir);
    let path = log_dir.join("app.log");
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "[{}] [{}] {}", now_string(), level, msg);
    }
}

/// `YYYY-MM-DD HH:MM:SS`（不引入 chrono，手工换算）。
pub fn now_string() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, mo, d, h, mi, s) = civil_from_unix(secs);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

/// 秒级时间戳转公历（Howard Hinnant 的 civil_from_days 算法）。
pub fn civil_from_unix(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as i64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (
        y,
        m,
        d,
        (rem / 3_600) as u32,
        ((rem % 3_600) / 60) as u32,
        (rem % 60) as u32,
    )
}

/// 用系统默认程序打开一个路径（失败时只回传错误，不 panic）。
pub fn open_path(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(path)
            .spawn()?;
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(path).spawn()?;
        return Ok(());
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(path).spawn()?;
        return Ok(());
    }
    #[allow(unreachable_code)]
    Ok(())
}

/// 清洗文件名里不允许的字符。
pub fn safe_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_trailing_separator() {
        let p = PathBuf::from("C:\\a\\b\\");
        assert_eq!(normalize_dir(&p), PathBuf::from("C:\\a\\b"));
    }

    #[test]
    fn normalize_strips_stray_quote() {
        // 批处理 --data-dir "%~dp0" 会拼出 `"...\fuckzhs-rust\"`，
        // 结尾的 `\"` 被当成转义引号，目录名里混进一个引号
        let p = PathBuf::from("C:\\a\\b\\\"");
        assert_eq!(normalize_dir(&p), PathBuf::from("C:\\a\\b"));
    }

    #[test]
    fn normalize_keeps_drive_root() {
        let p = PathBuf::from("C:");
        assert_eq!(normalize_dir(&p), PathBuf::from("C:\\"));
    }

    #[test]
    fn normalize_empty_is_dot() {
        let p = PathBuf::from("");
        assert_eq!(normalize_dir(&p), PathBuf::from("."));
    }
}
